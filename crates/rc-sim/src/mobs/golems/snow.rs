//! Snow golem (§ 4.5): snowballs at enemies, snow trail, melts in hot biomes and the Nether,
//! fears water and rain, pumpkin shearing.

use super::super::mob::{Ent, Mob};
use super::super::mth::dist2;
use super::GolemGoal;
use crate::server::Server;

/// End of the AI step: melting (attribute `gameplay/snow_golem_melts`), snow trail (rule
/// `mob_griefing`, four corners at ±0.25).
pub fn post_ai_step(s: &mut Server, m: &mut Mob) {
    if s.snow_golem_melts(m.block_pos()) {
        s.mob_hurt(m, "minecraft:on_fire", 1.0, None, None);
    }
    if !s.rule("minecraft:mob_griefing", true) {
        return;
    }
    let snow = s.default_state("minecraft:snow");
    for i in 0..4 {
        let xx = (m.pos[0] + f64::from((i % 2 * 2 - 1) as f32 * 0.25)).floor() as i32;
        let yy = m.pos[1].floor() as i32;
        let zz = (m.pos[2] + f64::from((i / 2 % 2 * 2 - 1) as f32 * 0.25)).floor() as i32;
        let p = [xx, yy, zz];
        if s.is_air(p) && s.can_survive(snow, p) {
            s.set_block(p, snow, 3);
            s.game_event_block("minecraft:block_place", p, Some(super::super::mob::Ent::Mob(m.id)), Some(snow));
        }
    }
}

/// Shears: pumpkin removed, shearing loot dropped at eye height.
pub fn interact(s: &mut Server, m: &mut Mob, slot: usize, item: &str) -> Option<bool> {
    if item != "minecraft:shears" || !m.golem.pumpkin {
        return None;
    }
    s.entity_sound(m.id, "minecraft:entity.snow_golem.shear", 7, 1.0, 1.0);
    m.golem.pumpkin = false;
    for d in s.simple_loot("minecraft:shearing/snow_golem", None) {
        let eye = m.eye_height();
        s.drop_at_height(m, d, eye);
    }
    s.sheared_by_player(m);
    s.hurt_item(slot, 1);
    Some(true)
}

impl Server {
    /// Melting: biome attribute, otherwise the dimension's (Nether).
    pub(crate) fn snow_golem_melts(&self, p: [i32; 3]) -> bool {
        let key = "minecraft:gameplay/snow_golem_melts";
        let biome = self.biome_str(p);
        if let Some(v) = self.life.pack.get("worldgen/biome", biome)
            && let Some(b) = v["attributes"][key].as_bool()
        {
            return b;
        }
        self.life
            .pack
            .get("dimension_type", &self.dim.name)
            .and_then(|v| v["attributes"][key].as_bool())
            .unwrap_or(false)
    }

    /// Throw of a snowball.
    fn snow_golem_shoot(&mut self, m: &mut Mob, t: Ent) {
        let (Some(tp), Some(eye)) = (self.ent_pos(t), self.ent_eye(t)) else { return };
        let xd = tp[0] - m.pos[0];
        let yd = eye - f64::from(1.1f32);
        let zd = tp[2] - m.pos[2];
        let yo = (xd * xd + zd * zd).sqrt() * f64::from(0.2f32);
        let start = [m.pos[0], m.eye_y() - f64::from(0.1f32), m.pos[2]];
        let dir = [xd, yd + yo - start[1], zd];
        self.throw_snowball(m.uuid, start, dir, 1.6, 12.0);
        let p = 0.4 / (m.rng.next_float() * 0.4 + 0.8);
        self.mob_sound(m, "minecraft:entity.snow_golem.shoot", 1.0, p);
    }

    pub(crate) fn snow_can_use(&mut self, g: &mut GolemGoal, m: &mut Mob) -> bool {
        let GolemGoal::Ranged { target, .. } = g else { return false };
        match m.target.filter(|t| self.ent_alive(*t)) {
            Some(t) => {
                *target = Some(t);
                true
            }
            None => false,
        }
    }

    pub(crate) fn snow_can_continue(&mut self, g: &mut GolemGoal, m: &mut Mob) -> bool {
        if self.snow_can_use(g, m) {
            return true;
        }
        let GolemGoal::Ranged { target, .. } = g else { return false };
        target.is_some_and(|t| self.ent_alive(t)) && !m.nav.is_done()
    }

    pub(crate) fn snow_stop(&mut self, g: &mut GolemGoal) {
        if let GolemGoal::Ranged { target, attack_time, see_time } = g {
            *target = None;
            *see_time = 0;
            *attack_time = -1;
        }
    }

    /// The tick of the ranged attack objective (speed 1.25, interval 20, radius 10).
    pub(crate) fn snow_tick(&mut self, g: &mut GolemGoal, m: &mut Mob) {
        const RADIUS: f32 = 10.0;
        let GolemGoal::Ranged { target, attack_time, see_time } = g else { return };
        let Some(t) = *target else { return };
        let (Some(tp), Some(eye)) = (self.ent_pos(t), self.ent_eye(t)) else { return };
        let d2 = dist2(m.pos, tp);
        let sight = self.line_of_sight(m, t);
        if sight {
            *see_time += 1;
        } else {
            *see_time = 0;
        }
        if d2 <= f64::from(RADIUS * RADIUS) && *see_time >= 5 {
            m.nav.stop();
        } else {
            self.move_to_entity(m, tp, 1.25);
        }
        self.look_at(m, [tp[0], eye, tp[2]], 30.0, 30.0);
        *attack_time -= 1;
        if *attack_time == 0 {
            if !sight {
                return;
            }
            // Power (distance / radius) has no effect on the snowball; interval
            // `⌊d × (max − min) + min⌋` = 20 (min = max).
            self.snow_golem_shoot(m, t);
            *attack_time = 20;
        } else if *attack_time < 0 {
            // Interval min = max = 20: constant interpolation.
            *attack_time = 20;
        }
    }
}
