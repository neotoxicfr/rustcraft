//! L10-COMBAT: damage dealt to a mannequin, the living entity the game makes of it (explosions,
//! the sweep): refused while invulnerable or dead, or within its hurt cooldown when it is not
//! more than the last hit; then the armor and toughness of its attributes (modified by the
//! weapon of a direct attacker), the resistance effect, the protection of its enchanted gear,
//! its absorption, its health; the hurt event, its velocity sent back, the default knockback
//! (none for the types without knockback), the generic hurt sound; at zero health the death
//! sound, its death (game event, death event, dying pose; its loot table is empty) and its
//! removal 20 ticks later. Also the knockback of the living decorations (stands, mannequins).

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::{DKind, dbl, flag};
use crate::commands::ents::E;
use crate::mobs::mob::Ent;
use crate::mobs::recent::vibration::Src;
use crate::nbtio;
use crate::player::Stack;
use crate::progress::pred::Who;
use crate::server::Server;

const TYPE: &str = "minecraft:mannequin";

/// Float field of a compound (`d` when absent or of another kind).
fn float(e: &Nbt, k: &str, d: f32) -> f32 {
    match e.get(k) {
        Some(Nbt::Float(v)) => *v,
        _ => d,
    }
}

/// Amplifier of the resistance effect among the saved effects, if any.
fn resistance(e: &Nbt) -> Option<i32> {
    nbtio::list(e.get("active_effects"))
        .iter()
        .find(|x| x.get("id").and_then(Nbt::as_str) == Some("minecraft:resistance"))
        .map(|x| nbtio::int(x, "amplifier").map_or(0, |a| i32::from(a as i8)))
}

impl Server {
    /// Damage `amount` of type `ty` dealt to mannequin `id` (author `by`, source position `src`,
    /// knockback direction `dir` if given as is); true if the hit landed.
    pub(crate) fn mannequin_hurt(&mut self, id: i32, ty: &str, amount: f32, by: Option<Ent>, src: Option<[f64; 3]>) -> bool {
        let Some(i) = self.deco.live.iter().position(|l| l.id == id && !l.removed && l.kind == DKind::Mannequin) else {
            return false;
        };
        let Some(e) = self.live_nbt(i).cloned() else { return false };
        let bypass = self.dmg_tag(ty, "minecraft:bypasses_invulnerability");
        if (flag(&e, "Invulnerable") && !bypass && !(by == Some(Ent::Player) && self.game_mode == 1))
            || float(&e, "Health", 20.0) <= 0.0
        {
            return false;
        }
        if self.dmg_tag(ty, "minecraft:is_fire")
            && nbtio::list(e.get("active_effects")).iter().any(|x| x.get("id").and_then(Nbt::as_str) == Some("minecraft:fire_resistance"))
        {
            return false;
        }
        let mut damage = amount.max(0.0);
        if !damage.is_finite() {
            damage = f32::MAX;
        }
        let bypass_cd = self.dmg_tag(ty, "minecraft:bypasses_cooldown");
        let l = &self.deco.live[i];
        let (cooldown, last) = (l.hurt_cooldown, l.last_hurt);
        let full = if cooldown > 10 && !bypass_cd {
            if damage <= last {
                return false;
            }
            self.mannequin_actually_hurt(i, &e, ty, damage - last, by);
            self.deco.live[i].last_hurt = damage;
            false
        } else {
            let l = &mut self.deco.live[i];
            l.last_hurt = damage;
            l.hurt_cooldown = 20;
            self.mannequin_actually_hurt(i, &e, ty, damage, by);
            self.deco_edit(id, |n| nbtio::set(n, "HurtTime", Nbt::Short(10)));
            true
        };
        let pos = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        if full {
            let _ = self.send(Event::Entity(EntityEvent::Hurt { id, yaw: 0.0 }));
            if !self.dmg_tag(ty, "minecraft:no_impact") {
                self.deco.live[i].track.sync_velocity = true;
            }
            if !self.dmg_tag(ty, "minecraft:no_knockback") {
                let (xd, zd) = crate::mobs::combat::hurt_knockback_dir(None, src, pos);
                self.deco_knockback(id, 0.4, xd, zd);
            }
        }
        let dead = self.deco_nbt(id).is_some_and(|n| float(n, "Health", 20.0) <= 0.0);
        if dead {
            if full {
                self.deco_sound(&e, "minecraft:entity.generic.death", 6, 1.0, self.living_pitch());
            }
            // The kill credited to the player who dealt it, before the death's own effects.
            if by == Some(Ent::Player) {
                let uuid = nbtio::uuid(e.get("UUID")).unwrap_or(0);
                self.s9_living_killed(TYPE, uuid, Who::E(E::Other(id)), pos, &|s| s.ent_display(E::Other(id)), ty, by);
            }
            // Emitted by the mannequin itself (never a player behind it: no shrieker answers).
            self.game_event_src("minecraft:entity_die", pos, Some(Src::Other), None);
            let _ = self.send(Event::Entity(EntityEvent::Event { id, event: 3 }));
            self.deco_data(id, vec![(6, Meta::Pose(7))]);
        } else if full {
            self.deco_sound(&e, "minecraft:entity.generic.hurt", 6, 1.0, self.living_pitch());
        }
        // Any hurt whose source is the player, once past the cooldown.
        if by == Some(Ent::Player) {
            self.s9_player_hurt_living(Some(Who::E(E::Other(id))), ty, amount.max(0.0), damage, false);
        }
        true
    }

    /// Voice pitch of a living entity, 1 ± 0.2 (the decoration keeps no random of its own: the
    /// sound seeds' source).
    fn living_pitch(&self) -> f32 {
        let mut r = crate::mobs::jrand::JRand::new(crate::vitals::sound_seed());
        (r.next_float() - r.next_float()) * 0.2 + 1.0
    }

    /// Armor, effects and enchantments, absorption, then health (`i`: registry index).
    fn mannequin_actually_hurt(&mut self, i: usize, e: &Nbt, ty: &str, mut dmg: f32, by: Option<Ent>) {
        let id = self.deco.live[i].id;
        let pos = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        let gear: Vec<Option<Stack>> = crate::mobs::mob::SLOT_NAMES
            .iter()
            .map(|k| e.get("equipment").and_then(|q| q.get(k)).and_then(Stack::from_nbt))
            .collect();
        let mut own = crate::mobs::jrand::JRand::new(0);
        if !self.dmg_tag(ty, "minecraft:bypasses_armor") {
            let armor = (self.nbt_attribute(e, TYPE, "minecraft:armor").unwrap_or(0.0) as f32).floor();
            let t = 2.0 + self.nbt_attribute(e, TYPE, "minecraft:armor_toughness").unwrap_or(0.0) as f32 / 4.0;
            let a = (armor - dmg / t).clamp(armor * 0.2, 20.0);
            let mut frac = a / 25.0;
            if let Some(w) = self.source_weapon(ty, by) {
                let ctx = self.hurt_ctx(ty, Who::E(E::Other(id)), pos, by);
                frac = self.armor_fraction(frac, Some(&w), &ctx, &mut own);
            }
            dmg *= 1.0 - frac;
        }
        if !self.dmg_tag(ty, "minecraft:bypasses_effects") {
            if !self.dmg_tag(ty, "minecraft:bypasses_resistance")
                && let Some(a) = resistance(e)
            {
                let before = dmg;
                dmg = (dmg * (25 - (a + 1) * 5) as f32 / 25.0).max(0.0);
                let resisted = before - dmg;
                if by == Some(Ent::Player) && resisted > 0.0 && resisted < 3.402_823_5e37 {
                    self.award_custom("damage_dealt_resisted", (resisted * 10.0).round() as i32);
                }
            }
            if dmg > 0.0 && !self.dmg_tag(ty, "minecraft:bypasses_enchantments") && gear.iter().any(Option::is_some) {
                let mut world = self.mobs.rng;
                let p = {
                    let g: Vec<(usize, Option<&Stack>)> = gear.iter().enumerate().map(|(k, s)| (k, s.as_ref())).collect();
                    let ctx = self.hurt_ctx(ty, Who::E(E::Other(id)), pos, by);
                    self.ench_protection(&g, &ctx, &mut world, &mut own)
                };
                self.mobs.rng = world;
                if p > 0.0 {
                    dmg *= 1.0 - p.clamp(0.0, 20.0) / 25.0;
                }
            }
        }
        let absorption = float(e, "AbsorptionAmount", 0.0);
        let original = dmg;
        dmg = (dmg - absorption).max(0.0);
        let mut absorption = (absorption - (original - dmg)).max(0.0);
        let absorbed = original - dmg;
        if by == Some(Ent::Player) && absorbed > 0.0 && absorbed < 3.402_823_5e37 {
            self.award_custom("damage_dealt_absorbed", (absorbed * 10.0).round() as i32);
        }
        if dmg == 0.0 {
            self.deco_edit(id, |n| nbtio::set(n, "AbsorptionAmount", Nbt::Float(absorption)));
            return;
        }
        let max = self.nbt_attribute(e, TYPE, "minecraft:max_health").unwrap_or(20.0) as f32;
        let health = (float(e, "Health", 20.0) - dmg).clamp(0.0, max);
        absorption = (absorption - dmg).max(0.0);
        self.deco_edit(id, |n| {
            nbtio::set(n, "Health", Nbt::Float(health));
            nbtio::set(n, "AbsorptionAmount", Nbt::Float(absorption));
        });
        self.deco_data(id, vec![(9, Meta::Float(health))]);
        // Emitted by the mannequin itself, whoever hurt it.
        self.game_event_src("minecraft:entity_damage", pos, Some(Src::Other), None);
    }

    /// Knockback of a living decoration (armor stand, mannequin), like any living entity: power
    /// reduced by its knockback resistance, half of its motion kept, lifted when on the ground;
    /// its velocity goes out with its tracking.
    pub(crate) fn deco_knockback(&mut self, id: i32, power: f64, mut xd: f64, mut zd: f64) {
        let Some(e) = self.deco_nbt(id) else { return };
        let Some(ty) = e.get("id").and_then(Nbt::as_str) else { return };
        let res = self.nbt_attribute(e, ty, "minecraft:knockback_resistance").unwrap_or(0.0);
        let power = power * (1.0 - res);
        if power <= 0.0 {
            return;
        }
        // Direction too short: drawn at random (the decoration keeps no random: a fixed one).
        let mut r = crate::mobs::jrand::JRand::new(i64::from(id));
        while xd * xd + zd * zd < f64::from(1.0e-5f32) {
            xd = (r.next_double() - r.next_double()) * 0.01;
            zd = (r.next_double() - r.next_double()) * 0.01;
        }
        let l = (xd * xd + zd * zd).sqrt();
        let (dx, dz) = (xd / l * power, zd / l * power);
        let v = nbtio::motion(e.get("Motion")).unwrap_or_default();
        let ground = flag(e, "OnGround");
        let nv = [v[0] / 2.0 - dx, if ground { (v[1] / 2.0 + power).min(0.4) } else { v[1] }, v[2] / 2.0 - dz];
        self.deco_edit(id, |n| nbtio::set(n, "Motion", dbl(nv)));
        if let Some(l) = self.deco_live(id) {
            l.track.needs_sync = true;
        }
    }

    /// Countdowns of a mannequin's tick: hurt cooldown, hurt animation, and death (removed 20
    /// ticks after it, with its smoke event). True if it was removed.
    pub(super) fn mannequin_tick_counts(&mut self, i: usize) -> bool {
        let l = &mut self.deco.live[i];
        if l.hurt_cooldown > 0 {
            l.hurt_cooldown -= 1;
        }
        let id = l.id;
        let Some(e) = self.live_nbt(i) else { return false };
        let hurt = nbtio::int(e, "HurtTime").unwrap_or(0);
        let dying = float(e, "Health", 20.0) <= 0.0;
        let death = nbtio::int(e, "DeathTime").unwrap_or(0);
        if hurt <= 0 && !dying {
            return false;
        }
        let death = if dying { death + 1 } else { death };
        self.deco_edit_at(i, |n| {
            if hurt > 0 {
                nbtio::set(n, "HurtTime", Nbt::Short((hurt - 1) as i16));
            }
            if dying {
                nbtio::set(n, "DeathTime", Nbt::Short(death as i16));
            }
        });
        if dying && death >= 20 {
            let _ = self.send(Event::Entity(EntityEvent::Event { id, event: 60 }));
            self.deco_remove(id);
            return true;
        }
        false
    }
}
