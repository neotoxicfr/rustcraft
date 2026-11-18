//! Fluid meshing (water, lava) after `docs/specs/fluids.md`: corner heights
//! averaged with the neighbours, faces removed against the same fluid or an occluding block,
//! flow texture oriented along the current read from the received states.
//!
//! Face removal: game rule by face shapes (`cull::CullTables::fluid_hidden`,
//! `RUSTCRAFT_OPT_SHAPECULL`); switch off, the old "one box covers the
//! side" approximation. The current comes from `rc_physics::FluidTables::flow`, shared with the physics.
//!
//! Shortcuts ([`FluidPaths`], same faces): the states around a block and the light of its level
//! read once; the block mesher passes over a fluid block among its own state.

use rc_data::{VersionData, flags, java_math};

use crate::blocks::Tint;
use crate::wire::MeshJob;

pub use rc_physics::Fluid;

/// Per-state tables for the fluids: those shared with the physics, plus rendering.
pub struct FluidTables {
    pub flow: rc_physics::FluidTables,
    pub legacy_solid: Vec<bool>,
    pub overlay: Vec<bool>,
}

impl FluidTables {
    pub fn new(d: &VersionData) -> Self {
        FluidTables {
            flow: rc_physics::FluidTables::new(d),
            legacy_solid: d.states.iter().map(|s| s.has(flags::LEGACY_SOLID)).collect(),
            overlay: d.states.iter().map(|s| s.has(flags::FLUID_OVERLAY)).collect(),
        }
    }
}

/// Shortcuts of the fluid meshing. Each returns the same faces as without it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FluidPaths {
    /// The states around a fluid block (its layer and the one above) read once from the border
    /// grid, instead of one lookup per use (`RUSTCRAFT_OPT_FLUIDNEIGHBORMEMO`).
    pub neighbor_memo: bool,
    /// A fluid block whose six neighbours hold its own state is passed over without the fluid
    /// mesher: it has no face (`RUSTCRAFT_OPT_FLUID_SKIP_BURIED`).
    pub skip_buried: bool,
    /// The light shared by the top and the four sides of a block read once for them all
    /// (`RUSTCRAFT_OPT_FLUID_FACE_LIGHT_ONCE`).
    pub face_light_once: bool,
}

impl FluidPaths {
    pub const ALL: FluidPaths = FluidPaths { neighbor_memo: true, skip_buried: true, face_light_once: true };
    /// Every fluid block through the fluid mesher, one world read and one light read per use.
    pub const OLD: FluidPaths = FluidPaths { neighbor_memo: false, skip_buried: false, face_light_once: false };

    /// Switches read once (all on by default, `=0` turns one off).
    pub fn from_env() -> Self {
        FluidPaths {
            neighbor_memo: rc_data::opt_switch!("RUSTCRAFT_OPT_FLUIDNEIGHBORMEMO"),
            skip_buried: rc_data::opt_switch!("RUSTCRAFT_OPT_FLUID_SKIP_BURIED"),
            face_light_once: rc_data::opt_switch!("RUSTCRAFT_OPT_FLUID_FACE_LIGHT_ONCE"),
        }
    }
}

/// Fluid face: four corners (x, y, z relative to the column; u, v in the texture),
/// texture, RGBA colour (tint × shading).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FluidQuad {
    pub corners: [[f32; 5]; 4],
    pub texture: u32,
    /// RGBA8 colour of each corner (tint, shading, occlusion).
    pub colors: [u32; 4],
    /// Packed light of each corner (block × 16 | sky × 16 << 8).
    pub light: [u16; 4],
}

// Directions (official order): down 0, up 1, north 2, south 3, west 4, east 5.
const DOWN: u8 = 0;
const UP: u8 = 1;
const NORTH: u8 = 2;
const SOUTH: u8 = 3;
const WEST: u8 = 4;
const EAST: u8 = 5;
const OPPOSITE: [u8; 6] = [UP, DOWN, SOUTH, NORTH, EAST, WEST];
const SOURCE_HEIGHT: f32 = 8.0 / 9.0;

/// Access to the states around the column (supplied by the block mesher).
pub trait Blocks {
    fn state(&self, x: i32, y: i32, z: i32) -> Option<u32>;

    /// States of the 3 × 3 blocks of layer `y` centred on (`x`, `z`): rows from north to south,
    /// each from west to east, 0 (air) where the world has no block. The block mesher answers
    /// from one slice of its border grid instead of nine position lookups.
    fn layer_around(&self, x: i32, y: i32, z: i32) -> [u32; 9] {
        layer_by_state(self, x, y, z)
    }
}

/// [`Blocks::layer_around`] read one position at a time.
pub fn layer_by_state<B: Blocks + ?Sized>(blocks: &B, x: i32, y: i32, z: i32) -> [u32; 9] {
    std::array::from_fn(|cell| {
        let (dx, dz) = (cell as i32 % 3 - 1, cell as i32 / 3 - 1);
        blocks.state(x + dx, y, z + dz).unwrap_or(0)
    })
}

/// States around the fluid block being meshed, by offset from it (one block at most on each axis).
trait Neighbors {
    fn state(&self, dx: i32, dy: i32, dz: i32) -> u32;
}

/// Every read goes to the world (old path, `RUSTCRAFT_OPT_FLUIDNEIGHBORMEMO=0`).
struct WorldReads<'a, B: Blocks> {
    blocks: &'a B,
    block: (i32, i32, i32),
}

impl<B: Blocks> Neighbors for WorldReads<'_, B> {
    fn state(&self, dx: i32, dy: i32, dz: i32) -> u32 {
        let (x, y, z) = self.block;
        self.blocks.state(x + dx, y + dy, z + dz).unwrap_or(0)
    }
}

/// The layer of the block and the one above, read once (`FluidPaths::neighbor_memo`): the face
/// filter, the heights, the corners, the current and the back face of the top read these 18 states
/// about thirty times for a block of a still surface, and the block mesher hands each layer over
/// from one slice of its grid. The layer below is read at its centre, and by the current only
/// next to a column without fluid: those reads stay world reads.
struct NeighborMemo<'a, B: Blocks> {
    level: [u32; 9],
    above: [u32; 9],
    world: WorldReads<'a, B>,
}

impl<'a, B: Blocks> NeighborMemo<'a, B> {
    fn read(world: WorldReads<'a, B>) -> Self {
        let (x, y, z) = world.block;
        NeighborMemo {
            level: world.blocks.layer_around(x, y, z),
            above: world.blocks.layer_around(x, y + 1, z),
            world,
        }
    }
}

/// Cell of a [`Blocks::layer_around`] layer for the offset (`dx`, `dz`) from its centre.
fn layer_cell(dx: i32, dz: i32) -> usize {
    ((dz + 1) * 3 + dx + 1) as usize
}

impl<B: Blocks> Neighbors for NeighborMemo<'_, B> {
    fn state(&self, dx: i32, dy: i32, dz: i32) -> u32 {
        match dy {
            0 => self.level[layer_cell(dx, dz)],
            1 => self.above[layer_cell(dx, dz)],
            _ => self.world.state(dx, dy, dz),
        }
    }
}

struct Ctx<'a, N: Neighbors> {
    near: &'a N,
    t: &'a FluidTables,
    covers: &'a [u8],
    /// Game rule by face shapes; `None`: old rule (`covers`).
    shapes: Option<&'a crate::cull::CullTables>,
    paths: FluidPaths,
}

impl<N: Neighbors> Ctx<'_, N> {
    fn fluid(&self, s: u32) -> Fluid {
        self.t.flow.fluid(s)
    }

    /// Height seen by the rendering (spec § 0.4) in the column at (`dx`, `dz`) from the block.
    fn h(&self, f: Fluid, dx: i32, dz: i32) -> f32 {
        let s = self.near.state(dx, 0, dz);
        if self.fluid(s) == f {
            if self.fluid(self.near.state(dx, 1, dz)) == f { 1.0 } else { self.t.flow.height[s as usize] }
        } else if self.t.legacy_solid.get(s as usize).copied().unwrap_or(false) {
            -1.0
        } else {
            0.0
        }
    }

    /// Face `dir` of the fluid, of height `h`, is hidden by the neighbouring state `s` (spec § 3.4).
    fn masked(&self, s: u32, dir: u8, h: f32) -> bool {
        if let Some(ct) = self.shapes {
            return ct.fluid_hidden(s, dir as usize, h);
        }
        let m = self.covers.get(s as usize).copied().unwrap_or(0);
        if m >> OPPOSITE[dir as usize] & 1 == 0 {
            return false;
        }
        // Surface under an occluding block: visible as long as it does not reach 1.
        !(dir == UP && h != 1.0)
    }

    fn corner(&self, f: Fluid, here: f32, s1: f32, s2: f32, diagonal: (i32, i32)) -> f32 {
        if s1 >= 1.0 || s2 >= 1.0 {
            return 1.0;
        }
        let (mut sum, mut w) = (0.0f32, 0.0f32);
        let mut add = |h: f32| {
            if h >= 0.8 {
                sum += h * 10.0;
                w += 10.0;
            } else if h >= 0.0 {
                sum += h;
                w += 1.0;
            }
        };
        if s1 > 0.0 || s2 > 0.0 {
            let hd = self.h(f, diagonal.0, diagonal.1);
            if hd >= 1.0 {
                return 1.0;
            }
            add(hd);
        }
        add(here);
        add(s1);
        add(s2);
        sum / w
    }

    /// Current vector of the block (spec § 4.4), x and z components normalised.
    fn flow(&self, f: Fluid) -> (f64, f64) {
        // The current only depends on the states around the block: asked at the origin, the
        // positions it reads are the offsets.
        let v = self.t.flow.flow(&|dx, dy, dz| self.near.state(dx, dy, dz), f, 0, 0, 0);
        (v[0], v[2])
    }
}

pub(crate) fn color(tint: u32, factor: f32) -> u32 {
    let ch = |s: u32| (((tint >> s) & 0xff) as f32 * factor).clamp(0.0, 255.0) as u32;
    (ch(16) << 16) | (ch(8) << 8) | ch(0) | 0xff00_0000
}

/// Faces of the fluid of block `(x, y, z)` (coordinates relative to the column). `paths`: the
/// shortcuts taken (`MeshJob::fluid`), same faces whichever they are.
#[allow(clippy::too_many_arguments)]
pub fn mesh_fluid<B: Blocks>(
    b: &B,
    t: &FluidTables,
    covers: &[u8],
    shapes: Option<&crate::cull::CullTables>,
    tex: &rc_assets::FluidTextures,
    job: &MeshJob,
    tint_of: &Tint,
    block: (i32, i32, i32),
    paths: FluidPaths,
    lum: &mut dyn FnMut(i32, i32, i32) -> u16,
    out: &mut Vec<FluidQuad>,
) {
    let world = WorldReads { blocks: b, block };
    if paths.neighbor_memo {
        let near = NeighborMemo::read(world);
        fluid_faces(&Ctx { near: &near, t, covers, shapes, paths }, tex, job, tint_of, block, lum, out);
    } else {
        fluid_faces(&Ctx { near: &world, t, covers, shapes, paths }, tex, job, tint_of, block, lum, out);
    }
}

/// Faces of the fluid of block `(x, y, z)`, its neighbours read through `c.near`.
fn fluid_faces<N: Neighbors>(
    c: &Ctx<N>,
    tex: &rc_assets::FluidTextures,
    job: &MeshJob,
    tint_of: &Tint,
    (x, y, z): (i32, i32, i32),
    lum: &mut dyn FnMut(i32, i32, i32) -> u16,
    out: &mut Vec<FluidQuad>,
) {
    // Light of a face: maximum per component of the position and of the one above (§ 6). The
    // top and the four sides all ask for the level of the block: kept after the first of them
    // (`FluidPaths::face_light_once`).
    let mut level_light = None;
    let mut face_light = |qy: i32| {
        if qy == y
            && let Some(light) = level_light
        {
            return light;
        }
        let (a, b) = (lum(x, qy, z), lum(x, qy + 1, z));
        let light = (a & 0xff).max(b & 0xff) | (a & 0xff00).max(b & 0xff00);
        if qy == y && c.paths.face_light_once {
            level_light = Some(light);
        }
        light
    };
    let (t, covers, shapes) = (c.t, c.covers, c.shapes);
    let n = |dx, dy, dz| c.near.state(dx, dy, dz);
    let me = n(0, 0, 0);
    let f = c.fluid(me);
    if f == Fluid::None {
        return;
    }
    let same = |dx, dy, dz| c.fluid(n(dx, dy, dz)) == f;
    // First filter (spec § 3.1).
    // The block itself (waterlogged) hides the face if its shape covers that whole side.
    let self_masks = |dir: u8| match shapes {
        Some(ct) => ct.fluid_self_hidden(me, dir as usize),
        None => c.covers.get(me as usize).is_some_and(|m| m >> dir & 1 == 1),
    };
    let top = !same(0, 1, 0);
    let bottom = !same(0, -1, 0) && !self_masks(DOWN) && !c.masked(n(0, -1, 0), DOWN, SOURCE_HEIGHT);
    let sides = [(NORTH, 0, -1), (SOUTH, 0, 1), (WEST, -1, 0), (EAST, 1, 0)]
        .map(|(d, dx, dz)| (d, dx, dz, !same(dx, 0, dz) && !self_masks(d)));
    if !top && !bottom && sides.iter().all(|s| !s.3) {
        return;
    }

    // Corner heights (spec § 2).
    let here = c.h(f, 0, 0);
    let (mut ne, mut nw, mut se, mut sw) = if here >= 1.0 {
        (1.0, 1.0, 1.0, 1.0)
    } else {
        let (hn, hs, he, hw) = (c.h(f, 0, -1), c.h(f, 0, 1), c.h(f, 1, 0), c.h(f, -1, 0));
        (
            c.corner(f, here, he, hn, (1, -1)),
            c.corner(f, here, hw, hn, (-1, -1)),
            c.corner(f, here, he, hs, (1, 1)),
            c.corner(f, here, hw, hs, (-1, 1)),
        )
    };

    // Tint computed at the first emitted face (buried water blocks have none).
    let tint_cell = std::cell::OnceCell::new();
    let tint = || {
        *tint_cell.get_or_init(|| match f {
            Fluid::Water => tint_of.get(3, x, y, z) | 0xff00_0000,
            _ => 0xffff_ffff,
        })
    };
    let (still, flow) = match f {
        Fluid::Water => (tex.water_still, tex.water_flow),
        _ => (tex.lava_still, tex.lava_flow),
    };
    let (fx, fy, fz) = (x as f32, y as f32, z as f32);
    let bottom_y = if bottom { 0.001 } else { 0.0 };

    // Top (spec § 4.1).
    let min_corner = ne.min(nw).min(se).min(sw);
    if top && !c.masked(n(0, 1, 0), UP, min_corner) {
        (ne, nw, se, sw) = (ne - 0.001, nw - 0.001, se - 0.001, sw - 0.001);
        let (fl_x, fl_z) = c.flow(f);
        let (texture, uv) = if fl_x == 0.0 && fl_z == 0.0 {
            (still, [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]])
        } else {
            let a = (fl_z.atan2(fl_x) - std::f64::consts::FRAC_PI_2) as f32;
            // The game widens the float angle before its table product (an index in double).
            let (s, k) = (java_math::sin(f64::from(a)) * 0.25, java_math::cos(f64::from(a)) * 0.25);
            (
                flow,
                [
                    [0.5 + (-k - s), 0.5 + (-k + s)],
                    [0.5 + (-k + s), 0.5 + (k + s)],
                    [0.5 + (k + s), 0.5 + (k - s)],
                    [0.5 + (k - s), 0.5 + (-k - s)],
                ],
            )
        };
        let p = [[fx, fy + nw, fz], [fx, fy + sw, fz + 1.0], [fx + 1.0, fy + se, fz + 1.0], [fx + 1.0, fy + ne, fz]];
        let quad = FluidQuad {
            corners: [0, 1, 2, 3].map(|i| [p[i][0], p[i][1], p[i][2], uv[i][0], uv[i][1]]),
            texture,
            colors: [color(tint(), job.shade[UP as usize]); 4],
            light: [face_light(y); 4],
        };
        // Back face, visible from under the water, if the layer above is not entirely closed.
        let open_above = (-1..=1).any(|dz| {
            (-1..=1).any(|dx| {
                let s = n(dx, 1, dz);
                c.fluid(s) != f && covers.get(s as usize).copied().unwrap_or(0) != 0x3f
            })
        });
        push_sided(out, quad, open_above, job.opts.water_double);
    }

    // Bottom (spec § 4.2).
    if bottom {
        let y0 = fy + bottom_y;
        out.push(FluidQuad {
            corners: [
                [fx, y0, fz, 0.0, 0.0],
                [fx + 1.0, y0, fz, 1.0, 0.0],
                [fx + 1.0, y0, fz + 1.0, 1.0, 1.0],
                [fx, y0, fz + 1.0, 0.0, 1.0],
            ],
            texture: still,
            colors: [color(tint(), job.shade[DOWN as usize]); 4],
            light: [face_light(y - 1); 4],
        });
    }

    // Sides (spec § 4.3).
    const E: f32 = 0.001;
    for (d, dx, dz, candidate) in sides {
        if !candidate {
            continue;
        }
        let ((h0, h1), (s0, s1)) = match d {
            NORTH => ((nw, ne), ([fx, fz + E], [fx + 1.0, fz + E])),
            SOUTH => ((se, sw), ([fx + 1.0, fz + 1.0 - E], [fx, fz + 1.0 - E])),
            WEST => ((sw, nw), ([fx + E, fz + 1.0], [fx + E, fz])),
            _ => ((ne, se), ([fx + 1.0 - E, fz], [fx + 1.0 - E, fz + 1.0])),
        };
        let neighbor = n(dx, 0, dz);
        if c.masked(neighbor, d, h0.max(h1)) {
            continue;
        }
        let overlay = f == Fluid::Water && t.overlay.get(neighbor as usize).copied().unwrap_or(false);
        let texture = if overlay { tex.water_overlay } else { flow };
        let (v0, v1) = ((1.0 - h0) * 0.5, (1.0 - h1) * 0.5);
        let side_shade = if d == NORTH || d == SOUTH { job.shade[NORTH as usize] } else { job.shade[WEST as usize] };
        let quad = FluidQuad {
            corners: [
                [s0[0], fy + h0, s0[1], 0.0, v0],
                [s1[0], fy + h1, s1[1], 0.5, v1],
                [s1[0], fy + bottom_y, s1[1], 0.5, 0.5],
                [s0[0], fy + bottom_y, s0[1], 0.0, 0.5],
            ],
            texture,
            colors: [color(tint(), job.shade[UP as usize] * side_shade); 4],
            light: [face_light(y); 4],
        };
        push_sided(out, quad, !overlay, job.opts.water_double);
    }
}

/// Bit of `FluidQuad::texture`: double-sided quad, its back face (corners 0, 3, 2, 1) is
/// drawn by the shader (`MeshOptions::water_double`) instead of a second quad.
pub const DOUBLE_FACE: u32 = 1 << 31;

/// Back face of a quad: same corners in the order 0, 3, 2, 1.
pub fn back_face(q: &FluidQuad) -> FluidQuad {
    let c = q.corners;
    FluidQuad { corners: [c[0], c[3], c[2], c[1]], ..*q }
}

/// Pushes a quad, and its back face if `both`: second quad, or double-face bit.
fn push_sided(out: &mut Vec<FluidQuad>, quad: FluidQuad, both: bool, double: bool) {
    match (both, double) {
        (true, true) => out.push(FluidQuad { texture: quad.texture | DOUBLE_FACE, ..quad }),
        (true, false) => out.extend([quad, back_face(&quad)]),
        (false, _) => out.push(quad),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A different state at every position, no block under y = 0.
    struct NumberedWorld;

    impl Blocks for NumberedWorld {
        fn state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
            (y >= 0).then(|| ((x + 50) * 10_000 + y * 100 + z + 50) as u32)
        }
    }

    /// The memo answers every offset of the 3 × 3 × 3 neighbourhood as a world read does: a cell
    /// taken from the wrong column or layer (a shifted diagonal) returns another number.
    #[test]
    fn neighbor_memo_answers_like_world_reads() {
        for block in [(3, 0, 5), (0, 7, 0), (-4, 1, 15)] {
            let world = WorldReads { blocks: &NumberedWorld, block };
            let memo = NeighborMemo::read(WorldReads { blocks: &NumberedWorld, block });
            for dy in -1..=1 {
                for dz in -1..=1 {
                    for dx in -1..=1 {
                        assert_eq!(memo.state(dx, dy, dz), world.state(dx, dy, dz), "{block:?} + ({dx}, {dy}, {dz})");
                    }
                }
            }
        }
    }
}
