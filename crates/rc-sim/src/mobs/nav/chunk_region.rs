//! Chunks around the searching mob (`RUSTCRAFT_OPT_PATH_REGION`): a search reads thousands of
//! blocks in a few chunks; each read otherwise hashes its chunk position into the server's chunk
//! map. The 7 × 7 chunks around the mob (like the navigation region of the game) are looked up
//! once, when first read; a block further away goes through the map.

use crate::chunk::Chunk;
use crate::server::Server;

const SIDE: i32 = 7;

pub(super) struct ChunkRegion<'a> {
    /// Chunk at the centre of the region.
    centre: (i32, i32),
    /// `None`: not looked up yet; `Some(None)`: no chunk there (blocks read as state 0).
    slots: [Option<Option<&'a Chunk>>; (SIDE * SIDE) as usize],
}

impl<'a> ChunkRegion<'a> {
    pub(super) fn around(block: [i32; 3]) -> ChunkRegion<'a> {
        ChunkRegion { centre: (block[0] >> 4, block[2] >> 4), slots: [None; (SIDE * SIDE) as usize] }
    }

    pub(super) fn chunk(&mut self, s: &'a Server, cx: i32, cz: i32) -> Option<&'a Chunk> {
        let lookup = || s.chunks.get(&(cx, cz)).and_then(|c| c.as_deref());
        let (dx, dz) = (cx - self.centre.0 + SIDE / 2, cz - self.centre.1 + SIDE / 2);
        if !(0..SIDE).contains(&dx) || !(0..SIDE).contains(&dz) {
            return lookup();
        }
        *self.slots[(dx * SIDE + dz) as usize].get_or_insert_with(lookup)
    }

    /// State at `p`, like the server's block read: 0 outside the loaded chunks and the height.
    pub(super) fn state(&mut self, s: &'a Server, p: [i32; 3]) -> u32 {
        let Some(c) = self.chunk(s, p[0] >> 4, p[2] >> 4) else { return 0 };
        let ly = p[1] - s.dim.height.min_y;
        let Some(section) = usize::try_from(ly >> 4).ok().and_then(|i| c.sections.get(i)) else { return 0 };
        section.blocks.get(rc_world::block_index((p[0] & 15) as usize, (ly & 15) as usize, (p[2] & 15) as usize))
    }
}
