//! Field-of-view tests: the planes of the view frustum, box tests against them, the ranges of sections
//! where the faces of a column can be seen from the front, and the visibility query handed to the
//! entity systems (frustum plus the sections reached by the occlusion walk).

use glam::{Mat4, Vec3, Vec4};

use super::{Renderer, perspective};
use rc_mesh::blocks::BUCKETS;

/// For each group of faces (order `Dir`, then unaligned), range of sections of the
/// column where it can be seen from the front. A face oriented +x is only visible if the camera is
/// beyond its plane; the planes of a section stay within `MARGE` blocks of its box (model
/// elements that overflow the block), hence a cautious test that only removes back faces.
pub(super) fn facing(eye: [f64; 3], column: (i32, i32), min_y: i32) -> [(usize, usize); BUCKETS] {
    const MARGE: f64 = 2.0;
    let all = (0, usize::MAX);
    let none = (0, 0);
    let (x0, z0) = (f64::from(column.0 * 16), f64::from(column.1 * 16));
    let y = eye[1] - f64::from(min_y);
    // Top (+y): sections whose bottom − margin is below the eye; bottom (−y): those whose
    // top + margin is above.
    let up_end = ((y + MARGE) / 16.0).ceil().max(0.0) as usize;
    let down_start = ((y - MARGE - 16.0) / 16.0).floor().max(-1.0) as i64 + 1;
    let side = |ok: bool| if ok { all } else { none };
    [
        (down_start as usize, usize::MAX),
        (0, up_end),
        side(eye[2] < z0 + 16.0 + MARGE),
        side(eye[2] > z0 - MARGE),
        side(eye[0] < x0 + 16.0 + MARGE),
        side(eye[0] > x0 - MARGE),
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
        let planes = frustum_planes(proj * view_m);
        let (_, min_y, sections, reached) = self.reached_set.clone();
        let occlusion = self.occlusion && !self.all_entities;
        move |min, max| {
            if !aabb_visible(&planes, Vec3::from(min), Vec3::from(max)) {
                return false;
            }
            if !occlusion || reached.is_empty() {
                return true;
            }
            // Occlusion walk (`visibility.md`): visible if one of the sections touched by the
            // box (+ 1 block) was reached; outside the world in height: kept.
            let cell = |v: f32, o: i32| (v.floor() as i32 + o).div_euclid(16);
            let (x0, x1) = (cell(min[0] - 1.0, origin[0]), cell(max[0] + 1.0, origin[0]));
            let (z0, z1) = (cell(min[2] - 1.0, origin[1]), cell(max[2] + 1.0, origin[1]));
            let (y0, y1) = (cell(min[1] - 1.0, -min_y), cell(max[1] + 1.0, -min_y));
            if y0 < 0 || y1 >= sections {
                return true;
            }
            (x0..=x1).any(|x| (z0..=z1).any(|z| (y0..=y1).any(|y| reached.contains(x, y, z))))
        }
    }

    /// Appearance fade (0 to 1) of the already meshed column `(x, z)`, `None` before.
    pub fn column_visibility(&self, x: i32, z: i32) -> Option<f32> {
        self.fade_ring.visibility((x, z), self.fade_secs)
    }
}
