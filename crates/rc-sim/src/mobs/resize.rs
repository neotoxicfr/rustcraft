//! Refit of a mob whose box has just grown (baby turned adult, placement, size change): the
//! center is moved to the nearest free point, searched in a small box around the old center, like
//! the official game's server does.
//!
//! The search reproduces the official game's shape operation: allowed box minus the union of the
//! collision boxes inflated by half the size, cut into cells on the grid of their edges, free
//! cells merged into boxes in the same order (z, then x, then y), and the first nearest point
//! kept (ties: the first box wins).

use rc_physics::Aabb;

use super::mob::Mob;
use crate::server::Server;

/// Box of center `c` and sides `s`.
fn of_size(c: [f64; 3], s: [f64; 3]) -> Aabb {
    Aabb { min: [0, 1, 2].map(|a| c[a] - s[a] / 2.0), max: [0, 1, 2].map(|a| c[a] + s[a] / 2.0) }
}

/// Cell edges along an axis: bounds of the allowed box and edges of the obstacles strictly
/// inside it, sorted, merged within 1e-7.
fn grid(lo: f64, hi: f64, obstacles: &[Aabb], a: usize) -> Vec<f64> {
    let mut g = Vec::with_capacity(2 + obstacles.len() * 2);
    g.push(lo);
    g.push(hi);
    for b in obstacles {
        for c in [b.min[a], b.max[a]] {
            if c > lo && c < hi {
                g.push(c);
            }
        }
    }
    g.sort_by(f64::total_cmp);
    g.dedup_by(|b, a| *b - *a < 1.0e-7);
    g
}

/// Free point nearest to `preferred` in `allowed` for a box of sides `size`, given the
/// collision boxes `blocks` (placed in the world); `None` if everything is taken.
pub(crate) fn closest_free(allowed: &Aabb, preferred: [f64; 3], size: [f64; 3], blocks: &[Aabb]) -> Option<[f64; 3]> {
    if (0..3).any(|a| allowed.max[a] - allowed.min[a] < 1.0e-7) {
        return None;
    }
    // Obstacles: boxes inflated by half the size that overlap the allowed box.
    let obstacles: Vec<Aabb> = blocks
        .iter()
        .map(|b| Aabb {
            min: [0, 1, 2].map(|a| b.min[a] - size[a] / 2.0),
            max: [0, 1, 2].map(|a| b.max[a] + size[a] / 2.0),
        })
        .filter(|o| (0..3).all(|a| o.min[a] < allowed.max[a] && o.max[a] > allowed.min[a]))
        .collect();
    let [gx, gy, gz] = [0, 1, 2].map(|a| grid(allowed.min[a], allowed.max[a], &obstacles, a));
    let (nx, ny, nz) = (gx.len() - 1, gy.len() - 1, gz.len() - 1);
    let mid = |g: &[f64], i: usize| (g[i] + g[i + 1]) / 2.0;
    let at = |x: usize, y: usize, z: usize| (x * ny + y) * nz + z;
    let mut full = vec![false; nx * ny * nz];
    for x in 0..nx {
        for y in 0..ny {
            for z in 0..nz {
                let p = [mid(&gx, x), mid(&gy, y), mid(&gz, z)];
                full[at(x, y, z)] = !obstacles.iter().any(|o| (0..3).all(|a| o.min[a] < p[a] && p[a] < o.max[a]));
            }
        }
    }
    let strip = |full: &[bool], z0: usize, z1: usize, x: usize, y: usize| {
        x < nx && y < ny && (z0..z1).all(|z| full[at(x, y, z)])
    };
    let mut best: Option<([f64; 3], f64)> = None;
    for y in 0..ny {
        for x in 0..nx {
            let mut start: Option<usize> = None;
            for z in 0..=nz {
                if z < nz && full[at(x, y, z)] {
                    start.get_or_insert(z);
                    continue;
                }
                let Some(z0) = start.take() else { continue };
                let clear = |full: &mut [bool], x: usize, y: usize| {
                    for zz in z0..z {
                        full[at(x, y, zz)] = false;
                    }
                };
                clear(&mut full, x, y);
                let mut ex = x;
                while strip(&full, z0, z, ex + 1, y) {
                    clear(&mut full, ex + 1, y);
                    ex += 1;
                }
                let mut ey = y;
                while (x..=ex).all(|cx| strip(&full, z0, z, cx, ey + 1)) {
                    for cx in x..=ex {
                        clear(&mut full, cx, ey + 1);
                    }
                    ey += 1;
                }
                let lo = [gx[x], gy[y], gz[z0]];
                let hi = [gx[ex + 1], gy[ey + 1], gz[z]];
                let c = [0, 1, 2].map(|a| preferred[a].clamp(lo[a], hi[a]));
                let d = (0..3).map(|a| (preferred[a] - c[a]) * (preferred[a] - c[a])).sum::<f64>();
                if best.is_none_or(|(_, bd)| d < bd) {
                    best = Some((c, d));
                }
            }
        }
    }
    best.map(|(c, _)| c)
}

impl Server {
    /// Size of the mob compared with the last one seen: if the box has grown (not the first
    /// reading, not a vex that passes through blocks, new size <= 4), the position is refit.
    /// Called at the start and at the end of the common tick.
    pub(crate) fn mob_refit(&mut self, m: &mut Mob) {
        let (w, h) = m.dims_wh();
        let Some((ow, oh)) = m.body_fx.dims.replace((w, h)) else { return };
        if (w <= ow && h <= oh) || w > 4.0 || h > 4.0 || super::illagers::no_physics(m) {
            return;
        }
        self.refit_after_growth(m, (ow, oh), (w, h));
    }

    /// Refit after growth from `old` to `new` (width, height): nearest free center in a box of
    /// the size of the gain around the old center; otherwise, if both dimensions have grown,
    /// the same search at the old height. True if the mob moved.
    pub(crate) fn refit_after_growth(&mut self, m: &mut Mob, old: (f32, f32), new: (f32, f32)) -> bool {
        let ((ow, oh), (w, h)) = (old, new);
        let center = [m.pos()[0], m.pos()[1] + f64::from(oh) / 2.0, m.pos()[2]];
        let wd = f64::from((w - ow).max(0.0)) + 1.0e-6;
        let hd = f64::from((h - oh).max(0.0)) + 1.0e-6;
        let (fw, fh, foh) = (f64::from(w), f64::from(h), f64::from(oh));
        if let Some(c) = self.find_free_position(&of_size(center, [wd, hd, wd]), center, [fw, fh, fw]) {
            m.set_pos([c[0], c[1] - fh / 2.0, c[2]]);
            return true;
        }
        if w > ow && h > oh {
            let allowed = of_size(center, [wd, 1.0e-6, wd]);
            if let Some(c) = self.find_free_position(&allowed, center, [fw, foh, fw]) {
                m.set_pos([c[0], c[1] - foh / 2.0 + 1.0e-6, c[2]]);
                return true;
            }
        }
        false
    }

    /// Free center for a box `size` in `allowed`: collision boxes of the blocks of the area
    /// inflated by the full size, without those that leave the world border.
    fn find_free_position(&self, allowed: &Aabb, preferred: [f64; 3], size: [f64; 3]) -> Option<[f64; 3]> {
        let area = Aabb { min: [0, 1, 2].map(|a| allowed.min[a] - size[a]), max: [0, 1, 2].map(|a| allowed.max[a] + size[a]) };
        let w = self.world();
        let mut boxes = Vec::new();
        rc_physics::collide::collect(&w, &area, &mut boxes);
        if let Some(border) = rc_physics::World::border(&w) {
            let e = f64::from(1.0e-5f32);
            boxes.retain(|b| border.within(b.min[0], b.min[2]) && border.within(b.max[0] - e, b.max[2] - e));
        }
        closest_free(allowed, preferred, size, &boxes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(x: f64, y: f64, z: f64) -> Aabb {
        Aabb { min: [x, y, z], max: [x + 1.0, y + 1.0, z + 1.0] }
    }

    /// Nothing around: the preferred center is free and kept as is.
    #[test]
    fn free_center_kept() {
        let allowed = of_size([0.5, 1.0, 0.5], [0.3, 0.7, 0.3]);
        assert_eq!(closest_free(&allowed, [0.5, 1.0, 0.5], [0.9, 1.4, 0.9], &[]), Some([0.5, 1.0, 0.5]));
    }

    /// Baby turned adult standing on the ground (floor y = 0..1, old height 0.7, new 1.4): only
    /// the top of the allowed box is free; the feet stay on the ground.
    #[test]
    fn adult_on_ground_stays_landed() {
        let (oh, h) = (0.7f64, 1.4f64);
        let center = [0.5, 1.0 + oh / 2.0, 0.5];
        let hd = (h - oh) + 1.0e-6;
        let allowed = of_size(center, [0.45 + 1.0e-6, hd, 0.45 + 1.0e-6]);
        let c = closest_free(&allowed, center, [0.9, h, 0.9], &[block(0.0, 0.0, 0.0)]).expect("free");
        assert!((c[1] - h / 2.0 - 1.0).abs() < 1.0e-6, "feet at {}", c[1] - h / 2.0);
    }

    /// Wall to the east: the center is pushed back west to the free edge.
    #[test]
    fn wall_pushes_back() {
        // Wall x = 1..2; box of width 0.9: center at most at x = 1 − 0.45.
        let allowed = of_size([0.5, 1.5, 0.5], [0.5, 0.1, 0.5]);
        let c = closest_free(&allowed, [0.9, 1.5, 0.5], [0.9, 1.0, 0.9], &[block(1.0, 1.0, 0.0)]).expect("free");
        assert!((c[0] - 0.55).abs() < 1.0e-9, "x = {}", c[0]);
    }

    /// Everything taken: no position.
    #[test]
    fn everything_taken() {
        let allowed = of_size([0.5, 0.5, 0.5], [0.2, 0.2, 0.2]);
        assert_eq!(closest_free(&allowed, [0.5, 0.5, 0.5], [0.5, 0.5, 0.5], &[block(0.0, 0.0, 0.0)]), None);
    }
}
