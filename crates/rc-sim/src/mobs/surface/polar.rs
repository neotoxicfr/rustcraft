//! Polar bear (§ 4.3): neutral (anger 20 to 39 s), rears up before striking, defends its
//! cubs, different panic for the cub.

use super::super::goals::surface_goals::SGoal;
use super::super::goals::{Goal, Melee, TBase, Who};
use super::super::mob::{Ent, Mob};
use crate::server::Server;

pub fn register(m: &mut Mob) {
    let g = &mut m.goals;
    g.add(0, Goal::Float);
    g.add(1, super::melee(1.25, true, Melee::PolarBear));
    g.add(1, super::panic(2.0, "minecraft:panic_causes"));
    g.add(4, super::follow_parent(1.25));
    g.add(5, Goal::Stroll { speed: 1.0, interval: 120, prob: None, wanted: [0.0; 3] });
    g.add(6, super::look_player(6.0));
    g.add(7, super::look_around());
    let t = &mut m.targets;
    t.add(1, Goal::Surf(SGoal::PolarHurtBy { base: TBase::new(true, false), stamp: 0 }));
    t.add(2, Goal::Surf(SGoal::PolarAttackPlayers { base: TBase::new(true, true), target: None }));
    t.add(
        3,
        Goal::Nearest {
            base: TBase::new(true, false),
            who: Who::Player,
            interval: super::super::mth::reduced(10),
            target: None,
            angry_only: true,
            non_tame: false,
        },
    );
    // Fox (not simulated in singleplayer): targeting goal with no possible prey.
    t.add(4, super::nearest(Who::Fox, true, 10));
    t.add(5, Goal::ResetAnger { stamp: 0, alert: false });
}

/// Warning sound (at least 40 ticks between two).
pub fn warning(s: &mut Server, m: &mut Mob) {
    if m.surf.warning <= 0 {
        s.make_sound(m, "minecraft:entity.polar_bear.warning");
        m.surf.warning = 40;
    }
}

/// End of `tick`: warning sound delay, persistent anger.
pub fn post_tick(s: &mut Server, m: &mut Mob) {
    if m.removed {
        return;
    }
    if m.surf.warning > 0 {
        m.surf.warning -= 1;
    }
    update_anger(s, m);
}

/// The persistent anger update (the persistent target is the player or a mob).
fn update_anger(s: &mut Server, m: &mut Mob) {
    let prev = m.target;
    if let Some(Ent::Mob(id)) = prev
        && !s.ent_alive(Ent::Mob(id))
        && m.anger_target == prev
    {
        m.stop_being_angry();
        return;
    }
    let target = m.target.filter(|t| s.ent_alive(*t));
    if let Some(t) = target {
        let new_target = m.anger_target != Some(t);
        if new_target {
            m.anger_target = Some(t);
            m.surf.angry_at = match t {
                Ent::Player => Some(s.uuid),
                Ent::Mob(id) => s.mob_ref(id).map(|o| o.uuid),
            };
        }
        // Always ("stays angry if the target is there").
        let ticks = m.rng.int_between(400, 780);
        m.anger_end = s.time + i64::from(ticks);
    }
    let angry = m.anger_end > 0 && m.anger_end - s.time > 0;
    let valid_player =
        |s: &Server, t: Ent| t == Ent::Player && s.game_mode != 1 && s.game_mode != 3 && s.level.difficulty_id() != 0;
    if m.anger_target.is_some() && !angry && target.is_none_or(|t| !valid_player(s, t)) {
        m.stop_being_angry();
    }
    if m.anger_target == Some(Ent::Player) && (s.game_mode == 1 || s.game_mode == 3 || s.level.difficulty_id() == 0) {
        m.stop_being_angry();
    }
}

