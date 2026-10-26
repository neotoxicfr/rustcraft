//! Random positions (the hover position draw, the aerial position draw,
//! the aerial or aquatic position draw): ten draws, best weight, on the entity random. Only the
//! air and water positions take a residence area into account (happy ghastling); the others
//! serve species that have none (bee, fox, panda).

use super::super::env::Fluid;
use super::super::goals::{rpos_restricted, static_type_of, within_home};
use super::super::mob::Mob;
use super::super::mth;
use crate::server::Server;

const SQRT_OF_TWO: f64 = std::f32::consts::SQRT_2 as f64;

/// The direction draw.
pub fn random_direction(m: &mut Mob, h: i32, v: i32) -> [i32; 3] {
    let x = m.rng.next_int_bound(2 * h + 1) - h;
    let y = m.rng.next_int_bound(2 * v + 1) - v;
    let z = m.rng.next_int_bound(2 * h + 1) - h;
    [x, y, z]
}

/// The direction draw in a cone.
#[allow(clippy::too_many_arguments)]
pub fn direction_within_radians(
    m: &mut Mob,
    min_h: f64,
    max_h: f64,
    v: i32,
    flying_height: i32,
    xdir: f64,
    zdir: f64,
    max_rad: f64,
) -> Option<[i32; 3]> {
    let center = mth::atan2(zdir, xdir) - f64::from(std::f32::consts::FRAC_PI_2);
    let yr = center + f64::from(2.0 * m.rng.next_float() - 1.0) * max_rad;
    let dist = (min_h + m.rng.next_double().sqrt() * (max_h - min_h)) * SQRT_OF_TWO;
    let xt = -dist * yr.sin();
    let zt = dist * yr.cos();
    if xt.abs() > max_h || zt.abs() > max_h {
        return None;
    }
    let yt = m.rng.next_int_bound(2 * v + 1) - v + flying_height;
    Some([xt.floor() as i32, yt, zt.floor() as i32])
}

/// The position draw toward a direction without residence: direction added to the position.
pub fn toward(m: &Mob, d: [i32; 3]) -> [i32; 3] {
    [
        (f64::from(d[0]) + m.pos[0]).floor() as i32,
        (f64::from(d[1]) + m.pos[1]).floor() as i32,
        (f64::from(d[2]) + m.pos[2]).floor() as i32,
    ]
}

fn max_y(s: &Server) -> i32 {
    s.dim.height.min_y + s.dim.height.sections as i32 * 16 - 1
}

fn outside_limits(s: &Server, p: [i32; 3]) -> bool {
    p[1] < s.dim.height.min_y || p[1] > max_y(s)
}

/// The path malus test (static type of the cell, species malus).
pub fn has_malus(s: &Server, m: &Mob, p: [i32; 3]) -> bool {
    let view = s.mob_view(m);
    super::super::nav::mob_malus(m, static_type_of(s, &view, p)) != 0.0
}

fn is_water(s: &Server, p: [i32; 3]) -> bool {
    s.mob_fluid(p) == Fluid::Water
}

/// Climbing out of the solid.
pub fn up_out_of_solid(s: &Server, mut p: [i32; 3]) -> [i32; 3] {
    if !s.is_solid(p) {
        return p;
    }
    p[1] += 1;
    while p[1] <= max_y(s) && s.is_solid(p) {
        p[1] += 1;
    }
    p
}

/// Climbing above the solid.
pub fn up_to_above_solid(s: &Server, mut p: [i32; 3], amount: i32) -> [i32; 3] {
    if !s.is_solid(p) {
        return p;
    }
    p[1] += 1;
    while p[1] <= max_y(s) && s.is_solid(p) {
        p[1] += 1;
    }
    let first = p[1];
    while p[1] <= max_y(s) && p[1] - first < amount {
        p[1] += 1;
        if s.is_solid(p) {
            p[1] -= 1;
            break;
        }
    }
    p
}

/// Ten tries, best weight strictly greater.
fn best_of(
    s: &Server,
    m: &mut Mob,
    mut make: impl FnMut(&Server, &mut Mob) -> Option<[i32; 3]>,
    weight: impl Fn(&Server, &Mob, [i32; 3]) -> f64,
) -> Option<[f64; 3]> {
    let mut best: Option<(f64, [i32; 3])> = None;
    for _ in 0..10 {
        if let Some(p) = make(s, m) {
            let w = weight(s, m, p);
            if w > best.map_or(f64::NEG_INFINITY, |b| b.0) {
                best = Some((w, p));
            }
        }
    }
    best.map(|(_, p)| [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5])
}

fn walk_weight(s: &Server, m: &Mob, p: [i32; 3]) -> f64 {
    f64::from(s.walk_target_value(m, p))
}

/// The land position draw toward a direction + climbing out of the solid.
fn land_candidate(s: &Server, m: &Mob, d: [i32; 3]) -> Option<[i32; 3]> {
    let p = toward(m, d);
    if outside_limits(s, p) || !s.nav_stable(m, p) {
        return None;
    }
    let p = up_out_of_solid(s, p);
    (!is_water(s, p) && !has_malus(s, m, p)).then_some(p)
}

/// The land position draw with an own weight.
pub fn land_pos(
    s: &Server,
    m: &mut Mob,
    h: i32,
    v: i32,
    weight: impl Fn(&Server, &Mob, [i32; 3]) -> f64,
) -> Option<[f64; 3]> {
    best_of(
        s,
        m,
        |s, m| {
            let d = random_direction(m, h, v);
            land_candidate(s, m, d)
        },
        weight,
    )
}

/// The hover position draw.
#[allow(clippy::too_many_arguments)]
pub fn hover_pos(
    s: &Server,
    m: &mut Mob,
    h: i32,
    v: i32,
    xdir: f64,
    zdir: f64,
    max_rad: f32,
    hover_max: i32,
    hover_min: i32,
) -> Option<[f64; 3]> {
    best_of(
        s,
        m,
        |s, m| {
            let d = direction_within_radians(m, 0.0, f64::from(h), v, 0, xdir, zdir, f64::from(max_rad))?;
            let p = toward(m, d);
            if outside_limits(s, p) || !s.nav_stable(m, p) {
                return None;
            }
            let amount = m.rng.next_int_bound(hover_max - hover_min + 1) + hover_min;
            let p = up_to_above_solid(s, p, amount);
            (!is_water(s, p) && !has_malus(s, m, p)).then_some(p)
        },
        walk_weight,
    )
}

/// Target point shifted toward the center of the residence if there is one
/// (happy ghastling), refused outside the area when `restrict`.
#[allow(clippy::too_many_arguments)]
pub fn air_and_water_candidate(
    s: &Server,
    m: &mut Mob,
    h: i32,
    v: i32,
    flying_height: i32,
    xdir: f64,
    zdir: f64,
    max_rad: f64,
    restrict: bool,
) -> Option<[i32; 3]> {
    let d = direction_within_radians(m, 0.0, f64::from(h), v, flying_height, xdir, zdir, max_rad)?;
    let p = Server::toward(m, f64::from(h), d);
    if outside_limits(s, p) || (restrict && !within_home(m, p)) {
        return None;
    }
    let p = up_out_of_solid(s, p);
    (!has_malus(s, m, p)).then_some(p)
}

/// The aerial or aquatic position draw.
#[allow(clippy::too_many_arguments)]
pub fn air_and_water_pos(
    s: &Server,
    m: &mut Mob,
    h: i32,
    v: i32,
    flying_height: i32,
    xdir: f64,
    zdir: f64,
    max_rad: f64,
) -> Option<[f64; 3]> {
    let restrict = rpos_restricted(m, f64::from(h));
    best_of(
        s,
        m,
        |s, m| air_and_water_candidate(s, m, h, v, flying_height, xdir, zdir, max_rad, restrict),
        walk_weight,
    )
}

/// Like air and water, but never in water.
pub fn air_pos_towards(
    s: &Server,
    m: &mut Mob,
    h: i32,
    v: i32,
    flying_height: i32,
    towards: [f64; 3],
    max_rad: f64,
) -> Option<[f64; 3]> {
    let dir = [towards[0] - m.pos[0], towards[2] - m.pos[2]];
    let restrict = rpos_restricted(m, f64::from(h));
    best_of(
        s,
        m,
        |s, m| {
            let p = air_and_water_candidate(s, m, h, v, flying_height, dir[0], dir[1], max_rad, restrict)?;
            (!is_water(s, p)).then_some(p)
        },
        walk_weight,
    )
}
