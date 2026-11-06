//! Micro-bench of the smoothed light paths, INTERLEAVED variants (order rotated at each pass,
//! best and median of 15) to hold up on a loaded machine. Ignored by default:
//! `cargo test --release -p rc-mesh --test light_bench -- --ignored --nocapture`
//! (`RC_LIGHT_AO=0`: occlusion off). Identical output proven by `tests/light_paths.rs`;
//! here only the time of the full meshing. `light_paths_bench` compares the paths of the cell
//! cache (D, A, B…); `addressed_reads_bench` the addressed reads and the zero sheets on top of
//! D + A + B.

mod common;

use std::hint::black_box;
use std::time::Instant;

use rayon::prelude::*;
use rc_mesh::blocks::mesh;
use rc_mesh::light::LightPaths;
use rc_mesh::wire::MeshJob;

#[test]
#[ignore]
fn addressed_reads_bench() {
    let dab = LightPaths { cell: true, corners: true, scratch: true, ..LightPaths::OLD };
    let new = LightPaths::ALL;
    let new = LightPaths { facetab: false, cell8: false, fluid: false, neigh: false, ..new };
    let with = |f: fn(&mut LightPaths)| {
        let mut p = dab;
        f(&mut p);
        p
    };
    let without = |f: fn(&mut LightPaths)| {
        let mut p = new;
        f(&mut p);
        p
    };
    // Percentages against the first variant: the former default.
    let variants = [
        ("D+A+B", dab),
        ("old", LightPaths::OLD),
        ("+offsets", with(|p| p.fixed_offsets = true)),
        ("+prop bits", with(|p| p.prop_bits = true)),
        ("+raw grid", with(|p| p.raw_grid = true)),
        ("+zero sheets", with(|p| p.zero_layers = true)),
        ("+offsets+props", with(|p| (p.fixed_offsets, p.prop_bits) = (true, true))),
        ("+offsets+raw", with(|p| (p.fixed_offsets, p.raw_grid) = (true, true))),
        ("+props+raw", with(|p| (p.prop_bits, p.raw_grid) = (true, true))),
        ("+all four", new),
        ("four but offsets", without(|p| p.fixed_offsets = false)),
        ("four but props", without(|p| p.prop_bits = false)),
        ("four but raw", without(|p| p.raw_grid = false)),
        ("four but zero", without(|p| p.zero_layers = false)),
        ("four, raw from 64", without(|p| p.raw_grid_min = 64)),
        ("four, raw from 256", without(|p| p.raw_grid_min = 256)),
        ("four, raw from 1024", without(|p| p.raw_grid_min = 1024)),
        ("default", LightPaths::DEFAULT),
    ];
    run_interleaved(&variants);
}

#[test]
#[ignore]
fn light_paths_bench() {
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
    run_interleaved(&variants);
}

/// Processor cycles spent by the calling thread so far (Windows; 0 elsewhere). Unlike the wall
/// clock, a thread put aside by the system for another process counts nothing meanwhile: the
/// 1-thread figure holds up better on a loaded machine.
#[cfg(windows)]
#[allow(unsafe_code)]
fn thread_cycles() -> u64 {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThread() -> isize;
        fn QueryThreadCycleTime(thread: isize, cycles: *mut u64) -> i32;
    }
    let mut c = 0u64;
    // SAFETY: the pseudo handle of the current thread is always valid and `c` outlives the call.
    unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut c) };
    c
}

#[cfg(not(windows))]
fn thread_cycles() -> u64 {
    0
}

/// Passes per variant: many short ones, whose best holds up better on a loaded machine than the
/// best of a few long ones.
const PASSES: usize = 15;

/// Meshes the capture with each variant, interleaved, `PASSES` passes: 1 thread on one column in
/// 8 (wall time and thread cycles), all threads on every column; prints best and median against
/// the first variant.
fn run_interleaved(variants: &[(&str, LightPaths)]) {
    let Some((_d, cols, t, palette)) = common::load() else { return };
    let mut keys: Vec<(i32, i32)> = cols.keys().copied().collect();
    keys.sort_unstable();
    let ao = rc_data::env_str!("RC_LIGHT_AO").is_none_or(|v| v != "0");
    // 1 thread: one column in 8 (156); all threads: all of them.
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
    let one: Vec<Vec<MeshJob>> = variants.iter().map(|v| jobs(v.1, 8)).collect();
    let par: Vec<Vec<MeshJob>> = variants.iter().map(|v| jobs(v.1, 1)).collect();
    let n = variants.len();
    let (mut t1, mut tp, mut c1) = (vec![Vec::new(); n], vec![Vec::new(); n], vec![Vec::new(); n]);
    for pass in 0..PASSES {
        for k in 0..n {
            let i = (k + pass * 3) % n;
            let (s, c) = (Instant::now(), thread_cycles());
            black_box(one[i].iter().map(|j| mesh(j, &t, &palette, common::MIN_Y).len()).sum::<usize>());
            t1[i].push(s.elapsed().as_secs_f64() * 1e3);
            c1[i].push((thread_cycles() - c) as f64 * 1e-6);
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
        "occlusion {ao}, best / median, 1 thread on {} columns (ms, Mcycles), all threads on {} (ms); face geometry {} B:",
        one[0].len(),
        par[0].len(),
        t.light_faces.heap_bytes()
    );
    let (b0, _) = stat(&mut t1[0].clone());
    let (k0, _) = stat(&mut c1[0].clone());
    let (p0, _) = stat(&mut tp[0].clone());
    for i in 0..n {
        let (a, am) = stat(&mut t1[i]);
        let (k, km) = stat(&mut c1[i]);
        let (b, bm) = stat(&mut tp[i]);
        eprintln!(
            "  {:<20} 1 thread {a:7.1} / {am:7.1} ({:+5.1} %) {k:7.0} / {km:7.0} ({:+5.1} %)   all {b:6.1} / {bm:6.1} ({:+5.1} %)",
            variants[i].0,
            (a / b0 - 1.0) * 100.0,
            (k / k0 - 1.0) * 100.0,
            (b / p0 - 1.0) * 100.0
        );
    }
}
