//! Micro-bench of the smoothed light paths, INTERLEAVED variants (order rotated at each pass,
//! best and median of 15) to hold up on a loaded machine. Ignored by default:
//! `cargo test --release -p rc-mesh --test light_bench -- --ignored --nocapture`
//! (`RC_LIGHT_AO=0`: occlusion off). Identical output proven by `tests/light_paths.rs`;
//! here only the time of the full meshing. `light_paths_bench` compares the paths of the cell
//! cache (D, A, B…); `addressed_reads_bench` the addressed reads and the zero sheets on top of
//! D + A + B; `zero_sheets_and_raw_threshold_bench` the zero sheets and the raw grid's threshold
//! on top of the default, with the default repeated as noise reference;
//! `default_off_switches_bench` the paths left off by the addressed reads, alone and together.

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
    let new = LightPaths { facetab: false, cell8: false, fluid: false, ..new };
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
        ("DABC+E", LightPaths { cell8: true, ..dabc }),
        ("DABC+F", LightPaths { fluid: true, ..dabc }),
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
        t.build_light_faces().heap_bytes()
    );
    let (b0, bm0) = stat(&mut t1[0].clone());
    let (k0, _) = stat(&mut c1[0].clone());
    let (p0, pm0) = stat(&mut tp[0].clone());
    let pct = |x: f64, reference: f64| (x / reference - 1.0) * 100.0;
    for i in 0..n {
        let (a, am) = stat(&mut t1[i]);
        let (k, km) = stat(&mut c1[i]);
        let (b, bm) = stat(&mut tp[i]);
        eprintln!(
            "  {:<22} 1 thread {a:7.1} / {am:7.1} ({:+5.1} / {:+5.1} %) {k:7.0} / {km:7.0} ({:+5.1} %)   all {b:6.1} / {bm:6.1} ({:+5.1} / {:+5.1} %)",
            variants[i].0,
            pct(a, b0),
            pct(am, bm0),
            pct(k, k0),
            pct(b, p0),
            pct(bm, pm0)
        );
    }
}

/// The paths that stayed off after the addressed reads (6 B cells, face geometry, fluids and flat
/// light through the section, zero sheets), each alone and all together on top of the default of
/// the addressed reads, which is repeated three times as noise reference.
#[test]
#[ignore]
fn default_off_switches_bench() {
    let before = LightPaths {
        cell: true,
        corners: true,
        scratch: true,
        fixed_offsets: true,
        prop_bits: true,
        raw_grid: true,
        ..LightPaths::OLD
    };
    let all_four = LightPaths { cell8: true, facetab: true, fluid: true, zero_layers: true, ..before };
    let variants = [
        ("addressed default", before),
        ("addressed again", before),
        ("+6 B cells", LightPaths { cell8: true, ..before }),
        ("+face geometry", LightPaths { facetab: true, ..before }),
        ("+fluids, flat", LightPaths { fluid: true, ..before }),
        ("+zero sheets", LightPaths { zero_layers: true, ..before }),
        ("all four", all_four),
        ("all four but cells", LightPaths { cell8: false, ..all_four }),
        ("all four but geometry", LightPaths { facetab: false, ..all_four }),
        ("all four but fluids", LightPaths { fluid: false, ..all_four }),
        ("all four but zero", LightPaths { zero_layers: false, ..all_four }),
        ("default", LightPaths::DEFAULT),
        ("addressed third time", before),
    ];
    run_interleaved(&variants);
}

/// Deterministic count behind the zero sheets (no clock): of the capture's non-empty sections,
/// those whose 27 layers are one constant once the shared sheets read as levels (uniform raw
/// grid), and those whose grid one thread already holds when it meshes the columns one after the
/// other, bottom to top (the candidate threshold left aside).
#[test]
#[ignore]
fn uniform_sections_count() {
    let Some((_d, cols, t, _palette)) = common::load() else { return };
    let mut keys: Vec<(i32, i32)> = cols.keys().copied().collect();
    keys.sort_unstable();
    let (mut sections, mut uniform, mut held, mut dark) = (0usize, 0usize, 0usize, 0usize);
    // Sections by number of constant layers among their 27 (0, 1-8, 9-17, 18-26, 27).
    let mut by_constants = [0usize; 5];
    // Value of every cell of the thread's grid after its last fill (`None`: mixed).
    let mut grid = None;
    for &(x, z) in &keys {
        let job = common::job(&cols, x, z);
        let view = rc_mesh::light::LightView::new(&job, &t.light_info);
        for (si, s) in job.center.sections.iter().enumerate() {
            if s.is_empty() {
                continue;
            }
            sections += 1;
            let (constants, u) = view.section_light_constants(si as i32 * 16);
            by_constants[match constants {
                0 => 0,
                1..=8 => 1,
                9..=17 => 2,
                18..=26 => 3,
                _ => 4,
            }] += 1;
            uniform += usize::from(u.is_some());
            dark += usize::from(u == Some(0));
            held += usize::from(u.is_some() && u == grid);
            grid = u;
        }
    }
    eprintln!(
        "{} columns, {sections} non-empty sections: {uniform} uniform ({dark} dark), {held} of them already held by the thread's grid",
        keys.len()
    );
    eprintln!("sections by constant layers among 27: none {}, 1-8 {}, 9-17 {}, 18-26 {}, all {}", by_constants[0], by_constants[1], by_constants[2], by_constants[3], by_constants[4]);
    assert!(sections > 0);
}

/// The two settings the first bench could not decide: zero sheets on top of the default, and the
/// raw grid's threshold. The default is repeated to show the bench's own noise.
#[test]
#[ignore]
fn zero_sheets_and_raw_threshold_bench() {
    let d = LightPaths::DEFAULT;
    let variants = [
        ("default", d),
        ("default again", d),
        ("+zero sheets", LightPaths { zero_layers: true, ..d }),
        ("raw from 0", LightPaths { raw_grid_min: 0, ..d }),
        ("raw from 256", LightPaths { raw_grid_min: 256, ..d }),
        ("raw from 1024", LightPaths { raw_grid_min: 1024, ..d }),
        ("default third time", d),
    ];
    run_interleaved(&variants);
}
