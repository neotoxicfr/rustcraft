//! Iron golem (§ 4.5): neutral toward the player (anger 20 to 39 s), defends the village, hits
//! hard by throwing the target into the air, holds out a poppy, repairs with an iron ingot,
//! cracks according to its HP; spawned by villagers (`summon_for_villager`).

use rc_client::{EntityEvent, Event};

use super::super::mob::{Ent, Mob};
use super::super::mth::dist2;
use super::super::species::Kind;
use super::GolemGoal;
use crate::server::Server;

/// Cracks: 0 none, 1 light (< 75%), 2 medium (< 50%), 3 heavy (< 25%).
pub fn crackiness(m: &Mob) -> i32 {
    let f = m.health / m.max_health();
    if f < 0.25 {
        3
    } else if f < 0.5 {
        2
    } else if f < 0.75 {
        1
    } else {
        0
    }
}

/// Section of the cube of radius `radius` around `center` closest to a village, among those
/// closer than `center` (otherwise `center`). Traversal x, then y, then z; on a tie, the first.
pub fn closest_section(center: [i32; 3], radius: i32, mut dist: impl FnMut([i32; 3]) -> i32) -> [i32; 3] {
    let here = dist(center);
    let mut best: Option<([i32; 3], i32)> = None;
    for z in -radius..=radius {
        for y in -radius..=radius {
            for x in -radius..=radius {
                let s = [center[0] + x, center[1] + y, center[2] + z];
                let d = dist(s);
                if d < here && best.is_none_or(|b| d < b.1) {
                    best = Some((s, d));
                }
            }
        }
    }
    best.map_or(center, |b| b.0)
}

/// End of the AI step: animation counters, persistent anger.
pub fn post_ai_step(s: &mut Server, m: &mut Mob) {
    if m.golem.attack_anim > 0 {
        m.golem.attack_anim -= 1;
    }
    if m.golem.offer_flower > 0 {
        m.golem.offer_flower -= 1;
    }
    update_anger(s, m);
}

fn valid_player(s: &Server, t: Ent) -> bool {
    t == Ent::Player && s.game_mode != 1 && s.game_mode != 3 && s.level.difficulty_id() != 0
}

fn update_anger(s: &mut Server, m: &mut Mob) {
    let prev = m.target;
    // Dead mob target that was the anger target: anger ends.
    if let Some(Ent::Mob(id)) = prev
        && s.mob_ref(id).is_some_and(|o| o.dead || o.health <= 0.0)
        && m.anger_target == prev
    {
        m.stop_being_angry();
        return;
    }
    let target = m.target.filter(|t| s.ent_alive(*t));
    if let Some(t) = target {
        let new_target = m.anger_target != Some(t);
        if new_target {
            m.anger_target = Some(t);
            m.golem.angry_at = match t {
                Ent::Player => Some(s.uuid),
                Ent::Mob(id) => s.mob_ref(id).map(|o| o.uuid),
            };
        }
        // "Stays angry if the target is there": timer restarted every tick.
        let ticks = m.rng.int_between(400, 780);
        m.anger_end = s.time + i64::from(ticks);
    }
    let angry = m.anger_end > 0 && m.anger_end - s.time > 0;
    // Reference present (even if its entity is not loaded) and anger expired: end.
    if m.golem.angry_at.is_some() && !angry && target.is_none_or(|t| !valid_player(s, t)) {
        m.stop_being_angry();
    }
    if m.anger_target == Some(Ent::Player) && (s.game_mode == 1 || s.game_mode == 3 || s.level.difficulty_id() == 0) {
        m.stop_being_angry();
    }
}

/// Anger reference read back (UUID): attached to the present entity carrying that UUID, player
/// or mob. At the first tick after reloading, this entity also becomes the target. As long
/// as no entity matches, the golem is angry at nobody present.
pub fn relink_anger(s: &Server, m: &mut Mob) {
    let first = std::mem::take(&mut m.golem.relink);
    let Some(u) = m.golem.angry_at else { return };
    if m.anger_target.is_some() {
        return;
    }
    let found =
        if u == s.uuid { Some(Ent::Player) } else { s.mobs.find_uuid_where(u, |o| !o.removed).map(|o| Ent::Mob(o.id)) };
    m.anger_target = found;
    if first && found.is_some() {
        m.target = found;
    }
}

/// Iron ingot: +25 HP, repair sound, ingot consumed; nothing if already full.
pub fn interact(s: &mut Server, m: &mut Mob, slot: usize, item: &str) -> Option<bool> {
    if item != "minecraft:iron_ingot" {
        return None;
    }
    let before = m.health;
    m.health = (m.health + 25.0).min(m.max_health());
    if m.health == before {
        return Some(false);
    }
    let p = 1.0 + (m.rng.next_float() - m.rng.next_float()) * 0.2;
    s.mob_sound(m, "minecraft:entity.iron_golem.repair", 1.0, p);
    s.consume_held(slot);
    Some(true)
}

impl Server {
    /// Iron golem hit: half the damage + draw, vertical launch.
    pub(crate) fn iron_golem_attack(&mut self, m: &mut Mob, t: Ent) -> bool {
        m.golem.attack_anim = 10;
        self.mob_event(m, 4);
        let atk = m.attrs.attack.value() as f32;
        let dmg = if atk as i32 > 0 { atk / 2.0 + m.rng.next_int_bound(atk as i32) as f32 } else { atk };
        let ty = "minecraft:mob_attack";
        let hurt = match t {
            Ent::Player => {
                let kb = (m.pos[0] - self.pos[0], m.pos[2] - self.pos[2]);
                let r = self.player_hurt_by_mob(m, ty, dmg, Some(kb));
                if r {
                    // `0.4 × max(0, 1 − knockback resistance)` (player attribute).
                    let scale = (1.0 - self.player_attribute("minecraft:knockback_resistance")).max(0.0);
                    let v = self.mobs.player_vel;
                    let nv = [v[0], v[1] + f64::from(0.4f32) * scale, v[2]];
                    self.mobs.player_vel = nv;
                    let velocity = super::super::track::lp_quantize(nv);
                    let _ = self.send(Event::Entity(EntityEvent::Motion { id: self.player_id, velocity }));
                }
                r
            }
            Ent::Mob(id) => {
                let Some(i) = self.mob_index(id) else { return false };
                let mut o = self.mobs.list[i].take().expect("mob");
                let r = self.mob_hurt_by(&mut o, m, ty, dmg);
                if r {
                    let scale = (1.0 - o.attrs.knockback_resistance.value()).max(0.0);
                    o.vel[1] += f64::from(0.4f32) * scale;
                    o.needs_sync = true;
                }
                self.mobs.list[i] = Some(o);
                r
            }
        };
        self.mob_sound(m, "minecraft:entity.iron_golem.attack", 1.0, 1.0);
        hurt
    }

    /// Event 11 (held out) or 34 (put away).
    fn offer_flower(&self, m: &mut Mob, offer: bool) {
        if offer {
            m.golem.offer_flower = 400;
            self.mob_event(m, 11);
        } else {
            m.golem.offer_flower = 0;
            self.mob_event(m, 34);
        }
    }

    /// Player's reputation with a villager (gossip from F05).
    pub(crate) fn villager_reputation(&self, villager: &Mob) -> i32 {
        villager.npc.as_ref().map_or(0, |n| n.gossips.reputation(self.uuid))
    }

    /// Distance in sections from a SECTION to the nearest village (POI registry from F05).
    pub(crate) fn section_to_village(&mut self, section: [i32; 3]) -> i32 {
        self.sections_to_village([section[0] * 16, section[1] * 16, section[2] * 16])
    }

    /// Does a villager want a golem: slept less than a day ago, no golem seen recently
    /// (memories of the F05 brain).
    pub(crate) fn villager_wants_golem(&self, villager: &Mob) -> bool {
        let t = self.time;
        villager
            .npc
            .as_ref()
            .and_then(|n| n.brain.as_ref())
            .is_some_and(|b| b.mem.last_slept.is_some_and(|s| t - s < 24000) && b.mem.golem.is_none())
    }

    // ---------- Goals ----------

    pub(crate) fn iron_can_use(&mut self, g: &mut GolemGoal, m: &mut Mob) -> bool {
        match g {
            GolemGoal::MoveTowardsTarget { wanted, target } => {
                *target = m.target;
                let Some(t) = *target else { return false };
                let Some(tp) = self.ent_pos(t) else { return false };
                if dist2(tp, m.pos) > 32.0 * 32.0 {
                    return false;
                }
                match self.pos_towards(m, 16, 7, tp, std::f32::consts::FRAC_PI_2) {
                    Some(p) => {
                        *wanted = p;
                        true
                    }
                    None => false,
                }
            }
            GolemGoal::MoveBackToVillage { wanted } => {
                let b = m.block_pos();
                let sec = [b[0] >> 4, b[1] >> 4, b[2] >> 4];
                // "In the village": within one section at most.
                if self.section_to_village(sec) <= 1 {
                    return false;
                }
                // The random stroll objective (interval 10, no idle delay).
                if m.rng.next_int_bound(super::super::mth::reduced(10)) != 0 {
                    return false;
                }
                let best = self.section_closest_to_village(sec, 2);
                if best == sec {
                    return false;
                }
                let c =
                    [f64::from(best[0] * 16 + 8) + 0.5, f64::from(best[1] * 16 + 8), f64::from(best[2] * 16 + 8) + 0.5];
                match self.pos_towards(m, 10, 7, c, std::f32::consts::FRAC_PI_2) {
                    Some(p) => {
                        *wanted = p;
                        true
                    }
                    None => false,
                }
            }
            GolemGoal::VillageStroll { wanted } => {
                if m.rng.next_int_bound(super::super::mth::reduced(240)) != 0 {
                    return false;
                }
                match self.village_stroll_pos(m) {
                    Some(p) => {
                        *wanted = p;
                        true
                    }
                    None => false,
                }
            }
            GolemGoal::OfferFlower { entity, .. } => {
                if !self.bright_outside() {
                    return false;
                }
                if m.rng.next_int_bound(8000) != 0 {
                    return false;
                }
                *entity = self.flower_candidate(m);
                entity.is_some()
            }
            GolemGoal::DefendVillage { potential, .. } => {
                let bb = m.bbox();
                let area = rc_physics::Aabb {
                    min: [bb.min[0] - 10.0, bb.min[1] - 8.0, bb.min[2] - 10.0],
                    max: [bb.max[0] + 10.0, bb.max[1] + 8.0, bb.max[2] + 10.0],
                };
                let player_near =
                    self.player_aabb().intersects(&area) && self.targeting_ok(m, Ent::Player, true, 64.0, false, true);
                let villagers: Vec<(i32, i32)> = self
                    .mobs
                    .list
                    .iter()
                    .flatten()
                    .filter(|o| o.kind == Kind::Villager && o.touches(&area))
                    .map(|o| (o.id, self.villager_reputation(o)))
                    .collect();
                for (id, rep) in villagers {
                    if self.targeting_ok(m, Ent::Mob(id), true, 64.0, false, true) && player_near && rep <= -100 {
                        *potential = Some(Ent::Player);
                    }
                }
                // The potential target is never forgotten (goal always valid afterward).
                potential.is_some() && !(self.game_mode == 1 || self.game_mode == 3)
            }
            GolemGoal::Ranged { .. } => false,
        }
    }

    pub(crate) fn iron_can_continue(&mut self, g: &mut GolemGoal, m: &mut Mob) -> bool {
        match g {
            GolemGoal::MoveTowardsTarget { target, .. } => {
                let Some(t) = *target else { return false };
                !m.nav.is_done() && self.ent_alive(t) && self.ent_pos(t).is_some_and(|p| dist2(p, m.pos) < 32.0 * 32.0)
            }
            GolemGoal::MoveBackToVillage { .. } | GolemGoal::VillageStroll { .. } => !m.nav.is_done(),
            GolemGoal::OfferFlower { tick, .. } => *tick > 0,
            _ => false,
        }
    }

    pub(crate) fn iron_start(&mut self, g: &mut GolemGoal, m: &mut Mob) {
        match g {
            GolemGoal::MoveTowardsTarget { wanted, .. } => {
                let w = *wanted;
                self.move_to_pos(m, w, 0.9);
            }
            GolemGoal::MoveBackToVillage { wanted } | GolemGoal::VillageStroll { wanted } => {
                let w = *wanted;
                self.move_to_pos(m, w, 0.6);
            }
            GolemGoal::OfferFlower { tick, .. } => {
                *tick = super::super::mth::reduced(400);
                self.offer_flower(m, true);
            }
            GolemGoal::DefendVillage { potential, .. } => {
                m.target = *potential;
            }
            GolemGoal::Ranged { .. } => {}
        }
    }

    pub(crate) fn iron_stop(&mut self, g: &mut GolemGoal, m: &mut Mob) {
        match g {
            GolemGoal::MoveTowardsTarget { target, .. } => *target = None,
            GolemGoal::MoveBackToVillage { .. } | GolemGoal::VillageStroll { .. } => m.nav.stop(),
            GolemGoal::OfferFlower { entity, tick } => {
                self.offer_flower(m, false);
                if *tick == 0
                    && let Some(Ent::Mob(id)) = *entity
                    && let Some(i) = self.mob_index(id)
                {
                    let bb = m.bbox();
                    let area = rc_physics::Aabb {
                        min: [bb.min[0] - 6.0, bb.min[1] - 2.0, bb.min[2] - 6.0],
                        max: [bb.max[0] + 6.0, bb.max[1] + 2.0, bb.max[2] + 6.0],
                    };
                    if let Some(o) = self.mobs.list[i].as_deref_mut()
                        && o.kind == Kind::CopperGolem
                        && o.equipment[7].is_none()
                        && area.intersects(&o.bbox())
                    {
                        super::give_antenna_poppy(o);
                    }
                }
                *entity = None;
            }
            _ => {}
        }
    }

    pub(crate) fn iron_tick(&mut self, g: &mut GolemGoal, m: &mut Mob) {
        if let GolemGoal::OfferFlower { entity, tick } = g {
            if let Some(e) = *entity
                && let (Some(p), Some(eye)) = (self.ent_pos(e), self.ent_eye(e))
            {
                self.look_at(m, [p[0], eye, p[2]], 30.0, 30.0);
            }
            *tick -= 1;
        }
    }

    /// Closest villager or copper golem (6 blocks, box ±6/±2/±6, no combat).
    fn flower_candidate(&self, m: &mut Mob) -> Option<Ent> {
        let bb = m.bbox();
        let area = rc_physics::Aabb {
            min: [bb.min[0] - 6.0, bb.min[1] - 2.0, bb.min[2] - 6.0],
            max: [bb.max[0] + 6.0, bb.max[1] + 2.0, bb.max[2] + 6.0],
        };
        let cands: Vec<(i32, [f64; 3])> = self
            .mobs
            .list
            .iter()
            .flatten()
            .filter(|o| (o.kind == Kind::Villager || o.kind == Kind::CopperGolem) && o.touches(&area))
            .map(|o| (o.id, o.pos))
            .collect();
        let mut best: Option<(f64, Ent)> = None;
        for (id, p) in cands {
            if !self.targeting_ok(m, Ent::Mob(id), false, 6.0, false, true) {
                continue;
            }
            let d = dist2(m.pos, p);
            if best.is_none_or(|b| d < b.0) {
                best = Some((d, Ent::Mob(id)));
            }
        }
        best.map(|b| b.1)
    }

    /// Closest neighboring section to a village (no random).
    /// Wander position in the village (world random for the
    /// choices, entity random for the cell).
    fn village_stroll_pos(&mut self, m: &mut Mob) -> Option<[f64; 3]> {
        if self.mobs.rng.next_float() < 0.3 {
            return self.land_random_pos(m, 10, 7);
        }
        let target = if self.mobs.rng.next_float() < 0.7 {
            match self.golem_towards_villager(m) {
                Some(p) => Some(p),
                None => self.golem_towards_poi(m),
            }
        } else {
            match self.golem_towards_poi(m) {
                Some(p) => Some(p),
                None => self.golem_towards_villager(m),
            }
        };
        target.or_else(|| self.land_random_pos(m, 10, 7))
    }

    /// Toward a villager (box widened by 32) who wants a golem, drawn on the world random.
    fn golem_towards_villager(&mut self, m: &mut Mob) -> Option<[f64; 3]> {
        let b = m.bbox();
        let area = rc_physics::Aabb {
            min: [b.min[0] - 32.0, b.min[1] - 32.0, b.min[2] - 32.0],
            max: [b.max[0] + 32.0, b.max[1] + 32.0, b.max[2] + 32.0],
        };
        let wants: Vec<[f64; 3]> = self
            .mobs
            .list
            .iter()
            .flatten()
            .filter(|o| o.kind == Kind::Villager && o.touches(&area) && self.villager_wants_golem(o))
            .map(|o| o.pos)
            .collect();
        if wants.is_empty() {
            return None;
        }
        let to = wants[self.mobs.rng.next_int_bound(wants.len() as i32) as usize];
        self.rpos_land_towards(m, None, 10, 7, to)
    }

    /// Toward an occupied POI of a village section (cube of radius 2 around the golem).
    fn golem_towards_poi(&mut self, m: &mut Mob) -> Option<[f64; 3]> {
        let b = m.block_pos();
        let c = [b[0] >> 4, b[1] >> 4, b[2] >> 4];
        let mut secs = Vec::new();
        // Traversal order of a cube of sections: x fastest, then y, then z.
        for z in -2..=2 {
            for y in -2..=2 {
                for x in -2..=2 {
                    let s = [c[0] + x, c[1] + y, c[2] + z];
                    if self.section_to_village(s) == 0 {
                        secs.push(s);
                    }
                }
            }
        }
        if secs.is_empty() {
            return None;
        }
        let s = secs[self.mobs.rng.next_int_bound(secs.len() as i32) as usize];
        let center = [s[0] * 16 + 8, s[1] * 16 + 8, s[2] * 16 + 8];
        let pois = self.poi_in_range(|_| true, center, 8, crate::poi::Occupancy::IsOccupied);
        if pois.is_empty() {
            return None;
        }
        let p = pois[self.mobs.rng.next_int_bound(pois.len() as i32) as usize].pos;
        self.rpos_land_towards(m, None, 10, 7, [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5])
    }

    /// Spawn of an iron golem near a villager ("legacy" strategy: 10 tries, ±8 horizontal,
    /// descent from +6 to −6); returns true if a golem was born. Called by villagers (panic,
    /// gossip: `spawn_golem_if_needed`, F05).
    pub(crate) fn summon_golem_for_villager(&mut self, center: [i32; 3]) -> bool {
        for _ in 0..10 {
            let dx = self.mobs.rng.next_int_bound(17) - 8;
            let dz = self.mobs.rng.next_int_bound(17) - 8;
            let start = [center[0] + dx, center[1] + 6, center[2] + dz];
            let Some(p) = self.legacy_golem_ground(start, 6) else { continue };
            // Yaw drawn on the world random, "summoned" finalization.
            let yaw = super::super::mth::wrap(self.mobs.rng.next_float() * 360.0);
            let pos = [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5];
            let mut g = self.new_mob(Kind::IronGolem, pos, yaw);
            g.head_yaw = g.yaw;
            g.body_yaw = g.yaw;
            let mut grp = super::super::spawn::Group::default();
            self.finalize_spawn(&mut g, super::super::spawn::Reason::Natural, &mut grp);
            // Spawn rule: walk value ≥ 0 (always true), then obstruction.
            if !self.golem_spawn_obstruction_ok(&g) {
                continue;
            }
            self.add_mob(g);
            return true;
        }
        false
    }

    /// The movement toward a possible spawn position ("legacy" strategy): going down from `range` to
    /// −`range`, first solid (non-excluded) block under an air or liquid cell.
    fn legacy_golem_ground(&self, start: [i32; 3], range: i32) -> Option<[i32; 3]> {
        const EXCLUDED: [&str; 11] = [
            "minecraft:cobweb",
            "minecraft:cactus",
            "minecraft:glass_pane",
            "minecraft:conduit",
            "minecraft:ice",
            "minecraft:tnt",
            "minecraft:glowstone",
            "minecraft:beacon",
            "minecraft:sea_lantern",
            "minecraft:frosted_ice",
            "minecraft:tinted_glass",
        ];
        let mut p = start;
        let mut above = self.st(p);
        for _ in (-range..=range).rev() {
            p[1] -= 1;
            let cur = self.st(p);
            let n = self.state_name(cur);
            let excluded = EXCLUDED.contains(&n)
                || n == "minecraft:glass"
                || n.ends_with("_stained_glass")
                || n.ends_with("_stained_glass_pane")
                || n.ends_with("_leaves");
            let an = self.state_name(above);
            let above_ok = self.v().is_air(above) || an == "minecraft:water" || an == "minecraft:lava";
            if !excluded && above_ok && (self.v().legacy_solid(cur) || n == "minecraft:powder_snow") {
                return Some([p[0], p[1] + 1, p[2]]);
            }
            above = cur;
        }
        None
    }

    /// The iron golem spawn obstruction test: full top face under the feet, two valid spawn cells
    /// above (fluid included), feet cell valid regardless of its fluid, no blocking entity in
    /// the box.
    pub(crate) fn golem_spawn_obstruction_ok(&self, m: &Mob) -> bool {
        let b = m.block_pos();
        if !self.face_full_up(self.st([b[0], b[1] - 1, b[2]])) {
            return false;
        }
        for i in 1..3 {
            if !self.valid_empty_spawn([b[0], b[1] + i, b[2]], Kind::IronGolem) {
                return false;
            }
        }
        self.valid_empty_spawn_dry(b, Kind::IronGolem) && self.unobstructed(&m.bbox(), m.id)
    }
}
