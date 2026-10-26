//! Fluid meshing (water, lava) after `docs/specs/fluids.md`: corner heights
//! averaged with the neighbours, faces removed against the same fluid or an occluding block,
//! flow texture oriented along the current read from the received states.
//!
//! Face removal: game rule by face shapes (`cull::CullTables::fluid_hidden`,
//! `RUSTCRAFT_OPT_SHAPECULL`); switch off, the old "one box covers the
//! side" approximation. The current comes from `rc_physics::FluidTables::flow`, shared with the physics.

use rc_data::{VersionData, flags};

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
}

struct Ctx<'a, B: Blocks> {
    b: &'a B,
    t: &'a FluidTables,
    covers: &'a [u8],
    /// Game rule by face shapes; `None`: old rule (`covers`).
    shapes: Option<&'a crate::cull::CullTables>,
}

impl<B: Blocks> Ctx<'_, B> {
    fn st(&self, x: i32, y: i32, z: i32) -> u32 {
        self.b.state(x, y, z).unwrap_or(0)
    }

    fn fluid(&self, s: u32) -> Fluid {
        self.t.flow.fluid(s)
    }

    /// Height seen by the rendering (spec § 0.4).
    fn h(&self, f: Fluid, x: i32, y: i32, z: i32) -> f32 {
        let s = self.st(x, y, z);
        if self.fluid(s) == f {
            if self.fluid(self.st(x, y + 1, z)) == f { 1.0 } else { self.t.flow.height[s as usize] }
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

    fn corner(&self, f: Fluid, here: f32, s1: f32, s2: f32, diag: (i32, i32, i32)) -> f32 {
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
            let hd = self.h(f, diag.0, diag.1, diag.2);
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

    /// Current vector (spec § 4.4), x and z components normalised.
    fn flow(&self, f: Fluid, x: i32, y: i32, z: i32) -> (f64, f64) {
        let v = self.t.flow.flow(&|x, y, z| self.st(x, y, z), f, x, y, z);
        (v[0], v[2])
    }
}

pub(crate) fn color(tint: u32, factor: f32) -> u32 {
    let ch = |s: u32| (((tint >> s) & 0xff) as f32 * factor).clamp(0.0, 255.0) as u32;
    (ch(16) << 16) | (ch(8) << 8) | ch(0) | 0xff00_0000
}

/// Sine table (65,536 entries), like the game.
fn table_sin(a: f32) -> f32 {
    let i = ((a * 10_430.378) as i64 & 0xffff) as f64;
    (i * std::f64::consts::TAU / 65_536.0).sin() as f32
}

fn table_cos(a: f32) -> f32 {
    let i = (((a * 10_430.378) as i64 + 16_384) & 0xffff) as f64;
    (i * std::f64::consts::TAU / 65_536.0).sin() as f32
}

/// Faces of the fluid of block `(x, y, z)` (coordinates relative to the column).
#[allow(clippy::too_many_arguments)]
pub fn mesh_fluid<B: Blocks>(
    b: &B,
    t: &FluidTables,
    covers: &[u8],
    shapes: Option<&crate::cull::CullTables>,
    tex: &rc_assets::FluidTextures,
    job: &MeshJob,
    tint_of: &Tint,
    (x, y, z): (i32, i32, i32),
    lum: &mut dyn FnMut(i32, i32, i32) -> u16,
    out: &mut Vec<FluidQuad>,
) {
    // Light of a face: maximum per component of the position and of the one above (§ 6).
    let mut face_light = |qy: i32| {
        let (a, b) = (lum(x, qy, z), lum(x, qy + 1, z));
        (a & 0xff).max(b & 0xff) | (a & 0xff00).max(b & 0xff00)
    };
    let c = Ctx { b, t, covers, shapes };
    let me = c.st(x, y, z);
    let f = c.fluid(me);
    if f == Fluid::None {
        return;
    }
    let same = |dx, dy, dz| c.fluid(c.st(x + dx, y + dy, z + dz)) == f;
    let n = |dx, dy, dz| c.st(x + dx, y + dy, z + dz);
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
    let here = c.h(f, x, y, z);
    let (mut ne, mut nw, mut se, mut sw) = if here >= 1.0 {
        (1.0, 1.0, 1.0, 1.0)
    } else {
        let (hn, hs, he, hw) = (c.h(f, x, y, z - 1), c.h(f, x, y, z + 1), c.h(f, x + 1, y, z), c.h(f, x - 1, y, z));
        (
            c.corner(f, here, he, hn, (x + 1, y, z - 1)),
            c.corner(f, here, hw, hn, (x - 1, y, z - 1)),
            c.corner(f, here, he, hs, (x + 1, y, z + 1)),
            c.corner(f, here, hw, hs, (x - 1, y, z + 1)),
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
        let (fl_x, fl_z) = c.flow(f, x, y, z);
        let (texture, uv) = if fl_x == 0.0 && fl_z == 0.0 {
            (still, [[0.0, 0.0], [0.0, 1.0], [1.0, 1.0], [1.0, 0.0]])
        } else {
            let a = (fl_z.atan2(fl_x) - std::f64::consts::FRAC_PI_2) as f32;
            let (s, k) = (table_sin(a) * 0.25, table_cos(a) * 0.25);
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
