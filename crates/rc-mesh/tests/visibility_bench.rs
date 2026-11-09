//! Micro-bench of the visibility paths on the reference capture: face communication of the
//! 13,768 non-empty sections through each dilation path (1 thread, then 1, 2, 4… threads up to
//! the logical cores), and the walk from five eyes through each combination of its ray paths. Variants
//! measured in alternation (same machine load for each), best time of `RC_BENCH_ROUNDS` passes
//! (7 by default). Launch:
//! `cargo test --release -p rc-mesh --test visibility_bench -- --ignored --nocapture`.

mod common;

use hashbrown::HashMap;
use std::hint::black_box;
use std::time::Instant;

use common::capture_solid_masks as capture_masks;
use rayon::prelude::*;
use rc_mesh::visibility::{DilationPaths, Graph, VIS_ALL, Vis, WalkPaths, section_vis_with};

fn rounds() -> usize {
    rc_data::env_str!("RC_BENCH_ROUNDS").and_then(|v| v.parse().ok()).unwrap_or(7)
}

/// Best time (ms) of `run` for each variant, the variants alternated within every pass.
fn best_ms<T: Copy>(variants: &[(&str, T)], mut run: impl FnMut(T)) -> Vec<f64> {
    let mut best = vec![f64::INFINITY; variants.len()];
    for _ in 0..rounds() {
        for (k, (_, v)) in variants.iter().enumerate() {
            let t0 = Instant::now();
            run(*v);
            best[k] = best[k].min(t0.elapsed().as_secs_f64() * 1e3);
        }
    }
    best
}

fn report<T>(what: &str, variants: &[(&str, T)], best: &[f64]) {
    for ((name, _), b) in variants.iter().zip(best) {
        eprintln!("{what} {name:>22}: {b:9.3} ms ({:+.1} %)", (b / best[0] - 1.0) * 100.0);
    }
}

#[test]
#[ignore]
fn section_vis_paths_bench() {
    let Some(cols) = capture_masks() else { return };
    let sections: Vec<[u64; 64]> = cols.iter().flat_map(|(_, m)| m.iter().flatten().copied()).collect();
    let variants: Vec<(String, DilationPaths)> = DilationPaths::every_combination()
        .into_iter()
        .map(|p| {
            let names = [(p.word_queue, "queue"), (p.stop_at_six_faces, "stop")];
            let name: Vec<&str> = names.iter().filter(|(on, _)| *on).map(|(_, n)| *n).collect();
            (if name.is_empty() { "old".to_string() } else { name.join("+") }, p)
        })
        .collect();
    let variants: Vec<(&str, DilationPaths)> = variants.iter().map(|(n, p)| (n.as_str(), *p)).collect();
    let one = |p| black_box(sections.iter().fold(0, |a, s| a ^ section_vis_with(s, p)));
    report(
        &format!("{} sections, 1 thread", sections.len()),
        &variants,
        &best_ms(&variants, |p| {
            one(p);
        }),
    );
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut threads = 1;
    while threads <= cores {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("pool");
        let all = |p| pool.install(|| sections.par_iter().map(|s| section_vis_with(s, p)).reduce(|| 0, |a, v| a ^ v));
        report(
            &format!("{threads:>2} threads"),
            &variants,
            &best_ms(&variants, |p| {
                black_box(all(p));
            }),
        );
        threads *= 2;
    }
}

#[test]
#[ignore]
fn walk_paths_bench() {
    let Some(cols) = capture_masks() else { return };
    let vis: HashMap<(i32, i32), Vec<Vis>> = cols
        .iter()
        .map(|(k, m)| (*k, m.iter().map(|s| s.map_or(VIS_ALL, |b| section_vis_with(&b, DilationPaths::ALL))).collect()))
        .collect();
    let n = vis.len() as i32;
    let (sx, sz) = vis.keys().fold((0, 0), |a, k| (a.0 + k.0, a.1 + k.1));
    let (cx, cz) = (f64::from(sx / n * 16), f64::from(sz / n * 16));
    let eyes = [
        ("ground", [cx + 8.3, 80.6, cz + 8.7]),
        ("cave", [cx + 3.5, -20.2, cz + 12.1]),
        ("high_air", [cx + 8.3, 190.4, cz + 8.7]),
        ("above_world", [cx + 1.1, 330.0, cz + 2.9]),
        ("off_centre", [cx + 150.6, 70.3, cz - 120.4]),
    ];
    let column = |x, z| vis.get(&(x, z)).map(Vec::as_slice);
    let names = ["old", "ray_per_axis", "clear_ray_per_corner", "both"];
    let variants: Vec<(&str, usize)> = names.iter().copied().zip(0..).collect();
    let mut graphs = WalkPaths::every_combination().map(Graph::with_paths);
    for (name, eye) in eyes {
        let best = best_ms(&variants, |k| {
            black_box(graphs[k].run(column, eye, -64, 24, 32, true).len());
        });
        report(&format!("walk {name:>11}"), &variants, &best);
    }
}
