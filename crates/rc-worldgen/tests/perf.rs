//! Order-of-magnitude measurement (`cargo test --release -p rc-worldgen --test perf -- --ignored --nocapture`).

mod common;

use std::time::Instant;

use rc_worldgen::Generator;

#[test]
#[ignore = "benchmark"]
fn final_density_timing() {
    let Some(pack) = common::data_pack() else { return };
    let t0 = Instant::now();
    let g = Generator::new(&pack, "overworld", 42).unwrap();
    eprintln!("preparation: {:.1} ms", t0.elapsed().as_secs_f64() * 1e3);
    let chunks: Vec<(i32, i32)> = (0..16).flat_map(|x| (0..16).map(move |z| (x - 8, z - 8))).collect();
    // Warm-up.
    let _ = g.final_density(100, 100);
    let t = Instant::now();
    for &(x, z) in &chunks[..32] {
        std::hint::black_box(g.final_density(x, z));
    }
    eprintln!("1 thread: {:.2} ms / chunk", t.elapsed().as_secs_f64() * 1e3 / 32.0);
    let t = Instant::now();
    let out = g.final_density_many(&chunks);
    let dt = t.elapsed().as_secs_f64();
    eprintln!(
        "{} threads: {} chunks in {:.1} ms ({:.2} ms / chunk)",
        rayon::current_num_threads(),
        out.len(),
        dt * 1e3,
        dt * 1e3 / out.len() as f64
    );
    let (src, _) = rc_worldgen::biome::BiomeSource::from_preset(&pack, "normal", "overworld").unwrap();
    let t = Instant::now();
    let mut s = rc_worldgen::biome::Searcher::default();
    for &(x, z) in &chunks[..64] {
        std::hint::black_box(src.chunk(&g, x, z, &mut s));
    }
    eprintln!("biomes, 1 thread: {:.2} ms / chunk", t.elapsed().as_secs_f64() * 1e3 / 64.0);
}
