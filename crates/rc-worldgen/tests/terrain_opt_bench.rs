//! Micro-benches of the optimised terrain paths (C1-M09, C1-M12), with allocation counter:
//! `cargo test --release -p rc-worldgen --test terrain_opt_bench -- --ignored --nocapture`.
//! 12 × 12 grid from (0, 0), seed 0, surface; each measurement is done twice in
//! alternation with the old path.
// Counting allocator (unsafe allowed, bench only, like rc-light/benches).
#![allow(unsafe_code)]

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::time::Instant;

use rayon::prelude::*;
use rc_worldgen::terrain::{BiomeCache, Stage, TerrainGenerator, TerrainOpts};

struct Counting;

static ALLOCS: AtomicU64 = AtomicU64::new(0);

// SAFETY: delegates everything to the system allocator, only counts the calls.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        // SAFETY: same guarantees as the caller.
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        // SAFETY: same guarantees as the caller.
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Relaxed);
        // SAFETY: same guarantees as the caller.
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static A: Counting = Counting;

fn grid() -> Vec<(i32, i32)> {
    (0..12).flat_map(|x| (0..12).map(move |z| (x, z))).collect()
}

/// ms / chunk on 1 thread and allocations / chunk.
fn time_stage(t: &TerrainGenerator, cache: &BiomeCache, g: &[(i32, i32)], stage: Stage) -> (f64, f64) {
    let a0 = ALLOCS.load(Relaxed);
    let s = Instant::now();
    for &(x, z) in g {
        std::hint::black_box(t.generate_until(x, z, cache, None, stage));
    }
    let dt = s.elapsed().as_secs_f64() * 1e3 / g.len() as f64;
    (dt, (ALLOCS.load(Relaxed) - a0) as f64 / g.len() as f64)
}

#[test]
#[ignore = "benchmark"]
fn terrain_opt_bench() {
    let Some(pack) = common::data_pack() else { return };
    let mut t = TerrainGenerator::from_preset(&pack, "minecraft:normal", "minecraft:overworld", 0).unwrap();
    let g = grid();
    let mut cache = BiomeCache::default();
    t.prepare_biomes(&mut cache, &g);
    let o = TerrainOpts::OLD;
    let _ = t.generate(50, 50, &cache, None);

    // Filling only (aquifers), 1 thread.
    let fill = [
        ("old", o),
        ("ids", TerrainOpts { ids: true, ..o }),
        ("colwin", TerrainOpts { aq_colwin: true, ..o }),
        ("colwin+net", TerrainOpts { aq_colwin: true, aq_net: true, ..o }),
        ("pretest", TerrainOpts { aq_pretest: true, ..o }),
        ("colwin+pretest", TerrainOpts { aq_colwin: true, aq_pretest: true, ..o }),
        ("colwin+net+pretest", TerrainOpts { aq_colwin: true, aq_net: true, aq_pretest: true, ..o }),
    ];
    for pass in 0..2 {
        for (name, v) in fill {
            t.opts = v;
            let (ms, al) = time_stage(&t, &cache, &g, Stage::Fill);
            eprintln!("pass {pass} filling {name:>20}: {ms:.3} ms / chunk, {al:.1} allocations");
        }
    }

    // Caves: masks only, 1 thread.
    let masks = [
        ("old", o),
        ("yd", TerrainOpts { carver_yd: true, ..o }),
        ("widths", TerrainOpts { carver_widths: true, ..o }),
        ("replay", TerrainOpts { carver_replay: true, ..o }),
        ("yd+widths+replay", TerrainOpts { carver_yd: true, carver_widths: true, carver_replay: true, ..o }),
    ];
    for pass in 0..2 {
        for (name, v) in masks {
            // Replay: new cache at each measurement (new generator), then hot cache.
            let mut fresh = TerrainGenerator::from_preset(&pack, "minecraft:normal", "minecraft:overworld", 0).unwrap();
            fresh.opts = v;
            for warm in [false, true] {
                let a0 = ALLOCS.load(Relaxed);
                let s = Instant::now();
                for &(x, z) in &g {
                    std::hint::black_box(fresh.carving_mask(x, z, &cache));
                }
                let ms = s.elapsed().as_secs_f64() * 1e3 / g.len() as f64;
                let al = (ALLOCS.load(Relaxed) - a0) as f64 / g.len() as f64;
                let (bytes, n) = fresh.carver_replay_usage();
                eprintln!(
                    "pass {pass} masks {name:>18} {}: {ms:.4} ms / chunk, {al:.1} allocations, cache {n} carvings {:.2} MB",
                    if warm { "hot" } else { "cold" },
                    bytes as f64 / 1e6
                );
            }
        }
    }

    // Full chunk, old against new, 1 thread then threads in powers of 2.
    for pass in 0..2 {
        for (name, v) in [("old", o), ("new", TerrainOpts::NEW)] {
            t.opts = v;
            let (ms, al) = time_stage(&t, &cache, &g, Stage::Carvers);
            eprintln!("pass {pass} full {name:>8} 1 thread: {ms:.3} ms / chunk, {al:.1} allocations");
        }
    }
    let n = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut threads = 1;
    while threads <= n {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
        for (name, v) in [("old", o), ("new", TerrainOpts::NEW)] {
            t.opts = v;
            let s = Instant::now();
            pool.install(|| {
                let out: Vec<_> = g.par_iter().map(|&(x, z)| t.generate(x, z, &cache, None)).collect();
                std::hint::black_box(out);
            });
            eprintln!(
                "full {name:>8} {threads:>2} threads: {:.3} ms / chunk",
                s.elapsed().as_secs_f64() * 1e3 / g.len() as f64
            );
        }
        threads *= 2;
    }
}

/// Cave paths over the replay, from the old one to all of them.
fn cave_variants() -> [(&'static str, TerrainOpts); 4] {
    let o = TerrainOpts::OLD;
    let replay = TerrainOpts { carver_yd: true, carver_widths: true, carver_replay: true, ..o };
    let pretest = TerrainOpts { carver_start_pretest: true, ..replay };
    let boxed = TerrainOpts { carver_reach_box: true, ..pretest };
    [
        ("old", o),
        ("replay", replay),
        ("+pretest", pretest),
        ("+pretest+box", boxed),
    ]
}

/// ms / chunk of the cave masks of `g` on `threads` threads.
fn time_masks(t: &TerrainGenerator, cache: &BiomeCache, g: &[(i32, i32)], threads: usize) -> f64 {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
    let s = Instant::now();
    if threads == 1 {
        for &(x, z) in g {
            std::hint::black_box(t.carving_mask(x, z, cache));
        }
    } else {
        pool.install(|| {
            g.par_iter().for_each(|&(x, z)| {
                std::hint::black_box(t.carving_mask(x, z, cache));
            })
        });
    }
    s.elapsed().as_secs_f64() * 1e3 / g.len() as f64
}

/// C1-M12: start pre-test, reach box, runs, batch biome tile, merged biome phase, cave top
/// material from the chunk biomes.
#[test]
#[ignore = "benchmark"]
fn cave_and_batch_biome_bench() {
    let Some(pack) = common::data_pack() else { return };
    let new_gen = |o: TerrainOpts| {
        let mut t = TerrainGenerator::from_preset(&pack, "minecraft:normal", "minecraft:overworld", 0).unwrap();
        t.opts = o;
        t
    };
    let g: Vec<(i32, i32)> = (0..24).flat_map(|x| (0..24).map(move |z| (x, z))).collect();
    let mut cache = BiomeCache::default();
    new_gen(TerrainOpts::OLD).prepare_biomes(&mut cache, &g);

    // Masks, 1 thread: cold cache (new generator) then hot.
    for pass in 0..2 {
        for (name, o) in cave_variants() {
            let t = new_gen(o);
            for warm in ["cold", "hot"] {
                let a0 = ALLOCS.load(Relaxed);
                let ms = time_masks(&t, &cache, &g, 1);
                let al = (ALLOCS.load(Relaxed) - a0) as f64 / g.len() as f64;
                let (bytes, n) = t.carver_replay_usage();
                eprintln!(
                    "pass {pass} masks {name:>18} {warm}: {ms:.4} ms / chunk, {al:.1} allocations, \
                     cache {n} carvings {:.2} MB",
                    bytes as f64 / 1e6
                );
            }
        }
    }
    // Masks, threads in powers of 2 (hot cache), best of 3 (loaded machine).
    let n = std::thread::available_parallelism().map_or(1, |n| n.get());
    for pass in 0..2 {
        let mut threads = 1;
        while threads <= n {
            for (name, o) in cave_variants() {
                let t = new_gen(o);
                time_masks(&t, &cache, &g, threads);
                let ms = (0..3).map(|_| time_masks(&t, &cache, &g, threads)).fold(f64::INFINITY, f64::min);
                eprintln!("pass {pass} masks {name:>18} {threads:>2} threads: {:.2} µs / chunk", ms * 1e3);
            }
            threads *= 2;
        }
    }
    // Reading the masks back: bit by bit against runs.
    let t = new_gen(TerrainOpts::NEW);
    let masks: Vec<_> = g.iter().map(|&(x, z)| t.carving_mask(x, z, &cache)).collect();
    for pass in 0..2 {
        for by_runs in [false, true] {
            let s = Instant::now();
            let mut sum = 0i64;
            for _ in 0..20 {
                for m in &masks {
                    m.visit_with(by_runs, |x, z, lo, hi| sum += i64::from(x + z + lo + hi));
                }
            }
            let us = s.elapsed().as_secs_f64() * 1e6 / (20 * masks.len()) as f64;
            eprintln!("pass {pass} mask read back by {}: {us:.2} µs / chunk ({sum})", if by_runs { "runs" } else { "bits" });
        }
    }
    batch_biome_bench(&new_gen);
    cave_top_bench(&new_gen, &g);
}

/// Serial bookkeeping of the batch biomes (9 × 9 batch, cache already holding everything as for
/// most of the generator's batches), a cold batch, and the masks read through the tile.
fn batch_biome_bench(new_gen: &dyn Fn(TerrainOpts) -> TerrainGenerator) {
    let base = TerrainOpts::NEW;
    let variants = [
        ("hash maps, 2 phases", TerrainOpts { batch_biome_tile: false, merged_biome_phase: false, ..base }),
        ("hash maps, merged", TerrainOpts { batch_biome_tile: false, merged_biome_phase: true, ..base }),
        ("tile, 2 phases", TerrainOpts { batch_biome_tile: true, merged_biome_phase: false, ..base }),
        ("tile, merged", base),
    ];
    let batch: Vec<(i32, i32)> = (0..9).flat_map(|x| (0..9).map(move |z| (x + 40, z - 30))).collect();
    for pass in 0..2 {
        for (name, o) in variants {
            let t = new_gen(o);
            let mut cache = BiomeCache::default();
            let s = Instant::now();
            t.prepare_biomes(&mut cache, &batch);
            let cold = s.elapsed().as_secs_f64() * 1e3;
            let s = Instant::now();
            for _ in 0..50 {
                t.prepare_biomes(&mut cache, &batch);
            }
            let warm = s.elapsed().as_secs_f64() * 1e3 / 50.0;
            let s = Instant::now();
            for &(x, z) in &batch {
                std::hint::black_box(t.carving_mask(x, z, &cache));
            }
            let masks = s.elapsed().as_secs_f64() * 1e3 / batch.len() as f64;
            let s = Instant::now();
            for &(x, z) in &batch {
                std::hint::black_box(t.carving_mask(x, z, &cache));
            }
            let masks_hot = s.elapsed().as_secs_f64() * 1e3 / batch.len() as f64;
            eprintln!(
                "pass {pass} batch biomes {name:>20}: cold {cold:.2} ms, all cached {warm:.3} ms / batch of 81, \
                 masks {masks:.4} then {masks_hot:.4} ms / chunk"
            );
        }
    }
}

/// Carve step (surface done) with the cave top material read from the source or from the chunk
/// biomes, and the chunks whose blocks differ between the two (tied climates only).
fn cave_top_bench(new_gen: &dyn Fn(TerrainOpts) -> TerrainGenerator, g: &[(i32, i32)]) {
    let mut cache = BiomeCache::default();
    let source = new_gen(TerrainOpts { cave_top_from_chunk_biomes: false, ..TerrainOpts::NEW });
    let chunks = new_gen(TerrainOpts::NEW);
    source.prepare_biomes(&mut cache, g);
    // Carve step = carved chunk − chunk after its surface, timed back to back on each chunk.
    let carve_step = |t: &TerrainGenerator, x: i32, z: i32| {
        let s = Instant::now();
        std::hint::black_box(t.generate_until(x, z, &cache, None, Stage::Surface));
        let surface = s.elapsed().as_secs_f64();
        let s = Instant::now();
        std::hint::black_box(t.generate_until(x, z, &cache, None, Stage::Carvers));
        s.elapsed().as_secs_f64() - surface
    };
    for pass in 0..3 {
        let (mut from_source, mut from_chunks) = (0.0, 0.0);
        for &(x, z) in g {
            from_source += carve_step(&source, x, z);
            from_chunks += carve_step(&chunks, x, z);
        }
        let per_chunk = |s: f64| s * 1e3 / g.len() as f64;
        eprintln!(
            "pass {pass} carve step, cave top from the source {:.4} ms / chunk, from the chunk biomes {:.4}",
            per_chunk(from_source),
            per_chunk(from_chunks)
        );
    }
    let far: Vec<(i32, i32)> = (0..3000).map(|i: i32| (i.wrapping_mul(7919) % 4000 - 2000, i.wrapping_mul(104_729) % 4000 - 2000)).collect();
    source.prepare_biomes(&mut cache, &far);
    let differ: Vec<(i32, i32)> = far
        .par_iter()
        .filter(|&&(x, z)| source.generate(x, z, &cache, None).blocks != chunks.generate(x, z, &cache, None).blocks)
        .copied()
        .collect();
    eprintln!("cave top from chunk biomes: {} chunks of {} differ {:?}", differ.len(), far.len(), &differ[..differ.len().min(10)]);
}

#[test]
#[ignore = "benchmark"]
fn base_height_bench() {
    let Some(pack) = common::data_pack() else { return };
    let cols: Vec<(i32, i32, bool)> = (0..10_000)
        .map(|i: i32| (i.wrapping_mul(7919) % 20_000 - 10_000, i.wrapping_mul(104_729) % 20_000 - 10_000, i % 2 == 0))
        .collect();
    for (preset, dim) in [
        ("minecraft:normal", "minecraft:overworld"),
        ("minecraft:normal", "minecraft:the_nether"),
        ("minecraft:normal", "minecraft:the_end"),
    ] {
        for pass in 0..2 {
            let lazy = TerrainOpts { baseh_lazy: true, ..TerrainOpts::OLD };
            let variants: [(&str, TerrainOpts, u8, i32); 11] = [
                ("old", TerrainOpts::OLD, 2, 1),
                ("fixed 1", lazy, 0, 1),
                ("fixed 4", lazy, 0, 4),
                ("fixed 8", lazy, 0, 8),
                ("double 1", lazy, 1, 1),
                ("double 2", lazy, 1, 2),
                ("probe 1", lazy, 2, 1),
                ("probe 2", lazy, 2, 2),
                ("probe 4", lazy, 2, 4),
                ("adaptive 1", lazy, 3, 1),
                ("ids", TerrainOpts { ids: true, ..TerrainOpts::OLD }, 2, 1),
            ];
            for (name, v, mode, cells) in variants {
                let mut t = TerrainGenerator::from_preset(&pack, preset, dim, 0).unwrap();
                t.opts = v;
                t.lazy_mode = mode;
                t.lazy_cells = cells;
                let a0 = ALLOCS.load(Relaxed);
                let s = Instant::now();
                for &(x, z, f) in &cols[..2000] {
                    std::hint::black_box(t.base_height(x, z, f));
                }
                let us = s.elapsed().as_secs_f64() * 1e6 / 2000.0;
                let al = (ALLOCS.load(Relaxed) - a0) as f64 / 2000.0;
                eprintln!("pass {pass} {dim} base_height {name:>12} 1 thread: {us:.1} µs / call, {al:.1} allocations");
            }
            for (name, v) in [("old", TerrainOpts::OLD), ("new", TerrainOpts::NEW)] {
                let mut t = TerrainGenerator::from_preset(&pack, preset, dim, 0).unwrap();
                t.opts = v;
                let s = Instant::now();
                let out: Vec<i32> = cols.par_iter().map(|&(x, z, f)| t.base_height(x, z, f)).collect();
                std::hint::black_box(out);
                eprintln!(
                    "pass {pass} {dim} base_height {name:>8} all threads: {:.1} µs / call",
                    s.elapsed().as_secs_f64() * 1e6 / cols.len() as f64
                );
            }
        }
    }
}
