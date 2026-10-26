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
use rc_worldgen::material::MatOpts;
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
        for (name, v) in [("old", o), ("reuse", MatOpts { reuse: true, ..o }), ("all", MatOpts::NEW)] {
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
        for (name, v) in [("old", o), ("new", MatOpts::NEW)] {
            t.material.opts = v;
            let run = |stage| {
                let s = Instant::now();
                pool.install(|| {
                    let out: Vec<_> = grid.par_iter().map(|&(x, z)| t.generate_until(x, z, &cache, None, stage)).collect();
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
