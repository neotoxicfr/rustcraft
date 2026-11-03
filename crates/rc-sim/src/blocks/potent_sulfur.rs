//! Active sulfur of the integrated server (G6-F09a).
//!
//! State (`potent_sulfur_state`) recomputed at placement and on every shape update: dry without
//! a source of water right above; continuous over a source of the tag `causes_continuous_geyser_eruptions`
//! (lava); over the tag `causes_periodic_geyser_eruptions` (magma), dormant (the countdown
//! restarts when it was neither dormant nor erupting) or erupting if it already was; wet
//! otherwise. Placed erupting or continuous: block event 0/0 (the client times the eruption), start
//! sound, game event `block_activate`.
//!
//! The block entity ticks outside the dry state (ticker removed in the dry state, recreated at the end
//! of the list when the water comes back, kept in place from one wet state to another):
//! - wet: nausea;
//! - dormant: countdown then nausea;
//! - erupting: launch then countdown;
//! - continuous: launch.
//!
//! The gas source is the first block above that is no longer source water (4 blocks of water
//! at most) and that lets the geyser through (air, water, empty collision shape). Nausea (every
//! 10 ticks): 80 ticks, ambient, to living beings whose box touches the source widened by
//! 2.5 horizontally, eyes in a free block at most 3 blocks from the center of the source,
//! above a source of water seen without obstacle from the water under the source. Countdown
//! (every 20 ticks, NBT `countdown`, -1 at the start) drawn from a positional random (world
//! seed mixed with a salt): 10 × (water − 1) + 15..30 before the eruption, water − 1 + 1..2 during; at
//! zero the state flips (end of eruption: `block_deactivate` event). Launch (every tick):
//! in the free column above (6 blocks per block of water at most), +0.2 in y to entities that
//! rise slower than 0.3 + 0.1 per block of water, except passengers, mounts piloted by a
//! player and types of the tag `not_affected_by_geysers`; the fall distance is brought down to 1 as long
//! as the entity does not descend faster than 0.5.

use rc_physics::Aabb;

use super::level::BeTick;
use super::{Kind, P, above, below, flag};
use crate::mobs::mob::Ent;
use crate::server::Server;
use crate::tag_bits::TagId;

const STATE: &str = "potent_sulfur_state";
/// Blocks of water at most between the block and the gas source.
const WATER_ABOVE_MAX: i32 = 4;
/// Salt mixed into the world seed for the countdown random draw.
const GEYSER_SALT: i64 = -904_011_478;
const NAUSEA_TICKS: i32 = 80;
const COUNTDOWN: &str = "countdown";

fn center(p: P) -> [f64; 3] {
    [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5]
}

impl Server {
    /// State from the water above and the block below, and "the countdown restarts"
    /// (the block becomes dormant without having been dormant or erupting).
    pub(crate) fn potent_sulfur_state(&self, s: u32, p: P) -> (u32, bool) {
        let v = self.view();
        let up = self.fluid_at(above(p));
        if !(up.kind == 1 && up.source) {
            return (v.set(s, STATE, "dry"), false);
        }
        let b = self.get(below(p));
        let bf = self.fluid_of(b);
        let source_if_fluid = bf.is_empty() || bf.source;
        if source_if_fluid && self.tag_id(TagId::CausesContinuousGeyserEruptions, v.block(b)) {
            return (v.set(s, STATE, "continuous"), false);
        }
        if source_if_fluid && self.tag_id(TagId::CausesPeriodicGeyserEruptions, v.block(b)) {
            let cur = v.get(s, STATE);
            let reset = !matches!(cur, Some("erupting" | "dormant"));
            return (if cur == Some("erupting") { s } else { v.set(s, STATE, "dormant") }, reset);
        }
        (v.set(s, STATE, "wet"), false)
    }

    /// Shape update: new state, block entity countdown reset to -1 when needed.
    pub(crate) fn potent_sulfur_update_shape(&mut self, s: u32, p: P) -> u32 {
        let (ns, reset) = self.potent_sulfur_state(s, p);
        if reset {
            self.be_set_int(p, COUNTDOWN, -1);
        }
        ns
    }

    /// Reaction to placement (any state change included).
    pub(crate) fn potent_sulfur_on_place(&mut self, s: u32, p: P) {
        let v = self.view();
        let sound = match v.get(s, STATE) {
            Some("erupting") => "minecraft:block.potent_sulfur.geyser_eruption",
            Some("continuous") => "minecraft:block.potent_sulfur.geyser_continuous_eruption",
            _ => return,
        };
        self.block_event(p, v.block(s), 0, 0);
        self.play_sound(sound, 4, center(p), 1.0, 1.0);
        self.game_event_block("minecraft:block_activate", p, None, Some(s));
    }

    /// Ticker according to the placed state (block entity created, reloaded or state changed).
    pub(crate) fn potent_sulfur_ticker_sync(&mut self, p: P, s: u32) {
        let dry = self.view().get(s, STATE) == Some("dry");
        let have = matches!(self.bs.be_ticks.get(&p), Some((_, BeTick::PotentSulfur)));
        if dry && have {
            self.bs.be_ticks.remove(&p);
        } else if !dry && !have {
            let id = self.add_ticker(p);
            self.bs.be_ticks.insert(p, (id, BeTick::PotentSulfur));
        }
    }

    /// Ticker still valid: active sulfur outside the dry state.
    pub(crate) fn potent_sulfur_alive(&self, s: u32) -> bool {
        self.view().kind(s) == Kind::PotentSulfur && self.view().get(s, STATE) != Some("dry")
    }

    /// Block entity tick.
    pub(crate) fn potent_sulfur_tick(&mut self, p: P) {
        let s = self.get(p);
        match self.view().get(s, STATE) {
            Some("wet") => self.sulfur_nausea(p),
            Some("dormant") => {
                self.sulfur_countdown(s, p);
                self.sulfur_nausea(p);
            }
            Some("erupting") => {
                self.sulfur_launch(p);
                self.sulfur_countdown(s, p);
            }
            Some("continuous") => self.sulfur_launch(p),
            _ => {}
        }
    }

    /// Does the geyser pass through this state? Air, water, or empty collision shape (seen from a
    /// block above the block: scaffolding and powder snow stop nothing).
    fn geyser_passable(&self, s: u32) -> bool {
        let v = self.view();
        let b = v.block(s);
        v.is_air(s)
            || b == self.bs.t.id.water
            || b == self.bs.t.id.powder_snow
            || v.kind(s) == Kind::Scaffolding
            || self.data.shape(v.info(s).collision).is_empty()
    }

    /// Gas source block above the sulfur at `p`.
    fn gas_source(&self, p: P) -> Option<P> {
        let mut q = above(p);
        while q[1] <= p[1] + WATER_ABOVE_MAX + 1 {
            let s = self.get(q);
            let f = self.fluid_of(s);
            let wet = f.kind == 1 && f.source;
            if !wet || (self.view().block(s) != self.bs.t.id.water && !self.geyser_passable(s)) {
                return (self.view().is_air(s) || self.geyser_passable(s)).then_some(q);
            }
            q = above(q);
        }
        None
    }

    /// Does the gas of source `src` reach eyes at `eye`?
    fn gas_reaches(&self, src: P, eye: [f64; 3]) -> bool {
        let cell = eye.map(|c| c.floor() as i32);
        if !self.geyser_passable(self.get(cell)) {
            return false;
        }
        let c = center(src);
        if (0..3).map(|i| (eye[i] - c[i]).powi(2)).sum::<f64>() > 9.0 {
            return false;
        }
        let from = center(below(src));
        let to = [eye[0], eye[1] - 1.0, eye[2]];
        let f = self.fluid_at(to.map(|c| c.floor() as i32));
        if !(f.kind == 1 && f.source) {
            return false;
        }
        let ray = crate::fluid_use::SimRay(self, true);
        rc_physics::ray::clip(&ray, from, to, rc_physics::ray::Fluids::None).miss
    }

    /// Nausea for living beings near the source (every 10 ticks).
    fn sulfur_nausea(&mut self, p: P) {
        if self.time % 10 != 0 {
            return;
        }
        let Some(src) = self.gas_source(p) else { return };
        let (x, y, z) = (f64::from(src[0]), f64::from(src[1]), f64::from(src[2]));
        let area = Aabb { min: [x - 2.5, y, z - 2.5], max: [x + 3.5, y + 1.0, z + 3.5] };
        let nausea = || {
            let mut e = crate::effects::Inst::new("minecraft:nausea", NAUSEA_TICKS, 0);
            e.ambient = true;
            e
        };
        let player_hit = |sv: &Server| {
            sv.game_mode != 3
                && !sv.life.vit.dead
                && sv.ent_bbox(Ent::Player).is_some_and(|b| b.intersects(&area))
                && sv.gas_reaches(src, [sv.pos[0], sv.player_eye_y(), sv.pos[2]])
        };
        if player_hit(self) {
            self.add_effect(nausea());
        }
        let guests = self.lan.as_ref().map_or(0, |l| l.guests.len());
        for i in 0..guests {
            self.as_guest(i, |sv| {
                if player_hit(sv) {
                    sv.add_effect(nausea());
                }
            });
        }
        for i in 0..self.mobs.list.len() {
            let hit = self.mobs.list[i].as_ref().is_some_and(|m| {
                !m.removed
                    && m.alive()
                    && m.touches(&area)
                    && self.gas_reaches(src, [m.pos[0], m.eye_y(), m.pos[2]])
            });
            if hit && let Some(mut m) = self.mobs.list[i].take() {
                self.add_mob_effect(&mut m, nausea());
                self.mobs.list[i] = Some(m);
            }
        }
    }

    /// Countdown (every 20 ticks): at zero, eruption or end of eruption.
    fn sulfur_countdown(&mut self, s: u32, p: P) {
        if self.time % 20 != 0 {
            return;
        }
        let Some(src) = self.gas_source(p) else { return };
        let dormant = self.view().get(s, STATE) == Some("dormant");
        let mut cd = self.be_get(p).and_then(|b| crate::nbtio::int(b, COUNTDOWN)).map_or(-1, |v| v as i32);
        if cd <= 0 {
            let water = src[1] - p[1] - 1;
            let mut r = rc_worldgen::rng::Rng::xoroshiro(self.world_seed() ^ GEYSER_SALT).fork_positional().at(p[0], p[1], p[2]);
            cd = if dormant {
                10 * (water - 1) + 15 + r.next_int_bounded(16)
            } else {
                let _ = r.next_int();
                water - 1 + 1 + r.next_int_bounded(2)
            };
        }
        if cd > 0 {
            cd -= 1;
        }
        self.be_set_int(p, COUNTDOWN, cd);
        if cd == 0 {
            let to = if dormant { "erupting" } else { "dormant" };
            let ns = self.view().set(s, STATE, to);
            self.set_block(p, ns, flag::ALL);
            if !dormant {
                self.game_event_block("minecraft:block_deactivate", p, None, Some(s));
            }
        }
    }

    /// Free height of the column above the sulfur (6 blocks per block of water at most).
    fn geyser_height(&self, p: P, water: i32) -> i32 {
        let max = 6 * water;
        (0..max).find(|&i| !self.geyser_passable(self.get([p[0], p[1] + 1 + i, p[2]]))).unwrap_or(max)
    }

    /// Launch of the entities in the column (every tick).
    fn sulfur_launch(&mut self, p: P) {
        let Some(src) = self.gas_source(p) else { return };
        let water = src[1] - p[1] - 1;
        let h = self.geyser_height(p, water);
        let (x, y, z) = (f64::from(p[0]), f64::from(p[1] + 1), f64::from(p[2]));
        let d = f64::from(h - 1);
        let area = Aabb { min: [x, y + d.min(0.0), z], max: [x + 1.0, y + 1.0 + d.max(0.0), z + 1.0] };
        let limit = f64::from(0.3f32) + f64::from(water) * 0.1;
        let push = f64::from(0.2f32);
        let lift = |vel: &mut [f64; 3]| {
            if vel[1] < limit {
                vel[1] += push;
            }
        };
        let cap_fall = |vel_y: f64, fall: &mut f64| {
            if vel_y > -0.5 && *fall > 1.0 {
                *fall = 1.0;
            }
        };
        for m in self.mobs.list.iter_mut().flatten() {
            if m.removed || !m.alive() || !m.touches(&area) {
                continue;
            }
            cap_fall(m.vel[1], &mut m.fall_distance);
            if !Self::player_controls(m) && m.mount.vehicle.is_none() && !m.kind.not_affected_by_geysers() {
                lift(&mut m.vel);
            }
        }
        for e in self.bs.entities.list.iter_mut() {
            if !e.removed && Self::entity_box(e).intersects(&area) {
                cap_fall(e.vel[1], &mut e.fall);
                lift(&mut e.vel);
            }
        }
        let around = |pos: [f64; 3], w: f64, h: f64| Aabb {
            min: [pos[0] - w / 2.0, pos[1], pos[2] - w / 2.0],
            max: [pos[0] + w / 2.0, pos[1] + h, pos[2] + w / 2.0],
        };
        for d in self.life.drops.iter_mut() {
            if !d.is_removed() && around(d.pos, 0.25, 0.25).intersects(&area) {
                lift(&mut d.vel);
            }
        }
        for o in self.life.orbs.iter_mut() {
            if !o.is_removed() && around(o.pos, 0.5, 0.5).intersects(&area) {
                lift(&mut o.vel);
            }
        }
        for pr in self.proj.list.iter_mut() {
            if !pr.removed && pr.bbox().intersects(&area) {
                lift(&mut pr.vel);
            }
        }
        for sp in self.mobs.mounts.spits.iter_mut() {
            if !sp.removed && around(sp.pos, 0.25, 0.25).intersects(&area) {
                lift(&mut sp.vel);
            }
        }
        for c in self.carts.list.iter_mut().flatten() {
            if !c.removed && c.bbox().intersects(&area) {
                cap_fall(c.vel[1], &mut c.fall_distance);
                lift(&mut c.vel);
            }
        }
    }
}
