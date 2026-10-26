//! World storage on the client side. Independent of the protocol and rendering: reused by
//! singleplayer (`rc-sim`). Section format: `docs/specs/chunks.md`.

mod container;
mod grid;
pub mod heightmap;

pub use container::{
    ColumnIter, Container, GetMode, is_canonical, longs_for, opt as container_opt, remap_first_seen, values_per_long,
};
pub use grid::ChunkGrid;

pub const SECTION_BLOCKS: usize = 4096;
pub const SECTION_BIOMES: usize = 64;
pub const LIGHT_BYTES: usize = 2048;

/// Index of a block in its section (local coordinates 0..16).
#[inline]
pub const fn block_index(x: usize, y: usize, z: usize) -> usize {
    (y << 8) | (z << 4) | x
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    pub non_empty: i16,
    pub blocks: Container,
    pub biomes: Container,
}

impl Section {
    pub fn empty(air: u32, biome: u32) -> Self {
        Section { non_empty: 0, blocks: Container::Single(air), biomes: Container::Single(biome) }
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.non_empty == 0
    }

    /// Applies `(index, state)` changes, repacks as compactly as possible and recounts
    /// the non-empty blocks. `direct_bits`: width of the global state palette.
    pub fn set_blocks(&mut self, changes: &[(u16, u32)], direct_bits: u8, is_air: impl Fn(u32) -> bool) {
        for &(i, state) in changes {
            let i = i as usize;
            if i >= SECTION_BLOCKS {
                continue;
            }
            let old = self.blocks.get(i);
            self.non_empty += i16::from(!is_air(state)) - i16::from(!is_air(old));
            self.blocks.set(i, state, 4, 8, direct_bits);
        }
    }

    /// Old method (decompress, modify, repack): reference of the tests and benches.
    #[doc(hidden)]
    pub fn set_blocks_repack(&mut self, changes: &[(u16, u32)], direct_bits: u8, is_air: impl Fn(u32) -> bool) {
        let mut values = [0u32; SECTION_BLOCKS];
        self.blocks.unpack_into(&mut values);
        for &(i, state) in changes {
            if let Some(v) = values.get_mut(i as usize) {
                *v = state;
            }
        }
        self.non_empty = values.iter().filter(|&&v| !is_air(v)).count() as i16;
        self.blocks = Container::pack(&values, 4, 8, direct_bits);
    }
}

#[cfg(test)]
mod heightmap_tests {
    use super::*;

    /// Chunk received without ocean floor: map created at zero and never computed; placements and breaks
    /// tracked on the four maps (stone = all, leaves = without "no leaves", 3 = air).
    #[test]
    fn client_heightmaps_follow_block_changes() {
        const H: usize = 32;
        let rules = [0u8, 15, 1 | 2 | 4, 0]; // air, stone, leaves, cave air
        let mut col = Column {
            x: 0,
            z: 0,
            sections: vec![Section::empty(0, 0), Section::empty(0, 0)].into_boxed_slice(),
            sky_light: Vec::new().into_boxed_slice(),
            block_light: Vec::new().into_boxed_slice(),
            heightmaps: vec![(4, vec![0; heightmap::longs(H)].into_boxed_slice())],
        };
        col.complete_heightmaps(&rules, H);
        let mut ids: Vec<i32> = col.heightmaps.iter().map(|m| m.0).collect();
        ids.sort_unstable();
        assert_eq!(ids, [1, 3, 4, 5]);
        let at = |c: &Column, id: i32| {
            heightmap::get(&c.heightmaps.iter().find(|m| m.0 == id).unwrap().1, 3 * 16 + 2, heightmap::bits(H))
        };
        let put = |c: &mut Column, y: usize, s: u32| {
            c.sections[y >> 4].set_blocks(&[(block_index(2, y & 15, 3) as u16, s)], 15, |v| v == 0 || v == 3);
            c.update_heightmaps(2, y, 3, s, &rules, H);
        };
        put(&mut col, 5, 1);
        put(&mut col, 20, 2);
        assert_eq!([1, 3, 4, 5].map(|id| at(&col, id)), [21, 21, 21, 6]);
        // Leaves broken: descent to the stone.
        put(&mut col, 20, 0);
        assert_eq!([1, 3, 4, 5].map(|id| at(&col, id)), [6, 6, 6, 6]);
        // Placement well below the top: nothing moves.
        put(&mut col, 2, 3);
        put(&mut col, 5, 3);
        assert_eq!([1, 3, 4, 5].map(|id| at(&col, id)), [0, 0, 0, 0]);
        // Tags not yet received: no update.
        col.update_heightmaps(2, 9, 3, 1, &[], H);
        assert_eq!(at(&col, 1), 0);
        // Chunk sent again while it is loaded: maps absent from the packet keep their
        // tracked value, the received map (4) takes that of the packet.
        put(&mut col, 5, 1);
        let mut again = Column { heightmaps: vec![(4, vec![0; heightmap::longs(H)].into_boxed_slice())], ..col.clone() };
        again.inherit_heightmaps(&col);
        again.complete_heightmaps(&rules, H);
        assert_eq!([1, 3, 4, 5].map(|id| at(&again, id)), [6, 6, 0, 6]);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// In-place write and repacking give the same blocks and the same count, on
    /// sections of all shapes (single, 4- and 8-bit palettes, overflow to direct).
    #[test]
    fn set_blocks_matches_repack() {
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = |n: u64| {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed % n
        };
        for distinct in [1u32, 3, 16, 17, 255, 256, 300, 5000] {
            let values: Vec<u32> = (0..4096u32).map(|i| (i * 7919) % distinct).collect();
            let mut s =
                Section { non_empty: 0, blocks: Container::pack(&values, 4, 8, 15), biomes: Container::Single(0) };
            s.non_empty = values.iter().filter(|&&v| v != 0).count() as i16;
            let mut r = s.clone();
            for _ in 0..400 {
                let changes: Vec<(u16, u32)> =
                    (0..1 + next(4)).map(|_| (next(4100) as u16, next(u64::from(distinct) + 400) as u32)).collect();
                s.set_blocks(&changes, 15, |v| v == 0);
                r.set_blocks_repack(&changes, 15, |v| v == 0);
                assert_eq!(s.non_empty, r.non_empty);
                for i in 0..SECTION_BLOCKS {
                    assert_eq!(s.blocks.get(i), r.blocks.get(i), "index {i}, {distinct} values");
                }
            }
        }
    }
}

/// Edges of a section touched by changed cells (per-section remeshing): bit set if a
/// changed cell is on the face x = 0, x = 15, y = 0, y = 15, z = 0 or z = 15 of its section;
/// [`edge::ANY`] always set as soon as a cell changed.
pub mod edge {
    pub const X0: u8 = 1;
    pub const X15: u8 = 2;
    pub const Y0: u8 = 4;
    pub const Y15: u8 = 8;
    pub const Z0: u8 = 16;
    pub const Z15: u8 = 32;
    pub const ANY: u8 = 64;
    /// All edges: the whole section changed.
    pub const ALL: u8 = 127;

    /// Edge bits of a local coordinate (0-15) on the axis whose bit for edge 0 is `lo`.
    pub const fn bits(v: usize, lo: u8) -> u8 {
        match v {
            0 => lo,
            15 => lo << 1,
            _ => 0,
        }
    }

    /// Bits of a cell of section index `i` (x + 16 z + 256 y), [`ANY`] included.
    pub const fn of_index(i: usize) -> u8 {
        ANY | bits(i & 15, X0) | bits(i >> 8, Y0) | bits((i >> 4) & 15, Z0)
    }

    /// Like [`bits`], at two cells from the edge: the ambient occlusion of a face reads the block state
    /// two cells along its normal (opacity of the corners behind the neighbouring cell).
    pub const fn near_bits(v: usize, lo: u8) -> u8 {
        match v {
            0 | 1 => lo,
            14 | 15 => lo << 1,
            _ => 0,
        }
    }

    /// Edges touched by a block state change at `i`: those within two cells at most (the
    /// states are read up to two blocks, the light up to one: see [`of_index`]).
    pub const fn near_of_index(i: usize) -> u8 {
        ANY | near_bits(i & 15, X0) | near_bits(i >> 8, Y0) | near_bits((i >> 4) & 15, Z0)
    }
}

/// Light sheet of a section: 4096 half-bytes; `None` = no data received.
pub type LightLayer = Option<std::sync::Arc<[u8; LIGHT_BYTES]>>;

/// Fully null sheet, shared by all "empty" light sections.
pub fn zero_layer() -> std::sync::Arc<[u8; LIGHT_BYTES]> {
    static ZERO: std::sync::OnceLock<std::sync::Arc<[u8; LIGHT_BYTES]>> = std::sync::OnceLock::new();
    ZERO.get_or_init(|| std::sync::Arc::new([0; LIGHT_BYTES])).clone()
}

#[derive(Debug, Clone, PartialEq)]
pub struct Column {
    pub x: i32,
    pub z: i32,
    /// Sections from bottom to top.
    pub sections: Box<[Section]>,
    /// Light: `sections + 2` positions (one below the world, one above).
    pub sky_light: Box<[LightLayer]>,
    pub block_light: Box<[LightLayer]>,
    pub heightmaps: Vec<(i32, Box<[i64]>)>,
}

impl Column {
    /// Block state at local coordinates (`y` relative to the world bottom).
    #[inline]
    pub fn block(&self, x: usize, y: usize, z: usize) -> Option<u32> {
        let s = self.sections.get(y >> 4)?;
        Some(s.blocks.get(block_index(x, y & 15, z)))
    }

    /// Maps of a received chunk, completed like the game's client: the four live maps
    /// absent from the packet are created at zero (world bottom, never computed, only placements
    /// make them rise); a received map of the wrong size is recomputed from the blocks.
    /// `rules`: `heightmap::HM_*` bits per state (empty: tags not yet received).
    pub fn complete_heightmaps(&mut self, rules: &[u8], height: usize) {
        let (bits, want) = (heightmap::bits(height), heightmap::longs(height));
        for (id, rule) in heightmap::LIVE_HEIGHTMAPS {
            match self.heightmaps.iter().position(|(k, _)| *k == id) {
                None => self.heightmaps.push((id, vec![0; want].into_boxed_slice())),
                Some(k) if self.heightmaps[k].1.len() != want && !rules.is_empty() => {
                    let mut l = vec![0i64; want];
                    for i in 0..256 {
                        let (x, z) = (i & 15, i >> 4);
                        let top = (0..height)
                            .rev()
                            .find(|&y| self.block(x, y, z).is_some_and(|s| rules.get(s as usize).is_some_and(|r| r & rule != 0)));
                        heightmap::set(&mut l, i, bits, top.map_or(0, |y| y as i32 + 1));
                    }
                    self.heightmaps[k].1 = l.into_boxed_slice();
                }
                Some(_) => {}
            }
        }
    }

    /// Chunk received in place of a still-loaded chunk: the game's client reuses the old one
    /// and overwrites only the received maps; the others keep their value (to be called before
    /// `complete_heightmaps`).
    pub fn inherit_heightmaps(&mut self, old: &Column) {
        for (id, longs) in &old.heightmaps {
            if !self.heightmaps.iter().any(|(k, _)| k == id) {
                self.heightmaps.push((*id, longs.clone()));
            }
        }
    }

    /// Local block (x, `y` relative to the world bottom, z) changed to `state`, section already
    /// updated: live maps tracked by the common rule (`heightmap::update`).
    pub fn update_heightmaps(&mut self, x: usize, y: usize, z: usize, state: u32, rules: &[u8], height: usize) {
        if rules.is_empty() {
            return;
        }
        let (bits, want) = (heightmap::bits(height), heightmap::longs(height));
        let rule_of = |s: u32| rules.get(s as usize).copied().unwrap_or(0);
        let r = rule_of(state);
        for (id, rule) in heightmap::LIVE_HEIGHTMAPS {
            let Some(k) = self.heightmaps.iter().position(|(m, l)| *m == id && l.len() == want) else { continue };
            let mut longs = std::mem::take(&mut self.heightmaps[k].1);
            heightmap::update(&mut longs, bits, z * 16 + x, y as i32, 0, r & rule != 0, |yy| {
                self.block(x, yy as usize, z).is_some_and(|s| rule_of(s) & rule != 0)
            });
            self.heightmaps[k].1 = longs;
        }
    }

    pub fn heap_bytes(&self) -> usize {
        let zero = zero_layer();
        let light =
            |l: &[LightLayer]| l.iter().flatten().filter(|a| !std::sync::Arc::ptr_eq(a, &zero)).count() * LIGHT_BYTES;
        self.sections.iter().map(|s| s.blocks.heap_bytes() + s.biomes.heap_bytes()).sum::<usize>()
            + light(&self.sky_light)
            + light(&self.block_light)
            + self.heightmaps.iter().map(|h| h.1.len() * 8).sum::<usize>()
    }
}
