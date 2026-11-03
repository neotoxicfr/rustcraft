//! G5-F04: smash attack of the mace. An attacker falling more than 1.5 blocks without gliding
//! deals `mace_smash` damage with a bonus by fallen block (4 per block up to 3 blocks, 2 up to
//! 8, then 1, plus the weapon's damage per fallen block: Density); on a landed hit its fall
//! stops (vertical speed 0.01, fall grace at the impact point), a sound plays (ground, heavy
//! ground beyond 5 blocks, air), the living entities within 3.5 blocks of the target are pushed
//! away (level event 2013) and, for a player, the fall distance is reset after the hit.

use rc_client::{EntityEvent, Event};
use rc_physics::Aabb;
use rc_proto::nbt::Nbt;

use crate::mobs::mob::{Ent, Mob};
use crate::player::Stack;
use crate::progress::pred::Ctx;
use crate::randsrc::Rand;
use crate::server::Server;

pub(crate) const MACE: &str = "minecraft:mace";

/// The attacker can smash: fall above 1.5 blocks, not gliding.
pub(crate) fn can_smash(fall: f64, gliding: bool) -> bool {
    fall > 1.5 && !gliding
}

/// Damage by fall of a smash, before the enchantments.
pub(crate) fn smash_base(fall: f64) -> f64 {
    if fall <= 3.0 {
        4.0 * fall
    } else if fall <= 8.0 {
        12.0 + 2.0 * (fall - 3.0)
    } else {
        22.0 + fall - 8.0
    }
}

/// The struck target, as the smash knockback sees it.
pub(crate) struct Struck {
    pub ent: Ent,
    pub uuid: u128,
    pub pos: [f64; 3],
    pub bbox: Aabb,
    pub on_ground: bool,
    /// Block under its feet (supporting block, otherwise the cell just below): place of the
    /// level event.
    pub on_pos: [i32; 3],
}

impl Struck {
    pub(crate) fn mob(s: &Server, m: &Mob) -> Struck {
        Struck {
            ent: Ent::Mob(m.id),
            uuid: m.uuid,
            pos: m.pos,
            bbox: m.bbox(),
            on_ground: m.on_ground,
            on_pos: s.on_pos(m.pos, m.support, 1.0e-5),
        }
    }
}

impl Server {
    /// Damage type of a weapon hit: the weapon's `damage_type` component, otherwise `mace_smash`
    /// for a mace that smashes, otherwise `default`.
    pub(crate) fn weapon_damage_type(&self, w: Option<&Stack>, smash: bool, default: &str) -> String {
        if let Some(w) = w {
            if let Some(t) = crate::items::component(w, "minecraft:damage_type").and_then(Nbt::as_str) {
                return crate::commands::args::id(t);
            }
            if crate::items::component(w, "!minecraft:damage_type").is_none()
                && let Some(t) = self.life.items.default_component(w, "minecraft:damage_type").and_then(|v| v.as_str().map(str::to_owned))
            {
                return crate::commands::args::id(&t);
            }
            if smash && w.id == MACE {
                return "minecraft:mace_smash".into();
            }
        }
        default.to_owned()
    }

    /// Attack damage bonus of the weapon: for a mace that smashes, the damage by fall plus the
    /// damage per fallen block of its enchantments (`own`: the victim's random) × the fall.
    pub(crate) fn mace_bonus(&self, w: Option<&Stack>, fall: f64, smash: bool, ctx: &Ctx<'_>, own: &mut dyn Rand) -> f32 {
        let Some(w) = w.filter(|w| w.id == MACE && smash) else { return 0.0 };
        let mut world = self.mobs.rng;
        let per = self.ench_weapon_value(w, "minecraft:smash_damage_per_fallen_block", 0.0, ctx, &mut world, own);
        (smash_base(fall) + f64::from(per) * fall) as f32
    }

    /// Landed smash of the player: fall stopped, sound, knockback around the target.
    pub(crate) fn player_smash(&mut self, t: &Struck) {
        let fall = self.life.vit.fall;
        let v = self.mobs.player_vel;
        self.mobs.player_vel = [v[0], f64::from(0.01f32), v[2]];
        // Impact point: that of the impulse in progress if it is not above the player.
        let impact = match self.life.vit.impulse_impact {
            Some(p) if p[1] <= self.pos[1] => p,
            _ => self.pos,
        };
        self.life.vit.impulse_from_explosion(true, impact);
        let nv = self.mobs.player_vel;
        let _ = self.send(Event::Entity(EntityEvent::Motion {
            id: self.player_id,
            velocity: crate::mobs::track::lp_quantize(nv),
        }));
        let sound = if t.on_ground {
            self.life.vit.extra_fall_particles = true;
            if fall > 5.0 { "minecraft:item.mace.smash_ground_heavy" } else { "minecraft:item.mace.smash_ground" }
        } else {
            "minecraft:item.mace.smash_air"
        };
        self.world_sound(sound, 7, self.pos, 1.0, 1.0);
        let name = self.name.clone();
        self.smash_knockback(Ent::Player, &name, fall, t);
    }

    /// Landed smash of a mob (its fall is never reset by the hit, like the game): vertical speed
    /// 0.01, sound, knockback around the target.
    pub(crate) fn mob_smash(&mut self, m: &mut Mob, t: &Struck) {
        let fall = m.fall_distance;
        m.vel[1] = f64::from(0.01f32);
        m.needs_sync = true;
        let sound = match (t.on_ground, fall > 5.0) {
            (true, true) => "minecraft:item.mace.smash_ground_heavy",
            (true, false) => "minecraft:item.mace.smash_ground",
            (false, _) => "minecraft:item.mace.smash_air",
        };
        let cat = Self::mob_sound_source(m);
        self.world_sound(sound, cat, m.pos, 1.0, 1.0);
        let name = crate::nbtio::uuid_text(m.uuid);
        self.ghost_push(m);
        self.smash_knockback(Ent::Mob(m.id), &name, fall, t);
        self.ghost_pop();
    }

    /// Same team (scoreboard), by score holder name.
    pub(crate) fn same_team(&self, a: &str, b: &str) -> bool {
        let s = &self.cmd.scores;
        matches!((s.team_of(a), s.team_of(b)), (Some(x), Some(y)) if x.name == y.name)
    }

    /// Living entities within 3.5 blocks of the target (box grown by 3.5, then distance between
    /// positions), except the attacker, the target, the attacker's allies, the animals tamed by
    /// the target (the game tests the target, not the attacker), armor stands that are markers
    /// and a player flying in creative mode or spectating: pushed by `(3.5 − distance) × 0.7`,
    /// doubled beyond 5 blocks of fall, reduced by their knockback resistance, along the
    /// direction from the target, 0.7 upwards. Mobs, armor stands, mannequins and players
    /// (the current one and the other players of the local network) alike.
    fn smash_knockback(&mut self, attacker: Ent, attacker_name: &str, fall: f64, t: &Struck) {
        self.level_event(2013, t.on_pos, 750);
        let b = &t.bbox;
        let area = Aabb {
            min: [b.min[0] - 3.5, b.min[1] - 3.5, b.min[2] - 3.5],
            max: [b.max[0] + 3.5, b.max[1] + 3.5, b.max[2] + 3.5],
        };
        let mult = if fall > 5.0 { 2.0 } else { 1.0 };
        let power = |d: [f64; 3], res: f64| smash_power(d, res, mult);
        let near = |pos: [f64; 3]| crate::mobs::mth::dist2(t.pos, pos) <= 3.5 * 3.5;
        let attacker_mob = match attacker {
            Ent::Mob(id) => self.mob_ref(id).cloned(),
            Ent::Player => None,
        };
        // `same_team` with each entity, the attacker's team looked up once and the entities'
        // UUIDs never formatted (team found by UUID).
        let attacker_team = self.cmd.scores.team_of(attacker_name).map(|t| t.name.clone());
        let ally = |s: &Self, uuid: u128| {
            attacker_team.as_ref().is_some_and(|own| s.cmd.scores.team_of_uuid(uuid).is_some_and(|t| t.name == *own))
        };
        let ids: Vec<i32> = self
            .mobs
            .list
            .iter()
            .flatten()
            .filter(|o| Ent::Mob(o.id) != attacker && Ent::Mob(o.id) != t.ent && o.touches(&area) && near(o.pos))
            .filter(|o| !(o.tame && o.owner == Some(t.uuid) && crate::mobs::species::tamable(o.kind)))
            .filter(|o| {
                !ally(self, o.uuid) && !attacker_mob.as_ref().is_some_and(|a| crate::mobs::illagers::allied(self, a, o))
            })
            .map(|o| o.id)
            .collect();
        for id in ids {
            let Some(o) = self.mobs.get_mut(id) else { continue };
            let d = [o.pos[0] - t.pos[0], o.pos[1] - t.pos[1], o.pos[2] - t.pos[2]];
            let (p, kb) = power(d, o.attrs.knockback_resistance.value());
            if p > 0.0 {
                o.vel = [o.vel[0] + kb[0], o.vel[1] + f64::from(0.7f32), o.vel[2] + kb[1]];
                o.needs_sync = true;
            }
        }
        // Armor stands (except markers) and mannequins, kept as compounds; their knockback
        // resistance comes from their attributes and worn equipment.
        for d in self.deco_living() {
            if d.marker || Ent::Mob(d.id) == attacker || Ent::Mob(d.id) == t.ent || !near(d.pos) || !d.bbox.intersects(&area)
            {
                continue;
            }
            if ally(self, d.uuid) {
                continue;
            }
            let res = self.deco_attribute(d.id, "minecraft:knockback_resistance").unwrap_or(0.0);
            let (p, kb) = power([d.pos[0] - t.pos[0], d.pos[1] - t.pos[1], d.pos[2] - t.pos[2]], res);
            if p > 0.0 {
                self.deco_push(d.id, [kb[0], f64::from(0.7f32), kb[1]]);
            }
        }
        // The current player as a bystander (a mob smashes), then the other players.
        if attacker != Ent::Player && t.ent != Ent::Player {
            self.smash_push_player(attacker_name, t.pos, &area, mult);
        }
        for o in self.lan_peers() {
            if Ent::Mob(o.id) == t.ent || o.game_mode == 3 || !near(o.pos) || !o.bbox.intersects(&area) {
                continue;
            }
            let name = attacker_name.to_owned();
            self.as_player(o.who, |sv| sv.smash_push_player(&name, t.pos, &area, mult));
        }
    }

    /// The smash knockback on the installed player as a bystander: alive, neither spectating nor
    /// flying in creative mode, in range, not allied with the attacker; its velocity is sent to it.
    fn smash_push_player(&mut self, attacker_name: &str, from: [f64; 3], area: &Aabb, mult: f64) {
        if self.game_mode == 3
            || (self.game_mode == 1 && self.flying)
            || !self.ent_alive(Ent::Player)
            || crate::mobs::mth::dist2(from, self.pos) > 3.5 * 3.5
            || !self.player_aabb().intersects(area)
            || self.same_team(attacker_name, &self.name.clone())
        {
            return;
        }
        let d = [self.pos[0] - from[0], self.pos[1] - from[1], self.pos[2] - from[2]];
        let res = self.player_attribute("minecraft:knockback_resistance");
        let (p, kb) = smash_power(d, res, mult);
        if p > 0.0 {
            self.player_add_velocity([kb[0], f64::from(0.7f32), kb[1]]);
        }
    }
}

/// Smash knockback of a bystander at offset `d` from the target: power `(3.5 − distance) × 0.7`
/// × `mult` × (1 − resistance) and its horizontal push along the normalized offset.
fn smash_power(d: [f64; 3], res: f64, mult: f64) -> (f64, [f64; 2]) {
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let p = (3.5 - len) * f64::from(0.7f32) * mult * (1.0 - res);
    let n = if len < f64::from(1.0e-5f32) { [0.0; 3] } else { d.map(|c| c / len) };
    (p, [n[0] * p, n[2] * p])
}
