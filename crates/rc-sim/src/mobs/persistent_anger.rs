//! Persistent anger of the neutral mobs that keep it as a reference (iron golem, wolf): the
//! anger target is remembered by UUID, saved as is, found again among the loaded entities, and
//! updated after each AI step while the mob keeps its target.

use rc_proto::nbt::Nbt;

use super::mob::{Ent, Mob};
use crate::nbtio;
use crate::server::Server;

/// Anger end (`anger_end_time`, -1 when absent) and target reference (`angry_at`, absent when
/// there is none) read back. The reference keeps the real UUID (player or mob): its entity is
/// found again at the first tick (`relink`), never assumed to be the player.
pub fn load(m: &mut Mob, e: &Nbt) {
    m.anger_end = e.get("anger_end_time").and_then(super::mob::f64_of).map_or(-1, |t| t as i64);
    m.anger_uuid = nbtio::uuid(e.get("angry_at"));
    m.anger_target = None;
    m.anger_relink = m.anger_uuid.is_some();
}

/// The anger end is always written (-1 included); the reference only when there is one, even if
/// its entity is not loaded (it is erased with the anger).
pub fn save(m: &Mob, e: &mut Nbt) {
    nbtio::set(e, "anger_end_time", Nbt::Long(m.anger_end));
    match m.anger_uuid {
        Some(u) => nbtio::set(e, "angry_at", nbtio::uuid_nbt(u)),
        None => nbtio::remove(e, "angry_at"),
    }
}

/// Anger reference read back: attached to the present entity carrying that UUID, player or mob.
/// At the first tick after reloading, this entity also becomes the target. As long as no entity
/// matches, the mob is angry at nobody present.
pub fn relink(s: &Server, m: &mut Mob) {
    let first = std::mem::take(&mut m.anger_relink);
    let Some(u) = m.anger_uuid else { return };
    if m.anger_target.is_some() {
        return;
    }
    let found =
        if u == s.uuid { Some(Ent::Player) } else { s.mobs.find_uuid_where(u, |o| !o.removed).map(|o| Ent::Mob(o.id)) };
    m.anger_target = found;
    if first && found.is_some() {
        m.target = found;
    }
}

/// A player the anger may stay on: neither creative nor spectator, outside Peaceful.
fn valid_player(s: &Server, t: Ent) -> bool {
    t == Ent::Player && s.game_mode != 1 && s.game_mode != 3 && s.level.difficulty_id() != 0
}

/// The update after the AI step, for a mob that stays angry while it has a target: a dead mob
/// target that was the anger target ends it; the current target becomes the anger target and
/// restarts the 20 to 39 s timer (mob's random); an expired anger without a valid player target
/// ends; anger at a player who cannot be attacked ends.
pub fn update(s: &Server, m: &mut Mob) {
    let previous = m.target;
    if let Some(Ent::Mob(id)) = previous
        && s.mob_ref(id).is_some_and(|o| o.dead || o.health <= 0.0)
        && m.anger_target == previous
    {
        m.stop_being_angry();
        return;
    }
    // The target as the game's getter hands it: none for a player who cannot be attacked.
    let target = m.target.filter(|&t| s.ent_alive(t) && (t != Ent::Player || valid_player(s, t)));
    if let Some(t) = target {
        if m.anger_target != Some(t) {
            m.anger_target = Some(t);
            m.anger_uuid = match t {
                Ent::Player => Some(s.uuid),
                Ent::Mob(id) => s.mob_ref(id).map(|o| o.uuid),
            };
        }
        m.anger_end = s.time + i64::from(m.rng.int_between(400, 780));
    }
    // Reference present (even if its entity is not loaded) and anger expired: end.
    if m.anger_uuid.is_some() && !m.is_angry(s.time) && target.is_none_or(|t| !valid_player(s, t)) {
        m.stop_being_angry();
    }
    if m.anger_target == Some(Ent::Player) && !valid_player(s, Ent::Player) {
        m.stop_being_angry();
    }
}
