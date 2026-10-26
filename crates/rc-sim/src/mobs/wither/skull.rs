//! Wither skulls (`WitherSkull`, "hurting" projectile): flight and tracking by the common
//! projectile engine (kind `WitherSkull`); here the shot, the hit (8 with an author, heal of 5
//! if it kills; magic 5 otherwise; wither effect depending on difficulty) and the power-1 explosion
//! (blue skull: destructible blocks of resistance 0.8 at most).

use rc_proto_v777::entity::Meta;

use super::super::mob::Ent;
use crate::player::Stack;
use crate::projectiles::{PKind, Proj};
use crate::server::Server;

fn normalize(v: [f64; 3]) -> [f64; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l < 1.0e-5 { [0.0; 3] } else { v.map(|c| c / l) }
}

impl Server {
    /// Throws a skull (`new WitherSkull(level, owner, direction)` then the placement): velocity =
    /// normalized direction × 0.1, rotation of the author.
    pub(crate) fn shoot_wither_skull(
        &mut self,
        owner: u128,
        pos: [f64; 3],
        dir: [f64; 3],
        rot: (f32, f32),
        dangerous: bool,
    ) {
        let id = self.proj_new_id();
        let item = Stack { id: PKind::WitherSkull.default_item().into(), count: 1, components: None };
        let mut f = Proj::new(PKind::WitherSkull, id, pos, item);
        f.owner = Some(owner);
        f.yaw = rot.0;
        f.pitch = rot.1;
        f.vel = normalize(normalize(dir)).map(|c| c * f.accel);
        if dangerous {
            f.dangerous = true;
            f.set_meta(8, Meta::Bool(true));
        }
        self.proj_add(f);
    }

    /// The impact of a wither head against an entity.
    pub(crate) fn wither_skull_hit_entity(&mut self, t: Ent, owner: Option<Ent>, kb: (f64, f64)) {
        let hurt = match owner {
            Some(o) => {
                let hurt = self.proj_damage(t, "minecraft:wither_skull", 8.0, Some(o), kb);
                // Victim killed: the author regains 5 HP.
                if hurt && !self.ent_alive(t) {
                    self.heal_ent(o, 5.0);
                }
                hurt
            }
            None => self.proj_damage(t, "minecraft:magic", 5.0, None, kb),
        };
        if !hurt {
            return;
        }
        let secs = match self.level.difficulty_id() {
            2 => 10,
            3 => 40,
            _ => 0,
        };
        if secs > 0 {
            match t {
                Ent::Player => {
                    self.give_player_effect("minecraft:wither", 20 * secs, 1);
                }
                Ent::Mob(id) => {
                    if let Some(i) = self.mob_index(id)
                        && let Some(mut o) = self.mobs.list[i].take()
                    {
                        self.give_mob_effect(&mut o, "minecraft:wither", 20 * secs, 1);
                        self.mobs.list[i] = Some(o);
                    }
                }
            }
        }
    }

    /// Power-1 explosion ("mob" interaction), removal.
    pub(crate) fn wither_skull_explode(&mut self, p: &mut Proj) {
        let by = self.proj_owner(p);
        let ty = if by.is_some() { "minecraft:player_explosion" } else { "minecraft:explosion" };
        self.mobs.wither.dangerous_blast = p.dangerous;
        let src = crate::mobs::recent::vibration::Src::Projectile(by);
        self.game_event_src("minecraft:explode", p.pos, Some(src), None);
        self.prog.expl_cause_next = Some("minecraft:wither_skull"); // W8c
        self.carts.blast_direct = Some(false); // G5-F06: the skull does not burn
        self.deco.blast_wet = p.in_water; // direct source: the skull
        self.explode_core(None, by, None, p.pos, 1.0, false, ty);
        self.prog.expl_cause_next = None;
        self.mobs.wither.dangerous_blast = false;
        p.removed = true;
    }

    /// The healing of a mob from a living entity.
    fn heal_ent(&mut self, e: Ent, amount: f32) {
        match e {
            Ent::Player => self.heal(amount),
            Ent::Mob(id) => {
                if let Some(o) = self.mobs.get_mut(id)
                    && o.health > 0.0
                {
                    o.health = (o.health + amount).clamp(0.0, o.max_health());
                }
            }
        }
    }
}
