//! Bench of the visible part of explosion targets (`seen_percent`): 1, 10, 100 and 1,000 boxes of
//! primed TNT size scattered within reach of an explosion centre (on the ground, in the air,
//! behind walls of fences, panes, slabs, stairs and full blocks), weighed one call per box or
//! together, for each configuration of the visible-part switches.
//!
//! `cargo test --release -p rc-sim --lib bench_visible_part -- --ignored --nocapture --test-threads=1`.

use crate::bench_kit::{cmd, report};
use crate::blocks::probe::Probe;
use crate::mobs::visible_part::{FORCED_VISIBLE_PART_PATHS, VisiblePartPaths};

pub(crate) const BENCH_CENTER: [f64; 3] = [60.5, -59.94, 20.5];

/// Walls of mixed shapes around `BENCH_CENTER`, and a few full blocks.
pub(crate) fn build_mixed_walls(p: &mut Probe) {
    for (i, block) in ["oak_fence", "glass_pane", "stone_slab", "oak_stairs", "cobblestone_wall", "stone"].iter().enumerate() {
        let z = 14 + 2 * i as i32;
        cmd(p, &format!("fill 52 -60 {z} 56 -59 {z} minecraft:{block}"));
        cmd(p, &format!("fill 64 -60 {z} 68 -58 {z} minecraft:{block}"));
    }
}

/// `n` primed TNT boxes within 8 blocks of `BENCH_CENTER`, from a small fixed generator: on the
/// ground one time in three, otherwise up to 6 blocks up.
pub(crate) fn scattered_boxes(n: usize) -> Vec<rc_physics::Aabb> {
    let mut x = 0x9e37_79b9_7f4a_7c15u64;
    let mut next = || {
        x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (x >> 11) as f64 / (1u64 << 53) as f64
    };
    (0..n)
        .map(|i| {
            let (dx, dz) = (next() * 11.0 - 5.5, next() * 11.0 - 5.5);
            let y = if i % 3 == 0 { -60.0 } else { -60.0 + next() * 6.0 };
            let (px, pz) = (BENCH_CENTER[0] + dx, BENCH_CENTER[2] + dz);
            tnt_box_at([px, y, pz])
        })
        .collect()
}

/// A primed TNT box standing on `at`.
fn tnt_box_at(at: [f64; 3]) -> rc_physics::Aabb {
    rc_physics::Aabb { min: [at[0] - 0.49, at[1], at[2] - 0.49], max: [at[0] + 0.49, at[1] + 0.98, at[2] + 0.49] }
}

/// One timing of `f`, in microseconds per box.
fn micros_per_box(boxes: usize, f: &mut dyn FnMut()) -> f64 {
    let rounds = (2000 / boxes).max(1);
    let start = std::time::Instant::now();
    for _ in 0..rounds {
        f();
    }
    start.elapsed().as_secs_f64() * 1e6 / (rounds * boxes) as f64
}

/// The paths the bench compares, and whether the boxes are weighed together: the former path,
/// the cube per call with its cells, with its shape count, with both, then together with both,
/// with equal boxes once, the defaults under several cube cost limits (0: no cube, 3: the
/// default), then on 1 thread and powers of 2 up to `RUSTCRAFT_THREADS` or every
/// logical core.
fn bench_configurations() -> Vec<(String, VisiblePartPaths, bool)> {
    let old = VisiblePartPaths::OLD;
    let always_cube = VisiblePartPaths { cube_cost_limit: usize::MAX, ..old };
    let per_call = VisiblePartPaths { cube_per_call: true, ..always_cube };
    let cube_and_count = VisiblePartPaths { cell_cube: true, air_shortcut: true, ..always_cube };
    let mut configs = vec![
        ("former".to_owned(), old, false),
        ("cell cube per call".to_owned(), VisiblePartPaths { cell_cube: true, ..per_call }, false),
        ("air shortcut per call".to_owned(), VisiblePartPaths { air_shortcut: true, ..per_call }, false),
        ("cube and shortcut per call".to_owned(), VisiblePartPaths { cube_per_call: true, ..cube_and_count }, false),
        ("cube and shortcut together".to_owned(), cube_and_count, true),
        ("same box once together".to_owned(), VisiblePartPaths { same_box_once: true, ..cube_and_count }, true),
    ];
    for limit in [0, 2, 3, 4] {
        let paths = VisiblePartPaths { cube_cost_limit: limit, ..VisiblePartPaths::NEW };
        configs.push((format!("defaults, cube cost limit {limit}"), paths, true));
    }
    let cores = rc_data::env_str!("RUSTCRAFT_THREADS")
        .and_then(|v| v.parse().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, std::num::NonZero::get));
    let mut threads = 1;
    while threads <= cores {
        let paths =
            VisiblePartPaths { same_box_once: true, parallel: true, walked_cells_per_task: 1, threads, ..cube_and_count };
        configs.push((format!("together on {threads} threads"), paths, true));
        threads *= 2;
    }
    configs
}

#[test]
#[ignore = "measurement bench"]
fn bench_visible_part() {
    let (mut p, _dir) = crate::tests_g3f13::open("visible-part-bench");
    build_mixed_walls(&mut p);
    let configs = bench_configurations();
    // A lone box 20 blocks away on x and z and 8 up: the reach of a strong `/summon` explosion,
    // the largest cube for the fewest sightlines.
    let far = [BENCH_CENTER[0] + 20.0, BENCH_CENTER[1] + 8.0, BENCH_CENTER[2] + 20.0];
    let cases: Vec<(String, Vec<rc_physics::Aabb>)> = [1, 2, 4, 10, 100, 1000]
        .into_iter()
        .map(|n| (format!("{n}_boxes"), scattered_boxes(n)))
        .chain([("1_far_box".to_owned(), vec![tnt_box_at(far)])])
        // Two boxes on the ground on either side of the centre: the cube grows as the square of
        // their distance, their sightlines only linearly.
        .chain([6.0, 10.0, 14.0, 20.0].map(|d| {
            let pair = [-d, d].map(|d| tnt_box_at([BENCH_CENTER[0] + d, -60.0, BENCH_CENTER[2] + d]));
            (format!("2_boxes_{d}_blocks_away"), pair.to_vec())
        }))
        .collect();
    for (case, boxes) in &cases {
        let n = boxes.len();
        let ratio = crate::mobs::visible_part::cube_cost_ratio(BENCH_CENTER, boxes).unwrap_or(f64::NAN);
        report("visible_part", case, "cube cost over walked cells", ratio);
        // The configurations alternate over 7 rounds, the best of each kept: a load that comes
        // and goes on the machine then weighs on all of them alike.
        let mut best = vec![f64::INFINITY; configs.len()];
        for _ in 0..7 {
            for ((_, paths, together), best) in configs.iter().zip(&mut best) {
                FORCED_VISIBLE_PART_PATHS.set(Some(*paths));
                let us = micros_per_box(n, &mut || {
                    if *together {
                        std::hint::black_box(p.s.seen_percents(BENCH_CENTER, boxes));
                    } else {
                        for b in boxes {
                            std::hint::black_box(p.s.seen_percent(BENCH_CENTER, b));
                        }
                    }
                });
                *best = best.min(us);
            }
        }
        for ((label, _, _), us) in configs.iter().zip(best) {
            report("visible_part", case, &format!("us_per_box {label}"), us);
        }
    }
    FORCED_VISIBLE_PART_PATHS.set(None);
}
