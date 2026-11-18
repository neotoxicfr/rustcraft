//! Speleothems of the integrated server (G6-F09a): pointed dripstone and sulfur spike, which
//! share their rules (tag `speleothems`).
//!
//! - Placement: the tip points away from the vertical gaze (looking up: tip down), otherwise
//!   the other way, otherwise no placement. The support is the sturdy face of the block behind
//!   the base, or the same block turned the same way. Thickness from the vertical neighbours;
//!   two tips facing each other merge, unless the player is sneaking.
//! - Shape: only vertical updates count. Support lost: a tick is scheduled (2 for a
//!   stalactite, 1 for a stalagmite), a stalactite already scheduled no longer moves;
//!   otherwise the thickness is recomputed (a merge stays a merge as long as it holds).
//! - Tick: a stalagmite without support is broken with its loot; a stalactite falls, along with
//!   every stalactite piece below it; the tip hurts on arrival
//!   (max(1 + height, 6) per block of fall, 40 at most).
//! - Random tick: 0.011377778 chance (five days per growth on average) at the top of a
//!   stalactite attached under its feeder block: the free tip (neither waterlogged nor merged)
//!   lengthens, or a stalagmite grows below (10 blocks searched). Sulfur spike: under
//!   sulfur, 2 blocks at most. Pointed dripstone: under the speleothem block, 7 blocks, with a
//!   source of water two blocks above; its stalagmite search stops at whatever would hold back a drop.
//! - A trident thrown faster than 0.6 block per tick breaks either one (`hits.rs`).

use rc_data::PropKey;

use super::{Dir, P, above, add, below, flag, rel};
use super::rules::*;
use crate::server::Server;
use crate::tag_bits::TagId;

const THICK: PropKey = PropKey::Thickness;
const TIP_DIR: PropKey = PropKey::VerticalDirection;
/// Chance of growing per random tick.
const GROWTH_CHANCE: f32 = 0.011_377_778;
/// Blocks walked below the tip to find where to grow a stalagmite.
const STALAGMITE_SCAN: i32 = 10;
/// Maximum damage of a falling tip.
const FALL_HURT_MAX: i32 = 40;
/// Column (x and z from 6 to 10 sixteenths) that a drop must find free to pass.
const DRIP_MIN: f64 = 6.0 / 16.0;
const DRIP_MAX: f64 = 10.0 / 16.0;
/// Chances of a fluid transfer per random tick (water, lava).
const DRIP_WATER: f32 = 0.175_781_25;
const DRIP_LAVA: f32 = 0.058_593_75;
/// Walks of a drop: from a stalactite to its root, from the tip to a cauldron and back (one
/// less cell than this count).
const DRIP_SCAN: i32 = 11;

impl Server {

    /// Shape update (the water tick is scheduled beforehand, for every block).
    pub(crate) fn speleothem_update_shape(&mut self, s: u32, p: P, dir: Dir) -> u32 {
        if dir.axis() != 1 {
            return s;
        }
        let v = self.view();
        let block = v.block(s);
        let tip = v.get_dir(s, TIP_DIR).unwrap_or(Dir::Up);
        if tip == Dir::Down && self.has_scheduled_tick(p, block) {
            return s;
        }
        if dir == tip.opposite() && !self.spel_valid_at(p, block, tip) {
            self.schedule_tick(p, block, if tip == Dir::Down { 2 } else { 1 });
            return s;
        }
        let merge = v.get(s, THICK) == Some("tip_merge");
        let th = self.spel_thickness(p, block, tip, merge);
        v.set(s, THICK, th)
    }

    /// Scheduled tick: unsupported stalagmite broken, otherwise the stalactite falls.
    pub(crate) fn speleothem_tick(&mut self, s: u32, p: P) {
        if self.spel_with_dir(s, Dir::Up) && !self.speleothem_survives(s, p) {
            self.destroy_block(p, true, 512);
            return;
        }
        // Chain of stalactite falling downwards; the tip hurts.
        let (mut q, mut st) = (p, s);
        while self.spel_with_dir(st, Dir::Down) {
            self.spawn_falling_block(q, st);
            if self.spel_is_tip(st, true) {
                let size = (1 + p[1] - q[1]).max(6);
                if let Some(e) = self.bs.entities.list.last_mut() {
                    e.hurt = Some((size as f32, FALL_HURT_MAX));
                }
                break;
            }
            q = below(q);
            st = self.get(q);
        }
    }

    /// Random tick. Pointed dripstone first draws the random value of its fluid transfer, then
    /// growth draws its own.
    pub(crate) fn speleothem_random_tick(&mut self, s: u32, p: P, dripstone: bool) {
        if dripstone {
            let r = self.bs.rng.next_float();
            self.dripstone_transfer(s, p, r);
        }
        if self.bs.rng.next_float() < GROWTH_CHANCE
            && self.spel_with_dir(s, Dir::Down)
            && self.view().block(self.get(above(p))) != self.view().block(s)
        {
            self.speleothem_grow(s, p, dripstone);
        }
    }

    fn speleothem_grow(&mut self, s: u32, p: P, dripstone: bool) {
        let v = self.view();
        let block = v.block(s);
        let t = &self.bs.t.id;
        let feeder = if dripstone { t.dripstone_block } else { t.sulfur };
        if v.block(self.get(above(p))) != feeder {
            return;
        }
        if dripstone {
            let f = self.fluid_at(add(p, 0, 2, 0));
            if !(f.kind == 1 && f.source) {
                return;
            }
        }
        let max_len = if dripstone { 7 } else { 2 };
        let Some(tip) = self.spel_find_tip(s, p, max_len) else { return };
        let ts = self.get(tip);
        let free = self.spel_with_dir(ts, Dir::Down)
            && v.get(ts, THICK) == Some("tip")
            && !v.get_bool(ts, PropKey::Waterlogged);
        if free && self.spel_tip_can_grow(ts, tip, block) {
            if self.bs.rng.next_bool() {
                self.spel_grow_into(tip, Dir::Down, block);
            } else {
                self.spel_grow_stalagmite_below(tip, block, dripstone);
            }
        }
    }

    /// Tip (unmerged) at the end of the speleothem starting at `p`, within `max` steps.
    fn spel_find_tip(&self, s: u32, p: P, max: i32) -> Option<P> {
        if self.spel_is_tip(s, false) {
            return Some(p);
        }
        let v = self.view();
        let d = v.get_dir(s, TIP_DIR)?;
        let block = v.block(s);
        let mut q = p;
        for _ in 1..max {
            q = rel(q, d);
            let st = self.get(q);
            if self.spel_is_tip(st, false) {
                return Some(q);
            }
            if !self.in_build_height(q[1]) || v.block(st) != block || v.get_dir(st, TIP_DIR) != Some(d) {
                return None;
            }
        }
        None
    }

    /// Can the tip lengthen (free block with no fluid, or opposite tip to merge with)?
    fn spel_tip_can_grow(&self, ts: u32, tip: P, block: u16) -> bool {
        let Some(d) = self.view().get_dir(ts, TIP_DIR) else { return false };
        let st = self.get(rel(tip, d));
        if !self.fluid_of(st).is_empty() {
            return false;
        }
        self.view().is_air(st) || self.spel_unmerged_tip(st, d.opposite(), block)
    }

    fn spel_grow_into(&mut self, from: P, d: Dir, block: u16) {
        let q = rel(from, d);
        let st = self.get(q);
        if self.spel_unmerged_tip(st, d.opposite(), block) {
            let (top, bottom) =
                if self.view().get_dir(st, TIP_DIR) == Some(Dir::Up) { (above(q), q) } else { (q, below(q)) };
            self.spel_create(top, Dir::Down, "tip_merge", block);
            self.spel_create(bottom, Dir::Up, "tip_merge", block);
        } else if self.view().is_air(st) || self.view().block(st) == self.bs.t.id.water {
            self.spel_create(q, d, "tip", block);
        }
    }

    fn spel_create(&mut self, q: P, d: Dir, th: &str, block: u16) {
        let v = self.view();
        let f = self.fluid_at(q);
        let s = v.set(v.set_dir(v.default_state(block), TIP_DIR, d), THICK, th);
        let s = v.set_bool(s, PropKey::Waterlogged, f.kind == 1 && f.source);
        self.set_block(q, s, flag::ALL);
    }

    /// Stalagmite growing below the tip at `tip`: the one already there lengthens, otherwise
    /// a new one is born on the first solid ground outside the water.
    fn spel_grow_stalagmite_below(&mut self, tip: P, block: u16, dripstone: bool) {
        let mut q = tip;
        for _ in 0..STALAGMITE_SCAN {
            q = below(q);
            let st = self.get(q);
            if !self.fluid_of(st).is_empty() {
                return;
            }
            if self.spel_unmerged_tip(st, Dir::Up, block) && self.spel_tip_can_grow(st, q, block) {
                self.spel_grow_into(q, Dir::Up, block);
                return;
            }
            if self.spel_valid_at(q, block, Dir::Up) && self.fluid_at(below(q)).kind != 1 {
                self.spel_grow_into(below(q), Dir::Up, block);
                return;
            }
            if dripstone && !self.drip_passes(st) {
                return;
            }
        }
    }

    // --- Fluid transfer of pointed dripstone (G6-F08). ---

    /// Fluid feeding a stalactite: walked up from `p` (same block, pointing down) to the first
    /// other block (the root), then the cell above it: mud gives water (unless water
    /// evaporates in this dimension), otherwise its fluid if it is a source. Returns (cell,
    /// fluid: 0 none, 1 water, 2 lava, its state); `None` if `s` is not a stalactite or the
    /// root is not found within 10 blocks.
    fn drip_source(&self, s: u32, p: P) -> Option<(P, u8, u32)> {
        if !self.spel_with_dir(s, Dir::Down) {
            return None;
        }
        let v = self.view();
        let block = v.block(s);
        let mut q = p;
        for _ in 1..DRIP_SCAN {
            q = above(q);
            let st = self.get(q);
            if v.block(st) != block {
                let a = above(q);
                let at = self.get(a);
                let fluid = if v.block(at) == self.bs.t.id.mud && self.dim.name != "minecraft:the_nether" {
                    1
                } else {
                    match self.fluid_of(at) {
                        f if f.source && f.kind == 1 => 1,
                        f if f.source && f.kind == 2 => 2,
                        _ => 0,
                    }
                };
                return Some((a, fluid, at));
            }
            if !self.in_build_height(q[1]) || v.get_dir(st, TIP_DIR) != Some(Dir::Down) {
                return None;
            }
        }
        None
    }

    /// The cauldron state `s` takes a drop of `fluid` (empty: water or lava; water cauldron:
    /// water, even when full).
    fn cauldron_takes_drip(&self, s: u32, fluid: u8) -> bool {
        let (b, id) = (self.view().block(s), &self.bs.t.id);
        (b == id.cauldron && fluid != 0) || (b == id.water_cauldron && fluid == 1)
    }

    /// First cell from `from` towards `d` (10 blocks at most) whose state satisfies `target`,
    /// through cells a drop passes; `None` outside the world or at a blocking cell.
    fn drip_walk(&self, from: P, d: Dir, target: impl Fn(u32) -> bool) -> Option<P> {
        let mut q = from;
        for _ in 1..DRIP_SCAN {
            q = rel(q, d);
            let st = self.get(q);
            if target(st) {
                return Some(q);
            }
            if !self.in_build_height(q[1]) || !self.drip_passes(st) {
                return None;
            }
        }
        None
    }

    /// Fluid transfer at the random tick (`r`: its draw): at the top of a stalactite, below
    /// 0.17578125 for water or 0.05859375 for lava, the tip (10 blocks at most) drips. Mud above
    /// the root dries into clay (entities raised, "block changed" with the clay); otherwise a
    /// cauldron that takes the fluid below the tip gets a tick in 50 + the fall height. Event
    /// 1504 at the tip in both cases.
    fn dripstone_transfer(&mut self, s: u32, p: P, r: f32) {
        if r > DRIP_WATER || self.view().block(self.get(above(p))) == self.view().block(s) {
            return;
        }
        let Some((src, fluid, src_state)) = self.drip_source(s, p) else { return };
        let chance = match fluid {
            1 => DRIP_WATER,
            2 => DRIP_LAVA,
            _ => return,
        };
        if r >= chance {
            return;
        }
        let Some(tip) = self.spel_find_tip(s, p, DRIP_SCAN) else { return };
        if fluid == 1 && self.view().block(src_state) == self.bs.t.id.mud {
            let clay = self.view().default_state(self.bs.t.id.clay);
            self.set_update(src, clay);
            // Gained part: from the top of the mud's collision shape to the full block.
            let mud_shape = self.data.shape(self.view().info(src_state).collision);
            let mud_top = mud_shape.iter().map(|b| b.max[1]).fold(0.0, f64::max);
            self.push_entities_up(src, mud_top, 1.0);
            self.game_event_block("minecraft:block_change", src, None, Some(clay));
            self.level_event(1504, tip, 0);
        } else if let Some(c) = self.drip_walk(tip, Dir::Down, |st| self.cauldron_takes_drip(st, fluid)) {
            self.level_event(1504, tip, 0);
            let cb = self.view().block(self.get(c));
            self.schedule_tick(c, cb, 50 + tip[1] - c[1]);
        }
    }

    /// Scheduled tick of an empty or water cauldron (the drop arrives): the free-hanging tip
    /// above (10 blocks at most, through cells a drop passes) fed by water or lava fills it. Empty:
    /// water cauldron (1047) or lava cauldron (1046); water cauldron: one level more unless
    /// full (1047). "Block changed" with the new state.
    pub(crate) fn cauldron_drip_tick(&mut self, s: u32, p: P) {
        let free_tip = |st: u32| {
            let v = self.view();
            self.spel_with_dir(st, Dir::Down) && v.get(st, THICK) == Some("tip") && !v.get_bool(st, PropKey::Waterlogged)
        };
        let Some(tip) = self.drip_walk(p, Dir::Up, free_tip) else { return };
        let fluid = match self.drip_source(self.get(tip), tip) {
            Some((_, f @ (1 | 2), _)) => f,
            _ => return,
        };
        if !self.cauldron_takes_drip(s, fluid) {
            return;
        }
        let v = self.view();
        let (ns, event) = if v.block(s) == self.bs.t.id.cauldron {
            if fluid == 1 {
                (v.default_state(self.bs.t.id.water_cauldron), 1047)
            } else {
                (self.default_state("minecraft:lava_cauldron"), 1046)
            }
        } else {
            let level = v.get_int(s, PropKey::Level);
            if level >= 3 {
                return;
            }
            (v.set_int(s, PropKey::Level, level + 1), 1047)
        };
        self.set_update(p, ns);
        self.game_event_block("minecraft:block_change", p, None, Some(ns));
        self.level_event(event, p, 0);
    }

    /// Does a drop pass through this state? Air yes; opaque block or fluid no; otherwise
    /// only if its collision shape leaves the central column free.
    fn drip_passes(&self, s: u32) -> bool {
        let v = self.view();
        if v.is_air(s) {
            return true;
        }
        if v.info(s).has(rc_data::flags::SOLID_RENDER) || !self.fluid_of(s).is_empty() {
            return false;
        }
        !self.data.shape(v.info(s).collision).iter().any(|b| {
            b.min[0] < DRIP_MAX && b.max[0] > DRIP_MIN && b.min[2] < DRIP_MAX && b.max[2] > DRIP_MIN && b.min[1] < 1.0 && b.max[1] > 0.0
        })
    }
}

/// Survival and placement rules of speleothems, shared by the server and the client's
/// placement prediction (one source, read through [`BlockReader`]).
pub(crate) trait SpeleothemRules: BlockReader {
    /// Speleothem (either block) whose tip points towards `d`.
    fn spel_with_dir(&self, s: u32, d: Dir) -> bool {
        let v = self.view();
        self.tag_id(TagId::Speleothems, v.block(s)) && v.get_dir(s, TIP_DIR) == Some(d)
    }

    /// Tip (merged included if `merged`).
    fn spel_is_tip(&self, s: u32, merged: bool) -> bool {
        let v = self.view();
        if !self.tag_id(TagId::Speleothems, v.block(s)) {
            return false;
        }
        match v.get(s, THICK) {
            Some("tip") => true,
            Some("tip_merge") => merged,
            _ => false,
        }
    }

    /// Unmerged tip of block `block`, pointing towards `d`.
    fn spel_unmerged_tip(&self, s: u32, d: Dir, block: u16) -> bool {
        self.spel_is_tip(s, false) && self.view().get_dir(s, TIP_DIR) == Some(d) && self.view().block(s) == block
    }

    /// Does block `block` hold at `p` with the tip towards `tip`?
    fn spel_valid_at(&self, p: P, block: u16, tip: Dir) -> bool {
        let b = self.get(rel(p, tip.opposite()));
        self.view().sturdy(b, tip) || (self.spel_with_dir(b, tip) && self.view().block(b) == block)
    }

    /// Survival of a placed state (shape and placement rule).
    fn speleothem_survives(&self, s: u32, p: P) -> bool {
        let v = self.view();
        self.spel_valid_at(p, v.block(s), v.get_dir(s, TIP_DIR).unwrap_or(Dir::Up))
    }

    /// Thickness at `p` from the block in front of the tip and the one behind the base.
    fn spel_thickness(&self, p: P, block: u16, tip: Dir, merge: bool) -> &'static str {
        let v = self.view();
        let base = tip.opposite();
        let front = self.get(rel(p, tip));
        if self.spel_with_dir(front, base) && v.block(front) == block {
            return if !merge && v.get(front, THICK) != Some("tip_merge") { "tip" } else { "tip_merge" };
        }
        if !self.spel_with_dir(front, tip) {
            return "tip";
        }
        match v.get(front, THICK) {
            Some("tip" | "tip_merge") => "frustum",
            _ => {
                if self.spel_with_dir(self.get(rel(p, base)), tip) {
                    "middle"
                } else {
                    "base"
                }
            }
        }
    }

    /// Placement state (`None`: no direction holds). `water`: source water at the position.
    fn speleothem_placement(&self, def: u32, p: P, pitch: f32, sneaking: bool, water: bool) -> Option<u32> {
        let v = self.view();
        let block = v.block(def);
        let first = if pitch < 0.0 { Dir::Down } else { Dir::Up };
        let tip = [first, first.opposite()].into_iter().find(|&d| self.spel_valid_at(p, block, d))?;
        let th = self.spel_thickness(p, block, tip, !sneaking);
        let s = v.set(v.set_dir(def, TIP_DIR, tip), THICK, th);
        Some(v.set_bool(s, PropKey::Waterlogged, water))
    }
}

impl<W: BlockReader + ?Sized> SpeleothemRules for W {}
