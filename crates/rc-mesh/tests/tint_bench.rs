//! Micro-bench of the biome tint paths (C2-M06, C2-M14), INTERLEAVED variants (order rotated at
//! each pass, best and median of 7) to hold up on a loaded machine. Ignored by default:
//! `cargo test --release -p rc-mesh --test tint_bench -- --ignored --nocapture`.
//! Identical output proven by `tests/digest.rs`. Two timings per variant:
//! - "tint": the tint requests of the real meshes, recorded once and replayed alone (the tint is a
//!   few percent of a mesh, below the noise of the full meshing on a loaded machine); the replays of
//!   every variant must give the same colours;
//! - "mesh": the full meshing.
//!
//! Blend radius: `RC_TINT_BLEND` (2 by default); thread count of the "all" passes:
//! `RAYON_NUM_THREADS`; `RC_TINT_MESH=0` skips the full meshing.

mod common;

use std::hint::black_box;
use std::time::Instant;

use rayon::prelude::*;
use rc_mesh::blocks::{TintCounters, TintPaths, mesh, record_tint_requests, replay_tint_requests, take_tint_counters};
use rc_mesh::wire::MeshJob;

/// Best and median of a list of timings.
fn best_and_median(v: &mut [f64]) -> (f64, f64) {
    v.sort_by(f64::total_cmp);
    (v[0], v[v.len() / 2])
}

#[test]
#[ignore]
fn tint_paths_bench() {
    let Some((_d, cols, t, palette)) = common::load() else { return };
    let mut keys: Vec<(i32, i32)> = cols.keys().copied().collect();
    keys.sort_unstable();
    let (all, old) = (TintPaths::ALL, TintPaths::OLD);
    // The paths before C2-M14: the reference of the new ones.
    let before = TintPaths { uniform_cells: false, four_block_rows: false, ..all };
    let variants = [
        ("old", old),
        ("before C2-M14", before),
        ("+ uniform cells", TintPaths { uniform_cells: true, ..before }),
        ("+ rows of four", TintPaths { four_block_rows: true, ..before }),
        ("+ cells + rows", TintPaths { uniform_cells: true, four_block_rows: true, ..before }),
        ("all except cells", TintPaths { uniform_cells: false, ..all }),
        ("all except rows", TintPaths { four_block_rows: false, ..all }),
        ("all", all),
    ];
    let rad: i32 = rc_data::env_str!("RC_TINT_BLEND").and_then(|v| v.parse().ok()).unwrap_or(2);
    let with_mesh = rc_data::env_str!("RC_TINT_MESH").is_none_or(|v| v != "0");
    let job = |p: TintPaths, (x, z): (i32, i32)| -> MeshJob {
        let mut j = common::job(&cols, x, z);
        j.opts.blend = rad;
        j.tint = p;
        j
    };
    // 1 thread: one column in 4 (312); all threads: all of them.
    let one: Vec<Vec<MeshJob>> =
        variants.iter().map(|v| keys.iter().step_by(4).map(|&k| job(v.1, k)).collect()).collect();
    let par: Vec<Vec<MeshJob>> = variants.iter().map(|v| keys.iter().map(|&k| job(v.1, k)).collect()).collect();
    // Tint requests of each column's real mesh (they do not depend on the tint paths).
    let requests: Vec<Vec<[i32; 4]>> =
        par[1].iter().map(|j| record_tint_requests(|| mesh(j, &t, &palette, common::MIN_Y)).1).collect();
    let one_requests: Vec<&Vec<[i32; 4]>> = requests.iter().step_by(4).collect();
    let n = variants.len();
    let (mut tint1, mut tintp, mut mesh1, mut meshp) =
        (vec![vec![]; n], vec![vec![]; n], vec![vec![]; n], vec![vec![]; n]);
    let mut counters = vec![TintCounters::default(); n];
    let mut sums = vec![0u64; n];
    for pass in 0..7 {
        for k in 0..n {
            let i = (k + pass * 3) % n;
            take_tint_counters();
            let s = Instant::now();
            let sum = one[i]
                .iter()
                .zip(&one_requests)
                .fold(0u64, |h, (j, r)| h.rotate_left(5) ^ replay_tint_requests(j, &palette, variants[i].1, r));
            tint1[i].push(s.elapsed().as_secs_f64() * 1e3);
            if pass == 0 {
                counters[i] = take_tint_counters();
                sums[i] = sum;
            }
            let s = Instant::now();
            black_box(
                par[i]
                    .par_iter()
                    .zip(&requests)
                    .map(|(j, r)| replay_tint_requests(j, &palette, variants[i].1, r))
                    .sum::<u64>(),
            );
            tintp[i].push(s.elapsed().as_secs_f64() * 1e3);
            if !with_mesh {
                continue;
            }
            let s = Instant::now();
            black_box(one[i].iter().map(|j| mesh(j, &t, &palette, common::MIN_Y).len()).sum::<usize>());
            mesh1[i].push(s.elapsed().as_secs_f64() * 1e3);
            let s = Instant::now();
            black_box(par[i].par_iter().map(|j| mesh(j, &t, &palette, common::MIN_Y).len()).sum::<usize>());
            meshp[i].push(s.elapsed().as_secs_f64() * 1e3);
        }
    }
    let total: usize = requests.iter().map(Vec::len).sum();
    eprintln!(
        "radius {rad}, ms (best / median, gain of the best against 'before C2-M14'), 1 thread on {} columns, {} threads on {}, {total} recorded requests:",
        one[0].len(),
        rayon::current_num_threads(),
        par[0].len()
    );
    let reference = [&tint1, &tintp, &mesh1, &meshp].map(|v| v[1].clone());
    for i in 0..n {
        let c = counters[i];
        let share = |hits: u64| 100.0 * hits as f64 / c.zoomed_blocks.max(1) as f64;
        let mut line = format!("  {:<18}", variants[i].0);
        for (name, (times, base)) in ["tint 1t", "tint all", "mesh 1t", "mesh all"]
            .iter()
            .zip([&tint1, &tintp, &mesh1, &meshp].iter().zip(&reference))
        {
            if times[i].is_empty() {
                continue;
            }
            let (b, m) = best_and_median(&mut times[i].clone());
            let (b0, _) = best_and_median(&mut base.clone());
            line += &format!("  {name} {b:7.1} / {m:7.1} ({:+5.1} %)", (b / b0 - 1.0) * 100.0);
        }
        eprintln!(
            "{line}  zooms {:>9}, uniform cells {:5.1} %",
            c.zoomed_blocks,
            share(c.uniform_cell_hits)
        );
    }
    assert!(sums.iter().all(|&s| s == sums[0]), "replayed colours differ between the paths: {sums:x?}");
}
