//! Fireballs of mobs (F12): small blaze fireball (damage
//! 5, sets fire for 5 s, places fire on the block hit) and large ghast fireball (damage 6,
//! explosion of power 1 with fire if the `mob_griefing` rule allows it, can be sent back by a player
//! hit). Flight, impacts, network tracking and saving are those of the common projectile
//! engine (`projectiles/`, kinds `SmallFireball` and `LargeFireball`); this module only
//! keeps the shot and the return by the player.

use super::mob::Ent;
use super::mth::{dist2, normalize};
use crate::player::Stack;
use crate::projectiles::{PKind, Proj};
use crate::server::Server;

/// State common to the mobs' fireballs.
#[derive(Debug, Clone, Default)]
pub struct FireballsWorld {
    /// True during the damage of a large fireball (direct hit or explosion): the direct
    /// source of the damage is then a large fireball.
    pub direct_large: bool,
}

impl Server {
    /// Launches a fireball: velocity =
    /// normalised direction × 0.1, rotation of the owner, at `pos`. `owner`: UUID of the owner (the
    /// shooting mob is out of the list during its tick).
    pub(crate) fn shoot_fireball(
        &mut self,
        owner: Option<u128>,
        pos: [f64; 3],
        dir: [f64; 3],
        rot: (f32, f32),
        large: bool,
        power: i32,
    ) {
        let id = self.proj_new_id();
        let kind = if large { PKind::LargeFireball } else { PKind::SmallFireball };
        let item = Stack { id: kind.default_item().into(), count: 1, components: None };
        let mut f = Proj::new(kind, id, pos, item);
        f.owner = owner;
        f.yaw = rot.0;
        f.pitch = rot.1;
        f.power = power;
        f.vel = normalize(normalize(dir)).map(|c| c * f.accel);
        // Always "on fire": entity flag 1.
        f.fire = 20;
        f.sync_fire_flag();
        self.proj_add(f);
    }

    /// Player hit on a fireball (large fireball only); true if
    /// the identifier is a fireball (the hit is then handled here).
    pub(crate) fn fireball_attacked(&mut self, id: i32) -> bool {
        // F16: the wither skull cannot be sent back (outside the tag of deflectable projectiles).
        let Some(i) = self
            .proj
            .list
            .iter()
            .position(|p| p.id == id && !p.removed && p.kind.hurting() && p.kind != PKind::WitherSkull)
        else {
            return false;
        };
        if self.game_mode == 3 || self.life.vit.dead {
            return true;
        }
        // Interaction range (3 + margin 3), as for mobs.
        let b = self.proj.list[i].bbox();
        let eye = [self.pos[0], self.player_eye_y(), self.pos[2]];
        let c: [f64; 3] = std::array::from_fn(|k| eye[k].clamp(b.min[k], b.max[k]));
        if dist2(eye, c) >= 36.0 {
            return true;
        }
        self.mobs.attack_ticker = 0;
        let pos = self.pos;
        // Tag `redirectable_projectile`: large fireball and wind charges; the player's
        // charge refuses deflection during its first 5 ticks. Otherwise the hit has no effect.
        let p = &self.proj.list[i];
        let redirectable = match p.kind {
            PKind::LargeFireball | PKind::BreezeWindCharge => true,
            PKind::WindCharge => p.tick_count >= 5,
            _ => false,
        };
        if !redirectable {
            self.world_sound("minecraft:entity.player.attack.nodamage", 7, pos, 1.0, 1.0);
            return true;
        }
        // "Aimed" deflection: velocity = player's look (power 1), the player becomes the owner,
        // acceleration reset to 0.1, velocity sent at the next tracking (velocity send requested).
        let look = super::nether::ghast::view_vector(self.pitch, self.yaw);
        let uuid = self.uuid;
        let f = &mut self.proj.list[i];
        f.vel = look;
        f.owner = Some(uuid);
        f.accel = 0.1;
        f.last_deflected_by = Some(Ent::Player);
        f.needs_sync = true;
        self.world_sound("minecraft:entity.player.attack.nodamage", 7, pos, 1.0, 1.0);
        true
    }

    /// UUID of the author of an explosion (mob held out of the list included).
    pub(crate) fn blast_owner_uuid(&self, e: Option<Ent>, held: Option<&super::mob::Mob>) -> Option<u128> {
        match e? {
            Ent::Player => Some(self.uuid),
            Ent::Mob(id) => held
                .filter(|m| m.id == id)
                .map(|m| m.uuid)
                .or_else(|| self.mob_ref(id).map(|m| m.uuid))
                .or_else(|| self.mobs.ticking.iter().rev().find(|t| t.1 == id).map(|t| t.0)),
        }
    }

    /// Projectiles in an explosion or a burst, like any entity: each one pushed from its eyes by
    /// the visible part of its box (× `kb_mult`), the velocity going out with its tracking. The
    /// damage of an ordinary explosion (`hurt`) marks them hurt (velocity sent back as is), except
    /// the hurting projectiles and the eyes of ender, which take none, and the shulker bullets,
    /// destroyed by the explosion elsewhere. Those of the tag `redirectable_projectile` (large
    /// fireball, wind charges) then belong to the author of the explosion's damage (none if
    /// anonymous). The bobber and the llama spits are pushed and hurt too.
    pub(crate) fn explosion_projectiles(&mut self, center: [f64; 3], radius: f32, uuid: Option<u128>, kb_mult: f64, hurt: bool) {
        let double = f64::from(radius * 2.0);
        // Push of an entity at `pos`, eyes `eye` above, box `b`, if in range.
        let push = |s: &Server, pos: [f64; 3], ty: &str, b: &rc_physics::Aabb| -> Option<[f64; 3]> {
            let dist = dist2(pos, center).sqrt() / double;
            if dist > 1.0 {
                return None;
            }
            let eye = f64::from(s.type_dims(ty).map_or(0.0, |d| d[2]));
            let dir = normalize([pos[0] - center[0], pos[1] + eye - center[1], pos[2] - center[2]]);
            let exposure = if !hurt && kb_mult == 0.0 { 0.0 } else { f64::from(s.seen_percent(center, b)) };
            let kp = (1.0 - dist) * exposure * kb_mult;
            Some(dir.map(|d| d * kp))
        };
        for i in 0..self.proj.list.len() {
            let p = &self.proj.list[i];
            if p.removed || (hurt && p.kind == PKind::ShulkerBullet) {
                continue;
            }
            let Some(kb) = push(self, p.pos, p.kind.name(), &p.bbox()) else { continue };
            let p = &mut self.proj.list[i];
            p.vel = [p.vel[0] + kb[0], p.vel[1] + kb[1], p.vel[2] + kb[2]];
            p.needs_sync = true;
            p.sync_velocity |= hurt && !p.kind.hurting() && p.kind != PKind::EyeOfEnder;
            if matches!(p.kind, PKind::LargeFireball | PKind::WindCharge | PKind::BreezeWindCharge) {
                p.owner = uuid;
            }
        }
        if let Some(h) = &self.proj.hook
            && let Some(kb) = push(self, h.p.pos, "minecraft:fishing_bobber", &h.p.bbox())
            && let Some(h) = self.proj.hook.as_mut()
        {
            h.p.vel = [h.p.vel[0] + kb[0], h.p.vel[1] + kb[1], h.p.vel[2] + kb[2]];
            h.p.needs_sync = true;
            h.p.sync_velocity |= hurt;
        }
        // Llama spits (no tracking of their own: the pushed velocity goes out at once, and so
        // does the velocity of a hurt one, pushed or not).
        for i in 0..self.mobs.mounts.spits.len() {
            let s = &self.mobs.mounts.spits[i];
            if s.removed {
                continue;
            }
            let Some(kb) = push(self, s.pos, "minecraft:llama_spit", &super::mounts::llama::spit_box(s.pos)) else { continue };
            let s = &mut self.mobs.mounts.spits[i];
            s.vel = [s.vel[0] + kb[0], s.vel[1] + kb[1], s.vel[2] + kb[2]];
            let (id, velocity) = (s.id, crate::mobs::track::lp_quantize(s.vel));
            if hurt || kb.iter().any(|&v| v != 0.0) {
                let _ = self.send(rc_client::Event::Entity(rc_client::EntityEvent::Motion { id, velocity }));
            }
        }
    }
}
