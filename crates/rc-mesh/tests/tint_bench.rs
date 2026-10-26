//! Micro-bench of the biome tint paths (C2-M06), INTERLEAVED variants (order rotated at
//! each pass, best and median of 7) to hold up on a loaded machine. Ignored by default:
//! `cargo test --release -p rc-mesh --test tint_bench -- --ignored --nocapture`.
//! Identical output proven by `tests/digest.rs`; here only the time of the full meshing.

mod common;

use std::hint::black_box;
use std::time::Instant;

use rayon::prelude::*;
use rc_mesh::blocks::{TintPaths, mesh};
use rc_mesh::wire::MeshJob;

#[test]
#[ignore]
fn tint_paths_bench() {
    let Some((_d, cols, t, palette)) = common::load() else { return };
    let mut keys: Vec<(i32, i32)> = cols.keys().copied().collect();
    keys.sort_unstable();
    let (all, old) = (TintPaths::ALL, TintPaths::OLD);
    let variants = [
        ("old", old),
        ("uni alone", TintPaths { uni: true, ..old }),
        ("lattice alone", TintPaths { lattice: true, ..old }),
        ("grid alone", TintPaths { grid: true, ..old }),
        ("idx alone", TintPaths { idx: true, ..old }),
        ("sums alone", TintPaths { boxsum: all.boxsum, ..old }),
        ("all except uni", TintPaths { uni: false, ..all }),
        ("all except lattice", TintPaths { lattice: false, ..all }),
        ("all except grid", TintPaths { grid: false, ..all }),
        ("all except idx", TintPaths { idx: false, ..all }),
        ("all except sums", TintPaths { boxsum: 0, ..all }),
        ("all, sums from 1", TintPaths { boxsum: 1, ..all }),
        ("all, sums from 8", TintPaths { boxsum: 8, ..all }),
        ("all, sums from 128", TintPaths { boxsum: 128, ..all }),
        ("all", all),
    ];
    let rad: i32 = rc_data::env_str!("RC_TINT_BLEND").and_then(|v| v.parse().ok()).unwrap_or(2);
    // 1 thread: one column in 4 (312); all threads: all of them.
    let jobs = |p: TintPaths, step: usize| -> Vec<MeshJob> {
        keys.iter()
            .step_by(step)
            .map(|&(x, z)| {
                let mut j = common::job(&cols, x, z);
                j.opts.blend = rad;
                j.tint = p;
                j
            })
            .collect()
    };
    let one: Vec<Vec<MeshJob>> = variants.iter().map(|v| jobs(v.1, 4)).collect();
    let par: Vec<Vec<MeshJob>> = variants.iter().map(|v| jobs(v.1, 1)).collect();
    let n = variants.len();
    let (mut t1, mut tp) = (vec![Vec::new(); n], vec![Vec::new(); n]);
    for pass in 0..7 {
        for k in 0..n {
            let i = (k + pass * 3) % n;
            let s = Instant::now();
            black_box(one[i].iter().map(|j| mesh(j, &t, &palette, common::MIN_Y).len()).sum::<usize>());
            t1[i].push(s.elapsed().as_secs_f64() * 1e3);
            let s = Instant::now();
            black_box(par[i].par_iter().map(|j| mesh(j, &t, &palette, common::MIN_Y).len()).sum::<usize>());
            tp[i].push(s.elapsed().as_secs_f64() * 1e3);
        }
    }
    let stat = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        (v[0], v[v.len() / 2])
    };
    eprintln!("radius {rad}, ms (best / median), 1 thread on {} columns, all threads on {}:", one[0].len(), par[0].len());
    let (b0, _) = stat(&mut t1[0].clone());
    let (p0, _) = stat(&mut tp[0].clone());
    for i in 0..n {
        let (a, am) = stat(&mut t1[i]);
        let (b, bm) = stat(&mut tp[i]);
        eprintln!(
            "  {:<22} 1 thread {a:8.1} / {am:8.1} ({:+5.1} %)   all {b:7.1} / {bm:7.1} ({:+5.1} %)",
            variants[i].0,
            (a / b0 - 1.0) * 100.0,
            (b / p0 - 1.0) * 100.0
        );
    }
}
