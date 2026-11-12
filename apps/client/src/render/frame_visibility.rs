//! The frame's visibility tests (`visibility.md`). Block entities, the walk's debug sections and,
//! with `RUSTCRAFT_OPT_PARTICLE_GAME_FRUSTUM=0`, particles: a box in the view frustum one of whose
//! sections (box plus one block) was reached by the occlusion walk. Entities, leashes and entity
//! debug boxes, the game's test ([`FrameVisibility::entity_shown`]): the box in the frustum, then the
//! section of the block position compiled and appeared (`compiled_sections.rs`), the walk not asked;
//! block entities ask that of their section too. A
//! concrete type lent to every caller, so each test is a direct, inlined call instead of a closure
//! behind a pointer.
//!
//! Paths, read once from their switches (`=0`: the test of before, [`FrameView::verdict`]); every
//! one gives the old verdict, class included (`frame_visibility_test.rs`):
//! - `RUSTCRAFT_OPT_FRAMEVISIBILITY`: floors without the C library and, on the reached grid, each
//!   axis range clamped to the grid once instead of a bounds check per section;
//! - `RUSTCRAFT_OPT_REACHYMASK`: the sections of one column in the box's height range tested as a
//!   run of bits (the grid keeps a column's heights side by side);
//! - `RUSTCRAFT_OPT_POINTVISIBILITY`: a point is tested with six dot products and, away from the
//!   edges of its section, one bit;
//! - `RUSTCRAFT_OPT_PLANE_MASKS`: the corner of a box each plane tests is chosen once per frame.

use std::sync::Arc;
use std::time::Instant;

use glam::Vec3;
use rc_data::java_math::floor_f32_to_i32;

use super::compiled_sections::CompiledSections;
use super::frustum::{BoxVerdict, FrameView, VerdictTally, aabb_visible};
use super::lists::Reach;

/// What the game's entity test reads besides the frustum: the compiled sections, and the instant
/// of the frame and the fade length that tell whether one has appeared enough.
pub(super) struct CompiledView {
    pub sections: Arc<CompiledSections>,
    pub now: Instant,
    pub fade_secs: f64,
}

/// Which paths the frame's visibility test takes; every field off is the test of before.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct VisibilityPaths {
    /// `RUSTCRAFT_OPT_FRAMEVISIBILITY`: the box path below instead of [`FrameView::verdict`].
    pub direct_grid: bool,
    /// `RUSTCRAFT_OPT_REACHYMASK`: a column's height range as one run of bits (on the box path).
    pub y_runs: bool,
    /// `RUSTCRAFT_OPT_POINTVISIBILITY`: points without corner selection, one bit inside a section.
    pub points: bool,
    /// `RUSTCRAFT_OPT_PLANE_MASKS`: corners of the planes chosen once per frame (on the box path).
    pub plane_masks: bool,
    #[cfg(test)]
    pub sabotage: Option<VisibilitySabotage>,
}

/// Faults injected by the tests, each of which must make the equivalence proof fail.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum VisibilitySabotage {
    /// A point on the edge block of its section is taken as lying in that section alone.
    EdgePointInOneSection,
    /// Only the section holding the box's low corner is looked up, its margin ignored.
    NeighboursIgnored,
}

impl VisibilityPaths {
    pub const OLD: VisibilityPaths = VisibilityPaths {
        direct_grid: false,
        y_runs: false,
        points: false,
        plane_masks: false,
        #[cfg(test)]
        sabotage: None,
    };

    pub fn from_switches() -> Self {
        VisibilityPaths {
            direct_grid: rc_data::opt_switch!("RUSTCRAFT_OPT_FRAMEVISIBILITY"),
            y_runs: rc_data::opt_switch!("RUSTCRAFT_OPT_REACHYMASK"),
            points: rc_data::opt_switch!("RUSTCRAFT_OPT_POINTVISIBILITY"),
            plane_masks: rc_data::opt_switch!("RUSTCRAFT_OPT_PLANE_MASKS"),
            #[cfg(test)]
            sabotage: None,
        }
    }

    #[cfg(test)]
    fn sabotaged(&self, s: VisibilitySabotage) -> bool {
        self.sabotage == Some(s)
    }
}

/// The frame's visibility test (module header), lent to the systems that draw boxes.
pub struct FrameVisibility {
    /// `None`: everything is visible (neutral frames such as the interface models, tests).
    view: Option<FrameView>,
    paths: VisibilityPaths,
    /// Per plane and axis, the side of a box (0 min, 1 max) the plane tests: the max side when
    /// the normal points that way, the choice `box_reaches_plane` makes on every call.
    corner_sides: [[usize; 3]; 6],
    tally: Option<VerdictTally>,
    /// `None`: every section counts as compiled and shown.
    compiled: Option<CompiledView>,
}

impl FrameVisibility {
    /// Test of the frame `view`; the perf log counts its calls per verdict.
    pub(super) fn new(view: FrameView, paths: VisibilityPaths) -> Self {
        let corner_sides = view.planes.map(|p| [p.x, p.y, p.z].map(|n| usize::from(n >= 0.0)));
        let tally = crate::perf_log::on().then(VerdictTally::default);
        FrameVisibility { view: Some(view), paths, corner_sides, tally, compiled: None }
    }

    /// The same test, telling the compiled sections apart with `compiled` ([`Self::section_shown`]).
    pub(super) fn with_compiled_sections(self, compiled: Option<CompiledView>) -> Self {
        FrameVisibility { compiled, ..self }
    }

    /// A scripted frame for the tests: a camera at `eye` looking south (+z) in a world of 24
    /// sections from y = -64, whose walk reached `reached` (section x, height from the bottom, z),
    /// with `compiled` (column, height) compiled within `radius` columns of the camera's column, all
    /// long enough ago to have appeared. Also gives the camera's integer origin. The instants are
    /// set by the test: no verdict depends on how fast the machine runs.
    #[cfg(test)]
    pub(crate) fn scripted_for_tests(
        eye: [f64; 3],
        reached: &[(i32, i32, i32)],
        compiled: &[((i32, i32), i32)],
        radius: i32,
    ) -> (Self, [i32; 2]) {
        use super::compiled_sections::ColumnSquare;
        let (origin, _, view_m) = crate::camera::view_matrix(eye, 0.0, 0.0);
        let planes = super::frustum::frustum_planes(super::perspective(70.0, 16.0 / 9.0, 0.05, 512.0) * view_m);
        let reached = Reach::Set(reached.iter().copied().collect());
        let view = FrameView { planes, origin, min_y: -64, sections: 24, reached: Arc::new(reached), occlusion: true };
        let base = Instant::now();
        let mut sections = CompiledSections::new(base);
        let center = (eye[0].floor() as i32 >> 4, eye[2].floor() as i32 >> 4);
        sections.reposition(ColumnSquare { center, radius });
        for &(key, sy) in compiled {
            sections.mark(key, 24, [sy].into_iter(), base);
        }
        let compiled = CompiledView {
            sections: Arc::new(sections),
            now: base + std::time::Duration::from_secs(10),
            fade_secs: 0.75,
        };
        (Self::new(view, VisibilityPaths::OLD).with_compiled_sections(Some(compiled)), origin)
    }

    /// A test that finds every box visible.
    pub fn everything() -> Self {
        FrameVisibility {
            view: None,
            paths: VisibilityPaths::OLD,
            corner_sides: [[0; 3]; 6],
            tally: None,
            compiled: None,
        }
    }

    /// The box `min..max` (frame relative to the camera's integer origin) is in the view frustum,
    /// the occlusion walk left out: the first half of the game's entity test.
    #[inline]
    pub fn frustum_box(&self, min: [f32; 3], max: [f32; 3]) -> bool {
        let Some(view) = &self.view else { return true };
        if self.paths.plane_masks {
            self.box_reaches_planes(view, min, max)
        } else {
            aabb_visible(&view.planes, Vec3::from(min), Vec3::from(max))
        }
    }

    /// The section of block `block` (world coordinates) is compiled and has appeared to 0.3 of its
    /// fade, or `block` is above or below the world: the second half of the game's entity test,
    /// and what it asks of a block entity's section (`compiled_sections.rs`).
    #[inline]
    pub fn section_shown(&self, block: [i32; 3]) -> bool {
        let (Some(view), Some(compiled)) = (&self.view, &self.compiled) else { return true };
        let sy = block[1].wrapping_sub(view.min_y) >> 4;
        if !(0..view.sections).contains(&sy) {
            return true;
        }
        compiled.sections.shown((block[0] >> 4, block[2] >> 4), sy, compiled.now, compiled.fade_secs)
    }

    /// The game's entity test, shared by the entity renderer, the leashes and the debug boxes drawn
    /// with the entities: the box `min..max` in the view frustum (or an entity the game draws
    /// outside it, `drawn_outside_the_field`), then the section of the entity's block `block`
    /// shown ([`Self::section_shown`]).
    #[inline]
    pub fn entity_shown(&self, drawn_outside_the_field: bool, min: [f32; 3], max: [f32; 3], block: [i32; 3]) -> bool {
        (drawn_outside_the_field || self.frustum_box(min, max)) && self.section_shown(block)
    }

    /// The box `min..max` (frame relative to the camera's integer origin) is visible.
    #[inline]
    pub fn aabb(&self, min: [f32; 3], max: [f32; 3]) -> bool {
        let Some(view) = &self.view else { return true };
        self.counted(self.box_verdict(view, min, max))
    }

    /// The point `p` (same frame) is visible: the box test of `p..p`.
    #[inline]
    pub fn point(&self, p: [f32; 3]) -> bool {
        let Some(view) = &self.view else { return true };
        let verdict = if self.paths.points { self.point_verdict(view, p) } else { self.box_verdict(view, p, p) };
        self.counted(verdict)
    }

    #[inline]
    fn counted(&self, verdict: BoxVerdict) -> bool {
        if let Some(t) = &self.tally {
            t.add(verdict);
        }
        verdict.visible()
    }

    /// Verdict of a box on the path the switches chose.
    pub(super) fn box_verdict(&self, view: &FrameView, min: [f32; 3], max: [f32; 3]) -> BoxVerdict {
        if !self.paths.direct_grid {
            return view.verdict(min, max);
        }
        let in_frustum = if self.paths.plane_masks {
            self.box_reaches_planes(view, min, max)
        } else {
            aabb_visible(&view.planes, Vec3::from(min), Vec3::from(max))
        };
        if !in_frustum {
            return BoxVerdict::Outside;
        }
        self.occlusion_verdict(view, min, max)
    }

    /// The box reaches the inner side of every plane, each testing the corner chosen for it once
    /// per frame (same corner, same arithmetic as `aabb_visible`).
    #[inline]
    fn box_reaches_planes(&self, view: &FrameView, min: [f32; 3], max: [f32; 3]) -> bool {
        let ends = [[min[0], max[0]], [min[1], max[1]], [min[2], max[2]]];
        view.planes.iter().zip(&self.corner_sides).all(|(p, side)| {
            // `& 1` lets the compiler drop the bounds checks (a side is 0 or 1).
            let corner = Vec3::new(ends[0][side[0] & 1], ends[1][side[1] & 1], ends[2][side[2] & 1]);
            p.truncate().dot(corner) + p.w >= 0.0
        })
    }

    /// Verdict of a point: in the frustum by six dot products (the corner of a point box is the
    /// point itself), then one bit when the point and its margin stay in one section.
    pub(super) fn point_verdict(&self, view: &FrameView, p: [f32; 3]) -> BoxVerdict {
        let at = Vec3::from(p);
        if !view.planes.iter().all(|plane| plane.truncate().dot(at) + plane.w >= 0.0) {
            return BoxVerdict::Outside;
        }
        if !view.occlusion || view.reached.is_empty() {
            return BoxVerdict::Unchecked;
        }
        let Some([x, y, z]) = self.section_holding_margin(view, p) else {
            return self.occlusion_verdict(view, p, p);
        };
        if y < 0 || y >= view.sections {
            return BoxVerdict::Unchecked;
        }
        verdict_of(true, view.reached.contains(x, y, z))
    }

    /// Section of point `p` when the point and its one-block margin lie in it: on each axis the
    /// point's block is 1 to 13 blocks above the bottom of its section. The box path rounds `p - 1`
    /// and `p + 1` to floats first; with an exact floor (|v| < 2^23) `p - 1` never drops below the
    /// block under `p`, and `p + 1` may round up to the block after next, never further, so the
    /// band keeps both in the section. `None` elsewhere, and for NaN, infinities and huge values.
    fn section_holding_margin(&self, view: &FrameView, p: [f32; 3]) -> Option<[i32; 3]> {
        #[cfg(test)]
        let band = if self.paths.sabotaged(VisibilitySabotage::EdgePointInOneSection) { 0..=15 } else { 1..=13 };
        #[cfg(not(test))]
        let band = 1..=13;
        let offsets = [view.origin[0], -view.min_y, view.origin[1]];
        let mut section = [0; 3];
        for axis in 0..3 {
            let v = p[axis];
            if v.is_nan() || v.abs() >= 8_388_608.0 {
                return None;
            }
            let block = floor_f32_to_i32(v).checked_add(offsets[axis])?;
            if !band.contains(&block.rem_euclid(16)) {
                return None;
            }
            section[axis] = block.div_euclid(16);
        }
        Some(section)
    }

    /// Occlusion verdict of a box already in the frustum (`visibility.md`): visible if one of the
    /// sections touched by the box plus one block was reached; outside the world in height, kept.
    fn occlusion_verdict(&self, view: &FrameView, min: [f32; 3], max: [f32; 3]) -> BoxVerdict {
        if !view.occlusion || view.reached.is_empty() {
            return BoxVerdict::Unchecked;
        }
        let span = |low: f32, high: f32, offset: i32| [section_of(low - 1.0, offset), section_of(high + 1.0, offset)];
        let xs = span(min[0], max[0], view.origin[0]);
        let zs = span(min[2], max[2], view.origin[1]);
        let ys = span(min[1], max[1], -view.min_y);
        if ys[0] < 0 || ys[1] >= view.sections {
            return BoxVerdict::Unchecked;
        }
        let one_section = xs[0] == xs[1] && ys[0] == ys[1] && zs[0] == zs[1];
        #[cfg(test)]
        if self.paths.sabotaged(VisibilitySabotage::NeighboursIgnored) {
            let (x, y, z) = (
                section_of(min[0], view.origin[0]),
                section_of(min[1], -view.min_y),
                section_of(min[2], view.origin[1]),
            );
            return verdict_of(one_section, view.reached.contains(x, y, z));
        }
        let reached = match &*view.reached {
            Reach::Grid(grid) => grid.any_in_box(xs, ys, zs, self.paths.y_runs),
            Reach::Set(set) => {
                (xs[0]..=xs[1]).any(|x| (zs[0]..=zs[1]).any(|z| (ys[0]..=ys[1]).any(|y| set.contains(&(x, y, z)))))
            }
        };
        verdict_of(one_section, reached)
    }
}

/// Section index along one axis of coordinate `v` shifted by the integer `offset`: the old
/// `(v.floor() as i32 + offset).div_euclid(16)`, its floor without the C library call (the
/// build has no SSE4.1 rounding instruction) and its sum wrapping as the release build did.
#[inline]
fn section_of(v: f32, offset: i32) -> i32 {
    floor_f32_to_i32(v).wrapping_add(offset).div_euclid(16)
}

fn verdict_of(one_section: bool, reached: bool) -> BoxVerdict {
    match (one_section, reached) {
        (true, true) => BoxVerdict::OneSectionReached,
        (true, false) => BoxVerdict::OneSectionHidden,
        (false, true) => BoxVerdict::StraddlingReached,
        (false, false) => BoxVerdict::StraddlingHidden,
    }
}

#[cfg(test)]
#[path = "frame_visibility_test.rs"]
mod tests;
