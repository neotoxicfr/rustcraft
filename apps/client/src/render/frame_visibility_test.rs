//! Every path of the frame's visibility test gives the verdict of the test of before, class
//! included, on random cameras, walks, boxes and points (small, across sections, out of the world in
//! height, one float step either side of a section face, NaN, infinities, huge), with the reached
//! sections as a grid or a set, occlusion off and no walk yet; each sabotage is caught.
//! Micro-bench (ignored): `frame_visibility_bench`.

use std::sync::Arc;

use super::*;
use crate::render::frustum::frustum_planes;
use crate::render::lists::ReachGrid;
use crate::render::perspective;

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

const MIN_Y: i32 = -64;
const SECTIONS: i32 = 24;

/// Every combination of the paths (16), the old one first.
fn all_paths() -> Vec<VisibilityPaths> {
    (0..16)
        .map(|bits| VisibilityPaths {
            direct_grid: bits & 1 != 0,
            y_runs: bits & 2 != 0,
            points: bits & 4 != 0,
            plane_masks: bits & 8 != 0,
            sabotage: None,
        })
        .collect()
}

/// A frame: random camera, reached sections around it (one in three; none in one round in seven),
/// as a grid (even rounds) or a set; occlusion off in one round in five.
fn random_view(rng: &mut Rng, round: u32) -> FrameView {
    let eye =
        [f64::from(rng.below(200)) - 100.5, f64::from(rng.below(120)) - 30.25, f64::from(rng.below(200)) - 100.75];
    let (yaw, pitch) = (rng.unit() * 360.0, rng.unit() * 180.0 - 90.0);
    let (origin, _, view_m) = crate::camera::view_matrix(eye, yaw, pitch);
    let planes = frustum_planes(perspective(70.0, 16.0 / 9.0, 0.05, 160.0) * view_m);
    let (cx, cz) = ((eye[0] / 16.0).floor() as i32, (eye[2] / 16.0).floor() as i32);
    let mut list = Vec::new();
    if !round.is_multiple_of(7) {
        for x in cx - 4..=cx + 4 {
            for z in cz - 4..=cz + 4 {
                for y in 0..SECTIONS {
                    if rng.below(3) == 0 {
                        list.push((x, y, z));
                    }
                }
            }
        }
    }
    let reached = if round.is_multiple_of(2) {
        Reach::Grid(ReachGrid::new(&list))
    } else {
        Reach::Set(list.iter().copied().collect())
    };
    FrameView {
        planes,
        origin,
        min_y: MIN_Y,
        sections: SECTIONS,
        reached: Arc::new(reached),
        occlusion: !round.is_multiple_of(5),
    }
}

/// A coordinate one float step around a face of a section or of its one-block margin, or of the
/// band of blocks the point path takes as inside (blocks 1 and 13 of a section), on `axis`.
fn near_a_face(rng: &mut Rng, view: &FrameView, axis: usize) -> f32 {
    let offset = [view.origin[0], -view.min_y, view.origin[1]][axis];
    let section = rng.below(12) - 6 + if axis == 1 { 6 } else { 0 };
    let blocks = [-2, -1, 0, 1, 2, 13, 14, 15][rng.below(8) as usize];
    let face = (section * 16 + blocks - offset) as f32;
    match rng.below(3) {
        0 => face.next_down(),
        1 => face,
        _ => face.next_up(),
    }
}

/// A coordinate of every kind: ordinary, near a face, out of the world in height, special.
fn coordinate(rng: &mut Rng, view: &FrameView, axis: usize) -> f32 {
    match rng.below(40) {
        0 => f32::NAN,
        1 => f32::INFINITY,
        2 => f32::NEG_INFINITY,
        3 => 1.0e30 * if rng.below(2) == 0 { 1.0 } else { -1.0 },
        4 => 9_000_000.0 + rng.unit() * 100.0,
        5..=15 => near_a_face(rng, view, axis),
        _ if axis == 1 => rng.unit() * 500.0 - 150.0,
        _ => rng.unit() * 120.0 - 60.0,
    }
}

/// A box: a point, a small box, a few blocks, or long (several sections).
fn random_box(rng: &mut Rng, view: &FrameView) -> ([f32; 3], [f32; 3]) {
    let min: [f32; 3] = std::array::from_fn(|a| coordinate(rng, view, a));
    let size = [0.0, 0.25, 3.0, 40.0][rng.below(4) as usize];
    let max = min.map(|v| v + rng.unit() * size);
    (min, max)
}

fn visibility(view: &FrameView, paths: VisibilityPaths) -> FrameVisibility {
    let copy = FrameView { reached: view.reached.clone(), ..*view };
    FrameVisibility::new(copy, paths)
}

/// Verdicts of `paths` that differ from the old test, over `rounds` frames of 250 boxes and 250
/// points; `seen` counts the old verdicts (to prove the cases are all drawn).
fn differences(paths: VisibilityPaths, rounds: u32, seen: &mut [u32; BoxVerdict::ALL.len()]) -> u32 {
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut different = 0;
    for round in 0..rounds {
        let view = random_view(&mut rng, round);
        let test = visibility(&view, paths);
        for _ in 0..250 {
            let (min, max) = random_box(&mut rng, &view);
            let before = view.verdict(min, max);
            seen[before as usize] += 1;
            let now = test.box_verdict(&view, min, max);
            different += u32::from(now != before || test.aabb(min, max) != before.visible());
            let p: [f32; 3] = std::array::from_fn(|a| coordinate(&mut rng, &view, a));
            let before = view.verdict(p, p);
            let now = if paths.points { test.point_verdict(&view, p) } else { test.box_verdict(&view, p, p) };
            different += u32::from(now != before || test.point(p) != before.visible());
        }
    }
    different
}

#[test]
fn every_path_gives_the_old_verdicts() {
    for paths in all_paths() {
        let mut seen = [0; BoxVerdict::ALL.len()];
        // 200 frames × (250 boxes + 250 points) = 10^5 tests per combination.
        assert_eq!(differences(paths, 200, &mut seen), 0, "{paths:?}");
        for (v, n) in BoxVerdict::ALL.iter().zip(seen) {
            assert!(n > 0, "verdict {v:?} never drawn: the test does not cover it");
        }
    }
}

#[test]
fn each_sabotage_is_caught() {
    let all_on = VisibilityPaths { direct_grid: true, y_runs: true, points: true, plane_masks: true, sabotage: None };
    for sabotage in [VisibilitySabotage::EdgePointInOneSection, VisibilitySabotage::NeighboursIgnored] {
        let mut seen = [0; BoxVerdict::ALL.len()];
        let wrong = differences(VisibilityPaths { sabotage: Some(sabotage), ..all_on }, 200, &mut seen);
        assert!(wrong > 0, "{sabotage:?} not caught");
    }
}

/// The test that finds everything visible answers yes even for NaN boxes, as the closure it
/// replaces did.
#[test]
fn the_neutral_test_shows_everything() {
    let all = FrameVisibility::everything();
    assert!(all.aabb([f32::NAN; 3], [f32::NAN; 3]));
    assert!(all.point([1.0e30, -5.0, f32::INFINITY]));
}

/// Reached runs read as bits answer like the section-by-section lookup, with columns taller than
/// a word of the grid (runs over two words and more) and ranges partly or wholly outside it.
#[test]
fn bit_runs_answer_like_the_sections_one_by_one() {
    let mut rng = Rng(0x2545_f491_4f6c_dd1d);
    for round in 0..60 {
        let height: u64 = [3, 24, 70, 200][round % 4];
        let list: Vec<_> = (0..400).map(|_| (rng.below(9) - 4, rng.below(height), rng.below(9) - 4)).collect();
        let list: Vec<_> = list.into_iter().filter(|_| rng.below(4) != 0).collect();
        let grid = ReachGrid::new(&list);
        for _ in 0..500 {
            let range = |rng: &mut Rng, span: i32| {
                let a = rng.below(span as u64 + 8) - 4;
                [a, a + rng.below(span as u64)]
            };
            let (xs, ys, zs) = (range(&mut rng, 10), range(&mut rng, height as i32 + 4), range(&mut rng, 10));
            let want =
                (xs[0]..=xs[1]).any(|x| (zs[0]..=zs[1]).any(|z| (ys[0]..=ys[1]).any(|y| grid.contains(x, y, z))));
            assert_eq!(grid.any_in_box(xs, ys, zs, false), want, "{xs:?} {ys:?} {zs:?}");
            assert_eq!(grid.any_in_box(xs, ys, zs, true), want, "{xs:?} {ys:?} {zs:?} (runs)");
        }
    }
}

/// Micro-bench (machine loaded by other agents' builds: an order of magnitude, not a decision):
/// entity-sized boxes and particle points on a frame with occlusion, per path.
#[test]
#[ignore = "micro-bench: cargo test --release -p rc-app frame_visibility_bench -- --ignored --nocapture"]
fn frame_visibility_bench() {
    let mut rng = Rng(0x0123_4567_89ab_cdef);
    let view = random_view(&mut rng, 2);
    let boxes: Vec<_> = (0..1_000_000)
        .map(|_| {
            let at = [rng.unit() * 120.0 - 60.0, rng.unit() * 160.0 - 80.0, rng.unit() * 120.0 - 60.0];
            (at, [at[0] + 0.6, at[1] + 1.8, at[2] + 0.6])
        })
        .collect();
    let off = VisibilityPaths::OLD;
    let on = VisibilityPaths { direct_grid: true, y_runs: true, points: true, plane_masks: true, sabotage: None };
    let cases = [
        ("old", off),
        ("direct grid", VisibilityPaths { direct_grid: true, ..off }),
        ("direct grid + y runs", VisibilityPaths { direct_grid: true, y_runs: true, ..off }),
        ("direct grid + plane masks", VisibilityPaths { direct_grid: true, plane_masks: true, ..off }),
        ("all", on),
    ];
    for (name, paths) in cases {
        let test = visibility(&view, paths);
        let best = (0..5)
            .map(|_| {
                let t = std::time::Instant::now();
                let n = boxes.iter().filter(|(a, b)| test.aabb(*a, *b)).count();
                std::hint::black_box(n);
                t.elapsed().as_secs_f64() * 1e9 / boxes.len() as f64
            })
            .fold(f64::MAX, f64::min);
        eprintln!("boxes, {name}: {best:.2} ns per box");
    }
    for (name, paths) in [("old", off), ("points", VisibilityPaths { points: true, ..off }), ("all", on)] {
        let test = visibility(&view, paths);
        let best = (0..5)
            .map(|_| {
                let t = std::time::Instant::now();
                let n = boxes.iter().filter(|(a, _)| test.point(*a)).count();
                std::hint::black_box(n);
                t.elapsed().as_secs_f64() * 1e9 / boxes.len() as f64
            })
            .fold(f64::MAX, f64::min);
        eprintln!("points, {name}: {best:.2} ns per point");
    }
}
