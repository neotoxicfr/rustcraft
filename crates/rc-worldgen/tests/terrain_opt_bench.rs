//! Micro-benches of the optimised terrain paths (C1-M09, C1-M12, C1-M14), with allocation counter:
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
use rc_worldgen::Ctx;
use rc_worldgen::sampler::EvalCounts;
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

/// Seconds spent per chunk in the two steps that open a chunk's filling.
#[derive(Debug, Default, Clone, Copy)]
struct FillOpening {
    /// Aquifer created (preliminary surface over its quarts).
    aquifer: f64,
    /// Final density of the chunk, in the context the aquifer was created in.
    density: f64,
}

/// Times the aquifer's creation then the final density of every chunk of `g`, each chunk in a
/// fresh context (created outside the timed parts), on the calling thread.
fn time_fill_opening(t: &TerrainGenerator, cache: &BiomeCache, g: &[(i32, i32)]) -> FillOpening {
    let mut sum = FillOpening::default();
    for &(x, z) in g {
        let vol = t.fill_volume(x, z).unwrap();
        let mut ctx = Ctx::cached();
        let s = Instant::now();
        std::hint::black_box(t.new_chunk_aquifer(&mut ctx, &vol, cache));
        sum.aquifer += s.elapsed().as_secs_f64();
        let s = Instant::now();
        std::hint::black_box(t.g.volume(t.g.router.final_density, &mut ctx, &vol));
        sum.density += s.elapsed().as_secs_f64();
    }
    FillOpening { aquifer: sum.aquifer / g.len() as f64, density: sum.density / g.len() as f64 }
}

/// [`time_fill_opening`] with the chunks spread over `threads` threads: mean time a thread spends
/// per chunk in each step (what a chunk costs while the other cores generate too).
fn time_fill_opening_on(t: &TerrainGenerator, cache: &BiomeCache, g: &[(i32, i32)], threads: usize) -> FillOpening {
    if threads == 1 {
        return time_fill_opening(t, cache, g);
    }
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
    let parts: Vec<FillOpening> = pool
        .install(|| g.par_chunks(g.len().div_ceil(threads)).map(|part| time_fill_opening(t, cache, part)).collect());
    let n = parts.len() as f64;
    FillOpening {
        aquifer: parts.iter().map(|p| p.aquifer).sum::<f64>() / n,
        density: parts.iter().map(|p| p.density).sum::<f64>() / n,
    }
}

/// What the evaluator does over the chunks of `g` to prepare the batch (cold cache), to create
/// the aquifers alone, then for the whole filling (deterministic: the same counts on a loaded
/// machine). Everything runs on the single thread of a pool: the counters are per thread, and the
/// preparation hands its work to the pool it is called from.
fn count_fill_opening(t: &TerrainGenerator, g: &[(i32, i32)]) -> [EvalCounts; 3] {
    use rc_worldgen::sampler::{count_evaluations, take_eval_counts};
    let one_thread = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    one_thread.install(|| {
        count_evaluations(true);
        take_eval_counts();
        let mut cache = BiomeCache::default();
        t.prepare_biomes(&mut cache, g);
        let preparation = take_eval_counts();
        for &(x, z) in g {
            let vol = t.fill_volume(x, z).unwrap();
            std::hint::black_box(t.new_chunk_aquifer(&mut Ctx::cached(), &vol, &cache));
        }
        let creation = take_eval_counts();
        for &(x, z) in g {
            std::hint::black_box(t.generate_until(x, z, &cache, None, Stage::Fill));
        }
        let filling = take_eval_counts();
        count_evaluations(false);
        [preparation, creation, filling]
    })
}

/// Wall seconds per chunk of the whole filling (aquifer, final density, blocks) of `g` on
/// `threads` threads.
fn time_filling_on(t: &TerrainGenerator, cache: &BiomeCache, g: &[(i32, i32)], threads: usize) -> f64 {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
    let s = Instant::now();
    pool.install(|| {
        g.par_iter().for_each(|&(x, z)| {
            std::hint::black_box(t.generate_until(x, z, cache, None, Stage::Fill));
        });
    });
    s.elapsed().as_secs_f64() / g.len() as f64
}

/// Seconds the surface tiles of `g` alone take on `threads` threads (the biomes already cached;
/// without tiles, what is left is the bookkeeping of a prepared batch), and the tiles computed.
fn time_surface_tiles_alone(t: &TerrainGenerator, g: &[(i32, i32)], threads: usize) -> (f64, usize) {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
    let mut cache = BiomeCache::default();
    pool.install(|| t.prepare_biomes(&mut cache, g));
    cache.surface_tiles = Default::default();
    let s = Instant::now();
    pool.install(|| t.prepare_biomes(&mut cache, g));
    (s.elapsed().as_secs_f64(), cache.surface_tiles.len())
}

/// Seconds per chunk to prepare then fill each chunk of `scattered` as a batch of its own, from
/// an empty cache, on one thread: the worst case of the tiles (whole tiles for one chunk).
fn time_chunks_prepared_alone(t: &TerrainGenerator, scattered: &[(i32, i32)]) -> f64 {
    let one_thread = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    let s = Instant::now();
    one_thread.install(|| {
        for &(x, z) in scattered {
            let mut cache = BiomeCache::default();
            t.prepare_biomes(&mut cache, &[(x, z)]);
            std::hint::black_box(t.generate_until(x, z, &cache, None, Stage::Fill));
        }
    });
    s.elapsed().as_secs_f64() / scattered.len() as f64
}

/// A flight east over fresh terrain: 12 batches of one row of 24 chunks each, every batch
/// prepared then filled before the next (the steady state of a moving player: most tiles come
/// from the batches before). Wall seconds per chunk spent preparing, then filling, on `threads`
/// threads.
fn time_flight(t: &TerrainGenerator, threads: usize) -> (f64, f64) {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
    let mut cache = BiomeCache::default();
    let (mut preparing, mut filling) = (0.0, 0.0);
    for row in flight_rows() {
        let s = Instant::now();
        pool.install(|| t.prepare_biomes(&mut cache, &row));
        preparing += s.elapsed().as_secs_f64();
        let s = Instant::now();
        pool.install(|| {
            row.par_iter().for_each(|&(cx, cz)| {
                std::hint::black_box(t.generate_until(cx, cz, &cache, None, Stage::Fill));
            });
        });
        filling += s.elapsed().as_secs_f64();
    }
    (preparing / FLIGHT_CHUNKS as f64, filling / FLIGHT_CHUNKS as f64)
}

const FLIGHT_CHUNKS: usize = 12 * 24;

/// Batches of the flight: rows of 24 chunks, one chunk further east each.
fn flight_rows() -> Vec<Vec<(i32, i32)>> {
    (0..12).map(|x| (0..24).map(|z| (x + 100, z - 50)).collect()).collect()
}

/// What the evaluator computes over the flight of [`time_flight`]: preparing, then filling (one
/// thread: the counters are per thread).
fn count_flight(t: &TerrainGenerator) -> [EvalCounts; 2] {
    use rc_worldgen::sampler::{count_evaluations, take_eval_counts};
    let one_thread = rayon::ThreadPoolBuilder::new().num_threads(1).build().unwrap();
    one_thread.install(|| {
        let mut cache = BiomeCache::default();
        let (mut preparing, mut filling) = (EvalCounts::default(), EvalCounts::default());
        count_evaluations(true);
        take_eval_counts();
        for row in flight_rows() {
            t.prepare_biomes(&mut cache, &row);
            preparing = sum_counts(preparing, take_eval_counts());
            for &(cx, cz) in &row {
                std::hint::black_box(t.generate_until(cx, cz, &cache, None, Stage::Fill));
            }
            filling = sum_counts(filling, take_eval_counts());
        }
        count_evaluations(false);
        [preparing, filling]
    })
}

/// Sum of the fields the bench prints (the counts themselves only subtract).
fn sum_counts(a: EvalCounts, b: EvalCounts) -> EvalCounts {
    EvalCounts {
        cache_volume_values: a.cache_volume_values + b.cache_volume_values,
        top_probes: a.top_probes + b.top_probes,
        point_noises: a.point_noises + b.point_noises,
        point_splines: a.point_splines + b.point_splines,
        cache_volume_sublattice: a.cache_volume_sublattice + b.cache_volume_sublattice,
        ..a
    }
}

/// Seconds a cold batch preparation of `g` takes on `threads` threads (biomes and surface tiles).
fn time_cold_preparation(t: &TerrainGenerator, g: &[(i32, i32)], threads: usize) -> f64 {
    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
    let mut cache = BiomeCache::default();
    let s = Instant::now();
    pool.install(|| t.prepare_biomes(&mut cache, g));
    s.elapsed().as_secs_f64()
}

/// A path of the aquifer's preliminary surface, and the grid's cache prepared on it.
struct SurfacePath {
    name: &'static str,
    opts: TerrainOpts,
    cache: BiomeCache,
}

/// The paths: a window computed by each chunk (old), levels copied from the tiles of the batch,
/// and the tiles also carrying the 2D climate under them (the default).
fn aquifer_surface_paths() -> [(&'static str, TerrainOpts); 3] {
    let window = TerrainOpts { aquifer_surface_tiles: false, shared_climate_tiles: false, ..TerrainOpts::NEW };
    let levels_only = TerrainOpts { shared_climate_tiles: false, ..TerrainOpts::NEW };
    [("window per chunk", window), ("tiles, levels only", levels_only), ("tiles with climate", TerrainOpts::NEW)]
}

/// Best and median of timed rounds (seconds), in µs.
fn best_and_median(rounds: &[f64]) -> (f64, f64) {
    let mut v: Vec<f64> = rounds.iter().map(|s| s * 1e6).collect();
    v.sort_by(f64::total_cmp);
    (v[0], v[v.len() / 2])
}

/// Runs `measure` on every path for `rounds` rounds and returns the measurements per path. Each
/// round starts from another path, so that none always runs right after the same one.
fn measure_in_rotation<M>(
    t: &mut TerrainGenerator,
    paths: &[SurfacePath],
    rounds: usize,
    mut measure: impl FnMut(&TerrainGenerator, &SurfacePath) -> M,
) -> Vec<Vec<M>> {
    let mut measured: Vec<Vec<M>> = paths.iter().map(|_| Vec::new()).collect();
    for round in 0..rounds {
        for i in (0..paths.len()).map(|k| (round + k) % paths.len()) {
            t.opts = paths[i].opts;
            measured[i].push(measure(t, &paths[i]));
        }
    }
    measured
}

/// C1-M14: share of the aquifer's creation in a chunk, and what the shared surface tiles and the
/// 2D climate they carry change. 12 × 12 grid, seed 0; rounds in rotation, best and median.
#[test]
#[ignore = "benchmark"]
fn aquifer_creation_bench() {
    let Some(pack) = common::data_pack() else { return };
    // One generator for every path, its switches set before each measurement: two generators do
    // not sit in the same memory, and the first one built measured 10 to 30 % slower on the biomes.
    let mut t = TerrainGenerator::from_preset(&pack, "minecraft:normal", "minecraft:overworld", 0).unwrap();
    let g = grid();
    let paths: Vec<SurfacePath> = aquifer_surface_paths()
        .into_iter()
        .map(|(name, opts)| {
            t.opts = opts;
            let mut cache = BiomeCache::default();
            t.prepare_biomes(&mut cache, &g);
            SurfacePath { name, opts, cache }
        })
        .collect();
    let all_threads = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut threads = 1;
    while threads <= all_threads {
        report_grid_times(&mut t, &paths, &g, threads);
        threads *= 2;
    }
    report_chunks_prepared_alone(&mut t, &paths);
    for threads in [1, all_threads] {
        report_flight_times(&mut t, &paths, threads);
    }
    report_counts(&mut t, &paths, &g);
}

/// What the bench times on the grid for one path and one round.
struct GridTimes {
    opening: FillOpening,
    filling: f64,
    cold_preparation: f64,
    tiles_alone: f64,
    tiles: usize,
}

/// Grid: the aquifer's creation and the final density per chunk, the whole filling, a cold
/// preparation of the batch, and the surface tiles alone.
fn report_grid_times(t: &mut TerrainGenerator, paths: &[SurfacePath], g: &[(i32, i32)], threads: usize) {
    let measured = measure_in_rotation(t, paths, 9, |t, path| {
        let (tiles_alone, tiles) = time_surface_tiles_alone(t, g, threads);
        GridTimes {
            opening: time_fill_opening_on(t, &path.cache, g, threads),
            filling: time_filling_on(t, &path.cache, g, threads),
            cold_preparation: time_cold_preparation(t, g, threads) / g.len() as f64,
            tiles_alone,
            tiles,
        }
    });
    for (path, rounds) in paths.iter().zip(&measured) {
        let stat = |f: fn(&GridTimes) -> f64| best_and_median(&rounds.iter().map(f).collect::<Vec<_>>());
        let (aquifer, density) = (stat(|r| r.opening.aquifer), stat(|r| r.opening.density));
        let (filling, preparation, tiles) =
            (stat(|r| r.filling), stat(|r| r.cold_preparation), stat(|r| r.tiles_alone));
        eprintln!(
            "{threads:>2} threads {:>20}: aquifer {:.1} / {:.1} µs, final density {:.1} / {:.1}; wall: whole filling \
             {:.1} / {:.1}, cold batch preparation {:.1} / {:.1} (best / median per chunk); the {} surface tiles alone \
             {:.0} / {:.0} µs wall",
            path.name,
            aquifer.0,
            aquifer.1,
            density.0,
            density.1,
            filling.0,
            filling.1,
            preparation.0,
            preparation.1,
            rounds[0].tiles,
            tiles.0,
            tiles.1
        );
    }
}

/// The worst case of the tiles: scattered chunks, each prepared and filled as a batch of its own.
fn report_chunks_prepared_alone(t: &mut TerrainGenerator, paths: &[SurfacePath]) {
    let scattered: Vec<(i32, i32)> =
        (0..24).map(|i: i32| (i.wrapping_mul(7919) % 4000 - 2000, i.wrapping_mul(104_729) % 4000 - 2000)).collect();
    let measured = measure_in_rotation(t, paths, 5, |t, _| time_chunks_prepared_alone(t, &scattered));
    for (path, rounds) in paths.iter().zip(&measured) {
        let (best, median) = best_and_median(rounds);
        eprintln!(
            "{:>20}: a chunk prepared and filled alone, 1 thread: {best:.0} / {median:.0} µs (best / median)",
            path.name
        );
    }
}

/// The steady state of a moving player: [`time_flight`].
fn report_flight_times(t: &mut TerrainGenerator, paths: &[SurfacePath], threads: usize) {
    let measured = measure_in_rotation(t, paths, 5, |t, _| time_flight(t, threads));
    for (path, rounds) in paths.iter().zip(&measured) {
        let stat = |f: fn(&(f64, f64)) -> f64| best_and_median(&rounds.iter().map(f).collect::<Vec<_>>());
        let (preparation, filling, both) = (stat(|r| r.0), stat(|r| r.1), stat(|r| r.0 + r.1));
        eprintln!(
            "{threads:>2} threads {:>20}, flight: preparation {:.1} / {:.1} µs wall per chunk, filling {:.1} / {:.1}, \
             both {:.1} / {:.1} (best / median)",
            path.name, preparation.0, preparation.1, filling.0, filling.1, both.0, both.1
        );
    }
}

/// Deterministic counts per chunk (the same on a loaded machine), on the grid then over the
/// flight, and the memory of the tiles.
fn report_counts(t: &mut TerrainGenerator, paths: &[SurfacePath], g: &[(i32, i32)]) {
    let print = |path: &SurfacePath, step: &str, c: EvalCounts, chunks: usize| {
        let per_chunk = |n: u64| n as f64 / chunks as f64;
        eprintln!(
            "{:>20}, {step}: {:.1} cache values computed, {:.1} top probes, {:.1} point noises, {:.1} point splines, \
             {:.1} sub-lattice reads (per chunk)",
            path.name,
            per_chunk(c.cache_volume_values),
            per_chunk(c.top_probes),
            per_chunk(c.point_noises),
            per_chunk(c.point_splines),
            per_chunk(c.cache_volume_sublattice),
        );
    };
    for path in paths {
        t.opts = path.opts;
        let tiles = &path.cache.surface_tiles;
        eprintln!(
            "{:>20}: {} surface tiles for the grid, {:.1} KiB, {:?} climate caches kept per tile (fewest, most)",
            path.name,
            tiles.len(),
            tiles.bytes() as f64 / 1024.0,
            tiles.climate_caches_kept()
        );
        for (step, counts) in ["grid, cold preparation", "grid, aquifer creation", "grid, whole filling"]
            .iter()
            .zip(count_fill_opening(t, g))
        {
            print(path, step, counts, g.len());
        }
        for (step, counts) in ["flight, preparation", "flight, filling"].iter().zip(count_flight(t)) {
            print(path, step, counts, FLIGHT_CHUNKS);
        }
    }
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
