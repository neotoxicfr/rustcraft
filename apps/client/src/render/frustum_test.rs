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

/// The game's particle test rebuilt as the game computes it (reference): culling matrix in floats
/// (projection with depth in [0, 1], times the camera rotation), six planes taken from its rows as
/// the game's frustum library does (near = w + z, a rule written for depth in [−1, 1]), normalised;
/// eye moved back by three view vectors (the depth row of that matrix) in doubles; point taken
/// relative to it in doubles, then cast to a float.
struct GameParticleTest {
    planes: [Vec4; 6],
    eye: [f64; 3],
}

impl GameParticleTest {
    fn new(eye: [f64; 3], yaw: f32, pitch: f32, (fov, aspect, far): (f32, f32, f32)) -> Self {
        let near = 0.05f32;
        let h = (fov.to_radians() * 0.5).tan();
        let projection = Mat4::from_cols(
            Vec4::new(1.0 / (h * aspect), 0.0, 0.0, 0.0),
            Vec4::new(0.0, 1.0 / h, 0.0, 0.0),
            Vec4::new(0.0, 0.0, far / (near - far), -1.0),
            Vec4::new(0.0, 0.0, far * near / (near - far), 0.0),
        );
        let m = projection * crate::camera::view_rotation(yaw, pitch);
        let r = [m.row(0), m.row(1), m.row(2), m.row(3)];
        let planes = [r[3] + r[0], r[3] - r[0], r[3] + r[1], r[3] - r[1], r[3] + r[2], r[3] - r[2]]
            .map(|p| p / p.truncate().length());
        let view_vector = m.row(2);
        let eye = std::array::from_fn(|i| eye[i] + f64::from(view_vector[i] * -3.0));
        GameParticleTest { planes, eye }
    }

    /// The game's answer at `p`, and whether `p` is too close to a plane for float rounding (or to
    /// the game's near plane, which its [−1, 1] rule puts at half the distance of ours: three
    /// blocks behind the camera, never on screen) to settle it the same way in both frames.
    fn test(&self, p: [f64; 3]) -> (bool, bool) {
        let q = Vec3::from(std::array::from_fn(|i| (p[i] - self.eye[i]) as f32));
        let d = self.planes.map(|plane| plane.truncate().dot(q) + plane.w);
        let close = d.iter().any(|v| v.abs() < 1e-3) || d[4].abs() < 0.06;
        (d.iter().all(|&v| v >= 0.0), close)
    }
}

/// Point relative to the integer origin of `eye`, as the particles hand it over.
fn relative(eye: [f64; 3], p: [f64; 3]) -> [f32; 3] {
    [(p[0] - eye[0].floor()) as f32, p[1] as f32, (p[2] - eye[2].floor()) as f32]
}

/// Particles drawn by `drawn` that the game does not draw or the reverse, over random cameras and
/// points (around the camera, out of the world in height, behind it), the close calls left out.
fn particle_differences(drawn: impl Fn([f64; 3], f32, f32, [f32; 3]) -> bool) -> u32 {
    let projection = (70.0, 16.0 / 9.0, 512.0);
    let mut rng = Rng(0x5851_f42d_4c95_7f2d);
    let mut different = 0;
    for _ in 0..400 {
        let eye = [
            f64::from(rng.below(2000)) - 1000.37,
            f64::from(rng.below(400)) - 70.6,
            f64::from(rng.below(2000)) - 999.81,
        ];
        let (yaw, pitch) = (rng.unit() * 360.0 - 180.0, rng.unit() * 180.0 - 90.0);
        let game = GameParticleTest::new(eye, yaw, pitch, projection);
        for _ in 0..250 {
            let reach = [4.0, 20.0, 80.0][rng.below(3) as usize];
            let p: [f64; 3] = std::array::from_fn(|i| eye[i] + f64::from(rng.unit() * 2.0 - 1.0) * reach);
            let (want, close) = game.test(p);
            if !close {
                different += u32::from(drawn(eye, yaw, pitch, relative(eye, p)) != want);
            }
        }
    }
    different
}

#[test]
fn particles_are_culled_like_the_game() {
    let projection = (70.0, 16.0 / 9.0, 512.0);
    for simd in [false, true] {
        let ours = |eye, yaw, pitch, p| particle_frustum_of(eye, yaw, pitch, projection, simd).contains_point(p);
        assert_eq!(particle_differences(ours), 0, "simd {simd}");
    }
    // Sabotages: the eye left in place; the entities' test (occlusion) kept for the particles.
    let not_moved_back = |eye, yaw, pitch, p| {
        let (_, _, view_m) = crate::camera::view_matrix(eye, yaw, pitch);
        let planes = frustum_planes(perspective(projection.0, projection.1, 0.05, projection.2) * view_m);
        ParticleFrustum::new(planes, Vec3::ZERO, false).contains_point(p)
    };
    assert!(particle_differences(not_moved_back) > 0, "eye not moved back: not caught");
    let with_occlusion = |eye: [f64; 3], yaw, pitch, p| {
        let (origin, _, view_m) = crate::camera::view_matrix(eye, yaw, pitch);
        let planes = frustum_planes(perspective(projection.0, projection.1, 0.05, projection.2) * view_m);
        let planes = ParticleFrustum::new(planes, game_view_vector(yaw, pitch, 0.05, projection.2) * 3.0, false).planes;
        // Only the camera's own section reached.
        let own = ((eye[0].floor() as i32) >> 4, (eye[1].floor() as i32 + 64) >> 4, (eye[2].floor() as i32) >> 4);
        let reached = std::sync::Arc::new(Reach::Set([own].into_iter().collect()));
        FrameView { planes, origin, min_y: -64, sections: 24, reached, occlusion: true }.verdict(p, p).visible()
    };
    assert!(particle_differences(with_occlusion) > 0, "occlusion kept: not caught");
}

/// The cases the game's particle test decides differently from the entities' test (A3): a centre
/// just off screen, in a section the walk did not reach, out of the world in height; and those it
/// decides like it: behind the camera, NaN and infinities.
#[test]
fn particle_test_edge_cases() {
    let projection = (70.0, 16.0 / 9.0, 512.0);
    let eye = [100.5, 70.2, -40.7];
    let (yaw, pitch) = (30.0, 10.0);
    let ours = particle_frustum_of(eye, yaw, pitch, projection, false);
    let game = GameParticleTest::new(eye, yaw, pitch, projection);
    let (right, up, forward) = crate::camera::axes(yaw, pitch);
    let at = |d: Vec3| std::array::from_fn(|i| eye[i] + f64::from(d[i]));
    // 20 blocks ahead, just beyond the right edge of the true field (half-angle along x).
    let half_width = (projection.0.to_radians() * 0.5).tan() * projection.1 * 20.0;
    let off_screen = at(forward * 20.0 + right * (half_width + 1.0));
    let (shown, _) = game.test(off_screen);
    assert!(shown, "the game draws a centre one block off screen at 20 blocks");
    assert!(ours.contains_point(relative(eye, off_screen)));
    let (origin, _, view_m) = crate::camera::view_matrix(eye, yaw, pitch);
    let true_field = frustum_planes(perspective(projection.0, projection.1, 0.05, projection.2) * view_m);
    let p = relative(eye, off_screen);
    assert!(!aabb_visible(&true_field, Vec3::from(p), Vec3::from(p)), "the entities' test hides it");
    // Behind the camera, farther than the three blocks: hidden by both.
    let behind = at(forward * -6.0);
    assert!(!game.test(behind).0 && !ours.contains_point(relative(eye, behind)));
    // In view, in a section nobody reached, and far above the world: drawn (no occlusion).
    let ahead = relative(eye, at(forward * 30.0 + up * 2.0));
    assert!(ours.contains_point(ahead));
    let empty_walk = std::sync::Arc::new(Reach::Set([(1000, 0, 1000)].into_iter().collect()));
    let entities =
        FrameView { planes: true_field, origin, min_y: -64, sections: 24, reached: empty_walk, occlusion: true };
    assert!(!entities.verdict(ahead, ahead).visible(), "the entities' test hides an unreached section");
    let sky = GameParticleTest::new([0.5, 400.0, 0.5], 0.0, -60.0, projection);
    let ours_sky = particle_frustum_of([0.5, 400.0, 0.5], 0.0, -60.0, projection, false);
    let high = [0.5, 420.0, 30.0];
    assert_eq!(ours_sky.contains_point(relative([0.5, 400.0, 0.5], high)), sky.test(high).0);
    for simd in [false, true] {
        let f = particle_frustum_of(eye, yaw, pitch, projection, simd);
        for p in [[f32::NAN, 0.0, 0.0], [0.0, f32::INFINITY, 0.0], [f32::NEG_INFINITY, 0.0, 5.0], [f32::NAN; 3]] {
            assert!(!f.contains_point(p), "{p:?}, simd {simd}");
        }
    }
}

/// The SIMD planes answer like the scalar ones, bit for bit, on points of every kind.
#[test]
fn particle_simd_test_matches_the_scalar_one() {
    let mut rng = Rng(0xd1b5_4a32_d192_ed03);
    let special = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.0, -0.0, 1.0e30, -1.0e30, f32::MIN_POSITIVE];
    for _ in 0..400 {
        let eye =
            [f64::from(rng.below(200)) - 100.5, f64::from(rng.below(120)) - 30.25, f64::from(rng.below(200)) - 100.75];
        let (yaw, pitch) = (rng.unit() * 360.0, rng.unit() * 180.0 - 90.0);
        let projection = (30.0 + rng.unit() * 80.0, 0.5 + rng.unit() * 2.0, 64.0 + rng.unit() * 900.0);
        let (scalar, simd) = (
            particle_frustum_of(eye, yaw, pitch, projection, false),
            particle_frustum_of(eye, yaw, pitch, projection, true),
        );
        for _ in 0..250 {
            let p: [f32; 3] = std::array::from_fn(|_| match rng.below(20) {
                0 => special[rng.below(special.len() as u64) as usize],
                _ => rng.unit() * 160.0 - 80.0,
            });
            assert_eq!(simd.contains_point(p), scalar.contains_point(p), "{p:?}");
        }
    }
}

/// Micro-bench (machine loaded by other agents' builds: an order of magnitude only): one particle
/// point per call, the game's test scalar and SIMD against the entities' test of before.
#[test]
#[ignore = "micro-bench: cargo test --release -p rc-app particle_frustum_bench -- --ignored --nocapture"]
fn particle_frustum_bench() {
    let mut rng = Rng(0x0123_4567_89ab_cdef);
    let eye = [10.5, 70.25, -3.75];
    let (yaw, pitch) = (20.0, 5.0);
    let projection = (70.0, 16.0 / 9.0, 512.0);
    let points: Vec<[f32; 3]> = (0..1_000_000)
        .map(|_| [rng.unit() * 120.0 - 60.0, 40.0 + rng.unit() * 60.0, rng.unit() * 120.0 - 60.0])
        .collect();
    let time = |name: &str, test: &dyn Fn([f32; 3]) -> bool| {
        let best = (0..5)
            .map(|_| {
                let t = std::time::Instant::now();
                std::hint::black_box(points.iter().filter(|p| test(**p)).count());
                t.elapsed().as_secs_f64() * 1e9 / points.len() as f64
            })
            .fold(f64::MAX, f64::min);
        eprintln!("particle points, {name}: {best:.2} ns per point");
    };
    let scalar = particle_frustum_of(eye, yaw, pitch, projection, false);
    let simd = particle_frustum_of(eye, yaw, pitch, projection, true);
    time("game frustum, scalar", &|p| scalar.contains_point(p));
    time("game frustum, SIMD", &|p| simd.contains_point(p));
    let (origin, _, view_m) = crate::camera::view_matrix(eye, yaw, pitch);
    let planes = frustum_planes(perspective(projection.0, projection.1, 0.05, projection.2) * view_m);
    let list: Vec<_> = (-4..=4).flat_map(|x| (0..24).flat_map(move |y| (-4..=4).map(move |z| (x, y, z)))).collect();
    let reached = std::sync::Arc::new(Reach::Grid(ReachGrid::new(&list)));
    let view = FrameView { planes, origin, min_y: -64, sections: 24, reached, occlusion: true };
    time("entities' test of before", &|p| view.verdict(p, p).visible());
}

/// The walk task's column index gives every column the run the two binary searches find: random
/// walks of every density, columns absent from the walk, an empty walk.
#[test]
fn reached_column_index_finds_the_runs_of_the_binary_searches() {
    let mut rng = Rng(0x5eed_1d3c);
    let mut columns = 0;
    for density in [0, 1, 4, 8] {
        let mut reached: Vec<_> = (-6..=6)
            .flat_map(|x| (-6..=6).flat_map(move |z| (0..24).map(move |sy| (x, sy, z))))
            .filter(|_| rng.below(8) < density)
            .collect();
        reached.sort_unstable_by_key(|&(x, sy, z)| (x, z, sy));
        let index = ReachedColumns::new(&reached);
        for x in -8..=8 {
            for z in -8..=8 {
                let want = column_run(&reached, (x, z));
                assert_eq!(index.run(&reached, (x, z)), want, "density {density}, column ({x}, {z})");
                columns += usize::from(!want.is_empty());
            }
        }
    }
    assert!(columns > 400, "too few reached columns ({columns})");
}

/// What makes the candidate list safe: the sections a meshing view keeps of a column are always
/// among the reached ones the list reads (`reached_sections`), whatever the view; along random views
/// over random walks, columns wholly in the field keep them all, others part or none.
#[test]
fn sections_in_view_are_among_the_reached_ones() {
    let mut rng = Rng(0x5eed_5ec7);
    let (mut all, mut part, mut none) = (0, 0, 0);
    for f in 0..200 {
        let eye = [f64::from(rng.below(64)) - 32.0, f64::from(rng.below(120)), f64::from(rng.below(64)) - 32.0];
        let (yaw, pitch) = (rng.below(360) as f32, rng.below(180) as f32 - 90.0);
        let (origin, _, view_m) = crate::camera::view_matrix(eye, yaw, pitch);
        let planes = frustum_planes(perspective(70.0, 16.0 / 9.0, CULL_NEAR, 4.0 * 12.0 * 16.0) * view_m);
        let view = MeshingView::new(planes, origin, eye, crate::camera::axes(yaw, pitch).2, -64, 24);
        for _ in 0..40 {
            let key = (rng.below(25) - 12, rng.below(25) - 12);
            let run: Vec<_> = (0..24).filter(|_| rng.below(3) == 0).map(|sy| (key.0, sy, key.1)).collect();
            let (seen, reached) = (view.sections_in_view(key, &run), sections_of_run(&run));
            assert_eq!(seen.intersection(reached), seen, "frame {f}, column {key:?}: a section in view not reached");
            match seen.count(24) {
                0 => none += 1,
                n if n == run.len() => all += 1,
                _ => part += 1,
            }
        }
    }
    assert!(all > 50 && part > 1000 && none > 1000, "cases {all} / {part} / {none}");
}

/// World of the meshing pick bench, like plains32: the columns within 32 of the origin, all drawn,
/// the walk reaching sections 5 to 12 of each (the surface), `dirty` columns spread around the
/// camera with marks under the surface (outside the walk: held under every view, the steady state
/// after loading) or on it (`on_surface`: held only while out of the field, as during loading).
struct PickWorld {
    held: hashbrown::HashSet<(i32, i32)>,
    reached: Vec<(i32, i32, i32)>,
    dirty: hashbrown::HashMap<(i32, i32), Sections>,
}

fn pick_world(dirty_columns: usize, on_surface: bool) -> PickWorld {
    let columns: Vec<(i32, i32)> =
        (-32..=32).flat_map(|x| (-32..=32).map(move |z| (x, z))).filter(|&(x, z)| x * x + z * z <= 32 * 32).collect();
    let mut reached: Vec<_> = columns.iter().flat_map(|&(x, z)| (5..=12).map(move |sy| (x, sy, z))).collect();
    reached.sort_unstable_by_key(|&(x, sy, z)| (x, z, sy));
    let mut marks = Sections::default();
    (if on_surface { [7, 8] } else { [1, 2] }).into_iter().for_each(|s| marks.insert(s));
    let mut rng = Rng(0x5eed_0b15);
    let mut dirty = hashbrown::HashMap::new();
    while dirty.len() < dirty_columns.min(columns.len()) {
        dirty.insert(columns[rng.below(columns.len() as u64) as usize], marks);
    }
    PickWorld { held: columns.into_iter().collect(), reached, dirty }
}

/// Micro-bench of the meshing pick (one core: it runs on the main thread, which owns the marks)
/// while the camera turns in place, 850 dirty columns out of the 3,209 drawn (plains32 held about
/// 850): every dirty column tested against the view (`RUSTCRAFT_OPT_MESH_PICK_REACHED_MARKS=0`), the
/// candidate list kept (nothing marked meanwhile) and the candidate list made again every frame (new
/// marks every frame, as when flying). Nothing launches (no free slot), so every frame does the
/// same work.
#[test]
#[ignore = "micro-bench: cargo test --release -p rc-app --bin rustcraft meshing_pick_bench -- --ignored --nocapture"]
fn meshing_pick_bench() {
    use crate::app_meshing::{CandidateKey, MeshCandidates, pick_columns, sections_to_launch, view_may_launch};
    const FRAMES: u32 = 2000;
    let eye = [0.5, 70.0, 0.5];
    let views: Vec<MeshingView> = (0..FRAMES)
        .map(|f| {
            let (yaw, pitch) = (f as f32 * 0.9, 10.0);
            let (origin, _, view_m) = crate::camera::view_matrix(eye, yaw, pitch);
            let planes = frustum_planes(perspective(70.0, 16.0 / 9.0, CULL_NEAR, 4.0 * 32.0 * 16.0) * view_m);
            MeshingView::new(planes, origin, eye, crate::camera::axes(yaw, pitch).2, -64, 24)
        })
        .collect();
    let cases = [("marks under the surface", false), ("marks on the surface", true)];
    for ((case, on_surface), indexed) in cases.into_iter().flat_map(|c| [(c, false), (c, true)]) {
        let w = pick_world(850, on_surface);
        let index = ReachedColumns::new(&w.reached);
        let run = |k| if indexed { index.run(&w.reached, k) } else { column_run(&w.reached, k) };
        let in_view = |view: &MeshingView, k| w.held.contains(&k).then(|| view.sections_in_view(k, run(k)));
        let reached_of = |k| w.held.contains(&k).then(|| sections_of_run(run(k)));
        let key = |marks| CandidateKey { marks, reach: [1, 1, 1], sync_mode: 0, player_marks: 0, eye_column: None };
        let case = format!("{case}, {}", if indexed { "column index" } else { "binary searches" });
        // Paths: no list; the list with nothing marked (turning in place), new marks every 4
        // frames, new marks every frame; and a list made every time the key changes (not kept:
        // the reason the list waits for a key seen twice).
        let paths =
            ["every dirty column", "list, no new mark", "list, marks every 4 frames", "list, marks every frame"];
        for pass in 0..2 {
            for path in paths.into_iter().chain(["list made at each new key, marks every frame"]) {
                let (mut dirty, mut list) = (w.dirty.clone(), MeshCandidates::default());
                let tested = std::cell::Cell::new(0usize);
                let t = std::time::Instant::now();
                for (f, view) in views.iter().enumerate() {
                    let launchable = |k, m| {
                        tested.set(tested.get() + 1);
                        sections_to_launch(m, in_view(view, k), true, false)
                    };
                    let may_launch = |k, m| view_may_launch(m, reached_of(k), false);
                    let among = match path {
                        "every dirty column" => None,
                        "list, no new mark" => list.list(key(0), &dirty, may_launch),
                        "list, marks every 4 frames" => list.list(key(f as u64 / 4), &dirty, may_launch),
                        "list, marks every frame" => list.list(key(f as u64), &dirty, may_launch),
                        _ => {
                            list.refresh(key(f as u64), &dirty, may_launch);
                            Some(list.columns.as_slice())
                        }
                    };
                    std::hint::black_box(pick_columns(&mut dirty, among, 0, (0, 0), |_, _| true, launchable, false));
                }
                let us = t.elapsed().as_secs_f64() * 1e6 / f64::from(FRAMES);
                if pass == 1 {
                    eprintln!(
                        "meshing pick, {case}, {path}: {us:.2} us per frame ({:.0} columns tested per frame)",
                        tested.get() as f64 / f64::from(FRAMES)
                    );
                }
            }
        }
    }
}
