//! Preliminary surface levels shared by the aquifers (`RUSTCRAFT_OPT_AQUIFER_SURFACE_TILES`):
//! a tile holds, quart for quart, the level the aquifer of each chunk computes for itself, and
//! the terrain is the same to the block with and without the tiles. Missing game data makes the
//! tests fail under `RUSTCRAFT_OPT_REQUIRE=1`.

mod common;

use rayon::prelude::*;
use rc_worldgen::data::DataPack;
use rc_worldgen::proto::ProtoTerrain;
use rc_worldgen::terrain::{BiomeCache, Stage, TerrainGenerator, TerrainOpts};
use rc_worldgen::{Ctx, Volume};

const SEEDS: [i64; 3] = [0, -6_214_586_474_519_478_921, 1_234_567_890_123_456_789];

/// The world presets whose noise settings have aquifers.
const AQUIFER_PRESETS: [&str; 3] = ["minecraft:normal", "minecraft:amplified", "minecraft:large_biomes"];

/// Tiles with the 2D climate under their levels (the default).
const TILES_ON: TerrainOpts = TerrainOpts::NEW;
/// Tiles holding the levels only.
const TILES_WITHOUT_CLIMATE: TerrainOpts = TerrainOpts { shared_climate_tiles: false, ..TerrainOpts::NEW };
/// Each aquifer computes its own window (the old path).
const TILES_OFF: TerrainOpts =
    TerrainOpts { aquifer_surface_tiles: false, shared_climate_tiles: false, ..TerrainOpts::NEW };

fn generator(pack: &DataPack, preset: &str, dimension: &str, seed: i64, opts: TerrainOpts) -> TerrainGenerator {
    let mut t = TerrainGenerator::from_preset(pack, preset, dimension, seed).expect("generator");
    t.opts = opts;
    t
}

/// Chunks whose aquifer windows sit inside a tile, on its edges and on its corners (a tile is
/// 4 × 4 chunks), on both sides of the origin, then far ones.
fn probe_chunks(seed: i64) -> Vec<(i32, i32)> {
    let mut chunks = vec![(0, 0), (-1, -1), (1, 2), (3, 3), (4, 4), (-4, -5), (-5, 3), (7, -8), (15, 16), (-17, 12)];
    let mut state = seed as u64 ^ 0x9E37_79B9_7F4A_7C15;
    let mut draw = || {
        state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
        ((state >> 33) % 6000) as i32 - 3000
    };
    chunks.extend((0..10).map(|_| (draw(), draw())));
    chunks
}

/// A chunk and the four next to it: a batch that always gets its tiles (a lone chunk far from
/// every tile computes its own window instead).
fn plus_around((cx, cz): (i32, i32)) -> [(i32, i32); 5] {
    [(cx, cz), (cx - 1, cz), (cx + 1, cz), (cx, cz - 1), (cx, cz + 1)]
}

/// Cache of `t` prepared around each chunk of `chunks`, one batch per chunk.
fn prepared_around(t: &TerrainGenerator, chunks: &[(i32, i32)]) -> BiomeCache {
    let mut cache = BiomeCache::default();
    for &chunk in chunks {
        t.prepare_biomes(&mut cache, &plus_around(chunk));
    }
    cache
}

/// 2 × 2 chunks at the origin: their batch prepares the four tiles around the origin, which hold
/// the chunks −4 to 3 in x and z.
const BLOCK_AT_ORIGIN: [(i32, i32); 4] = [(0, 0), (1, 0), (0, 1), (1, 1)];

/// Quarts the aquifer of a chunk may read: 27 × 19, from 4 chunks west to 2 chunks east (less
/// its last quart) and 2 chunks north to 2 south of the chunk.
const QUARTS_READ_BY_AN_AQUIFER: usize = 27 * 19;

/// Levels compared and levels that differ between the tiles of `cache` and what the aquifer of
/// each chunk of `chunks` computes for itself: its creation window through one volume, the quarts
/// further out point by point, in the chunk's own context.
fn tile_level_differences(own: &TerrainGenerator, cache: &BiomeCache, chunks: &[(i32, i32)]) -> (usize, Vec<String>) {
    let per_chunk: Vec<(usize, Vec<String>)> = chunks
        .par_iter()
        .map(|&(cx, cz)| {
            let vol = own.fill_volume(cx, cz).expect("chunk volume");
            let mut ctx = Ctx::cached();
            let mut aquifer = own.new_aquifer(&mut ctx, &vol);
            let (mut compared, mut differences) = (0, Vec::new());
            for qz in cz * 4 - 8..=cz * 4 + 10 {
                for qx in cx * 4 - 16..=cx * 4 + 10 {
                    let Some(shared) = cache.surface_tiles.level(qx, qz) else { continue };
                    let computed = aquifer.preliminary_surface_level(&own.g, &mut ctx, qx << 2, qz << 2);
                    compared += 1;
                    if computed != Some(shared) {
                        differences
                            .push(format!("chunk {cx},{cz} quart {qx},{qz}: tile {shared}, aquifer {computed:?}"));
                    }
                }
            }
            (compared, differences)
        })
        .collect();
    let compared = per_chunk.iter().map(|(n, _)| n).sum();
    (compared, per_chunk.into_iter().flat_map(|(_, d)| d).collect())
}

/// Without this property the tiles are never filled and every aquifer silently computes its own
/// window: the 26.3 data must have it.
#[test]
fn surface_levels_can_be_shared_in_the_26_3_data() {
    let Some(pack) = common::data_pack() else { return };
    for preset in AQUIFER_PRESETS {
        let t = generator(&pack, preset, "minecraft:overworld", 0, TILES_ON);
        assert!(
            t.surface_levels_shareable(),
            "{preset}: the preliminary surface level depends on more than the position"
        );
        let mut cache = BiomeCache::default();
        t.prepare_biomes(&mut cache, &BLOCK_AT_ORIGIN);
        assert_eq!(cache.surface_tiles.len(), 4, "{preset}: surface tiles prepared");
    }
    for dimension in ["minecraft:the_nether", "minecraft:the_end"] {
        let t = generator(&pack, "minecraft:normal", dimension, 0, TILES_ON);
        assert!(!t.surface_levels_shareable(), "{dimension} has no noise aquifer");
        let mut cache = BiomeCache::default();
        t.prepare_biomes(&mut cache, &BLOCK_AT_ORIGIN);
        assert!(cache.surface_tiles.is_empty(), "{dimension}: surface tiles prepared without aquifers");
    }
}

/// A batch adds tiles while it needs at most two per chunk: a lone chunk far from every tile
/// would compute 4 to 6 tiles for itself and keeps its own window; next to tiles already there it
/// adds the one or two it lacks.
#[test]
fn small_batches_only_add_the_tiles_they_are_worth() {
    let Some(pack) = common::data_pack() else { return };
    let t = generator(&pack, "minecraft:normal", "minecraft:overworld", 0, TILES_ON);
    let mut cache = BiomeCache::default();
    t.prepare_biomes(&mut cache, &[(100, 100)]);
    assert_eq!(cache.surface_tiles.len(), 0, "tiles prepared for a lone chunk");
    t.prepare_biomes(&mut cache, &BLOCK_AT_ORIGIN);
    assert_eq!(cache.surface_tiles.len(), 4, "tiles prepared for a block of 2 × 2 chunks");
    // The chunk (2, 0) reads up to the chunk 4, in the next tiles east.
    t.prepare_biomes(&mut cache, &[(2, 0)]);
    assert_eq!(cache.surface_tiles.len(), 6, "tiles after a lone chunk next to the block");
}

/// The sharing guard on built functions. Shareable: an exact function; a top surface search whose
/// bound is exact, whatever its density (the 26.3 shape: the density multiplies two functions).
/// Not shareable: a bound that is not exact, and a cache over an interpolation anywhere below
/// (what it answers depends on the points its buffer holds).
#[test]
fn sharing_guard_refuses_levels_that_depend_on_how_they_are_read() {
    let Some(pack) = common::data_pack() else { return };
    const EXACT_NOISE: &str = r#"{"type":"minecraft:noise","noise":"minecraft:erosion","xz_scale":0.25,"y_scale":0.0}"#;
    const PRODUCT_OF_NOISES: &str = r#"{"type":"minecraft:mul",
        "left":{"type":"minecraft:noise","noise":"minecraft:erosion","xz_scale":0.25,"y_scale":0.0},
        "right":{"type":"minecraft:noise","noise":"minecraft:ridge","xz_scale":0.25,"y_scale":0.0}}"#;
    const CACHE_OVER_INTERPOLATION: &str = r#"{"type":"minecraft:cache","input":{"type":"minecraft:interpolated",
        "cell_size_xz":4,"cell_size_y":8,"input":{"type":"minecraft:noise","noise":"minecraft:cave_cheese",
        "xz_scale":1.0,"y_scale":0.6666666666666666}}}"#;
    let top_surface = |density: &str, upper_bound: &str| {
        format!(
            r#"{{"type":"minecraft:find_top_surface","density":{density},"upper_bound":{upper_bound},"lower_bound":-64,"cell_height":8}}"#
        )
    };
    let mut g = rc_worldgen::Generator::new(&pack, "overworld", 0).expect("generator");
    let mut shareable = |text: &str| {
        let function = rc_worldgen::density::parse(&rc_worldgen::json::parse(text).expect("json")).expect("function");
        let sampler = g.compile_function(&function).expect("compiled");
        g.program.value_depends_only_on_position(sampler)
    };
    assert!(shareable(EXACT_NOISE), "an exact function");
    assert!(shareable(&top_surface(PRODUCT_OF_NOISES, EXACT_NOISE)), "exact bound, density that is not exact");
    assert!(!shareable(PRODUCT_OF_NOISES), "a product: its point and volume values can differ");
    assert!(!shareable(&top_surface(EXACT_NOISE, PRODUCT_OF_NOISES)), "bound that is not exact");
    assert!(
        !shareable(&top_surface(CACHE_OVER_INTERPOLATION, EXACT_NOISE)),
        "cache over an interpolation in the density"
    );
    assert!(
        !shareable(&top_surface(EXACT_NOISE, CACHE_OVER_INTERPOLATION)),
        "cache over an interpolation in the bound"
    );
}

/// Tile level = level of the chunk's own aquifer, on every quart an aquifer reads, for chunks
/// inside, on the edges and on the corners of the tiles.
#[test]
fn tile_levels_match_each_aquifers_own_levels() {
    let Some(pack) = common::data_pack() else { return };
    let mut failures = Vec::new();
    for preset in AQUIFER_PRESETS {
        for seed in SEEDS {
            let shared = generator(&pack, preset, "minecraft:overworld", seed, TILES_ON);
            let own = generator(&pack, preset, "minecraft:overworld", seed, TILES_OFF);
            let chunks = probe_chunks(seed);
            let cache = prepared_around(&shared, &chunks);
            let (compared, differences) = tile_level_differences(&own, &cache, &chunks);
            eprintln!("{preset} seed {seed}: {compared} levels compared, {} tiles", cache.surface_tiles.len());
            // Preparing a chunk prepares every quart its aquifer may read.
            assert_eq!(compared, QUARTS_READ_BY_AN_AQUIFER * chunks.len(), "{preset} seed {seed}: levels compared");
            failures.extend(differences.into_iter().map(|d| format!("{preset} seed {seed} {d}")));
        }
    }
    assert!(
        failures.is_empty(),
        "{} levels differ:\n{}",
        failures.len(),
        failures[..failures.len().min(20)].join("\n")
    );
}

/// The check above sees tiles read one quart off.
#[test]
fn tile_level_check_catches_a_shifted_key() {
    let Some(pack) = common::data_pack() else { return };
    let shared = generator(&pack, "minecraft:normal", "minecraft:overworld", 0, TILES_ON);
    let own = generator(&pack, "minecraft:normal", "minecraft:overworld", 0, TILES_OFF);
    let chunks = probe_chunks(0);
    let mut cache = prepared_around(&shared, &chunks);
    cache.surface_tiles = cache.surface_tiles.shifted_by_one_quart();
    let (compared, differences) = tile_level_differences(&own, &cache, &chunks);
    eprintln!("shifted tiles: {} of {compared} levels differ", differences.len());
    // The levels are multiples of 8 blocks: most neighbouring quarts share theirs.
    assert!(
        differences.len() * 20 > compared,
        "a shift of one quart changed only {} of {compared} levels",
        differences.len()
    );
    // And the terrain built on the shifted tiles is not the terrain: the tiles are really read.
    let block: Vec<(i32, i32)> = block_across_tiles();
    let mut honest = BiomeCache::default();
    shared.prepare_biomes(&mut honest, &block);
    let mut shifted = BiomeCache::default();
    shared.prepare_biomes(&mut shifted, &block);
    shifted.surface_tiles = shifted.surface_tiles.shifted_by_one_quart();
    let changed = block
        .par_iter()
        .filter(|&&(x, z)| {
            diff(&shared.generate(x, z, &honest, None), &shared.generate(x, z, &shifted, None)).is_some()
        })
        .count();
    eprintln!("shifted tiles: {changed} of {} chunks change", block.len());
    assert!(changed > 0, "shifted surface levels changed no chunk of {}", block.len());
}

/// Difference between two chunks, `None` if identical.
fn diff(a: &ProtoTerrain, b: &ProtoTerrain) -> Option<String> {
    if let Some(i) = a.blocks.iter().zip(&b.blocks).position(|(x, y)| x != y) {
        return Some(format!("block {i}: {} ≠ {}", a.blocks[i], b.blocks[i]));
    }
    if !a.world_surface_wg.iter().eq(b.world_surface_wg.iter()) {
        return Some("surface map".into());
    }
    if !a.ocean_floor_wg.iter().eq(b.ocean_floor_wg.iter()) {
        return Some("floor map".into());
    }
    if a.section_counts != b.section_counts {
        return Some("section counts".into());
    }
    if a.post_processing != b.post_processing {
        return Some(format!("marks ({} against {})", a.post_processing.len(), b.post_processing.len()));
    }
    None
}

/// 6 × 6 chunks across the corner shared by four tiles.
fn block_across_tiles() -> Vec<(i32, i32)> {
    (1..7).flat_map(|x| (-3..3).map(move |z| (x, z))).collect()
}

/// Chunks of `targets` that differ between the generator `a` reading `cache_a` and `b` reading
/// `cache_b`.
fn terrain_differences(
    (a, cache_a): (&TerrainGenerator, &BiomeCache),
    (b, cache_b): (&TerrainGenerator, &BiomeCache),
    targets: &[(i32, i32)],
) -> Vec<String> {
    targets
        .par_iter()
        .filter_map(|&(x, z)| {
            let d = diff(&a.generate(x, z, cache_a, None), &b.generate(x, z, cache_b, None))?;
            Some(format!("chunk {x},{z}: {d}"))
        })
        .collect()
}

/// Complete chunks (noise, aquifers, surface, caves) with the tiles, without them, and with the
/// sharing forbidden by the generator: the same to the block. Nether and End have no noise
/// aquifer and take the same path either way.
#[test]
fn terrain_identical_with_and_without_surface_tiles() {
    let Some(pack) = common::data_pack() else { return };
    let dimensions = [
        ("minecraft:normal", "minecraft:overworld"),
        ("minecraft:amplified", "minecraft:overworld"),
        ("minecraft:large_biomes", "minecraft:overworld"),
        ("minecraft:normal", "minecraft:the_nether"),
        ("minecraft:normal", "minecraft:the_end"),
    ];
    let mut failures = Vec::new();
    for (preset, dimension) in dimensions {
        let has_aquifers = dimension == "minecraft:overworld";
        for seed in SEEDS {
            let block = block_across_tiles();
            let scattered: Vec<(i32, i32)> = probe_chunks(seed).into_iter().skip(5).collect();
            let targets = [block.clone(), scattered.clone()].concat();
            let with_tiles = generator(&pack, preset, dimension, seed, TILES_ON);
            let levels_only = generator(&pack, preset, dimension, seed, TILES_WITHOUT_CLIMATE);
            let without = generator(&pack, preset, dimension, seed, TILES_OFF);
            let mut forbidden = generator(&pack, preset, dimension, seed, TILES_ON);
            forbidden.forbid_shared_surface_levels();
            // The block as one batch, then a batch around each scattered chunk.
            let prepared = |t: &TerrainGenerator| {
                let mut cache = prepared_around(t, &scattered);
                t.prepare_biomes(&mut cache, &block);
                cache
            };
            let cache_without = prepared(&without);
            assert!(cache_without.surface_tiles.is_empty());
            let label = format!("{dimension} {preset} seed {seed}");
            for (path, t, tiles_expected, climate_expected) in [
                ("tiles with climate", &with_tiles, has_aquifers, has_aquifers),
                ("tiles, levels only", &levels_only, has_aquifers, false),
                ("sharing forbidden", &forbidden, false, false),
            ] {
                let cache = prepared(t);
                assert_eq!(!cache.surface_tiles.is_empty(), tiles_expected, "{label} [{path}]: tiles prepared");
                assert_eq!(
                    cache.surface_tiles.climate_caches_kept().0 > 0,
                    climate_expected,
                    "{label} [{path}]: climate kept"
                );
                let differences = terrain_differences((&without, &cache_without), (t, &cache), &targets);
                failures.extend(differences.into_iter().map(|d| format!("{label} [{path}] {d}")));
            }
        }
    }
    assert!(failures.is_empty(), "{} differences:\n{}", failures.len(), failures.join("\n"));
}

/// A chunk whose window the tiles hold only in part, or not at all, computes its own window (and
/// keeps the levels the tiles did hold for its cells): same terrain.
#[test]
fn chunks_outside_the_prepared_tiles_compute_their_own_window() {
    let Some(pack) = common::data_pack() else { return };
    for seed in SEEDS {
        let with_tiles = generator(&pack, "minecraft:normal", "minecraft:overworld", seed, TILES_ON);
        let without = generator(&pack, "minecraft:normal", "minecraft:overworld", seed, TILES_OFF);
        // Tiles of the block at the origin only: those of the chunks −4 to 3 in x and z. Both
        // caches hold the same biomes.
        let mut cache = BiomeCache::default();
        with_tiles.prepare_biomes(&mut cache, &BLOCK_AT_ORIGIN);
        let mut cache_without = BiomeCache::default();
        without.prepare_biomes(&mut cache_without, &BLOCK_AT_ORIGIN);
        let tiles_before = cache.surface_tiles.len();
        assert_eq!((tiles_before, cache_without.surface_tiles.len()), (4, 0), "tiles prepared for the block");
        // Window half inside (3, 0), cells reading into the tiles from outside (5, 1), nothing held (40, 40).
        let targets = [(3, 0), (0, 3), (3, 3), (-5, 0), (5, 1), (40, 40)];
        let differences = terrain_differences((&without, &cache_without), (&with_tiles, &cache), &targets);
        assert!(differences.is_empty(), "seed {seed}: {}", differences.join("\n"));
        assert_eq!(cache.surface_tiles.len(), tiles_before, "generating a chunk must not add tiles");
    }
}

/// Positions where two buffers differ in bits.
fn bit_differences(a: &[f32], b: &[f32]) -> usize {
    assert_eq!(a.len(), b.len());
    a.iter().zip(b).filter(|(x, y)| x.to_bits() != y.to_bits()).count()
}

/// What the tiles' climate changes for the chunks of `chunks`, against a window computed by each
/// chunk: the cache volumes compared, and the differences. After the aquifer is created, each
/// climate cache must hold over the window the same values to the bit on both paths, and the
/// final density computed next in the same context must be the same to the bit.
fn climate_differences(
    shared: &TerrainGenerator,
    own: &TerrainGenerator,
    cache: &BiomeCache,
    chunks: &[(i32, i32)],
) -> (usize, Vec<String>) {
    let per_chunk: Vec<(usize, Vec<String>)> = chunks
        .par_iter()
        .map(|&(cx, cz)| {
            let vol = own.fill_volume(cx, cz).expect("chunk volume");
            let (mut ctx_own, mut ctx_shared) = (Ctx::cached(), Ctx::cached());
            own.new_aquifer(&mut ctx_own, &vol);
            shared.new_chunk_aquifer(&mut ctx_shared, &vol, cache);
            // The aquifer's creation window: the 11 × 11 quarts from the chunk before.
            let window = Volume::stepped(11, 1, 11, (cx - 1) * 16, 0, (cz - 1) * 16, 4, 1, 4);
            let (mut compared, mut differences) = (0, Vec::new());
            for &id in own.surface_climate_caches() {
                // A cache one path did not fill over the window (a spline branch no quart of the
                // window, or of one of its tiles, took) is computed later by whoever reads it:
                // nothing to compare.
                let (Some(computed), Some(carried)) =
                    (ctx_own.cached_volume(id, &window), ctx_shared.cached_volume(id, &window))
                else {
                    continue;
                };
                compared += 1;
                let n = bit_differences(computed, carried);
                if n > 0 {
                    differences.push(format!("chunk {cx},{cz} cache {id}: {n} of {} values differ", computed.len()));
                }
            }
            let final_density = own.g.router.final_density;
            let wanted = own.g.volume(final_density, &mut ctx_own, &vol);
            let got = shared.g.volume(final_density, &mut ctx_shared, &vol);
            let n = bit_differences(&wanted, &got);
            if n > 0 {
                differences.push(format!("chunk {cx},{cz} final density: {n} values differ"));
            }
            (compared, differences)
        })
        .collect();
    let compared = per_chunk.iter().map(|(n, _)| n).sum();
    (compared, per_chunk.into_iter().flat_map(|(_, d)| d).collect())
}

/// The climate carried by the tiles is, to the bit, what the window computed by each chunk leaves
/// in the caches, and the final density read through it is the same to the bit.
#[test]
fn climate_carried_by_tiles_is_what_each_window_computes() {
    let Some(pack) = common::data_pack() else { return };
    let mut failures = Vec::new();
    for preset in AQUIFER_PRESETS {
        for seed in SEEDS {
            let shared = generator(&pack, preset, "minecraft:overworld", seed, TILES_ON);
            let own = generator(&pack, preset, "minecraft:overworld", seed, TILES_OFF);
            let chunks = probe_chunks(seed);
            let cache = prepared_around(&shared, &chunks);
            let (compared, differences) = climate_differences(&shared, &own, &cache, &chunks);
            eprintln!(
                "{preset} seed {seed}: {compared} cache volumes compared over {} chunks, {} climate caches",
                chunks.len(),
                own.surface_climate_caches().len()
            );
            assert!(compared >= 5 * chunks.len(), "{preset} seed {seed}: only {compared} cache volumes compared");
            failures.extend(differences.into_iter().map(|d| format!("{preset} seed {seed} {d}")));
        }
    }
    assert!(failures.is_empty(), "{} differences:\n{}", failures.len(), failures[..failures.len().min(20)].join("\n"));
}

/// The check above sees a climate read one quart off, in the caches and in the final density.
#[test]
fn climate_check_catches_a_shifted_key() {
    let Some(pack) = common::data_pack() else { return };
    let shared = generator(&pack, "minecraft:normal", "minecraft:overworld", 0, TILES_ON);
    let own = generator(&pack, "minecraft:normal", "minecraft:overworld", 0, TILES_OFF);
    let chunks = probe_chunks(0);
    let mut cache = prepared_around(&shared, &chunks);
    cache.surface_tiles = cache.surface_tiles.with_climate_shifted_by_one_quart();
    let (compared, differences) = climate_differences(&shared, &own, &cache, &chunks);
    let densities = differences.iter().filter(|d| d.contains("final density")).count();
    eprintln!(
        "shifted climate: {} differences for {compared} cache volumes, {densities} final densities of {}",
        differences.len(),
        chunks.len()
    );
    assert!(
        differences.len() - densities >= compared / 2,
        "a shifted climate was seen in only {} cache volumes",
        differences.len()
    );
    assert!(densities > 0, "a shifted climate changed no final density");
}

/// Chunks whose biomes differ between two caches holding the same chunks.
fn biome_differences(a: &BiomeCache, b: &BiomeCache) -> Vec<(i32, i32)> {
    assert_eq!(a.chunks.len(), b.chunks.len(), "chunks with biomes");
    assert_eq!(a.carver, b.carver, "cave biomes of the sources");
    a.chunks.iter().filter(|&(at, biomes)| b.chunks.get(at) != Some(biomes)).map(|(at, _)| *at).collect()
}

/// A first batch leaves tiles around its chunks; the biomes of the chunks prepared next read
/// their climate from those tiles where they hold it, and compute it where they do not: the same
/// biomes as computed from scratch. A climate read one quart off is seen.
#[test]
fn biomes_over_the_climate_of_earlier_tiles_are_the_same() {
    let Some(pack) = common::data_pack() else { return };
    let first = BLOCK_AT_ORIGIN;
    // Around the first block: inside its tiles (chunks −4 to 3), across their edges, beyond them.
    let second = [(-3, -3), (3, 3), (-3, 2), (2, -3), (6, 0), (0, -6), (20, 20)];
    let prepared = |t: &TerrainGenerator, shift_climate: bool| {
        let mut cache = BiomeCache::default();
        t.prepare_biomes(&mut cache, &first);
        if shift_climate {
            cache.surface_tiles = cache.surface_tiles.with_climate_shifted_by_one_quart();
        }
        t.prepare_biomes(&mut cache, &second);
        cache
    };
    let mut changed_by_shift = 0;
    for preset in AQUIFER_PRESETS {
        for seed in SEEDS {
            let shared = generator(&pack, preset, "minecraft:overworld", seed, TILES_ON);
            let own = generator(&pack, preset, "minecraft:overworld", seed, TILES_OFF);
            let from_scratch = prepared(&own, false);
            let differences = biome_differences(&from_scratch, &prepared(&shared, false));
            assert!(differences.is_empty(), "{preset} seed {seed}: biomes differ in chunks {differences:?}");
            changed_by_shift += biome_differences(&from_scratch, &prepared(&shared, true)).len();
        }
    }
    eprintln!("climate shifted by one quart: {changed_by_shift} chunks with other biomes");
    assert!(changed_by_shift > 0, "a shifted climate changed no biome: the tiles are not read");
}

/// The biomes of a chunk whose tile an earlier batch left compute less than half of the climate
/// they compute from scratch.
#[test]
fn biomes_compute_less_over_earlier_tiles() {
    let Some(pack) = common::data_pack() else { return };
    use rc_worldgen::sampler::{count_evaluations, take_eval_counts};
    // Cache values computed by the biomes of the chunks a second batch adds next to the first.
    // One thread: the counters are per thread and the batch hands its work to the calling pool.
    let one_thread = rayon::ThreadPoolBuilder::new().num_threads(1).build().expect("pool");
    let climate_computed = |opts: TerrainOpts| {
        let t = generator(&pack, "minecraft:normal", "minecraft:overworld", 0, opts);
        one_thread.install(|| {
            let mut cache = BiomeCache::default();
            t.prepare_biomes(&mut cache, &BLOCK_AT_ORIGIN);
            let (chunks, tiles) = (cache.chunks.len(), cache.surface_tiles.len());
            count_evaluations(true);
            take_eval_counts();
            // The chunk two rows north reads the same tiles: only the biomes of 6 more chunks are
            // computed (2 rows of 3).
            t.prepare_biomes(&mut cache, &[(0, -2)]);
            let counts = take_eval_counts();
            count_evaluations(false);
            assert_eq!(
                (cache.chunks.len() - chunks, cache.surface_tiles.len() - tiles),
                (6, 0),
                "computed by the second batch"
            );
            counts.cache_volume_values
        })
    };
    let (over_tiles, from_scratch) = (climate_computed(TILES_ON), climate_computed(TILES_OFF));
    eprintln!("cache values computed by the biomes of 6 chunks: {over_tiles} over tiles, {from_scratch} from scratch");
    assert!(over_tiles * 2 < from_scratch, "{over_tiles} cache values over tiles, {from_scratch} from scratch");
    assert_eq!(
        climate_computed(TILES_WITHOUT_CLIMATE),
        from_scratch,
        "tiles without climate change nothing for the biomes"
    );
}

/// With the tiles, creating the aquifer of a prepared chunk evaluates nothing and its filling
/// never searches the top surface; without them the creation probes it over its 11 × 11 quarts.
/// (Proof that the tests above compare two paths.)
#[test]
fn prepared_tiles_replace_the_window_volume() {
    let Some(pack) = common::data_pack() else { return };
    use rc_worldgen::sampler::{count_evaluations, take_eval_counts};
    // Counts of the aquifer's creation alone, then of the whole filling, of one prepared chunk.
    let counts = |opts: TerrainOpts| {
        let t = generator(&pack, "minecraft:normal", "minecraft:overworld", 0, opts);
        let mut cache = BiomeCache::default();
        t.prepare_biomes(&mut cache, &BLOCK_AT_ORIGIN);
        let vol = t.fill_volume(1, 0).expect("chunk volume");
        count_evaluations(true);
        take_eval_counts();
        std::hint::black_box(t.new_chunk_aquifer(&mut Ctx::cached(), &vol, &cache));
        let creation = take_eval_counts();
        std::hint::black_box(t.generate_until(1, 0, &cache, None, Stage::Fill));
        let filling = take_eval_counts();
        count_evaluations(false);
        (creation, filling)
    };
    let (creation, filling) = counts(TILES_ON);
    assert_eq!(
        (creation.top_probes, creation.cache_volume_values, creation.point_noises),
        (0, 0, 0),
        "with tiles, at creation: (top probes, cache values computed, point noises)"
    );
    assert_eq!(filling.top_probes, 0, "with tiles: every surface level of the filling comes from the tiles");
    let (own_creation, own_filling) = counts(TILES_OFF);
    assert_eq!(own_creation.top_probes, 121, "without tiles: one top probe per quart of the window");
    assert!(own_creation.cache_volume_values > 0 && own_filling.top_probes >= 121);
    // With the tiles' climate the final density finds its corners in the caches, as after the
    // chunk's own window: the filling computes what the old one computes once its window is done.
    assert_eq!(
        filling.cache_volume_values,
        own_filling.cache_volume_values - own_creation.cache_volume_values,
        "cache values computed by the filling"
    );
    let (_, levels_only) = counts(TILES_WITHOUT_CLIMATE);
    assert!(levels_only.cache_volume_values > filling.cache_volume_values, "the climate saves no cache value");
}
