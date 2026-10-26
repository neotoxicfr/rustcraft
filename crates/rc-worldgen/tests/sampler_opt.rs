//! Density evaluator (C1-M05): new paths (`sampler::Opts::NEW`) against the old code
//! (`Opts::OLD`) bit for bit, on the router roots and the vein functions; micro-benches.
//! Without game data, the tests are skipped (explicit failure under `RUSTCRAFT_OPT_REQUIRE=1`).
//!
//! Benches (quiet machine): `cargo test --release -p rc-worldgen --test sampler_opt -- --ignored --nocapture`.

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use rc_worldgen::sampler::{Opts, Sid};
use rc_worldgen::{Ctx, Generator, Volume};

static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

/// Allocation counter of the test binary (per-chunk allocation bench), around
/// the system allocator.
#[allow(unsafe_code)]
mod counting {
    use super::{ALLOCS, BYTES, GlobalAlloc, Layout, Ordering, System};

    struct Counting;

    // SAFETY: delegates as is to the system allocator; only two counters are added.
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(l.size() as u64, Ordering::Relaxed);
            // SAFETY: same preconditions as the caller.
            unsafe { System.alloc(l) }
        }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            // SAFETY: `p` comes from this allocator, hence from the system.
            unsafe { System.dealloc(p, l) }
        }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
            ALLOCS.fetch_add(1, Ordering::Relaxed);
            BYTES.fetch_add(n as u64, Ordering::Relaxed);
            // SAFETY: same preconditions as the caller.
            unsafe { System.realloc(p, l, n) }
        }
    }

    #[global_allocator]
    static A: Counting = Counting;
}

const SEED: i64 = 20_261_011;

/// Each path alone, then the combinations with `range_yvol`, then all: a faulty track is
/// named by its own failure.
const VARIANTS: [(&str, Opts); 6] = [
    ("range_yvol", Opts { range_yvol: true, vol_pool: false, spline_stack: false }),
    ("vol_pool", Opts { range_yvol: false, vol_pool: true, spline_stack: false }),
    ("spline_stack", Opts { range_yvol: false, vol_pool: false, spline_stack: true }),
    ("yvol+pool", Opts { range_yvol: true, vol_pool: true, spline_stack: false }),
    ("yvol+spline", Opts { range_yvol: true, vol_pool: false, spline_stack: true }),
    ("all", Opts::NEW),
];

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
            let mk = |o: Opts| if cached { Ctx::cached_with(o) } else { Ctx::uncached_with(o) };
            let mut old_ctx = mk(Opts::OLD);
            let mut new_ctx: Vec<Ctx> = VARIANTS.iter().map(|(_, o)| mk(*o)).collect();
            for v in &vols {
                for (name, s) in &roots {
                    let want = g.volume(*s, &mut old_ctx, v);
                    for ((label, _), ctx) in VARIANTS.iter().zip(&mut new_ctx) {
                        let got = g.volume(*s, ctx, v);
                        if let Some((bad, i)) = first_diff(&got, &want) {
                            failures.push(format!(
                                "{preset} {label} {name} chunk {cx},{cz} {v:?} caches {cached}: {bad} differences, \
                                 first {i}: {} ≠ {}",
                                got[i], want[i]
                            ));
                        }
                        compared += got.len();
                        // Buffer returned: the next volume takes it back (`vol_pool`), dirty.
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

/// Survey of the common subexpressions (SAMPLER_CSE track): uncached nodes that
/// the volume evaluation of a root computes several times identically (same structure,
/// same children), with the number of noises under each.
#[test]
#[ignore = "benchmark"]
fn cse_census() {
    use hashbrown::HashMap;
    use rc_worldgen::sampler::S;
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

/// Final density of 256 chunks: each path alone, then all; 1 thread then powers of 2
/// up to the number of cores; allocations per chunk (1 thread).
#[test]
#[ignore = "benchmark"]
fn bench_final_density() {
    let Some(pack) = common::data_pack() else { return };
    let g = Generator::new(&pack, "overworld", 42).unwrap();
    let chunks: Vec<(i32, i32)> = (0..16).flat_map(|x| (0..16).map(move |z| (x - 8, z - 8))).collect();
    let mut cases = vec![("old", Opts::OLD)];
    cases.extend(VARIANTS);
    let max = std::thread::available_parallelism().map_or(1, |n| n.get());
    // `kept`: one context per thread kept from one chunk to the next, output buffer handed back (case where
    // `vol_pool` has something to take back); otherwise a new context per chunk (the terrain).
    for pass in 0..2 {
        for ((label, o), keep) in cases.iter().flat_map(|c| [(c, false), (c, true)]) {
            let mut t = 1;
            let mut line = format!("pass {pass} {label:12} {}", if keep { "kept " } else { "new  " });
            while t <= max {
                let pool = rayon::ThreadPoolBuilder::new().num_threads(t).build().unwrap();
                let run = |cs: &[(i32, i32)]| {
                    use rayon::prelude::*;
                    pool.install(|| {
                        if keep {
                            cs.par_iter()
                                .map_init(
                                    || Ctx::cached_with(*o),
                                    |ctx, &(x, z)| {
                                        let b = g.volume(g.router.final_density, ctx, &g.chunk_volume(x, z));
                                        let n = b.len();
                                        ctx.recycle(b);
                                        n
                                    },
                                )
                                .sum::<usize>()
                        } else {
                            cs.par_iter().map(|&(x, z)| final_density_with(&g, *o, x, z).len()).sum::<usize>()
                        }
                    })
                };
                run(&chunks[..8]);
                let (a0, b0) = (ALLOCS.load(Ordering::Relaxed), BYTES.load(Ordering::Relaxed));
                let s = Instant::now();
                std::hint::black_box(run(&chunks));
                let dt = s.elapsed().as_secs_f64() * 1e3 / chunks.len() as f64;
                if t == 1 {
                    let n = chunks.len() as u64;
                    let (a, b) = (ALLOCS.load(Ordering::Relaxed) - a0, BYTES.load(Ordering::Relaxed) - b0);
                    line += &format!(" | {} alloc, {:.0} KB / chunk", a / n, b as f64 / n as f64 / 1024.0);
                }
                line += &format!(" | {t} threads {dt:.3} ms");
                t *= 2;
            }
            eprintln!("{line}");
        }
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
