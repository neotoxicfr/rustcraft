//! Ravager: base speed sliding toward 0.3 (0.35 with a target), foliage torn off on
//! collision (rule `mob_griefing`) otherwise jump, hit (10 ticks), stun
//! (40 ticks) then roar (damage 6 and strong push at 4 blocks).

use super::super::mob::{Ent, Mob};
use super::super::species::Kind;
use crate::server::Server;

impl Server {
    /// End of the ravager's AI step (after the rest of the AI).
    pub(crate) fn ravager_ai_step(&mut self, m: &mut Mob) {
        if super::immobile(m) {
            m.attrs.speed.base = 0.0;
        } else {
            let max = if m.target.is_some() { 0.35 } else { 0.3 };
            let base = m.attrs.speed.base;
            m.attrs.speed.base = base + 0.1 * (max - base);
        }
        if m.horizontal_collision && self.rule("minecraft:mob_griefing", true) {
            let b = m.bbox();
            let (lo, hi) = (b.min.map(|v| (v - 0.2).floor() as i32), b.max.map(|v| (v + 0.2).floor() as i32));
            let mut destroyed = false;
            for x in lo[0]..=hi[0] {
                for y in lo[1]..=hi[1] {
                    for z in lo[2]..=hi[2] {
                        let p = [x, y, z];
                        if self.btag_id(p, crate::tag_bits::TagId::Leaves) {
                            destroyed = self.mob_destroy_block(p, true, m.id) || destroyed;
                        }
                    }
                }
            }
            if !destroyed && m.on_ground {
                self.jump_from_ground(m);
            }
        }
        if m.ill.roar_tick > 0 {
            m.ill.roar_tick -= 1;
            if m.ill.roar_tick == 10 {
                self.ravager_roar(m);
            }
        }
        if m.ill.attack_tick > 0 {
            m.ill.attack_tick -= 1;
        }
        if m.ill.stun_tick > 0 {
            m.ill.stun_tick -= 1;
            // Stun particles (client side): one draw per tick.
            if m.rng.next_int_bound(6) == 0 {
                let _ = m.rng.next_double();
                let _ = m.rng.next_double();
            }
            if m.ill.stun_tick == 0 {
                self.mob_sound(m, "minecraft:entity.ravager.roar", 1.0, 1.0);
                m.ill.roar_tick = 20;
            }
        }
    }

    /// `roar`: mobs within 4 blocks (except ravagers; without `mob_griefing` or armor stands),
    /// 6 damage except illagers, push except player; event 69.
    fn ravager_roar(&mut self, m: &mut Mob) {
        if !m.alive() {
            return;
        }
        let b = m.bbox();
        let area = rc_physics::Aabb { min: b.min.map(|v| v - 4.0), max: b.max.map(|v| v + 4.0) };
        let ids: Vec<i32> = self
            .mobs
            .list
            .iter()
            .flatten()
            .filter(|o| o.alive() && o.kind != Kind::Ravager && o.touches(&area))
            .map(|o| o.id)
            .collect();
        for id in ids {
            let Some(i) = self.mob_index(id) else { continue };
            let mut o = self.mobs.list[i].take().expect("mob");
            if !super::is_illager(o.kind) {
                self.mob_hurt_by(&mut o, m, "minecraft:mob_attack", 6.0);
            }
            strong_knockback(m.pos, &mut o);
            self.mobs.list[i] = Some(o);
        }
        if self.game_mode != 3 && self.ent_alive(Ent::Player) && self.player_aabb().intersects(&area) {
            self.player_hurt_by_mob(
                m,
                "minecraft:mob_attack",
                6.0,
                Some((m.pos[0] - self.pos[0], m.pos[2] - self.pos[2])),
            );
        }
        self.mob_event(m, 69);
    }
}

impl Server {
    /// Destruction of a block by an entity by a mob: block loot (empty tool),
    /// particles and sound (event 2001), block removed.
    pub(crate) fn mob_destroy_block(&mut self, p: [i32; 3], drop: bool, by: i32) -> bool {
        // The loot is handled by the common destruction (`destroy_block_by`).
        self.destroy_block_by(p, drop, 512, Some(crate::mobs::recent::vibration::Src::Living(Ent::Mob(by))))
    }
}

/// Horizontal push 4 / d², vertical 0.2.
fn strong_knockback(from: [f64; 3], o: &mut Mob) {
    let xd = o.pos[0] - from[0];
    let zd = o.pos[2] - from[2];
    let dd = (xd * xd + zd * zd).max(0.001);
    o.vel = [o.vel[0] + xd / dd * 4.0, o.vel[1] + 0.2, o.vel[2] + zd / dd * 4.0];
    o.needs_sync = true;
}
