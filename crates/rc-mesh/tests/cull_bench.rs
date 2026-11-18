//! Micro-bench of the per-block loop paths (C2-M10, C2-M08): all the columns of the capture,
//! variants measured in alternation (same machine load for each), best time of
//! `RC_BENCH_ROUNDS` passes (5 by default), 1 thread then all threads. Then the partial remesh
//! of one section per column (the 9th), neighbours' border rows decoded alone or whole. Launch:
//! `cargo test --release -p rc-mesh --test cull_bench -- --ignored --nocapture`.
// Direct read allowed (bench: setting read at launch, off the measured path).
#![allow(clippy::disallowed_methods)]

mod common;

use std::hint::black_box;
use std::sync::Arc;
use std::time::Instant;

use rayon::prelude::*;
use rc_mesh::cull::CullPaths;
use rc_mesh::wire::MeshJob;

/// Best time (s) of each job set over `rounds` alternated passes, 1 thread or all threads.
fn best_times(
    jobs: &[Vec<MeshJob>],
    t: &rc_mesh::blocks::Tables,
    palette: &rc_mesh::blocks::BiomePalette,
    rounds: usize,
    par: bool,
) -> Vec<f64> {
    let mut best = vec![f64::INFINITY; jobs.len()];
    for _ in 0..rounds {
        for (k, js) in jobs.iter().enumerate() {
            let t0 = Instant::now();
            let n: usize = if par {
                js.par_iter().map(|j| black_box(rc_mesh::blocks::mesh(j, t, palette, common::MIN_Y)).len()).sum()
            } else {
                js.iter().map(|j| black_box(rc_mesh::blocks::mesh(j, t, palette, common::MIN_Y)).len()).sum()
            };
            black_box(n);
            best[k] = best[k].min(t0.elapsed().as_secs_f64());
        }
    }
    best
}

fn report(names: &[&str], best: &[f64], par: bool) {
    let base = best[0];
    for (name, b) in names.iter().zip(best) {
        eprintln!(
            "{} {name:>26}: {:8.2} ms ({:+.1} %)",
            if par { "all" } else { "1 thread" },
            b * 1e3,
            (b / base - 1.0) * 100.0
        );
    }
}

#[test]
#[ignore]
fn cull_paths_bench() {
    let Some((_d, cols, t, palette)) = common::load() else { return };
    let mut keys: Vec<(i32, i32)> = cols.keys().copied().collect();
    keys.sort_unstable();
    let old = CullPaths::OLD;
    let props = CullPaths { props: true, ..old };
    // The default without the fill and candidate paths of C2-M08, then each of them alone and
    // all together (the default).
    let before = CullPaths {
        fill_rows: false,
        solid_from_props: false,
        simd_candidates: false,
        border_rows_only: false,
        ..CullPaths::DEFAULT
    };
    let variants: Vec<(&str, CullPaths)> = vec![
        ("old", old),
        ("props", props),
        ("props_pal", CullPaths { palprops: true, ..props }),
        ("props_rows", CullPaths { rows: true, ..props }),
        ("props_pal_rows", CullPaths { palprops: true, rows: true, ..props }),
        ("pile", CullPaths { pile: true, ..old }),
        ("props_rows_pile", CullPaths { rows: true, pile: true, ..props }),
        ("all_old_rule", CullPaths { shape: false, ..CullPaths::ALL }),
        ("shape_only", CullPaths { shape: true, ..old }),
        ("before_rows_fill", before),
        ("rows_fill", CullPaths { fill_rows: true, ..before }),
        ("solid_from_props", CullPaths { solid_from_props: true, ..before }),
        ("simd_candidates", CullPaths { simd_candidates: true, ..before }),
        ("rows_fill_solid", CullPaths { fill_rows: true, solid_from_props: true, ..before }),
        ("rows_fill_simd", CullPaths { fill_rows: true, simd_candidates: true, ..before }),
        ("default_by_stacks", CullPaths { fill_rows: false, ..CullPaths::DEFAULT }),
        ("default", CullPaths::DEFAULT),
        ("all", CullPaths::ALL),
    ];
    let names: Vec<&str> = variants.iter().map(|(n, _)| *n).collect();
    let jobs: Vec<Vec<MeshJob>> =
        variants.iter().map(|(_, p)| keys.iter().map(|&(x, z)| common::job_with(&cols, x, z, *p)).collect()).collect();
    let rounds: usize = rc_data::env_str!("RC_BENCH_ROUNDS").and_then(|v| v.parse().ok()).unwrap_or(5);
    for par in [false, true] {
        report(&names, &best_times(&jobs, &t, &palette, rounds, par), par);
    }
    // Partial remesh of the 9th section on the base of the full mesh: neighbours (8th and 10th)
    // decoded whole, or only their border rows.
    let partial: Vec<(&str, CullPaths)> = vec![
        ("partial_whole_neighbours", CullPaths { border_rows_only: false, ..CullPaths::DEFAULT }),
        ("partial_border_rows", CullPaths::DEFAULT),
    ];
    let mut mask = rc_mesh::wire::Sections::default();
    mask.insert(8);
    let partial_jobs: Vec<Vec<MeshJob>> = partial
        .iter()
        .map(|(_, p)| {
            keys.iter()
                .map(|&(x, z)| {
                    let mut j = common::job_with(&cols, x, z, *p);
                    let base: rc_mesh::blocks::Packed =
                        Arc::new(rc_mesh::blocks::pack_exact(&rc_mesh::blocks::mesh(&j, &t, &palette, common::MIN_Y)));
                    j.reuse = Some((mask, base));
                    j
                })
                .collect()
        })
        .collect();
    let names: Vec<&str> = partial.iter().map(|(n, _)| *n).collect();
    for par in [false, true] {
        report(&names, &best_times(&partial_jobs, &t, &palette, rounds * 4, par), par);
    }
}
