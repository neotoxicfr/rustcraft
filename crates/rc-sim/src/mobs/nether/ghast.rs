//! Ghast (`server-mobs.md` § 4.2): free flight (friction 0.91, no gravity), jerky movement
//! control (every 2 to 6 ticks, 0.1 toward the wanted point if the path is
//! free), random floating in a ±16 cube, body turned toward the target or the
//! movement, big fireball shot (warning at 10, shot at 20, recharge at −40).
//! The "cautious" control (happy ghast, F14) goes through the same functions.

use super::super::mob::{Mob, MoveOp};
use super::super::mth::{DEG, atan2, cos, sin};
use crate::server::Server;

/// Flight speed (`flying_speed`, attribute 0.06).
const FLYING_SPEED: f64 = 0.06;

/// Without a target, yaw follows the velocity; target within 64 blocks:
/// yaw toward it (body aligned).
pub fn face_movement(s: &Server, m: &mut Mob) {
    let deg = 180.0f32 / std::f32::consts::PI;
    match m.target {
        None => {
            let v = m.vel;
            m.yaw = -(atan2(v[0], v[2]) as f32) * deg;
            m.body_yaw = m.yaw;
        }
        Some(t) => {
            let Some(tp) = s.ent_pos(t) else { return };
            if super::super::mth::dist2(tp, m.pos) < 4096.0 {
                m.yaw = -(atan2(tp[0] - m.pos[0], tp[2] - m.pos[2]) as f32) * deg;
                m.body_yaw = m.yaw;
            }
        }
    }
}

/// `travel`: movement in flight (water 0.8, lava 0.5, air 0.91), input ×0.02.
pub fn travel(s: &mut Server, m: &mut Mob) {
    s.move_relative_pub(m, 0.02);
    let v = m.vel;
    s.mob_move(m, v);
    let k = if m.in_water {
        f64::from(0.8f32)
    } else if m.lava_height > 0.0 {
        0.5
    } else {
        f64::from(0.91f32)
    };
    m.vel = m.vel.map(|x| x * k);
}

/// Tick of the ghast's movement control (non-cautious): jerks toward the wanted point, wait if blocked.
pub fn move_control(s: &Server, m: &mut Mob) {
    if m.move_ctl.op != MoveOp::MoveTo {
        return;
    }
    let old = m.neth.float_duration;
    m.neth.float_duration -= 1;
    if old > 0 {
        return;
    }
    m.neth.float_duration += m.rng.next_int_bound(5) + 2;
    let w = m.move_ctl.wanted;
    let travel = [w[0] - m.pos[0], w[1] - m.pos[1], w[2] - m.pos[2]];
    if can_reach(s, m, travel) {
        let n = crate::mobs::mth::normalize(travel);
        let k = FLYING_SPEED * 5.0 / 3.0;
        m.vel = [m.vel[0] + n[0] * k, m.vel[1] + n[1] * k, m.vel[2] + n[2] * k];
    } else {
        m.move_ctl.op = MoveOp::Wait;
    }
}

/// Walk over the swept cells, shared with the crossed-block pass.
pub(crate) use crate::inside_blocks::each_cell_between;

/// Does the entity's box slid from `from` to `to` touch any
/// of the boxes?
pub(crate) fn collided_along(entity: &rc_physics::Aabb, travel: [f64; 3], boxes: &[rc_physics::Aabb]) -> bool {
    let half = [
        (entity.max[0] - entity.min[0]) * 0.5 - 1.0e-7,
        (entity.max[1] - entity.min[1]) * 0.5 - 1.0e-7,
        (entity.max[2] - entity.min[2]) * 0.5 - 1.0e-7,
    ];
    let from = [
        (entity.min[0] + entity.max[0]) * 0.5,
        (entity.min[1] + entity.max[1]) * 0.5,
        (entity.min[2] + entity.max[2]) * 0.5,
    ];
    let to = [from[0] + travel[0], from[1] + travel[1], from[2] + travel[2]];
    for b in boxes {
        let g = rc_physics::Aabb {
            min: [b.min[0] - half[0], b.min[1] - half[1], b.min[2] - half[2]],
            max: [b.max[0] + half[0], b.max[1] + half[1], b.max[2] + half[2]],
        };
        let contains = |p: [f64; 3]| (0..3).all(|i| p[i] >= g.min[i] && p[i] < g.max[i]);
        if contains(to) || contains(from) {
            return true;
        }
        if super::super::arrows::ray_t(&g, from, travel).is_some() {
            return true;
        }
    }
    false
}

/// The accessibility test (non-cautious): every crossed cell is air, or its collision shape is
/// not touched by the sliding box; the cells of the starting box are ignored.
fn can_reach(s: &Server, m: &Mob, travel: [f64; 3]) -> bool {
    let bb = m.bbox();
    let at = bb.offset(travel[0], travel[1], travel[2]);
    let start = m.pos;
    let end = [start[0] + travel[0], start[1] + travel[1], start[2] + travel[2]];
    each_cell_between(start, end, &at, &mut |p| {
        let unit = rc_physics::Aabb {
            min: [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])],
            max: [f64::from(p[0] + 1), f64::from(p[1] + 1), f64::from(p[2] + 1)],
        };
        if bb.intersects(&unit) {
            return true;
        }
        if s.is_air(p) {
            return true;
        }
        let mut shapes = Vec::new();
        s.phys.collision(s.st(p), &mut shapes);
        let shapes: Vec<rc_physics::Aabb> =
            shapes.iter().map(|b| b.offset(f64::from(p[0]), f64::from(p[1]), f64::from(p[2]))).collect();
        !collided_along(&bb, travel, &shapes)
    })
}

/// Start condition of the "float at random" goal: no wanted point, or point within 1 or beyond 60.
pub fn float_can_use(m: &Mob) -> bool {
    if m.move_ctl.op != MoveOp::MoveTo {
        return true;
    }
    let w = m.move_ctl.wanted;
    let d = [w[0] - m.pos[0], w[1] - m.pos[1], w[2] - m.pos[2]];
    let dd = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
    !(1.0..=3600.0).contains(&dd)
}

/// Start of the "float at random" goal: point drawn in a ±16 cube (entity's random source). Without a
/// restriction zone and with zero block distance, the first point is always kept:
/// the height correction (point brought back under the surface) never happens for the ghast.
pub fn float_start(_s: &mut Server, m: &mut Mob) {
    let c = m.pos;
    let mut r = [0.0; 3];
    for i in 0..3 {
        r[i] = c[i] + f64::from((m.rng.next_float() * 2.0 - 1.0) * 16.0);
    }
    super::super::nav::set_wanted(m, r, 1.0);
}

/// View vector (tabulated sines).
pub fn view_vector(pitch: f32, yaw: f32) -> [f64; 3] {
    let f = pitch * DEG;
    let g = -yaw * DEG;
    let (h, i, j, k) = (cos(g), sin(g), cos(f), sin(f));
    [f64::from(i * j), f64::from(-k), f64::from(h * j)]
}

/// Tick of the ghast's fireball shot goal.
pub fn shoot_tick(s: &mut Server, m: &mut Mob, charge: &mut i32) {
    let Some(t) = m.target else { return };
    let (Some(tp), Some(tb)) = (s.ent_pos(t), s.ent_bbox(t)) else { return };
    if super::super::mth::dist2(tp, m.pos) < 4096.0 && s.line_of_sight(m, t) {
        *charge += 1;
        if *charge == 10 {
            s.level_event(1015, m.block_pos(), 0);
        }
        if *charge == 20 {
            let v = view_vector(m.pitch, m.yaw);
            let half = m.pos[1] + f64::from(m.height()) * 0.5;
            let xd = tp[0] - (m.pos[0] + v[0] * 4.0);
            let yd = (tb.min[1] + (tb.max[1] - tb.min[1]) * 0.5) - (0.5 + half);
            let zd = tp[2] - (m.pos[2] + v[2] * 4.0);
            s.level_event(1016, m.block_pos(), 0);
            // Position: x and z offset by 4 blocks in the view direction; z added to the
            // position already set (the ghast's), like the game.
            let pos = [m.pos[0] + v[0] * 4.0, half + 0.5, m.pos[2] + v[2] * 4.0];
            let power = m.neth.explosion_power;
            s.shoot_fireball(Some(m.uuid), pos, [xd, yd, zd], (m.yaw, m.pitch), true, power);
            *charge = -40;
        }
    } else if *charge > 0 {
        *charge -= 1;
    }
    m.neth.charging = *charge > 10;
}
