//! Primitives of the coded pieces (`generation-decoration.md` § 8.6–8.14, conventions): oriented
//! local frame, placement filtered by the cut-out, full or random boxes, half-sphere, columns,
//! reoriented chests.

use crate::decor_blocks::{Blocks, Dir, State};
use crate::decor_region::{BeData, Hm, Region};
use crate::decor_rng::DecoRng;
use crate::template::{BBox, Mirror, Rot, StateTransforms};

/// Piece being placed in a chunk.
pub struct P<'a, 'r> {
    pub r: &'a mut Region<'r>,
    pub b: &'r Blocks,
    pub tr: &'a StateTransforms,
    pub zone: BBox,
    pub bbox: BBox,
    pub orient: Option<Dir>,
    mirror: Mirror,
    rot: Rot,
    /// Blocks never replaced by `place` (mineshaft).
    pub keep: Vec<u16>,
}

/// Blocks whose placement is marked for post-processing (shapes of the neighbours).
const SHAPE_CHECK: [&str; 12] = [
    "nether_brick_fence",
    "torch",
    "wall_torch",
    "oak_fence",
    "spruce_fence",
    "dark_oak_fence",
    "pale_oak_fence",
    "acacia_fence",
    "birch_fence",
    "jungle_fence",
    "ladder",
    "iron_bars",
];

impl<'a, 'r> P<'a, 'r> {
    pub fn new(r: &'a mut Region<'r>, tr: &'a StateTransforms, zone: BBox, bbox: BBox, orient: Option<Dir>) -> Self {
        let (mirror, rot) = match orient {
            Some(Dir::South) => (Mirror::LeftRight, Rot::None),
            Some(Dir::West) => (Mirror::LeftRight, Rot::Cw90),
            Some(Dir::East) => (Mirror::None, Rot::Cw90),
            _ => (Mirror::None, Rot::None),
        };
        let b = r.b;
        P { r, b, tr, zone, bbox, orient, mirror, rot, keep: Vec::new() }
    }

    pub fn world(&self, x: i32, y: i32, z: i32) -> [i32; 3] {
        let bb = &self.bbox;
        match self.orient {
            None => [x, y, z],
            Some(Dir::North) => [bb.min[0] + x, y + bb.min[1], bb.max[2] - z],
            Some(Dir::South) => [bb.min[0] + x, y + bb.min[1], bb.min[2] + z],
            Some(Dir::West) => [bb.max[0] - z, y + bb.min[1], bb.min[2] + x],
            Some(Dir::East) => [bb.min[0] + z, y + bb.min[1], bb.min[2] + x],
            Some(_) => [x, y + bb.min[1], z],
        }
    }

    /// Local block (air outside the cut-out).
    pub fn get(&self, x: i32, y: i32, z: i32) -> State {
        let p = self.world(x, y, z);
        if self.zone.contains(p) { self.r.get_p(p) } else { self.b.default_state(self.b.k.air) }
    }

    pub fn is_air(&self, x: i32, y: i32, z: i32) -> bool {
        self.b.is_air(self.get(x, y, z))
    }

    /// Placement of a local block (piece orientation applied to the state).
    pub fn place(&mut self, s: State, x: i32, y: i32, z: i32) {
        let p = self.world(x, y, z);
        if !self.zone.contains(p) {
            return;
        }
        if !self.keep.is_empty() {
            let cur = self.get(x, y, z);
            if self.keep.contains(&self.b.block(cur)) {
                return;
            }
        }
        let s = self.tr.apply(s, self.mirror, self.rot);
        self.r.set_p(p, s, 2);
        let now = self.r.get_p(p);
        if self.b.fluid(now) != 0 {
            self.r.schedule_fluid_tick(p, now);
        }
        let name = self.b.block_name(self.b.block(s));
        let n = name.strip_prefix("minecraft:").unwrap_or(name);
        if SHAPE_CHECK.contains(&n) {
            self.r.mark_post_process(p);
        }
    }

    /// Interior: (x, y + 1, z) within the cut-out and under the generation ocean floor.
    pub fn is_interior(&self, x: i32, y: i32, z: i32) -> bool {
        let p = self.world(x, y + 1, z);
        self.zone.contains(p) && p[1] < self.r.height_at(Hm::OceanFloorWg, p[0], p[2])
    }

    #[allow(clippy::too_many_arguments)]
    pub fn fill(
        &mut self,
        x0: i32,
        y0: i32,
        z0: i32,
        x1: i32,
        y1: i32,
        z1: i32,
        edge: State,
        inner: State,
        skip_air: bool,
    ) {
        for y in y0..=y1 {
            for x in x0..=x1 {
                for z in z0..=z1 {
                    if skip_air && self.is_air(x, y, z) {
                        continue;
                    }
                    let border = y == y0 || y == y1 || x == x0 || x == x1 || z == z0 || z == z1;
                    self.place(if border { edge } else { inner }, x, y, z);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn maybe_fill(
        &mut self,
        rng: &mut DecoRng,
        p: f32,
        x0: i32,
        y0: i32,
        z0: i32,
        x1: i32,
        y1: i32,
        z1: i32,
        edge: State,
        inner: State,
        skip_air: bool,
        inside: bool,
    ) {
        for y in y0..=y1 {
            for x in x0..=x1 {
                for z in z0..=z1 {
                    if rng.next_float() > p {
                        continue;
                    }
                    if skip_air && self.is_air(x, y, z) {
                        continue;
                    }
                    if inside && !self.is_interior(x, y, z) {
                        continue;
                    }
                    let border = y == y0 || y == y1 || x == x0 || x == x1 || z == z0 || z == z1;
                    self.place(if border { edge } else { inner }, x, y, z);
                }
            }
        }
    }

    pub fn maybe_block(&mut self, rng: &mut DecoRng, p: f32, x: i32, y: i32, z: i32, s: State) {
        if rng.next_float() < p {
            self.place(s, x, y, z);
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn upper_half_sphere(
        &mut self,
        x0: i32,
        y0: i32,
        z0: i32,
        x1: i32,
        y1: i32,
        z1: i32,
        s: State,
        skip_air: bool,
    ) {
        let (dx, dy, dz) = ((x1 - x0 + 1) as f32, (y1 - y0 + 1) as f32, (z1 - z0 + 1) as f32);
        let cx = x0 as f32 + dx / 2.0;
        let cz = z0 as f32 + dz / 2.0;
        for y in y0..=y1 {
            let ny = (y - y0) as f32 / dy;
            for x in x0..=x1 {
                let nx = (x as f32 - cx) / (dx * 0.5);
                for z in z0..=z1 {
                    let nz = (z as f32 - cz) / (dz * 0.5);
                    if skip_air && self.is_air(x, y, z) {
                        continue;
                    }
                    if nx * nx + ny * ny + nz * nz <= 1.05 {
                        self.place(s, x, y, z);
                    }
                }
            }
        }
    }

    /// Replaceable by a structure: air, liquid, glow lichen, seagrass.
    pub fn replaceable_by_structures(&self, s: State) -> bool {
        let b = self.b;
        if b.is_air(s) || b.has_flag(s, rc_data::flags::LIQUID) {
            return true;
        }
        let n = b.block_name(b.block(s));
        matches!(n.strip_prefix("minecraft:").unwrap_or(n), "glow_lichen" | "seagrass" | "tall_seagrass")
    }

    /// Column downwards (general primitive): as long as replaceable and above the bottom + 1.
    pub fn fill_column_down(&mut self, s: State, x: i32, y: i32, z: i32) {
        let mut p = self.world(x, y, z);
        if !self.zone.contains(p) {
            return;
        }
        while self.replaceable_by_structures(self.r.get_p(p)) && p[1] > self.r.min_y + 1 {
            self.r.set_p(p, s, 2);
            p[1] -= 1;
        }
    }

    /// Chest (world position): placed if within the cut-out and not already a chest; seed drawn
    /// only if it is placed.
    pub fn chest(&mut self, rng: &mut DecoRng, p: [i32; 3], table: &'static str, state: Option<State>) -> bool {
        let b = self.b;
        if !self.zone.contains(p) || b.is(self.r.get_p(p), b.k.chest) {
            return false;
        }
        let s = state.unwrap_or_else(|| crate::feature_basic::reorient_chest(self.r, p, b.default_state(b.k.chest)));
        self.r.set_p(p, s, 2);
        let seed = rng.next_long();
        self.r.note_block_entity(p, BeData::Loot { table, seed });
        true
    }

    pub fn state(&self, name: &str) -> State {
        st(self.b, name)
    }

    /// Full box of a single state (no air skip).
    #[allow(clippy::too_many_arguments)]
    pub fn bx(&mut self, x0: i32, y0: i32, z0: i32, x1: i32, y1: i32, z1: i32, s: State) {
        self.fill(x0, y0, z0, x1, y1, z1, s, s, false);
    }
}

/// Default state of a named block (name without namespace accepted).
pub fn st(b: &Blocks, name: &str) -> State {
    b.block_id(name).map(|i| b.default_state(i)).unwrap_or(0)
}

/// State property set by name (ignored if the block does not carry it).
pub trait SetProp {
    fn set(self, b: &Blocks, k: &str, v: &str) -> State;
}

impl SetProp for State {
    fn set(self, b: &Blocks, k: &str, v: &str) -> State {
        match b.prop(k) {
            Some(pr) if b.has(self, pr) => b.with_str(self, pr, v),
            _ => self,
        }
    }
}
