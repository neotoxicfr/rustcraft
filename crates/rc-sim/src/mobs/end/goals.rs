//! Shulker goals (F15), dispatched by `Goal::End`: look at the player in the plane (probability
//! 0.02), attack (opening, bullets), half-opening at rest.

use super::super::goals::{Goal, LOOK, MOVE};
use super::super::mob::{Ent, Mob};
use super::super::mth::{dist2, reduced};
use crate::server::Server;

#[derive(Debug, Clone)]
pub enum EGoal {
    /// The look-at-player objective.
    LookAtPlayer { at: Option<Ent>, time: i32 },
    /// Shulker attack goal (movement, gaze; every tick).
    Attack { time: i32 },
    /// "Peek out of the shell" goal of the shulker (no flag).
    Peek { time: i32 },
}

impl EGoal {
    pub fn wrap(self) -> Goal {
        Goal::End(self)
    }

    pub fn flags(&self) -> u8 {
        match self {
            EGoal::LookAtPlayer { .. } => LOOK,
            EGoal::Attack { .. } => MOVE | LOOK,
            EGoal::Peek { .. } => 0,
        }
    }

    pub fn every_tick(&self) -> bool {
        matches!(self, EGoal::Attack { .. })
    }
}

impl Server {
    pub(crate) fn end_can_use(&mut self, g: &mut EGoal, m: &mut Mob) -> bool {
        match g {
            EGoal::LookAtPlayer { at, .. } => {
                if m.rng.next_float() >= 0.02 {
                    return false;
                }
                // Official: the target is read then always overwritten by the closest-player search.
                if let Some(t) = m.target {
                    *at = Some(t);
                }
                *at = (!self.carries_player(m) && self.targeting_ok(m, Ent::Player, false, 8.0, true, true))
                    .then_some(Ent::Player);
                at.is_some()
            }
            EGoal::Attack { .. } => m.target.is_some_and(|t| self.ent_alive(t)) && self.level.difficulty_id() != 0,
            EGoal::Peek { .. } => {
                m.target.is_none()
                    && m.rng.next_int_bound(reduced(40)) == 0
                    && self.shulker_can_stay(m, m.block_pos(), m.end.attach)
            }
        }
    }

    pub(crate) fn end_can_continue(&mut self, g: &mut EGoal, m: &mut Mob) -> bool {
        match g {
            EGoal::LookAtPlayer { at, time } => {
                let Some(e) = *at else { return false };
                self.ent_alive(e) && self.ent_pos(e).is_some_and(|p| dist2(m.pos, p) <= 64.0) && *time > 0
            }
            EGoal::Peek { time } => m.target.is_none() && *time > 0,
            EGoal::Attack { .. } => self.end_can_use(g, m),
        }
    }

    pub(crate) fn end_start(&mut self, g: &mut EGoal, m: &mut Mob) {
        match g {
            EGoal::LookAtPlayer { time, .. } => *time = reduced(40 + m.rng.next_int_bound(40)),
            EGoal::Attack { time } => {
                *time = 20;
                self.shulker_set_peek(m, 100);
            }
            EGoal::Peek { time } => {
                *time = reduced(20 * (1 + m.rng.next_int_bound(3)));
                self.shulker_set_peek(m, 30);
            }
        }
    }

    pub(crate) fn end_stop(&mut self, g: &mut EGoal, m: &mut Mob) {
        match g {
            EGoal::LookAtPlayer { at, .. } => *at = None,
            EGoal::Attack { .. } => self.shulker_set_peek(m, 0),
            EGoal::Peek { .. } => {
                if m.target.is_none() {
                    self.shulker_set_peek(m, 0);
                }
            }
        }
    }

    pub(crate) fn end_tick(&mut self, g: &mut EGoal, m: &mut Mob) {
        match g {
            EGoal::LookAtPlayer { at, time } => {
                let Some(e) = *at else { return };
                if self.ent_alive(e)
                    && let Some(p) = self.ent_pos(e)
                {
                    // In the plane: eye height of the shulker.
                    let y = m.eye_y();
                    self.look_at(m, [p[0], y, p[2]], 10.0, 180.0);
                    *time -= 1;
                }
            }
            EGoal::Attack { time } => self.shulker_attack_tick(m, time),
            EGoal::Peek { time } => *time -= 1,
        }
    }
}
