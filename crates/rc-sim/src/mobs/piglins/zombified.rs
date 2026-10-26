//! Zombified piglin (`ZombifiedPiglin`): neutral zombie (persistent anger 20 to 39 s, recorded
//! anger target), call to fellows every 4 to 6 s, deferred anger sound, speed
//! +0.05 when angry. Its goals are set by `brains.rs` (zombie base, without attacking villagers).

use rc_proto::nbt::Nbt;

use super::super::mob::{Ent, Mob};
use super::super::species::Kind;
use crate::nbtio;
use crate::server::Server;

pub fn load(m: &mut Mob, e: &Nbt) {
    // End recorded, otherwise old `AngerTime`, otherwise −1; the
    // target read back also becomes the current target (resolved at the first tick).
    m.anger_end = match e.get("anger_end_time").and_then(super::super::mob::f64_of) {
        Some(t) => t as i64,
        None => -1,
    };
    m.pig.had_target = false;
    if let Some(u) = nbtio::uuid(e.get("angry_at")) {
        m.pig.anger_uuid = Some(u);
    }
}

pub fn save(m: &Mob, e: &mut Nbt) {
    nbtio::set(e, "IsBaby", Nbt::Byte(i8::from(m.baby)));
    nbtio::set(e, "anger_end_time", Nbt::Long(m.anger_end));
    let uuid = m.pig.anger_uuid;
    match uuid {
        Some(u) => nbtio::set(e, "angry_at", nbtio::uuid_nbt(u)),
        None => nbtio::remove(e, "angry_at"),
    }
}

/// Anger end in the future.
pub fn is_angry(s: &Server, m: &Mob) -> bool {
    m.anger_end > 0 && m.anger_end - s.time > 0
}

/// On going from "no target" to a target, anger sound in 0 to
/// 1 s and next call in 4 to 6 s (entity random). Called before each target set.
pub fn before_set_target(m: &mut Mob, new: Option<Ent>) {
    if m.kind != Kind::ZombifiedPiglin || m.target.is_some() || new.is_none() {
        return;
    }
    m.pig.first_anger_sound_in = m.rng.int_between(0, 20);
    m.pig.ticks_until_alert = m.rng.int_between(80, 120);
}

/// Player target valid for anger.
fn valid_player(s: &Server, t: Ent) -> bool {
    t == Ent::Player && s.game_mode != 1 && s.game_mode != 3 && s.level.difficulty_id() != 0
}

/// 20 to 39 s (entity random).
fn start_timer(s: &Server, m: &mut Mob) {
    m.anger_end = s.time + i64::from(m.rng.int_between(400, 780));
}

/// The species' own server AI step of the zombified piglin.
pub fn custom_ai_step(s: &mut Server, m: &mut Mob) {
    // Target read back from a save: found again at the first tick.
    if m.anger_target.is_none()
        && let Some(u) = m.pig.anger_uuid
        && let Some(e) = s.pig_ent_of(u)
    {
        m.anger_target = Some(e);
        if m.target.is_none() {
            m.target = Some(e);
        }
    }
    if is_angry(s, m) {
        if !m.baby {
            m.attrs.speed.set_mod("minecraft:attacking", 0.05, 0);
        }
        if m.pig.first_anger_sound_in > 0 {
            m.pig.first_anger_sound_in -= 1;
            if m.pig.first_anger_sound_in == 0 {
                let v = m.sound_volume() * 2.0;
                let p = Server::voice_pitch(m) * 1.8;
                s.mob_sound(m, "minecraft:entity.zombified_piglin.angry", v, p);
            }
        }
    } else {
        m.attrs.speed.remove_mod("minecraft:attacking");
    }
    update_persistent_anger(s, m);
    if let Some(t) = m.target {
        if m.pig.ticks_until_alert > 0 {
            m.pig.ticks_until_alert -= 1;
        } else {
            if s.line_of_sight(m, t) {
                alert_others(s, m, t);
            }
            m.pig.ticks_until_alert = m.rng.int_between(80, 120);
        }
    }
}

/// The persistent anger update.
fn update_persistent_anger(s: &mut Server, m: &mut Mob) {
    let prev = m.target;
    let pat = m.anger_target;
    if let Some(p) = prev
        && s.pig_dead(p)
        && pat == Some(p)
        && p != Ent::Player
    {
        m.stop_being_angry();
        return;
    }
    let target = m.target;
    if let Some(t) = target {
        let new = pat != Some(t);
        if new {
            m.anger_target = Some(t);
            m.pig.anger_uuid = s.pig_uuid(t);
        }
        start_timer(s, m);
    }
    if pat.is_some() && !is_angry(s, m) && (target.is_none() || !target.is_some_and(|t| valid_player(s, t))) {
        m.stop_being_angry();
    }
    if pat == Some(Ent::Player) && (s.game_mode == 1 || s.game_mode == 3 || s.level.difficulty_id() == 0) {
        m.stop_being_angry();
    }
}

/// Zombified piglins without target in the tracking box (±10 in height).
fn alert_others(s: &mut Server, m: &Mob, t: Ent) {
    let r = m.follow_range();
    let p = m.pos;
    let area = rc_physics::Aabb {
        min: [p[0] - r, p[1] - 10.0, p[2] - r],
        max: [p[0] + 1.0 + r, p[1] + 1.0 + 10.0, p[2] + 1.0 + r],
    };
    for o in s.mobs.list.iter_mut().flatten() {
        if o.kind == Kind::ZombifiedPiglin && o.id != m.id && o.target.is_none() && o.touches(&area) {
            before_set_target(o, Some(t));
            o.target = Some(t);
        }
    }
}

/// `equipment` of the zombified piglin (own, world random):
/// gold spear 1 time in 20, otherwise gold sword.
pub fn default_equipment(s: &mut Server, m: &mut Mob) {
    let roll = s.mobs.rng.next_int_bound(20);
    set_weapon(m, roll);
}

fn set_weapon(m: &mut Mob, roll: i32) {
    let w = if roll == 0 { "minecraft:golden_spear" } else { "minecraft:golden_sword" };
    m.equipment[0] = Some(crate::player::Stack { id: w.into(), count: 1, components: None });
}

impl Server {
    /// Pig struck by lightning outside peaceful: replaced by a zombified piglin
    /// (simple conversion without the equipment, pickup kept), gold weapon drawn with the pig's
    /// random, persistent. True if converted; otherwise the common lightning strike applies.
    pub(crate) fn pig_struck(&mut self, pig: &mut Mob) -> bool {
        if self.level.difficulty_id() == 0 || pig.removed {
            return false;
        }
        let mut n = self.converted(pig, Kind::ZombifiedPiglin, false, true);
        let roll = pig.rng.next_int_bound(20);
        set_weapon(&mut n, roll);
        n.persistent = true;
        pig.removed = true;
        self.add_mob(n);
        true
    }
}
