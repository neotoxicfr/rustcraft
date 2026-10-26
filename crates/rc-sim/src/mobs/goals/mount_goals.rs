//! Goals specific to mounts (F04): wild run of the untamed mounted horse, random rearing,
//! llama spitting and caravan, llama targeting (retaliation that stops after a spit,
//! wild wolves at reduced range), skeleton horse trap.

use super::super::mob::{Ent, Mob};
use super::super::mounts;
use super::{Goal, LOOK, MOVE, TARGET};
use crate::server::Server;

#[derive(Debug, Clone)]
pub enum MGoal {
    /// Mounted without being tamed, runs off at random then throws off.
    RunAround { speed: f64, pos: [f64; 3] },
    /// Spontaneous rearing (counter specific to the goal).
    RandomStand { next: i32 },
    /// The llama's ranged attack objective (spit), fixed interval 40, range 20.
    Spit { speed: f64, target: Option<Ent>, attack_time: i32, see: i32 },
    /// The llama's caravan-following objective.
    Caravan { speed: f64, dist_check: i32 },
    /// Llama retaliation: stops as soon as it has spat.
    LlamaHurtBy(Box<Goal>),
    /// Nearest untamed wolf, follow range ×0.25.
    WolfTarget(Box<Goal>),
    /// The skeleton trap objective.
    Trap,
}

impl MGoal {
    pub fn flags(&self) -> u8 {
        match self {
            MGoal::RunAround { .. } | MGoal::Caravan { .. } => MOVE,
            MGoal::Spit { .. } => MOVE | LOOK,
            MGoal::LlamaHurtBy(_) | MGoal::WolfTarget(_) => TARGET,
            MGoal::RandomStand { .. } | MGoal::Trap => 0,
        }
    }

    pub fn every_tick(&self) -> bool {
        matches!(self, MGoal::RandomStand { .. } | MGoal::Spit { .. })
    }
}

/// Follow range divided by 4 for the duration of a call (overridden).
const WOLF_RANGE: &str = "rustcraft:llama_wolf_range";

fn quarter_range(m: &mut Mob, on: bool) {
    if on {
        m.attrs.follow.set_mod(WOLF_RANGE, -0.75, 2);
    } else {
        m.attrs.follow.remove_mod(WOLF_RANGE);
    }
}

impl Server {
    pub(super) fn mount_can_use(&mut self, g: &mut MGoal, m: &mut Mob) -> bool {
        match g {
            MGoal::RunAround { pos, .. } => {
                if mounts::mob_controlled(m) || mounts::is_tamed(m) || m.mount.passengers.is_empty() {
                    return false;
                }
                match self.default_random_pos(m, 5, 4) {
                    Some(p) => {
                        *pos = p;
                        true
                    }
                    None => false,
                }
            }
            MGoal::RandomStand { next } => {
                *next += 1;
                if *next > 0 && m.rng.next_int_bound(1000) < *next {
                    *next = -m.kind.info().ambient_interval;
                    !mounts::immobile(m) && m.rng.next_int_bound(10) == 0
                } else {
                    false
                }
            }
            MGoal::Spit { target, .. } => match m.target {
                Some(t) if self.ent_alive(t) => {
                    *target = Some(t);
                    true
                }
                _ => false,
            },
            MGoal::Caravan { .. } => self.caravan_can_use(m),
            MGoal::LlamaHurtBy(inner) => self.goal_can_use(inner, m),
            MGoal::WolfTarget(inner) => {
                quarter_range(m, true);
                let r = self.goal_can_use(inner, m);
                quarter_range(m, false);
                r
            }
            MGoal::Trap => m.mount.trap && self.player_near_alive(m.pos, 10.0),
        }
    }

    pub(super) fn mount_can_continue(&mut self, g: &mut MGoal, m: &mut Mob) -> bool {
        match g {
            MGoal::RunAround { .. } => !mounts::is_tamed(m) && !m.nav.is_done() && !m.mount.passengers.is_empty(),
            MGoal::RandomStand { .. } => false,
            MGoal::Spit { target, .. } => {
                let now = matches!(m.target, Some(t) if self.ent_alive(t));
                if now {
                    *target = m.target;
                    return true;
                }
                target.is_some_and(|t| self.ent_alive(t)) && !m.nav.is_done()
            }
            MGoal::Caravan { speed, dist_check } => self.caravan_can_continue(m, speed, dist_check),
            MGoal::LlamaHurtBy(inner) => {
                if m.mount.did_spit {
                    m.mount.did_spit = false;
                    return false;
                }
                self.goal_can_continue(inner, m)
            }
            MGoal::WolfTarget(inner) => {
                quarter_range(m, true);
                let r = self.goal_can_continue(inner, m);
                quarter_range(m, false);
                r
            }
            MGoal::Trap => m.mount.trap && self.player_near_alive(m.pos, 10.0),
        }
    }

    pub(super) fn mount_start(&mut self, g: &mut MGoal, m: &mut Mob) {
        match g {
            MGoal::RunAround { speed, pos } => {
                let (s, p) = (*speed, *pos);
                self.move_to_pos(m, p, s);
            }
            MGoal::RandomStand { .. } => {
                mounts::stand_if_possible(m);
                if let Some(s) = mounts::ambient_sound(m) {
                    self.mount_sound(m, &s, 1.0, 1.0);
                }
            }
            MGoal::LlamaHurtBy(inner) => self.goal_start(inner, m),
            MGoal::WolfTarget(inner) => {
                quarter_range(m, true);
                self.goal_start(inner, m);
                quarter_range(m, false);
            }
            _ => {}
        }
    }

    pub(super) fn mount_stop(&mut self, g: &mut MGoal, m: &mut Mob) {
        match g {
            MGoal::Spit { target, attack_time, see, .. } => {
                *target = None;
                *see = 0;
                *attack_time = -1;
            }
            MGoal::Caravan { speed, .. } => {
                self.leave_caravan(m);
                *speed = 2.1;
            }
            MGoal::LlamaHurtBy(inner) => self.goal_stop(inner, m),
            MGoal::WolfTarget(inner) => {
                quarter_range(m, true);
                self.goal_stop(inner, m);
                quarter_range(m, false);
            }
            _ => {}
        }
    }

    pub(super) fn mount_tick(&mut self, g: &mut MGoal, m: &mut Mob) {
        match g {
            MGoal::RunAround { .. } => self.run_around_tick(m),
            MGoal::Spit { speed, target, attack_time, see } => {
                let Some(t) = *target else { return };
                self.spit_tick(m, t, *speed, attack_time, see);
            }
            MGoal::Caravan { speed, .. } => {
                let s = *speed;
                self.caravan_tick(m, s);
            }
            MGoal::LlamaHurtBy(inner) => self.goal_tick(inner, m),
            MGoal::WolfTarget(inner) => {
                quarter_range(m, true);
                self.goal_tick(inner, m);
                quarter_range(m, false);
            }
            MGoal::Trap => self.trap_trigger(m),
            MGoal::RandomStand { .. } => {}
        }
    }

    /// Living player, not a spectator, within `r`.
    pub(crate) fn player_near_alive(&self, p: [f64; 3], r: f64) -> bool {
        self.game_mode != 3 && self.ent_alive(Ent::Player) && super::dist2(self.pos, p) < r * r
    }

    /// Tick of the wild run: 1 chance in 25 to try taming or to throw off.
    fn run_around_tick(&mut self, m: &mut Mob) {
        if mounts::is_tamed(m) || m.rng.next_int_bound(super::reduced(50)) != 0 {
            return;
        }
        let Some(first) = m.mount.passengers.first().copied() else { return };
        if first == Ent::Player {
            let temper = m.mount.temper;
            let max = mounts::max_temper(m.kind);
            if max > 0 && m.rng.next_int_bound(max) < temper {
                self.tame_with_name(m);
                return;
            }
            mounts::modify_temper(m, 5);
        }
        self.eject_passengers(m);
        self.mount_make_mad(m);
        self.mob_event(m, 6);
    }

    /// Owner, tamed flag, event 7 (hearts).
    pub(crate) fn tame_with_name(&mut self, m: &mut Mob) {
        m.mount.owner = Some(self.uuid);
        m.mount.set_flag(mounts::FLAG_TAME, true);
        self.mob_event(m, 7);
        self.s9_tamed(m); // S9
    }

    /// Tick of the spit: approach, stop at seen range 5 ticks, spit every 40 ticks.
    fn spit_tick(&mut self, m: &mut Mob, t: Ent, speed: f64, attack_time: &mut i32, see: &mut i32) {
        let Some(tp) = self.ent_pos(t) else { return };
        let d2 = super::dist2(m.pos, tp);
        let sight = self.line_of_sight(m, t);
        if sight {
            *see += 1;
        } else {
            *see = 0;
        }
        if d2 <= 400.0 && *see >= 5 {
            m.nav.stop();
        } else {
            self.move_to_entity(m, tp, speed);
        }
        let eye = self.ent_eye(t).unwrap_or(tp[1]);
        self.look_at(m, [tp[0], eye, tp[2]], 30.0, 30.0);
        // Minimum interval = maximum = 40: the delay does not depend on the distance (the
        // "power" computed by the game is not used by the spit).
        *attack_time -= 1;
        if *attack_time == 0 {
            if !sight {
                return;
            }
            self.llama_spit(m, t);
            *attack_time = 40;
        } else if *attack_time < 0 {
            *attack_time = 40;
        }
    }

    /// Does the trap still hold (goal present and trap active)?
    fn trap_goal_present(m: &Mob) -> bool {
        m.goals.goals.iter().any(|w| matches!(w.goal, Goal::Mount(MGoal::Trap)))
    }

    /// The trap goal is added at the end of the list (priority 1).
    pub(crate) fn add_trap_goal(m: &mut Mob) {
        if !Self::trap_goal_present(m) {
            m.goals.add(1, Goal::Mount(MGoal::Trap));
        }
    }
}
