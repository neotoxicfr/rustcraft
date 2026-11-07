//! Field-of-view tests: the planes of the view frustum, box tests against them, the ranges of sections
//! where the faces of a column can be seen from the front, and the visibility query handed to the
//! entity systems (frustum plus the sections reached by the occlusion walk).

use glam::{Mat4, Vec3, Vec4};

use super::{Renderer, perspective};
use rc_mesh::blocks::BUCKETS;
use rc_mesh::wire::Sections;

/// For each group of faces (order `Dir`, then unaligned), range of sections of the
/// column where it can be seen from the front. A face oriented +x is only visible if the camera is
/// beyond its plane; the planes of a section stay within `MARGIN` blocks of its box (model
/// elements that overflow the block), hence a cautious test that only removes back faces.
pub(super) fn facing(eye: [f64; 3], column: (i32, i32), min_y: i32) -> [(usize, usize); BUCKETS] {
    const MARGIN: f64 = 2.0;
    let all = (0, usize::MAX);
    let none = (0, 0);
    let (x0, z0) = (f64::from(column.0 * 16), f64::from(column.1 * 16));
    let y = eye[1] - f64::from(min_y);
    // Top (+y): sections whose bottom − margin is below the eye; bottom (−y): those whose
    // top + margin is above.
    let up_end = ((y + MARGIN) / 16.0).ceil().max(0.0) as usize;
    let down_start = ((y - MARGIN - 16.0) / 16.0).floor().max(-1.0) as i64 + 1;
    let side = |ok: bool| if ok { all } else { none };
    [
        (down_start as usize, usize::MAX),
        (0, up_end),
        side(eye[2] < z0 + 16.0 + MARGIN),
        side(eye[2] > z0 - MARGIN),
        side(eye[0] < x0 + 16.0 + MARGIN),
        side(eye[0] > x0 - MARGIN),
        all,
    ]
}

/// Planes of the view frustum (camera frame), normals pointing inwards.
pub(super) fn frustum_planes(m: Mat4) -> [Vec4; 6] {
    let r = [m.row(0), m.row(1), m.row(2), m.row(3)];
    // Reversed [0, 1] depth: far plane = row 2 alone, near plane = row 3 − row 2.
    [r[3] + r[0], r[3] - r[0], r[3] + r[1], r[3] - r[1], r[2], r[3] - r[2]]
}

pub(super) fn aabb_visible(planes: &[Vec4; 6], min: Vec3, max: Vec3) -> bool {
    planes.iter().all(|p| box_reaches_plane(p, min, max))
}

/// Part of the box is on the inner side of plane `p`: its most advanced corner in the direction
/// of the normal is.
#[inline]
pub(super) fn box_reaches_plane(p: &Vec4, min: Vec3, max: Vec3) -> bool {
    let v = Vec3::new(
        if p.x >= 0.0 { max.x } else { min.x },
        if p.y >= 0.0 { max.y } else { min.y },
        if p.z >= 0.0 { max.z } else { min.z },
    );
    p.truncate().dot(v) + p.w >= 0.0
}

/// Box entirely in the field (its least advanced corner is on the right side of each plane):
/// any box it contains is too.
pub(super) fn aabb_inside(planes: &[Vec4; 6], min: Vec3, max: Vec3) -> bool {
    planes.iter().all(|p| box_inside_plane(p, min, max))
}

/// The whole box is on the inner side of plane `p`: its least advanced corner is.
#[inline]
pub(super) fn box_inside_plane(p: &Vec4, min: Vec3, max: Vec3) -> bool {
    let v = Vec3::new(
        if p.x >= 0.0 { min.x } else { max.x },
        if p.y >= 0.0 { min.y } else { max.y },
        if p.z >= 0.0 { min.z } else { max.z },
    );
    p.truncate().dot(v) + p.w >= 0.0
}

/// Box of section `sy` of column `(x, z)` in the frame relative to the camera's integer `origin`,
/// with one block of margin (a model can overflow its block).
pub(super) fn section_box(origin: [i32; 2], min_y: i32, (x, z): (i32, i32), sy: i32) -> (Vec3, Vec3) {
    let min = Vec3::new((x * 16 - origin[0]) as f32, (min_y + sy * 16) as f32, (z * 16 - origin[1]) as f32) - 1.0;
    (min, min + Vec3::splat(18.0))
}

/// Box of the whole column `(x, z)` (`sections` high), same frame and margin as [`section_box`].
pub(super) fn column_box(origin: [i32; 2], min_y: i32, sections: i32, (x, z): (i32, i32)) -> (Vec3, Vec3) {
    let min = Vec3::new((x * 16 - origin[0]) as f32, min_y as f32, (z * 16 - origin[1]) as f32) - 1.0;
    (min, min + Vec3::new(18.0, (sections * 16) as f32 + 2.0, 18.0))
}

/// Planes moved back along the look direction `forward` (unit), in steps of 4 blocks, until the
/// 8-block cell holding the eye lies wholly inside them. The game picks the sections to rebuild in
/// such a frustum, so the sections just behind and beside the camera are rebuilt before a turn
/// shows them. Frame relative to the camera's integer `origin` (x, z), like the planes.
pub(super) fn planes_holding_camera_cell(
    planes: [Vec4; 6],
    origin: [i32; 2],
    eye: [f64; 3],
    forward: Vec3,
) -> [Vec4; 6] {
    const CELL: f64 = 8.0;
    const STEP: f32 = 4.0;
    // Bound for a degenerate view (NaN planes never hold the cell); a 7-degree zoom needs ~30 steps.
    const MAX_STEPS: u32 = 256;
    let offset = [f64::from(origin[0]), 0.0, f64::from(origin[1])];
    let corner =
        |round: fn(f64) -> f64| Vec3::from(std::array::from_fn(|i| (round(eye[i] / CELL) * CELL - offset[i]) as f32));
    let (low, high) = (corner(f64::floor), corner(f64::ceil));
    let mut moved = planes;
    let mut back = 0.0;
    for _ in 0..MAX_STEPS {
        if aabb_inside(&moved, low, high) {
            break;
        }
        back += STEP;
        // Eye moved by −back·forward: each plane tests p + back·forward with the old planes.
        moved = planes.map(|p| p + Vec4::new(0.0, 0.0, 0.0, p.truncate().dot(forward * back)));
    }
    moved
}

/// What the meshing treats as in view, from the last frame drawn: the draw's frustum moved back
/// to hold the camera's cell ([`planes_holding_camera_cell`]), camera integer origin (x, z), bottom
/// of the world, number of sections.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct MeshingView {
    pub planes: [Vec4; 6],
    pub origin: [i32; 2],
    pub min_y: i32,
    pub sections: i32,
}

impl MeshingView {
    /// View of a frame whose draw frustum is `drawn_planes`, eye `eye` looking along `forward`.
    pub fn new(
        drawn_planes: [Vec4; 6],
        origin: [i32; 2],
        eye: [f64; 3],
        forward: Vec3,
        min_y: i32,
        sections: i32,
    ) -> Self {
        let planes = planes_holding_camera_cell(drawn_planes, origin, eye, forward);
        MeshingView { planes, origin, min_y, sections }
    }

    /// Sections among `reached` (the reached sections of column `key`) whose box lies in the
    /// view: the column test of the draw lists, as a set, on the moved planes.
    pub fn sections_in_view(&self, key: (i32, i32), reached: &[(i32, i32, i32)]) -> Sections {
        self.sections_in_view_with(key, reached, super::lists::ListPaths::from_switches())
    }

    /// [`Self::sections_in_view`] with the culling paths `paths` (the tests compare them).
    pub(super) fn sections_in_view_with(
        &self,
        key: (i32, i32),
        reached: &[(i32, i32, i32)],
        paths: super::lists::ListPaths,
    ) -> Sections {
        let world = (self.min_y, self.sections);
        super::lists::sections_in_field(&self.planes, self.origin, world, key, reached, paths)
    }
}

/// Verdict of the frame's visibility test for one box; the perf log counts the calls per verdict
/// (share a per-section verdict cache could answer: boxes within one section).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BoxVerdict {
    /// Outside the view frustum.
    Outside,
    /// In the frustum, occlusion not consulted (off, no walk yet, box above or below the world).
    Unchecked,
    /// In the frustum, box (+ 1 block) within one section, reached by the walk or not.
    OneSectionReached,
    OneSectionHidden,
    /// In the frustum, box (+ 1 block) across several sections, one reached or none.
    StraddlingReached,
    StraddlingHidden,
}

impl BoxVerdict {
    pub const ALL: [BoxVerdict; 6] = [
        BoxVerdict::Outside,
        BoxVerdict::Unchecked,
        BoxVerdict::OneSectionReached,
        BoxVerdict::OneSectionHidden,
        BoxVerdict::StraddlingReached,
        BoxVerdict::StraddlingHidden,
    ];

    pub fn visible(self) -> bool {
        !matches!(self, BoxVerdict::Outside | BoxVerdict::OneSectionHidden | BoxVerdict::StraddlingHidden)
    }

    pub fn name(self) -> &'static str {
        match self {
            BoxVerdict::Outside => "outside",
            BoxVerdict::Unchecked => "unchecked",
            BoxVerdict::OneSectionReached => "one_section_reached",
            BoxVerdict::OneSectionHidden => "one_section_hidden",
            BoxVerdict::StraddlingReached => "straddling_reached",
            BoxVerdict::StraddlingHidden => "straddling_hidden",
        }
    }
}

/// What the frame's visibility test reads: frustum planes, camera integer origin (x, z), world
/// bottom and height in sections, sections reached by the walk.
struct FrameView {
    planes: [Vec4; 6],
    origin: [i32; 2],
    min_y: i32,
    sections: i32,
    reached: std::sync::Arc<super::lists::Reach>,
    occlusion: bool,
}

impl FrameView {
    /// Verdict of the box `min..max` (frame relative to the camera's integer origin).
    fn verdict(&self, min: [f32; 3], max: [f32; 3]) -> BoxVerdict {
        if !aabb_visible(&self.planes, Vec3::from(min), Vec3::from(max)) {
            return BoxVerdict::Outside;
        }
        if !self.occlusion || self.reached.is_empty() {
            return BoxVerdict::Unchecked;
        }
        // Occlusion walk (`visibility.md`): visible if one of the sections touched by the
        // box (+ 1 block) was reached; outside the world in height: kept.
        let cell = |v: f32, o: i32| (v.floor() as i32 + o).div_euclid(16);
        let (x0, x1) = (cell(min[0] - 1.0, self.origin[0]), cell(max[0] + 1.0, self.origin[0]));
        let (z0, z1) = (cell(min[2] - 1.0, self.origin[1]), cell(max[2] + 1.0, self.origin[1]));
        let (y0, y1) = (cell(min[1] - 1.0, -self.min_y), cell(max[1] + 1.0, -self.min_y));
        if y0 < 0 || y1 >= self.sections {
            return BoxVerdict::Unchecked;
        }
        let reached = (x0..=x1).any(|x| (z0..=z1).any(|z| (y0..=y1).any(|y| self.reached.contains(x, y, z))));
        match (x0 == x1 && y0 == y1 && z0 == z1, reached) {
            (true, true) => BoxVerdict::OneSectionReached,
            (true, false) => BoxVerdict::OneSectionHidden,
            (false, true) => BoxVerdict::StraddlingReached,
            (false, false) => BoxVerdict::StraddlingHidden,
        }
    }
}

/// Calls of one frame's visibility test per verdict, handed to the perf log when the test is
/// dropped (plain cells: the test runs on the main thread, thousands of times per frame).
#[derive(Default)]
struct VerdictTally([std::cell::Cell<u32>; BoxVerdict::ALL.len()]);

impl VerdictTally {
    fn add(&self, v: BoxVerdict) {
        let c = &self.0[v as usize];
        c.set(c.get() + 1);
    }
}

impl Drop for VerdictTally {
    fn drop(&mut self) {
        crate::perf_log::visibility_calls(&self.0.each_ref().map(std::cell::Cell::get));
    }
}

impl Renderer {
    /// "Box in the field of view" test of the frame (frame relative to the camera's integer origin),
    /// same projection as the render.
    pub fn visibility(
        &self,
        eye: [f64; 3],
        yaw: f32,
        pitch: f32,
        far: f32,
    ) -> impl Fn([f32; 3], [f32; 3]) -> bool + use<> {
        let (origin, _, view_m) = crate::camera::view_matrix(eye, yaw, pitch);
        let aspect = self.config.width as f32 / self.config.height.max(1) as f32;
        let proj = perspective(self.view.cull_fov, aspect, 0.05, far);
        let (_, min_y, sections, reached) = self.reached_set.clone();
        let view = FrameView {
            planes: frustum_planes(proj * view_m),
            origin,
            min_y,
            sections,
            reached,
            occlusion: self.occlusion && !self.all_entities,
        };
        let tally = crate::perf_log::on().then(VerdictTally::default);
        move |min, max| {
            let verdict = view.verdict(min, max);
            if let Some(t) = &tally {
                t.add(verdict);
            }
            verdict.visible()
        }
    }

    /// Perf log: the last visibility walk reached a section of column `(x, z)`; `None` before
    /// the first walk.
    pub fn column_reached(&self, x: i32, z: i32) -> Option<bool> {
        if self.reached.is_empty() {
            return None;
        }
        // Sorted by column then height (`frame.rs`).
        let i = self.reached.partition_point(|&(rx, _, rz)| (rx, rz) < (x, z));
        Some(self.reached.get(i).is_some_and(|&(rx, _, rz)| (rx, rz) == (x, z)))
    }

    /// Sections of column `(x, z)` the meshing treats as seen: reached by the last walk and in the
    /// meshing view of the last frame drawn ([`MeshingView`]). `None` when that cannot be told: no
    /// frame or no walk yet, or no mesh of the column held here (the walk only enters the columns
    /// it holds, so it would never reach a column left unmeshed).
    pub fn sections_in_view(&self, x: i32, z: i32) -> Option<Sections> {
        let view = self.meshing_view.as_ref()?;
        let held = self.columns.contains_key(&(x, z)) || self.empty_vis.contains_key(&(x, z));
        if !held || self.reached.is_empty() {
            return None;
        }
        // Sorted by column then height (`frame.rs`): the column's sections are one run.
        let lo = self.reached.partition_point(|&(rx, _, rz)| (rx, rz) < (x, z));
        let len = self.reached[lo..].partition_point(|&(rx, _, rz)| (rx, rz) == (x, z));
        Some(view.sections_in_view((x, z), &self.reached[lo..lo + len]))
    }

    /// Everything [`Self::sections_in_view`] reads, as revisions: the meshing view, the walk's
    /// reached sections and the columns held. The same stamp gives the same verdicts.
    pub fn meshing_view_stamp(&self) -> [u64; 4] {
        [self.meshing_view_rev, self.reached_gen, self.vis_rev, self.columns_rev]
    }

    /// Appearance fade (0 to 1) of the already meshed column `(x, z)`, `None` before.
    pub fn column_visibility(&self, x: i32, z: i32) -> Option<f32> {
        self.fade_ring.visibility((x, z), self.fade_secs)
    }
}

#[cfg(test)]
#[path = "frustum_test.rs"]
mod tests;
