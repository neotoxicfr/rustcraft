//! Equivalence proofs of the density functions and noises parsed on every core
//! (`RUSTCRAFT_OPT_PARALLEL_DEFS_LOAD`) and of the pack's parse cache
//! (`RUSTCRAFT_OPT_JSON_PARSE_CACHE`). Game data required for the jar proof (jar 26.3 and rc-data
//! cache); absent: skipped, except `RUSTCRAFT_OPT_REQUIRE=1`.

use std::sync::Arc;

use crate::data::DataPack;
use crate::generator::{Defs, Generator, NoiseSettings};
use crate::json::Json;

/// Every entry of the definitions, rendered and sorted (the maps' own order is not observable).
fn sorted_entries(d: &Defs) -> Vec<String> {
    let mut v: Vec<String> = d.functions.iter().map(|(k, f)| format!("function {k} {f:?}")).collect();
    v.extend(d.noises.iter().map(|(k, n)| format!("noise {k} {n:?}")));
    v.sort();
    v
}

/// Compiled program of a generator: nodes, noise instances and constants, bit for bit (the debug
/// rendering of a float is its shortest exact form).
fn program_rendering(g: &Generator) -> String {
    format!("{:?}", (&g.program, &g.router))
}

/// Definitions parsed on every core = parsed one file after the other: the same entries, and the
/// same program compiled from them for every noise setting of the jar.
#[test]
fn parallel_defs_load_matches_the_series_one() {
    let Some(w) = crate::structure::tests::world_data() else { return };
    let pack = &w.pack;
    let series = Arc::new(Defs::load_mode(pack, false).expect("definitions in series"));
    let parallel = Arc::new(Defs::load_mode(pack, true).expect("definitions in parallel"));
    assert!(series.functions.len() > 30 && series.noises.len() > 30, "definitions of the jar");
    assert_eq!(sorted_entries(&parallel), sorted_entries(&series));
    let settings = pack.ids("noise_settings");
    assert!(settings.len() >= 6, "noise settings of the jar: {settings:?}");
    for id in &settings {
        let a = Generator::with_defs(series.clone(), NoiseSettings::load(pack, id).unwrap(), 20_261_011).unwrap();
        let b = Generator::with_defs(parallel.clone(), NoiseSettings::load(pack, id).unwrap(), 20_261_011).unwrap();
        assert_eq!(program_rendering(&b), program_rendering(&a), "{id}");
    }
}

/// Two broken files: both paths report the first one in identifier order.
#[test]
fn parallel_defs_load_reports_the_same_error() {
    let mut pack = DataPack::default();
    for name in ["d", "b", "c", "a"] {
        pack.insert(&format!("density_function/minecraft:{name}"), "1.5".into());
    }
    pack.insert("density_function/minecraft:c", "{".into());
    pack.insert("density_function/minecraft:b", "[1,".into());
    let series = Defs::load_mode(&pack, false).expect_err("broken pack").to_string();
    assert!(series.contains("minecraft:b"), "{series}");
    for _ in 0..20 {
        assert_eq!(Defs::load_mode(&pack, true).expect_err("broken pack").to_string(), series);
    }
}

/// The two trees hold the same block of members: the second one is a copy of the first.
fn same_tree(a: &Json, b: &Json) -> bool {
    match (a, b) {
        (Json::Obj(a), Json::Obj(b)) => a.shares_members_with(b),
        _ => panic!("objects expected: {a:?}, {b:?}"),
    }
}

/// A cached file is parsed once and its tree shared, equal to a fresh parse; a replaced file is
/// parsed again; the kinds a pack turns into shared structures are never kept; errors are the
/// same with or without the cache.
#[test]
fn parse_cache_shares_one_tree_per_file() {
    let mut pack = DataPack::default();
    pack.insert("biome/minecraft:plains", r#"{"temperature": 0.8, "downfall": 0.4}"#.into());
    pack.insert("noise/minecraft:offset", r#"{"firstOctave": -3, "amplitudes": [1.0]}"#.into());
    pack.insert("biome/minecraft:broken", r#"{"temperature": }"#.into());
    let first = pack.json_mode("biome", "plains", true).unwrap();
    assert!(same_tree(&first, &pack.json_mode("biome", "minecraft:plains", true).unwrap()), "tree shared");
    let fresh = pack.json_mode("biome", "plains", false).unwrap();
    assert!(!same_tree(&first, &fresh), "cache off: parsed again");
    assert_eq!(first, fresh);
    let noise = pack.json_mode("noise", "offset", true).unwrap();
    assert!(!same_tree(&noise, &pack.json_mode("noise", "offset", true).unwrap()), "noise trees not kept");
    pack.insert("biome/minecraft:plains", r#"{"temperature": 0.9}"#.into());
    assert_eq!(pack.json_mode("biome", "plains", true).unwrap().get("temperature").and_then(Json::as_f32), Some(0.9));
    for cache in [true, false] {
        let broken = pack.json_mode("biome", "broken", cache).expect_err("broken file").to_string();
        assert!(broken.contains("biome/broken : invalid JSON at byte 16"), "{broken}");
        assert!(pack.json_mode("biome", "absent", cache).is_err());
    }
}
