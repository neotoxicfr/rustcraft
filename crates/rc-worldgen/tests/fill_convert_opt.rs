//! Equivalence of the column filling (`RUSTCRAFT_OPT_FILL_COLUMN_TRACKING`) and of the conversion
//! of terrain proto-chunks into decoration chunks (`SKIP_AIR_SECTIONS`, `CONVERT_WITH_HEIGHTMAPS`,
//! `REUSE_PROTO_BUFFER`) with the old paths, and their benches (`--ignored`):
//! `cargo test --release -p rc-worldgen --test fill_convert_opt -- --ignored --nocapture`.
//! Game data missing: tests skipped, failure under `RUSTCRAFT_OPT_REQUIRE=1`.

use std::sync::{Arc, OnceLock};
use std::time::Instant;

use rayon::prelude::*;
use rc_worldgen::biome::BiomeSource;
use rc_worldgen::chunkgen::{ConversionOpts, DimGen, GenSpec, SourceSpec, WorldData};
use rc_worldgen::decor_region::{DecoChunk, FINAL_HMS, Retro};
use rc_worldgen::palette::AIR;
use rc_worldgen::proto::ProtoTerrain;
use rc_worldgen::terrain::{BiomeCache, Stage, TerrainGenerator, TerrainOpts};

/// Allocation counter around the client's allocator (mimalloc), so that the bench counts and times
/// what the game does.
#[allow(unsafe_code)]
mod counting {
    use std::alloc::{GlobalAlloc, Layout};
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

    use mimalloc::MiMalloc;

    static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
    static BYTES: AtomicU64 = AtomicU64::new(0);

    /// Allocations and bytes asked since the start of the process.
    pub fn totals() -> (u64, u64) {
        (ALLOCATIONS.load(Relaxed), BYTES.load(Relaxed))
    }

    fn count(bytes: usize) {
        ALLOCATIONS.fetch_add(1, Relaxed);
        BYTES.fetch_add(bytes as u64, Relaxed);
    }

    struct Counting;

    // SAFETY: delegates as is to mimalloc; only two counters are added.
    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            count(l.size());
            // SAFETY: same preconditions as the caller.
            unsafe { MiMalloc.alloc(l) }
        }
        unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
            count(l.size());
            // SAFETY: same preconditions as the caller.
            unsafe { MiMalloc.alloc_zeroed(l) }
        }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            // SAFETY: `p` comes from this allocator, hence from mimalloc.
            unsafe { MiMalloc.dealloc(p, l) }
        }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
            count(n);
            // SAFETY: same preconditions as the caller.
            unsafe { MiMalloc.realloc(p, l, n) }
        }
    }

    #[global_allocator]
    static ALLOCATOR: Counting = Counting;

    /// Index of `mi_option_page_commit_on_demand` in the `mi_option_t` enumeration of the linked
    /// library (same constant as `tests/sampler_opt.rs` and the client's `debug_sys.rs`).
    const MI_OPTION_PAGE_COMMIT_ON_DEMAND: i32 = 40;

    /// Setting of the allocator in the client: pages committed on demand.
    pub fn client_options() {
        unsafe extern "C" {
            fn mi_option_set(option: i32, value: std::ffi::c_long);
        }
        // SAFETY: function of the linked mimalloc library (global allocator); the value 1 is documented.
        unsafe { mi_option_set(MI_OPTION_PAGE_COMMIT_ON_DEMAND, 1) }
    }
}

/// Noise settings of the main world whose filling block is cave air (a data pack may pick any
/// block): cells that count for no map and no section without being plain air.
const CAVE_AIR_SETTINGS: &str = "minecraft:rustcraft_test_cave_air_filling";

const SEEDS: [i64; 3] = [0, -6_214_586_474_519_478_921, 1_234_567_890_123_456_789];

/// Chunks compared: near the origin and far away in every direction.
const TARGETS: [(i32, i32); 8] =
    [(0, 0), (-1, -1), (5, -7), (12, 3), (-1500, 220), (873, -2950), (2999, 2999), (-2400, -1300)];

/// Game data, with the cave air settings added to the pack.
fn world() -> Option<&'static WorldData> {
    static WORLD: OnceLock<Option<WorldData>> = OnceLock::new();
    WORLD
        .get_or_init(|| {
            let jar = rc_data::locate("26.3").ok().map(|i| i.client_jar).filter(|j| j.is_file());
            let jar = rc_testdata::need(jar, "jar client 26.3")?;
            let version = rc_testdata::need(rc_data::load_cached("26.3"), "rc-data cache 26.3")?;
            let reports = rc_data::cache_root().join("26.3").join("generated").join("reports");
            let mut w = WorldData::load(&jar, Some(&reports), &version).expect("world data");
            let overworld =
                w.pack.raw("noise_settings", "minecraft:overworld").expect("main world settings").to_owned();
            let cave_air = with_default_block(&overworld, "minecraft:cave_air");
            Arc::make_mut(&mut w.pack).insert(&format!("noise_settings/{CAVE_AIR_SETTINGS}"), cave_air);
            Some(w)
        })
        .as_ref()
}

/// Main world settings text with another filling block than stone.
fn with_default_block(settings: &str, block: &str) -> String {
    let stone = "\"default_block\": \"minecraft:stone\"";
    assert!(settings.contains(stone), "filling block of the main world settings not found");
    settings.replacen(stone, &format!("\"default_block\": \"{block}\""), 1)
}

// ---- Filling --------------------------------------------------------------------------------

/// Terrain generators compared: main world (normal, amplified), Nether, End, main world filled
/// with cave air.
fn terrain_generators(w: &WorldData, seed: i64) -> Vec<(&'static str, TerrainGenerator)> {
    let presets = [
        ("main world", "minecraft:normal", "minecraft:overworld"),
        ("amplified", "minecraft:amplified", "minecraft:overworld"),
        ("Nether", "minecraft:normal", "minecraft:the_nether"),
        ("End", "minecraft:normal", "minecraft:the_end"),
    ];
    let mut out: Vec<_> = presets
        .iter()
        .map(|&(label, preset, dim)| {
            (label, TerrainGenerator::from_preset(&w.pack, preset, dim, seed).expect("generator"))
        })
        .collect();
    let (source, _) = BiomeSource::from_preset(&w.pack, "minecraft:normal", "minecraft:overworld").expect("source");
    let cave_air = TerrainGenerator::new(&w.pack, CAVE_AIR_SETTINGS, source, seed, -64, 384).expect("generator");
    out.push(("cave air filling", cave_air));
    out
}

fn generate_targets(t: &TerrainGenerator, cache: &BiomeCache, stage: Stage) -> Vec<ProtoTerrain> {
    TARGETS.par_iter().map(|&(x, z)| t.generate_until(x, z, cache, None, stage)).collect()
}

/// First difference between two proto-chunks (blocks, section counts, both generation maps, fluid
/// marks in their order), `None` if identical.
fn proto_difference(a: &ProtoTerrain, b: &ProtoTerrain) -> Option<String> {
    if let Some(i) = a.blocks.iter().zip(&b.blocks).position(|(x, y)| x != y) {
        return Some(format!("block {i}: {} against {}", a.blocks[i], b.blocks[i]));
    }
    if a.section_counts != b.section_counts {
        return Some(format!("section counts {:?} against {:?}", a.section_counts, b.section_counts));
    }
    if a.world_surface_wg != b.world_surface_wg {
        return Some("WORLD_SURFACE_WG".into());
    }
    if a.ocean_floor_wg != b.ocean_floor_wg {
        return Some("OCEAN_FLOOR_WG".into());
    }
    if a.post_processing != b.post_processing {
        return Some(format!("fluid marks ({} against {})", a.post_processing.len(), b.post_processing.len()));
    }
    None
}

/// Column filling against the block by block one, after the filling and after the caves (which
/// read the counts and the maps), on the old aquifer paths and on the default ones.
#[test]
fn column_filling_matches_block_by_block() {
    let Some(w) = world() else { return };
    let mut compared = 0;
    let mut failures = Vec::new();
    for seed in SEEDS {
        for (label, mut t) in terrain_generators(w, seed) {
            let mut cache = BiomeCache::default();
            t.prepare_biomes(&mut cache, &TARGETS);
            for (base_name, base) in [("old aquifer", TerrainOpts::OLD), ("default", TerrainOpts::from_env())] {
                for stage in [Stage::Fill, Stage::Carvers] {
                    t.opts = TerrainOpts { fill_column_tracking: false, ..base };
                    let reference = generate_targets(&t, &cache, stage);
                    t.opts = TerrainOpts { fill_column_tracking: true, ..base };
                    let got = generate_targets(&t, &cache, stage);
                    for ((a, b), at) in reference.iter().zip(&got).zip(TARGETS) {
                        compared += 1;
                        if let Some(d) = proto_difference(a, b) {
                            failures.push(format!("{label} seed {seed} {at:?} {base_name} {stage:?}: {d}"));
                        }
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{} differences:\n{}", failures.len(), failures.join("\n"));
    assert_eq!(compared, SEEDS.len() * 5 * 2 * 2 * TARGETS.len(), "chunks compared");
}

/// What a column filling keeping the LOWEST counted block of each column (instead of the first one
/// met from the top) would give.
fn maps_at_lowest_counted_block(c: &ProtoTerrain, t: &TerrainGenerator) -> ProtoTerrain {
    let mut s = c.clone();
    for z in 0..16 {
        for x in 0..16 {
            let lowest = |counts: &dyn Fn(u16) -> bool| {
                (c.min_y..=c.max_y()).find(|&y| counts(c.get(x, y, z))).map_or(c.min_y, |y| y + 1)
            };
            let col = (x + 16 * z) as usize;
            s.world_surface_wg[col] = lowest(&|b| !t.blocks.is_air(b));
            s.ocean_floor_wg[col] = lowest(&|b| t.blocks.blocks_motion(b));
        }
    }
    s
}

/// What a filling counting every cell that is not plain air (cave air included) would give.
fn counts_of_cells_not_plain_air(c: &ProtoTerrain) -> ProtoTerrain {
    let mut s = c.clone();
    for (count, section) in s.section_counts.iter_mut().zip(c.blocks.as_chunks::<4096>().0) {
        *count = section.iter().filter(|&&b| b != AIR).count() as u16;
    }
    s
}

/// The data and the comparison of `column_filling_matches_block_by_block` see both mistakes a
/// column filling could make: maps set at the last counted block, and cave air counted (only the
/// cave air filling has such cells, so it must be the one that catches it).
#[test]
fn filling_comparison_catches_sabotage() {
    let Some(w) = world() else { return };
    let mut caught_maps = Vec::new();
    let mut caught_counts = Vec::new();
    for (label, mut t) in terrain_generators(w, SEEDS[0]) {
        t.opts = TerrainOpts { fill_column_tracking: true, ..TerrainOpts::from_env() };
        let mut cache = BiomeCache::default();
        t.prepare_biomes(&mut cache, &TARGETS[..2]);
        for &(x, z) in &TARGETS[..2] {
            let c = t.generate_until(x, z, &cache, None, Stage::Fill);
            if proto_difference(&c, &maps_at_lowest_counted_block(&c, &t)).is_some() {
                caught_maps.push(label);
            }
            if proto_difference(&c, &counts_of_cells_not_plain_air(&c)).is_some() {
                caught_counts.push(label);
            }
        }
    }
    assert!(caught_maps.len() >= 8, "maps at the last counted block seen only in {caught_maps:?}");
    assert!(caught_counts.contains(&"cave air filling"), "cave air counted not seen ({caught_counts:?})");
}

// ---- Conversion -----------------------------------------------------------------------------

/// Generators compared: main world with structures (3 seeds), Nether, End, main world filled with
/// cave air (whole sections of cave air under the ground).
fn dim_generators(w: &WorldData) -> Vec<(String, DimGen)> {
    let noise = |settings: &str, source: &str, dim: &str, seed: i64| {
        let spec = GenSpec::Noise { settings: settings.into(), source: SourceSpec::MultiNoise(source.into()) };
        DimGen::new(w, &spec, dim, seed).expect("generator")
    };
    let mut out = Vec::new();
    for seed in SEEDS {
        let mut dg = noise("minecraft:overworld", "minecraft:overworld", "minecraft:overworld", seed);
        dg.enable_structures(w).expect("structures");
        out.push((format!("main world seed {seed}"), dg));
    }
    out.push(("Nether".into(), noise("minecraft:nether", "minecraft:nether", "minecraft:the_nether", SEEDS[1])));
    let end = GenSpec::Noise { settings: "minecraft:end".into(), source: SourceSpec::TheEnd };
    out.push(("End".into(), DimGen::new(w, &end, "minecraft:the_end", SEEDS[2]).expect("generator")));
    out.push((
        "cave air filling".into(),
        noise(CAVE_AIR_SETTINGS, "minecraft:overworld", "minecraft:overworld", SEEDS[0]),
    ));
    out
}

/// Each conversion switch alone, the pairs, then all.
fn conversion_variants() -> Vec<(&'static str, ConversionOpts)> {
    let old = ConversionOpts::OLD;
    vec![
        ("skip air", ConversionOpts { skip_air_sections: true, ..old }),
        ("maps while converting", ConversionOpts { heightmaps_while_converting: true, ..old }),
        ("reuse buffer", ConversionOpts { reuse_proto_buffer: true, ..old }),
        ("skip air + maps", ConversionOpts { skip_air_sections: true, heightmaps_while_converting: true, ..old }),
        ("skip air + reuse", ConversionOpts { skip_air_sections: true, reuse_proto_buffer: true, ..old }),
        ("maps + reuse", ConversionOpts { heightmaps_while_converting: true, reuse_proto_buffer: true, ..old }),
        ("all", ConversionOpts::NEW),
    ]
}

/// First difference between two decoration chunks at status `terrain`, `None` if identical.
fn chunk_difference(a: &DecoChunk, b: &DecoChunk) -> Option<String> {
    if let Some(i) = a.blocks.iter().zip(&b.blocks).position(|(x, y)| x != y) {
        return Some(format!("state {i}: {} against {}", a.blocks[i], b.blocks[i]));
    }
    if a.blocks.len() != b.blocks.len() {
        return Some("chunk height".into());
    }
    if let Some(m) = (0..6).find(|&m| a.heights[m] != b.heights[m]) {
        return Some(format!("height map {m}"));
    }
    if a.biomes != b.biomes {
        return Some("biomes".into());
    }
    if a.post_process != b.post_process {
        return Some(format!("fluid marks ({} against {})", a.post_process.len(), b.post_process.len()));
    }
    None
}

fn terrain_targets(dg: &DimGen, cache: &BiomeCache) -> Vec<DecoChunk> {
    TARGETS.par_iter().map(|&(x, z)| dg.terrain(x, z, cache, None)).collect()
}

/// Every conversion path against the old one, twice in a row (the second run takes the buffers
/// handed back by the first). The cave air filling must hold sections of cave air only, which a
/// skip keyed on the section counts would turn into plain air.
#[test]
fn conversion_paths_match_old() {
    let Some(w) = world() else { return };
    let mut failures = Vec::new();
    let mut cave_air_sections = 0;
    let cave_air = w.blocks.default_state(w.blocks.k.cave_air);
    for (label, mut dg) in dim_generators(w) {
        let mut cache = BiomeCache::default();
        dg.prepare_biomes(&mut cache, &TARGETS);
        dg.conversion = ConversionOpts::OLD;
        let reference = terrain_targets(&dg, &cache);
        cave_air_sections += reference
            .iter()
            .flat_map(|c| c.blocks.as_chunks::<4096>().0)
            .filter(|s| s.iter().all(|&v| v == cave_air))
            .count();
        for (name, o) in conversion_variants() {
            dg.conversion = o;
            for run in 0..2 {
                for ((a, b), at) in reference.iter().zip(terrain_targets(&dg, &cache)).zip(TARGETS) {
                    if let Some(d) = chunk_difference(a, &b) {
                        failures.push(format!("{label} {at:?} [{name}, run {run}]: {d}"));
                    }
                }
            }
        }
    }
    assert!(failures.is_empty(), "{} differences:\n{}", failures.len(), failures.join("\n"));
    assert!(cave_air_sections > 0, "no section of cave air only: the cave air case is not tested");
}

/// The data and the comparison of `conversion_paths_match_old` see the mistakes of a skipping
/// conversion: a walk of the final maps started one section too low, and sections skipped by their
/// count (cave air turned into plain air).
#[test]
fn conversion_comparison_catches_sabotage() {
    let Some(w) = world() else { return };
    let b = &*w.blocks;
    let air = b.default_state(b.k.air);
    let (mut too_low_seen, mut by_count_seen) = (0, 0);
    for (label, mut dg) in dim_generators(w) {
        dg.conversion = ConversionOpts::OLD;
        let mut cache = BiomeCache::default();
        dg.prepare_biomes(&mut cache, &TARGETS[..2]);
        let t = dg.terrain_generator().expect("noise generator");
        for &(x, z) in &TARGETS[..2] {
            let reference = dg.terrain(x, z, &cache, None);
            let mut too_low = reference.clone();
            too_low.prime_many(b, &FINAL_HMS, reference.top_non_air(b) - 16);
            too_low_seen += usize::from(chunk_difference(&reference, &too_low).is_some());
            let p = t.generate(x, z, &cache, None);
            let mut by_count = reference.clone();
            let empty_by_count = p.section_counts.iter().enumerate().filter(|(_, n)| **n == 0);
            for (k, _) in empty_by_count {
                by_count.blocks[k * 4096..(k + 1) * 4096].fill(air);
            }
            let seen = chunk_difference(&reference, &by_count).is_some();
            by_count_seen += usize::from(seen);
            if seen {
                eprintln!("{label} {x} {z}: sections skipped by count differ");
            }
        }
    }
    assert!(too_low_seen >= 10, "ceiling one section too low seen in {too_low_seen} chunks only");
    assert!(by_count_seen > 0, "a skip by section count is not seen");
}

/// Regeneration below y = 0 of a converted chunk: the band's old blocks are overwritten on every
/// path (an old chunk standing for itself, with a few columns without bedrock).
#[test]
fn regeneration_below_zero_matches_old() {
    let Some(w) = world() else { return };
    let spec = GenSpec::Noise {
        settings: "minecraft:overworld".into(),
        source: SourceSpec::MultiNoise("minecraft:overworld".into()),
    };
    let mut dg = DimGen::new(w, &spec, "minecraft:overworld", SEEDS[1]).expect("generator");
    let mut cache = BiomeCache::default();
    dg.prepare_biomes(&mut cache, &TARGETS);
    let mut compared = 0;
    for &(x, z) in &TARGETS {
        dg.conversion = ConversionOpts::OLD;
        let mut old = dg.terrain(x, z, &cache, None);
        old.retro = Some(Retro { missing_bedrock: vec![0x00ff_00ff_00ff_00ff, 0, 0, u64::MAX] });
        let reference = dg.retro_terrain(&old).expect("regeneration");
        for (name, o) in conversion_variants() {
            dg.conversion = o;
            let got = dg.retro_terrain(&old).expect("regeneration");
            if let Some(d) = chunk_difference(&reference, &got) {
                panic!("({x}, {z}) [{name}]: {d}");
            }
            compared += 1;
        }
    }
    assert_eq!(compared, TARGETS.len() * conversion_variants().len());
}

// ---- Benches (loaded machine unless stated otherwise) ---------------------------------------

/// 12 × 12 grid from (0, 0).
fn grid() -> Vec<(i32, i32)> {
    (0..12).flat_map(|x| (0..12).map(move |z| (x, z))).collect()
}

/// Runs every variant once per round, rounds alternated, and prints the best and the median
/// figure of each (`run` returns it, in `unit`): the best is the figure least touched by the
/// other work of a loaded machine.
fn alternated<T: Copy>(label: &str, unit: &str, rounds: usize, variants: &[(&str, T)], mut run: impl FnMut(T) -> f64) {
    let mut figures = vec![Vec::with_capacity(rounds); variants.len()];
    for _ in 0..rounds {
        for (k, &(_, v)) in variants.iter().enumerate() {
            figures[k].push(run(v));
        }
    }
    for ((name, _), mut f) in variants.iter().zip(figures) {
        f.sort_by(f64::total_cmp);
        eprintln!("{label} {name:>22}: best {:.4}, median {:.4} {unit} ({rounds} rounds)", f[0], f[rounds / 2]);
    }
}

/// Processor cycles spent by the calling thread so far (Windows; 0 elsewhere). Unlike the wall
/// clock, a thread put aside by the system for another process counts nothing meanwhile: the
/// 1-thread figure holds up better on a loaded machine.
#[cfg(windows)]
#[allow(unsafe_code)]
fn thread_cycles() -> u64 {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentThread() -> isize;
        fn QueryThreadCycleTime(thread: isize, cycles: *mut u64) -> i32;
    }
    let mut c = 0u64;
    // SAFETY: the pseudo handle of the current thread is always valid and `c` outlives the call.
    unsafe { QueryThreadCycleTime(GetCurrentThread(), &mut c) };
    c
}

#[cfg(not(windows))]
fn thread_cycles() -> u64 {
    0
}

/// Millions of cycles of the calling thread per item of `f`, which handles `items` of them.
fn megacycles_per_item(items: usize, f: impl FnOnce()) -> f64 {
    let c0 = thread_cycles();
    f();
    (thread_cycles() - c0) as f64 / 1e6 / items as f64
}

/// Logical cores, then 1 and the powers of 2 below them.
fn thread_counts() -> Vec<usize> {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut counts: Vec<usize> = std::iter::successors(Some(1), |n| Some(n * 2)).take_while(|&n| n < cores).collect();
    counts.push(cores);
    counts
}

/// Filling alone (`Stage::Fill`), block by block against by column, on the default aquifer paths:
/// 1 thread, then threads in powers of 2 up to the logical cores; 10 alternated rounds each.
#[test]
#[ignore = "bench"]
fn fill_bench() {
    let Some(w) = world() else { return };
    let mut t =
        TerrainGenerator::from_preset(&w.pack, "minecraft:normal", "minecraft:overworld", 0).expect("generator");
    let g = grid();
    let mut cache = BiomeCache::default();
    t.prepare_biomes(&mut cache, &g);
    let base = TerrainOpts::from_env();
    let variants = [
        ("block by block", TerrainOpts { fill_column_tracking: false, ..base }),
        ("by column", TerrainOpts { fill_column_tracking: true, ..base }),
    ];
    let _ = t.generate_until(50, 50, &cache, None, Stage::Fill);
    alternated("filling, cycles of the thread", "Mcycles / chunk", 10, &variants, |o| {
        t.opts = o;
        megacycles_per_item(g.len(), || {
            for &(x, z) in &g {
                std::hint::black_box(t.generate_until(x, z, &cache, None, Stage::Fill));
            }
        })
    });
    for threads in thread_counts() {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("pool");
        alternated(&format!("filling {threads:>2} threads"), "ms / chunk", 10, &variants, |o| {
            t.opts = o;
            let s = Instant::now();
            pool.install(|| {
                let out: Vec<_> =
                    g.par_iter().map(|&(x, z)| t.generate_until(x, z, &cache, None, Stage::Fill)).collect();
                std::hint::black_box(out);
            });
            s.elapsed().as_secs_f64() * 1e3 / g.len() as f64
        });
    }
}

/// Conversion alone of 25 noise proto-chunks (main world seed 1, with structures) into decoration
/// chunks, every path, best of 20 on 1 thread; then the whole `DimGen::terrain` of the 12 × 12 grid
/// on 1 thread and on every core, buffer reuse off and on, with allocations counted.
#[test]
#[ignore = "bench"]
fn conversion_bench() {
    let Some(w) = world() else { return };
    let spec = GenSpec::Noise {
        settings: "minecraft:overworld".into(),
        source: SourceSpec::MultiNoise("minecraft:overworld".into()),
    };
    let mut dg = DimGen::new(w, &spec, "minecraft:overworld", 1).expect("generator");
    dg.enable_structures(w).expect("structures");
    let around: Vec<(i32, i32)> = (-2..=2).flat_map(|x| (-2..=2).map(move |z| (x, z))).collect();
    let mut cache = BiomeCache::default();
    dg.prepare_biomes(&mut cache, &around);
    counting::client_options();
    let t = dg.terrain_generator().expect("noise generator");
    let protos: Vec<ProtoTerrain> = around.iter().map(|&(x, z)| t.generate(x, z, &cache, None)).collect();
    let mut variants = vec![("old", ConversionOpts::OLD)];
    variants.extend(conversion_variants().into_iter().filter(|(_, o)| !o.reuse_proto_buffer));
    alternated("conversion alone, 1 thread", "ms / chunk", 20, &variants, |o| {
        dg.conversion = o;
        let t = dg.terrain_generator().expect("noise generator");
        let s = Instant::now();
        for p in &protos {
            std::hint::black_box(dg.convert_noise_terrain(t, p, &cache));
        }
        s.elapsed().as_secs_f64() * 1e3 / protos.len() as f64
    });
    alternated("conversion alone, cycles of the thread", "Mcycles / chunk", 20, &variants, |o| {
        dg.conversion = o;
        let t = dg.terrain_generator().expect("noise generator");
        megacycles_per_item(protos.len(), || {
            for p in &protos {
                std::hint::black_box(dg.convert_noise_terrain(t, p, &cache));
            }
        })
    });
    let g = grid();
    dg.prepare_biomes(&mut cache, &g);
    let whole = [
        ("old", ConversionOpts::OLD),
        ("new, no reuse", ConversionOpts { reuse_proto_buffer: false, ..ConversionOpts::NEW }),
        ("new", ConversionOpts::NEW),
    ];
    // Warm-up: structure starts and caches of the grid filled before anything is counted.
    for &(x, z) in &g {
        std::hint::black_box(dg.terrain(x, z, &cache, None));
    }
    for (name, o) in whole {
        dg.conversion = o;
        let (a0, b0) = counting::totals();
        for &(x, z) in &g {
            std::hint::black_box(dg.terrain(x, z, &cache, None));
        }
        let (a1, b1) = counting::totals();
        let n = g.len() as f64;
        let (allocations, kb) = ((a1 - a0) as f64 / n, (b1 - b0) as f64 / n / 1024.0);
        eprintln!("terrain {name:>14}, 1 thread: {allocations:.1} allocations, {kb:.0} KB allocated / chunk");
    }
    alternated("terrain, cycles of the thread", "Mcycles / chunk", 6, &whole, |o| {
        dg.conversion = o;
        megacycles_per_item(g.len(), || {
            for &(x, z) in &g {
                std::hint::black_box(dg.terrain(x, z, &cache, None));
            }
        })
    });
    for threads in thread_counts() {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("pool");
        alternated(&format!("terrain {threads:>2} threads"), "ms / chunk", 6, &whole, |o| {
            dg.conversion = o;
            let s = Instant::now();
            pool.install(|| {
                g.par_iter().for_each(|&(x, z)| {
                    std::hint::black_box(dg.terrain(x, z, &cache, None));
                });
            });
            s.elapsed().as_secs_f64() * 1e3 / g.len() as f64
        });
    }
}
