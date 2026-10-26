// Direct read allowed (test: settings read at launch, outside the hot path); the game goes through rc_data::env_*!.
#![allow(clippy::disallowed_methods)]
//! Features placed outside the decoration probes: End return gateway (exit noted),
//! void platform, filled layer of a flat world. Game data required (jar 26.3 and
//! rc-data cache); absent: tests skipped, failure under `RUSTCRAFT_OPT_REQUIRE=1`.

mod common;

use rc_proto::nbt::Nbt;
use rc_worldgen::chunkgen::{DimGen, GenSpec, SourceSpec, WorldData};
use rc_worldgen::decor_region::{BeData, DecoChunk, Region};

fn world_data() -> Option<WorldData> {
    let jar = common::client_jar()?;
    let v = rc_testdata::version_data()?;
    let reports = rc_data::cache_root().join("26.3").join("generated").join("reports");
    rc_testdata::need(WorldData::load(&jar, Some(&reports), &v).ok(), "generation data 26.3")
}

/// The return gateway (`end_gateway_return`) notes its exact exit (100, 50, 0) in its
/// block entity; the "deferred" gateway notes none.
#[test]
fn return_gateway_records_exact_exit() {
    let Some(w) = world_data() else { return };
    let spec = GenSpec::Noise { settings: "minecraft:end".into(), source: SourceSpec::TheEnd };
    let dg = DimGen::new(&w, &spec, "minecraft:the_end", 1).unwrap();
    let deco = dg.deco.clone().unwrap();
    let mut chunks: Vec<DecoChunk> =
        (0..9).map(|k| DecoChunk::new(100 + k % 3 - 1, 100 + k / 3 - 1, dg.min_y, dg.height)).collect();
    let o = [1608, 60, 1608];
    {
        let mut it = chunks.iter_mut();
        let refs: [&mut DecoChunk; 9] = std::array::from_fn(|_| it.next().unwrap());
        let mut r = Region::new(refs, &dg.blocks, dg.sea_level, dg.seed);
        let mut cx = rc_worldgen::decor::Ctx::new(&mut r, &deco);
        let mut rng = rc_worldgen::decor_rng::DecoRng::xoroshiro(0);
        let f = deco.reg.feature_id("minecraft:end_gateway_return").expect("feature");
        assert!(rc_worldgen::feature::place_feature(&mut cx, f, &mut rng, o));
    }
    let c = &chunks[4];
    let gw = dg.blocks.block_id("end_gateway").unwrap();
    assert!(dg.blocks.is(c.blocks[c.index(o[0], o[1], o[2])], gw));
    let raw = c.be_data.iter().find(|(p, _)| *p == o).map(|(_, d)| d);
    let Some(BeData::Raw(n)) = raw else { panic!("exit block entity missing: {raw:?}") };
    assert_eq!(n.get("exit_portal"), Some(&Nbt::IntArray(vec![100, 50, 0])));
    assert_eq!(n.get("ExactTeleport"), Some(&Nbt::Byte(1)));
    eprintln!("return gateway: exit noted {n:?}");
}

/// "The void" preset: 33×33 stone platform, cobblestone at (8, -61, 8), only in the
/// chunks at a distance of 1 at most from chunk (0, 0).
#[test]
fn void_platform_around_origin() {
    let Some(w) = world_data() else { return };
    let spec = GenSpec::Flat(rc_worldgen::chunkgen::FlatSpec {
        layers: vec![("minecraft:air".into(), 1)],
        biome: "minecraft:the_void".into(),
        features: true,
        lakes: false,
        structure_overrides: Some(Vec::new()),
    });
    let dg = DimGen::new(&w, &spec, "minecraft:overworld", 0).unwrap();
    let b = &dg.blocks;
    let (stone, cobble) = (b.block_id("stone").unwrap(), b.block_id("cobblestone").unwrap());
    let cache = rc_worldgen::terrain::BiomeCache::default();
    for (ccx, ccz, expect) in [(0, 0, true), (-1, 1, true), (2, 0, false)] {
        let mut chunks: Vec<DecoChunk> =
            (0..9).map(|k| dg.terrain(ccx + k % 3 - 1, ccz + k / 3 - 1, &cache, None)).collect();
        {
            let mut it = chunks.iter_mut();
            let refs: [&mut DecoChunk; 9] = std::array::from_fn(|_| it.next().unwrap());
            dg.decorate(refs);
        }
        let c = &chunks[4];
        let n = c.blocks.iter().filter(|&&s| b.is(s, stone) || b.is(s, cobble)).count();
        if !expect {
            assert_eq!(n, 0, "chunk ({ccx}, {ccz})");
            continue;
        }
        // Cells of the chunk within 16 of (8, 8), all at y = -61.
        let want = (ccx * 16..ccx * 16 + 16)
            .flat_map(|x| (ccz * 16..ccz * 16 + 16).map(move |z| (x, z)))
            .filter(|(x, z)| (x - 8).abs().max((z - 8).abs()) <= 16)
            .count();
        assert_eq!(n, want, "chunk ({ccx}, {ccz})");
        for x in ccx * 16..ccx * 16 + 16 {
            for z in ccz * 16..ccz * 16 + 16 {
                if (x - 8).abs().max((z - 8).abs()) <= 16 {
                    let s = c.blocks[c.index(x, -61, z)];
                    assert!(b.is(s, if (x, z) == (8, 8) { cobble } else { stone }), "({x}, {z})");
                }
            }
        }
    }
}

/// Flat world whose top layer does not block movement (torch): absent from the filling,
/// placed as the last step over the whole layer left empty.
#[test]
fn flat_non_blocking_layer_filled_last() {
    let Some(w) = world_data() else { return };
    let spec = GenSpec::Flat(rc_worldgen::chunkgen::FlatSpec {
        layers: vec![("minecraft:bedrock".into(), 1), ("minecraft:stone".into(), 2), ("minecraft:torch".into(), 1)],
        biome: "minecraft:plains".into(),
        features: false,
        lakes: false,
        structure_overrides: Some(Vec::new()),
    });
    let dg = DimGen::new(&w, &spec, "minecraft:overworld", 0).unwrap();
    let b = &dg.blocks;
    let torch = b.block_id("torch").unwrap();
    let cache = rc_worldgen::terrain::BiomeCache::default();
    let mut chunks: Vec<DecoChunk> = (0..9).map(|k| dg.terrain(k % 3 - 1, k / 3 - 1, &cache, None)).collect();
    let c = &chunks[4];
    assert!(c.blocks[c.index(3, -61, 3)] == b.default_state(b.k.air), "torch absent from the filling");
    assert_eq!(dg.spawn_height(), -60);
    {
        let mut it = chunks.iter_mut();
        let refs: [&mut DecoChunk; 9] = std::array::from_fn(|_| it.next().unwrap());
        dg.decorate(refs);
    }
    let c = &chunks[4];
    for x in 0..16 {
        for z in 0..16 {
            assert!(b.is(c.blocks[c.index(x, -61, z)], torch), "({x}, {z})");
        }
    }
}

/// Generated village: its mobs (sheep, cows, cats, villagers…) leave marked for the
/// server's spawn finalisation; what is not a mob (armor stand) stays raw.
#[test]
fn village_mobs_marked_for_spawn_finalization() {
    let Some(w) = world_data() else { return };
    let spec = GenSpec::Noise {
        settings: "minecraft:overworld".into(),
        source: SourceSpec::MultiNoise("minecraft:overworld".into()),
    };
    let mut dg = DimGen::new(&w, &spec, "minecraft:overworld", 12345).unwrap();
    dg.enable_structures(&w).unwrap();
    let sg = dg.structures.clone().unwrap();
    let mut found = None;
    'f: for r in 0..160i32 {
        for x in -r..=r {
            for z in -r..=r {
                if x.abs() != r && z.abs() != r {
                    continue;
                }
                let st = dg.structure_starts(x, z);
                if let Some(s) = st.iter().find(|s| sg.defs[s.structure].id.starts_with("minecraft:village_")) {
                    found = Some(s.clone());
                    break 'f;
                }
            }
        }
    }
    let s = found.expect("village within a radius of 160 chunks");
    let (x0, z0, x1, z1) = (s.bbox.min[0] >> 4, s.bbox.min[2] >> 4, s.bbox.max[0] >> 4, s.bbox.max[2] >> 4);
    let mut cache = rc_worldgen::terrain::BiomeCache::default();
    let all: Vec<(i32, i32)> = (x0 - 1..=x1 + 1).flat_map(|x| (z0 - 1..=z1 + 1).map(move |z| (x, z))).collect();
    dg.prepare_biomes(&mut cache, &all);
    let mut chunks: Vec<DecoChunk> = all.iter().map(|&(x, z)| dg.terrain(x, z, &cache, None)).collect();
    for cx in x0..=x1 {
        for cz in z0..=z1 {
            let mut picked: Vec<DecoChunk> = Vec::new();
            for i in 0..9 {
                let p = (cx + i % 3 - 1, cz + i / 3 - 1);
                let k = chunks.iter().position(|c| (c.cx, c.cz) == p).unwrap();
                picked.push(chunks.swap_remove(k));
            }
            {
                let mut it = picked.iter_mut();
                let refs: [&mut DecoChunk; 9] = std::array::from_fn(|_| it.next().unwrap());
                dg.decorate(refs);
            }
            chunks.extend(picked);
        }
    }
    let (mut marked, mut plain_mobs, mut others_marked) = (0, 0, 0);
    let mut ids = std::collections::BTreeMap::new();
    for e in chunks.iter().flat_map(|c| c.entities.iter()) {
        let id = e.get("id").and_then(Nbt::as_str).unwrap_or("?").to_owned();
        let m = e.get("rc_finalize_structure").is_some();
        *ids.entry((id.clone(), m)).or_insert(0) += 1;
        let mob = [
            "minecraft:sheep",
            "minecraft:cow",
            "minecraft:pig",
            "minecraft:cat",
            "minecraft:horse",
            "minecraft:camel",
            "minecraft:villager",
            "minecraft:zombie_villager",
            "minecraft:iron_golem",
        ]
        .contains(&id.as_str());
        match (mob, m) {
            (true, true) => marked += 1,
            (true, false) => plain_mobs += 1,
            (false, true) => others_marked += 1,
            (false, false) => {}
        }
    }
    eprintln!("{}: entities {ids:?}", sg.defs[s.structure].id);
    assert!(marked > 0, "no village mob marked");
    assert_eq!((plain_mobs, others_marked), (0, 0));
}
