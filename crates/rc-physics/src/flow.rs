//! Fluids per state and current vector (`docs/specs/fluids.md` § 4.4), shared by the
//! physics (current push) and the rendering (orientation of the flow texture).

use std::sync::atomic::{AtomicBool, Ordering};

use rc_data::{VersionData, flags};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fluid {
    None,
    Water,
    Lava,
}

/// Horizontal directions in the game's order (north, south, west, east): face index, step.
const HORIZONTAL: [(u8, i32, i32); 4] = [(2, 0, -1), (3, 0, 1), (4, -1, 0), (5, 1, 0)];
const SOURCE_HEIGHT: f32 = 0.888_888_9;

/// Per-state tables for the fluids.
pub struct FluidTables {
    pub kind: Vec<Fluid>,
    /// Own height: amount / 9.
    pub height: Vec<f32>,
    pub falling: Vec<bool>,
    pub ice: Vec<bool>,
    /// Source (full amount, not falling).
    pub source: Vec<bool>,
    /// Sturdy faces (bit = direction).
    pub sturdy: Vec<u8>,
    block: Vec<u16>,
    /// Blocks of the `minecraft:blocks_fluid_flow` tag; atomic so they can be set afterwards on
    /// tables already shared between threads (tags arrive before the chunks).
    blocks_flow: Vec<AtomicBool>,
}

impl FluidTables {
    pub fn new(d: &VersionData) -> Self {
        let fluid_kind: Vec<Fluid> = d
            .fluids
            .iter()
            .map(|n| match n.as_str() {
                "minecraft:water" | "minecraft:flowing_water" => Fluid::Water,
                "minecraft:lava" | "minecraft:flowing_lava" => Fluid::Lava,
                _ => Fluid::None,
            })
            .collect();
        FluidTables {
            kind: d.states.iter().map(|s| fluid_kind.get(s.fluid as usize).copied().unwrap_or(Fluid::None)).collect(),
            height: d.states.iter().map(|s| f32::from(s.fluid_amount) / 9.0).collect(),
            falling: d.states.iter().map(|s| s.has(flags::FLUID_FALLING)).collect(),
            ice: d.states.iter().map(|s| s.has(flags::ICE)).collect(),
            source: d.states.iter().map(|s| s.has(flags::FLUID_SOURCE)).collect(),
            sturdy: d.states.iter().map(|s| s.sturdy).collect(),
            block: d.states.iter().map(|s| s.block).collect(),
            blocks_flow: (0..d.blocks.len()).map(|_| AtomicBool::new(false)).collect(),
        }
    }

    /// Minimal tables for the tests: one state per entry, no tag.
    pub fn from_parts(kind: Vec<Fluid>, height: Vec<f32>) -> Self {
        let n = kind.len();
        FluidTables {
            kind,
            height,
            falling: vec![false; n],
            ice: vec![false; n],
            source: vec![false; n],
            sturdy: vec![0; n],
            block: (0..n as u16).collect(),
            blocks_flow: (0..n).map(|_| AtomicBool::new(false)).collect(),
        }
    }

    /// Applies the `minecraft:blocks_fluid_flow` tag (network identifiers = block indices).
    pub fn set_block_tags(&self, tags: &[(String, Vec<i32>)]) {
        let list = tags.iter().find(|(n, _)| n == "minecraft:blocks_fluid_flow").map_or(&[][..], |(_, v)| v);
        self.blocks_flow.iter().for_each(|b| b.store(false, Ordering::Relaxed));
        for &b in list {
            if let Some(c) = self.blocks_flow.get(b as usize) {
                c.store(true, Ordering::Relaxed);
            }
        }
    }

    pub fn fluid(&self, s: u32) -> Fluid {
        self.kind.get(s as usize).copied().unwrap_or(Fluid::None)
    }

    /// Height of the fluid of state `s` (above it: `above`): 1 under the same fluid, otherwise
    /// own height.
    pub fn height_at(&self, s: u32, above: u32) -> f32 {
        if self.fluid(above) == self.fluid(s) { 1.0 } else { self.own_height(s) }
    }

    /// Height seen by the camera: a source under a sturdy bottom face fills the block.
    pub fn camera_height(&self, s: u32, above: u32) -> f32 {
        let sturdy_down = self.sturdy.get(above as usize).is_some_and(|m| m & 1 == 1);
        if self.source.get(s as usize).copied().unwrap_or(false) && sturdy_down {
            1.0
        } else {
            self.height_at(s, above)
        }
    }

    fn own_height(&self, s: u32) -> f32 {
        self.height.get(s as usize).copied().unwrap_or(0.0)
    }

    fn blocks_flow(&self, s: u32) -> bool {
        self.block.get(s as usize).is_some_and(|&b| self.blocks_flow[b as usize].load(Ordering::Relaxed))
    }

    /// Normalised current of fluid `f` at (x, y, z), `st` giving the state of a position.
    /// Distances in `f32`, sums in `f64`, like the game.
    pub fn flow<S: Fn(i32, i32, i32) -> u32>(&self, st: &S, f: Fluid, x: i32, y: i32, z: i32) -> [f64; 3] {
        let me = st(x, y, z);
        let hp = self.own_height(me);
        let same_or_empty = |s: u32| matches!(self.fluid(s), Fluid::None) || self.fluid(s) == f;
        let (mut fx, mut fz) = (0.0f64, 0.0f64);
        for (_, dx, dz) in HORIZONTAL {
            let v = st(x + dx, y, z + dz);
            if !same_or_empty(v) {
                continue;
            }
            let hv = if self.fluid(v) == f { self.own_height(v) } else { 0.0 };
            let mut d = 0.0f32;
            if hv == 0.0 {
                if !self.blocks_flow(v) {
                    let below = st(x + dx, y - 1, z + dz);
                    if same_or_empty(below) {
                        let hb = if self.fluid(below) == f { self.own_height(below) } else { 0.0 };
                        if hb > 0.0 {
                            d = hp - (hb - SOURCE_HEIGHT);
                        }
                    }
                }
            } else if hv > 0.0 {
                d = hp - hv;
            }
            if d != 0.0 {
                fx += f64::from(dx as f32 * d);
                fz += f64::from(dz as f32 * d);
            }
        }
        let mut v = [fx, 0.0, fz];
        if self.falling.get(me as usize).copied().unwrap_or(false) {
            let solid = |sx: i32, sy: i32, sz: i32, dir: u8| {
                let s = st(sx, sy, sz);
                self.fluid(s) != f
                    && !self.ice.get(s as usize).copied().unwrap_or(false)
                    && self.sturdy.get(s as usize).is_some_and(|m| m >> dir & 1 == 1)
            };
            if HORIZONTAL
                .iter()
                .any(|&(dir, dx, dz)| solid(x + dx, y, z + dz, dir) || solid(x + dx, y + 1, z + dz, dir))
            {
                v = normalize(v);
                v[1] -= 6.0;
            }
        }
        normalize(v)
    }
}

fn normalize(v: [f64; 3]) -> [f64; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l < f64::from(1.0e-5f32) { [0.0; 3] } else { [v[0] / l, v[1] / l, v[2] / l] }
}
