// Direct read allowed (test: settings read at launch, outside the hot path); the game goes through rc_data::env_*!.
#![allow(clippy::disallowed_methods)]
//! Timing of the `terrain` step (`cargo test --release -p rc-worldgen --test terrain_perf -- --ignored --nocapture`).
//! Same grid as the Java probe (`terrain-bench`): 12 × 12 chunks from (0, 0), seed 0.
//! `RUSTCRAFT_TERRAIN_DIM=nether|end|overworld`.

mod common;

use std::time::Instant;

use rayon::prelude::*;
use rc_worldgen::terrain::{BiomeCache, Stage, TerrainGenerator};

#[test]
#[ignore = "benchmark"]
fn terrain_timing() {
    let Some(pack) = common::data_pack() else { return };
    let dim = std::env::var("RUSTCRAFT_TERRAIN_DIM").unwrap_or_else(|_| "overworld".into());
    let d = match dim.as_str() {
        "nether" => "minecraft:the_nether",
        "end" => "minecraft:the_end",
        _ => "minecraft:overworld",
    };
    let t0 = Instant::now();
    let t = TerrainGenerator::from_preset(&pack, "minecraft:normal", d, 0).unwrap();
    eprintln!("{dim}: preparation {:.1} ms", t0.elapsed().as_secs_f64() * 1e3);
    let grid: Vec<(i32, i32)> = (0..12).flat_map(|x| (0..12).map(move |z| (x, z))).collect();
    let mut cache = BiomeCache::default();
    let tb = Instant::now();
    t.prepare_biomes(&mut cache, &grid);
    eprintln!(
        "biomes ({} chunks, {} cave sources): {:.1} ms, all threads",
        cache.chunks.len(),
        cache.carver.len(),
        tb.elapsed().as_secs_f64() * 1e3
    );
    let _ = t.generate(50, 50, &cache, None);
    for (stage, name) in [(Stage::Fill, "filling"), (Stage::Surface, "+ surface"), (Stage::Carvers, "+ caves")] {
        let s = Instant::now();
        for &(x, z) in &grid {
            std::hint::black_box(t.generate_until(x, z, &cache, None, stage));
        }
        eprintln!("1 thread, {name}: {:.2} ms / chunk", s.elapsed().as_secs_f64() * 1e3 / grid.len() as f64);
    }
    // Same condition as the Java probe (cave biome of the sources recomputed at each chunk).
    let mut cold = BiomeCache::default();
    std::mem::swap(&mut cold.chunks, &mut cache.chunks);
    let s = Instant::now();
    for &(x, z) in &grid[..48] {
        std::hint::black_box(t.generate(x, z, &cold, None));
    }
    eprintln!("1 thread, cave biomes without cache: {:.2} ms / chunk", s.elapsed().as_secs_f64() * 1e3 / 48.0);
    std::mem::swap(&mut cold.chunks, &mut cache.chunks);
    for _ in 0..3 {
        let s = Instant::now();
        let out: Vec<_> = grid.par_iter().map(|&(x, z)| t.generate(x, z, &cache, None)).collect();
        let dt = s.elapsed().as_secs_f64();
        eprintln!(
            "{} threads: {} chunks in {:.1} ms ({:.3} ms / chunk)",
            rayon::current_num_threads(),
            out.len(),
            dt * 1e3,
            dt * 1e3 / out.len() as f64
        );
    }
    // View radius 12 (25 × 25 chunks) from nothing: biomes included.
    let r = 12;
    let area: Vec<(i32, i32)> = (-r..=r).flat_map(|x| (-r..=r).map(move |z| (x + 200, z + 200))).collect();
    let s = Instant::now();
    let out = t.generate_many(&area);
    let dt = s.elapsed().as_secs_f64();
    eprintln!(
        "radius {r} ({} chunks, biomes included): {:.0} ms ({:.3} ms / chunk)",
        out.len(),
        dt * 1e3,
        dt * 1e3 / out.len() as f64
    );
}
