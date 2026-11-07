// Direct read allowed (tests and benches: settings read at launch, outside the measured path); the game goes through rc_data::env_*!.
#![allow(clippy::disallowed_methods)]
//! Equivalence proofs and bench of the generator setup shared per world data: density
//! functions and noises parsed once per pack, biome parameter lists and processor lists once per
//! world data, noise settings read once, spawn search scored on every core. Game data required
//! (jar 26.3 and rc-data cache); absent: tests skipped, except `RUSTCRAFT_OPT_REQUIRE=1`.

use std::fmt::Write as _;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::chunkgen::{DimGen, GenSpec, SourceSpec, WorldData};
use crate::generator::{Defs, Generator, NoiseSettings};
use crate::structure_place::ProcLists;
use crate::structure::tests::world_data;

/// Every switch of this file, forced together on the current thread.
const SHARING: [&str; 6] = [
    "SHARED_GENERATOR_DEFS",
    "SHARED_CLIMATE_LISTS",
    "SHARED_PROC_LISTS",
    "NOISE_SETTINGS_READ_ONCE",
    "NOISE_LOOKUP_BEFORE_CLONE",
    "PARALLEL_SPAWN_SEARCH",
];

const SEEDS: [i64; 3] = [0, 20_261_011, -4_172_144_997_902_289_642];

/// Digest of a debug rendering, without building the (large) text.
struct DigestWriter(std::collections::hash_map::DefaultHasher);

impl std::fmt::Write for DigestWriter {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        s.hash(&mut self.0);
        Ok(())
    }
}

fn digest(v: &impl std::fmt::Debug) -> u64 {
    let mut w = DigestWriter(std::collections::hash_map::DefaultHasher::new());
    write!(w, "{v:?}").expect("debug rendering");
    w.0.finish()
}

/// Compiled program of a generator: nodes, noise instances and constants, bit for bit (the debug
/// rendering of a float is its shortest exact form).
fn program_digest(g: &Generator) -> u64 {
    digest(&(&g.program, &g.router, &g.settings.min_y, &g.settings.height, &g.settings.sea_level))
}

fn noise_spec(preset: &str) -> GenSpec {
    noise_spec_with("minecraft:overworld", preset)
}

fn noise_spec_with(settings: &str, preset: &str) -> GenSpec {
    GenSpec::Noise { settings: settings.into(), source: SourceSpec::MultiNoise(preset.into()) }
}

/// Generator built with every sharing switch forced to `on`.
fn dim_gen(w: &WorldData, spec: &GenSpec, dimension: &str, seed: i64, on: bool, structures: bool) -> DimGen {
    crate::opt::force(&SHARING, Some(on));
    let mut dg = DimGen::new(w, spec, dimension, seed).expect("generator");
    if structures {
        dg.enable_structures(w).expect("structures");
    }
    crate::opt::force(&SHARING, None);
    dg
}

/// Shared density functions and noises (and the noise lookup before the copy) compile the same
/// program as definitions parsed again for each generator, for every noise setting of the jar and
/// several seeds; a second generator built from the same shared definitions too.
#[test]
fn shared_defs_compile_the_same_program() {
    let Some(w) = world_data() else { return };
    let pack = &w.pack;
    let shared = pack.generator_defs().expect("definitions");
    assert!(Arc::ptr_eq(&shared, &pack.generator_defs().unwrap()), "definitions parsed once per pack");
    let settings = pack.ids("noise_settings");
    assert!(settings.len() >= 6, "noise settings of the jar: {settings:?}");
    let mut compared = 0;
    for id in &settings {
        for seed in SEEDS {
            crate::opt::force(&["NOISE_LOOKUP_BEFORE_CLONE"], Some(false));
            let old = Generator::with_defs(Arc::new(Defs::load(pack).unwrap()), NoiseSettings::load(pack, id).unwrap(), seed)
                .unwrap();
            crate::opt::force(&["NOISE_LOOKUP_BEFORE_CLONE"], Some(true));
            let first = Generator::with_defs(shared.clone(), NoiseSettings::load(pack, id).unwrap(), seed).unwrap();
            let second = Generator::with_defs(shared.clone(), NoiseSettings::load(pack, id).unwrap(), seed).unwrap();
            crate::opt::force(&["NOISE_LOOKUP_BEFORE_CLONE"], None);
            let expected = program_digest(&old);
            assert_eq!(program_digest(&first), expected, "{id}, seed {seed}: shared definitions");
            assert_eq!(program_digest(&second), expected, "{id}, seed {seed}: second user of the definitions");
            compared += 1;
        }
    }
    // The digest sees the seed (it would not catch anything otherwise).
    let a = Generator::with_defs(shared.clone(), NoiseSettings::load(pack, &settings[0]).unwrap(), 1).unwrap();
    let b = Generator::with_defs(shared, NoiseSettings::load(pack, &settings[0]).unwrap(), 2).unwrap();
    assert_ne!(program_digest(&a), program_digest(&b), "digest blind to the seed");
    eprintln!("{compared} programs compared");
}

/// Processor lists of the shared table = those of a table built for the caller, list by list;
/// the table is built once per world data.
#[test]
fn shared_proc_lists_parse_the_same_processors() {
    let Some(w) = world_data() else { return };
    crate::opt::force(&["SHARED_PROC_LISTS"], Some(true));
    let shared = w.proc_lists();
    assert!(Arc::ptr_eq(&shared, &w.proc_lists()), "processor lists built once");
    crate::opt::force(&["SHARED_PROC_LISTS"], None);
    let fresh = ProcLists::new(&w.decor);
    let ids = w.decor.ids("processor_list");
    assert!(ids.len() > 20, "processor lists of the jar: {}", ids.len());
    for id in &ids {
        let key = crate::json::Json::Str(id.clone());
        let a = format!("{:?}", shared.get(&key, &w.blocks));
        let b = format!("{:?}", fresh.get(&key, &w.blocks));
        assert_eq!(a, b, "processor list {id}");
    }
}

/// Biome parameter list kept per world data = list parsed from the report, every preset.
#[test]
fn shared_climate_list_matches_the_report() {
    let Some(w) = world_data() else { return };
    for preset in ["minecraft:overworld", "minecraft:nether"] {
        let fresh = digest(&crate::biome::ClimateList::from_report(&w.pack, preset).unwrap());
        let a = dim_gen(w, &noise_spec(preset), "minecraft:overworld", 1, true, false);
        let b = dim_gen(w, &noise_spec(preset), "minecraft:overworld", 1, false, false);
        let list = |dg: &DimGen| match &dg.terrain_generator().unwrap().source {
            crate::biome::BiomeSource::MultiNoise(l) => digest(l),
            other => panic!("multi noise source expected: {other:?}"),
        };
        assert_eq!(list(&a), fresh, "{preset}: kept list");
        assert_eq!(list(&b), fresh, "{preset}: old path");
    }
}

/// Generators of the main world, the Nether, the End and a flat world with structures, built with
/// every sharing on and every sharing off: same program, same biome list, same spawn chunk, same
/// terrain and same structure starts around the origin.
#[test]
fn shared_setup_builds_the_same_generators() {
    let Some(w) = world_data() else { return };
    let nether = GenSpec::Noise {
        settings: "minecraft:nether".into(),
        source: SourceSpec::MultiNoise("minecraft:nether".into()),
    };
    let end = GenSpec::Noise { settings: "minecraft:end".into(), source: SourceSpec::TheEnd };
    let flat = GenSpec::Flat(crate::chunkgen::FlatSpec::classic());
    let cases = [
        (noise_spec("minecraft:overworld"), "minecraft:overworld"),
        (nether, "minecraft:the_nether"),
        (end, "minecraft:the_end"),
        (flat, "minecraft:overworld"),
    ];
    for (spec, dimension) in &cases {
        for seed in [20_261_011, -4_172_144_997_902_289_642] {
            let new = dim_gen(w, spec, dimension, seed, true, true);
            let old = dim_gen(w, spec, dimension, seed, false, true);
            let what = format!("{dimension} {spec:?} seed {seed}");
            if let (Some(a), Some(b)) = (new.structure_terrain(), old.structure_terrain()) {
                assert_eq!(program_digest(&a.g), program_digest(&b.g), "{what}: program");
            }
            assert_eq!(new.biome_names, old.biome_names, "{what}: biomes");
            assert_eq!(new.spawn_chunk(), old.spawn_chunk(), "{what}: spawn chunk");
            let mut cache = crate::terrain::BiomeCache::default();
            let chunks = [(0, 0), (3, -2), (-7, 5)];
            new.prepare_biomes(&mut cache, &chunks);
            for &(cx, cz) in &chunks {
                let (a, b) = (new.terrain(cx, cz, &cache, None), old.terrain(cx, cz, &cache, None));
                assert!(a.blocks == b.blocks && a.biomes == b.biomes, "{what}: terrain of ({cx}, {cz})");
            }
            for cx in -8..8 {
                for cz in -8..8 {
                    assert_eq!(
                        digest(&new.structures_nbt(cx, cz)),
                        digest(&old.structures_nbt(cx, cz)),
                        "{what}: starts of ({cx}, {cz})"
                    );
                }
            }
        }
    }
}

/// The spawn search as written before the change (one loop that scores each point as it walks),
/// kept as the reference.
fn spawn_chunk_reference(dg: &DimGen) -> (i32, i32) {
    use super::{Kind, param_distance};
    let Kind::Noise(t) = &dg.kind else { return (0, 0) };
    if dg.spawn_targets.is_empty() {
        return (0, 0);
    }
    let fitness = |x: i32, z: i32| -> i64 {
        let (qx, qz) = ((x >> 2) << 2, (z >> 2) << 2);
        let mut ctx = crate::sampler::Ctx::uncached();
        let mut best = i64::MAX;
        for point in &dg.spawn_targets {
            let mut f = 0i64;
            for &(sid, range) in point {
                let v = crate::biome::quantize(t.g.value(sid, &mut ctx, qx, 0, qz));
                let d = param_distance(range, v);
                f = f.wrapping_add(d.wrapping_mul(d));
            }
            best = best.min(f);
        }
        let bias = i64::from(x) * i64::from(x) + i64::from(z) * i64::from(z);
        best.wrapping_mul(2048 * 2048).wrapping_add(bias)
    };
    let mut best = ((0, 0), fitness(0, 0));
    for (max_r, step) in [(2048.0f32, 512.0f32), (512.0, 32.0)] {
        let origin = best.0;
        let mut angle = 0.0f32;
        let mut radius = step;
        while radius <= max_r {
            let x = origin.0 + (crate::jmath::sin(f64::from(angle)) * f64::from(radius)) as i32;
            let z = origin.1 + (crate::jmath::cos(f64::from(angle)) * f64::from(radius)) as i32;
            let f = fitness(x, z);
            if f < best.1 {
                best = ((x, z), f);
            }
            angle += step / radius;
            if f64::from(angle) > std::f64::consts::PI * 2.0 {
                angle = 0.0;
                radius += step;
            }
        }
    }
    (best.0.0 >> 4, best.0.1 >> 4)
}

/// Spawn chunks of the main world presets, scored on every core = one point at a time = the search
/// as written before the change = stored values.
#[test]
fn parallel_spawn_search_matches_sequential() {
    let Some(w) = world_data() else { return };
    // Noise settings of the main world presets (normal, large biomes, amplified), seed, spawn chunk.
    let expected: [(&str, i64, (i32, i32)); 4] = [
        ("minecraft:overworld", 20_261_011, (-4, -12)),
        ("minecraft:overworld", -4_172_144_997_902_289_642, (-28, -27)),
        ("minecraft:large_biomes", 1, (57, -78)),
        ("minecraft:amplified", 20_261_011, (-4, -12)),
    ];
    let mut got = Vec::new();
    for &(settings, seed, _) in &expected {
        let spec = noise_spec_with(settings, "minecraft:overworld");
        let dg = dim_gen(w, &spec, "minecraft:overworld", seed, false, false);
        crate::opt::force(&["PARALLEL_SPAWN_SEARCH"], Some(false));
        let sequential = dg.spawn_chunk();
        crate::opt::force(&["PARALLEL_SPAWN_SEARCH"], Some(true));
        let parallel = dg.spawn_chunk();
        crate::opt::force(&["PARALLEL_SPAWN_SEARCH"], None);
        assert_eq!(parallel, sequential, "{settings} seed {seed}");
        assert_eq!(spawn_chunk_reference(&dg), sequential, "{settings} seed {seed}: reference search");
        eprintln!("        (\"{settings}\", {seed}, {sequential:?}),");
        got.push((settings, seed, sequential));
    }
    assert_eq!(got.as_slice(), expected.as_slice(), "spawn chunks differ from the stored values");
}

/// Noise settings parsed while the read-once switch is off still keep their spawn targets: a
/// generator whose settings were loaded under one switch value and whose targets are read under
/// the other must not end up with an empty target list (silent spawn at chunk 0, 0).
#[test]
fn spawn_targets_kept_whatever_the_switch() {
    let Some(w) = world_data() else { return };
    let spec = noise_spec("minecraft:overworld");
    let dg = dim_gen(w, &spec, "minecraft:overworld", 20_261_011, true, false);
    for on in [false, true] {
        crate::opt::force(&["NOISE_SETTINGS_READ_ONCE"], Some(on));
        let settings = NoiseSettings::load(&w.pack, "minecraft:overworld").expect("noise settings");
        crate::opt::force(&["NOISE_SETTINGS_READ_ONCE"], None);
        assert!(settings.spawn_target.is_some(), "switch {on}: spawn targets dropped by the settings");
    }
    assert!(!dg.spawn_targets.is_empty(), "spawn targets of the main world");
    assert_ne!(dg.spawn_chunk(), (0, 0), "spawn search ran on real targets");
}

fn ms(t: std::time::Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1e3
}

/// Minimum and median of a few repetitions, in milliseconds.
fn time(reps: usize, mut f: impl FnMut()) -> (f64, f64) {
    let mut v: Vec<f64> = (0..reps)
        .map(|_| {
            let t = std::time::Instant::now();
            f();
            ms(t)
        })
        .collect();
    v.sort_by(f64::total_cmp);
    (v[0], v[v.len() / 2])
}

/// Setup of a main world generator, step by step, then whole with every sharing off and on
/// (min / median of 5, one thread then all threads for the parallel steps):
/// `cargo test --release -p rc-worldgen --lib generator_sharing_tests::bench_generator_setup -- --ignored --nocapture`.
#[test]
#[ignore = "benchmark"]
fn bench_generator_setup() {
    let Some(w) = world_data() else { return };
    let pack = &w.pack;
    let spec = noise_spec("minecraft:overworld");
    let line = |what: &str, (min, med): (f64, f64)| eprintln!("{what:<48} {min:>8.1} / {med:>8.1} ms");
    line("density functions and noises (Defs::load)", time(5, || drop(Defs::load(pack).unwrap())));
    line("noise settings file", time(5, || drop(NoiseSettings::load(pack, "minecraft:overworld").unwrap())));
    line(
        "biome parameter list (report)",
        time(5, || drop(crate::biome::ClimateList::from_report(pack, "minecraft:overworld").unwrap())),
    );
    let list = crate::biome::ClimateList::from_report(pack, "minecraft:overworld").unwrap();
    line("biome parameter list (copy)", time(5, || drop(list.clone())));
    line("processor lists", time(5, || drop(ProcLists::new(&w.decor))));
    let names = dim_gen(w, &spec, "minecraft:overworld", 1, false, false).biome_names.clone();
    line(
        "decorator",
        time(5, || drop(crate::decor::Decorator::new(&w.decor, &w.blocks, &names, 1).unwrap())),
    );
    for on in [false, true] {
        let label = if on { "sharing on " } else { "sharing off" };
        line(&format!("{label}: generator"), time(5, || drop(dim_gen(w, &spec, "minecraft:overworld", 1, on, false))));
        line(
            &format!("{label}: generator with structures"),
            time(5, || drop(dim_gen(w, &spec, "minecraft:overworld", 1, on, true))),
        );
    }
    let max = std::thread::available_parallelism().map_or(1, |n| n.get());
    for threads in [1, max] {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
        let dg = dim_gen(w, &spec, "minecraft:overworld", 1, true, true);
        pool.install(|| {
            for parallel in [false, true] {
                crate::opt::force(&["PARALLEL_SPAWN_SEARCH"], Some(parallel));
                line(&format!("{threads:>2} threads: spawn search, parallel {parallel}"), time(5, || {
                    std::hint::black_box(dg.spawn_chunk());
                }));
            }
            crate::opt::force(&["PARALLEL_SPAWN_SEARCH"], None);
            let t = std::time::Instant::now();
            dg.warm_rings();
            eprintln!("{threads:>2} threads: stronghold rings (first time)          {:>8.1} ms", ms(t));
        });
    }
}
