//! Micro-benches of the material rules (C1-M08), with allocation counter:
//! `cargo test --release -p rc-worldgen --test material_opt_bench -- --ignored --nocapture`.
//! Cost of the surface = surface step − filling step (same chunk); 12 × 12 grid from
//! (0, 0), seed 0, then 24 badlands chunks. Each variant is measured twice in
//! alternation with the old path.
// Counting allocator (unsafe allowed, bench only, like rc-light/benches).
#![allow(unsafe_code)]

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Instant;

use rayon::prelude::*;
use rc_worldgen::Ctx;
use rc_worldgen::Volume;
use rc_worldgen::biome::{ChunkBiomes, Searcher};
use rc_worldgen::material::{BiomeView, Env, MatOpts, possible_biomes, record_top_material_calls};
use rc_worldgen::proto::ProtoTerrain;
use rc_worldgen::terrain::{BiomeCache, Stage, TerrainGenerator};

struct Counting;

static ALLOCS: AtomicU64 = AtomicU64::new(0);
static BYTES: AtomicU64 = AtomicU64::new(0);

// SAFETY: delegates everything to the system allocator, only counts the calls.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        BYTES.fetch_add(l.size() as u64, Relaxed);
        // SAFETY: same guarantees as the caller.
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // SAFETY: same guarantees as the caller.
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        BYTES.fetch_add(n as u64, Relaxed);
        // SAFETY: same guarantees as the caller.
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static A: Counting = Counting;

/// ms / chunk on 1 thread, allocations and MB allocated / chunk.
fn time_stage(t: &TerrainGenerator, cache: &BiomeCache, g: &[(i32, i32)], stage: Stage) -> (f64, f64, f64) {
    let (a0, b0) = (ALLOCS.load(Relaxed), BYTES.load(Relaxed));
    let s = Instant::now();
    for &(x, z) in g {
        std::hint::black_box(t.generate_until(x, z, cache, None, stage));
    }
    let n = g.len() as f64;
    let dt = s.elapsed().as_secs_f64() * 1e3 / n;
    (dt, (ALLOCS.load(Relaxed) - a0) as f64 / n, (BYTES.load(Relaxed) - b0) as f64 / n / 1e6)
}

fn badlands(t: &TerrainGenerator, n: usize) -> Vec<(i32, i32)> {
    let ids: Vec<u16> = ["minecraft:badlands", "minecraft:eroded_badlands", "minecraft:wooded_badlands"]
        .iter()
        .filter_map(|b| t.biome_names.iter().position(|x| x == b))
        .map(|i| i as u16)
        .collect();
    let grid: Vec<(i32, i32)> = (-60..60).flat_map(|x| (-60..60).map(move |z| (x * 5, z * 5))).collect();
    let hits: Vec<bool> =
        grid.par_iter().map(|&(x, z)| t.chunk_biomes(x, z).data.iter().all(|b| ids.contains(b))).collect();
    grid.iter().zip(&hits).filter(|(_, h)| **h).map(|(c, _)| *c).take(n).collect()
}

/// Biomes of the 3 × 3 chunks around (cx, cz), in the order of the surface step.
fn region(cache: &BiomeCache, cx: i32, cz: i32) -> [&ChunkBiomes; 9] {
    std::array::from_fn(|i| &cache.chunks[&(cx + i as i32 / 3 - 1, cz + i as i32 % 3 - 1)])
}

/// Vein volume of the surface step: the filling volume cut at the top of the highest filled section.
fn surface_volume(t: &TerrainGenerator, chunk: &ProtoTerrain) -> (Volume, Volume) {
    let full = t.fill_volume(chunk.cx, chunk.cz).expect("filling volume");
    let top_section = chunk.highest_filled_section().map_or(-1, |s| s as i32);
    let max_block_y = chunk.min_y + top_section * 16 + 15;
    let narrowed = Volume::new(full.sx, (max_block_y - full.y0 + 1).max(1), full.sz, full.x0, full.y0, full.z0);
    (full, narrowed)
}

/// Milliseconds per chunk of the surface step phases on 1 thread: vein pre-filling (compile
/// without the chunk biomes), compile with the row classes, whole surface step; the column walk
/// is the whole step minus the compile.
fn surface_phases(t: &TerrainGenerator, cache: &BiomeCache, filled: &[ProtoTerrain]) -> [f64; 3] {
    let mut out = [0.0; 3];
    let mut copies = filled.to_vec();
    for (k, slot) in out.iter_mut().enumerate() {
        let s = Instant::now();
        for (chunk, copy) in filled.iter().zip(copies.iter_mut()) {
            let chunks = region(cache, chunk.cx, chunk.cz);
            let possible = possible_biomes(t.biome_names.len(), &chunks);
            let mut view = BiomeView::Region {
                chunks,
                cx: chunk.cx,
                cz: chunk.cz,
                zoom_seed: t.zoom_seed,
                min_qy: t.world_min_y >> 2,
                max_qy: (t.world_min_y >> 2) + (t.world_height >> 2) - 1,
            };
            let mut ctx = Ctx::cached();
            let mut env = Env { g: &t.g, ctx: &mut ctx, biomes: &mut view };
            let (full, narrowed) = surface_volume(t, chunk);
            match k {
                0 => drop(std::hint::black_box(t.material.compile(&mut env, narrowed, None))),
                1 => drop(std::hint::black_box(t.material.compile(&mut env, narrowed, Some(&possible)))),
                _ => t.material.build_surface(copy, &t.blocks, &mut env, full, Some(&possible)),
            }
        }
        *slot = s.elapsed().as_secs_f64() * 1e3 / filled.len() as f64;
    }
    out
}

/// Cave top material on 1 thread: calls per chunk and microseconds per call, replayed right after
/// the surface step of the same chunk (so that the column levels kept for it are found), through
/// the view the caves use today or the biomes of the chunks around.
fn top_material_cost(t: &TerrainGenerator, cache: &BiomeCache, g: &[(i32, i32)], from_chunks: bool) -> (f64, f64) {
    let (mut n, mut seconds) = (0usize, 0.0);
    for &(cx, cz) in g {
        let (_, calls) = record_top_material_calls(|| t.generate(cx, cz, cache, None));
        let chunk = t.generate_until(cx, cz, cache, None, Stage::Surface);
        let mut ctx = Ctx::cached();
        let mut view = if from_chunks {
            BiomeView::ChunkThenSource {
                chunks: region(cache, cx, cz),
                cx,
                cz,
                zoom_seed: t.zoom_seed,
                min_qy: t.world_min_y >> 2,
                max_qy: (t.world_min_y >> 2) + (t.world_height >> 2) - 1,
                src: &t.source,
                searcher: Searcher::default(),
            }
        } else {
            BiomeView::Source { src: &t.source, zoom_seed: t.zoom_seed, searcher: Searcher::default() }
        };
        let mut env = Env { g: &t.g, ctx: &mut ctx, biomes: &mut view };
        let s = Instant::now();
        for &[x, y, z] in &calls {
            let (lx, lz) = (x - chunk.min_x(), z - chunk.min_z());
            std::hint::black_box(t.material.top_material(&chunk, &mut env, lx, y, lz, false));
        }
        seconds += s.elapsed().as_secs_f64();
        n += calls.len();
    }
    (n as f64 / g.len() as f64, seconds * 1e6 / n.max(1) as f64)
}

/// Chunks of `t` whose biomes are all (`all`) or partly among `names`, on a coarse grid.
fn chunks_in(t: &TerrainGenerator, names: &[&str], all: bool, n: usize) -> Vec<(i32, i32)> {
    let ids: Vec<u16> =
        names.iter().filter_map(|b| t.biome_names.iter().position(|x| x == b)).map(|i| i as u16).collect();
    let grid: Vec<(i32, i32)> = (-60..60).flat_map(|x| (-60..60).map(move |z| (x * 7, z * 7))).collect();
    let hits: Vec<bool> = grid
        .par_iter()
        .map(|&(x, z)| {
            let b = t.chunk_biomes(x, z);
            if all { b.data.iter().all(|b| ids.contains(b)) } else { b.data.iter().any(|b| ids.contains(b)) }
        })
        .collect();
    grid.iter().zip(&hits).filter(|(_, h)| **h).map(|(c, _)| *c).take(n).collect()
}

/// Highest block that is neither air nor fluid in a filled chunk.
fn highest_solid(t: &TerrainGenerator, c: &ProtoTerrain) -> i32 {
    (0..256)
        .map(|i| {
            let (x, z) = (i & 15, i >> 4);
            (c.min_y..=c.surface_height(x, z)).rev().find(|&y| t.blocks.is_solid(c.get(x, y, z))).unwrap_or(c.min_y)
        })
        .max()
        .unwrap_or(c.min_y)
}

/// Milliseconds per chunk of the vein pre-filling alone, rows restricted to the live ones, with
/// and without the bound at the highest solid (5 repetitions, 1 thread).
fn vein_prefill_cost(t: &TerrainGenerator, filled: &[ProtoTerrain]) -> (f64, f64) {
    let yrange = MatOpts { vein_yrange: true, vein_lazyrich: true, vein_shared: true, ..MatOpts::OLD };
    let bounded = MatOpts { vein_top_bound: true, ..yrange };
    let mut out = [0.0; 2];
    for (k, o) in [yrange, bounded].into_iter().enumerate() {
        let s = Instant::now();
        for _ in 0..5 {
            for c in filled {
                let mut ctx = Ctx::cached();
                let mut view =
                    BiomeView::Source { src: &t.source, zoom_seed: t.zoom_seed, searcher: Searcher::default() };
                let mut env = Env { g: &t.g, ctx: &mut ctx, biomes: &mut view };
                let (_, narrowed) = surface_volume(t, c);
                std::hint::black_box(t.material.prefill_with(&mut env, narrowed, o, Some(highest_solid(t, c))));
            }
        }
        out[k] = s.elapsed().as_secs_f64() * 1e3 / (5 * filled.len()) as f64;
    }
    (out[0], out[1])
}

/// The new paths of the first material work, without the band kernels and what came with them.
const EARLIER_PATHS: MatOpts = MatOpts {
    band_kernels: false,
    row_kernel: false,
    direct_solid_write: false,
    vein_top_bound: false,
    cave_top_column_memo: false,
    ..MatOpts::NEW
};

/// Phases of the surface step for each lead alone over the earlier paths, then combined, and cost
/// of the cave top material, 2 alternating passes, 1 thread:
/// `cargo test --release -p rc-worldgen --test material_opt_bench surface_phases -- --ignored --nocapture`.
#[test]
#[ignore = "benchmark"]
fn surface_phases_bench() {
    let Some(pack) = common::data_pack() else { return };
    let mut t = TerrainGenerator::from_preset(&pack, "minecraft:normal", "minecraft:overworld", 0).unwrap();
    let grid: Vec<(i32, i32)> = (0..12).flat_map(|x| (0..12).map(move |z| (x, z))).collect();
    let bad = badlands(&t, 24);
    let deep = ["minecraft:deep_ocean", "minecraft:deep_cold_ocean", "minecraft:deep_lukewarm_ocean"];
    let ocean = chunks_in(&t, &deep, true, 24);
    let peaks = ["minecraft:jagged_peaks", "minecraft:stony_peaks", "minecraft:frozen_peaks", "minecraft:snowy_slopes"];
    let mountains = chunks_in(&t, &peaks, false, 24);
    eprintln!("{} badlands, {} deep ocean, {} mountain chunks", bad.len(), ocean.len(), mountains.len());
    let mut cache = BiomeCache::default();
    for g in [&grid, &bad, &ocean, &mountains] {
        t.prepare_biomes(&mut cache, g);
    }
    let filled = |t: &TerrainGenerator, g: &[(i32, i32)]| -> Vec<ProtoTerrain> {
        g.iter().map(|&(x, z)| t.generate_until(x, z, &cache, None, Stage::Fill)).collect()
    };
    let sets = [
        ("grid", filled(&t, &grid)),
        ("badlands", filled(&t, &bad)),
        ("ocean", filled(&t, &ocean)),
        ("mountain", filled(&t, &mountains)),
    ];
    let e = EARLIER_PATHS;
    let variants = [
        ("old", MatOpts::OLD),
        ("earlier", e),
        ("+band_kernels", MatOpts { band_kernels: true, ..e }),
        ("+band+row", MatOpts { band_kernels: true, row_kernel: true, ..e }),
        ("+direct_write", MatOpts { direct_solid_write: true, ..e }),
        ("+band+direct", MatOpts { band_kernels: true, direct_solid_write: true, ..e }),
        ("+vein_top", MatOpts { vein_top_bound: true, ..e }),
        ("new but row", MatOpts { row_kernel: false, ..MatOpts::NEW }),
        ("new", MatOpts::NEW),
    ];
    for pass in 0..2 {
        for (name, o) in variants {
            t.material.opts = o;
            for (label, f) in &sets {
                let [prefill, compile, step] = surface_phases(&t, &cache, f);
                eprintln!(
                    "pass {pass} {label:>8} {name:>14}: veins {prefill:.3}, walk {:.3}, step {step:.3} ms / chunk",
                    step - compile
                );
            }
        }
        for (label, f) in &sets[..3] {
            let (unbounded, bounded) = vein_prefill_cost(&t, f);
            eprintln!(
                "pass {pass} {label:>8} veins alone: {unbounded:.3} ms / chunk, bounded at the highest solid {bounded:.3}"
            );
        }
        for (name, o, from_chunks) in [
            ("earlier, source", e, false),
            ("+memo, source", MatOpts { cave_top_column_memo: true, ..e }, false),
            ("earlier, chunks", e, true),
            ("new, chunks", MatOpts::NEW, true),
        ] {
            t.material.opts = o;
            let (calls, us) = top_material_cost(&t, &cache, &grid, from_chunks);
            eprintln!("pass {pass} caves {name:>16}: {calls:.1} top material calls / chunk, {us:.2} µs / call");
        }
    }
}

/// "Same block" by name against by base identifier over every pair of states of the table
/// (the eroded badlands compare each cell of a column with the default block).
#[test]
#[ignore = "benchmark"]
fn same_block_bench() {
    let Some(pack) = common::data_pack() else { return };
    let t = TerrainGenerator::from_preset(&pack, "minecraft:normal", "minecraft:overworld", 0).unwrap();
    let n = t.blocks.len() as u16;
    for pass in 0..2 {
        for (name, by_id) in [("by name", false), ("by base id", true)] {
            let s = Instant::now();
            let mut hits = 0u32;
            for _ in 0..200 {
                for a in 0..n {
                    for b in 0..n {
                        let same = if by_id {
                            t.blocks.same_block_by_base_id(a, b)
                        } else {
                            t.blocks.same_block_by_name(a, b)
                        };
                        hits += u32::from(std::hint::black_box(same));
                    }
                }
            }
            let ns = s.elapsed().as_secs_f64() * 1e9 / (200.0 * f64::from(n) * f64::from(n));
            eprintln!("pass {pass} same block {name:>10}: {ns:.2} ns / pair ({hits} hits)");
        }
    }
}

#[test]
#[ignore = "benchmark"]
fn material_opt_bench() {
    let Some(pack) = common::data_pack() else { return };
    let mut t = TerrainGenerator::from_preset(&pack, "minecraft:normal", "minecraft:overworld", 0).unwrap();
    let grid: Vec<(i32, i32)> = (0..12).flat_map(|x| (0..12).map(move |z| (x, z))).collect();
    let bad = badlands(&t, 24);
    eprintln!("{} badlands chunks", bad.len());
    let mut cache = BiomeCache::default();
    t.prepare_biomes(&mut cache, &grid);
    t.prepare_biomes(&mut cache, &bad);
    let o = MatOpts::OLD;
    let _ = t.generate(50, 50, &cache, None);
    let variants = [
        ("old", o),
        ("ore_idx", MatOpts { ore_idx: true, ..o }),
        ("ore_early", MatOpts { ore_early: true, ..o }),
        ("surf_skip", MatOpts { surf_skip: true, ..o }),
        ("surf_bands", MatOpts { surf_bands: true, ..o }),
        ("skip+bands", MatOpts { surf_skip: true, surf_bands: true, ..o }),
        ("vein_yrange", MatOpts { vein_yrange: true, ..o }),
        ("yrange+lazyrich", MatOpts { vein_yrange: true, vein_lazyrich: true, ..o }),
        ("yrange+shared", MatOpts { vein_yrange: true, vein_shared: true, ..o }),
        ("yrange+lazy+shared", MatOpts { vein_yrange: true, vein_lazyrich: true, vein_shared: true, ..o }),
        ("reuse", MatOpts { reuse: true, ..o }),
        ("colcache", MatOpts { colcache: true, ..o }),
        ("all but shared", MatOpts { vein_shared: false, ..MatOpts::NEW }),
        ("all but bands", MatOpts { surf_bands: false, ..MatOpts::NEW }),
        ("all but skip", MatOpts { surf_skip: false, ..MatOpts::NEW }),
        ("earlier", EARLIER_PATHS),
        ("all", MatOpts::NEW),
    ];
    for (label, g) in [("grid", &grid), ("badlands", &bad)] {
        t.material.opts = o;
        let (fill, fa, fb) = time_stage(&t, &cache, g, Stage::Fill);
        eprintln!("{label} filling: {fill:.3} ms / chunk, {fa:.0} allocations, {fb:.2} MB");
        for pass in 0..2 {
            for (name, v) in variants {
                t.material.opts = v;
                let (f, fa, fb) = time_stage(&t, &cache, g, Stage::Fill);
                let (s, sa, sb) = time_stage(&t, &cache, g, Stage::Surface);
                eprintln!(
                    "pass {pass} {label} surface {name:>20}: {:.3} ms / chunk (step {s:.3}), {:.0} allocations, {:.2} MB",
                    s - f,
                    sa - fa,
                    sb - fb
                );
            }
        }
    }
    // Caves step (top material replayed), grid.
    for pass in 0..2 {
        for (name, v) in
            [("old", o), ("reuse", MatOpts { reuse: true, ..o }), ("earlier", EARLIER_PATHS), ("all", MatOpts::NEW)]
        {
            t.material.opts = v;
            let (s, sa, sb) = time_stage(&t, &cache, &grid, Stage::Surface);
            let (c, ca, cb) = time_stage(&t, &cache, &grid, Stage::Carvers);
            eprintln!(
                "pass {pass} caves {name:>8}: {:.3} ms / chunk (step {c:.3}), {:.0} allocations, {:.2} MB",
                c - s,
                ca - sa,
                cb - sb
            );
        }
    }
    // Surface, old against new, threads in powers of 2.
    let n = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut threads = 1;
    while threads <= n {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
        for (name, v) in [("old", o), ("earlier", EARLIER_PATHS), ("new", MatOpts::NEW)] {
            t.material.opts = v;
            let run = |stage| {
                let s = Instant::now();
                pool.install(|| {
                    let out: Vec<_> =
                        grid.par_iter().map(|&(x, z)| t.generate_until(x, z, &cache, None, stage)).collect();
                    std::hint::black_box(out);
                });
                s.elapsed().as_secs_f64() * 1e3 / grid.len() as f64
            };
            let (f, s) = (run(Stage::Fill), run(Stage::Surface));
            eprintln!("surface {name:>8} {threads:>2} threads: {:.3} ms / chunk (step {s:.3})", s - f);
        }
        threads *= 2;
    }
}
