//! Vex: flight through blocks (no physics during its tick, no gravity), own movement
//! control (acceleration toward the target, yaw turned toward the target or the movement),
//! wandering around its linked origin, limited life (1 starvation damage every 20 ticks once
//! elapsed), target copied from its summoner.

use super::super::mob::{Mob, MoveOp};
use super::super::mth::{DEG, atan2, cos, sin};
use crate::server::Server;

/// The movement control's position setpoint.
pub fn set_wanted(m: &mut Mob, p: [f64; 3], speed: f64) {
    m.move_ctl.wanted = p;
    m.move_ctl.speed_mod = speed;
    m.move_ctl.op = MoveOp::MoveTo;
}

impl Server {
    /// Summoner resolved (id from the recorded UUID).
    pub(crate) fn vex_owner(&self, m: &mut Mob) -> Option<i32> {
        if let Some(id) = m.ill.owner_id
            && self.mob_ref(id).is_some()
        {
            return Some(id);
        }
        let u = m.ill.owner?;
        let id = self.mobs.get_uuid(u).map(|o| o.id);
        m.ill.owner_id = id;
        id
    }

    /// Tick of the vex's movement control.
    pub(crate) fn vex_move_control(&self, m: &mut Mob) {
        if m.move_ctl.op != MoveOp::MoveTo {
            return;
        }
        let w = m.move_ctl.wanted;
        let d = [w[0] - m.pos[0], w[1] - m.pos[1], w[2] - m.pos[2]];
        let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
        let b = m.bbox();
        let size = ((b.max[0] - b.min[0]) + (b.max[1] - b.min[1]) + (b.max[2] - b.min[2])) / 3.0;
        if len < size {
            m.move_ctl.op = MoveOp::Wait;
            m.vel = m.vel.map(|v| v * 0.5);
            return;
        }
        let k = m.move_ctl.speed_mod * 0.05 / len;
        m.vel = [m.vel[0] + d[0] * k, m.vel[1] + d[1] * k, m.vel[2] + d[2] * k];
        let deg = 180.0f32 / std::f32::consts::PI;
        match m.target.and_then(|t| self.ent_pos(t)) {
            None => {
                let v = m.vel;
                m.yaw = -(atan2(v[0], v[2]) as f32) * deg;
            }
            Some(tp) => {
                m.yaw = -(atan2(tp[0] - m.pos[0], tp[2] - m.pos[2]) as f32) * deg;
            }
        }
        m.body_yaw = m.yaw;
    }

    /// The vex's `travel`: air (friction 0.91, vertical 0.98, no gravity) or fluid,
    /// movement without collision.
    pub(crate) fn vex_travel(&mut self, m: &mut Mob) {
        let input = [f64::from(m.xxa), f64::from(m.yya), f64::from(m.zza)];
        // Air as fluid: 0.02 (the vex is never on the ground).
        let speed = 0.02f32;
        let len = input[0] * input[0] + input[1] * input[1] + input[2] * input[2];
        if len >= 1.0e-7 {
            let v = if len > 1.0 { input.map(|x| x / len.sqrt()) } else { input };
            let v = v.map(|x| x * f64::from(speed));
            let (s, c) = (f64::from(sin(m.yaw * DEG)), f64::from(cos(m.yaw * DEG)));
            m.vel[0] += v[0] * c - v[2] * s;
            m.vel[1] += v[1];
            m.vel[2] += v[2] * c + v[0] * s;
        }
        // Without physics: position advanced without collision, collision flags cleared.
        let v = m.vel;
        m.pos = [m.pos[0] + v[0], m.pos[1] + v[1], m.pos[2] + v[2]];
        m.horizontal_collision = false;
        m.vertical_collision = false;
        if m.in_water {
            m.vel = [m.vel[0] * f64::from(0.8f32), m.vel[1] * f64::from(0.8f32), m.vel[2] * f64::from(0.8f32)];
        } else if m.lava_height > 0.0 {
            m.vel = m.vel.map(|x| x * 0.5);
        } else {
            let (h, vt) = (f64::from(0.91f32), f64::from(0.98f32));
            m.vel = [m.vel[0] * h, m.vel[1] * vt, m.vel[2] * h];
        }
    }

    /// Tick of the vex's random movement goal: three tries at ±7 / ±5 / ±7 from the origin (or the position).
    pub(crate) fn vex_random_move(&self, m: &mut Mob) {
        let o = m.ill.bound.unwrap_or_else(|| m.block_pos());
        for _ in 0..3 {
            let p = [
                o[0] + m.rng.next_int_bound(15) - 7,
                o[1] + m.rng.next_int_bound(11) - 5,
                o[2] + m.rng.next_int_bound(15) - 7,
            ];
            if self.is_air(p) {
                let w = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
                set_wanted(m, w, 0.25);
                if m.target.is_none() {
                    self.look_at(m, w, 180.0, 20.0);
                }
                break;
            }
        }
    }

    /// End of the tick: limited life.
    pub(crate) fn vex_post_tick(&mut self, m: &mut Mob) {
        if let Some(l) = m.ill.life.as_mut() {
            *l -= 1;
            if *l <= 0 {
                *l = 20;
                self.mob_hurt(m, "minecraft:starve", 1.0, None, None);
            }
        }
    }
}
