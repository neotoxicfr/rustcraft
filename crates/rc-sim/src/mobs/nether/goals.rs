//! Goals of the F12 family, dispatched by `Goal::Neth`: blaze attack, floating, gaze and
//! shot of the ghast, return to lava and look at another strider.

use super::super::goals::{Goal, LOOK, MOVE, max_head_x};
use super::super::mob::{Ent, Mob};
use super::super::mth::{dist2, reduced};
use super::super::species::Kind;
use crate::server::Server;

#[derive(Debug, Clone)]
pub enum NGoal {
    /// Blaze attack goal (movement, look; every tick).
    BlazeAttack { step: i32, time: i32, last_seen: i32 },
    /// "Float at random" goal (movement; never continued).
    GhastFloat,
    /// Ghast look goal (look; always active, every tick).
    GhastLook,
    /// Ghast fireball shot goal (no flag; every tick).
    GhastShoot { charge: i32 },
    /// Strider "go to lava" goal (range 8, height 2, target: the lava itself).
    ToLava { next_start: i32, try_ticks: i32, pos: [i32; 3], reached: bool },
    /// Another strider.
    LookAtStrider { at: Option<i32>, time: i32 },
}

impl NGoal {
    pub fn wrap(self) -> Goal {
        Goal::Neth(self)
    }

    pub fn flags(&self) -> u8 {
        match self {
            NGoal::BlazeAttack { .. } => MOVE | LOOK,
            NGoal::GhastFloat => MOVE,
            NGoal::GhastLook | NGoal::LookAtStrider { .. } => LOOK,
            NGoal::GhastShoot { .. } => 0,
            NGoal::ToLava { .. } => MOVE | super::super::goals::JUMP,
        }
    }

    pub fn every_tick(&self) -> bool {
        matches!(self, NGoal::BlazeAttack { .. } | NGoal::GhastLook | NGoal::GhastShoot { .. } | NGoal::ToLava { .. })
    }
}

/// Player only outside peaceful difficulty, alive and attackable.
pub(super) fn can_attack(s: &Server, t: Ent) -> bool {
    if !s.ent_alive(t) {
        return false;
    }
    match t {
        Ent::Player => s.game_mode != 1 && s.game_mode != 3 && s.level.difficulty_id() != 0,
        // Never a ghast.
        Ent::Mob(id) => !s.mob_ref(id).is_some_and(|o| o.kind == Kind::Ghast),
    }
}

/// The nearest-block search of "go to a block" (the game's traversal order).
fn find_lava(s: &Server, m: &Mob, range: i32, vrange: i32) -> Option<[i32; 3]> {
    let b = m.block_pos();
    let mut y = 0;
    while y <= vrange {
        for r in 0..range {
            let mut x = 0;
            while x <= r {
                let mut z = if x < r && x > -r { r } else { 0 };
                while z <= r {
                    let p = [b[0] + x, b[1] + y - 1, b[2] + z];
                    if super::strider::lava_target(s, p) {
                        return Some(p);
                    }
                    z = if z > 0 { -z } else { 1 - z };
                }
                x = if x > 0 { -x } else { 1 - x };
            }
        }
        y = if y > 0 { -y } else { 1 - y };
    }
    None
}

impl Server {
    pub(crate) fn neth_can_use(&mut self, g: &mut NGoal, m: &mut Mob) -> bool {
        match g {
            NGoal::BlazeAttack { .. } => m.target.is_some_and(|t| can_attack(self, t)),
            NGoal::GhastFloat => super::ghast::float_can_use(m),
            NGoal::GhastLook => true,
            NGoal::GhastShoot { .. } => m.target.is_some(),
            NGoal::ToLava { next_start, pos, .. } => {
                if m.lava_height > 0.0 {
                    return false;
                }
                if *next_start > 0 {
                    *next_start -= 1;
                    return false;
                }
                *next_start = reduced(200 + m.rng.next_int_bound(200));
                match find_lava(self, m, 8, 2) {
                    Some(p) => {
                        *pos = p;
                        true
                    }
                    None => false,
                }
            }
            NGoal::LookAtStrider { at, .. } => {
                if m.rng.next_float() >= 0.02 {
                    return false;
                }
                // Nearest strider in the box inflated by 8 / 3 / 8, at most 8 blocks away.
                let bb = m.bbox();
                let area = rc_physics::Aabb {
                    min: [bb.min[0] - 8.0, bb.min[1] - 3.0, bb.min[2] - 8.0],
                    max: [bb.max[0] + 8.0, bb.max[1] + 3.0, bb.max[2] + 8.0],
                };
                let eye = [m.pos[0], m.eye_y(), m.pos[2]];
                let mut best: Option<(f64, i32)> = None;
                for o in self.mobs.list.iter().flatten() {
                    if o.kind != Kind::Strider || o.id == m.id || !o.alive() || !o.touches(&area) {
                        continue;
                    }
                    let d = dist2(eye, o.pos);
                    if d > 64.0 {
                        continue;
                    }
                    if best.is_none_or(|b| d < b.0) {
                        best = Some((d, o.id));
                    }
                }
                *at = best.map(|b| b.1);
                at.is_some()
            }
        }
    }

    pub(crate) fn neth_can_continue(&mut self, g: &mut NGoal, m: &mut Mob) -> bool {
        match g {
            NGoal::GhastFloat => false,
            NGoal::ToLava { pos, .. } => m.lava_height <= 0.0 && super::strider::lava_target(self, *pos),
            NGoal::LookAtStrider { at, time } => {
                let Some(o) = at.and_then(|id| self.mob_ref(id)) else { return false };
                o.alive() && dist2(m.pos, o.pos) <= 64.0 && *time > 0
            }
            _ => self.neth_can_use(g, m),
        }
    }

    pub(crate) fn neth_start(&mut self, g: &mut NGoal, m: &mut Mob) {
        match g {
            NGoal::BlazeAttack { step, .. } => *step = 0,
            NGoal::GhastFloat => super::ghast::float_start(self, m),
            NGoal::GhastShoot { charge } => *charge = 0,
            NGoal::ToLava { pos, try_ticks, .. } => {
                // One block above the target; the stay time is drawn
                // (entity's random source) even if the strider does not use it.
                let p = [f64::from(pos[0]) + 0.5, f64::from(pos[1] + 1), f64::from(pos[2]) + 0.5];
                self.move_to_pos(m, p, 1.0);
                *try_ticks = 0;
                let inner = m.rng.next_int_bound(1200) + 1200;
                let _max_stay = m.rng.next_int_bound(inner) + 1200;
            }
            NGoal::LookAtStrider { time, .. } => *time = reduced(40 + m.rng.next_int_bound(40)),
            NGoal::GhastLook => {}
        }
    }

    pub(crate) fn neth_stop(&mut self, g: &mut NGoal, m: &mut Mob) {
        match g {
            NGoal::BlazeAttack { last_seen, .. } => {
                m.neth.charged = false;
                *last_seen = 0;
            }
            NGoal::GhastShoot { .. } => m.neth.charging = false,
            NGoal::LookAtStrider { at, .. } => *at = None,
            _ => {}
        }
    }

    pub(crate) fn neth_tick(&mut self, g: &mut NGoal, m: &mut Mob) {
        match g {
            NGoal::BlazeAttack { step, time, last_seen } => super::blaze::attack_tick(self, m, step, time, last_seen),
            NGoal::GhastLook => super::ghast::face_movement(self, m),
            NGoal::GhastShoot { charge } => super::ghast::shoot_tick(self, m, charge),
            NGoal::ToLava { pos, try_ticks, reached, .. } => {
                // Target: the lava block itself (accepted within 1 block of the center), path recomputed
                // every 20 ticks.
                let t = *pos;
                let c = [f64::from(t[0]) + 0.5, f64::from(t[1]) + 0.5, f64::from(t[2]) + 0.5];
                if dist2(c, m.pos) >= 1.0 {
                    *reached = false;
                    *try_ticks += 1;
                    if *try_ticks % 20 == 0 {
                        let p = [f64::from(t[0]) + 0.5, f64::from(t[1]), f64::from(t[2]) + 0.5];
                        self.move_to_pos(m, p, 1.0);
                    }
                } else {
                    *reached = true;
                    *try_ticks -= 1;
                }
            }
            NGoal::LookAtStrider { at, time } => {
                let Some((p, eye)) = at.and_then(|id| self.mob_ref(id)).map(|o| (o.pos, o.eye_y())) else { return };
                self.look_at(m, [p[0], eye, p[2]], 10.0, max_head_x(m));
                *time -= 1;
            }
            NGoal::GhastFloat => {}
        }
    }
}
