//! Random positions of villagers and traders (the default position draw,
//! the land position draw, the objective tools) with the residence area (restriction to an area) and a
//! supplied walk value (null for traders: the first valid position wins).

use super::super::mob::Mob;
use crate::server::Server;

/// Residence area of a mob (center, radius), `None` without restriction.
pub type Home = Option<([i32; 3], i32)>;

fn rand_dir(m: &mut Mob, h: i32, v: i32) -> [i32; 3] {
    let x = m.rng.next_int_bound(2 * h + 1) - h;
    let y = m.rng.next_int_bound(2 * v + 1) - v;
    let z = m.rng.next_int_bound(2 * h + 1) - h;
    [x, y, z]
}

/// The direction draw in a cone (cone around a direction).
fn dir_within(m: &mut Mob, min_h: f64, max_h: f64, v: i32, dx: f64, dz: f64, rad: f64) -> Option<[i32; 3]> {
    let center = super::super::mth::atan2(dz, dx) - f64::from(std::f32::consts::FRAC_PI_2);
    let yr = center + f64::from(2.0 * m.rng.next_float() - 1.0) * rad;
    let t = m.rng.next_double().sqrt();
    let dist = super::super::mth::rpos_dist(t, min_h, max_h);
    let xt = -dist * yr.sin();
    let zt = dist * yr.cos();
    if xt.abs() > max_h || zt.abs() > max_h {
        return None;
    }
    let yt = m.rng.next_int_bound(2 * v + 1) - v;
    Some([xt.floor() as i32, yt, zt.floor() as i32])
}

fn restricted(m: &Mob, home: Home, h: f64) -> bool {
    home.is_some_and(|(c, r)| {
        let d = [f64::from(c[0]) + 0.5 - m.pos[0], f64::from(c[1]) + 0.5 - m.pos[1], f64::from(c[2]) + 0.5 - m.pos[2]];
        let lim = f64::from(r) + h + 1.0;
        d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < lim * lim
    })
}

fn within_home(home: Home, p: [i32; 3]) -> bool {
    match home {
        None => true,
        Some((c, r)) => {
            let d = [p[0] - c[0], p[1] - c[1], p[2] - c[2]];
            i64::from(d[0]).pow(2) + i64::from(d[1]).pow(2) + i64::from(d[2]).pow(2) < i64::from(r) * i64::from(r)
        }
    }
}

/// Shifted toward the residence center.
fn toward(m: &mut Mob, home: Home, xz: f64, d: [i32; 3]) -> [i32; 3] {
    let (mut xt, mut zt) = (f64::from(d[0]), f64::from(d[2]));
    if let Some((c, _)) = home
        && xz > 1.0
    {
        if m.pos[0] > f64::from(c[0]) {
            xt -= m.rng.next_double() * xz / 2.0;
        } else {
            xt += m.rng.next_double() * xz / 2.0;
        }
        if m.pos[2] > f64::from(c[2]) {
            zt -= m.rng.next_double() * xz / 2.0;
        } else {
            zt += m.rng.next_double() * xz / 2.0;
        }
    }
    [(xt + m.pos[0]).floor() as i32, (f64::from(d[1]) + m.pos[1]).floor() as i32, (zt + m.pos[2]).floor() as i32]
}

impl Server {
    fn outside_height(&self, p: [i32; 3]) -> bool {
        let h = self.dim.height;
        p[1] < h.min_y || p[1] >= h.min_y + h.sections as i32 * 16
    }

    pub(crate) fn pos_has_malus(&self, m: &Mob, p: [i32; 3]) -> bool {
        let view = self.mob_view(m);
        super::super::nav::mob_malus(m, super::super::goals::static_type_of(self, &view, p)) != 0.0
    }

    /// Common filter: height, residence, stable destination (and malus for the default shape).
    fn rpos_valid(&self, m: &Mob, home: Home, restrict: bool, p: [i32; 3], malus: bool) -> bool {
        !self.outside_height(p)
            && !(restrict && !within_home(home, p))
            && self.nav_stable(m, p)
            && !(malus && self.pos_has_malus(m, p))
    }

    /// Climbing out of the solid then refusing water and maluses.
    fn up_out(&self, m: &Mob, mut p: [i32; 3]) -> Option<[i32; 3]> {
        let max = self.dim.height.min_y + self.dim.height.sections as i32 * 16 - 1;
        if self.is_solid(p) {
            p[1] += 1;
            while p[1] <= max && self.is_solid(p) {
                p[1] += 1;
            }
        }
        if self.mob_fluid(p) == super::super::env::Fluid::Water || self.pos_has_malus(m, p) {
            return None;
        }
        Some(p)
    }

    /// 10 tries, best value (strictly greater).
    fn rbest_of(
        &self,
        m: &mut Mob,
        weight: &dyn Fn(&Server, [i32; 3]) -> f64,
        mut generate: impl FnMut(&Server, &mut Mob) -> Option<[i32; 3]>,
    ) -> Option<[f64; 3]> {
        let mut best: Option<(f64, [i32; 3])> = None;
        for _ in 0..10 {
            if let Some(p) = generate(self, m) {
                let w = weight(self, p);
                if w > best.map_or(f64::NEG_INFINITY, |b| b.0) {
                    best = Some((w, p));
                }
            }
        }
        best.map(|b| [f64::from(b.1[0]) + 0.5, f64::from(b.1[1]), f64::from(b.1[2]) + 0.5])
    }

    /// The default position draw.
    pub(crate) fn rpos_default(&self, m: &mut Mob, home: Home, h: i32, v: i32) -> Option<[f64; 3]> {
        let restrict = restricted(m, home, f64::from(h));
        self.rbest_of(m, &|_, _| 0.0, |s, m| {
            let d = rand_dir(m, h, v);
            let p = toward(m, home, f64::from(h), d);
            s.rpos_valid(m, home, restrict, p, true).then_some(p)
        })
    }

    /// The position draw toward a point.
    pub(crate) fn rpos_default_towards(
        &self,
        m: &mut Mob,
        home: Home,
        h: i32,
        v: i32,
        to: [f64; 3],
        rad: f64,
    ) -> Option<[f64; 3]> {
        let (dx, dz) = (to[0] - m.pos[0], to[2] - m.pos[2]);
        let restrict = restricted(m, home, f64::from(h));
        self.rbest_of(m, &|_, _| 0.0, |s, m| {
            let d = dir_within(m, 0.0, f64::from(h), v, dx, dz, rad)?;
            let p = toward(m, home, f64::from(h), d);
            s.rpos_valid(m, home, restrict, p, true).then_some(p)
        })
    }

    /// The land position draw (supplied walk value).
    pub(crate) fn rpos_land(
        &self,
        m: &mut Mob,
        home: Home,
        h: i32,
        v: i32,
        weight: &dyn Fn(&Server, [i32; 3]) -> f64,
    ) -> Option<[f64; 3]> {
        let restrict = restricted(m, home, f64::from(h));
        self.rbest_of(m, weight, |s, m| {
            let d = rand_dir(m, h, v);
            let p = toward(m, home, f64::from(h), d);
            if !s.rpos_valid(m, home, restrict, p, false) {
                return None;
            }
            s.up_out(m, p)
        })
    }

    fn rpos_land_dir(
        &self,
        m: &mut Mob,
        home: Home,
        min_h: f64,
        max_h: f64,
        v: i32,
        dir: [f64; 2],
    ) -> Option<[f64; 3]> {
        let restrict = restricted(m, home, max_h);
        self.rbest_of(m, &|_, _| 0.0, |s, m| {
            let d = dir_within(m, min_h, max_h, v, dir[0], dir[1], f64::from(std::f32::consts::FRAC_PI_2))?;
            let p = toward(m, home, max_h, d);
            if !s.rpos_valid(m, home, restrict, p, false) {
                return None;
            }
            s.up_out(m, p)
        })
    }

    /// The land position draw toward a point (iron golem wandering in the village, F06).
    pub(crate) fn rpos_land_towards(&self, m: &mut Mob, home: Home, h: i32, v: i32, to: [f64; 3]) -> Option<[f64; 3]> {
        let dir = [to[0] - m.pos[0], to[2] - m.pos[2]];
        self.rpos_land_dir(m, home, 0.0, f64::from(h), v, dir)
    }

    /// The land position draw opposite a point.
    pub(crate) fn rpos_land_away(&self, m: &mut Mob, home: Home, h: i32, v: i32, from: [f64; 3]) -> Option<[f64; 3]> {
        let mut dir = [m.pos[0] - from[0], m.pos[2] - from[2]];
        let dy = m.pos[1] - from[1];
        if dir[0] * dir[0] + dy * dy + dir[1] * dir[1] == 0.0 {
            dir = [m.rng.next_double() - 0.5, m.rng.next_double() - 0.5];
        }
        self.rpos_land_dir(m, home, 0.0, f64::from(h), v, dir)
    }
}
