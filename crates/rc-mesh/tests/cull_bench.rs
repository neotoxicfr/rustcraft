//! Micro-bench of the per-block loop paths (C2-M10): all the columns of the capture,
//! variants measured in alternation (same machine load for each), best time of
//! `RC_BENCH_ROUNDS` passes (5 by default), 1 thread then all threads. Launch:
//! `cargo test --release -p rc-mesh --test cull_bench -- --ignored --nocapture`.
// Direct read allowed (bench: setting read at launch, off the measured path).
#![allow(clippy::disallowed_methods)]

mod common;

use std::hint::black_box;
use std::time::Instant;

use rayon::prelude::*;
use rc_mesh::cull::CullPaths;
use rc_mesh::wire::MeshJob;

#[test]
#[ignore]
fn cull_paths_bench() {
    let Some((_d, cols, t, palette)) = common::load() else { return };
    let mut keys: Vec<(i32, i32)> = cols.keys().copied().collect();
    keys.sort_unstable();
    let old = CullPaths::OLD;
    let props = CullPaths { props: true, ..old };
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
        ("default", CullPaths::DEFAULT),
        ("all", CullPaths::ALL),
    ];
    let jobs: Vec<Vec<MeshJob>> =
        variants.iter().map(|(_, p)| keys.iter().map(|&(x, z)| common::job_with(&cols, x, z, *p)).collect()).collect();
    let rounds: usize = rc_data::env_str!("RC_BENCH_ROUNDS").and_then(|v| v.parse().ok()).unwrap_or(5);
    for par in [false, true] {
        let mut best = vec![f64::INFINITY; variants.len()];
        for _ in 0..rounds {
            for (k, js) in jobs.iter().enumerate() {
                let t0 = Instant::now();
                let n: usize = if par {
                    js.par_iter().map(|j| black_box(rc_mesh::blocks::mesh(j, &t, &palette, common::MIN_Y)).len()).sum()
                } else {
                    js.iter().map(|j| black_box(rc_mesh::blocks::mesh(j, &t, &palette, common::MIN_Y)).len()).sum()
                };
                black_box(n);
                best[k] = best[k].min(t0.elapsed().as_secs_f64());
            }
        }
        let base = best[0];
        for ((name, _), b) in variants.iter().zip(&best) {
            eprintln!(
                "{} {name:>16}: {:8.1} ms ({:+.1} %)",
                if par { "all" } else { "1 thread" },
                b * 1e3,
                (b / base - 1.0) * 100.0
            );
        }
    }
}
