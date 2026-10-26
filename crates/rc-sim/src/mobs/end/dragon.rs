//! Ender Dragon (F15): own tick (the game calls neither the common AI nor movement), flight
//! history, eight parts (ids reserved right after the dragon), healing by the closest crystal,
//! flight toward the phase target, contacts (wings: knockback and 5 damage, head and neck: 10),
//! blocks crossed (removed if the rule allows, otherwise a "wall" that slows), damage by the part
//! hit, fatal blow (death phase) and death animation (200 ticks, experience, victory signaled
//! to the fight).

use super::super::mob::{Ent, Mob};
use super::super::mth::{DEG, atan2, cos, rotate_if_necessary, sin, wrap};
use super::graph::Graph;
use super::phases::{Phase, States, norm, wrap_d};
use crate::server::Server;

/// Parts: head, neck, body, three tail segments, two wings (width, height).
pub const PART_SIZES: [(f64, f64); 8] =
    [(1.0, 1.0), (3.0, 3.0), (5.0, 3.0), (2.0, 2.0), (2.0, 2.0), (2.0, 2.0), (4.0, 2.0), (4.0, 2.0)];
pub const HEAD: usize = 0;
pub const NECK: usize = 1;
pub const BODY: usize = 2;
const WING1: usize = 6;
const WING2: usize = 7;

/// Flight history: 64 samples (height, yaw).
#[derive(Debug, Clone)]
pub struct History {
    samples: [(f64, f32); 64],
    head: i32,
}

impl Default for History {
    fn default() -> History {
        History { samples: [(0.0, 0.0); 64], head: -1 }
    }
}

impl History {
    pub fn record(&mut self, y: f64, yaw: f32) {
        if self.head < 0 {
            self.samples = [(y, yaw); 64];
        }
        self.head += 1;
        if self.head == 64 {
            self.head = 0;
        }
        self.samples[self.head as usize] = (y, yaw);
    }

    pub fn get(&self, delay: i32) -> (f64, f32) {
        self.samples[((self.head - delay) & 63) as usize]
    }
}

/// The dragon's own state.
#[derive(Debug, Clone)]
pub struct Dragon {
    pub phase: Phase,
    pub ph: States,
    pub history: History,
    /// Part positions (bottom-center of their box).
    pub parts: [[f64; 3]; 8],
    pub in_wall: bool,
    pub death_time: i32,
    pub y_rot_a: f32,
    /// Crystal healing it (network id).
    pub nearest_crystal: Option<i32>,
    /// Tied to the dimension's fight (same UUID).
    pub fight: bool,
    /// Fight origin (podium).
    pub origin: [i32; 3],
    pub sitting_damage: f32,
    pub graph: Graph,
    /// Fatal blow received: "dying" posture sent.
    pub dying_pose: bool,
    /// Breath clouds to remove (end of the breath phase).
    pub discard_clouds: Vec<i32>,
}

impl Default for Dragon {
    fn default() -> Dragon {
        Dragon {
            phase: Phase::Hover,
            ph: States::default(),
            history: History::default(),
            parts: [[0.0; 3]; 8],
            in_wall: false,
            death_time: 0,
            y_rot_a: 0.0,
            nearest_crystal: None,
            fight: false,
            origin: [0, 0, 0],
            sitting_damage: 0.0,
            graph: Graph::default(),
            dying_pose: false,
            discard_clouds: Vec::new(),
        }
    }
}

/// Box of a part.
pub fn part_box(d: &Dragon, i: usize) -> rc_physics::Aabb {
    let p = d.parts[i];
    let (w, h) = PART_SIZES[i];
    rc_physics::Aabb { min: [p[0] - w / 2.0, p[1], p[2] - w / 2.0], max: [p[0] + w / 2.0, p[1] + h, p[2] + w / 2.0] }
}

fn inflate(b: rc_physics::Aabb, x: f64, y: f64, z: f64) -> rc_physics::Aabb {
    rc_physics::Aabb {
        min: [b.min[0] - x, b.min[1] - y, b.min[2] - z],
        max: [b.max[0] + x, b.max[1] + y, b.max[2] + z],
    }
}

impl Server {
    /// "Motion blocking" height (`MOTION_BLOCKING`): first free block above the
    /// highest solid or fluid block.
    pub(crate) fn hm_motion_blocking(&self, x: i32, z: i32) -> i32 {
        self.hm_scan(x, z, false)
    }

    /// `MOTION_BLOCKING_NO_LEAVES` height (leaves ignored).
    pub(crate) fn hm_no_leaves(&self, x: i32, z: i32) -> i32 {
        self.hm_scan(x, z, true)
    }

    /// Live map of the chunk if it exists, otherwise a scan with the same rule: tag
    /// `blocks_motion_in_heightmap` (or its leafless variant), or non-empty fluid.
    fn hm_scan(&self, x: i32, z: i32, no_leaves: bool) -> i32 {
        let map = if no_leaves { 5 } else { 4 };
        if let Some(Some(c)) = self.chunks.get(&(x >> 4, z >> 4))
            && c.heightmaps.iter().any(|(k, _)| *k == map)
        {
            return self.surface_y(x, z, map);
        }
        let tag = if no_leaves {
            "minecraft:blocks_motion_in_heightmap_no_leaves"
        } else {
            "minecraft:blocks_motion_in_heightmap"
        };
        let min = self.dim.height.min_y;
        for y in (min..self.top_y()).rev() {
            let p = [x, y, z];
            if self.btag(p, tag) || !self.fluid_of(self.get(p)).is_empty() {
                return y + 1;
            }
        }
        min
    }

    /// Dragon tick (reduced to what the dragon uses).
    pub(crate) fn dragon_tick(&mut self, m: &mut Mob) {
        let Some(mut d) = m.end.dragon.take() else { return };
        m.old = m.pos;
        m.pitch_o = m.pitch;
        m.head_yaw_o = m.head_yaw;
        m.tick_count += 1;
        if m.pos[1] < f64::from(self.dim.height.min_y - 64) {
            m.removed = true;
            m.end.dragon = Some(d);
            return;
        }
        if m.fire > 0 {
            m.fire = 0;
        }
        if m.hurt_time > 0 {
            m.hurt_time -= 1;
        }
        if m.damage_cooldown > 0 {
            m.damage_cooldown -= 1;
        }
        if m.health <= 0.0 {
            self.dragon_tick_death(m, &mut d);
            if m.removed {
                m.end.dragon = Some(d);
                return;
            }
        }
        if m.last_hurt_by_player > 0 {
            m.last_hurt_by_player -= 1;
        }
        if let Some((e, ts)) = m.last_hurt_by_mob
            && (!self.ent_alive(e) || m.tick_count - ts > 100)
        {
            m.last_hurt_by_mob = None;
        }
        // Ambient sound (entity random).
        if m.alive() {
            let r = m.rng.next_int_bound(1000);
            let t = m.ambient_time;
            m.ambient_time += 1;
            if r < t {
                m.ambient_time = -m.kind.info().ambient_interval;
                self.make_sound(m, "minecraft:entity.ender_dragon.ambient");
            }
        }
        self.dragon_ai_step(m, &mut d);
        // Body and head (body = yaw, head at 75° at most).
        m.body_yaw = m.yaw;
        m.head_yaw = rotate_if_necessary(m.head_yaw, m.body_yaw, 75.0);
        let discard = std::mem::take(&mut d.discard_clouds);
        m.end.dragon = Some(d);
        for id in discard {
            self.discard_cloud(id);
        }
    }

    /// The dragon's AI step.
    fn dragon_ai_step(&mut self, m: &mut Mob, d: &mut Dragon) {
        // Lines of sight memoized for this tick only.
        m.sight.clear();
        if !d.fight && self.mobs.end.fight.as_ref().is_some_and(|f| f.dragon_uuid == Some(m.uuid)) {
            d.fight = true;
        }
        if m.health <= 0.0 {
            // Client explosion particles: the three draws also happen server-side.
            for _ in 0..3 {
                m.rng.next_float();
            }
            return;
        }
        self.dragon_check_crystals(m, d);
        m.yaw = wrap(m.yaw);
        if m.no_ai {
            return;
        }
        d.history.record(m.pos[1], m.yaw);
        let before = d.phase;
        self.phase_tick(m, d);
        if d.phase != before {
            self.phase_tick(m, d);
        }
        if let Some(t) = d.fly_target() {
            self.dragon_fly(m, d, t);
        }
        m.body_yaw = m.yaw;
        let (h5, h10) = (d.history.get(5), d.history.get(10));
        let tilt = (h5.0 - h10.0) as f32 * 10.0 * DEG;
        let (cc_tilt, ss_tilt) = (cos(tilt), sin(tilt));
        let rot1 = m.yaw * DEG;
        let (ss1, cc1) = (sin(rot1), cos(rot1));
        let p = m.pos;
        let set = |d: &mut Dragon, i: usize, x: f64, y: f64, z: f64| d.parts[i] = [p[0] + x, p[1] + y, p[2] + z];
        set(d, BODY, f64::from(ss1 * 0.5), 0.0, f64::from(-cc1 * 0.5));
        set(d, WING1, f64::from(cc1 * 4.5), 2.0, f64::from(ss1 * 4.5));
        set(d, WING2, f64::from(cc1 * -4.5), 2.0, f64::from(ss1 * -4.5));
        // Contacts (head and neck at their position of the previous tick), if it has not been hurt.
        if m.hurt_time <= 0 {
            for w in [WING1, WING2] {
                let mut b = inflate(part_box(d, w), 4.0, 2.0, 4.0);
                b.min[1] -= 2.0;
                b.max[1] -= 2.0;
                self.dragon_knockback(m, d, b);
            }
            for part in [HEAD, NECK] {
                let b = inflate(part_box(d, part), 1.0, 1.0, 1.0);
                self.dragon_bite(m, b);
            }
        }
        let ss2 = sin(m.yaw * DEG - d.y_rot_a * 0.01);
        let cc2 = cos(m.yaw * DEG - d.y_rot_a * 0.01);
        let y_off = if d.phase.sitting() { -1.0 } else { (d.history.get(5).0 - d.history.get(0).0) as f32 };
        set(d, HEAD, f64::from(ss2 * 6.5 * cc_tilt), f64::from(y_off + ss_tilt * 6.5), f64::from(-cc2 * 6.5 * cc_tilt));
        set(d, NECK, f64::from(ss2 * 5.5 * cc_tilt), f64::from(y_off + ss_tilt * 5.5), f64::from(-cc2 * 5.5 * cc_tilt));
        let p1 = d.history.get(5);
        for i in 0..3 {
            let p0 = d.history.get(12 + i * 2);
            let rot = m.yaw * DEG + wrap_d(f64::from(p0.1 - p1.1)) as f32 * DEG;
            let (ss, cc) = (sin(rot), cos(rot));
            let dd = (i + 1) as f32 * 2.0;
            set(
                d,
                3 + i as usize,
                f64::from(-(ss1 * 1.5 + ss * dd) * cc_tilt),
                p0.0 - p1.0 - f64::from((dd + 1.5) * ss_tilt) + 1.5,
                f64::from((cc1 * 1.5 + cc * dd) * cc_tilt),
            );
        }
        // Walls: head, neck, body (each tested, `|` without short-circuit).
        let a = self.dragon_walls(m, part_box(d, HEAD));
        let b = self.dragon_walls(m, part_box(d, NECK));
        let c = self.dragon_walls(m, part_box(d, BODY));
        d.in_wall = a | b | c;
        if d.fight {
            self.fight_update_dragon(m);
        }
    }

    /// Steering toward the phase target.
    fn dragon_fly(&mut self, m: &mut Mob, d: &mut Dragon, t: [f64; 3]) {
        let xdd = t[0] - m.pos[0];
        let mut ydd = t[1] - m.pos[1];
        let zdd = t[2] - m.pos[2];
        let dist = xdd * xdd + ydd * ydd + zdd * zdd;
        let max = f64::from(d.phase.fly_speed());
        let hd = (xdd * xdd + zdd * zdd).sqrt();
        if hd > 0.0 {
            ydd = (ydd / hd).clamp(-max, max);
        }
        m.vel[1] += ydd * 0.01;
        m.yaw = wrap(m.yaw);
        let aim = norm([t[0] - m.pos[0], t[1] - m.pos[1], t[2] - m.pos[2]]);
        let dir = norm([f64::from(sin(m.yaw * DEG)), m.vel[1], f64::from(-cos(m.yaw * DEG))]);
        let dot = ((((dir[0] * aim[0] + dir[1] * aim[1] + dir[2] * aim[2]) as f32) + 0.5) / 1.5).max(0.0);
        if xdd.abs() > f64::from(1.0e-5f32) || zdd.abs() > f64::from(1.0e-5f32) {
            let deg = 180.0f32 / std::f32::consts::PI;
            let y_rot_d = wrap(180.0 - (atan2(xdd, zdd) as f32) * deg - m.yaw).clamp(-50.0, 50.0);
            d.y_rot_a *= 0.8;
            d.y_rot_a += y_rot_d * d.turn_speed(m.vel);
            m.yaw += d.y_rot_a * 0.1;
        }
        let span = (2.0 / (dist + 1.0)) as f32;
        let speed = 0.06f32 * (dot * span + (1.0 - span));
        // The relative movement.
        let (s, c) = (sin(m.yaw * DEG), cos(m.yaw * DEG));
        let sp = f64::from(speed);
        m.vel[0] += -(-sp) * f64::from(s);
        m.vel[2] += -sp * f64::from(c);
        let mv = if d.in_wall { m.vel.map(|v| v * f64::from(0.8f32)) } else { m.vel };
        m.pos = [m.pos[0] + mv[0], m.pos[1] + mv[1], m.pos[2] + mv[2]];
        let actual = norm(m.vel);
        let slide = 0.8 + 0.15 * (actual[0] * dir[0] + actual[1] * dir[1] + actual[2] * dir[2] + 1.0) / 2.0;
        m.vel = [m.vel[0] * slide, m.vel[1] * f64::from(0.91f32), m.vel[2] * slide];
    }

    /// Healing by the followed crystal, new crystal 1 time in 10.
    fn dragon_check_crystals(&mut self, m: &mut Mob, d: &mut Dragon) {
        if let Some(id) = d.nearest_crystal {
            if self.mobs.end.crystals.iter().all(|c| c.id != id || c.removed) {
                d.nearest_crystal = None;
            } else if m.tick_count % 10 == 0 && m.health < m.max_health() {
                m.health = (m.health + 1.0).min(m.max_health());
            }
        }
        if m.rng.next_int_bound(10) == 0 {
            let area = inflate(m.bbox(), 32.0, 32.0, 32.0);
            let mut best: Option<(f64, i32)> = None;
            for c in self.mobs.end.crystals.iter().filter(|c| !c.removed && c.bbox().intersects(&area)) {
                let dd = super::super::mth::dist2(c.pos, m.pos);
                if best.is_none_or(|b| dd < b.0) {
                    best = Some((dd, c.id));
                }
            }
            d.nearest_crystal = best.map(|b| b.1);
        }
    }

    /// Wings: living entities in the box are pushed back, and hurt (5) outside the sitting
    /// phase if their last injury by a mob is at least 3 ticks old.
    fn dragon_knockback(&mut self, m: &Mob, d: &Dragon, area: rc_physics::Aabb) {
        let body = part_box(d, BODY);
        let xm = (body.min[0] + body.max[0]) / 2.0;
        let zm = (body.min[2] + body.max[2]) / 2.0;
        let ids: Vec<i32> = self
            .mobs
            .list
            .iter()
            .flatten()
            .filter(|o| o.id != m.id && o.alive() && o.touches(&area))
            .map(|o| o.id)
            .collect();
        for id in ids {
            let Some(i) = self.mob_index(id) else { continue };
            let Some(mut o) = self.mobs.list[i].take() else { continue };
            let (xd, zd) = (o.pos[0] - xm, o.pos[2] - zm);
            let dd = (xd * xd + zd * zd).max(0.1);
            o.vel = [o.vel[0] + xd / dd * 4.0, o.vel[1] + f64::from(0.2f32), o.vel[2] + zd / dd * 4.0];
            o.needs_sync = true;
            let stamp = o.last_hurt_by_mob.map_or(0, |h| h.1);
            if !d.phase.sitting() && stamp < o.tick_count - 2 {
                self.mob_hurt_by(&mut o, m, "minecraft:mob_attack", 5.0);
            }
            self.mobs.list[i] = Some(o);
        }
        if self.game_mode != 1
            && self.game_mode != 3
            && self.ent_alive(Ent::Player)
            && self.player_aabb().intersects(&area)
        {
            let (xd, zd) = (self.pos[0] - xm, self.pos[2] - zm);
            let dd = (xd * xd + zd * zd).max(0.1);
            self.player_push([xd / dd * 4.0, f64::from(0.2f32), zd / dd * 4.0]);
            let stamp = self.mobs.player_hurt_by.map_or(0, |h| h.1);
            if !d.phase.sitting() && stamp < self.mobs.player_ticks - 2 {
                let kb = Some((m.pos[0] - self.pos[0], m.pos[2] - self.pos[2]));
                self.player_hurt_by_mob(m, "minecraft:mob_attack", 5.0, kb);
            }
        }
    }

    /// Head and neck: 10 damage to every living entity in the box.
    fn dragon_bite(&mut self, m: &Mob, area: rc_physics::Aabb) {
        let ids: Vec<i32> = self
            .mobs
            .list
            .iter()
            .flatten()
            .filter(|o| o.id != m.id && o.alive() && o.touches(&area))
            .map(|o| o.id)
            .collect();
        for id in ids {
            let Some(i) = self.mob_index(id) else { continue };
            let Some(mut o) = self.mobs.list[i].take() else { continue };
            self.mob_hurt_by(&mut o, m, "minecraft:mob_attack", 10.0);
            self.mobs.list[i] = Some(o);
        }
        if self.game_mode != 1
            && self.game_mode != 3
            && self.ent_alive(Ent::Player)
            && self.player_aabb().intersects(&area)
        {
            let kb = Some((m.pos[0] - self.pos[0], m.pos[2] - self.pos[2]));
            self.player_hurt_by_mob(m, "minecraft:mob_attack", 10.0, kb);
        }
    }

    // Player push (`push`): `player_push`, shared with F14 (`recent/mod.rs`).

    /// Non-empty blocks outside `dragon_transparent` removed (without loot) if the
    /// rule allows and outside `dragon_immune`, otherwise "wall". A removed block: event 2008
    /// at a position drawn in the box.
    fn dragon_walls(&mut self, m: &mut Mob, bb: rc_physics::Aabb) -> bool {
        let lo = bb.min.map(|v| v.floor() as i32);
        let hi = bb.max.map(|v| v.floor() as i32);
        let griefing = self.rule("minecraft:mob_griefing", true);
        let mut hit = false;
        let mut destroyed = false;
        for x in lo[0]..=hi[0] {
            for y in lo[1]..=hi[1] {
                for z in lo[2]..=hi[2] {
                    let p = [x, y, z];
                    let s = self.get(p);
                    let v = self.v();
                    if v.is_air(s) {
                        continue;
                    }
                    let b = v.block(s);
                    if self.tag_id(crate::tag_bits::TagId::DragonTransparent, b) {
                        continue;
                    }
                    if griefing && !self.tag_id(crate::tag_bits::TagId::DragonImmune, b) {
                        destroyed = self.remove_block(p, false) || destroyed;
                    } else {
                        hit = true;
                    }
                }
            }
        }
        if destroyed {
            let rp = [
                lo[0] + m.rng.next_int_bound(hi[0] - lo[0] + 1),
                lo[1] + m.rng.next_int_bound(hi[1] - lo[1] + 1),
                lo[2] + m.rng.next_int_bound(hi[2] - lo[2] + 1),
            ];
            self.level_event(2008, rp, 0);
        }
        hit
    }

    /// Damage received: nothing during death; arrows and wind charges
    /// without effect in the sitting phase; outside head and neck, damage/4 + min(damage, 1); only
    /// those from a player or explosions count (tag `always_hurts_ender_dragons`); in the
    /// sitting phase, more than 25% of cumulative max HP → takeoff. The source stays that of the
    /// hit received by the part: origin and knockback direction (`dir`, projectile) pass as is.
    pub(crate) fn dragon_hurt(
        &mut self,
        m: &mut Mob,
        ty: &str,
        amount: f32,
        by: Option<Ent>,
        src: Option<[f64; 3]>,
        dir: Option<(f64, f64)>,
    ) -> bool {
        let part = self.mobs.end.part.take().unwrap_or(BODY);
        let Some(phase) = m.end.dragon.as_ref().map(|d| d.phase) else { return false };
        if phase == Phase::Dying {
            return false;
        }
        let mut dmg = amount;
        if phase.sitting_class() && matches!(ty, "minecraft:arrow" | "minecraft:trident" | "minecraft:wind_charge") {
            dmg = 0.0;
        }
        if part != HEAD && part != NECK {
            dmg = dmg / 4.0 + dmg.min(1.0);
        }
        if dmg < 0.01 {
            return false;
        }
        if by == Some(Ent::Player) || self.dmg_tag(ty, "minecraft:always_hurts_ender_dragons") {
            let before = m.health;
            self.mobs.end.really = true;
            self.mob_hurt_dir(m, ty, dmg, by, src, dir);
            self.mobs.end.really = false;
            let max = m.max_health();
            if let Some(d) = m.end.dragon.as_mut()
                && d.phase.sitting()
            {
                d.sitting_damage += before - m.health;
                if d.sitting_damage > 0.25 * max {
                    d.sitting_damage = 0.0;
                    d.set_phase(Phase::Takeoff);
                }
            }
            let discard = m.end.dragon.as_mut().map(|d| std::mem::take(&mut d.discard_clouds)).unwrap_or_default();
            for id in discard {
                self.discard_cloud(id);
            }
        }
        true
    }

    /// Fatal blow (`die` with its own fatal-blow handling): never "dead"; outside the sitting
    /// phase, HP reset to 1 and death phase; event 3 and "dying" posture sent.
    pub(crate) fn dragon_die(&mut self, m: &mut Mob, ty: &str, by: Option<Ent>) {
        if m.removed {
            return;
        }
        self.s9_mob_died(m, ty, by);
        if let Some(d) = m.end.dragon.as_mut() {
            if !d.phase.sitting() {
                m.health = 1.0;
                d.set_phase(Phase::Dying);
            }
            d.dying_pose = true;
        }
        self.mob_event(m, 3);
        m.needs_sync = true;
    }

    /// Death animation: 200 ticks, rises by 0.1 per tick, experience (12,000
    /// for the world's first dragon, 500 afterward), victory signaled to the fight.
    fn dragon_tick_death(&mut self, m: &mut Mob, d: &mut Dragon) {
        if d.fight {
            self.fight_update_dragon(m);
        }
        d.death_time += 1;
        // Big client explosions (180..200): draws also made server-side.
        if (180..=200).contains(&d.death_time) {
            for _ in 0..3 {
                m.rng.next_float();
            }
        }
        let first = d.fight && self.mobs.end.fight.as_ref().is_some_and(|f| !f.previously_killed);
        let xp = if first { 12_000 } else { 500 };
        let drops = self.rule("minecraft:mob_drops", true);
        if d.death_time > 150 && d.death_time % 5 == 0 && drops {
            self.spawn_orbs(m.pos, (xp as f32 * 0.08).floor() as i32);
        }
        if d.death_time == 1 && !super::silent(m) {
            self.global_level_event(1028, m.block_pos(), 0);
        }
        let up = f64::from(0.1f32);
        m.pos[1] += up;
        for p in &mut d.parts {
            p[1] += up;
        }
        if d.death_time >= 200 {
            if drops {
                self.spawn_orbs(m.pos, (xp as f32 * 0.2).floor() as i32);
            }
            if d.fight {
                self.fight_dragon_killed(m.uuid);
            }
            self.mob_removed_effects(m); // "killed" removal
            m.removed = true;
        }
    }

    /// Part of the dragon hit by a projectile (projectiles target the parts, never
    /// the dragon itself).
    pub(crate) fn dragon_part_hit(&self, h: crate::projectiles::physics::EntityHit) -> Option<usize> {
        match h.ent {
            crate::projectiles::physics::Target::Part(_, i) => Some(usize::from(i)),
            _ => None,
        }
    }

    /// Player hit on a dragon part (dragon id + 1..8); true if handled.
    pub(crate) fn dragon_part_attacked(&mut self, id: i32) -> bool {
        let Some(i) = self.mobs.list.iter().position(|o| {
            o.as_ref()
                .is_some_and(|o| o.kind == super::super::species::Kind::EnderDragon && id > o.id && id <= o.id + 8)
        }) else {
            return false;
        };
        if self.game_mode == 3 || self.life.vit.dead {
            return true;
        }
        let Some(mut m) = self.mobs.list[i].take() else { return true };
        let part = (id - m.id - 1) as usize;
        let reach_ok = m.end.dragon.as_ref().is_some_and(|d| {
            let b = part_box(d, part);
            let eye = [self.pos[0], self.player_eye_y(), self.pos[2]];
            let c: [f64; 3] = std::array::from_fn(|k| eye[k].clamp(b.min[k], b.max[k]));
            super::super::mth::dist2(eye, c) < 36.0
        });
        if reach_ok && m.alive() {
            self.mobs.end.part = Some(part);
            self.mobs.end.part_attack = true;
            self.player_attack_mob(&mut m);
            self.mobs.end.part_attack = false;
            self.mobs.end.part = None;
        }
        self.mobs.list[i] = Some(m);
        true
    }

    /// Crystal destroyed (the dragon's): 10 explosion damage to the head if
    /// it was the followed crystal (attributed to the player), then the phase is notified.
    pub(crate) fn dragon_crystal_destroyed(&mut self, dragon: i32, crystal: i32, player: Option<Ent>) {
        let Some(i) = self.mob_index(dragon) else { return };
        let Some(mut m) = self.mobs.list[i].take() else { return };
        if m.end.dragon.as_ref().is_some_and(|d| d.nearest_crystal == Some(crystal)) {
            self.mobs.end.part = Some(HEAD);
            let ty = if player.is_some() { "minecraft:player_explosion" } else { "minecraft:explosion" };
            self.mob_hurt(&mut m, ty, 10.0, player, None);
            self.mobs.end.part = None;
        }
        if let Some(mut d) = m.end.dragon.take() {
            self.phase_crystal_destroyed(&mut m, &mut d, player);
            let discard = std::mem::take(&mut d.discard_clouds);
            m.end.dragon = Some(d);
            for id in discard {
                self.discard_cloud(id);
            }
        }
        self.mobs.list[i] = Some(m);
    }
}
