//! Micro-benches of the optimised terrain paths (C1-M09), with allocation counter:
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
