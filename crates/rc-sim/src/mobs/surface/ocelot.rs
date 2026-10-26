//! Ocelot (§ 4.3): wary (flees from players, scareable tempting), trust gained through
//! fish, hunts chickens and baby turtles, crouching posture and sprint.

use super::super::goals::{Goal, Who};
use super::super::mob::{Mob, MoveOp};
use super::super::mth::dist2;
use crate::server::Server;

pub fn register(m: &mut Mob) {
    let g = &mut m.goals;
    g.add(1, Goal::Float);
    g.add(3, super::tempt(0.6, "minecraft:ocelot_food", true));
    g.add(7, Goal::Leap { yd: 0.3, target: None });
    g.add(8, Goal::OcelotAttack { target: None, time: 0 });
    g.add(9, super::breed_goal(0.8));
    g.add(10, Goal::Stroll { speed: 0.8, interval: 120, prob: Some(1.000_000_1e-5), wanted: [0.0; 3] });
    g.add(11, super::look_player(10.0));
    m.targets.add(1, super::nearest(Who::Chicken, false, 10));
    m.targets.add(1, super::nearest(Who::BabyTurtleOnLand, false, 10));
    reassess(m);
}

/// Flee from players, present as long as it does not trust (added at the end of the list).
pub fn reassess(m: &mut Mob) {
    let idx = m.goals.goals.iter().position(|w| matches!(w.goal, Goal::Avoid { who: Who::Player, .. }));
    if let Some(i) = idx {
        if m.goals.goals[i].running {
            m.goals.goals[i].running = false;
        }
        m.goals.goals.remove(i);
        m.goals.reset_locks();
    }
    if !m.surf.trusting {
        m.goals.add(4, super::avoid(Who::Player, 16.0, 0.8, 1.33));
    }
}

/// Posture and sprint according to the requested speed.
pub fn custom_ai_step(m: &mut Mob) {
    let (crouch, sprint) = if m.move_ctl.op == MoveOp::MoveTo {
        let sp = m.move_ctl.speed_mod;
        if sp == 0.6 {
            (true, false)
        } else if sp == 1.33 {
            (false, true)
        } else {
            (false, false)
        }
    } else {
        (false, false)
    };
    m.surf.crouching = crouch;
    set_sprinting(m, sprint);
}

/// Shared flag and speed modifier (+30% total).
pub fn set_sprinting(m: &mut Mob, v: bool) {
    m.surf.sprinting = v;
    m.attrs.speed.remove_mod("minecraft:sprinting");
    if v {
        m.attrs.speed.set_mod("minecraft:sprinting", f64::from(0.3f32), 2);
    }
}

pub fn interact(s: &mut Server, m: &mut Mob, slot: usize, item: &str) -> Option<bool> {
    let tempting = m.goals.goals.iter().any(|w| w.running && matches!(w.goal, Goal::Tempt { .. }));
    if tempting && !m.surf.trusting && s.item_tag(item, "minecraft:ocelot_food") && dist2(s.pos, m.pos) < 9.0 {
        if s.game_mode != 1
            && let Some(st) = &mut s.slots[slot]
        {
            st.count -= 1;
            if st.count <= 0 {
                s.slots[slot] = None;
            }
        }
        if m.rng.next_int_bound(3) == 0 {
            m.surf.trusting = true;
            reassess(m);
            // Heart particles (client): draws server-side too.
            trust_particles(m);
            s.mob_event(m, 41);
        } else {
            trust_particles(m);
            s.mob_event(m, 40);
        }
        return Some(true);
    }
    None
}

/// The trust particles server-side: 7 × (3 gaussians + 3 random positions).
fn trust_particles(m: &mut Mob) {
    for _ in 0..7 {
        let _ = (m.rng.next_gaussian(), m.rng.next_gaussian(), m.rng.next_gaussian());
        let _ = (m.rng.next_double(), m.rng.next_double(), m.rng.next_double());
    }
}
