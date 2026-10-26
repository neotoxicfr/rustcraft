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
        let l = (travel[0] * travel[0] + travel[1] * travel[1] + travel[2] * travel[2]).sqrt();
        let n = if l < 1.0e-5 { [0.0; 3] } else { travel.map(|c| c / l) };
        let k = FLYING_SPEED * 5.0 / 3.0;
        m.vel = [m.vel[0] + n[0] * k, m.vel[1] + n[1] * k, m.vel[2] + n[2] * k];
    } else {
        m.move_ctl.op = MoveOp::Wait;
    }
}

/// Whole box (bounds included) of the cells touched by `b`.
fn cells(b: &rc_physics::Aabb) -> [i32; 6] {
    [
        b.min[0].floor() as i32,
        b.min[1].floor() as i32,
        b.min[2].floor() as i32,
        b.max[0].floor() as i32,
        b.max[1].floor() as i32,
        b.max[2].floor() as i32,
    ]
}

fn for_cells(c: [i32; 6], f: &mut dyn FnMut([i32; 3]) -> bool) -> bool {
    let (lo, hi) = ([c[0].min(c[3]), c[1].min(c[4]), c[2].min(c[5])], [c[0].max(c[3]), c[1].max(c[4]), c[2].max(c[5])]);
    for x in lo[0]..=hi[0] {
        for y in lo[1]..=hi[1] {
            for z in lo[2]..=hi[2] {
                if !f([x, y, z]) {
                    return false;
                }
            }
        }
    }
    true
}

/// Computation of the intersection of a ray with a box of one unit cell from `from` toward `to`: entry point through a face.
fn clip_cell(c: [i32; 3], from: [f64; 3], to: [f64; 3]) -> Option<[f64; 3]> {
    let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    // Starting scale 1, fraction kept if 0 < t < scale (strict bounds, tolerance 1e-7).
    let mut scale = 1.0f64;
    let mut hit = false;
    for a in 0..3 {
        let (b1, b2) = ((a + 1) % 3, (a + 2) % 3);
        let plane = if d[a] > 1.0e-7 {
            f64::from(c[a])
        } else if d[a] < -1.0e-7 {
            f64::from(c[a] + 1)
        } else {
            continue;
        };
        let t = (plane - from[a]) / d[a];
        if !(t > 0.0 && t < scale) {
            continue;
        }
        let p1 = from[b1] + t * d[b1];
        let p2 = from[b2] + t * d[b2];
        let inside = |p: f64, i: usize| f64::from(c[i]) - 1.0e-7 < p && p < f64::from(c[i] + 1) + 1.0e-7;
        if inside(p1, b1) && inside(p2, b2) {
            scale = t;
            hit = true;
        }
    }
    hit.then(|| [from[0] + d[0] * scale, from[1] + d[1] * scale, from[2] + d[2] * scale])
}

/// The farthest corner (the game's axis combination reproduced as is).
fn furthest_corner(d: [f64; 3]) -> [i32; 3] {
    let (xd, yd, zd) = (d[0].abs(), d[1].abs(), d[2].abs());
    let sx = if d[0] >= 0.0 { 1 } else { -1 };
    let sy = if d[1] >= 0.0 { 1 } else { -1 };
    let sz = if d[2] >= 0.0 { 1 } else { -1 };
    if xd <= yd && xd <= zd {
        [-sx, -sz, sy]
    } else if yd <= zd {
        [sz, -sy, -sx]
    } else {
        [-sy, sx, -sz]
    }
}

/// True if every visited cell accepts passage.
pub(crate) fn each_cell_between(
    from: [f64; 3],
    to: [f64; 3],
    at_target: &rc_physics::Aabb,
    visit: &mut dyn FnMut([i32; 3]) -> bool,
) -> bool {
    let t = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
    let lsq = t[0] * t[0] + t[1] * t[1] + t[2] * t[2];
    if lsq < f64::from(1.0e-5f32 * 1.0e-5f32) {
        return for_cells(cells(at_target), visit);
    }
    let mut seen = hashbrown::HashSet::new();
    let back = at_target.offset(-t[0], -t[1], -t[2]);
    let ok = for_cells(cells(&back), &mut |p| {
        seen.insert(p);
        visit(p)
    });
    if !ok {
        return false;
    }
    // Corner traversed cell by cell along the path, box of the entity's size at each step.
    let size =
        [at_target.max[0] - at_target.min[0], at_target.max[1] - at_target.min[1], at_target.max[2] - at_target.min[2]];
    let dir = furthest_corner(t);
    let center = [
        (at_target.min[0] + at_target.max[0]) * 0.5,
        (at_target.min[1] + at_target.max[1]) * 0.5,
        (at_target.min[2] + at_target.max[2]) * 0.5,
    ];
    let to_corner: [f64; 3] = std::array::from_fn(|i| center[i] + size[i] * 0.5 * f64::from(dir[i]));
    let from_corner: [f64; 3] = std::array::from_fn(|i| to_corner[i] - t[i]);
    let mut cell = from_corner.map(|v| v.floor() as i32);
    let sign = t.map(|v| {
        if v > 0.0 {
            1
        } else if v < 0.0 {
            -1
        } else {
            0
        }
    });
    let delta: [f64; 3] = std::array::from_fn(|i| if sign[i] == 0 { f64::MAX } else { f64::from(sign[i]) / t[i] });
    let frac = |v: f64| v - v.floor();
    let mut tt: [f64; 3] =
        std::array::from_fn(|i| delta[i] * if sign[i] > 0 { 1.0 - frac(from_corner[i]) } else { frac(from_corner[i]) });
    while tt[0] <= 1.0 || tt[1] <= 1.0 || tt[2] <= 1.0 {
        let a = if tt[0] < tt[1] {
            if tt[0] < tt[2] { 0 } else { 2 }
        } else if tt[1] < tt[2] {
            1
        } else {
            2
        };
        cell[a] += sign[a];
        tt[a] += delta[a];
        let Some(hit) = clip_cell(cell, from_corner, to_corner) else { continue };
        let eps = f64::from(1.0e-5f32);
        let ch: [f64; 3] =
            std::array::from_fn(|i| hit[i].clamp(f64::from(cell[i]) + eps, f64::from(cell[i]) + 1.0 - eps));
        let opp: [i32; 3] = std::array::from_fn(|i| (ch[i] - size[i] * f64::from(dir[i])).floor() as i32);
        let ok = for_cells([cell[0], cell[1], cell[2], opp[0], opp[1], opp[2]], &mut |p| !seen.insert(p) || visit(p));
        if !ok {
            return false;
        }
    }
    for_cells(cells(at_target), &mut |p| !seen.insert(p) || visit(p))
}

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
