// Direct read allowed (test: settings read at launch, outside the hot path); the game goes through rc_data::env_*!.
#![allow(clippy::disallowed_methods)]
//! Blocks of a chunk at the end of the `terrain` step (filling, surface, carved caves) compared
//! block for block with the Java probe (`terrain_*` in the references folder), with the generation height
//! maps and the fluid post-processing marks.
//! `RUSTCRAFT_TERRAIN_FILTER=<substring>` restricts the list (e.g. `nether`, `overworld 0 12 3`).

mod common;

use hashbrown::HashMap;
use std::collections::BTreeSet;

use rc_worldgen::proto::ProtoTerrain;
use rc_worldgen::terrain::{BiomeCache, Stage, TerrainGenerator};

fn preset(dim: &str) -> (&'static str, &'static str) {
    match dim {
        "overworld" => ("minecraft:normal", "minecraft:overworld"),
        "large_biomes" => ("minecraft:large_biomes", "minecraft:overworld"),
        "amplified" => ("minecraft:amplified", "minecraft:overworld"),
        "nether" => ("minecraft:normal", "minecraft:the_nether"),
        "end" => ("minecraft:normal", "minecraft:the_end"),
        other => panic!("dimension {other}"),
    }
}

struct Reference {
    names: Vec<String>,
    blocks: Vec<u16>,
    ws: Vec<i32>,
    of: Vec<i32>,
    pp: BTreeSet<u32>,
}

fn load(base: &str) -> Reference {
    let dir = common::ref_dir().expect("references");
    let names: Vec<String> =
        std::fs::read_to_string(dir.join(format!("{base}.pal"))).expect("pal").lines().map(str::to_owned).collect();
    let bin = std::fs::read(dir.join(format!("{base}.bin"))).expect("bin");
    let n = bin.len() - 256 * 8;
    let blocks = bin[..n].as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).collect();
    let ints: Vec<i32> = bin[n..].as_chunks::<4>().0.iter().map(|c| i32::from_le_bytes(*c)).collect();
    let pp = std::fs::read_to_string(dir.join(format!("{base}.pp")))
        .expect("pp")
        .lines()
        .filter(|l| !l.is_empty())
        .map(|l| l.parse().expect("pp"))
        .collect();
    Reference { names, blocks, ws: ints[..256].to_vec(), of: ints[256..].to_vec(), pp }
}

/// Differences (blocks, surface map, floor map, marks); prints the first ones.
fn compare(label: &str, t: &TerrainGenerator, c: &ProtoTerrain, r: &Reference) -> usize {
    let mut bad = 0usize;
    let mut kinds: HashMap<(String, String), usize> = HashMap::new();
    for (i, (&a, &b)) in c.blocks.iter().zip(&r.blocks).enumerate() {
        let (ra, rb) = (t.blocks.name(a), r.names[b as usize].as_str());
        if ra != rb {
            if bad < 5 {
                let (x, z, y) = (i & 15, (i >> 4) & 15, (i >> 8) as i32 + c.min_y);
                eprintln!("{label}: block ({x}, {y}, {z}) rust {ra} java {rb}");
            }
            *kinds.entry((ra.to_owned(), rb.to_owned())).or_default() += 1;
            bad += 1;
        }
    }
    if bad > 0 {
        let mut k: Vec<_> = kinds.into_iter().collect();
        k.sort_by_key(|e| std::cmp::Reverse(e.1));
        eprintln!("{label}: {bad} different blocks; {:?}", &k[..k.len().min(6)]);
    }
    if std::env::var_os("RUSTCRAFT_TERRAIN_DEBUG").is_some() {
        eprintln!("rust of {:?}\njava of {:?}", &c.ocean_floor_wg[..8], &r.of[..8]);
    }
    let ws = c.world_surface_wg.iter().zip(&r.ws).filter(|(a, b)| a != b).count();
    let of = c.ocean_floor_wg.iter().zip(&r.of).filter(|(a, b)| a != b).count();
    let pp: BTreeSet<u32> = c.post_processing.iter().copied().collect();
    let ppd = pp.symmetric_difference(&r.pp).count();
    if ws + of + ppd > 0 {
        eprintln!("{label}: maps surface {ws}, floor {of}; marks {ppd} (rust {}, java {})", pp.len(), r.pp.len());
    }
    bad + ws + of + ppd
}

#[test]
fn terrain_matches_game() {
    let Some(lines) = common::read_lines("terrain_list.txt") else { return };
    let Some(pack) = common::data_pack() else { return };
    let filter = std::env::var("RUSTCRAFT_TERRAIN_FILTER").ok();
    let mut gens: HashMap<String, TerrainGenerator> = HashMap::new();
    let mut total = 0usize;
    let mut n = 0;
    for l in lines.iter().filter(|l| filter.as_ref().is_none_or(|f| l.contains(f.as_str()))) {
        let p: Vec<&str> = l.split(' ').collect();
        let (dim, seed, cx, cz) =
            (p[0], p[1].parse::<i64>().unwrap(), p[2].parse::<i32>().unwrap(), p[3].parse::<i32>().unwrap());
        let t = gens.entry(format!("{dim} {seed}")).or_insert_with(|| {
            let (pr, d) = preset(dim);
            TerrainGenerator::from_preset(&pack, pr, d, seed).expect("generator")
        });
        let mut cache = BiomeCache::default();
        t.prepare_biomes(&mut cache, &[(cx, cz)]);
        let base = format!("terrain_{dim}_{seed}_{cx}_{cz}");
        let mut errs = 0;
        for (stage, suffix) in [(Stage::Fill, "fill"), (Stage::Surface, "surface"), (Stage::Carvers, "carvers")] {
            let c = t.generate_until(cx, cz, &cache, None, stage);
            let e = compare(&format!("{l} {suffix}"), t, &c, &load(&format!("{base}_{suffix}")));
            errs += e;
            if e > 0 {
                break;
            }
        }
        total += errs;
        n += 1;
    }
    eprintln!("{n} chunks, {total} differences");
    assert_eq!(total, 0, "differences from the game");
}
