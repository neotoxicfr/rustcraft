//! Density evaluator (C1-M05, C1-M06, C1-M13): new paths (`sampler::Opts::NEW`) against the old
//! code (`Opts::OLD`) bit for bit, on the router roots and the vein functions; a context kept from
//! one chunk to the next against a fresh one; cache reads of a sub-lattice; evaluation counts;
//! micro-benches.
//! Without game data, the tests are skipped (explicit failure under `RUSTCRAFT_OPT_REQUIRE=1`).
//!
//! Benches (quiet machine): `cargo test --release -p rc-worldgen --test sampler_opt -- --ignored --nocapture`.

mod common;

use std::alloc::{GlobalAlloc, Layout};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use rayon::prelude::*;
use rc_worldgen::sampler::{EvalCounts, Opts, S, Sid};
use rc_worldgen::terrain::{BiomeCache, Stage, TerrainGenerator};
use rc_worldgen::{Ctx, Generator, Volume};

static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

/// Allocation counter of the test binary (per-chunk allocation bench), around the client's
/// allocator (mimalloc), so that the counts and timings are those of the game.
#[allow(unsafe_code)]
mod counting {
    use super::{ALLOCS, BYTES, GlobalAlloc, Layout, Ordering};
    use mimalloc::MiMalloc;

    struct Counting;

    fn count(bytes: usize) {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        BYTES.fetch_add(bytes as u64, Ordering::Relaxed);
    }

    // SAFETY: delegates as is to mimalloc; only two counters are added.
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            count(l.size());
            // SAFETY: same preconditions as the caller.
            unsafe { MiMalloc.alloc(l) }
        }
        unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
            count(l.size());
            // SAFETY: same preconditions as the caller.
            unsafe { MiMalloc.alloc_zeroed(l) }
        }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            // SAFETY: `p` comes from this allocator, hence from mimalloc.
            unsafe { MiMalloc.dealloc(p, l) }
        }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
            count(n);
            // SAFETY: same preconditions as the caller.
            unsafe { MiMalloc.realloc(p, l, n) }
        }
    }

    #[global_allocator]
    static A: Counting = Counting;

    /// Index of `mi_option_page_commit_on_demand` in the `mi_option_t` enumeration of the linked
    /// library (libmimalloc-sys 0.1.49, mimalloc v3, `include/mimalloc.h`): to recheck on every
    /// bump, together with the same constant of the client (`apps/client/src/debug_sys.rs`).
    const MI_OPTION_PAGE_COMMIT_ON_DEMAND: i32 = 40;

    /// Setting of the allocator in the client (`debug_sys::commit_on_demand`): pages committed
    /// on demand.
    pub fn client_options() {
        unsafe extern "C" {
            fn mi_option_set(option: i32, value: std::ffi::c_long);
        }
        // SAFETY: function of the linked mimalloc library (global allocator); the value 1 is documented.
        unsafe { mi_option_set(MI_OPTION_PAGE_COMMIT_ON_DEMAND, 1) }
    }
}

/// Page faults and private bytes of the process (Windows; `None` elsewhere).
#[allow(unsafe_code)]
mod process {
    #[cfg(windows)]
    pub fn memory() -> Option<(u64, u64)> {
        // Layout of the system structure: only two fields are read.
        #[allow(dead_code)]
        #[repr(C)]
        #[derive(Default)]
        struct Counters {
            cb: u32,
            page_fault_count: u32,
            peak_working_set_size: usize,
            working_set_size: usize,
            quota_peak_paged_pool_usage: usize,
            quota_paged_pool_usage: usize,
            quota_peak_non_paged_pool_usage: usize,
            quota_non_paged_pool_usage: usize,
            pagefile_usage: usize,
            peak_pagefile_usage: usize,
            private_usage: usize,
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentProcess() -> isize;
            fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, cb: u32) -> i32;
        }
        let mut c = Counters { cb: size_of::<Counters>() as u32, ..Counters::default() };
        // SAFETY: documented call of kernel32 on the current process with a structure of the
        // size given in `cb` (PROCESS_MEMORY_COUNTERS_EX).
        let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) } != 0;
        ok.then_some((u64::from(c.page_fault_count), c.private_usage as u64))
    }

    #[cfg(not(windows))]
    pub fn memory() -> Option<(u64, u64)> {
        None
    }
}

const SEED: i64 = 20_261_011;

/// Each path alone, then the combinations with `range_yvol`, then all: a faulty track is
/// named by its own failure.
const VARIANTS: [(&str, Opts); 13] = [
    ("shifted_noise_in_batches", Opts { shifted_noise_in_batches: true, ..Opts::OLD }),
    ("column_memo_y_independent", Opts { column_memo_y_independent: true, ..Opts::OLD }),
    ("memo+sublattice", Opts { column_memo_y_independent: true, sublattice_cache_read: true, ..Opts::OLD }),
    ("range_yvol", Opts { range_yvol: true, ..Opts::OLD }),
    ("spline_stack", Opts { spline_stack: true, ..Opts::OLD }),
    ("take_without_zeroing", Opts { take_without_zeroing: true, ..Opts::OLD }),
    ("best_fit_buffers", Opts { best_fit_buffers: true, ..Opts::OLD }),
    ("sublattice_cache_read", Opts { sublattice_cache_read: true, ..Opts::OLD }),
    ("spline_inputs_on_stack", Opts { spline_inputs_on_stack: true, ..Opts::OLD }),
    ("floor_java", Opts { floor_java: true, ..Opts::OLD }),
    ("yvol+spline", Opts { range_yvol: true, spline_stack: true, ..Opts::OLD }),
    ("yvol+sublattice", Opts { range_yvol: true, sublattice_cache_read: true, ..Opts::OLD }),
    ("all", Opts::NEW),
];

/// Contexts of the new paths: each variant, plus all the paths with every taken buffer filled
/// with NaN (a caller that reads an element it did not write changes the result).
fn variant_contexts(cached: bool) -> Vec<(&'static str, Ctx)> {
    let mk = |o: Opts| if cached { Ctx::cached_with(o) } else { Ctx::uncached_with(o) };
    let mut v: Vec<(&str, Ctx)> = VARIANTS.iter().map(|(label, o)| (*label, mk(*o))).collect();
    let mut poisoned = mk(Opts::NEW);
    poisoned.poison_taken_buffers();
    v.push(("all, taken buffers poisoned", poisoned));
    v
}

/// Pseudo-random chunks (near, far, negative), reproducible.
fn chunks(n: usize) -> Vec<(i32, i32)> {
    let mut s: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut next = || {
        s = s.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (s >> 33) as i32
    };
    (0..n)
        .map(|i| match i % 4 {
            0 => (next() % 64 - 32, next() % 64 - 32),
            1 => (next() % 4096 - 2048, next() % 4096 - 2048),
            2 => (next() % 1_800_000 - 900_000, next() % 1_800_000 - 900_000),
            _ => (next() % 20_000, -(next() % 20_000)),
        })
        .collect()
}

fn roots(g: &mut Generator, veins: bool) -> Vec<(String, Sid)> {
    let r = g.router;
    let mut v: Vec<(String, Sid)> = [
        ("temperature", r.temperature),
        ("vegetation", r.vegetation),
        ("continents", r.continents),
        ("erosion", r.erosion),
        ("depth", r.depth),
        ("ridges", r.ridges),
        ("chunk_surface_level", r.chunk_surface_level),
        ("final_density", r.final_density),
    ]
    .into_iter()
    .map(|(n, s)| (n.to_owned(), s))
    .collect();
    if veins {
        for id in ["toggle", "mask", "copper_density", "iron_density", "richness", "gap"] {
            let name = format!("minecraft:overworld/ore_vein/{id}");
            let s = g.compile_named(&name).unwrap();
            v.push((name, s));
        }
        let s = g.compile_named("minecraft:overworld/caves/noodle").unwrap();
        v.push(("noodle".into(), s));
    }
    v
}

/// Volumes of a chunk: whole blocks, grid of cells, partial height, unaligned bottom,
/// isolated column, flat layer.
fn volumes(g: &Generator, cx: i32, cz: i32) -> Vec<Volume> {
    let (x, z, y0, h) = (cx * 16, cz * 16, g.settings.min_y, g.settings.height);
    vec![
        Volume::new(16, h, 16, x, y0, z),
        Volume::stepped(5, h / 8 + 1, 5, x, y0, z, 4, 8, 4),
        Volume::new(16, h.min(200), 16, x, y0, z),
        Volume::new(16, 100, 16, x, y0 + 3, z),
        Volume::new(1, h, 1, x + 5, y0, z + 9),
        Volume::new(16, 1, 16, x, 0, z),
    ]
}

fn first_diff(a: &[f32], b: &[f32]) -> Option<(usize, usize)> {
    let bad = a.iter().zip(b).filter(|(x, y)| x.to_bits() != y.to_bits()).count();
    (bad > 0).then(|| (bad, a.iter().zip(b).position(|(x, y)| x.to_bits() != y.to_bits()).unwrap()))
}

/// Volume path: each root, each volume, context with caches (roots evaluated one after
/// another in the same context, like the filling) and without cache.
fn check_volumes(preset: &str, n: usize, veins: bool) {
    let Some(pack) = common::data_pack() else { return };
    let mut g = Generator::new(&pack, preset, SEED).unwrap();
    let roots = roots(&mut g, veins);
    let mut failures = Vec::new();
    let mut compared = 0usize;
    for (k, (cx, cz)) in chunks(n).into_iter().enumerate() {
        let vols = volumes(&g, cx, cz);
        // Fewer side volumes to fit the test time; the whole volume always.
        let vols: Vec<Volume> = if k % 8 == 0 { vols } else { vec![vols[0], vols[1 + k % 5]] };
        for cached in [true, false] {
            let mut old_ctx = if cached { Ctx::cached_with(Opts::OLD) } else { Ctx::uncached_with(Opts::OLD) };
            let mut new_ctx = variant_contexts(cached);
            for v in &vols {
                for (name, s) in &roots {
                    let want = g.volume(*s, &mut old_ctx, v);
                    for (label, ctx) in &mut new_ctx {
                        let got = g.volume(*s, ctx, v);
                        if let Some((bad, i)) = first_diff(&got, &want) {
                            failures.push(format!(
                                "{preset} {label} {name} chunk {cx},{cz} {v:?} caches {cached}: {bad} differences, \
                                 first {i}: {} ≠ {}",
                                got[i], want[i]
                            ));
                        }
                        compared += got.len();
                        // Buffer returned: a later buffer taken by the evaluator may be this one, dirty.
                        ctx.recycle(got);
                    }
                }
            }
        }
        assert!(failures.len() < 50, "{}", failures.join("\n"));
    }
    eprintln!("{preset}: {compared} values compared");
    assert!(failures.is_empty(), "{} differences:\n{}", failures.len(), failures.join("\n"));
}

#[test]
fn volume_overworld_200_chunks() {
    check_volumes("overworld", 200, true);
}

#[test]
fn volume_other_presets() {
    for p in ["large_biomes", "amplified", "nether", "end", "caves", "floating_islands"] {
        check_volumes(p, 24, false);
    }
}

/// Sub-volume aligned on the cells == rows of the whole volume (premise of the restriction),
/// and, conversely, an unaligned start does change values (the test can fail).
#[test]
fn rows_subvolume_premise() {
    let Some(pack) = common::data_pack() else { return };
    let mut g = Generator::new(&pack, "overworld", SEED).unwrap();
    let roots = roots(&mut g, true);
    let mut unaligned_differs = false;
    for (cx, cz) in chunks(16) {
        let full = Volume::new(16, 384, 16, cx * 16, -64, cz * 16);
        for (name, s) in &roots {
            let want = g.volume(*s, &mut Ctx::uncached_with(Opts::OLD), &full);
            // Starts unaligned at 3 and 5 rows from the cell bottom (at 1 row, `lo + step` is
            // exactly the first addition: no difference possible).
            for (r0, r1) in [(8, 56), (64, 120), (0, 50), (13 * 8, 384), (13, 61), (75, 130)] {
                let sub = Volume::new(16, r1 - r0, 16, full.x0, full.y0 + r0, full.z0);
                let got = g.volume(*s, &mut Ctx::uncached_with(Opts::OLD), &sub);
                let n = (r1 - r0) as usize;
                let same = got
                    .chunks_exact(n)
                    .enumerate()
                    .all(|(c, col)| col.iter().zip(&want[c * 384 + r0 as usize..]).all(|(a, b)| a.to_bits() == b.to_bits()));
                if r0 % 8 == 0 {
                    assert!(same, "{name} chunk {cx},{cz} rows {r0}..{r1}: aligned sub-volume ≠ whole volume");
                } else if !same {
                    unaligned_differs = true;
                }
            }
        }
    }
    assert!(unaligned_differs, "an unaligned start should change the interpolation (test without power)");
}

/// Point path: 10⁶ random positions + extremes, climate roots; 10⁴ for the
/// heavy roots (final density, surface).
#[test]
fn point_values_old_new() {
    let Some(pack) = common::data_pack() else { return };
    let mut g = Generator::new(&pack, "overworld", SEED).unwrap();
    let roots = roots(&mut g, true);
    let mut s: u64 = 12_345;
    let mut next = || {
        s = s.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        (s >> 32) as u32 as i32
    };
    let mut pts: Vec<[i32; 3]> = Vec::new();
    for e in [-30_000_000, -29_999_999, -1, 0, 1, 29_999_999, 30_000_000] {
        for y in [-2048, -64, 0, 63, 320, 2047] {
            pts.push([e, y, -e]);
            pts.push([e, y, e]);
        }
    }
    while pts.len() < 1_000_000 {
        pts.push([next() % 30_000_000, next() % 400 - 80, next() % 30_000_000]);
    }
    let mut failures = Vec::new();
    for (name, sid) in &roots {
        let heavy = matches!(name.as_str(), "final_density" | "chunk_surface_level") || name.contains("ore_vein") || name == "noodle";
        let n = if heavy { 10_000 } else { pts.len() };
        let (mut old, mut new) = (Ctx::uncached_with(Opts::OLD), Ctx::uncached_with(Opts::NEW));
        for p in &pts[..n] {
            let a = g.value(*sid, &mut old, p[0], p[1], p[2]);
            let b = g.value(*sid, &mut new, p[0], p[1], p[2]);
            if a.to_bits() != b.to_bits() {
                failures.push(format!("{name} {p:?} : {b} ≠ {a}"));
                break;
            }
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Context with caches, first filled by the final density volume, then queried point by
/// point on each cache of the program: the values read (cache buffer or point computation)
/// must be those of the old path bit for bit, even where a branch restricted in y only
/// filled its cache for a few rows.
#[test]
fn point_after_volume_on_every_cache() {
    let Some(pack) = common::data_pack() else { return };
    let g = Generator::new(&pack, "overworld", SEED).unwrap();
    let caches: Vec<Sid> = (0..g.program.nodes.len() as Sid)
        .filter(|&s| matches!(g.program.nodes[s as usize], rc_worldgen::sampler::S::Cache { .. }))
        .collect();
    assert!(!caches.is_empty(), "no cache in the program");
    let mut failures = Vec::new();
    let mut compared = 0usize;
    for (cx, cz) in chunks(6) {
        let v = g.chunk_volume(cx, cz);
        let (mut old, mut new) = (Ctx::cached_with(Opts::OLD), Ctx::cached_with(Opts::NEW));
        let a = g.volume(g.router.final_density, &mut old, &v);
        let b = g.volume(g.router.final_density, &mut new, &v);
        assert!(first_diff(&a, &b).is_none(), "volume {cx},{cz} differs");
        for &c in &caches {
            for (x, z) in [(0, 0), (4, 8), (8, 4), (12, 12), (15, 3)] {
                for r in 0..v.sy {
                    let (x, y, z) = (v.x0 + x, v.by(r), v.z0 + z);
                    let want = g.value(c, &mut old, x, y, z);
                    let got = g.value(c, &mut new, x, y, z);
                    compared += 1;
                    if want.to_bits() != got.to_bits() {
                        failures.push(format!("cache {c} chunk {cx},{cz} ({x},{y},{z}): {got} ≠ {want}"));
                        break;
                    }
                }
            }
        }
        assert!(failures.len() < 50, "{}", failures.join("\n"));
    }
    eprintln!("{} caches, {compared} values compared", caches.len());
    assert!(failures.is_empty(), "{} differences:\n{}", failures.len(), failures.join("\n"));
}

/// Same check on a function built for the edge case (no 26.3 data has one): a
/// cache whose point path differs from the volume (interpolated, noise with a non-power-of-2 y scale)
/// under the branch of a choice in y that is only used on part of the rows.
#[test]
fn point_after_volume_cache_under_choice() {
    let Some(pack) = common::data_pack() else { return };
    let mut g = Generator::new(&pack, "overworld", SEED).unwrap();
    let text = r#"{"type":"minecraft:range_choice","min_inclusive":0.5,"max_exclusive":2.0,
        "input":{"type":"minecraft:gradient","axis":"y","from_coordinate":-64,"to_coordinate":0,"from_value":0.0,"to_value":1.0},
        "when_in_range":{"type":"minecraft:cache","input":{"type":"minecraft:add",
            "left":{"type":"minecraft:interpolated","cell_size_xz":4,"cell_size_y":8,
                "input":{"type":"minecraft:noise","noise":"minecraft:cave_cheese","xz_scale":1.0,"y_scale":0.6666666666666666}},
            "right":{"type":"minecraft:noise","noise":"minecraft:cave_layer","xz_scale":1.0,"y_scale":0.7142857142857143}}},
        "when_out_of_range":{"type":"minecraft:constant","value":0.0}}"#;
    let f = rc_worldgen::density::parse(&rc_worldgen::json::parse(text).unwrap()).unwrap();
    let root = g.compile_function(&f).unwrap();
    let cache = (0..g.program.nodes.len() as Sid)
        .rev()
        .find(|&s| matches!(g.program.nodes[s as usize], rc_worldgen::sampler::S::Cache { .. }))
        .expect("compiled cache");
    let mut failures = Vec::new();
    for (cx, cz) in chunks(3) {
        let v = g.chunk_volume(cx, cz);
        let (mut old, mut new) = (Ctx::cached_with(Opts::OLD), Ctx::cached_with(Opts::NEW));
        let a = g.volume(root, &mut old, &v);
        let b = g.volume(root, &mut new, &v);
        assert!(first_diff(&a, &b).is_none(), "volume {cx},{cz} differs");
        for (x, z) in [(0, 0), (5, 9), (15, 15)] {
            for r in 0..v.sy {
                let (x, y, z) = (v.x0 + x, v.by(r), v.z0 + z);
                let want = g.value(cache, &mut old, x, y, z);
                let got = g.value(cache, &mut new, x, y, z);
                if want.to_bits() != got.to_bits() {
                    failures.push(format!("chunk {cx},{cz} ({x},{y},{z}): {got} ≠ {want}"));
                    break;
                }
            }
        }
    }
    assert!(failures.is_empty(), "{} differences:\n{}", failures.len(), failures.join("\n"));
}

/// Shifted noises a data pack can declare but the 26.3 data never does: a shift in y (compiled to
/// `ShiftedXyz`), and no shift in y but a y scale over volumes of several rows. Volumes of several
/// batches (1 024 points), ending on a partial batch, and stepped: the batches against the noise
/// point by point, bit for bit.
#[test]
fn shifted_noise_batches_data_pack_cases() {
    let Some(pack) = common::data_pack() else { return };
    let mut g = Generator::new(&pack, "overworld", SEED).unwrap();
    // Each shift reads its own noise, scaled so that neighbouring points fall in other cells.
    let shift = |noise: &str| {
        format!(
            r#"{{"type":"minecraft:mul","left":200.0,
                "right":{{"type":"minecraft:noise","noise":"{noise}","xz_scale":0.5,"y_scale":0.5}}}}"#
        )
    };
    let shifted_xyz = format!(
        r#"{{"type":"minecraft:noise","noise":"minecraft:temperature","xz_scale":0.25,"y_scale":0.75,
            "shift_x":{},"shift_y":{},"shift_z":{}}}"#,
        shift("minecraft:cave_cheese"),
        shift("minecraft:cave_layer"),
        shift("minecraft:vegetation")
    );
    let shifted_xz = format!(
        r#"{{"type":"minecraft:noise","noise":"minecraft:continentalness","xz_scale":0.25,"y_scale":0.75,
            "shift_x":{},"shift_z":{}}}"#,
        shift("minecraft:cave_cheese"),
        shift("minecraft:vegetation")
    );
    let xyz = compile_text(&mut g, &shifted_xyz);
    let xz = compile_text(&mut g, &shifted_xz);
    assert!(matches!(g.program.nodes[xyz as usize], S::ShiftedXyz { .. }), "shift in y compiled to ShiftedXyz");
    assert!(matches!(g.program.nodes[xz as usize], S::ShiftedXz { y, .. } if y != 0.0), "ShiftedXz with a y scale");
    let pairs = [
        ("old", Opts { shifted_noise_in_batches: true, ..Opts::OLD }, Opts::OLD),
        ("default", Opts::current(), Opts { shifted_noise_in_batches: false, ..Opts::current() }),
    ];
    let mut failures = Vec::new();
    for (cx, cz) in chunks(4) {
        let (x, z, y0) = (cx * 16, cz * 16, g.settings.min_y);
        let volumes = [
            Volume::new(16, 4, 16, x, y0 + 61, z),
            Volume::new(7, 9, 11, x + 3, -5, z - 2),
            Volume::stepped(5, 7, 5, x, y0, z, 4, 8, 4),
        ];
        for (root_name, root) in [("shifted xyz", xyz), ("shifted xz", xz)] {
            for v in &volumes {
                for (label, batches, point_by_point) in &pairs {
                    let want = g.volume(root, &mut Ctx::cached_with(*point_by_point), v);
                    let got = g.volume(root, &mut Ctx::cached_with(*batches), v);
                    if let Some((bad, first)) = first_diff(&got, &want) {
                        failures.push(format!(
                            "{root_name} {label}, chunk {cx},{cz}, {}x{}x{}: {bad} differences, first at {first}",
                            v.sx, v.sy, v.sz
                        ));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{} differences:\n{}", failures.len(), failures.join("\n"));
}

/// Pairs of volumes of a chunk evaluated one after the other in one context: a larger one, then
/// one whose points it holds. The aquifer's quart grid then the corners of the final density (the
/// order of the terrain); a block volume then a grid of step 4 inside it; a block volume then a
/// smaller one that starts in the middle of an interpolation cell.
fn nested_volumes(g: &Generator, cx: i32, cz: i32) -> [(Volume, Volume); 3] {
    let (x, z, y0, h) = (cx * 16, cz * 16, g.settings.min_y, g.settings.height);
    [
        (Volume::stepped(11, 1, 11, x - 16, 0, z - 16, 4, 1, 4), Volume::stepped(5, h / 8 + 1, 5, x, y0, z, 4, 8, 4)),
        (Volume::new(16, 120, 16, x, y0, z), Volume::stepped(4, 15, 4, x, y0, z, 4, 8, 4)),
        (Volume::new(16, 120, 16, x, y0, z), Volume::new(8, 50, 8, x + 3, y0 + 3, z + 5)),
    ]
}

/// Values seen in a fresh context with the paths `o`: `root` over `first`, then over `second`, then
/// point reads on every cache inside `second`, on its points and between them.
fn nested_reads(g: &Generator, o: Opts, root: Sid, caches: &[Sid], (first, second): (Volume, Volume)) -> Vec<f32> {
    let mut ctx = Ctx::cached_with(o);
    let mut out = g.volume(root, &mut ctx, &first);
    out.extend(g.volume(root, &mut ctx, &second));
    let spots = [
        (second.bx(1), second.by(2), second.bz(1)),
        (second.x0 + 1, second.by(1) + 3, second.z0 + 2),
        (second.bx(second.sx - 1), second.by(second.sy - 1), second.bz(0)),
    ];
    for &c in caches {
        for &(x, y, z) in &spots {
            out.push(g.value(c, &mut ctx, x, y, z));
        }
    }
    out
}

/// A cache read through a sub-lattice of the volume it holds gives, bit for bit, what the old code
/// computes: the volume itself and the point reads that follow on every cache. Router roots, vein
/// functions and noodle caves (overworld, amplified, nether, end), plus the built cache over an
/// interpolation, whose input is not exact and must not be served that way.
#[test]
fn sublattice_reads_match_old_code() {
    let Some(pack) = common::data_pack() else { return };
    rc_worldgen::sampler::count_evaluations(true);
    rc_worldgen::sampler::take_eval_counts();
    let mut failures = Vec::new();
    for preset in ["overworld", "amplified", "nether", "end"] {
        let mut g = Generator::new(&pack, preset, SEED).unwrap();
        let mut roots = roots(&mut g, preset == "overworld");
        roots.push(("cache over interpolation".into(), compile_text(&mut g, CACHE_OVER_INTERPOLATION)));
        // The 26.3 caches read in three dimensions sit under interpolations, whose corner grids
        // start on a cell: the built ones (one is an exact noise in 3D) are read with a y offset.
        for (k, text) in CACHES_OVER_Y.iter().enumerate() {
            roots.push((format!("built cache over y {k}"), compile_text(&mut g, text)));
        }
        let caches = cache_nodes(&g);
        for (cx, cz) in chunks(if preset == "overworld" { 12 } else { 4 }) {
            for pair in nested_volumes(&g, cx, cz) {
                for (name, s) in &roots {
                    let want = nested_reads(&g, Opts::OLD, *s, &caches, pair);
                    for (label, o) in [("sublattice", Opts { sublattice_cache_read: true, ..Opts::OLD }), ("all", Opts::NEW)] {
                        let got = nested_reads(&g, o, *s, &caches, pair);
                        if let Some((bad, i)) = first_diff(&got, &want) {
                            failures.push(format!(
                                "{preset} {label} {name} chunk {cx},{cz} {pair:?}: {bad} differences, first {i}: {} ≠ {}",
                                got[i], want[i]
                            ));
                        }
                    }
                }
            }
        }
    }
    let served = rc_worldgen::sampler::take_eval_counts().cache_volume_sublattice;
    rc_worldgen::sampler::count_evaluations(false);
    eprintln!("{served} cache reads served by a sub-lattice");
    assert!(served > 0, "no cache read went through a sub-lattice: the test proves nothing");
    assert!(failures.is_empty(), "{} differences:\n{}", failures.len(), failures.join("\n"));
}

/// Caches built so that a wrong shortcut shows: over a y gradient plus a flat noise, over a noise
/// with a y scale (a cache keyed without y would answer wrongly); over a product of two flat noises
/// (served once per column); over products whose left side is often exactly zero, flat and nested
/// in 3D (the volume gives −0 where the point gives +0, so their buffer must not answer for a point).
const CACHES_OVER_Y: [&str; 5] = [
    r#"{"type":"minecraft:cache","input":{"type":"minecraft:add",
        "left":{"type":"minecraft:gradient","axis":"y","from_coordinate":-64,"from_value":1.5,"to_coordinate":320,"to_value":-1.5},
        "right":{"type":"minecraft:noise","noise":"minecraft:erosion","xz_scale":0.25,"y_scale":0.0}}}"#,
    r#"{"type":"minecraft:cache","input":{"type":"minecraft:noise","noise":"minecraft:cave_cheese","xz_scale":1.0,"y_scale":0.5}}"#,
    r#"{"type":"minecraft:cache","input":{"type":"minecraft:mul",
        "left":{"type":"minecraft:noise","noise":"minecraft:erosion","xz_scale":0.25,"y_scale":0.0},
        "right":{"type":"minecraft:noise","noise":"minecraft:ridge","xz_scale":0.25,"y_scale":0.0}}}"#,
    r#"{"type":"minecraft:cache","input":{"type":"minecraft:mul",
        "left":{"type":"minecraft:clamp","min":0.0,"max":1.0,
            "input":{"type":"minecraft:noise","noise":"minecraft:erosion","xz_scale":0.25,"y_scale":0.0}},
        "right":{"type":"minecraft:add","left":-2.0,
            "right":{"type":"minecraft:noise","noise":"minecraft:ridge","xz_scale":0.25,"y_scale":0.0}}}}"#,
    r#"{"type":"minecraft:cache","input":{"type":"minecraft:add","left":1.0,
        "right":{"type":"minecraft:cache","input":{"type":"minecraft:mul",
            "left":{"type":"minecraft:clamp","min":0.0,"max":1.0,
                "input":{"type":"minecraft:noise","noise":"minecraft:cave_cheese","xz_scale":1.0,"y_scale":0.5}},
            "right":{"type":"minecraft:add","left":-2.0,
                "right":{"type":"minecraft:noise","noise":"minecraft:cave_layer","xz_scale":1.0,"y_scale":0.5}}}}}}"#,
];

/// Point reads in a fresh context with the paths `o`, in the order of the terrain: the aquifer's
/// quart grid of the preliminary surface (and of every root), point reads of every root at heights
/// that change from one read to the next (the aquifer reads its cells), the corners of the final
/// density, then the same point reads again.
fn column_reads(g: &Generator, o: Opts, surface: Sid, roots: &[Sid], (cx, cz): (i32, i32)) -> Vec<f32> {
    let mut ctx = Ctx::cached_with(o);
    let (x, z) = (cx * 16, cz * 16);
    let quarts = Volume::stepped(11, 1, 11, x - 16, 0, z - 16, 4, 1, 4);
    let mut out = g.volume(surface, &mut ctx, &quarts);
    // Every cache gets a buffer at y = 0, which a point read at another height could wrongly use.
    for &r in roots {
        out.extend(g.volume(r, &mut ctx, &quarts));
    }
    let spots: Vec<(i32, i32, i32)> =
        (0..24).map(|k| (x + (k * 5) % 16, g.settings.min_y + (k * 37) % g.settings.height, z + (k * 11) % 16)).collect();
    let read_all = |ctx: &mut Ctx, out: &mut Vec<f32>| {
        for &(px, py, pz) in &spots {
            for &r in roots {
                out.push(g.value(r, ctx, px, py, pz));
                out.push(g.value(r, ctx, px, py + 8, pz));
            }
        }
    };
    read_all(&mut ctx, &mut out);
    let corners = Volume::stepped(5, g.settings.height / 8 + 1, 5, x, g.settings.min_y, z, 4, 8, 4);
    out.extend(g.volume(g.router.final_density, &mut ctx, &corners));
    read_all(&mut ctx, &mut out);
    back_to_a_point_after_its_volume(g, &mut ctx, roots, &spots, &mut out);
    out
}

/// A point, the one-block volume holding it, the same column higher, then the point again. The
/// volume keeps the key on the point but its buffer may give −0 where the point gave +0: a cache
/// answering the higher read from its column must move its key there too, as the game's computed
/// point does, or the last read finds the stale point instead of the buffer.
fn back_to_a_point_after_its_volume(g: &Generator, ctx: &mut Ctx, roots: &[Sid], spots: &[(i32, i32, i32)], out: &mut Vec<f32>) {
    for &(px, py, pz) in spots {
        for &r in roots {
            out.push(g.value(r, ctx, px, py, pz));
            out.extend(g.volume(r, ctx, &Volume::new(1, 1, 1, px, py, pz)));
            out.push(g.value(r, ctx, px, py + 16, pz));
            out.push(g.value(r, ctx, px, py, pz));
        }
    }
}

/// A cache whose input never reads y answers a point read at any height from its last point (and
/// its buffer at y = 0) with the bits the old code computes: every cache of the overworld programs
/// (normal, large biomes, amplified), the climate roots and the built caches over y, in a context
/// with caches, point heights changing from one read to the next. Counted: the memo must compute
/// fewer cache points than the old code, or the test proves nothing.
#[test]
fn column_memo_reads_match_old_code() {
    let Some(pack) = common::data_pack() else { return };
    rc_worldgen::sampler::count_evaluations(true);
    let mut failures = Vec::new();
    let (mut old_computed, mut memo_computed) = (0, 0);
    for (preset, folder) in [("overworld", "overworld"), ("large_biomes", "overworld_large_biomes"), ("amplified", "overworld_amplified")] {
        let mut g = Generator::new(&pack, preset, SEED).unwrap();
        let surface = g.compile_named(&format!("minecraft:{folder}/preliminary_surface_level")).unwrap();
        let built: Vec<Sid> = CACHES_OVER_Y.iter().map(|t| compile_text(&mut g, t)).collect();
        let r = g.router;
        let mut roots = vec![r.temperature, r.vegetation, r.continents, r.erosion, r.depth, r.ridges];
        roots.extend(built);
        roots.extend(cache_nodes(&g));
        for (cx, cz) in chunks(12) {
            rc_worldgen::sampler::take_eval_counts();
            let want = column_reads(&g, Opts::OLD, surface, &roots, (cx, cz));
            old_computed += rc_worldgen::sampler::take_eval_counts().cache_point_computed;
            for (label, o) in [("memo", Opts { column_memo_y_independent: true, ..Opts::OLD }), ("all", Opts::NEW)] {
                let got = column_reads(&g, o, surface, &roots, (cx, cz));
                if label == "memo" {
                    memo_computed += rc_worldgen::sampler::take_eval_counts().cache_point_computed;
                }
                if let Some((bad, i)) = first_diff(&got, &want) {
                    failures.push(format!("{preset} {label} chunk {cx},{cz}: {bad} differences, first {i}: {} ≠ {}", got[i], want[i]));
                }
            }
        }
    }
    rc_worldgen::sampler::count_evaluations(false);
    eprintln!("cache points computed: old {old_computed}, memo {memo_computed}");
    assert!(memo_computed < old_computed, "no cache answered from its column: the test proves nothing");
    assert!(failures.is_empty(), "{} differences:\n{}", failures.len(), failures.join("\n"));
}

/// The counts per chunk of a step, one line.
fn census_line(step: &str, c: EvalCounts, chunks: usize) -> String {
    let n = chunks as f64;
    let per = |v: u64| v as f64 / n;
    format!(
        "{step:28} noises {:9.1} | splines {:8.1} | top probes {:7.1} | cache point last/buffer/column/computed {:8.1} {:8.1} {:6.1} {:8.1} \
         | cache volume same/rows/sublattice/computed {:5.1} {:5.1} {:5.1} {:5.1}, {:8.0} values \
         | choice branches {:5.1}: read {:8.0}, whole hull {:8.0}, column hulls {:8.0}, \
         plain branches noise values whole hull {:8.0}, column hulls {:8.0}",
        per(c.point_noises),
        per(c.point_splines),
        per(c.top_probes),
        per(c.cache_point_last),
        per(c.cache_point_buffer),
        per(c.cache_point_same_column),
        per(c.cache_point_computed),
        per(c.cache_volume_same),
        per(c.cache_volume_rows),
        per(c.cache_volume_sublattice),
        per(c.cache_volume_computed),
        per(c.cache_volume_values),
        per(c.choice_branches),
        per(c.branch_values_read),
        per(c.branch_values_whole_hull),
        per(c.branch_values_column_hulls),
        per(c.plain_branch_noise_values_whole_hull),
        per(c.plain_branch_noise_values_column_hulls),
    )
}

/// Runs `f` on a pool of one thread, so that its parallel work stays on the thread whose counts are
/// read, and returns what it counted.
fn counted(f: impl FnOnce() + Send) -> EvalCounts {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    pool.install(|| {
        rc_worldgen::sampler::take_eval_counts();
        f();
        rc_worldgen::sampler::take_eval_counts()
    })
}

/// What a terrain chunk evaluates, per step (64 chunks, seed 42; counts, so they hold on a loaded
/// machine): biomes of the chunks, creation of the aquifer (top surface search), filling, surface,
/// caves, and the base heights of 16 columns per chunk (memo off). Counted with the paths of the
/// environment: run it again with a switch at 0 to see what that switch changes.
#[test]
#[ignore = "benchmark"]
fn point_evaluation_census() {
    let Some(pack) = common::data_pack() else { return };
    rc_worldgen::sampler::count_evaluations(true);
    let targets: Vec<(i32, i32)> = (0..8).flat_map(|x| (0..8).map(move |z| (x - 4, z - 4))).collect();
    let n = targets.len();
    let sources = [
        ("minecraft:normal", "minecraft:overworld"),
        ("minecraft:amplified", "minecraft:overworld"),
        ("minecraft:large_biomes", "minecraft:overworld"),
        ("minecraft:normal", "minecraft:the_nether"),
        ("minecraft:normal", "minecraft:the_end"),
    ];
    for (preset, dim) in sources {
        let mut t = TerrainGenerator::from_preset(&pack, preset, dim, 42).expect("generator");
        t.opts.baseh_memo = false;
        let mut cache = BiomeCache::default();
        let biomes = counted(|| t.prepare_biomes(&mut cache, &targets));
        let aquifers = counted(|| {
            for &(x, z) in &targets {
                if let Some(v) = t.fill_volume(x, z) {
                    std::hint::black_box(t.new_aquifer(&mut Ctx::cached(), &v));
                }
            }
        });
        let until = |stage: Stage| {
            counted(|| {
                for &(x, z) in &targets {
                    std::hint::black_box(t.generate_until(x, z, &cache, None, stage));
                }
            })
        };
        let (fill, surface, carvers) = (until(Stage::Fill), until(Stage::Surface), until(Stage::Carvers));
        let heights = counted(|| {
            for &(cx, cz) in &targets {
                for k in 0..16 {
                    std::hint::black_box(t.base_height(cx * 16 + k, cz * 16 + (k * 7) % 16, false));
                }
            }
        });
        eprintln!("{preset} {dim}");
        eprintln!("{}", census_line("  biomes", biomes, n));
        eprintln!("{}", census_line("  aquifer creation", aquifers, n));
        eprintln!("{}", census_line("  filling (with the aquifer)", fill, n));
        eprintln!("{}", census_line("  surface", surface - fill, n));
        eprintln!("{}", census_line("  caves", carvers - surface, n));
        eprintln!("{}", census_line("  16 base heights", heights, n));
    }
    rc_worldgen::sampler::count_evaluations(false);
}

const KEEP_SEEDS: [i64; 3] = [0, -6_214_586_474_519_478_921, 1_234_567_890_123_456_789];

/// Function built so that a stale cache cell shows (no 26.3 cache is read this way): a cache over
/// an interpolated noise whose y scale is not a power of 2, so that its point value differs
/// from its volume value away from the cell corners.
const CACHE_OVER_INTERPOLATION: &str = r#"{"type":"minecraft:cache","input":{"type":"minecraft:interpolated",
    "cell_size_xz":4,"cell_size_y":8,"input":{"type":"minecraft:noise","noise":"minecraft:cave_cheese",
    "xz_scale":1.0,"y_scale":0.6666666666666666}}}"#;

/// Positions read in a chunk (offsets x, z; world y): away from the interpolation corners.
const SPOTS: [(i32, i32, i32); 3] = [(5, 37, 7), (13, 101, 2), (2, 66, 11)];

fn compile_text(g: &mut Generator, text: &str) -> Sid {
    let f = rc_worldgen::density::parse(&rc_worldgen::json::parse(text).unwrap()).unwrap();
    g.compile_function(&f).unwrap()
}

fn cache_nodes(g: &Generator) -> Vec<Sid> {
    (0..g.program.nodes.len() as Sid).filter(|&s| matches!(g.program.nodes[s as usize], S::Cache { .. })).collect()
}

/// 200 chunks: 8 runs of 25 neighbours along x, from the test chunks.
fn chunk_walk() -> Vec<(i32, i32)> {
    chunks(8).into_iter().flat_map(|(cx, cz)| (0..25).map(move |k| (cx + k, cz))).collect()
}

fn read_spots<'a>(
    g: &Generator,
    ctx: &mut Ctx,
    caches: &[Sid],
    (cx, cz): (i32, i32),
    spots: impl Iterator<Item = &'a (i32, i32, i32)>,
    out: &mut Vec<f32>,
) {
    for &(ox, y, oz) in spots {
        for &c in caches {
            out.push(g.value(c, ctx, cx * 16 + ox, y, cz * 16 + oz));
        }
    }
}

/// Values seen by one chunk in `ctx`, in the order of the terrain: point reads on every cache in
/// the previous chunk (whose volumes a kept context could still hold), the volumes of the chunk
/// (buffers handed back), then point reads in the chunk (in reverse order: the first one is
/// the last point the previous chunk read) and in the next chunk.
fn chunk_reads(g: &Generator, ctx: &mut Ctx, roots: &[Sid], caches: &[Sid], (cx, cz): (i32, i32)) -> Vec<f32> {
    let mut out = Vec::new();
    read_spots(g, ctx, caches, (cx - 1, cz), SPOTS.iter(), &mut out);
    let v = g.chunk_volume(cx, cz);
    for &root in roots {
        let b = g.volume(root, ctx, &v);
        out.extend_from_slice(&b);
        ctx.recycle(b);
    }
    read_spots(g, ctx, caches, (cx, cz), SPOTS.iter().rev(), &mut out);
    read_spots(g, ctx, caches, (cx + 1, cz), SPOTS.iter(), &mut out);
    out
}

/// Chunks of the walk in a context kept and reset at each chunk, against a fresh context.
fn kept_context_failures(pack: &rc_worldgen::data::DataPack, preset: &str, seed: i64) -> Vec<String> {
    let mut g = Generator::new(pack, preset, seed).unwrap();
    let built = compile_text(&mut g, CACHE_OVER_INTERPOLATION);
    let roots = [g.router.final_density, built];
    let caches = cache_nodes(&g);
    let mut kept = Ctx::cached_with(Opts::NEW);
    let mut failures = Vec::new();
    for pos in chunk_walk() {
        kept.reset_for_reuse();
        let got = chunk_reads(&g, &mut kept, &roots, &caches, pos);
        let want = chunk_reads(&g, &mut Ctx::cached_with(Opts::NEW), &roots, &caches, pos);
        if let Some((bad, i)) = first_diff(&got, &want) {
            failures.push(format!("{preset} seed {seed} chunk {pos:?}: {bad} differences, first {i}: {} ≠ {}", got[i], want[i]));
            if failures.len() >= 5 {
                break;
            }
        }
    }
    failures
}

/// A context kept from one chunk to the next and reset at each one (the benches) gives
/// what a fresh context gives, bit for bit: 200 chunks × 3 seeds × 3 dimensions, final density
/// and the built cache over an interpolation, point reads on every cache around the chunk.
#[test]
fn kept_context_matches_fresh_context() {
    let Some(pack) = common::data_pack() else { return };
    let jobs: Vec<(&str, i64)> =
        ["overworld", "nether", "end"].into_iter().flat_map(|p| KEEP_SEEDS.map(|s| (p, s))).collect();
    let failures: Vec<String> =
        jobs.par_iter().flat_map_iter(|&(preset, seed)| kept_context_failures(&pack, preset, seed)).collect();
    assert!(failures.is_empty(), "{} chunks differ:\n{}", failures.len(), failures.join("\n"));
}

/// Survey of the common subexpressions (SAMPLER_CSE track): uncached nodes that
/// the volume evaluation of a root computes several times identically (same structure,
/// same children), with the number of noises under each.
#[test]
#[ignore = "benchmark"]
fn cse_census() {
    use hashbrown::HashMap;
    let Some(pack) = common::data_pack() else { return };
    for preset in ["overworld", "nether", "end"] {
        let mut g = Generator::new(&pack, preset, SEED).unwrap();
        let roots = roots(&mut g, preset == "overworld");
        let nodes = &g.program.nodes;
        // Structural key: text of the node with the children replaced by their key.
        let mut key: Vec<String> = Vec::with_capacity(nodes.len());
        for n in nodes {
            let mut kids = Vec::new();
            n.children(|c| kids.push(key[c as usize].clone()));
            let own = format!("{n:?}");
            let head = own.split(['(', '{']).next().unwrap_or("").to_owned();
            key.push(if kids.is_empty() { own } else { format!("{head}[{}]", kids.join(",")) });
        }
        let is_noise = |n: &S| matches!(n, S::Noise { .. } | S::ShiftedXz { .. } | S::ShiftedXyz { .. } | S::ShiftB { .. });
        let noises = |s: Sid| -> usize {
            let mut st = vec![s];
            let mut n = 0;
            while let Some(x) = st.pop() {
                let node = &nodes[x as usize];
                n += usize::from(is_noise(node));
                node.children(|c| st.push(c));
            }
            n
        };
        for (name, root) in &roots {
            // Evaluations per key, descending the whole tree (a cache is only descended once).
            let mut count: HashMap<&str, (usize, Sid)> = HashMap::new();
            let mut seen_cache = hashbrown::HashSet::new();
            let mut st = vec![*root];
            while let Some(x) = st.pop() {
                let node = &nodes[x as usize];
                if let S::Cache { id, .. } = node
                    && !seen_cache.insert(*id)
                {
                    continue;
                }
                count.entry(key[x as usize].as_str()).or_insert((0, x)).0 += 1;
                node.children(|c| st.push(c));
            }
            // Extra noises: visits of a noise node beyond the first per key.
            let (mut visits, mut distinct) = (0, 0);
            for (n, s) in count.values() {
                if is_noise(&nodes[*s as usize]) {
                    visits += n;
                    distinct += 1;
                }
            }
            let shared = count.values().filter(|(n, s)| *n > 1 && noises(*s) > 0).count();
            eprintln!(
                "{preset} {name}: {visits} noises evaluated, {distinct} distinct, {} extra; {shared} repeated noise subtrees",
                visits - distinct
            );
        }
    }
}

fn final_density_with(g: &Generator, o: Opts, cx: i32, cz: i32) -> Vec<f32> {
    g.volume(g.router.final_density, &mut Ctx::cached_with(o), &g.chunk_volume(cx, cz))
}

/// Final density of a chunk in a context kept from one chunk to the next (evaluation time without
/// the allocations): reset first, output taken from the spare buffers and handed back.
fn final_density_kept(g: &Generator, ctx: &mut Ctx, cx: i32, cz: i32) -> usize {
    ctx.reset_for_reuse();
    let v = g.chunk_volume(cx, cz);
    let mut b = ctx.take(v.size());
    g.program.volume(g.router.final_density, ctx, &mut b, &v);
    let n = b.len();
    ctx.recycle(b);
    n
}

/// Allocations, allocated KB, page faults and private KB per chunk since `before`.
fn per_chunk_costs(before: (u64, u64, Option<(u64, u64)>), chunks: usize) -> String {
    let n = chunks as f64;
    let (allocs, bytes) = (ALLOCS.load(Ordering::Relaxed) - before.0, BYTES.load(Ordering::Relaxed) - before.1);
    let mut s = format!("{:.1} alloc, {:.0} KB", allocs as f64 / n, bytes as f64 / n / 1024.0);
    if let (Some((f0, p0)), Some((f1, p1))) = (before.2, process::memory()) {
        s += &format!(", {:.1} page faults, private {:+.0} KB", (f1 - f0) as f64 / n, (p1 as f64 - p0 as f64) / n / 1024.0);
    }
    s + " / chunk"
}

fn costs_now() -> (u64, u64, Option<(u64, u64)>) {
    (ALLOCS.load(Ordering::Relaxed), BYTES.load(Ordering::Relaxed), process::memory())
}

/// Final density of 256 chunks: each path alone, then all; 1 thread then powers of 2
/// up to the number of cores; allocations, page faults and private memory per chunk (1 thread).
/// `kept`: one context per rayon job kept from one chunk to the next and reset at each one, output
/// taken from its spare buffers and handed back; `new`: a new context per chunk (the terrain).
#[test]
#[ignore = "benchmark"]
fn bench_final_density() {
    let Some(pack) = common::data_pack() else { return };
    counting::client_options();
    let g = Generator::new(&pack, "overworld", 42).unwrap();
    let chunks: Vec<(i32, i32)> = (0..16).flat_map(|x| (0..16).map(move |z| (x - 8, z - 8))).collect();
    let mut cases = vec![("old", Opts::OLD)];
    cases.extend(VARIANTS);
    // Each track of C1-M06 removed from all the others (combinations).
    cases.extend([
        ("all but best_fit", Opts { best_fit_buffers: false, ..Opts::NEW }),
        ("all but no_zeroing", Opts { take_without_zeroing: false, ..Opts::NEW }),
    ]);
    let max = std::thread::available_parallelism().map_or(1, |n| n.get());
    for pass in 0..2 {
        for ((label, o), keep) in cases.iter().flat_map(|c| [(c, false), (c, true)]) {
            let mut t = 1;
            let mut line = format!("pass {pass} {label:20} {}", if keep { "kept" } else { "new " });
            while t <= max {
                let pool = rayon::ThreadPoolBuilder::new().num_threads(t).build().unwrap();
                let run = |cs: &[(i32, i32)]| {
                    pool.install(|| {
                        if keep {
                            let one = |ctx: &mut Ctx, &(x, z): &(i32, i32)| final_density_kept(&g, ctx, x, z);
                            cs.par_iter().map_init(|| Ctx::cached_with(*o), one).sum::<usize>()
                        } else {
                            cs.par_iter().map(|&(x, z)| final_density_with(&g, *o, x, z).len()).sum::<usize>()
                        }
                    })
                };
                run(&chunks[..8]);
                let before = costs_now();
                let s = Instant::now();
                std::hint::black_box(run(&chunks));
                let dt = s.elapsed().as_secs_f64() * 1e3 / chunks.len() as f64;
                if t == 1 {
                    line += &format!(" | {}", per_chunk_costs(before, chunks.len()));
                }
                line += &format!(" | {t} threads {dt:.3} ms");
                t *= 2;
            }
            eprintln!("{line}");
        }
    }
    // Reserve held by a kept context after 256 chunks (bound of the memory kept per thread).
    for (label, o) in &cases {
        let mut ctx = Ctx::cached_with(*o);
        for &(x, z) in &chunks {
            final_density_kept(&g, &mut ctx, x, z);
        }
        ctx.reset_for_reuse();
        eprintln!("{label:20} reserve of a kept context: {} KB", ctx.spare_bytes() / 1024);
    }
}

/// Minimum and median of the samples, ms.
fn min_and_median(mut samples: Vec<f64>) -> (f64, f64) {
    samples.sort_by(f64::total_cmp);
    (samples[0], samples[samples.len() / 2])
}

/// Time per chunk of `run` over `chunks`, ms.
fn ms_per_chunk(chunks: &[(i32, i32)], mut run: impl FnMut(i32, i32)) -> f64 {
    let s = Instant::now();
    for &(x, z) in chunks {
        run(x, z);
    }
    s.elapsed().as_secs_f64() * 1e3 / chunks.len() as f64
}

/// New context per chunk against the kept one, on 1 thread, final density alternated over 12 rounds
/// (switches of the environment): minimum and median per chunk. Alternating and keeping the minimum
/// resists a loaded machine better than two blocks.
#[test]
#[ignore = "benchmark"]
fn bench_kept_context_alternated() {
    let Some(pack) = common::data_pack() else { return };
    counting::client_options();
    let g = Generator::new(&pack, "overworld", 42).unwrap();
    let chunks: Vec<(i32, i32)> = (0..8).flat_map(|x| (0..8).map(move |z| (x - 4, z - 4))).collect();
    // (label, paths, kept context): the default paths, then each track of C1-M06 taken out.
    let cases = [
        ("default, new context", Opts::current(), false),
        ("default, kept", Opts::current(), true),
        ("default but no_zeroing, kept", Opts { take_without_zeroing: false, ..Opts::current() }, true),
        ("default but best_fit, kept", Opts { best_fit_buffers: false, ..Opts::current() }, true),
        ("default but both, new context", Opts { take_without_zeroing: false, best_fit_buffers: false, ..Opts::current() }, false),
    ];
    let mut kept_contexts: Vec<Ctx> = cases.iter().map(|(_, o, _)| Ctx::cached_with(*o)).collect();
    let mut samples = vec![Vec::new(); cases.len()];
    for _ in 0..12 {
        for (i, (_, o, keep)) in cases.iter().enumerate() {
            let ctx = &mut kept_contexts[i];
            samples[i].push(ms_per_chunk(&chunks, |x, z| {
                let n = if *keep { final_density_kept(&g, ctx, x, z) } else { final_density_with(&g, *o, x, z).len() };
                std::hint::black_box(n);
            }));
        }
    }
    for ((label, ..), s) in cases.iter().zip(samples) {
        let (min, median) = min_and_median(s);
        eprintln!("final density, {label:32} ms/chunk min {min:.3} median {median:.3}");
    }
}

/// Final density of 64 chunks on 1 thread (kept context), shifted noises in batches against point by
/// point, alternated over 15 rounds: minimum and median per chunk. The noise kernel inside the
/// batches (AVX2, shared cell) follows the environment: run it again with those switches at 0.
#[test]
#[ignore = "benchmark"]
fn bench_shifted_noise_batches_alternated() {
    let Some(pack) = common::data_pack() else { return };
    counting::client_options();
    let g = Generator::new(&pack, "overworld", 42).unwrap();
    let chunks: Vec<(i32, i32)> = (0..8).flat_map(|x| (0..8).map(move |z| (x - 4, z - 4))).collect();
    eprintln!("noise paths {:?}", rc_worldgen::noise::Paths::current());
    let cases = [
        ("default (batches)", Opts::current()),
        ("default, point by point", Opts { shifted_noise_in_batches: false, ..Opts::current() }),
    ];
    let mut contexts: Vec<Ctx> = cases.iter().map(|(_, o)| Ctx::cached_with(*o)).collect();
    let mut samples = vec![Vec::new(); cases.len()];
    for _ in 0..15 {
        for (i, ctx) in contexts.iter_mut().enumerate() {
            samples[i].push(ms_per_chunk(&chunks, |x, z| {
                std::hint::black_box(final_density_kept(&g, ctx, x, z));
            }));
        }
    }
    for ((label, _), s) in cases.iter().zip(samples) {
        let (min, median) = min_and_median(s);
        eprintln!("final density, {label:26} ms/chunk min {min:.3} median {median:.3}");
    }
}

/// Aquifer creation then final density of a chunk in a context kept by the thread and reset, the
/// order of the terrain's filling (the aquifer fills the quart grid of the flat caches first).
fn fill_sequence(t: &TerrainGenerator, ctx: &mut Ctx, (cx, cz): (i32, i32)) -> usize {
    ctx.reset_for_reuse();
    let Some(v) = t.fill_volume(cx, cz) else { return 0 };
    std::hint::black_box(t.new_aquifer(ctx, &v));
    let mut b = ctx.take(v.size());
    t.g.program.volume(t.g.router.final_density, ctx, &mut b, &v);
    let n = b.len();
    ctx.recycle(b);
    n
}

/// Aquifer creation + final density (see [`fill_sequence`]), 64 chunks on 1 thread: the default
/// paths, then each track of C1-M13 taken out, alternated over 12 rounds; minimum and median per
/// chunk, allocations per chunk. Then the default paths and all three tracks out on 1 thread up to
/// every core.
#[test]
#[ignore = "benchmark"]
fn bench_fill_sequence() {
    let Some(pack) = common::data_pack() else { return };
    counting::client_options();
    let t = TerrainGenerator::from_preset(&pack, "minecraft:normal", "minecraft:overworld", 42).unwrap();
    let chunks: Vec<(i32, i32)> = (0..8).flat_map(|x| (0..8).map(move |z| (x - 4, z - 4))).collect();
    let d = Opts::current();
    let cases = [
        ("default", d),
        ("default but sublattice", Opts { sublattice_cache_read: false, ..d }),
        ("default but column memo", Opts { column_memo_y_independent: false, ..d }),
        ("default but spline inputs on stack", Opts { spline_inputs_on_stack: false, ..d }),
        ("default but spline_stack", Opts { spline_stack: false, ..d }),
        (
            "default but the three",
            Opts { sublattice_cache_read: false, column_memo_y_independent: false, spline_inputs_on_stack: false, ..d },
        ),
    ];
    let mut contexts: Vec<Ctx> = cases.iter().map(|(_, o)| Ctx::cached_with(*o)).collect();
    let mut samples = vec![Vec::new(); cases.len()];
    let mut allocs = vec![0.0; cases.len()];
    for round in 0..12 {
        for (i, ctx) in contexts.iter_mut().enumerate() {
            let before = ALLOCS.load(Ordering::Relaxed);
            samples[i].push(ms_per_chunk(&chunks, |x, z| {
                std::hint::black_box(fill_sequence(&t, ctx, (x, z)));
            }));
            if round == 11 {
                allocs[i] = (ALLOCS.load(Ordering::Relaxed) - before) as f64 / chunks.len() as f64;
            }
        }
    }
    for (((label, _), s), a) in cases.iter().zip(samples).zip(allocs) {
        let (min, median) = min_and_median(s);
        eprintln!("aquifer + final density, {label:36} ms/chunk min {min:.3} median {median:.3} | {a:.1} alloc / chunk");
    }
    let max = std::thread::available_parallelism().map_or(1, |n| n.get());
    for pass in 0..2 {
        for (label, o) in [cases[0], cases[5]] {
            let mut line = format!("pass {pass} {label:24}");
            let mut threads = 1;
            while threads <= max {
                let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
                let run = |cs: &[(i32, i32)]| {
                    pool.install(|| {
                        cs.par_iter().map_init(|| Ctx::cached_with(o), |ctx, &pos| fill_sequence(&t, ctx, pos)).sum::<usize>()
                    })
                };
                run(&chunks[..8]);
                let s = Instant::now();
                std::hint::black_box(run(&chunks));
                line += &format!(" | {threads} threads {:.3} ms", s.elapsed().as_secs_f64() * 1e3 / chunks.len() as f64);
                threads *= 2;
            }
            eprintln!("{line}");
        }
    }
}

/// Point climate (the 6 roots at 20,000 quarts, as a biome lookup at a point reads them), the point
/// spline on the stack or not, in a context without caches (structures, surface) and with caches
/// (`/locate biome`); alternated over 12 rounds, minimum and median in µs per quart, allocations.
#[test]
#[ignore = "benchmark"]
fn bench_point_climate_alternated() {
    let Some(pack) = common::data_pack() else { return };
    counting::client_options();
    let g = Generator::new(&pack, "overworld", 42).unwrap();
    let r = g.router;
    let roots = [r.temperature, r.vegetation, r.continents, r.erosion, r.depth, r.ridges];
    let d = Opts::current();
    let cases = [
        ("spline on the heap, no caches", Opts { spline_stack: false, ..d }, false),
        ("spline on the stack, no caches", Opts { spline_stack: true, ..d }, false),
        ("spline on the heap, caches", Opts { spline_stack: false, ..d }, true),
        ("spline on the stack, caches", Opts { spline_stack: true, ..d }, true),
    ];
    let quarts = 20_000i32;
    let mut samples = vec![Vec::new(); cases.len()];
    let mut allocs = vec![0.0; cases.len()];
    for _ in 0..12 {
        for (i, (_, o, cached)) in cases.iter().enumerate() {
            let mut ctx = if *cached { Ctx::cached_with(*o) } else { Ctx::uncached_with(*o) };
            let before = ALLOCS.load(Ordering::Relaxed);
            let s = Instant::now();
            let mut acc = 0.0f32;
            for q in 0..quarts {
                let (qx, qy, qz) = (q % 317 * 7 - 1000, q % 23 - 16, q / 317 * 5 - 700);
                for &root in &roots {
                    acc += g.value(root, &mut ctx, qx << 2, qy << 2, qz << 2);
                }
            }
            std::hint::black_box(acc);
            samples[i].push(s.elapsed().as_secs_f64() * 1e6 / f64::from(quarts));
            allocs[i] = (ALLOCS.load(Ordering::Relaxed) - before) as f64 / f64::from(quarts);
        }
    }
    for (((label, ..), s), a) in cases.iter().zip(samples).zip(allocs) {
        let (min, median) = min_and_median(s);
        eprintln!("point climate, {label:32} µs/quart min {min:.3} median {median:.3} | {a:.2} alloc / quart");
    }
}

/// Vein pre-filling (density and richness of copper and iron over a chunk's volume,
/// same context) and point climate (100,000 quarts): old, each path, all.
#[test]
#[ignore = "benchmark"]
fn bench_veins_and_climate() {
    let Some(pack) = common::data_pack() else { return };
    let mut g = Generator::new(&pack, "overworld", 42).unwrap();
    let ids: Vec<Sid> = ["copper_density", "richness", "iron_density"]
        .iter()
        .map(|id| g.compile_named(&format!("minecraft:overworld/ore_vein/{id}")).unwrap())
        .collect();
    let chunks: Vec<(i32, i32)> = (0..8).flat_map(|x| (0..8).map(move |z| (x - 4, z - 4))).collect();
    let mut cases = vec![("old", Opts::OLD)];
    cases.extend(VARIANTS);
    for pass in 0..2 {
        for (label, o) in &cases {
            let s = Instant::now();
            for &(cx, cz) in &chunks {
                let mut ctx = Ctx::cached_with(*o);
                let v = Volume::new(16, 200, 16, cx * 16, -64, cz * 16);
                for id in &ids {
                    let b = g.volume(*id, &mut ctx, &v);
                    std::hint::black_box(&b);
                    ctx.recycle(b);
                }
            }
            let veins = s.elapsed().as_secs_f64() * 1e3 / chunks.len() as f64;
            let r = g.router;
            let s = Instant::now();
            let mut ctx = Ctx::uncached_with(*o);
            let mut acc = 0.0f32;
            for i in 0..100_000i32 {
                let (qx, qz) = (i % 317 * 7 - 1000, i / 317 * 5 - 700);
                for sid in [r.temperature, r.vegetation, r.continents, r.erosion, r.depth, r.ridges] {
                    acc += g.value(sid, &mut ctx, qx << 2, 64, qz << 2);
                }
            }
            std::hint::black_box(acc);
            let climate = s.elapsed().as_secs_f64() * 1e6 / 100_000.0;
            eprintln!("pass {pass} {label:12} | veins {veins:.3} ms / chunk | climate {climate:.2} µs / quart");
        }
    }
}
