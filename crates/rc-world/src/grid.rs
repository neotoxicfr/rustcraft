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
}

/// Unique stamp in the process (never 0).
fn stamp() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

impl ChunkGrid {
    pub fn new(radius: u32) -> Self {
        let size = Self::size_for(radius);
        let s = stamp();
        ChunkGrid { size, slots: (0..size * size).map(|_| None).collect(), loaded: 0, stamps: vec![s; (size * size) as usize] }
    }

    /// Stamp of the cell of column (x, z): if it equals a previous read, nothing can
    /// have changed in that cell between the two (column present or absent, blocks, biomes,
    /// light). Serves as a key for the world's read caches.
    #[inline]
    pub fn stamp(&self, x: i32, z: i32) -> u64 {
        self.stamps[self.slot(x, z)]
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
        self.stamps[i] = stamp();
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
        let i = self.slot(x, z);
        self.stamps[i] = stamp();
        self.slots[i].as_mut().filter(|c| c.x == x && c.z == z).map(Arc::make_mut)
    }

    pub fn remove(&mut self, x: i32, z: i32) -> Option<Arc<Column>> {
        let i = self.slot(x, z);
        self.stamps[i] = stamp();
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
        self.stamps = vec![stamp(); (size * size) as usize];
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
