//! Pillager crossbow: charge (25 ticks, loading sounds
//! at 20%, 50% and 100%), projectiles loaded onto the item (component
//! `charged_projectiles`, visible to the client; multishot: intangible copies), aimed shot of
//! each loaded projectile by the common projectile engine (power 1.6, inaccuracy
//! 14 − 4 × difficulty, multishot spread, rockets), shot sound, crossbow wear.

use rc_proto::nbt::Nbt;

use super::super::mob::{Ent, Mob};
use crate::player::Stack;
use crate::projectiles::weapons::{rotate_axis, view_vector};
use crate::server::Server;

/// Component of the loaded projectiles.
const CHARGED: &str = "minecraft:charged_projectiles";

impl Server {
    /// Hand holding the crossbow: 0 right, 1 left.
    fn crossbow_hand(m: &Mob) -> usize {
        if m.equipment[0].as_ref().is_some_and(|s| s.id == "minecraft:crossbow") { 0 } else { 1 }
    }

    /// 1.25 s, minus 0.25 s per level of Quick Charge.
    pub(crate) fn crossbow_charge_duration(&self, m: &Mob) -> i32 {
        let h = Self::crossbow_hand(m);
        let quick = m.equipment[h].as_ref().map_or(0, |s| self.life.items.enchantment(s, "minecraft:quick_charge"));
        let d = (1.25f32 - 0.25 * quick as f32).max(0.0);
        (d * 20.0).floor() as i32
    }

    fn crossbow_charged(m: &Mob) -> bool {
        let h = Self::crossbow_hand(m);
        m.equipment[h]
            .as_ref()
            .and_then(|s| crate::items::component(s, CHARGED))
            .is_some_and(|c| matches!(c, Nbt::List(l) if !l.is_empty()))
    }

    /// Crossbow use tick, called before the use counter advances:
    /// start and middle sounds (flags shared by all crossbows, like the game's single
    /// item), loading and end sound at 100%.
    pub(crate) fn crossbow_use_tick(&mut self, m: &mut Mob) {
        let h = Self::crossbow_hand(m);
        if m.equipment[h].as_ref().is_none_or(|s| s.id != "minecraft:crossbow") {
            return;
        }
        let pct = m.use_ticks as f32 / self.crossbow_charge_duration(m) as f32;
        if pct < 0.2 {
            self.mobs.ill.crossbow_start_played = false;
            self.mobs.ill.crossbow_mid_played = false;
        }
        if pct >= 0.2 && !self.mobs.ill.crossbow_start_played {
            self.mobs.ill.crossbow_start_played = true;
            self.world_sound("minecraft:item.crossbow.loading_start", 7, m.pos, 0.5, 1.0);
        }
        if pct >= 0.5 && !self.mobs.ill.crossbow_mid_played {
            self.mobs.ill.crossbow_mid_played = true;
            self.world_sound("minecraft:item.crossbow.loading_middle", 7, m.pos, 0.5, 1.0);
        }
        if pct >= 1.0 && !Self::crossbow_charged(m) {
            // Ammunition held (off-hand then main hand: arrows,
            // rockets), otherwise an arrow; multishot: intangible copies beyond the first.
            let ammo = [1usize, 0]
                .iter()
                .find_map(|&k| {
                    m.equipment[k].as_ref().filter(|s| {
                        matches!(
                            s.id.as_str(),
                            "minecraft:arrow"
                                | "minecraft:tipped_arrow"
                                | "minecraft:spectral_arrow"
                                | "minecraft:firework_rocket"
                        )
                    })
                })
                .cloned()
                .unwrap_or_else(|| Stack { id: "minecraft:arrow".into(), count: 1, components: None });
            let n = m.equipment[h].clone().map_or(1, |w| {
                let ctx = crate::projectiles::ench::Ctx { tool: Some(&w), ..Default::default() };
                self.ench_value(&w, "minecraft:projectile_count", 1.0, &ctx) as i32
            });
            let mut list = Vec::new();
            for i in 0..n.max(1) {
                let mut c = ammo.clone();
                c.count = 1;
                if i > 0 {
                    crate::items::set_component(&mut c, "minecraft:intangible_projectile", Nbt::Compound(Vec::new()));
                }
                list.push(c.to_nbt(None));
            }
            if let Some(s) = m.equipment[h].as_mut() {
                crate::items::set_component(s, CHARGED, Nbt::List(list));
            }
            self.send_mob_equipment(m, &[h]);
            let p = 1.0 / (self.mobs.rng.next_float() * 0.5 + 1.0) + 0.2;
            self.mob_sound_unmuted(m, "minecraft:item.crossbow.loading_end", 1.0, p);
        }
    }

    /// Loaded projectiles removed (goal interrupted).
    pub(crate) fn crossbow_unload(&mut self, m: &mut Mob) {
        let h = Self::crossbow_hand(m);
        if let Some(s) = m.equipment[h].as_mut()
            && crate::items::component(s, CHARGED).is_some()
        {
            crate::items::remove_component(s, CHARGED);
            self.send_mob_equipment(m, &[h]);
        }
    }

    /// Loaded projectiles removed,
    /// each fired toward the target in the multishot spread, shot sound
    /// per projectile, wear (rocket 3, otherwise 1); stop if the crossbow breaks.
    pub(crate) fn crossbow_shoot(&mut self, m: &mut Mob, target: Ent) {
        let h = Self::crossbow_hand(m);
        let Some(w) = m.equipment[h].clone() else { return };
        if w.id != "minecraft:crossbow" {
            return;
        }
        let list = Self::charged(&w);
        if let Some(s) = m.equipment[h].as_mut() {
            crate::items::remove_component(s, CHARGED);
        }
        if list.is_empty() {
            self.send_mob_equipment(m, &[h]);
            return;
        }
        let (Some(tp), Some(tb)) = (self.ent_pos(target), self.ent_bbox(target)) else {
            self.send_mob_equipment(m, &[h]);
            return;
        };
        let diff = self.level.difficulty_id();
        let unc = (14 - diff * 4) as f32;
        let weapon = m.equipment[h].clone().unwrap_or(w);
        let ctx = crate::projectiles::ench::Ctx { tool: Some(&weapon), ..Default::default() };
        let max_angle = self.ench_value(&weapon, "minecraft:projectile_spread", 0.0, &ctx);
        let n = list.len();
        let step = if n == 1 { 0.0 } else { 2.0 * max_angle / (n - 1) as f32 };
        let offset = ((n - 1) % 2) as f32 * step / 2.0;
        let mut dir = 1.0f32;
        for (index, ammo) in list.into_iter().enumerate() {
            let angle = offset + dir * index.div_ceil(2) as f32 * step;
            dir = -dir;
            let firework = ammo.id == "minecraft:firework_rocket";
            let mut p = if firework {
                let pos = [m.pos[0], m.eye_y() - f64::from(0.15f32), m.pos[2]];
                self.new_firework(pos, ammo.clone(), Some(m.uuid), true)
            } else {
                let mut a = self.mob_create_arrow(m, &ammo, Some(&weapon));
                a.hit_sound = "minecraft:item.crossbow.hit";
                a
            };
            // Firing one projectile toward the target: aim vector rotated by `angle` around its
            // "up" perpendicular.
            let xd = tp[0] - m.pos[0];
            let zd = tp[2] - m.pos[2];
            let dist = (xd * xd + zd * zd).sqrt();
            let yd =
                tb.min[1] + (tb.max[1] - tb.min[1]) * 0.333_333_333_333_333_3 - p.pos[1] + dist * f64::from(0.2f32);
            let v = [xd as f32, yd as f32, zd as f32];
            let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            let inv = 1.0 / l;
            let view = v.map(|c| c * inv);
            let mut right = [-view[2], 0.0, view[0]];
            if right[0] * right[0] + right[2] * right[2] <= 1.0e-7 {
                let up = view_vector(m.pitch - 90.0, m.yaw);
                right = [
                    view[1] * up[2] - view[2] * up[1],
                    view[2] * up[0] - view[0] * up[2],
                    view[0] * up[1] - view[1] * up[0],
                ];
            }
            let half_pi = std::f32::consts::FRAC_PI_2;
            let axis = rotate_axis(view, half_pi, right);
            let shot = rotate_axis(view, angle * (std::f32::consts::PI / 180.0), axis);
            p.shoot(shot.map(f64::from), 1.6, unc);
            self.proj_spawn(p, &ammo);
            let pitch = if index == 0 {
                1.0
            } else {
                let high = index & 1 == 1;
                1.0 / (m.rng.next_float() * 0.5 + 1.8) + if high { 0.63 } else { 0.43 }
            };
            self.mob_sound_unmuted(m, "minecraft:item.crossbow.shoot", 1.0, pitch);
            self.mob_hurt_item(m, h, if firework { 3 } else { 1 });
            if m.equipment[h].is_none() {
                break;
            }
        }
        self.send_mob_equipment(m, &[h]);
    }
}
