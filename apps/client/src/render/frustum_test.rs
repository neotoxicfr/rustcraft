//! The frame's visibility test with verdicts (perf log classes) answers like the test of before,
//! on random cameras, walks and boxes, with the reached sections as a set or as a grid.

use super::*;
use crate::render::lists::{Reach, ReachGrid};

/// The visibility test as written before the verdicts (reference, kept as is).
#[allow(clippy::too_many_arguments)]
fn visible_before(
    planes: &[Vec4; 6],
    origin: [i32; 2],
    min_y: i32,
    sections: i32,
    reached: &Reach,
    occlusion: bool,
    min: [f32; 3],
    max: [f32; 3],
) -> bool {
    if !aabb_visible(planes, Vec3::from(min), Vec3::from(max)) {
        return false;
    }
    if !occlusion || reached.is_empty() {
        return true;
    }
    let cell = |v: f32, o: i32| (v.floor() as i32 + o).div_euclid(16);
    let (x0, x1) = (cell(min[0] - 1.0, origin[0]), cell(max[0] + 1.0, origin[0]));
    let (z0, z1) = (cell(min[2] - 1.0, origin[1]), cell(max[2] + 1.0, origin[1]));
    let (y0, y1) = (cell(min[1] - 1.0, -min_y), cell(max[1] + 1.0, -min_y));
    if y0 < 0 || y1 >= sections {
        return true;
    }
    (x0..=x1).any(|x| (z0..=z1).any(|z| (y0..=y1).any(|y| reached.contains(x, y, z))))
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    fn unit(&mut self) -> f32 {
        (self.next() % 10_000) as f32 / 10_000.0
    }
    fn below(&mut self, n: u64) -> i32 {
        (self.next() % n) as i32
    }
}

#[test]
fn verdicts_answer_like_before() {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut seen = [0u32; BoxVerdict::ALL.len()];
    for round in 0..400 {
        let eye =
            [f64::from(rng.below(200)) - 100.5, f64::from(rng.below(120)) - 30.25, f64::from(rng.below(200)) - 100.75];
        let (yaw, pitch) = (rng.unit() * 360.0, rng.unit() * 180.0 - 90.0);
        let (origin, _, view_m) = crate::camera::view_matrix(eye, yaw, pitch);
        let planes = frustum_planes(perspective(70.0, 16.0 / 9.0, 0.05, 160.0) * view_m);
        let (min_y, sections) = (-64, 24);
        // Reached sections around the camera (one in three), sometimes none.
        let (cx, cz) = ((eye[0] / 16.0).floor() as i32, (eye[2] / 16.0).floor() as i32);
        let mut list = Vec::new();
        if round % 7 != 0 {
            for x in cx - 4..=cx + 4 {
                for z in cz - 4..=cz + 4 {
                    for y in 0..sections {
                        if rng.below(3) == 0 {
                            list.push((x, y, z));
                        }
                    }
                }
            }
        }
        let reached = if round % 2 == 0 {
            Reach::Grid(ReachGrid::new(&list))
        } else {
            Reach::Set(list.iter().copied().collect())
        };
        let reached = std::sync::Arc::new(reached);
        let occlusion = round % 5 != 0;
        let view = FrameView { planes, origin, min_y, sections, reached: reached.clone(), occlusion };
        for _ in 0..250 {
            // Boxes relative to the integer origin: small, a few blocks, or long (several sections).
            let size = match rng.below(3) {
                0 => 0.25,
                1 => 3.0,
                _ => 40.0,
            };
            let at = [rng.unit() * 120.0 - 60.0, rng.unit() * 160.0 - 80.0, rng.unit() * 120.0 - 60.0];
            let min = at;
            let max = [at[0] + rng.unit() * size, at[1] + rng.unit() * size, at[2] + rng.unit() * size];
            let before = visible_before(&planes, origin, min_y, sections, &reached, occlusion, min, max);
            let verdict = view.verdict(min, max);
            assert_eq!(verdict.visible(), before, "{verdict:?} for {min:?}..{max:?}, round {round}");
            seen[verdict as usize] += 1;
        }
    }
    for (v, n) in BoxVerdict::ALL.iter().zip(seen) {
        assert!(n > 0, "verdict {v:?} never drawn: the test does not cover it");
    }
}

/// One verdict per class, on a hand-made case: camera at the origin looking along −z (yaw 0
/// faces +z in the game; the box positions are taken in front whatever the convention).
#[test]
fn verdict_classes_follow_the_sections() {
    let (origin, _, view_m) = crate::camera::view_matrix([0.5, 0.5, 0.5], 0.0, 0.0);
    let planes = frustum_planes(perspective(70.0, 1.0, 0.05, 500.0) * view_m);
    let front = |z: f32| {
        // The side of the camera that the frustum contains.
        let probe = [-0.1, -0.1, z];
        if aabb_visible(&planes, Vec3::from(probe), Vec3::new(0.1, 0.1, z + 0.2)) { z } else { -z - 0.2 }
    };
    let z = front(40.0);
    let section = |v: f32| (v.floor() as i32).div_euclid(16);
    let reached = std::sync::Arc::new(Reach::Set([(0, 4, section(z))].into_iter().collect()));
    let view = FrameView { planes, origin, min_y: -64, sections: 24, reached: reached.clone(), occlusion: true };
    let no_occlusion = FrameView { planes, origin, min_y: -64, sections: 24, reached, occlusion: false };
    // y 0.5 is section 4 above −64; a box of 0.2 in the middle of its section stays in it.
    let mid = section(z) as f32 * 16.0 + 8.0;
    assert_eq!(view.verdict([2.0, 2.0, mid], [2.2, 2.2, mid + 0.2]), BoxVerdict::OneSectionReached);
    let other = mid + 16.0 * if z > 0.0 { 1.0 } else { -1.0 };
    assert_eq!(view.verdict([2.0, 2.0, other], [2.2, 2.2, other + 0.2]), BoxVerdict::OneSectionHidden);
    // Touching the next section through the +1 block margin: straddling.
    let edge = section(z) as f32 * 16.0 + 15.5;
    assert_eq!(view.verdict([2.0, 2.0, edge], [2.2, 2.2, edge + 0.2]), BoxVerdict::StraddlingReached);
    let behind = -z;
    assert_eq!(view.verdict([0.0, 0.0, behind], [0.2, 0.2, behind + 0.2]), BoxVerdict::Outside);
    assert_eq!(no_occlusion.verdict([2.0, 2.0, other], [2.2, 2.2, other + 0.2]), BoxVerdict::Unchecked);
}
