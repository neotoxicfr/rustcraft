//! Fireballs of mobs (F12): small blaze fireball (damage
//! 5, sets fire for 5 s, places fire on the block hit) and large ghast fireball (damage 6,
//! explosion of power 1 with fire if the `mob_griefing` rule allows it, can be sent back by a player
//! hit). Flight, impacts, network tracking and saving are those of the common projectile
//! engine (`projectiles/`, kinds `SmallFireball` and `LargeFireball`); this module only
//! keeps the shot and the return by the player.

use super::mob::Ent;
use super::mth::dist2;
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

fn normalize(v: [f64; 3]) -> [f64; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l < 1.0e-5 { [0.0; 3] } else { v.map(|c| c / l) }
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

    /// Projectiles of the tag `redirectable_projectile` in an explosion: pushed like
    /// entities, then the author of the explosion's damage becomes their
    /// owner (none if the explosion is anonymous).
    pub(crate) fn explosion_redirect(&mut self, center: [f64; 3], radius: f32, uuid: Option<u128>, kb_mult: f64) {
        let double = f64::from(radius * 2.0);
        for i in 0..self.proj.list.len() {
            let p = &self.proj.list[i];
            if p.removed || !matches!(p.kind, PKind::LargeFireball | PKind::WindCharge | PKind::BreezeWindCharge) {
                continue;
            }
            let dist = dist2(p.pos, center).sqrt() / double;
            if dist > 1.0 {
                continue;
            }
            let bb = p.bbox();
            let eye = if p.kind == PKind::LargeFireball { f64::from(0.85f32) } else { 0.0 };
            let mut dir = [p.pos[0] - center[0], p.pos[1] + eye - center[1], p.pos[2] - center[2]];
            let l = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
            dir = if l < 1.0e-4 { [0.0; 3] } else { dir.map(|v| v / l) };
            let exposure = if kb_mult == 0.0 { 0.0 } else { f64::from(self.seen_percent(center, &bb)) };
            let kp = (1.0 - dist) * exposure * kb_mult;
            let p = &mut self.proj.list[i];
            for (v, d) in p.vel.iter_mut().zip(dir) {
                *v += d * kp;
            }
            p.needs_sync = true;
            p.owner = uuid;
        }
    }
}
