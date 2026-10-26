//! Witch: potions drunk (water breathing, fire resistance, healing, swiftness; 32 ticks,
//! speed −0.25), ally healing delay in a raid, witch particles (event 15) and
//! thrown potions (common projectile engine).

use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::super::mob::{Ent, Mob};
use super::super::mth::dist2;
use super::super::species::Kind;
use crate::player::Stack;
use crate::projectiles::{PKind, Proj};
use crate::server::Server;

pub(crate) fn potion_stack(item: &str, potion: &str) -> Stack {
    let mut s = Stack { id: item.into(), count: 1, components: None };
    crate::items::set_component(
        &mut s,
        "minecraft:potion_contents",
        Nbt::Compound(vec![("potion".into(), Nbt::String(potion.into()))]),
    );
    s
}

impl Server {
    /// Start of the witch's AI step (before the raid pillager's).
    pub(crate) fn witch_ai_step(&mut self, m: &mut Mob) {
        m.ill.heal_cooldown -= 1;
        m.ill.attack_players = m.ill.heal_cooldown <= 0;
        if m.ill.drinking {
            let t = m.ill.using_time;
            m.ill.using_time -= 1;
            if t <= 0 {
                m.ill.drinking = false;
                let held = m.equipment[0].take();
                self.send_mob_equipment(m, &[0]);
                if let Some(s) = held
                    && s.id == "minecraft:potion"
                    && let Some(p) = crate::potions::potion_of(&s)
                {
                    // Even instant healing goes through the list (applied at the next effects tick,
                    // S7), unlike thrown potions.
                    for &(id, dur, amp) in crate::potions::potion_effects(&p) {
                        self.give_mob_effect(m, id, dur, amp);
                    }
                }
                self.mob_game_event(m, "minecraft:drink");
                m.attrs.speed.remove_mod("minecraft:drinking");
            }
        } else {
            let eye_water = m.eyes_in_water;
            let has = |m: &Mob, id: &str| m.x.effects.iter().any(|e| e.id == id);
            let fire_hit = m.last_hit.as_ref().is_some_and(|h| self.dmg_tag(&h.ty, "minecraft:is_fire"));
            let potion = if m.rng.next_float() < 0.15 && eye_water && !has(m, "minecraft:water_breathing") {
                Some("minecraft:water_breathing")
            } else if m.rng.next_float() < 0.15 && (m.on_fire() || fire_hit) && !has(m, "minecraft:fire_resistance") {
                Some("minecraft:fire_resistance")
            } else if m.rng.next_float() < 0.05 && m.health < m.max_health() {
                Some("minecraft:healing")
            } else if m.rng.next_float() < 0.5
                && m.target.is_some()
                && !has(m, "minecraft:speed")
                && m.target.and_then(|t| self.ent_pos(t)).is_some_and(|p| dist2(p, m.pos) > 121.0)
            {
                Some("minecraft:swiftness")
            } else {
                None
            };
            if let Some(p) = potion {
                m.equipment[0] = Some(potion_stack("minecraft:potion", p));
                self.send_mob_equipment(m, &[0]);
                m.ill.using_time = 32;
                m.ill.drinking = true;
                // Silent: neither sound nor pitch draw.
                if !m.x.silent {
                    let pitch = 0.8 + m.rng.next_float() * 0.4;
                    self.mob_sound(m, "minecraft:entity.witch.drink", 1.0, pitch);
                }
                m.attrs.speed.remove_mod("minecraft:drinking");
                m.attrs.speed.set_mod("minecraft:drinking", -0.25, 0);
            }
        }
        if m.rng.next_float() < 7.5e-4 {
            self.mob_event(m, 15);
        }
    }

    /// Villager struck by lightning (outside peaceful): replaced by a witch
    /// (position, rotation, name, AI copied; persistent; finalized "conversion"; POIs returned).
    /// True if converted.
    pub(crate) fn villager_struck(&mut self, v: &mut Mob) -> bool {
        if self.level.difficulty_id() == 0 || v.removed {
            return false;
        }
        let mut w = self.new_mob(Kind::Witch, v.pos, v.yaw);
        w.pitch = v.pitch;
        w.head_yaw = v.head_yaw;
        w.body_yaw = v.body_yaw;
        w.custom_name = v.custom_name.clone();
        w.no_ai = v.no_ai;
        w.persistent = v.persistent;
        let mut g = super::super::spawn::Group::default();
        self.finalize_spawn(&mut w, super::super::spawn::Reason::Conversion, &mut g);
        w.persistent = true;
        self.release_all_pois(v);
        v.removed = true;
        self.add_mob(w);
        true
    }

    /// The witch's ranged attack: potion chosen according to the target, thrown toward its future
    /// position, throw sound.
    pub(crate) fn witch_throw(&mut self, m: &mut Mob, target: Ent, _power: f32) {
        if m.ill.drinking {
            return;
        }
        let (Some(tp), Some(eye)) = (self.ent_pos(target), self.ent_eye(target)) else { return };
        let tv = match target {
            Ent::Player => self.mobs.player_vel,
            Ent::Mob(id) => self.mob_ref(id).map_or([0.0; 3], |o| o.vel),
        };
        let xd = tp[0] + tv[0] - m.pos[0];
        let yd = eye - f64::from(1.1f32) - m.pos[1];
        let zd = tp[2] + tv[2] - m.pos[2];
        let dist = (xd * xd + zd * zd).sqrt();
        let (t_health, t_raider, has) = match target {
            Ent::Player => (self.life.vit.health, false, self.fx().has("minecraft:slowness")),
            Ent::Mob(id) => match self.mob_ref(id) {
                Some(o) => (o.health, super::is_raider(o.kind), false),
                None => return,
            },
        };
        let target_has = |id: &str| match target {
            Ent::Player => self.fx().has(id),
            Ent::Mob(mid) => self.mob_ref(mid).is_some_and(|o| o.x.effects.iter().any(|e| e.id == id)),
        };
        let mut potion = "minecraft:harming";
        if t_raider {
            potion = if t_health <= 4.0 { "minecraft:healing" } else { "minecraft:regeneration" };
            m.target = None;
        } else if dist >= 8.0 && !(has || target_has("minecraft:slowness")) {
            potion = "minecraft:slowness";
        } else if t_health >= 8.0 && !target_has("minecraft:poison") {
            potion = "minecraft:poison";
        } else if dist <= 3.0 && !target_has("minecraft:weakness") && m.rng.next_float() < 0.25 {
            potion = "minecraft:weakness";
        }
        let pow = if dist <= 2.0 { 0.45f32 } else { 0.75 };
        self.spawn_thrown_potion(m, potion, [xd, yd + dist * 0.2, zd], pow, 8.0);
        if !m.x.silent {
            let pitch = 0.8 + m.rng.next_float() * 0.4;
            self.mob_sound(m, "minecraft:entity.witch.throw", 1.0, pitch);
        }
    }

    /// Potion thrown from the thrower's eyes − 0.1, by the common
    /// projectile engine (`projectiles/`, S8: flight, 4-block splash, durations scaled to
    /// the distance, events 2002 / 2007).
    fn spawn_thrown_potion(&mut self, m: &Mob, potion: &str, dir: [f64; 3], pow: f32, unc: f32) {
        let id = self.proj_new_id();
        let pos = [m.pos[0], m.eye_y() - f64::from(0.1f32), m.pos[2]];
        let one = potion_stack("minecraft:splash_potion", potion);
        let mut p = Proj::new(PKind::SplashPotion, id, pos, one.clone());
        p.owner = Some(m.uuid);
        let net = self.net(Some(&one));
        p.set_meta(8, Meta::Item(net));
        p.shoot(dir, pow, unc);
        self.proj_spawn(p, &one);
    }
}
