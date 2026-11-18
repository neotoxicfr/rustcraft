//! Named density functions and final density of chunks, compared with the game at 0 ulp.

mod common;

use hashbrown::HashMap;
use std::sync::Arc;

use rc_worldgen::generator::{Defs, NoiseSettings};
use rc_worldgen::{Ctx, Generator, Volume};

const POINTS: [[i32; 3]; 12] = [
    [0, 0, 0],
    [1, 2, 3],
    [-17, 64, -33],
    [123, -50, -456],
    [1000, 100, -1000],
    [-2_097_153, 70, 2_097_155],
    [30_000_000, -64, -29_999_999],
    [7, 319, -9],
    [-1, -1, -1],
    [4, 8, 4],
    [-5, 12, 9],
    [16, 40, -16],
];

const SEEDS: [i64; 3] = [0, -6_214_586_474_519_478_921, 1_234_567_890_123_456_789];

fn generator(defs: &Arc<Defs>, preset: &str, seed: i64) -> Generator {
    let pack = common::data_pack().unwrap();
    Generator::with_defs(defs.clone(), NoiseSettings::load(&pack, preset).unwrap(), seed).unwrap()
}

#[test]
fn named_functions_match_game() {
    let Some(pack) = common::data_pack() else { return };
    let defs = Arc::new(Defs::load(&pack).unwrap());
    let mut failures = Vec::new();
    let mut checked = 0;
    for seed in SEEDS {
        let Some(lines) = common::read_lines(&format!("df_{seed}.txt")) else { return };
        let mut gens: HashMap<String, Generator> = HashMap::new();
        for l in &lines {
            let t: Vec<&str> = l.split(' ').collect();
            let (id, preset) = (t[0], t[1]);
            let g = gens.entry(preset.to_owned()).or_insert_with(|| generator(&defs, preset, seed));
            let (range, axes) = g.declared(id).unwrap();
            if range.min.to_bits() != common::hex_f32(t[3]).to_bits()
                || range.max.to_bits() != common::hex_f32(t[4]).to_bits()
            {
                failures.push(format!(
                    "{id} range {range:?} ≠ [{} ; {}]",
                    common::hex_f32(t[3]),
                    common::hex_f32(t[4])
                ));
            }
            if axes.to_string() != t[6] {
                failures.push(format!("{id} axes {axes} ≠ {}", t[6]));
            }
            let s = g.compile_named(id).unwrap();
            let mut k = 8;
            for p in POINTS {
                let v = g.value(s, &mut Ctx::uncached(), p[0], p[1], p[2]);
                let r = common::hex_f32(t[k]);
                if v.to_bits() != r.to_bits() {
                    failures.push(format!("{id} seed {seed} point {p:?}: {v} ≠ {r}"));
                }
                k += 1;
            }
            for vol in [Volume::new(4, 9, 4, -6, -3, 5), Volume::stepped(3, 4, 3, 8, -64, -12, 4, 8, 4)] {
                assert_eq!(t[k], "vol");
                k += 1;
                let out = g.volume(s, &mut Ctx::uncached(), &vol);
                let bad =
                    out.iter().enumerate().filter(|(i, v)| v.to_bits() != common::hex_f32(t[k + i]).to_bits()).count();
                if bad > 0 {
                    let i = out
                        .iter()
                        .enumerate()
                        .position(|(i, v)| v.to_bits() != common::hex_f32(t[k + i]).to_bits())
                        .unwrap();
                    failures.push(format!(
                        "{id} seed {seed} volume {vol:?}: {bad} differences, first {i}: {} ≠ {}",
                        out[i],
                        common::hex_f32(t[k + i])
                    ));
                }
                k += vol.size();
            }
            checked += 1;
        }
    }
    assert!(failures.is_empty(), "{} differences:\n{}", failures.len(), failures.join("\n"));
    assert!(checked > 100, "{checked} functions checked");
}

#[test]
fn final_density_matches_game() {
    let Some(pack) = common::data_pack() else { return };
    if common::ref_dir().is_none() {
        return;
    }
    let defs = Arc::new(Defs::load(&pack).unwrap());
    type Case = (&'static str, i64, Vec<(i32, i32)>);
    let mut cases: Vec<Case> = SEEDS
        .iter()
        .map(|s| ("overworld", *s, vec![(0, 0), (-1, -1), (5, -7), (200_000, -150_000), (-37, 12)]))
        .collect();
    cases.push(("large_biomes", 0, vec![(0, 0), (-3, 2)]));
    cases.push(("amplified", 0, vec![(0, 0), (-3, 2)]));
    cases.push(("nether", 0, vec![(0, 0), (-3, 2)]));
    cases.push(("end", 0, vec![(0, 0), (80, -60)]));
    let mut failures = Vec::new();
    let (mut compared, mut solid) = (0usize, 0usize);
    for (preset, seed, chunks) in cases {
        let g = generator(&defs, preset, seed);
        for (cx, cz) in chunks {
            let r = common::read_f32_bin(&format!("final_{preset}_{seed}_{cx}_{cz}.bin")).unwrap();
            let out = g.final_density(cx, cz);
            assert_eq!(out.len(), r.len());
            compared += out.len();
            solid += out.iter().filter(|v| **v > 0.0).count();
            let bad: Vec<usize> = (0..out.len()).filter(|&i| out[i].to_bits() != r[i].to_bits()).collect();
            if !bad.is_empty() {
                failures.push(format!(
                    "{preset} seed {seed} chunk {cx},{cz}: {} differences, first {}: {} ≠ {}",
                    bad.len(),
                    bad[0],
                    out[bad[0]],
                    r[bad[0]]
                ));
            }
        }
    }
    eprintln!("{compared} densities compared, {solid} solid");
    assert!(solid > compared / 10 && solid < compared, "degenerate buffers");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// Isolated column (base height, noise column): 1 × height × 1 volume.
#[test]
fn isolated_columns_match_game_and_chunk() {
    let Some(pack) = common::data_pack() else { return };
    if common::ref_dir().is_none() {
        return;
    }
    let defs = Arc::new(Defs::load(&pack).unwrap());
    let cols = [(0, 0), (5, -3), (-17, 33), (1001, -999), (-3_000_001, 2_999_999)];
    for seed in SEEDS {
        let g = generator(&defs, "overworld", seed);
        let r = common::read_f32_bin(&format!("columns_{seed}.bin")).unwrap();
        for (n, (x, z)) in cols.iter().enumerate() {
            let v = Volume::new(1, 384, 1, *x, -64, *z);
            let out = g.volume(g.router.final_density, &mut Ctx::cached(), &v);
            let want = &r[n * 384..(n + 1) * 384];
            let bad = (0..384).filter(|&i| out[i].to_bits() != want[i].to_bits()).count();
            assert_eq!(bad, 0, "seed {seed} column {x},{z}");
            // Same result as the corresponding column of the chunk.
            let chunk = g.final_density(x >> 4, z >> 4);
            let cv = g.chunk_volume(x >> 4, z >> 4);
            let same = (0..384).all(|y| chunk[cv.index(x & 15, y, z & 15)].to_bits() == out[y as usize].to_bits());
            assert!(same, "seed {seed} column {x},{z} ≠ chunk");
        }
    }
}
