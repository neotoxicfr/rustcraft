//! Decoration bench (ignored by default): 7×7 decorated Overworld chunks (seed 1),
//! then post-processing at the pass to `full`. `cargo test --release -p rc-worldgen --test
//! decor_perf -- --ignored --nocapture`.
#![allow(clippy::disallowed_methods)]

use std::time::Instant;

use rc_worldgen::chunkgen::{DimGen, GenSpec, SourceSpec, WorldData};
use rc_worldgen::decor_region::DecoChunk;

#[test]
#[ignore = "benchmark"]
fn decorate_7x7_timing() {
    let jar = rc_data::locate("26.3").expect("installed game 26.3").client_jar;
    let v = rc_data::load_cached("26.3").expect("cache rc-data 26.3");
    let reports = rc_data::cache_root().join("26.3").join("generated").join("reports");
    let w = WorldData::load(&jar, Some(&reports), &v).expect("world data");
    let spec = GenSpec::Noise {
        settings: "minecraft:overworld".into(),
        source: SourceSpec::MultiNoise("minecraft:overworld".into()),
    };
    let dg = DimGen::new(&w, &spec, "minecraft:overworld", 1).expect("generator");
    let all: Vec<(i32, i32)> = (-4..=4).flat_map(|x| (-4..=4).map(move |z| (x, z))).collect();
    let mut cache = rc_worldgen::terrain::BiomeCache::default();
    dg.prepare_biomes(&mut cache, &all);
    let base: Vec<DecoChunk> = all.iter().map(|&(x, z)| dg.terrain(x, z, &cache, None)).collect();
    let (mut best_d, mut best_p) = (f64::MAX, f64::MAX);
    for pass in 0..5 {
        let mut cs = base.clone();
        let t = Instant::now();
        for cx in -3..=3 {
            for cz in -3..=3 {
                let mut picked: Vec<DecoChunk> = Vec::with_capacity(9);
                for i in 0..9 {
                    let p = (cx + i % 3 - 1, cz + i / 3 - 1);
                    let k = cs.iter().position(|c| (c.cx, c.cz) == p).unwrap();
                    picked.push(cs.swap_remove(k));
                }
                {
                    let mut it = picked.iter_mut();
                    let refs: [&mut DecoChunk; 9] = std::array::from_fn(|_| it.next().unwrap());
                    dg.decorate(refs);
                }
                cs.extend(picked);
            }
        }
        let d = t.elapsed().as_secs_f64();
        let (mut marks, mut fluid) = (0, 0);
        let snapshot = cs.clone();
        let outside = |q: [i32; 3]| {
            snapshot
                .iter()
                .find(|c| c.cx == q[0] >> 4 && c.cz == q[2] >> 4)
                .map_or(0, |c| c.blocks[c.index(q[0], q[1], q[2])])
        };
        let t = Instant::now();
        for c in cs.iter_mut().filter(|c| c.cx.abs() <= 2 && c.cz.abs() <= 2) {
            marks += c.post_process.len();
            fluid += c.fluid_ticks.len();
            rc_worldgen::decor_region::post_process_full(c, &dg.blocks, &outside);
        }
        let p = t.elapsed().as_secs_f64();
        best_d = best_d.min(d);
        best_p = best_p.min(p);
        eprintln!(
            "pass {pass}: decoration 49 chunks {:.1} ms, post-processing 25 chunks {:.3} ms ({marks} marks, {fluid} fluid ticks)",
            d * 1e3,
            p * 1e3
        );
    }
    eprintln!("best: decoration {:.1} ms, post-processing {:.3} ms", best_d * 1e3, best_p * 1e3);
}
