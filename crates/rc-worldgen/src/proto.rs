//! Terrain proto-chunk (spec generation-terrain § 1, § 6): blocks of a 16 × 16 column over the
//! whole world height, generation height maps (`WORLD_SURFACE_WG`,
//! `OCEAN_FLOOR_WG`) kept at each placement, positions marked for fluid post-processing.

use crate::palette::{AIR, BlockId, BlockTable};

/// Height map: "first free y" per column (index `x + 16·z`).
pub type Heightmap = [i32; 256];

#[derive(Debug, Clone)]
pub struct ProtoTerrain {
    pub cx: i32,
    pub cz: i32,
    pub min_y: i32,
    pub height: i32,
    /// Index `((y − min_y)·16 + z)·16 + x`.
    pub blocks: Vec<BlockId>,
    /// Non-air blocks per section (bottom to top).
    pub section_counts: Vec<u16>,
    pub world_surface_wg: Heightmap,
    pub ocean_floor_wg: Heightmap,
    /// Post-processing marks: `(section index << 16) | (x | y << 4 | z << 8)`, in
    /// placement order (duplicates possible, like the game).
    pub post_processing: Vec<u32>,
}

#[inline]
fn col(x: i32, z: i32) -> usize {
    (x + z * 16) as usize
}

impl ProtoTerrain {
    pub fn new(cx: i32, cz: i32, min_y: i32, height: i32) -> Self {
        ProtoTerrain {
            cx,
            cz,
            min_y,
            height,
            blocks: vec![AIR; (256 * height) as usize],
            section_counts: vec![0; (height >> 4) as usize],
            world_surface_wg: [min_y; 256],
            ocean_floor_wg: [min_y; 256],
            post_processing: Vec::new(),
        }
    }

    #[inline]
    pub fn max_y(&self) -> i32 {
        self.min_y + self.height - 1
    }

    #[inline]
    pub fn min_x(&self) -> i32 {
        self.cx * 16
    }

    #[inline]
    pub fn min_z(&self) -> i32 {
        self.cz * 16
    }

    #[inline]
    fn index(&self, x: i32, y: i32, z: i32) -> usize {
        ((((y - self.min_y) << 4) + z) * 16 + x) as usize
    }

    /// State in local coordinates x, z (0..16) and world y; air outside the height.
    #[inline]
    pub fn get(&self, x: i32, y: i32, z: i32) -> BlockId {
        if y < self.min_y || y > self.max_y() { AIR } else { self.blocks[self.index(x, y, z)] }
    }

    /// Raw placement into the section (without height maps), like the filling.
    #[inline]
    pub fn set_raw(&mut self, t: &BlockTable, x: i32, y: i32, z: i32, s: BlockId) -> BlockId {
        let i = self.index(x, y, z);
        let old = self.blocks[i];
        self.blocks[i] = s;
        let sec = ((y - self.min_y) >> 4) as usize;
        match (t.is_air(old), t.is_air(s)) {
            (true, false) => self.section_counts[sec] += 1,
            (false, true) => self.section_counts[sec] -= 1,
            _ => {}
        }
        old
    }

    #[inline]
    pub fn section_empty(&self, y: i32) -> bool {
        self.section_counts[((y - self.min_y) >> 4) as usize] == 0
    }

    /// Placement in a proto-chunk (§ 1): ignored outside the height or if air is placed in an
    /// empty section; otherwise generation maps updated.
    pub fn set(&mut self, t: &BlockTable, x: i32, y: i32, z: i32, s: BlockId) {
        if y < self.min_y || y > self.max_y() {
            return;
        }
        if s == AIR && self.section_empty(y) {
            return;
        }
        self.set_raw(t, x, y, z, s);
        self.update_heightmaps(t, x, y, z, s);
    }

    /// Update of the two generation maps for a placement (game order: seabed then surface).
    pub fn update_heightmaps(&mut self, t: &BlockTable, x: i32, y: i32, z: i32, s: BlockId) {
        let of = self.ocean_floor_wg[col(x, z)];
        if let Some(v) = self.updated(x, y, z, of, t.blocks_motion(s), |b| t.blocks_motion(b)) {
            self.ocean_floor_wg[col(x, z)] = v;
        }
        let ws = self.world_surface_wg[col(x, z)];
        if let Some(v) = self.updated(x, y, z, ws, !t.is_air(s), |b| !t.is_air(b)) {
            self.world_surface_wg[col(x, z)] = v;
        }
    }

    fn updated(&self, x: i32, y: i32, z: i32, first: i32, counts: bool, pred: impl Fn(BlockId) -> bool) -> Option<i32> {
        if y <= first - 2 {
            return None;
        }
        if counts {
            return (y >= first).then_some(y + 1);
        }
        if first - 1 == y {
            for yy in (self.min_y..y).rev() {
                if pred(self.blocks[self.index(x, yy, z)]) {
                    return Some(yy + 1);
                }
            }
            return Some(self.min_y);
        }
        None
    }

    /// `WORLD_SURFACE_WG` − 1: y of the highest non-air block (like the game's height read).
    #[inline]
    pub fn surface_height(&self, x: i32, z: i32) -> i32 {
        self.world_surface_wg[col(x, z)] - 1
    }

    /// Marks a position (local coordinates, world y) for post-processing.
    pub fn mark_post_processing(&mut self, x: i32, y: i32, z: i32) {
        let sec = ((y - self.min_y) >> 4) as u32;
        let packed = (x & 15) as u32 | (((y & 15) as u32) << 4) | (((z & 15) as u32) << 8);
        self.post_processing.push((sec << 16) | packed);
    }

    /// Index of the highest non-empty section (`None` if everything is empty).
    pub fn highest_filled_section(&self) -> Option<usize> {
        self.section_counts.iter().rposition(|c| *c != 0)
    }

    /// Final maps (§ 6, priming at the end of `terrain`): `WORLD_SURFACE`, `OCEAN_FLOOR`,
    /// `MOTION_BLOCKING`, `MOTION_BLOCKING_NO_LEAVES` (without leaves in terrain: equal to the previous one).
    pub fn final_heightmaps(&self, t: &BlockTable) -> [Heightmap; 4] {
        let mut out = [[self.min_y; 256]; 4];
        let top = self.highest_filled_section().map_or(self.min_y, |s| self.min_y + (s as i32) * 16 + 16);
        for x in 0..16 {
            for z in 0..16 {
                let mut open = [true; 4];
                for y in (self.min_y..top).rev() {
                    let b = self.blocks[self.index(x, y, z)];
                    if b == AIR {
                        continue;
                    }
                    let preds = [
                        !t.is_air(b),
                        t.blocks_motion(b),
                        t.blocks_motion(b) || t.has_fluid(b),
                        t.blocks_motion(b) || t.has_fluid(b),
                    ];
                    for k in 0..4 {
                        if open[k] && preds[k] {
                            out[k][col(x, z)] = y + 1;
                            open[k] = false;
                        }
                    }
                    if !open.iter().any(|o| *o) {
                        break;
                    }
                }
            }
        }
        out
    }
}
