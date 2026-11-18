//! Goals of aquatic fauna (F09): random swimming, school leader following, pufferfish
//! puffing, squid movements and fleeing, dolphin goals. Dispatched by `Goal::Sea`.

use super::super::goals::{Goal, JUMP, LOOK, MOVE};
use super::super::mob::Mob;
use super::super::mth::reduced;
use crate::server::Server;

#[derive(Debug, Clone)]
pub enum AGoal {
    /// The random swim objective (fish: only outside a school).
    Swim { speed: f64, interval: i32, wanted: [f64; 3] },
    /// The school-leader following objective (no flag).
    FollowLeader { recalc: i32, next_start: i32 },
    /// Pufferfish puffing goal (no flag).
    Puff,
    /// Squid random movement goal.
    SquidMove,
    /// Squid flee goal (every tick).
    SquidFlee { ticks: i32 },
    // Dolphin.
    BreathAir,
    FindLiquid,
    Treasure { stuck: bool },
    SwimWithPlayer { on: bool },
    Jump { interval: i32, breached: bool },
    MoveToItem { cooldown: i32 },
    PlayWithItems,
    /// The objective of following a player's mount (nautilus ridden by the player; no flag).
    FollowRidden { following: bool, recalc: i32, toward: bool },
}

impl AGoal {
    pub fn wrap(self) -> Goal {
        Goal::Sea(self)
    }

    pub fn swim(speed: f64, interval: i32) -> Goal {
        AGoal::Swim { speed, interval, wanted: [0.0; 3] }.wrap()
    }

    pub fn flags(&self) -> u8 {
        match self {
            AGoal::Swim { .. } | AGoal::MoveToItem { .. } => MOVE,
            AGoal::BreathAir | AGoal::Treasure { .. } | AGoal::SwimWithPlayer { .. } => MOVE | LOOK,
            AGoal::Jump { .. } => MOVE | JUMP,
            _ => 0,
        }
    }

    pub fn every_tick(&self) -> bool {
        matches!(self, AGoal::SquidFlee { .. })
    }

    /// Breathing, treasure and jump do not give way.
    pub fn interruptable(&self) -> bool {
        !matches!(self, AGoal::BreathAir | AGoal::Treasure { .. } | AGoal::Jump { .. })
    }
}

impl Server {
    /// Default position, redrawn (10 times at most) as long as
    /// it is not swimmable.
    pub(crate) fn random_swimmable_pos(&self, m: &mut Mob, h: i32, v: i32) -> Option<[f64; 3]> {
        let mut p = self.default_random_pos(m, h, v);
        let mut count = 0;
        while let Some(q) = p {
            if self.mob_fluid(super::super::mth::block_of(q)) == super::super::env::Fluid::Water {
                break;
            }
            count += 1;
            if count > 10 {
                break;
            }
            p = self.default_random_pos(m, h, v);
        }
        p
    }

    pub(crate) fn sea_can_use(&mut self, g: &mut AGoal, m: &mut Mob) -> bool {
        match g {
            AGoal::Swim { interval, wanted, .. } => {
                // School fish following a leader: no free swimming (tested first).
                if super::is_schooling(m.kind) && super::fish::is_follower(self, m) {
                    return false;
                }
                if !m.mount.passengers.is_empty() && Self::mob_controller(m).is_some() {
                    return false;
                }
                if m.no_action_time >= 100 || m.rng.next_int_bound(reduced(*interval)) != 0 {
                    return false;
                }
                match self.random_swimmable_pos(m, 10, 7) {
                    Some(p) => {
                        *wanted = p;
                        true
                    }
                    None => false,
                }
            }
            AGoal::FollowLeader { next_start, .. } => self.follow_leader_can_use(m, next_start),
            AGoal::Puff => super::fish::puff_can_use(self, m),
            AGoal::SquidMove => true,
            AGoal::SquidFlee { .. } => super::squid::flee_can_use(self, m),
            _ => self.dolphin_can_use(g, m),
        }
    }

    pub(crate) fn sea_can_continue(&mut self, g: &mut AGoal, m: &mut Mob) -> bool {
        match g {
            AGoal::Swim { .. } => !m.nav.is_done() && Self::mob_controller(m).is_none(),
            AGoal::FollowLeader { .. } => super::fish::is_follower(self, m) && self.in_range_of_leader(m),
            AGoal::Puff => super::fish::puff_can_use(self, m),
            AGoal::SquidMove => true,
            AGoal::SquidFlee { .. } => super::squid::flee_can_use(self, m),
            _ => self.dolphin_can_continue(g, m),
        }
    }

    pub(crate) fn sea_start(&mut self, g: &mut AGoal, m: &mut Mob) {
        match g {
            AGoal::Swim { speed, wanted, .. } => {
                let (w, sp) = (*wanted, *speed);
                self.move_to_pos(m, w, sp);
            }
            AGoal::FollowLeader { recalc, .. } => *recalc = 0,
            AGoal::Puff => {
                m.sea.inflate = 1;
                m.sea.deflate = 0;
            }
            AGoal::SquidFlee { ticks } => *ticks = 0,
            AGoal::SquidMove => {}
            _ => self.dolphin_start(g, m),
        }
    }

    pub(crate) fn sea_stop(&mut self, g: &mut AGoal, m: &mut Mob) {
        match g {
            AGoal::Swim { .. } => m.nav.stop(),
            AGoal::FollowLeader { .. } => super::fish::stop_following(self, m),
            AGoal::Puff => m.sea.inflate = 0,
            AGoal::SquidMove | AGoal::SquidFlee { .. } => {}
            _ => self.dolphin_stop(g, m),
        }
    }

    pub(crate) fn sea_tick(&mut self, g: &mut AGoal, m: &mut Mob) {
        match g {
            AGoal::FollowLeader { recalc, .. } => {
                *recalc -= 1;
                if *recalc <= 0 {
                    *recalc = reduced(10);
                    self.path_to_leader(m);
                }
            }
            AGoal::SquidMove => super::squid::random_move_tick(m),
            AGoal::SquidFlee { ticks } => {
                *ticks += 1;
                super::squid::flee_tick(self, m);
            }
            AGoal::Swim { .. } | AGoal::Puff => {}
            _ => self.dolphin_tick(g, m),
        }
    }
}
