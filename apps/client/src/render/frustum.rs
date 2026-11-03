//! Field-of-view tests: the planes of the view frustum, box tests against them, the ranges of sections
//! where the faces of a column can be seen from the front, and the visibility query handed to the
//! entity systems (frustum plus the sections reached by the occlusion walk).

use glam::{Mat4, Vec3, Vec4};

use super::{Renderer, perspective};
use rc_mesh::blocks::BUCKETS;

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
    planes.iter().all(|p| {
        // Most advanced corner in the direction of the normal.
        let v = Vec3::new(
            if p.x >= 0.0 { max.x } else { min.x },
            if p.y >= 0.0 { max.y } else { min.y },
            if p.z >= 0.0 { max.z } else { min.z },
        );
        p.truncate().dot(v) + p.w >= 0.0
    })
}

/// Box entirely in the field (its least advanced corner is on the right side of each plane):
/// any box it contains is too.
pub(super) fn aabb_inside(planes: &[Vec4; 6], min: Vec3, max: Vec3) -> bool {
    planes.iter().all(|p| {
        let v = Vec3::new(
            if p.x >= 0.0 { min.x } else { max.x },
            if p.y >= 0.0 { min.y } else { max.y },
            if p.z >= 0.0 { min.z } else { max.z },
        );
        p.truncate().dot(v) + p.w >= 0.0
    })
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

    /// Appearance fade (0 to 1) of the already meshed column `(x, z)`, `None` before.
    pub fn column_visibility(&self, x: i32, z: i32) -> Option<f32> {
        self.fade_ring.visibility((x, z), self.fade_secs)
    }
}

#[cfg(test)]
#[path = "frustum_test.rs"]
mod tests;
