//! Circular grid of the loaded columns: O(1) access by `(x mod N, z mod N)`, without a
//! hash table. N = 2 × radius + 3 (margin of one column on each side, like the
//! official cache), recomputed when the server changes the view distance.

use std::sync::Arc;

use crate::Column;

pub struct ChunkGrid {
    size: i32,
    slots: Vec<Option<Arc<Column>>>,
    loaded: usize,
    /// Stamp per cell, changed at each write access (insertion, removal, `get_mut`):
    /// values unique in the process, never reused by another grid.
    stamps: Vec<u64>,
    /// Stamp of the last write access to any cell, or of the last resize ([`ChunkGrid::epoch`]).
    epoch: u64,
}

/// Unique stamp in the process (never 0).
fn stamp() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

/// Who asks for write access to a column ([`ChunkGrid::get_mut_by`]): splits the column copies
/// made because another thread (a meshing task) still holds it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ColumnWriter {
    /// Block changes of a section.
    Blocks,
    /// Light layers received from the server.
    LightUpdate,
    /// Biome containers received from the server.
    Biomes,
    /// Layers written by the local light engine.
    Relight,
    /// Any other writer.
    Other,
}

impl ColumnWriter {
    pub const ALL: [ColumnWriter; 5] = [
        ColumnWriter::Blocks,
        ColumnWriter::LightUpdate,
        ColumnWriter::Biomes,
        ColumnWriter::Relight,
        ColumnWriter::Other,
    ];

    pub fn name(self) -> &'static str {
        match self {
            ColumnWriter::Blocks => "blocks",
            ColumnWriter::LightUpdate => "light_update",
            ColumnWriter::Biomes => "biomes",
            ColumnWriter::Relight => "relight",
            ColumnWriter::Other => "other",
        }
    }
}

/// Column copies per writer since startup ([`ColumnWriter`] order).
pub type ColumnCopies = [u64; ColumnWriter::ALL.len()];

static COPIES: [std::sync::atomic::AtomicU64; ColumnWriter::ALL.len()] =
    [const { std::sync::atomic::AtomicU64::new(0) }; ColumnWriter::ALL.len()];

/// Column copies made by write accesses since startup, per writer (perf log).
pub fn column_copies() -> ColumnCopies {
    std::array::from_fn(|k| COPIES[k].load(std::sync::atomic::Ordering::Relaxed))
}

impl ChunkGrid {
    pub fn new(radius: u32) -> Self {
        let size = Self::size_for(radius);
        let s = stamp();
        ChunkGrid {
            size,
            slots: (0..size * size).map(|_| None).collect(),
            loaded: 0,
            stamps: vec![s; (size * size) as usize],
            epoch: s,
        }
    }

    /// Stamp of the cell of column (x, z): if it equals a previous read, nothing can
    /// have changed in that cell between the two (column present or absent, blocks, biomes,
    /// light). Serves as a key for the world's read caches.
    #[inline]
    pub fn stamp(&self, x: i32, z: i32) -> u64 {
        self.stamps[self.slot(x, z)]
    }

    /// Write epoch of the whole grid: if it equals a previous read, no cell can have changed
    /// between the two (same rule as [`ChunkGrid::stamp`], for any column; never 0). One load for
    /// the read caches whose entries may land in any column, such as the particles' light.
    #[inline]
    pub fn epoch(&self) -> u64 {
        self.epoch
    }

    /// New stamp for cell `i`, which is also the grid's epoch.
    fn touch(&mut self, i: usize) {
        let s = stamp();
        self.stamps[i] = s;
        self.epoch = s;
    }

    fn size_for(radius: u32) -> i32 {
        (radius.clamp(2, 128) as i32) * 2 + 3
    }

    #[inline]
    fn slot(&self, x: i32, z: i32) -> usize {
        (x.rem_euclid(self.size) * self.size + z.rem_euclid(self.size)) as usize
    }

    pub fn len(&self) -> usize {
        self.loaded
    }

    pub fn is_empty(&self) -> bool {
        self.loaded == 0
    }

    /// Inserts a column; returns the one it replaces in the cell, if there was one.
    pub fn insert(&mut self, column: Column) -> Option<Arc<Column>> {
        let i = self.slot(column.x, column.z);
        self.touch(i);
        let old = self.slots[i].replace(Arc::new(column));
        if old.is_none() {
            self.loaded += 1;
        }
        old
    }

    #[inline]
    pub fn get(&self, x: i32, z: i32) -> Option<&Column> {
        self.slots[self.slot(x, z)].as_deref().filter(|c| c.x == x && c.z == z)
    }

    /// Shared column (copy-free read from another thread).
    #[inline]
    pub fn get_arc(&self, x: i32, z: i32) -> Option<Arc<Column>> {
        self.slots[self.slot(x, z)].as_ref().filter(|c| c.x == x && c.z == z).cloned()
    }

    /// Write access: copies the column only if another thread is still reading it.
    #[inline]
    pub fn get_mut(&mut self, x: i32, z: i32) -> Option<&mut Column> {
        self.get_mut_by(x, z, ColumnWriter::Other)
    }

    /// [`get_mut`](Self::get_mut), the copy (column still held elsewhere) counted for `writer`.
    #[inline]
    pub fn get_mut_by(&mut self, x: i32, z: i32, writer: ColumnWriter) -> Option<&mut Column> {
        let i = self.slot(x, z);
        self.touch(i);
        let column = self.slots[i].as_mut().filter(|c| c.x == x && c.z == z)?;
        // Same test as the copy inside `make_mut` (one load): only the copies are counted.
        if Arc::strong_count(column) > 1 {
            COPIES[writer as usize].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
        Some(Arc::make_mut(column))
    }

    pub fn remove(&mut self, x: i32, z: i32) -> Option<Arc<Column>> {
        let i = self.slot(x, z);
        self.touch(i);
        if self.slots[i].as_ref().is_some_and(|c| c.x == x && c.z == z) {
            self.loaded -= 1;
            self.slots[i].take()
        } else {
            None
        }
    }

    /// Changes the view distance while keeping the columns that still fit.
    pub fn set_radius(&mut self, radius: u32) {
        let size = Self::size_for(radius);
        if size == self.size {
            return;
        }
        let old = std::mem::take(&mut self.slots);
        self.size = size;
        self.slots = (0..size * size).map(|_| None).collect();
        self.epoch = stamp();
        self.stamps = vec![self.epoch; (size * size) as usize];
        self.loaded = 0;
        for c in old.into_iter().flatten() {
            let i = self.slot(c.x, c.z);
            if self.slots[i].is_none() {
                self.slots[i] = Some(c);
                self.loaded += 1;
            }
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = &Column> {
        self.slots.iter().filter_map(|s| s.as_deref())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn col(x: i32, z: i32) -> Column {
        Column {
            x,
            z,
            sections: Box::new([]),
            sky_light: Box::new([]),
            block_light: Box::new([]),
            heightmaps: Vec::new(),
        }
    }

    #[test]
    fn insert_get_remove_with_negative_coords() {
        let mut g = ChunkGrid::new(4);
        g.insert(col(-3, 7));
        g.insert(col(0, 0));
        assert_eq!(g.len(), 2);
        assert!(g.get(-3, 7).is_some());
        // Same cell, other column: no false positive.
        assert!(g.get(-3 + 11, 7).is_none());
        assert!(g.remove(-3 + 11, 7).is_none());
        assert!(g.remove(-3, 7).is_some());
        assert_eq!(g.len(), 1);
    }

    /// A write access copies a column only while another holder keeps it, and counts that copy
    /// for its writer; alone, no copy and nothing counted (writers no other test of this crate uses).
    #[test]
    fn copies_counted_per_writer() {
        let (biomes, relight) = (ColumnWriter::Biomes as usize, ColumnWriter::Relight as usize);
        let mut g = ChunkGrid::new(4);
        g.insert(col(1, 2));
        let before = column_copies();
        assert!(g.get_mut_by(1, 2, ColumnWriter::Relight).is_some());
        let held = g.get_arc(1, 2).expect("column present");
        g.get_mut_by(1, 2, ColumnWriter::Biomes).expect("column present").sky_light = Box::new([None]);
        assert!(held.sky_light.is_empty(), "the holder keeps its own column");
        assert_eq!(g.get(1, 2).map(|c| c.sky_light.len()), Some(1), "the grid holds the written copy");
        drop(held);
        assert!(g.get_mut_by(1, 2, ColumnWriter::Relight).is_some());
        assert!(g.get_mut_by(9, 9, ColumnWriter::Biomes).is_none(), "absent column");
        let after = column_copies();
        assert_eq!((after[biomes] - before[biomes], after[relight] - before[relight]), (1, 0));
    }

    /// The epoch moves with every write access to any cell and with a resize, never with a read;
    /// a new grid never starts on the epoch of another.
    #[test]
    fn epoch_moves_with_every_write_only() {
        fn moved(g: &ChunkGrid, last: &mut u64, what: &str) {
            assert_ne!(g.epoch(), *last, "{what} must move the epoch");
            *last = g.epoch();
        }
        let mut g = ChunkGrid::new(4);
        let mut last = g.epoch();
        g.insert(col(1, 2));
        moved(&g, &mut last, "insertion");
        let _ = (g.get(1, 2), g.get_arc(1, 2), g.stamp(1, 2), g.iter().count(), g.len());
        assert_eq!(g.epoch(), last, "reads leave the epoch");
        g.get_mut_by(1, 2, ColumnWriter::LightUpdate).expect("column present").sky_light = Box::new([None]);
        moved(&g, &mut last, "light write");
        g.get_mut(7, 7);
        moved(&g, &mut last, "write access to an empty cell");
        g.remove(1, 2);
        moved(&g, &mut last, "removal");
        g.set_radius(9);
        moved(&g, &mut last, "resize");
        assert_ne!(ChunkGrid::new(4).epoch(), g.epoch(), "another grid");
    }

    #[test]
    fn resize_keeps_columns() {
        let mut g = ChunkGrid::new(2);
        for x in -2..=2 {
            for z in -2..=2 {
                g.insert(col(x, z));
            }
        }
        g.set_radius(10);
        assert_eq!(g.len(), 25);
        assert!(g.get(-2, 2).is_some());
    }
}
