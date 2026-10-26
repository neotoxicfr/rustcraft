//! Local player physics, faithful to the official game (`docs/specs/movement.md`).
//! Independent of rendering and network: the world is seen through the [`World`] trait.
//!
//! Covered: inputs and "square" correction, sprint (double tap, conditions), sneaking
//! and caution at the edge, jump and delay, ground / air (friction, acceleration, gravity,
//! drag), creative flight (double tap, ascent, descent), per-axis collisions with
//! step-up, supporting block, climbable blocks, speed and jump factors,
//! water and lava (detection, swimming, movement, exit).
//! Not yet: bounces (slime, beds), slowing blocks (cobwebs…), current push,
//! push between entities, glider. Tracked effects: jump boost, levitation, slow
//! falling, dolphin's grace, blindness; postures (crawling) and exit from suffocating blocks.

pub mod body;
pub mod border;
pub mod collide;
mod flow;
pub mod mounts;
pub mod piston;
pub mod ray;

pub use border::WorldBorder;
pub use flow::{Fluid, FluidTables};

use hashbrown::HashMap;

use rc_data::{VersionData, inside};

pub use collide::is_free;

/// Axis-aligned box (`f64`, like the game).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl Aabb {
    pub fn offset(&self, x: f64, y: f64, z: f64) -> Aabb {
        Aabb {
            min: [self.min[0] + x, self.min[1] + y, self.min[2] + z],
            max: [self.max[0] + x, self.max[1] + y, self.max[2] + z],
        }
    }

    /// Extent towards a movement (the side matching the sign of each component).
    pub fn expand_towards(&self, d: [f64; 3]) -> Aabb {
        let mut b = *self;
        for ((lo, hi), v) in b.min.iter_mut().zip(b.max.iter_mut()).zip(d) {
            if v < 0.0 {
                *lo += v;
            } else {
                *hi += v;
            }
        }
        b
    }

    pub fn deflate(&self, e: f64) -> Aabb {
        Aabb { min: self.min.map(|v| v + e), max: self.max.map(|v| v - e) }
    }

    pub fn intersects(&self, o: &Aabb) -> bool {
        (0..3).all(|a| self.min[a] < o.max[a] && self.max[a] > o.min[a])
    }
}

/// View of the world for the physics.
pub trait World {
    /// State of the block; `None` if its chunk is not loaded.
    fn state(&self, x: i32, y: i32, z: i32) -> Option<u32>;
    /// Local collision boxes (0..1) of this block into `out`; returns true if the shape
    /// overflows the unit cube (fences, walls…).
    fn collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool;
    /// World border (none by default).
    fn border(&self) -> Option<&WorldBorder> {
        None
    }
    /// Boxes of the "solid" entities for the one that moves (boats, shulkers, happy ghast…)
    /// that touch `area` (inflated by 1e-7), added to `out`; none by default.
    fn entity_colliders(&self, _area: &Aabb, _out: &mut Vec<Aabb>) {}
    /// Cell on two edges of the searched zone: only a piston moving block counts there;
    /// its shape goes into `out` (returns true if there is one). None by default.
    fn moving_collision(&self, _x: i32, _y: i32, _z: i32, _out: &mut Vec<Aabb>) -> bool {
        false
    }
    /// Bottom of the dimension's world (−64 by default).
    fn min_y(&self) -> i32 {
        -64
    }
}

/// The world as seen by the local player's collision searches: scaffolding and powder
/// snow have a shape that depends on it (feet height, Shift, fall, boots); the rest
/// is read as is.
struct CtxWorld<'a> {
    w: &'a dyn World,
    t: &'a Tables,
    /// Bottom of the player's box.
    bottom: f64,
    /// Shift held: the player goes down.
    descending: bool,
    /// Fall distance.
    fall: f64,
    /// Leather boots on the feet.
    boots: bool,
}

/// Unstable scaffolding: 2/16 slab at the foot of the block.
const SCAFFOLDING_BOTTOM: Aabb = Aabb { min: [0.0; 3], max: [1.0, 0.125, 1.0] };

impl CtxWorld<'_> {
    /// Feet above the top `top` of a shape (within 1e-5, in `f32` like the game).
    fn above(&self, top: f64) -> bool {
        self.bottom > top - f64::from(1.0e-5f32)
    }
}

impl World for CtxWorld<'_> {
    fn state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        self.w.state(x, y, z)
    }

    fn collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
        let Some(s) = self.w.state(x, y, z) else { return false };
        let k = self.t.special.of(s);
        let fy = f64::from(y);
        match k & 0x0F {
            // Platform (extracted shape) if one is above without going down; otherwise, for an
            // unstable scaffolding in the air, the bottom slab if one is above its base.
            SP_SCAFFOLDING => {
                if self.above(fy + 1.0) && !self.descending {
                    self.t.collision(s, out);
                } else if k & SP_FLAG != 0 && k & SP_BOTTOM != 0 && self.above(fy) {
                    out.push(SCAFFOLDING_BOTTOM);
                }
                false
            }
            // Powder snow: cushions a fall of more than 2.5 blocks (0.9 shape); carries a
            // walker in leather boots above that does not go down; otherwise it is passed through.
            SP_POWDER_SNOW => {
                if self.fall > 2.5 {
                    out.push(Aabb { min: [0.0; 3], max: [1.0, f64::from(0.9f32), 1.0] });
                } else if self.boots && self.above(fy + 1.0) && !self.descending {
                    out.push(Aabb { min: [0.0; 3], max: [1.0; 3] });
                }
                false
            }
            _ => self.w.collision(x, y, z, out),
        }
    }

    fn border(&self) -> Option<&WorldBorder> {
        self.w.border()
    }

    fn entity_colliders(&self, area: &Aabb, out: &mut Vec<Aabb>) {
        self.w.entity_colliders(area, out);
    }

    fn moving_collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
        self.w.moving_collision(x, y, z, out)
    }

    fn min_y(&self) -> i32 {
        self.w.min_y()
    }
}

/// Look direction: tabulated sines, `f32` components.
pub fn view_vector(pitch: f32, yaw: f32) -> [f64; 3] {
    let (xr, yr) = (pitch * DEG, -yaw * DEG);
    let (yc, ys, xc, xs) = (cos_t(yr), sin_t(yr), cos_t(xr), sin_t(xr));
    [f64::from(ys * xc), f64::from(-xs), f64::from(yc * xc)]
}

/// Slide along a honey block at `c`: speed after the slide, `None` if the entity does not
/// slide (on the ground, above the block, not fast enough downwards, or not against a side
/// face). Common to the physics engine's entities and the integrated server's projectiles.
pub fn honey_slide(pos: [f64; 3], vel: [f64; 3], on_ground: bool, width: f32, c: [i32; 3]) -> Option<[f64; 3]> {
    let old = |vy: f64| vy / f64::from(0.98f32) + 0.08;
    let new = |vy: f64| (vy - 0.08) * f64::from(0.98f32);
    let reach = 0.4375 + f64::from(width / 2.0);
    let sliding = !on_ground
        && pos[1] <= f64::from(c[1]) + 0.9375 - 1.0e-7
        && old(vel[1]) < -0.08
        && ((f64::from(c[0]) + 0.5 - pos[0]).abs() + 1.0e-7 > reach
            || (f64::from(c[2]) + 0.5 - pos[2]).abs() + 1.0e-7 > reach);
    if !sliding {
        return None;
    }
    let o = old(vel[1]);
    // Factor 1 outside braking: multiplying by 1 returns the exact value.
    let k = if o < -0.13 { -0.05 / o } else { 1.0 };
    Some([vel[0] * k, new(-0.05), vel[2] * k])
}

/// Boxes of `boxes` that touch `area` inflated by 1e-7 (the game's search rule for solid
/// entities; nothing if the zone is tiny).
pub fn entity_boxes_in(boxes: &[Aabb], area: &Aabb, out: &mut Vec<Aabb>) {
    let size = (0..3).map(|a| area.max[a] - area.min[a]).sum::<f64>() / 3.0;
    if size < 1.0e-7 {
        return;
    }
    let zone = Aabb { min: area.min.map(|v| v - 1.0e-7), max: area.max.map(|v| v + 1.0e-7) };
    out.extend(boxes.iter().filter(|b| b.intersects(&zone)));
}

// ---------- Tabulated sine ----------

fn sin_table() -> &'static [f32] {
    use std::sync::OnceLock;
    static T: OnceLock<Vec<f32>> = OnceLock::new();
    T.get_or_init(|| (0..65_536).map(|i| (f64::from(i) / 10_430.378_350_470_453).sin() as f32).collect())
}

pub fn sin_t(a: f32) -> f32 {
    sin_table()[((f64::from(a) * 10_430.378_350_470_453) as i64 & 65_535) as usize]
}

pub fn cos_t(a: f32) -> f32 {
    sin_table()[((f64::from(a) * 10_430.378_350_470_453 + 16_384.0) as i64 & 65_535) as usize]
}

const DEG: f32 = std::f32::consts::PI / 180.0;

// ---------- Per-state data ----------

/// Per-state tables and tags useful to the physics.
pub struct Tables {
    shapes: Vec<Vec<Aabb>>,
    /// Top of the collision shape per state (`f64::NEG_INFINITY`: empty shape); empty in the
    /// synthetic test tables (falls back to the shape).
    top: Vec<f64>,
    oversized: Vec<bool>,
    pub friction: Vec<f32>,
    pub speed_factor: Vec<f32>,
    pub jump_factor: Vec<f32>,
    pub fluids: FluidTables,
    /// Bounce restitution per state.
    pub bounce: Vec<f32>,
    /// Effect per state (`rc_data::inside`), downward bubble column coded `BUBBLES_DOWN`.
    inside: Vec<u8>,
    block: Vec<u16>,
    /// Blocks (indices) per block tag, once the tags are received.
    climbable: Vec<bool>,
    support_keep: Vec<u8>,
    /// Blocks of the `minecraft:suppresses_bounce` tag.
    no_bounce: Vec<bool>,
    /// Index of the lily pad block (ignored by the boats' friction), `u16::MAX` if it is missing.
    lily_pad: u16,
    /// Suffocation rule per state ([`SUFF_NEVER`], [`SUFF_ALWAYS`], [`SUFF_TAG`]).
    suff_rule: Vec<u8>,
    /// Suffocating state, once the tags are received.
    suffocating: Vec<bool>,
    /// Air blocks (air, cave air, void air), per block.
    air: Vec<bool>,
    /// States with collision or climbing that depends on the player (scaffolding, powder snow, trapdoor,
    /// ladder, water), tag `minecraft:can_glide_through`.
    special: Special,
    /// Pistons: bases, heads, moving blocks, slime and honey.
    piston: piston::PistonStates,
}

/// Per-state data of the blocks with player-specific rules; empty by default (no case).
#[derive(Debug, Clone, Default)]
pub struct Special {
    /// Per state: kind (`SP_*`, 4 low bits) and properties (4 high bits).
    kind: Vec<u8>,
    /// Per block: tag `minecraft:can_glide_through`.
    glide_through: Vec<bool>,
}

const SP_WATER: u8 = 1;
const SP_SCAFFOLDING: u8 = 2;
const SP_POWDER_SNOW: u8 = 3;
const SP_TRAPDOOR: u8 = 4;
const SP_LADDER: u8 = 5;
/// Scaffolding: `bottom=true`.
const SP_BOTTOM: u8 = 0x10;
/// Scaffolding: non-zero `distance`; trapdoor: open.
const SP_FLAG: u8 = 0x20;

impl Special {
    fn new(d: &VersionData) -> Self {
        let kind = (0..d.states.len() as u32)
            .map(|id| {
                let Some(b) = d.block_of(id) else { return 0 };
                let name = b.name.trim_start_matches("minecraft:");
                let prop = |k: &str| d.state_properties(id).find(|p| p.0 == k).map(|p| p.1);
                let facing = match prop("facing") {
                    Some("south") => 1u8,
                    Some("west") => 2,
                    Some("east") => 3,
                    _ => 0,
                } << 6;
                match name {
                    "water" => SP_WATER,
                    "scaffolding" => {
                        SP_SCAFFOLDING
                            | if prop("bottom") == Some("true") { SP_BOTTOM } else { 0 }
                            | if prop("distance").is_some_and(|v| v != "0") { SP_FLAG } else { 0 }
                    }
                    "powder_snow" => SP_POWDER_SNOW,
                    "ladder" => SP_LADDER | facing,
                    _ if name.ends_with("_trapdoor") => {
                        SP_TRAPDOOR | facing | if prop("open") == Some("true") { SP_FLAG } else { 0 }
                    }
                    _ => 0,
                }
            })
            .collect();
        Special { kind, glide_through: vec![false; d.blocks.len()] }
    }

    fn of(&self, s: u32) -> u8 {
        self.kind.get(s as usize).copied().unwrap_or(0)
    }
}

const SUFF_NEVER: u8 = 0;
const SUFF_ALWAYS: u8 = 1;
/// Suffocates if the block carries the `minecraft:causes_suffocation` tag (full shape already checked).
const SUFF_TAG: u8 = 2;

/// Suffocation rule of a state: never for glasses, foliage, mangrove roots,
/// copper grates and the moving piston; always for farmland, soul
/// sand, dirt path, mud and shulker boxes (held closed); piston only
/// retracted; otherwise the tag, and only for a full collision shape.
fn suff_rule(name: &str, extended: bool, full: bool) -> u8 {
    let name = name.trim_start_matches("minecraft:");
    let never = name == "glass"
        || name == "tinted_glass"
        || name.ends_with("_stained_glass")
        || name.ends_with("leaves")
        || name == "mangrove_roots"
        || name.ends_with("copper_grate")
        || name == "moving_piston";
    if never {
        SUFF_NEVER
    } else if matches!(name, "farmland" | "soul_sand" | "dirt_path" | "mud") || name.ends_with("shulker_box") {
        SUFF_ALWAYS
    } else if matches!(name, "piston" | "sticky_piston") {
        if extended { SUFF_NEVER } else { SUFF_ALWAYS }
    } else if full {
        SUFF_TAG
    } else {
        SUFF_NEVER
    }
}

/// Supporting block kept under the feet (`Tables::support_keep`, bits): fence (tag
/// `minecraft:fences`, only for an offset ≤ 0.5), wall (tag `minecraft:walls`),
/// fence gate (kind of the block, insensitive to tags as in the game).
const KEEP_FENCE: u8 = 1;
const KEEP_WALL: u8 = 2;
const KEEP_GATE: u8 = 4;

/// The game's fence gates (wood, bamboo, Nether): all and only those named `*_fence_gate`.
fn is_fence_gate(name: &str) -> bool {
    name.ends_with("_fence_gate")
}

/// Largest `max[1]` of the boxes (same fold as the old walk of a copy of the shape).
fn top_of(shape: &[Aabb]) -> Option<f64> {
    shape.iter().map(|b| b.max[1]).fold(None, |m, y| Some(m.map_or(y, |m: f64| m.max(y))))
}

/// Bubble column that pulls downwards (the rest follows `rc_data::inside`).
const BUBBLES_DOWN: u8 = 0x80 | inside::BUBBLE_COLUMN;

impl Tables {
    pub fn new(d: &VersionData) -> Self {
        let shape = |i: u16| -> Vec<Aabb> { d.shape(i).iter().map(|b| Aabb { min: b.min, max: b.max }).collect() };
        let shapes: Vec<Vec<Aabb>> = d.states.iter().map(|s| shape(s.collision)).collect();
        let oversized = shapes
            .iter()
            .map(|v| v.iter().any(|b| b.min.iter().any(|&c| c < 0.0) || b.max.iter().any(|&c| c > 1.0)))
            .collect();
        let per_block = |f: &dyn Fn(&rc_data::BlockInfo) -> f32| -> Vec<f32> {
            d.states.iter().map(|s| f(&d.blocks[s.block as usize])).collect()
        };
        let top = shapes.iter().map(|v| top_of(v).unwrap_or(f64::NEG_INFINITY)).collect();
        Tables {
            shapes,
            top,
            oversized,
            friction: per_block(&|b| b.friction),
            speed_factor: per_block(&|b| b.speed_factor),
            jump_factor: per_block(&|b| b.jump_factor),
            fluids: FluidTables::new(d),
            bounce: per_block(&|b| b.bounce),
            inside: (0..d.states.len() as u32)
                .map(|id| {
                    let k = d.block_of(id).map_or(inside::NONE, |b| b.inside);
                    let down = k == inside::BUBBLE_COLUMN && d.state_properties(id).any(|p| p == ("drag", "true"));
                    if down { BUBBLES_DOWN } else { k }
                })
                .collect(),
            block: d.states.iter().map(|s| s.block).collect(),
            climbable: vec![false; d.blocks.len()],
            support_keep: d.blocks.iter().map(|b| if is_fence_gate(&b.name) { KEEP_GATE } else { 0 }).collect(),
            no_bounce: vec![false; d.blocks.len()],
            lily_pad: d.blocks.iter().position(|b| b.name == "minecraft:lily_pad").map_or(u16::MAX, |i| i as u16),
            suff_rule: (0..d.states.len() as u32)
                .map(|id| {
                    let full = d
                        .shape(d.states[id as usize].collision)
                        .iter()
                        .map(|b| (b.min, b.max))
                        .eq([([0.0; 3], [1.0; 3])]);
                    let extended = d.state_properties(id).any(|p| p == ("extended", "true"));
                    d.block_of(id).map_or(SUFF_NEVER, |b| suff_rule(&b.name, extended, full))
                })
                .collect(),
            suffocating: vec![false; d.states.len()],
            air: d
                .blocks
                .iter()
                .map(|b| matches!(b.name.as_str(), "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air"))
                .collect(),
            special: Special::new(d),
            piston: piston::PistonStates::new(d),
        }
        .with_untagged_suffocation()
    }

    /// Suffocation without received tags: only the fixed rules count.
    fn with_untagged_suffocation(mut self) -> Self {
        self.suffocating = self.suff_rule.iter().map(|&r| r == SUFF_ALWAYS).collect();
        self
    }

    /// The state is air (without a table: state 0 only).
    pub fn is_air(&self, s: u32) -> bool {
        match self.block.get(s as usize) {
            Some(&b) if !self.air.is_empty() => self.air.get(b as usize).copied().unwrap_or(false),
            _ => s == 0,
        }
    }

    /// The state suffocates.
    pub fn suffocating(&self, s: u32) -> bool {
        self.suffocating.get(s as usize).copied().unwrap_or(false)
    }

    fn lily_pad(&self, s: u32) -> bool {
        self.block.get(s as usize).is_some_and(|&b| b == self.lily_pad)
    }

    /// Applies the received block tags (network identifiers = block indices).
    pub fn set_block_tags(&mut self, tags: &[(String, Vec<i32>)]) {
        let get = |name: &str| tags.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_slice()).unwrap_or(&[]);
        self.climbable.iter_mut().for_each(|c| *c = false);
        // The fence gate bit comes from the block itself: a reload of the tags keeps it.
        self.support_keep.iter_mut().for_each(|c| *c &= KEEP_GATE);
        self.no_bounce.iter_mut().for_each(|c| *c = false);
        self.special.glide_through.iter_mut().for_each(|c| *c = false);
        self.fluids.set_block_tags(tags);
        for (tag, set) in [
            ("minecraft:climbable", &mut self.climbable),
            ("minecraft:suppresses_bounce", &mut self.no_bounce),
            ("minecraft:can_glide_through", &mut self.special.glide_through),
        ] {
            for &b in get(tag) {
                if let Some(c) = set.get_mut(b as usize) {
                    *c = true;
                }
            }
        }
        let mut causes = vec![false; self.climbable.len()];
        for &b in get("minecraft:causes_suffocation") {
            if let Some(c) = causes.get_mut(b as usize) {
                *c = true;
            }
        }
        for (i, s) in self.suffocating.iter_mut().enumerate() {
            *s = match self.suff_rule[i] {
                SUFF_ALWAYS => true,
                SUFF_TAG => causes.get(self.block[i] as usize).copied().unwrap_or(false),
                _ => false,
            };
        }
        // Fence (kept if offset ≤ 0.5) and wall (always kept) follow their tag; the
        // fence gate is recognised by its block kind (`KEEP_GATE`), not by the tag.
        for (tag, v) in [("minecraft:fences", KEEP_FENCE), ("minecraft:walls", KEEP_WALL)] {
            for &b in get(tag) {
                if let Some(c) = self.support_keep.get_mut(b as usize) {
                    *c |= v;
                }
            }
        }
    }

    /// Local collision boxes of the state into `out`; true if the shape overflows the cube.
    pub fn collision(&self, state: u32, out: &mut Vec<Aabb>) -> bool {
        let i = state as usize;
        if let Some(s) = self.shapes.get(i) {
            out.extend_from_slice(s);
        }
        self.oversized.get(i).copied().unwrap_or(false)
    }

    /// Local collision boxes of the state, borrowed (empty for an unknown state).
    pub fn shape(&self, state: u32) -> &[Aabb] {
        self.shapes.get(state as usize).map_or(&[], Vec::as_slice)
    }

    /// Top of the collision shape of the state (`None` if it is empty), walked over the shape.
    pub fn collision_top(&self, state: u32) -> Option<f64> {
        top_of(self.shape(state))
    }

    /// Like `collision_top`, read from the precomputed table (falls back to the shape if the table is missing).
    pub fn collision_top_table(&self, state: u32) -> Option<f64> {
        match self.top.get(state as usize) {
            Some(&y) => (y != f64::NEG_INFINITY).then_some(y),
            None => self.collision_top(state),
        }
    }

    pub fn climbable(&self, s: u32) -> bool {
        self.block.get(s as usize).is_some_and(|&b| self.climbable[b as usize])
    }

    /// Speed factor read only on the occupied block (water block or bubble column,
    /// not a merely flooded block): never on the block below.
    pub fn speed_factor_here_only(&self, s: u32) -> bool {
        self.special.of(s) & 0x0F == SP_WATER || self.inside(s) & 0x7F == inside::BUBBLE_COLUMN
    }

    /// The occupied block is a scaffolding.
    pub fn is_scaffolding(&self, s: u32) -> bool {
        self.special.of(s) & 0x0F == SP_SCAFFOLDING
    }

    /// Open trapdoor placed on a ladder of the same orientation (`below`): climbable.
    fn trapdoor_ladder(&self, s: u32, below: u32) -> bool {
        let (k, b) = (self.special.of(s), self.special.of(below));
        k & 0x0F == SP_TRAPDOOR && k & SP_FLAG != 0 && b & 0x0F == SP_LADDER && k >> 6 == b >> 6
    }

    fn glide_through(&self, s: u32) -> bool {
        self.block
            .get(s as usize)
            .is_some_and(|&b| self.special.glide_through.get(b as usize).copied().unwrap_or(false))
    }

    fn suppresses_bounce(&self, s: u32) -> bool {
        self.block.get(s as usize).is_some_and(|&b| self.no_bounce[b as usize])
    }

    fn inside(&self, s: u32) -> u8 {
        self.inside.get(s as usize).copied().unwrap_or(inside::NONE)
    }
}

// ---------- Attributes ----------

#[derive(Debug, Clone, Default)]
struct Attr {
    base: f64,
    /// (identifier, amount, operation 0/1/2)
    modifiers: Vec<(String, f64, i32)>,
}

impl Attr {
    fn value(&self) -> f64 {
        let mut base = self.base;
        for m in self.modifiers.iter().filter(|m| m.2 == 0) {
            base += m.1;
        }
        let mut r = base;
        for m in self.modifiers.iter().filter(|m| m.2 == 1) {
            r += base * m.1;
        }
        for m in self.modifiers.iter().filter(|m| m.2 == 2) {
            r *= 1.0 + m.1;
        }
        r
    }
}

fn l2(v: [f64; 3]) -> f64 {
    v[0] * v[0] + v[1] * v[1] + v[2] * v[2]
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// Sprint modifier: same identifier as the server's, so that a received attribute
/// replaces it instead of adding to it.
const SPRINT_MODIFIER: &str = "minecraft:sprinting";

/// Base values of the player's attributes (replaced by those received from the server).
/// Range of values of an attribute (min, max); `None` for an unbounded or unknown attribute.
fn attribute_range(name: &str) -> Option<(f64, f64)> {
    Some(match name {
        "minecraft:movement_speed" | "minecraft:flying_speed" => (0.0, 1024.0),
        "minecraft:jump_strength" => (0.0, 32.0),
        "minecraft:step_height" => (0.0, 10.0),
        "minecraft:gravity" => (-1.0, 1.0),
        "minecraft:sneaking_speed" | "minecraft:movement_efficiency" | "minecraft:water_movement_efficiency" => {
            (0.0, 1.0)
        }
        "minecraft:bounciness" => (0.0, 1.0),
        "minecraft:friction_modifier" | "minecraft:air_drag_modifier" => (0.0, 2048.0),
        "minecraft:scale" => (0.0625, 16.0),
        "minecraft:safe_fall_distance" | "minecraft:luck" => (-1024.0, 1024.0),
        "minecraft:max_health" => (1.0, 1024.0),
        "minecraft:max_absorption" => (0.0, 2048.0),
        "minecraft:name_tag_distance" | "minecraft:below_name_distance" => (0.0, 512.0),
        _ => return None,
    })
}

/// Final value of an attribute brought back into its range; a NaN gives the minimum.
pub fn sanitize_attribute(name: &str, v: f64) -> f64 {
    match attribute_range(name) {
        Some((lo, hi)) => {
            if v.is_nan() {
                lo
            } else {
                v.clamp(lo, hi)
            }
        }
        None => v,
    }
}

const DEFAULTS: [(&str, f64); 12] = [
    ("minecraft:movement_speed", 0.1),
    ("minecraft:jump_strength", 0.42),
    ("minecraft:step_height", 0.6),
    ("minecraft:gravity", 0.08),
    ("minecraft:sneaking_speed", 0.3),
    ("minecraft:friction_modifier", 1.0),
    ("minecraft:air_drag_modifier", 1.0),
    ("minecraft:movement_efficiency", 0.0),
    ("minecraft:water_movement_efficiency", 0.0),
    ("minecraft:bounciness", 0.0),
    ("minecraft:scale", 1.0),
    ("minecraft:safe_fall_distance", 3.0),
];

// ---------- Tools common to entities ----------

fn state_of(w: &dyn World, p: [i32; 3]) -> u32 {
    w.state(p[0], p[1], p[2]).unwrap_or(0)
}

/// Fluids touched by the box `b` (shrunk by 0.001) of an entity with feet `pos`: heights
/// of water and lava above the feet, eyes (`eye_y`) in water; adds the push of the
/// currents to `vel` if `pushed` (§ 8.1). Player rule: average current of the touched blocks.
#[allow(clippy::too_many_arguments)]
pub fn scan_fluids(
    w: &dyn World,
    t: &Tables,
    b: &Aabb,
    pos: [f64; 3],
    eye_y: f64,
    pushed: bool,
    fast_lava: bool,
    vel: &mut [f64; 3],
) -> (f64, f64, bool) {
    scan_fluids_rule(w, t, b, pos, eye_y, pushed, fast_lava, vel, true)
}

/// Like `scan_fluids` for an entity other than a player (items, orbs, projectiles,
/// vehicles, mobs): the sum of the currents is brought to a length of 1 instead of being
/// averaged.
#[allow(clippy::too_many_arguments)]
pub fn scan_fluids_entity(
    w: &dyn World,
    t: &Tables,
    b: &Aabb,
    pos: [f64; 3],
    eye_y: f64,
    pushed: bool,
    fast_lava: bool,
    vel: &mut [f64; 3],
) -> (f64, f64, bool) {
    scan_fluids_rule(w, t, b, pos, eye_y, pushed, fast_lava, vel, false)
}

#[allow(clippy::too_many_arguments)]
fn scan_fluids_rule(
    w: &dyn World,
    t: &Tables,
    b: &Aabb,
    pos: [f64; 3],
    eye_y: f64,
    pushed: bool,
    fast_lava: bool,
    vel: &mut [f64; 3],
    player: bool,
) -> (f64, f64, bool) {
    let b = b.deflate(0.001);
    let lo = b.min.map(|v| v.floor() as i32);
    let hi = b.max.map(|v| v.ceil() as i32 - 1);
    let (mut water, mut lava) = (0.0f64, 0.0f64);
    let (ex, ez) = (pos[0].floor() as i32, pos[2].floor() as i32);
    let mut eyes = false;
    // Current per fluid: sum of the flows, count, greatest height seen so far.
    let mut current = [([0.0f64; 3], 0u32, 0.0f64); 2];
    let st = |x: i32, y: i32, z: i32| state_of(w, [x, y, z]);
    for x in lo[0]..=hi[0] {
        for y in lo[1]..=hi[1] {
            for z in lo[2]..=hi[2] {
                let s = st(x, y, z);
                let f = t.fluids.fluid(s);
                if f == Fluid::None {
                    continue;
                }
                let h = if t.fluids.fluid(st(x, y + 1, z)) == f { 1.0 } else { f64::from(t.fluids.height[s as usize]) };
                let top = f64::from(y) + h;
                if top < b.min[1] {
                    continue;
                }
                let depth = top - pos[1];
                let (level, i) = match f {
                    Fluid::Water => (&mut water, 0),
                    _ => (&mut lava, 1),
                };
                *level = level.max(depth);
                // Eye in water: height seen by the camera (source under a sturdy ceiling = 1).
                if f == Fluid::Water
                    && x == ex
                    && z == ez
                    && eye_y >= f64::from(y)
                    && eye_y <= f64::from(y) + f64::from(t.fluids.camera_height(s, st(x, y + 1, z)))
                {
                    eyes = true;
                }
                if pushed {
                    let c = &mut current[i];
                    c.2 = c.2.max(*level);
                    let mut flow = t.fluids.flow(&st, f, x, y, z);
                    if c.2 < 0.4 {
                        flow = flow.map(|v| v * c.2);
                    }
                    for (a, v) in c.0.iter_mut().zip(flow) {
                        *a += v;
                    }
                    c.1 += 1;
                }
            }
        }
    }
    if pushed {
        let lava_scale = if fast_lava { 0.007 } else { 0.002_333_333_333_333_333_5 };
        for (i, height, scale) in [(0, water, 0.014), (1, lava, lava_scale)] {
            let (sum, n, _) = current[i];
            if height <= 0.0 || n == 0 || l2(sum) < f64::from(1.0e-5f32) {
                continue;
            }
            let mut imp = if player {
                sum.map(|v| v / f64::from(n) * scale)
            } else {
                let l = l2(sum).sqrt();
                sum.map(|v| v / l * scale)
            };
            const MIN: f64 = 0.004_500_000_000_000_000_5;
            if vel[0].abs() < 0.003 && vel[2].abs() < 0.003 && l2(imp).sqrt() < MIN {
                let l = l2(imp).sqrt();
                imp = if l < f64::from(1.0e-5f32) { [0.0; 3] } else { imp.map(|v| v / l * MIN) };
            }
            for (v, d) in vel.iter_mut().zip(imp) {
                *v += d;
            }
        }
    }
    (water, lava, eyes)
}

/// Main supporting block of an entity on the ground with box `b` and feet `pos` (§ 6.5, without the
/// second offset search).
pub fn support_of(w: &dyn World, b: &Aabb, pos: [f64; 3]) -> Option<[i32; 3]> {
    let zone = Aabb { min: [b.min[0], b.min[1] - 1.0e-6, b.min[2]], max: [b.max[0], b.min[1], b.max[2]] };
    let mut best: Option<([i32; 3], f64)> = None;
    // Same cells as the collisions (one-block margin, corners ignored, edges reserved for shapes
    // that overflow): standing on a fence or a wall (1.5 blocks), the support is under the
    // row of the feet.
    collide::with_cell(collide::is_free_mode(), |cell| {
        collide::scan(w, &zone, false, cell, |[x, y, z], _| {
            let c = [f64::from(x) + 0.5, f64::from(y) + 0.5, f64::from(z) + 0.5];
            let d2 = (0..3).map(|a| (c[a] - pos[a]).powi(2)).sum::<f64>();
            let better = match best {
                None => true,
                Some((p, bd)) => d2 < bd || (d2 == bd && (y, z, x) > (p[1], p[2], p[0])),
            };
            if better {
                best = Some(([x, y, z], d2));
            }
            false
        })
    });
    best.map(|b| b.0)
}

/// Position under the feet `pos` with an offset `h`, according to the supporting block (§ 4.2).
pub fn below_of(support: Option<[i32; 3]>, pos: [f64; 3], t: &Tables, w: &dyn World, h: f32) -> [i32; 3] {
    let y = (pos[1] - f64::from(h)).floor() as i32;
    if let Some(s) = support {
        if h <= 1.0e-5 {
            return s;
        }
        let keep = w
            .state(s[0], s[1], s[2])
            .and_then(|st| t.block.get(st as usize))
            .map_or(0, |&b| t.support_keep[b as usize]);
        if keep & (KEEP_WALL | KEEP_GATE) != 0 || (keep & KEEP_FENCE != 0 && h <= 0.5) {
            return s;
        }
        return [s[0], y, s[2]];
    }
    [pos[0].floor() as i32, y, pos[2].floor() as i32]
}

// ---------- Player ----------

/// Keys of the tick (in the order of the input packet).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Input {
    pub forward: bool,
    pub backward: bool,
    pub left: bool,
    pub right: bool,
    pub jump: bool,
    pub shift: bool,
    pub sprint: bool,
}

impl Input {
    pub fn bits(&self) -> u8 {
        [self.forward, self.backward, self.left, self.right, self.jump, self.shift, self.sprint]
            .iter()
            .enumerate()
            .fold(0, |acc, (i, &b)| acc | (u8::from(b) << i))
    }
}

/// Status effects of the local player that touch movement (`movement.md` § 2.3, 4.3, 4.5, 4.6, 8.4);
/// an effect is active from its addition by the server until its removal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct MoveEffects {
    /// Jump boost: amplifier.
    pub jump_boost: Option<i32>,
    /// Levitation: amplifier.
    pub levitation: Option<i32>,
    pub slow_falling: bool,
    pub dolphins_grace: bool,
    /// Blindness: restricted mobility (no sprint).
    pub blindness: bool,
}

impl MoveEffects {
    /// Effect added (`Some(amplifier)`) or removed (`None`), by registry name.
    pub fn set(&mut self, name: &str, amplifier: Option<i32>) {
        match name {
            "minecraft:jump_boost" => self.jump_boost = amplifier,
            "minecraft:levitation" => self.levitation = amplifier,
            "minecraft:slow_falling" => self.slow_falling = amplifier.is_some(),
            "minecraft:dolphins_grace" => self.dolphins_grace = amplifier.is_some(),
            "minecraft:blindness" => self.blindness = amplifier.is_some(),
            _ => {}
        }
    }
}

/// Posture of the player, which sets its box and its eyes (`movement.md` § 6.1). `Swimming` is also used
/// for crawling out of the water.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Pose {
    #[default]
    Standing,
    Crouching,
    Swimming,
    /// Elytra gliding (0.6 × 0.6, eyes at 0.4).
    FallFlying,
    /// Spin attack of the Riptide trident (0.6 × 0.6, eyes at 0.4).
    SpinAttack,
}

/// How a neighbouring entity pushes the local player at its own tick (client side, only the
/// local player is pushed by the others).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Pusher {
    /// Does not push (item, bat, armor stand, parrot, team rule…).
    #[default]
    None,
    /// Alive: pushes when the boxes overlap.
    Living,
    /// Boat: box inflated by 0.2 (−0.01 in height), if the player's feet are not higher
    /// than the bottom of the boat.
    Boat,
}

/// Entity near the local player, supplied by the application before each tick (push,
/// spin attack).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Nearby {
    pub bb: Aabb,
    pub pos: [f64; 3],
    /// Alive (stops a spin attack that touches it).
    pub living: bool,
    pub pusher: Pusher,
}

/// What the tick asks to send to the server.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Signals {
    /// New sprint state (player command).
    pub sprinting: Option<bool>,
    /// New flight state (abilities packet).
    pub flying: Option<bool>,
    /// Jump forced by auto-jump this tick (to include in the input packet).
    pub auto_jump: bool,
    /// Glide started this tick (player command `START_FALL_FLYING`).
    pub start_fall_flying: bool,
}

#[derive(Debug, Clone)]
pub struct Player {
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub horizontal_collision: bool,
    pub vertical_collision: bool,
    pub minor_collision: bool,
    pub flying: bool,
    pub may_fly: bool,
    /// Spectator mode: forced flight, no collision or block effect, never on the ground (§ 2.1, 6.2).
    pub spectator: bool,
    pub fly_speed: f32,
    pub sprinting: bool,
    /// Sneaking in the sense of inputs (slowdown, caution at the edge).
    pub crouching: bool,
    /// Swimming (state), distinct from the posture.
    pub swimming: bool,
    /// Posture: box and eye height, updated at the end of the tick.
    pub pose: Pose,
    /// Status effects that touch movement.
    pub effects: MoveEffects,
    pub fall_distance: f64,
    pub water_height: f64,
    pub lava_height: f64,
    pub eyes_in_water: bool,
    /// The player can eat (food > 6); received from the server, true while waiting.
    pub food_ok: bool,
    /// Fast-lava dimension (environment attribute, the Nether by default).
    pub fast_lava: bool,
    /// Item being used (except as a passenger): speed multiplier and sprint allowed
    /// by its use effects (§ 2.3, 2.4).
    pub using_item: Option<(f32, bool)>,
    /// Auto-jump (`autoJump` setting).
    pub auto_jump: bool,
    /// Forward double-tap window to sprint, in ticks (`sprintWindow` setting, 7).
    pub sprint_window: u32,
    /// Gliding (shared entity flag 7): set here at the start, then as the server
    /// sends it back in the entity data.
    pub fall_flying: bool,
    /// A usable glider item is worn in place (elytra not about to break).
    pub glider: bool,
    /// Leather boots on the feet: walks on powder snow and climbs.
    pub leather_boots: bool,
    /// Spin attack (Riptide): ticks remaining, and entity flag (posture).
    pub spin_attack_ticks: u32,
    pub spin_attack: bool,
    /// Neighbouring entities of the tick (filled by the application).
    pub nearby: Vec<Nearby>,
    /// Firework rockets hooked to the player this tick (push while gliding).
    pub rocket_boosts: u32,
    /// In powder snow during the tick, and at the previous tick.
    in_powder_snow: bool,
    was_in_powder_snow: bool,
    /// Shift held this tick ("goes down" for collision shapes).
    descending: bool,
    jump_delay: u32,
    fly_toggle: u32,
    sprint_tap: u32,
    auto_jump_time: u32,
    /// Normalised key vector (sideways, forward) of the tick.
    move_vec: (f32, f32),
    prev_jump: bool,
    prev_shift: bool,
    prev_forward: bool,
    support: Option<[i32; 3]>,
    attrs: HashMap<String, Attr>,
    /// Blocking multiplier (cobweb, berries…) applied to the next movement.
    stuck: [f64; 3],
    /// Piston pushes accumulated during the tick (cap per axis).
    piston_deltas: piston::PistonDeltas,
    /// Position at the start of the tick.
    old_pos: [f64; 3],
    /// Movements of the tick: (start, end, requested movement).
    moves: Vec<([f64; 3], [f64; 3], [f64; 3])>,
}

impl Player {
    pub fn new(pos: [f64; 3], yaw: f32, pitch: f32) -> Self {
        Player {
            pos,
            vel: [0.0; 3],
            yaw,
            pitch,
            on_ground: false,
            horizontal_collision: false,
            vertical_collision: false,
            minor_collision: false,
            flying: false,
            may_fly: false,
            spectator: false,
            fly_speed: 0.05,
            sprinting: false,
            crouching: false,
            swimming: false,
            pose: Pose::Standing,
            effects: MoveEffects::default(),
            fall_distance: 0.0,
            water_height: 0.0,
            lava_height: 0.0,
            eyes_in_water: false,
            food_ok: true,
            fast_lava: false,
            using_item: None,
            auto_jump: false,
            sprint_window: 7,
            fall_flying: false,
            glider: false,
            leather_boots: false,
            spin_attack_ticks: 0,
            spin_attack: false,
            nearby: Vec::new(),
            rocket_boosts: 0,
            in_powder_snow: false,
            was_in_powder_snow: false,
            descending: false,
            jump_delay: 0,
            fly_toggle: 0,
            sprint_tap: 0,
            auto_jump_time: 0,
            move_vec: (0.0, 0.0),
            prev_jump: false,
            prev_shift: false,
            prev_forward: false,
            support: None,
            stuck: [0.0; 3],
            piston_deltas: Default::default(),
            old_pos: pos,
            moves: Vec::new(),
            attrs: DEFAULTS.iter().map(|&(n, v)| (n.to_owned(), Attr { base: v, modifiers: Vec::new() })).collect(),
        }
    }

    /// Velocity imposed by the server (entity movement packet: knockback from hits,
    /// arrows, wind charges…): replaces the velocity as is.
    pub fn set_motion(&mut self, v: [f64; 3]) {
        self.vel = v;
    }

    /// Explosion knockback (`protocol-supplements.md` § 3): added as is to the velocity, if
    /// its three components are finite; the next tick does the rest.
    pub fn push(&mut self, v: [f64; 3]) -> bool {
        if !v.iter().all(|c| c.is_finite()) {
            return false;
        }
        for (a, b) in self.vel.iter_mut().zip(v) {
            *a += b;
        }
        true
    }

    /// Attribute received from the server: base and modifiers replaced in full (the local sprint
    /// included, like the game; the server sends back its own under the same identifier). A
    /// duplicated identifier counts only once (the last received).
    pub fn set_attribute(&mut self, name: &str, base: f64, modifiers: Vec<(String, f64, i32)>) {
        let a = self.attrs.entry(name.to_owned()).or_default();
        a.base = base;
        a.modifiers.clear();
        for m in modifiers {
            a.modifiers.retain(|o| o.0 != m.0);
            a.modifiers.push(m);
        }
    }

    /// Value of an attribute received from the server, if it sent it.
    pub fn attribute(&self, name: &str) -> Option<f64> {
        self.attrs.get(name).map(|a| sanitize_attribute(name, a.value()))
    }

    fn attr(&self, name: &str) -> f64 {
        self.attrs.get(name).map_or(0.0, |a| sanitize_attribute(name, a.value()))
    }

    fn set_sprinting(&mut self, on: bool, sig: &mut Signals) {
        if self.sprinting == on {
            return;
        }
        self.sprinting = on;
        let a = self.attrs.entry("minecraft:movement_speed".into()).or_default();
        a.modifiers.retain(|m| m.0 != SPRINT_MODIFIER);
        if on {
            a.modifiers.push((SPRINT_MODIFIER.into(), f64::from(0.3f32), 2));
        }
        sig.sprinting = Some(on);
    }

    fn scale(&self) -> f64 {
        self.attr("minecraft:scale")
    }

    /// Dimensions (width, height) of the current posture.
    pub fn dimensions(&self) -> (f64, f64) {
        self.dimensions_of(self.pose)
    }

    /// Width, height and eyes of the posture at scale 1, in `f32` like the game.
    fn base_dimensions(pose: Pose) -> (f32, f32, f32) {
        match pose {
            Pose::Swimming | Pose::FallFlying | Pose::SpinAttack => (0.6, 0.6, 0.4),
            Pose::Crouching => (0.6, 1.5, 1.27),
            Pose::Standing => (0.6, 1.8, 1.62),
        }
    }

    /// (width, height): products in `f32` (dimensions × scale), like the game's box.
    fn dimensions_of(&self, pose: Pose) -> (f64, f64) {
        let s = self.scale() as f32;
        let (w, h, _) = Self::base_dimensions(pose);
        let (w, h) = if s == 1.0 { (w, h) } else { (w * s, h * s) };
        (f64::from(w), f64::from(h))
    }

    pub fn eye_height(&self) -> f64 {
        let s = self.scale() as f32;
        let e = Self::base_dimensions(self.pose).2;
        f64::from(if s == 1.0 { e } else { e * s })
    }

    /// The box of this posture (shrunk by 1e-7) touches no block nor solid entity.
    fn fits(&self, w: &dyn World, pose: Pose) -> bool {
        let (bw, bh) = self.dimensions_of(pose);
        is_free(w, &self.box_with(bw, bh).deflate(1.0e-7))
    }

    /// The world as seen by the player's collision searches: shapes of scaffolding and
    /// powder snow depending on its height, Shift, its fall and its boots.
    fn ctx<'a>(&self, w: &'a dyn World, t: &'a Tables) -> CtxWorld<'a> {
        CtxWorld {
            w,
            t,
            bottom: self.pos[1],
            descending: self.descending,
            fall: self.fall_distance,
            boots: self.leather_boots,
        }
    }

    /// The same world, for whoever wants to list the collisions as the player sees them
    /// (collision shape debug tracer).
    pub fn collision_view<'a>(&self, w: &'a dyn World, t: &'a Tables) -> impl World + 'a {
        self.ctx(w, t)
    }

    /// Main supporting block (`None` in the air).
    pub fn supporting_block(&self) -> Option<[i32; 3]> {
        self.support
    }

    /// Occupied climbable block: never as a spectator nor in flight; not while gliding,
    /// in a block one glides through; an open trapdoor placed on a ladder of the same
    /// orientation counts as a ladder.
    pub fn on_climbable(&self, w: &dyn World, t: &Tables) -> bool {
        if self.spectator || self.flying {
            return false;
        }
        let p = self.block_pos();
        let s = Self::state_at(w, p);
        if self.fall_flying && t.glide_through(s) {
            return false;
        }
        t.climbable(s) || t.trapdoor_ladder(s, Self::state_at(w, [p[0], p[1] - 1, p[2]]))
    }

    /// End-of-tick posture: the targeted one (swimming, otherwise sneaking if Shift outside flight, otherwise standing)
    /// if it fits, or always as a spectator; otherwise sneaking if that fits; otherwise crawling.
    /// Nothing changes if even the swimming box does not fit.
    fn update_pose(&mut self, w: &dyn World, shift: bool) {
        if !self.fits(w, Pose::Swimming) {
            return;
        }
        let desired = if self.swimming {
            Pose::Swimming
        } else if self.fall_flying {
            Pose::FallFlying
        } else if self.spin_attack {
            Pose::SpinAttack
        } else if shift && !self.flying {
            Pose::Crouching
        } else {
            Pose::Standing
        };
        self.pose = if self.spectator || self.fits(w, desired) {
            desired
        } else if self.fits(w, Pose::Crouching) {
            Pose::Crouching
        } else {
            Pose::Swimming
        };
    }

    /// Passenger posture: always the targeted one (sneaking if Shift, otherwise standing).
    pub fn set_passenger_pose(&mut self, shift: bool) {
        self.pose = if shift && !self.flying { Pose::Crouching } else { Pose::Standing };
    }

    /// Crawls: swimming posture out of the water (or gliding posture without gliding, like the game).
    pub fn crawling(&self) -> bool {
        let visually_swimming = self.pose == Pose::Swimming || (!self.fall_flying && self.pose == Pose::FallFlying);
        visually_swimming && self.water_height <= 0.0
    }

    fn box_with(&self, w: f64, h: f64) -> Aabb {
        let r = w / 2.0;
        Aabb {
            min: [self.pos[0] - r, self.pos[1], self.pos[2] - r],
            max: [self.pos[0] + r, self.pos[1] + h, self.pos[2] + r],
        }
    }

    pub fn bounding_box(&self) -> Aabb {
        let (w, h) = self.dimensions();
        self.box_with(w, h)
    }

    /// Position under the feet with an offset (§ 4.2).
    fn below(&self, t: &Tables, w: &dyn World, h: f32) -> [i32; 3] {
        below_of(self.support, self.pos, t, w, h)
    }

    /// Block under the feet with an offset `h` (1e-5: support block; 0.2: effect block).
    pub fn on_pos(&self, t: &Tables, w: &dyn World, h: f32) -> [i32; 3] {
        self.below(t, w, h)
    }

    fn state_at(w: &dyn World, p: [i32; 3]) -> u32 {
        w.state(p[0], p[1], p[2]).unwrap_or(0)
    }

    fn block_pos(&self) -> [i32; 3] {
        self.pos.map(|v| v.floor() as i32)
    }

    /// One tick of the local player, in the official order. Returns the changes to report.
    pub fn tick(&mut self, input: Input, w: &dyn World, t: &Tables) -> Signals {
        let mut input = input;
        let mut sig = Signals::default();
        self.old_pos = self.pos;
        self.descending = input.shift;
        // Ticks of the other entities, replayed here: hooked rockets, pushes from neighbours.
        for _ in 0..std::mem::take(&mut self.rocket_boosts) {
            self.rocket_boost();
        }
        self.pushed_by_neighbours(w, t);
        // Powder snow: the state of the previous tick, reset (base tick).
        self.was_in_powder_snow = self.in_powder_snow;
        self.in_powder_snow = false;
        // Spectator: never on the ground (§ 2.1, step 1).
        if self.spectator {
            self.on_ground = false;
        }
        // Fluids (base tick): read on the state of the previous tick for "underwater".
        let was_eyes_in_water = self.eyes_in_water;
        self.update_fluids(w, t);
        let in_water = self.water_height > 0.0;
        let under_water = was_eyes_in_water && in_water;
        // Swimming (§ 8.2).
        if self.flying {
            self.swimming = false;
        } else if self.swimming {
            self.swimming = self.sprinting && in_water;
        } else {
            let here = Self::state_at(w, self.block_pos());
            self.swimming = self.sprinting && under_water && t.fluids.fluid(here) == Fluid::Water;
        }

        // --- AI of the local player (§ 2.2) ---
        if self.sprint_tap > 0 {
            self.sprint_tap -= 1;
        }
        let (prev_jump, prev_shift, prev_forward) = (self.prev_jump, self.prev_shift, self.prev_forward);
        // Sneaking.
        self.crouching = !self.flying && !self.swimming && {
            let cw = self.ctx(w, t);
            self.fits(&cw, Pose::Crouching) && (prev_shift || !self.fits(&cw, Pose::Standing))
        };
        // Keys → normalised vector (sideways, forward).
        let fwd = f32::from(i8::from(input.forward) - i8::from(input.backward));
        let side = f32::from(i8::from(input.left) - i8::from(input.right));
        let len = (fwd * fwd + side * side).sqrt();
        let (mut sx, mut fz) = if len > 0.0 { (side / len, fwd / len) } else { (0.0, 0.0) };
        self.move_vec = (sx, fz);
        // Auto-jump decided at the previous tick's movement: forced jump (§ 2.6).
        let mut was_auto = false;
        if self.auto_jump_time > 0 {
            self.auto_jump_time -= 1;
            was_auto = true;
            input.jump = true;
            sig.auto_jump = true;
        }
        // Push out of the suffocating blocks, at the four corners (§ 2.2).
        if !self.spectator {
            let r = self.dimensions().0 * 0.35;
            let (x, z) = (self.pos[0], self.pos[2]);
            for (dx, dz) in [(-r, r), (-r, -r), (r, -r), (r, r)] {
                self.move_towards_closest_space(w, t, x + dx, z + dz);
            }
        }
        let forward_now = fz > 1.0e-5;
        let slowing_use = self.using_item.is_some_and(|u| !u.1);
        if prev_shift || input.backward || slowing_use {
            self.sprint_tap = 0;
        }
        // Sprint (§ 2.3).
        // Blindness: restricted mobility, sprint impossible.
        let sprint_possible = |p: &Player, allow_shallow: bool| {
            !p.effects.blindness && (p.food_ok || p.may_fly) && (allow_shallow || !(in_water && !under_water))
        };
        let moving_slowly = self.crouching || self.crawling();
        if !self.sprinting
            && forward_now
            && sprint_possible(self, self.flying)
            && !slowing_use
            && (!self.fall_flying || under_water)
            && (!moving_slowly || under_water)
        {
            if !prev_forward {
                if self.sprint_tap > 0 {
                    self.set_sprinting(true, &mut sig);
                } else {
                    self.sprint_tap = self.sprint_window;
                }
            }
            if input.sprint {
                self.set_sprinting(true, &mut sig);
            }
        }
        if self.sprinting {
            let stop = if self.swimming {
                !sprint_possible(self, true) || !in_water || (!forward_now && !self.on_ground && !input.shift)
            } else {
                !sprint_possible(self, self.flying)
                    || !forward_now
                    || (self.horizontal_collision && !self.minor_collision)
            };
            if stop {
                self.set_sprinting(false, &mut sig);
            }
        }
        // Double tap on jump: flight (§ 5.1). Spectator: flight imposed, reported to the server.
        let mut toggled = false;
        if self.may_fly && self.spectator {
            if !self.flying {
                self.flying = true;
                toggled = true;
                sig.flying = Some(true);
            }
        } else if self.may_fly && input.jump && !prev_jump && !was_auto {
            if self.fly_toggle == 0 {
                self.fly_toggle = 7;
            } else if !self.swimming {
                self.flying = !self.flying;
                toggled = true;
                sig.flying = Some(self.flying);
                self.fly_toggle = 0;
                if self.flying && self.on_ground {
                    self.jump_from_ground(w, t);
                }
            }
        }
        // Gliding: jump in the air (new press, outside flight toggle and outside ladder) with a
        // worn glider, off the ground, fluid, levitation and flight (`START_FALL_FLYING`).
        if input.jump && !toggled && !prev_jump && !self.on_climbable(w, t) && self.try_start_fall_flying() {
            sig.start_fall_flying = true;
        }
        // Shift in water: descent.
        if in_water && input.shift && !self.flying {
            self.vel[1] -= f64::from(0.04f32);
        }
        // Flight: ascent / descent (§ 5.2).
        if self.flying {
            let k = i32::from(input.jump) - i32::from(input.shift);
            if k != 0 {
                self.vel[1] += f64::from(k as f32 * self.fly_speed * 3.0);
            }
        }

        // --- Generic AI (§ 3) ---
        if self.fly_toggle > 0 {
            self.fly_toggle -= 1;
        }
        if self.flying {
            self.fall_distance = 0.0;
        }
        if self.jump_delay > 0 {
            self.jump_delay -= 1;
        }
        if self.vel[0] * self.vel[0] + self.vel[2] * self.vel[2] < 9.0e-6 {
            self.vel[0] = 0.0;
            self.vel[2] = 0.0;
        }
        if self.vel[1].abs() < 0.003 {
            self.vel[1] = 0.0;
        }
        // Inputs (§ 2.4): ×0.98, sneaking, "square" correction.
        if sx != 0.0 || fz != 0.0 {
            sx *= 0.98;
            fz *= 0.98;
            if let Some((k, _)) = self.using_item {
                sx *= k;
                fz *= k;
            }
            if self.crouching || self.crawling() {
                let k = self.attr("minecraft:sneaking_speed") as f32;
                sx *= k;
                fz *= k;
            }
            let l = (sx * sx + fz * fz).sqrt();
            let (ax, az) = (sx.abs(), fz.abs());
            let ratio = ax.min(az) / ax.max(az);
            let nl = (l * (1.0 + ratio * ratio).sqrt()).min(1.0);
            sx = sx / l * nl;
            fz = fz / l * nl;
        }
        // Jump (§ 4.5).
        if input.jump && !self.flying {
            let h = if self.lava_height > 0.0 { self.lava_height } else { self.water_height };
            let threshold = if self.eye_height() < 0.4 { 0.0 } else { 0.4 };
            let water_h = in_water && h > 0.0;
            let swim_up = (water_h && (!self.on_ground || h > threshold))
                || (self.lava_height > 0.0 && !(self.on_ground && h <= threshold));
            if swim_up {
                self.vel[1] += f64::from(0.04f32);
            } else if (self.on_ground || (water_h && h <= threshold)) && self.jump_delay == 0 {
                self.jump_from_ground(w, t);
                self.jump_delay = 10;
            }
        } else {
            self.jump_delay = 0;
        }
        // Gliding: the fall distance falls back to 1 as long as one does not descend fast.
        if self.fall_flying && self.vel[1] > -0.5 && self.fall_distance > 1.0 {
            self.fall_distance = 1.0;
        }
        let before_travel = self.bounding_box();
        // Movement, then effects of the blocks touched.
        self.travel(sx, fz, input, w, t);
        self.block_effects(input, w, t);
        // Spin attack: countdown; an entity in the swept zone stops it (bounce of
        // −0.2 on a mob), otherwise a wall.
        if self.spin_attack_ticks > 0 {
            self.spin_attack_ticks -= 1;
            let now = self.bounding_box();
            let swept = Aabb {
                min: std::array::from_fn(|a| before_travel.min[a].min(now.min[a])),
                max: std::array::from_fn(|a| before_travel.max[a].max(now.max[a])),
            };
            let mut touched = self.nearby.iter().filter(|e| e.bb.intersects(&swept)).peekable();
            if touched.peek().is_some() {
                if touched.any(|e| e.living) {
                    self.spin_attack_ticks = 0;
                    self.vel = self.vel.map(|v| v * -0.2);
                }
            } else if self.horizontal_collision {
                self.spin_attack_ticks = 0;
            }
        }
        // End of AI: the flight stops on the ground (never as a spectator).
        if self.on_ground && self.flying && !self.spectator {
            self.flying = false;
            sig.flying = Some(false);
        }
        self.prev_jump = input.jump;
        self.prev_shift = input.shift;
        self.prev_forward = forward_now;
        // End-of-tick posture (after the movement).
        let cw = self.ctx(w, t);
        self.update_pose(&cw, input.shift);
        // World bounds in X and Z.
        self.pos[0] = self.pos[0].clamp(-2.999_999_9e7, 2.999_999_9e7);
        self.pos[2] = self.pos[2].clamp(-2.999_999_9e7, 2.999_999_9e7);
        sig
    }

    /// Jump factor of the occupied block, otherwise of the one that carries (honey: 0.5).
    fn block_jump_factor(&self, w: &dyn World, t: &Tables) -> f32 {
        let here = Self::state_at(w, self.block_pos());
        let factor = t.jump_factor.get(here as usize).copied().unwrap_or(1.0);
        if factor != 1.0 {
            return factor;
        }
        let b = Self::state_at(w, self.below(t, w, 0.500_001));
        t.jump_factor.get(b as usize).copied().unwrap_or(1.0)
    }

    /// Auto-jump (§ 2.6), after each movement: an obstacle of more than half a
    /// block and at most 1.2 blocks (+ 0.75 per jump boost level) in front, with no
    /// ceiling, triggers a jump at the next tick.
    fn update_auto_jump(&mut self, before: [f64; 3], input: Input, w: &dyn World, t: &Tables) {
        let moving = self.move_vec.0 != 0.0 || self.move_vec.1 != 0.0;
        if !self.auto_jump
            || self.auto_jump_time > 0
            || !self.on_ground
            || input.shift
            || !moving
            || self.block_jump_factor(w, t) < 1.0
        {
            return;
        }
        let xa = (self.pos[0] - before[0]) as f32;
        let za = (self.pos[2] - before[2]) as f32;
        let speed = self.attr("minecraft:movement_speed") as f32;
        let mut diff = [f64::from(xa), 0.0, f64::from(za)];
        let mut dist_sq = (diff[0] * diff[0] + diff[2] * diff[2]) as f32;
        if dist_sq <= 0.001 {
            let (ix, iz) = (speed * self.move_vec.0, speed * self.move_vec.1);
            let r = self.yaw * DEG;
            let (s, c) = (sin_t(r), cos_t(r));
            diff = [f64::from(ix * c - iz * s), 0.0, f64::from(iz * c + ix * s)];
            dist_sq = (diff[0] * diff[0] + diff[2] * diff[2]) as f32;
            if dist_sq <= 0.001 {
                return;
            }
        }
        let inv = 1.0 / dist_sq.sqrt();
        let dir = diff.map(|v| v * f64::from(inv));
        // Look (yaw and pitch), only x and z.
        let (p, y) = (self.pitch * DEG, -self.yaw * DEG);
        let (fx, fz) = (f64::from(sin_t(y) * cos_t(p)), f64::from(cos_t(y) * cos_t(p)));
        if ((fx * dir[0] + fz * dir[2]) as f32) < -0.15 {
            return;
        }
        let mut cell = Vec::new();
        let empty = |x: i32, y: i32, z: i32, out: &mut Vec<Aabb>| {
            out.clear();
            w.collision(x, y, z, out);
            out.is_empty()
        };
        let bb = self.bounding_box();
        let (cx, cz) = (self.pos[0].floor() as i32, self.pos[2].floor() as i32);
        let mut ceil_y = bb.max[1].floor() as i32;
        if !empty(cx, ceil_y, cz, &mut cell) {
            return;
        }
        ceil_y += 1;
        if !empty(cx, ceil_y, cz, &mut cell) {
            return;
        }
        // Jump boost: + 0.75 per level.
        let jump_height = 1.2f32 + self.effects.jump_boost.map_or(0.0, |a| (a + 1) as f32 * 0.75);
        let look = (speed * 7.0).max(1.0 / inv);
        let begin = self.pos;
        let end = [begin[0] + f64::from(xa), begin[1], begin[2] + f64::from(za)];
        let seg_end = [0, 1, 2].map(|a| end[a] + dir[a] * f64::from(look));
        let (bw, bh) = self.dimensions();
        let (bw, bh) = (bw as f32, bh as f32);
        let lo = [0, 1, 2].map(|a| begin[a].min(seg_end[a] + if a == 1 { f64::from(bh) } else { 0.0 }));
        let hi = [0, 1, 2].map(|a| begin[a].max(seg_end[a] + if a == 1 { f64::from(bh) } else { 0.0 }));
        let grow = [f64::from(bw), 0.0, f64::from(bw)];
        let test = Aabb { min: [0, 1, 2].map(|a| lo[a] - grow[a]), max: [0, 1, 2].map(|a| hi[a] + grow[a]) };
        let lift = f64::from(0.51f32);
        let sb = [begin[0], begin[1] + lift, begin[2]];
        let se = [seg_end[0], seg_end[1] + lift, seg_end[2]];
        // Right side: direction × (0, 1, 0).
        let half = f64::from(bw * 0.5);
        let off = [-dir[2] * half, 0.0, dir[0] * half];
        let seg = |a: [f64; 3], b: [f64; 3]| Aabb {
            min: [0, 1, 2].map(|i| a[i].min(b[i])),
            max: [0, 1, 2].map(|i| a[i].max(b[i])),
        };
        let left = seg(sub(sb, off), sub(se, off));
        let right = seg([0, 1, 2].map(|i| sb[i] + off[i]), [0, 1, 2].map(|i| se[i] + off[i]));
        let mut boxes = Vec::new();
        collide::collect_ordered(&self.ctx(w, t), &test, &mut boxes);
        let mut obstacle = f32::from_bits(1);
        for b in &boxes {
            if !b.intersects(&left) && !b.intersects(&right) {
                continue;
            }
            obstacle = b.max[1] as f32;
            let center = [0, 1, 2].map(|a| ((b.min[a] + b.max[a]) * 0.5).floor() as i32);
            let mut steps = 1;
            while (steps as f32) < jump_height {
                let above = [center[0], center[1] + steps, center[2]];
                cell.clear();
                w.collision(above[0], above[1], above[2], &mut cell);
                if !cell.is_empty() {
                    let top = cell.iter().fold(f64::NEG_INFINITY, |m, c| m.max(c.max[1]));
                    obstacle = top as f32 + above[1] as f32;
                    if f64::from(obstacle) - self.pos[1] > f64::from(jump_height) {
                        return;
                    }
                }
                if steps > 1 {
                    ceil_y += 1;
                    if !empty(cx, ceil_y, cz, &mut cell) {
                        return;
                    }
                }
                steps += 1;
            }
            break;
        }
        if obstacle != f32::from_bits(1) {
            let dy = (f64::from(obstacle) - self.pos[1]) as f32;
            if dy > 0.5 && dy <= jump_height {
                self.auto_jump_time = 1;
            }
        }
    }

    fn jump_from_ground(&mut self, w: &dyn World, t: &Tables) {
        let factor = self.block_jump_factor(w, t);
        let p = self.attr("minecraft:jump_strength") as f32 * factor + self.jump_boost_power();
        if p <= 1.0e-5 {
            return;
        }
        self.vel[1] = f64::from(p).max(self.vel[1]);
        if self.sprinting {
            let r = self.yaw * DEG;
            self.vel[0] += f64::from(-sin_t(r) * 0.2);
            self.vel[2] += f64::from(cos_t(r) * 0.2);
        }
    }

    /// Jump boost bonus: 0.1 × (amplifier + 1).
    fn jump_boost_power(&self) -> f32 {
        self.effects.jump_boost.map_or(0.0, |a| 0.1 * (a as f32 + 1.0))
    }

    /// Effective gravity: capped at 0.01 when descending under slow falling.
    fn effective_gravity(&self) -> f64 {
        let g = self.attr("minecraft:gravity");
        if self.vel[1] <= 0.0 && self.effects.slow_falling { g.min(0.01) } else { g }
    }

    /// Push towards the nearest free cell when the corner (`x`, `z`) is in a block that
    /// suffocates: speed of 0.1 towards the nearest free face (west, east, north, south).
    fn move_towards_closest_space(&mut self, w: &dyn World, t: &Tables, x: f64, z: f64) {
        let p = [x.floor() as i32, self.pos[1].floor() as i32, z.floor() as i32];
        if !self.suffocates_at(w, t, p[0], p[2]) {
            return;
        }
        let (xd, zd) = (x - f64::from(p[0]), z - f64::from(p[2]));
        let mut best: Option<(usize, i32)> = None;
        let mut closest = f64::MAX;
        // (axis 0 = x, 2 = z; step)
        for (axis, step) in [(0usize, -1i32), (0, 1), (2, -1), (2, 1)] {
            let d = if axis == 0 { xd } else { zd };
            let edge = if step > 0 { 1.0 - d } else { d };
            let (nx, nz) = if axis == 0 { (p[0] + step, p[2]) } else { (p[0], p[2] + step) };
            if edge < closest && !self.suffocates_at(w, t, nx, nz) {
                closest = edge;
                best = Some((axis, step));
            }
        }
        if let Some((axis, step)) = best {
            self.vel[axis] = 0.1 * f64::from(step);
        }
    }

    /// A suffocating block touches the column (`x`, `z`) over the height of the player's box.
    fn suffocates_at(&self, w: &dyn World, t: &Tables, x: i32, z: i32) -> bool {
        let bb = self.bounding_box();
        let test = Aabb {
            min: [f64::from(x), bb.min[1], f64::from(z)],
            max: [f64::from(x) + 1.0, bb.max[1], f64::from(z) + 1.0],
        }
        .deflate(1.0e-7);
        let lo = test.min.map(|v| v.floor() as i32);
        let hi = test.max.map(|v| v.floor() as i32);
        let mut cell = Vec::new();
        for y in lo[1]..=hi[1] {
            for cz in lo[2]..=hi[2] {
                for cx in lo[0]..=hi[0] {
                    let Some(s) = w.state(cx, y, cz) else { continue };
                    if !t.suffocating(s) {
                        continue;
                    }
                    cell.clear();
                    w.collision(cx, y, cz, &mut cell);
                    let o = [f64::from(cx), f64::from(y), f64::from(cz)];
                    if cell.iter().any(|b| b.offset(o[0], o[1], o[2]).intersects(&test)) {
                        return true;
                    }
                }
            }
        }
        false
    }

    /// Acceleration according to the yaw (§ 4.3 step 3).
    fn accelerate(&mut self, sx: f32, fz: f32, a: f32) {
        let (mut ex, mut ez) = (f64::from(sx), f64::from(fz));
        let l2 = ex * ex + ez * ez;
        if l2 < 1.0e-7 {
            return;
        }
        if l2 > 1.0 {
            let l = l2.sqrt();
            ex /= l;
            ez /= l;
        }
        ex *= f64::from(a);
        ez *= f64::from(a);
        let r = self.yaw * DEG;
        let (s, c) = (f64::from(sin_t(r)), f64::from(cos_t(r)));
        self.vel[0] += ex * c - ez * s;
        self.vel[2] += ez * c + ex * s;
    }

    fn corrected(x: f32, m: f64) -> f32 {
        (1.0 - (1.0 - x) * m as f32).clamp(0.0, 1.0)
    }

    fn travel(&mut self, sx: f32, fz: f32, input: Input, w: &dyn World, t: &Tables) {
        // Slow falling or levitation: the fall distance restarts from zero.
        if self.effects.slow_falling || self.effects.levitation.is_some() {
            self.fall_distance = 0.0;
        }
        if (self.water_height > 0.0 || self.lava_height > 0.0) && !self.flying {
            let g = self.effective_gravity();
            return self.travel_fluid(sx, fz, input, w, t, g);
        }
        // Creative flight: the previous vertical speed, × 0.6, replaces that of the movement.
        let fly_vy = self.vel[1];
        if self.fall_flying {
            self.travel_fall_flying(sx, fz, input, w, t);
        } else {
            self.travel_in_air(sx, fz, input, w, t);
        }
        if self.flying {
            self.vel[1] = fly_vy * 0.6;
        }
    }

    /// Gliding (§ elytra): on a ladder, normal movement and end of the flight; otherwise
    /// lift according to the pitch, conversion of the fall into speed, friction 0.99/0.98/0.99.
    fn travel_fall_flying(&mut self, sx: f32, fz: f32, input: Input, w: &dyn World, t: &Tables) {
        if self.on_climbable(w, t) {
            self.travel_in_air(sx, fz, input, w, t);
            self.fall_flying = false;
            return;
        }
        self.vel = self.fall_flying_movement(self.vel);
        self.move_with_collisions(sx, fz, input, w, t);
    }

    /// Speed of the gliding tick from `v`.
    fn fall_flying_movement(&self, mut v: [f64; 3]) -> [f64; 3] {
        let look = view_vector(self.pitch, self.yaw);
        let lean = self.pitch * DEG;
        let look_hor = (look[0] * look[0] + look[2] * look[2]).sqrt();
        let move_hor = (v[0] * v[0] + v[2] * v[2]).sqrt();
        let g = self.effective_gravity();
        let lift = f64::from(lean).cos().powi(2);
        v[1] += g * (-1.0 + lift * 0.75);
        if v[1] < 0.0 && look_hor > 0.0 {
            let c = v[1] * -0.1 * lift;
            v[0] += look[0] * c / look_hor;
            v[1] += c;
            v[2] += look[2] * c / look_hor;
        }
        if lean < 0.0 && look_hor > 0.0 {
            let c = move_hor * f64::from(-sin_t(lean)) * 0.04;
            v[0] += -look[0] * c / look_hor;
            v[1] += c * 3.2;
            v[2] += -look[2] * c / look_hor;
        }
        if look_hor > 0.0 {
            v[0] += (look[0] / look_hor * move_hor - v[0]) * 0.1;
            v[2] += (look[2] / look_hor * move_hor - v[2]) * 0.1;
        }
        [v[0] * f64::from(0.99f32), v[1] * f64::from(0.98f32), v[2] * f64::from(0.99f32)]
    }

    /// Movement in open air or on the ground (§ 4.3), ladders included (§ 7.1).
    fn travel_in_air(&mut self, sx: f32, fz: f32, input: Input, w: &dyn World, t: &Tables) {
        let speed = self.attr("minecraft:movement_speed") as f32;
        // Slowing block, read before the movement (also used for the "chunk loaded" test).
        let fp = self.below(t, w, 0.500_001);
        let fb = if self.on_ground {
            let s = Self::state_at(w, fp);
            Self::corrected(
                t.friction.get(s as usize).copied().unwrap_or(0.6),
                self.attr("minecraft:friction_modifier"),
            )
        } else {
            1.0
        };
        let a = if self.on_ground {
            if f64::from(fb) > 0.6 { speed * (0.216_000_02 / (fb * fb * fb)) } else { speed }
        } else if self.flying {
            if self.sprinting { self.fly_speed * 2.0 } else { self.fly_speed }
        } else if self.sprinting {
            0.025_999_999
        } else {
            0.02
        };
        self.accelerate(sx, fz, a);
        // Ladder (§ 7.1): 0.15 bounds in `f32`; Shift holds back the descent, except in a
        // scaffolding (one goes down by sneaking there).
        if self.on_climbable(w, t) {
            self.fall_distance = 0.0;
            let m = f64::from(0.15f32);
            self.vel[0] = self.vel[0].clamp(-m, m);
            self.vel[2] = self.vel[2].clamp(-m, m);
            self.vel[1] = self.vel[1].max(-m);
            let in_scaffolding = t.is_scaffolding(Self::state_at(w, self.block_pos()));
            if self.vel[1] < 0.0 && !in_scaffolding && input.shift {
                self.vel[1] = 0.0;
            }
        }
        self.move_with_collisions(sx, fz, input, w, t);
        // Ascent: ladder (judged at the new position) or powder snow in leather boots.
        if (self.horizontal_collision || input.jump)
            && (self.on_climbable(w, t) || (self.was_in_powder_snow && self.leather_boots))
        {
            self.vel[1] = 0.2;
        }
        // Gravity (levitation: towards 0.05 × (amplifier + 1) instead); chunk absent:
        // -0.1 above the bottom of the world, 0 below.
        if let Some(a) = self.effects.levitation {
            self.vel[1] += (0.05 * f64::from(a + 1) - self.vel[1]) * 0.2;
        } else if w.state(fp[0], fp[1], fp[2]).is_some() {
            self.vel[1] -= self.effective_gravity();
        } else {
            self.vel[1] = if self.pos[1] > f64::from(w.min_y()) { -0.1 } else { 0.0 };
        }
        // Friction.
        let air_m = self.attr("minecraft:air_drag_modifier");
        let air = Self::corrected(0.91, air_m);
        let h = f64::from(fb * air);
        self.vel[0] *= h;
        self.vel[2] *= h;
        self.vel[1] *= f64::from(Self::corrected(0.98, air_m));
    }

    /// Hooked rocket (its tick): while gliding, the speed tends towards 1.5 × the look.
    fn rocket_boost(&mut self) {
        if !self.fall_flying {
            return;
        }
        let look = view_vector(self.pitch, self.yaw);
        for (v, l) in self.vel.iter_mut().zip(look) {
            *v += l * 0.1 + (l * 1.5 - *v) * 0.5;
        }
    }

    /// Pushes received from neighbouring entities (entity push or the boat's): the local
    /// player moves away from each one, if pushable (neither spectator nor on a ladder).
    fn pushed_by_neighbours(&mut self, w: &dyn World, t: &Tables) {
        if self.nearby.is_empty() || self.spectator || self.on_climbable(w, t) {
            return;
        }
        let bb = self.bounding_box();
        let nearby = std::mem::take(&mut self.nearby);
        for e in &nearby {
            let touching = match e.pusher {
                Pusher::None => false,
                Pusher::Living => e.bb.intersects(&bb),
                Pusher::Boat => {
                    let inflated = Aabb {
                        min: [
                            e.bb.min[0] - f64::from(0.2f32),
                            e.bb.min[1] + 0.01f32 as f64,
                            e.bb.min[2] - f64::from(0.2f32),
                        ],
                        max: [
                            e.bb.max[0] + f64::from(0.2f32),
                            e.bb.max[1] - 0.01f32 as f64,
                            e.bb.max[2] + f64::from(0.2f32),
                        ],
                    };
                    inflated.intersects(&bb) && bb.min[1] <= e.bb.min[1]
                }
            };
            if touching {
                self.push_away_from(e.pos);
            }
        }
        self.nearby = nearby;
    }

    /// Separation from a neighbouring entity at `p`.
    fn push_away_from(&mut self, p: [f64; 3]) {
        let (mut xa, mut za) = (p[0] - self.pos[0], p[2] - self.pos[2]);
        let mut dd = xa.abs().max(za.abs());
        if dd >= f64::from(0.01f32) {
            dd = dd.sqrt();
            xa /= dd;
            za /= dd;
            let pow = (1.0 / dd).min(1.0);
            xa *= pow;
            za *= pow;
            xa *= f64::from(0.05f32);
            za *= f64::from(0.05f32);
            self.push([-xa, 0.0, -za]);
        }
    }

    /// New player after respawn without keeping entity data: gliding,
    /// spin attack, powder snow and posture restart from their initial value.
    pub fn reset_entity_state(&mut self) {
        self.fall_flying = false;
        self.spin_attack = false;
        self.spin_attack_ticks = 0;
        self.in_powder_snow = false;
        self.was_in_powder_snow = false;
        self.pose = Pose::Standing;
    }

    /// Starts gliding if possible: not already gliding,
    /// glider worn, off the ground, without levitation or creative flight, out of water and lava.
    fn try_start_fall_flying(&mut self) -> bool {
        let can_glide = !self.on_ground && self.effects.levitation.is_none() && self.glider && !self.flying;
        let in_liquid = self.water_height > 0.0 || self.lava_height > 0.0;
        if !self.fall_flying && can_glide && !in_liquid {
            self.fall_flying = true;
            true
        } else {
            false
        }
    }

    /// Riptide trident released: momentum of `strength` along the look axis,
    /// 20 ticks of spin attack and, on the ground, a 1.2 movement upwards. `shift`:
    /// Shift held (caution at the edge during this movement).
    pub fn riptide(&mut self, strength: f32, shift: bool, w: &dyn World, t: &Tables) {
        let (yr, xr) = (self.yaw * DEG, self.pitch * DEG);
        let mut xd = -sin_t(yr) * cos_t(xr);
        let mut yd = -sin_t(xr);
        let mut zd = cos_t(yr) * cos_t(xr);
        let dist = (xd * xd + yd * yd + zd * zd).sqrt();
        xd *= strength / dist;
        yd *= strength / dist;
        zd *= strength / dist;
        self.push([f64::from(xd), f64::from(yd), f64::from(zd)]);
        self.spin_attack_ticks = 20;
        self.spin_attack = true;
        if self.on_ground {
            let input = Input { shift, ..Default::default() };
            self.descending = shift;
            self.move_by([0.0, f64::from(1.199_999_9f32), 0.0], 0.0, 0.0, input, w, t);
        }
    }

    fn travel_fluid(&mut self, sx: f32, fz: f32, input: Input, w: &dyn World, t: &Tables, g: f64) {
        let falling = self.vel[1] <= 0.0;
        let y0 = self.pos[1];
        if self.water_height > 0.0 {
            // Swimming: vertical adjustment according to the look (§ 8.3).
            if self.swimming {
                let ly = f64::from(-sin_t(self.pitch * DEG));
                let k = if ly < -0.2 { 0.085 } else { 0.06 };
                let p = self.block_pos();
                let above = Self::state_at(w, [p[0], (self.pos[1] + 0.9).floor() as i32, p[2]]);
                if ly <= 0.0 || input.jump || t.fluids.fluid(above) != Fluid::None {
                    self.vel[1] += (ly - self.vel[1]) * k;
                }
            }
            let mut fh: f32 = if self.sprinting { 0.9 } else { 0.8 };
            let mut a: f32 = 0.02;
            let mut eff = self.attr("minecraft:water_movement_efficiency") as f32;
            if !self.on_ground {
                eff *= 0.5;
            }
            if eff > 0.0 {
                fh += (0.546_000_06 - fh) * eff;
                a += (self.attr("minecraft:movement_speed") as f32 - a) * eff;
            }
            // Dolphin's grace: water friction at 0.96.
            if self.effects.dolphins_grace {
                fh = 0.96;
            }
            self.accelerate(sx, fz, a);
            self.move_with_collisions(sx, fz, input, w, t);
            if self.horizontal_collision && self.on_climbable(w, t) {
                self.vel[1] = 0.2;
            }
            self.vel[0] *= f64::from(fh);
            self.vel[1] *= f64::from(0.8f32);
            self.vel[2] *= f64::from(fh);
            self.fluid_gravity(g, falling);
        } else {
            self.accelerate(sx, fz, 0.02);
            self.move_with_collisions(sx, fz, input, w, t);
            if self.lava_height <= 0.4 {
                self.vel[0] *= 0.5;
                self.vel[1] *= f64::from(0.8f32);
                self.vel[2] *= 0.5;
                self.fluid_gravity(g, falling);
            } else {
                self.vel = self.vel.map(|v| v * 0.5);
            }
            if g != 0.0 {
                self.vel[1] -= g / 4.0;
            }
        }
        // Leaving the fluid.
        if self.horizontal_collision {
            let b = self.bounding_box().offset(
                self.vel[0],
                self.vel[1] + f64::from(0.6f32) - self.pos[1] + y0,
                self.vel[2],
            );
            if is_free(&self.ctx(w, t), &b) && !self.box_has_fluid(w, t, &b) {
                self.vel[1] = f64::from(0.3f32);
            }
        }
    }

    fn fluid_gravity(&mut self, g: f64, falling: bool) {
        if g == 0.0 || self.sprinting {
            return;
        }
        if falling && (self.vel[1] - 0.005).abs() >= 0.003 && (self.vel[1] - g / 16.0).abs() < 0.003 {
            self.vel[1] = -0.003;
        } else {
            self.vel[1] -= g / 16.0;
        }
    }

    fn box_has_fluid(&self, w: &dyn World, t: &Tables, b: &Aabb) -> bool {
        let lo = b.min.map(|v| v.floor() as i32);
        let hi = b.max.map(|v| v.ceil() as i32 - 1);
        (lo[1]..=hi[1]).any(|y| {
            (lo[2]..=hi[2])
                .any(|z| (lo[0]..=hi[0]).any(|x| t.fluids.fluid(Self::state_at(w, [x, y, z])) != Fluid::None))
        })
    }

    /// Height of immersed water and lava, eyes in water (§ 8.1, without current).
    fn update_fluids(&mut self, w: &dyn World, t: &Tables) {
        let eye_y = self.pos[1] + self.eye_height();
        let (water, lava, eyes) =
            scan_fluids(w, t, &self.bounding_box(), self.pos, eye_y, !self.flying, self.fast_lava, &mut self.vel);
        self.water_height = water;
        self.lava_height = lava;
        self.eyes_in_water = eyes;
        if water > 0.0 {
            self.fall_distance = 0.0;
        }
    }

    /// Movement of the velocity with collisions (§ 6.2), flags and speed factor.
    fn move_with_collisions(&mut self, sx: f32, fz: f32, input: Input, w: &dyn World, t: &Tables) {
        self.move_by(self.vel, sx, fz, input, w, t);
    }

    /// Movement `d` with collisions (`move(SELF, d)`).
    fn move_by(&mut self, d: [f64; 3], sx: f32, fz: f32, input: Input, w: &dyn World, t: &Tables) {
        self.move_typed(d, sx, fz, input, w, t, false);
    }

    /// Piston push (`move(PISTON, d)`) at game tick `time`: cap of 0.51 per axis and
    /// per tick, blocking (cobweb…) cancelled without reducing the push, no caution at the edge; then
    /// effects of the blocks crossed by this movement alone, which is not kept for the tick
    /// (the last one recorded is removed, even if it is not its own, like the game). `w` must
    /// ignore the pushing block in its direction (`noclip`).
    pub fn piston_move(&mut self, d: [f64; 3], time: i64, w: &dyn World, t: &Tables) {
        let input = Input { shift: self.descending, ..Input::default() };
        let from = self.pos;
        // Spectator (no physics): moved as is, without a cap.
        let d = if self.spectator { d } else { self.piston_deltas.limit(time, d) };
        // Push capped to zero: no movement, but block effects (zero segment) and
        // removal of the last one recorded all the same, like the game.
        if self.spectator || d != [0.0; 3] {
            self.move_typed(d, 0.0, 0.0, input, w, t, true);
        }
        self.moves.pop();
        let saved = std::mem::replace(&mut self.moves, vec![(from, self.pos, [0.0; 3])]);
        self.block_effects(input, w, t);
        self.moves = saved;
    }

    #[allow(clippy::too_many_arguments)]
    fn move_typed(&mut self, d: [f64; 3], sx: f32, fz: f32, input: Input, w: &dyn World, t: &Tables, piston: bool) {
        let before = self.pos;
        let mut d = d;
        // Spectator: passes through everything, collision flags false, nothing else (§ 6.2, step 0).
        if self.spectator {
            for (p, v) in self.pos.iter_mut().zip(d) {
                *p += v;
            }
            self.horizontal_collision = false;
            self.vertical_collision = false;
            self.minor_collision = false;
            return;
        }
        if l2(self.stuck) > 1.0e-7 {
            if !piston {
                for (v, k) in d.iter_mut().zip(self.stuck) {
                    *v *= k;
                }
            }
            self.stuck = [0.0; 3];
            self.vel = [0.0; 3];
        }
        let step = self.attr("minecraft:step_height");
        // Collision searches with the player's context (scaffolding, powder snow).
        let cw = self.ctx(w, t);
        // Caution at the edge (§ 6.4), not for a piston push.
        if !piston && !self.flying && d[1] <= 0.0 && input.shift && self.above_ground(&cw, step) {
            d = self.back_off_edge(d, &cw, step);
        }
        let start = self.bounding_box();
        let m = collide::resolve(&cw, &start, d, self.on_ground, step);
        if l2(m) > 1.0e-7 || l2(d) - l2(m) < 1.0e-7 {
            let from = self.pos;
            for (p, v) in self.pos.iter_mut().zip(m) {
                *p += v;
            }
            if self.moves.len() >= 100 {
                let second = self.moves.remove(1);
                self.moves[0].1 = second.1;
            }
            self.moves.push((from, self.pos, d));
        }
        let cx = (d[0] - m[0]).abs() >= 1.0e-5;
        let cz = (d[2] - m[2]).abs() >= 1.0e-5;
        self.horizontal_collision = cx || cz;
        self.vertical_collision = d[1] != m[1];
        self.on_ground = self.vertical_collision && d[1] < 0.0;
        self.update_support(&self.ctx(w, t));
        self.minor_collision = self.horizontal_collision && {
            let r = self.yaw * DEG;
            let (s, c) = (sin_t(r), cos_t(r));
            let (gx, gz) = (f64::from(sx * c - fz * s), f64::from(fz * c + sx * s));
            let (g2, m2) = (gx * gx + gz * gz, m[0] * m[0] + m[2] * m[2]);
            if g2 >= f64::from(1.0e-5f32) && m2 >= f64::from(1.0e-5f32) {
                let cos = (gx * m[0] + gz * m[2]) / (g2.sqrt() * m2.sqrt());
                cos.clamp(-1.0, 1.0).acos() < f64::from(0.139_626_34f32)
            } else {
                false
            }
        };
        // Fall.
        if self.on_ground {
            self.fall_distance = 0.0;
        } else if m[1] < 0.0 {
            self.fall_distance -= m[1];
        }
        // Stop on the blocked axis and bounces (§ 6.7).
        if (d[1] != 0.0 && self.vertical_collision) || self.horizontal_collision {
            self.restitute(cx, cz, m, input, w, t);
        }
        // Speed factor of the block (§ 4.4): none in flight nor while gliding.
        if !self.flying && !self.fall_flying {
            // Occupied block, otherwise (outside a water block and bubble column) the slowing one.
            let here = Self::state_at(w, self.block_pos());
            let mut f = t.speed_factor.get(here as usize).copied().unwrap_or(1.0);
            if !t.speed_factor_here_only(here) && f == 1.0 {
                let b = Self::state_at(w, self.below(t, w, 0.500_001));
                f = t.speed_factor.get(b as usize).copied().unwrap_or(1.0);
            }
            let eff = self.attr("minecraft:movement_efficiency") as f32;
            let f = f + (1.0 - f) * eff;
            self.vel[0] *= f64::from(f);
            self.vel[2] *= f64::from(f);
        }
        self.update_auto_jump(before, input, w, t);
    }

    fn restitute(&mut self, cx: bool, cz: bool, m: [f64; 3], input: Input, w: &dyn World, t: &Tables) {
        let v = self.vel;
        let mut r = if input.shift { 0.0 } else { self.attr("minecraft:bounciness") };
        if cx {
            self.vel[0] = -v[0] * r;
        }
        if cz {
            self.vel[2] = -v[2] * r;
        }
        if self.vertical_collision {
            let g = self.effective_gravity();
            if self.on_ground {
                let s = Self::state_at(w, self.below(t, w, 0.2));
                r = if -v[1] > g && !input.shift && !t.suppresses_bounce(s) {
                    r.max(f64::from(t.bounce.get(s as usize).copied().unwrap_or(0.0)))
                } else {
                    0.0
                };
            }
            let (gc, drag) = if r > 0.0 {
                let p = m[1] / v[1];
                let air = f64::from(Self::corrected(0.98, self.attr("minecraft:air_drag_modifier")));
                (p * g, 1.0 + p * (air - 1.0))
            } else {
                (0.0, 1.0)
            };
            self.vel[1] = (gc - v[1]) * drag * r;
        }
    }

    /// Effects of the blocks touched during the tick (§ 7.2): walking on slime, then crossed
    /// blocks (cobweb, berries, powder snow, honey, bubble columns).
    fn block_effects(&mut self, input: Input, w: &dyn World, t: &Tables) {
        // Spectator: insensitive to blocks (no physics).
        if self.spectator {
            self.moves.clear();
            return;
        }
        if self.on_ground {
            let s = Self::state_at(w, self.below(t, w, 0.2));
            let vy = self.vel[1].abs();
            if t.inside(s) == inside::SLIME && vy < 0.1 && !input.shift {
                let k = 0.4 + vy * 0.2;
                self.vel[0] *= k;
                self.vel[2] *= k;
            }
        }
        let mut moves = std::mem::take(&mut self.moves);
        match moves.last() {
            None => moves.push((self.old_pos, self.pos, [0.0; 3])),
            Some(l) if l2(sub(l.1, self.pos)) > f64::from(9.999_999_4e-11f32) => moves.push((l.1, self.pos, [0.0; 3])),
            _ => {}
        }
        let mut seen: Vec<[i32; 3]> = Vec::new();
        for &(from, to, d) in &moves {
            if l2(d) > 0.0 {
                // Axis by axis: Y, then X and Z, the smaller of the two last.
                let order = if d[0].abs() < d[2].abs() { [1, 2, 0] } else { [1, 0, 2] };
                let travel = sub(to, from);
                let mut p = from;
                for a in order {
                    if travel[a] != 0.0 {
                        let mut q = p;
                        q[a] += travel[a];
                        self.inside_between(p, q, &mut seen, w, t);
                        p = q;
                    }
                }
            } else {
                self.inside_between(from, to, &mut seen, w, t);
            }
        }
        moves.clear();
        self.moves = moves;
    }

    /// Blocks swept by the box (shrunk by 1e-5) between `from` and `to`. The segments
    /// follow a single axis: the envelope of the two boxes is exactly the swept zone.
    fn inside_between(&mut self, from: [f64; 3], to: [f64; 3], seen: &mut Vec<[i32; 3]>, w: &dyn World, t: &Tables) {
        let (wd, h) = self.dimensions();
        let e = f64::from(1.0e-5f32);
        let r = wd / 2.0;
        let at = |p: [f64; 3]| Aabb {
            min: [p[0] - r + e, p[1] + e, p[2] - r + e],
            max: [p[0] + r - e, p[1] + h - e, p[2] + r - e],
        };
        let (a, b) = (at(from), at(to));
        let lo: [i32; 3] = std::array::from_fn(|i| a.min[i].min(b.min[i]).floor() as i32);
        let hi: [i32; 3] = std::array::from_fn(|i| a.max[i].max(b.max[i]).floor() as i32);
        let far = l2(sub(to, from)) > 0.999_990_000_000_252_6f64.powi(2);
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    let s = Self::state_at(w, [x, y, z]);
                    let k = t.inside(s);
                    if k == inside::NONE || k == inside::SLIME || seen.contains(&[x, y, z]) {
                        continue;
                    }
                    seen.push([x, y, z]);
                    let (fx, fy, fz) = (f64::from(x), f64::from(y), f64::from(z));
                    let cell = Aabb { min: [fx, fy, fz], max: [fx + 1.0, fy + 1.0, fz + 1.0] };
                    self.inside_effect(k, [x, y, z], far || b.intersects(&cell), w, t);
                }
            }
        }
    }

    fn inside_effect(&mut self, k: u8, c: [i32; 3], precise: bool, w: &dyn World, t: &Tables) {
        let stuck = |p: &mut Self, m: [f64; 3]| {
            if !p.flying {
                p.fall_distance = 0.0;
                p.stuck = m;
            }
        };
        match k {
            inside::WEB => stuck(self, [0.25, f64::from(0.05f32), 0.25]),
            inside::BERRY_BUSH => stuck(self, [f64::from(0.8f32), 0.75, f64::from(0.8f32)]),
            inside::POWDER_SNOW => {
                // Freeze: in powder snow this tick (used for climbing in leather boots).
                self.in_powder_snow = true;
                if t.inside(Self::state_at(w, self.block_pos())) == inside::POWDER_SNOW {
                    stuck(self, [f64::from(0.9f32), 1.5, f64::from(0.9f32)]);
                }
            }
            inside::HONEY => {
                let w = self.dimensions().0 as f32;
                if let Some(v) = honey_slide(self.pos, self.vel, self.on_ground, w, c) {
                    self.vel = v;
                    self.fall_distance = 0.0;
                }
            }
            inside::BUBBLE_COLUMN | BUBBLES_DOWN if precise && !self.flying => {
                let down = k == BUBBLES_DOWN;
                let up = Self::state_at(w, [c[0], c[1] + 1, c[2]]);
                let mut shape = Vec::new();
                t.collision(up, &mut shape);
                let open = shape.is_empty() && t.fluids.fluid(up) == Fluid::None;
                let vy = self.vel[1];
                self.vel[1] = match (open, down) {
                    (true, true) => (vy - 0.03).max(-0.9),
                    (true, false) => (vy + 0.1).min(1.8),
                    (false, true) => (vy - 0.03).max(-0.3),
                    (false, false) => (vy + 0.06).min(0.7),
                };
            }
            _ => {}
        }
    }

    fn above_ground(&self, w: &dyn World, step: f64) -> bool {
        self.on_ground || (self.fall_distance < step && !self.can_fall(w, 0.0, 0.0, step - self.fall_distance))
    }

    fn can_fall(&self, w: &dyn World, dx: f64, dz: f64, h: f64) -> bool {
        let b = self.bounding_box();
        let probe = Aabb {
            min: [b.min[0] + 1.0e-7 + dx, b.min[1] - h - 1.0e-7, b.min[2] + 1.0e-7 + dz],
            max: [b.max[0] - 1.0e-7 + dx, b.min[1], b.max[2] - 1.0e-7 + dz],
        };
        is_free(w, &probe)
    }

    fn back_off_edge(&self, d: [f64; 3], w: &dyn World, step: f64) -> [f64; 3] {
        const S: f64 = 0.05;
        let (mut dx, mut dz) = (d[0], d[2]);
        let shrink = |v: &mut f64, sign: f64| {
            if v.abs() <= S {
                *v = 0.0;
            } else {
                *v -= sign * S;
            }
        };
        while dx != 0.0 && self.can_fall(w, dx, 0.0, step) {
            shrink(&mut dx, d[0].signum());
        }
        while dz != 0.0 && self.can_fall(w, 0.0, dz, step) {
            shrink(&mut dz, d[2].signum());
        }
        while dx != 0.0 && dz != 0.0 && self.can_fall(w, dx, dz, step) {
            shrink(&mut dx, d[0].signum());
            shrink(&mut dz, d[2].signum());
        }
        [dx, d[1], dz]
    }

    /// Main supporting block (§ 6.5, without the second offset search).
    fn update_support(&mut self, w: &dyn World) {
        self.support = if self.on_ground { support_of(w, &self.bounding_box(), self.pos) } else { None };
    }
}

#[cfg(test)]
mod opt_tables_tests;
#[cfg(test)]
mod tests;
