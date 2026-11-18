//! Client-side world border (`docs/specs/weather-border.md` § 4.2, 4.5): centre, fixed
//! size or in transition (in ticks), interpolated and "current" bounds, warning,
//! collision shape.

use crate::Aabb;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Extent {
    Fixed(f64),
    /// From `from` to `to` in `total` ticks; `left` ticks remaining, current and previous size.
    Moving {
        from: f64,
        to: f64,
        total: i64,
        left: i64,
        size: f64,
        prev: f64,
    },
}

/// Border state: growing, shrinking or fixed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Growing,
    Shrinking,
    Stationary,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WorldBorder {
    pub center: [f64; 2],
    /// Maximum absolute size (`M`).
    pub max_size: i32,
    pub warn_blocks: i32,
    pub warn_ticks: i32,
    extent: Extent,
}

impl Default for WorldBorder {
    fn default() -> Self {
        WorldBorder {
            center: [0.0, 0.0],
            max_size: 29_999_984,
            warn_blocks: 5,
            warn_ticks: 15,
            extent: Extent::Fixed(59_999_968.0),
        }
    }
}

/// Bounds `[min_x, max_x, min_z, max_z]`.
pub type Bounds = [f64; 4];

impl WorldBorder {
    pub fn set_center(&mut self, x: f64, z: f64) {
        self.center = [x, z];
    }

    pub fn set_size(&mut self, s: f64) {
        self.extent = Extent::Fixed(s);
    }

    /// Transition from `from` to `to` in `ticks` ticks (fixed size if both are equal).
    pub fn lerp(&mut self, from: f64, to: f64, ticks: i64) {
        self.extent = if from == to || ticks <= 0 {
            Extent::Fixed(to)
        } else {
            Extent::Moving { from, to, total: ticks, left: ticks, size: from, prev: from }
        };
    }

    /// Initialisation packet: centre, size or transition, then maximum and warnings.
    #[allow(clippy::too_many_arguments)]
    pub fn init(&mut self, x: f64, z: f64, old: f64, new: f64, ticks: i64, max: i32, blocks: i32, warn: i32) {
        self.set_center(x, z);
        if ticks > 0 {
            self.lerp(old, new, ticks)
        } else {
            self.set_size(new)
        }
        self.max_size = max;
        self.warn_blocks = blocks;
        self.warn_ticks = warn;
    }

    /// Client tick: the transition advances by one tick.
    pub fn tick(&mut self) {
        if let Extent::Moving { from, to, total, left, size, prev } = &mut self.extent {
            *left -= 1;
            *prev = *size;
            let progress = (*total - *left) as f64 / *total as f64;
            *size = if progress >= 1.0 { *to } else { *from + (*to - *from) * progress };
            if *left <= 0 {
                self.extent = Extent::Fixed(*to);
            }
        }
    }

    pub fn size(&self) -> f64 {
        match self.extent {
            Extent::Fixed(s) => s,
            Extent::Moving { size, .. } => size,
        }
    }

    fn size_at(&self, pt: f64) -> f64 {
        match self.extent {
            Extent::Fixed(s) => s,
            Extent::Moving { size, prev, .. } => prev + (size - prev) * pt,
        }
    }

    pub fn status(&self) -> Status {
        match self.extent {
            Extent::Moving { from, to, .. } if to > from => Status::Growing,
            Extent::Moving { from, to, .. } if to < from => Status::Shrinking,
            _ => Status::Stationary,
        }
    }

    /// Transition speed (blocks per tick) and target size.
    pub fn speed_and_target(&self) -> (f64, f64) {
        match self.extent {
            Extent::Fixed(s) => (0.0, s),
            Extent::Moving { from, to, total, .. } => ((from - to).abs() / total as f64, to),
        }
    }

    /// Bounds interpolated at tick fraction `pt` (rendering).
    pub fn bounds_at(&self, pt: f64) -> Bounds {
        let h = self.size_at(pt) / 2.0;
        let m = f64::from(self.max_size);
        let c = |v: f64| v.clamp(-m, m);
        [c(self.center[0] - h), c(self.center[0] + h), c(self.center[1] - h), c(self.center[1] + h)]
    }

    /// "Current" bounds (collision, distance, vignette): interpolated at `pt = 0`.
    pub fn bounds(&self) -> Bounds {
        self.bounds_at(0.0)
    }

    /// Point within the current bounds (minimum included, maximum excluded); for a block, its integer
    /// (x, z) corner.
    pub fn within(&self, x: f64, z: f64) -> bool {
        let b = self.bounds();
        x >= b[0] && x < b[1] && z >= b[2] && z < b[3]
    }

    /// Block within the current bounds.
    pub fn block_within(&self, pos: [i32; 3]) -> bool {
        self.within(f64::from(pos[0]), f64::from(pos[2]))
    }

    /// Distance of a point to the current bounds, negative outside.
    pub fn distance(&self, x: f64, z: f64) -> f64 {
        let b = self.bounds();
        (x - b[0]).min(b[1] - x).min(z - b[2]).min(b[3] - z)
    }

    /// Weight of the vignette warning (0 far, 1 outside), `hud.md` § 5.3.
    pub fn warning(&self, x: f64, z: f64) -> f32 {
        let d = self.distance(x, z);
        let (speed, target) = self.speed_and_target();
        let s = f64::from(self.warn_blocks).max((speed * f64::from(self.warn_ticks)).min((target - self.size()).abs()));
        if d < s { (1.0 - d / s).clamp(0.0, 1.0) as f32 } else { 0.0 }
    }

    /// The border's shape counts for the entity standing at `asker` (x, z) whose box, tested or
    /// swept, is `b` (§ 4.5): that entity stands inside the border grown by the box's widest
    /// horizontal side (at least 1), closer than twice that to an edge. The game judges it from
    /// where the entity stands, not from the box: a box tested away from its entity (a pet's
    /// teleport) can cross a border its entity is far from.
    pub fn close_to(&self, asker: [f64; 2], b: &Aabb) -> bool {
        let [x, z] = asker;
        let m = (b.max[0] - b.min[0]).abs().max((b.max[2] - b.min[2]).abs()).max(1.0);
        let k = self.bounds();
        let inside = k[0] - m <= x && x < k[1] + m && k[2] - m <= z && z < k[3] + m;
        inside && self.distance(x, z) < 2.0 * m
    }

    /// Boxes of the "everything but the interior" shape into `out` when it counts for the entity
    /// standing at `asker` with box `b` ([`Self::close_to`]); nothing if it is far or outside.
    pub fn collision(&self, asker: [f64; 2], b: &Aabb, out: &mut Vec<Aabb>) {
        if !self.close_to(asker, b) {
            return;
        }
        const BIG: f64 = 1.0e30;
        let k = self.bounds();
        let (x0, x1, z0, z1) = (k[0].floor(), k[1].ceil(), k[2].floor(), k[3].ceil());
        out.push(Aabb { min: [-BIG, -BIG, -BIG], max: [x0, BIG, BIG] });
        out.push(Aabb { min: [x1, -BIG, -BIG], max: [BIG, BIG, BIG] });
        out.push(Aabb { min: [x0, -BIG, -BIG], max: [x1, BIG, z0] });
        out.push(Aabb { min: [x0, -BIG, z1], max: [x1, BIG, BIG] });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Bounds of a border of 10 centred on 0: −5 included, 5 excluded, on x as on z.
    #[test]
    fn block_within_bounds() {
        let mut b = WorldBorder::default();
        b.set_size(10.0);
        assert!(b.block_within([-5, 64, 4]) && b.block_within([4, -60, -5]));
        assert!(!b.block_within([5, 64, 0]) && !b.block_within([0, 64, -6]));
        assert!(b.within(4.99, -5.0) && !b.within(5.0, 0.0));
    }

    #[test]
    fn transition_runs_in_ticks_and_current_bounds_lag() {
        let mut w = WorldBorder::default();
        w.lerp(100.0, 50.0, 10);
        assert_eq!(w.status(), Status::Shrinking);
        w.tick();
        assert_eq!(w.size(), 95.0);
        // Current bounds on the previous size (one tick behind).
        assert_eq!(w.bounds(), [-50.0, 50.0, -50.0, 50.0]);
        assert_eq!(w.bounds_at(1.0), [-47.5, 47.5, -47.5, 47.5]);
        for _ in 0..9 {
            w.tick();
        }
        assert_eq!((w.size(), w.status()), (50.0, Status::Stationary));
    }

    #[test]
    fn warning_and_collision_near_edge() {
        let mut w = WorldBorder::default();
        w.set_center(1230.0, 30.0);
        w.set_size(20.0);
        assert_eq!(w.warning(1230.0, 30.0), 0.0);
        assert!((w.warning(1239.0, 30.0) - 0.8).abs() < 1e-6);
        assert_eq!(w.warning(1241.0, 30.0), 1.0);
        let player = Aabb { min: [1238.7, -60.0, 29.7], max: [1239.3, -58.2, 30.3] };
        let mut boxes = Vec::new();
        w.collision([1239.0, 30.0], &player, &mut boxes);
        assert_eq!(boxes.len(), 4);
        // Movement towards the east blocked at the edge (x = 1240).
        let m = crate::collide::sweep(&boxes, &player, [2.0, 0.0, 0.0]);
        assert!((m[0] - 0.7).abs() < 1e-9, "{m:?}");
        // Far from the edge: no shape.
        let far = Aabb { min: [1229.7, -60.0, 29.7], max: [1230.3, -58.2, 30.3] };
        boxes.clear();
        w.collision([1230.0, 30.0], &far, &mut boxes);
        assert!(boxes.is_empty());
    }

    /// The border counts from where the entity stands: a box across the edge tested for an entity
    /// far inside does not see it, a box far inside tested for an entity at the edge does; a wider
    /// box (a movement's swept box) brings the border from further away.
    #[test]
    fn closeness_is_judged_from_the_entity_not_the_box() {
        let mut w = WorldBorder::default();
        w.set_size(20.0);
        let across_the_edge = Aabb { min: [9.7, 0.0, -0.3], max: [10.3, 1.8, 0.3] };
        let far_inside = Aabb { min: [-0.3, 0.0, -0.3], max: [0.3, 1.8, 0.3] };
        assert!(!w.close_to([0.0, 0.0], &across_the_edge), "its entity 10 blocks from the edge");
        assert!(w.close_to([9.0, 0.0], &far_inside), "its entity 1 block from the edge");
        assert!(!w.close_to([11.5, 0.0], &across_the_edge), "its entity more than the box's side outside");
        // 2.5 blocks from the edge: a box of 1 does not count it, a box swept 2 blocks wide does.
        let swept = far_inside.expand_towards([2.0, 0.0, 0.0]);
        assert!(!w.close_to([7.5, 0.0], &far_inside) && w.close_to([7.5, 0.0], &swept));
    }

    /// An empty world with a border, searched for an entity standing at `asker` (`None`: at the
    /// centre of its box).
    struct Open<'a> {
        border: &'a WorldBorder,
        asker: Option<[f64; 2]>,
    }

    impl crate::World for Open<'_> {
        fn state(&self, _x: i32, _y: i32, _z: i32) -> Option<u32> {
            Some(0)
        }

        fn collision(&self, _x: i32, _y: i32, _z: i32, _out: &mut Vec<Aabb>) -> bool {
            false
        }

        fn border(&self) -> Option<&WorldBorder> {
            Some(self.border)
        }

        fn asker_position(&self) -> Option<[f64; 2]> {
            self.asker
        }
    }

    /// A fast movement counts the border from as far as its swept box is wide: a box 2.5 blocks from
    /// the edge moving 3 blocks stops at it (judged from the box before the movement, it crossed).
    #[test]
    fn a_fast_movement_stops_at_the_border() {
        let mut w = WorldBorder::default();
        w.set_size(20.0);
        let b = Aabb { min: [7.2, 0.0, -0.3], max: [7.8, 1.8, 0.3] };
        let m = crate::collide::resolve(&Open { border: &w, asker: None }, &b, [3.0, 0.0, 0.0], false, 0.0);
        assert!((m[0] - 2.2).abs() < 1e-9, "stopped at the edge x = 10: {m:?}");
    }

    /// A free-room test counts the border from where its entity stands, not from the box tested.
    #[test]
    fn a_free_room_test_counts_the_border_from_its_entity() {
        let mut w = WorldBorder::default();
        w.set_size(20.0);
        let across = Aabb { min: [9.7, 0.0, -0.3], max: [10.3, 1.8, 0.3] };
        assert!(!crate::collide::is_free(&Open { border: &w, asker: None }, &across), "for the box's own entity");
        assert!(crate::collide::is_free(&Open { border: &w, asker: Some([0.0, 0.0]) }, &across), "for one far inside");
        assert!(crate::collide::is_free_for_no_entity(&Open { border: &w, asker: None }, &across), "for no entity");
    }
}
