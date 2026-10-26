//! Micro-bench of the smoothed light paths (C2-M07), INTERLEAVED variants (order rotated at
//! each pass, best and median of 7) to hold up on a loaded machine. Ignored by default:
//! `cargo test --release -p rc-mesh --test light_bench -- --ignored --nocapture`
//! (`RC_LIGHT_AO=0`: occlusion off). Identical output proven by `tests/light_paths.rs`;
//! here only the time of the full meshing.

mod common;

use std::hint::black_box;
use std::time::Instant;

use rayon::prelude::*;
use rc_mesh::blocks::mesh;
use rc_mesh::light::LightPaths;
use rc_mesh::wire::MeshJob;

#[test]
#[ignore]
fn light_paths_bench() {
    let Some((_d, cols, t, palette)) = common::load() else { return };
    let mut keys: Vec<(i32, i32)> = cols.keys().copied().collect();
    keys.sort_unstable();
    let old = LightPaths::OLD;
    let d = LightPaths { scratch: true, ..old };
    let da = LightPaths { cell: true, ..d };
    let dab = LightPaths { corners: true, ..da };
    let dabc = LightPaths { facetab: true, ..dab };
    let all = LightPaths::ALL;
    let variants = [
        ("old", old),
        ("D kept per thread", d),
        ("D+A", da),
        ("D+A+B", dab),
        ("D+A+B+C", dabc),
        ("A alone", LightPaths { cell: true, ..old }),
        ("B alone", LightPaths { corners: true, ..old }),
        ("C alone", LightPaths { facetab: true, ..old }),
        ("E alone", LightPaths { cell8: true, ..old }),
        ("F alone", LightPaths { fluid: true, ..old }),
        ("G alone", LightPaths { neigh: true, ..old }),
        ("DABC+E", LightPaths { cell8: true, ..dabc }),
        ("DABC+F", LightPaths { fluid: true, ..dabc }),
        ("DABC+G", LightPaths { neigh: true, ..dabc }),
        ("DABC+E+F", LightPaths { cell8: true, fluid: true, ..dabc }),
        ("all except A", LightPaths { cell: false, ..all }),
        ("all except B", LightPaths { corners: false, ..all }),
        ("all except C", LightPaths { facetab: false, ..all }),
        ("all except D", LightPaths { scratch: false, ..all }),
        ("all except E", LightPaths { cell8: false, ..all }),
        ("all except F", LightPaths { fluid: false, ..all }),
        ("all", all),
        ("default", LightPaths::DEFAULT),
    ];
    let ao = rc_data::env_str!("RC_LIGHT_AO").is_none_or(|v| v != "0");
    // 1 thread: one column in 4 (312); all threads: all of them.
    let jobs = |p: LightPaths, step: usize| -> Vec<MeshJob> {
        keys.iter()
            .step_by(step)
            .map(|&(x, z)| {
                let mut j = common::job(&cols, x, z);
                j.opts.ao = ao;
                j.light = p;
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
    eprintln!(
        "occlusion {ao}, ms (best / median), 1 thread on {} columns, all threads on {}; face geometry {} B:",
        one[0].len(),
        par[0].len(),
        t.light_faces.heap_bytes()
    );
    let (b0, _) = stat(&mut t1[0].clone());
    let (p0, _) = stat(&mut tp[0].clone());
    for i in 0..n {
        let (a, am) = stat(&mut t1[i]);
        let (b, bm) = stat(&mut tp[i]);
        eprintln!(
            "  {:<18} 1 thread {a:8.1} / {am:8.1} ({:+5.1} %)   all {b:7.1} / {bm:7.1} ({:+5.1} %)",
            variants[i].0,
            (a / b0 - 1.0) * 100.0,
            (b / p0 - 1.0) * 100.0
        );
    }
}
