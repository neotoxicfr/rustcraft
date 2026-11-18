//! Equivalence proofs of the beard paths (column lists, column buffer, kernel tables, parallel
//! rows) against the point-by-point path, bit for bit, on real structures and on random
//! synthetic elements placed on the reach bounds. Game data required for the real structures
//! (jar 26.3 and rc-data cache); absent: those tests are skipped, except
//! `RUSTCRAFT_OPT_REQUIRE=1` (explicit failure).

use std::time::Instant;

use crate::chunkgen::DimGen;
use crate::jigsaw::{Junction, Projection};
use crate::structure::tests::{dim, world_data};
use crate::structure::{Adapt, Start};
use crate::template::BBox;
use crate::terrain::{BiomeCache, StructureDensity};
use crate::volume::Volume;

use super::{Beard, BeardPaths, Rigid, junction_reach, junction_term};

/// Seed of the main test world and the chunk of its village at (-1408, 368).
const SEED: i64 = 20261011;
const VILLAGE_CHUNK: (i32, i32) = (-88, 23);

/// Structures generated at chunk (10, 10) whatever the biome, one per terrain adaptation.
const PLACED: [&str; 4] =
    ["minecraft:ancient_city", "minecraft:stronghold", "minecraft:trail_ruins", "minecraft:trial_chambers"];

/// Expected fingerprint per scene, taken with the point-by-point path before the column paths
/// existed: (scene, chunks with a beard, points changed, FNV-1a of the volume bits).
const EXPECTED: [(&str, usize, usize, u64); 5] = [
    ("village", 24, 194077, 0xddb1c59a6bb80663),
    ("minecraft:ancient_city", 12, 155604, 0xfd930c771a749143),
    ("minecraft:stronghold", 12, 78495, 0x2e712d466ecf2cf2),
    ("minecraft:trail_ruins", 12, 43728, 0x1cd729619ac07953),
    ("minecraft:trial_chambers", 12, 157885, 0x5803ba116f7b981f),
];

pub(super) struct Rng(u64);

impl Rng {
    pub(super) fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    pub(super) fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Integer in `lo..=hi`.
    pub(super) fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next() % (hi - lo + 1) as u64) as i32
    }
}

/// Density volume before the beard: values in [-2, 2] with exact +0.0 and -0.0 sprinkled in, so
/// that a wrong summation order, a missing term and a lost `+= 0.0` all change some bits.
pub(super) fn prefilled_density(len: usize, seed: u64) -> Vec<f32> {
    let mut rng = Rng::new(seed);
    (0..len)
        .map(|i| match i % 41 {
            0 => -0.0,
            1 => 0.0,
            _ => (rng.next() >> 40) as f32 / (1u64 << 22) as f32 - 2.0,
        })
        .collect()
}

pub(super) fn fnv_bits(h: &mut u64, values: &[f32]) {
    for v in values {
        for b in v.to_bits().to_le_bytes() {
            *h ^= u64::from(b);
            *h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

/// A chunk of a scene: its references, its beard and its filling volume.
pub(super) struct BeardChunk {
    pub(super) cx: i32,
    pub(super) cz: i32,
    pub(super) refs: Vec<(usize, Vec<Start>)>,
    pub(super) beard: Beard,
    pub(super) volume: Volume,
}

fn fill_volume(dg: &DimGen, cx: i32, cz: i32) -> Volume {
    dg.terrain_generator().and_then(|t| t.fill_volume(cx, cz)).expect("noise world")
}

/// Chunks of the square of side `2 r + 1` around `center` whose references give a beard.
pub(super) fn referenced_chunks(dg: &DimGen, center: (i32, i32), r: i32) -> Vec<BeardChunk> {
    let sg = dg.structures.as_ref().expect("structures");
    let t = dg.structure_terrain().expect("terrain");
    let mut out = Vec::new();
    for cx in center.0 - r..=center.0 + r {
        for cz in center.1 - r..=center.1 + r {
            let refs = sg.referenced(cx, cz, t, &dg.blocks);
            if let Some(beard) = Beard::new(cx, cz, &refs, &sg.defs) {
                out.push(BeardChunk { cx, cz, refs, beard, volume: fill_volume(dg, cx, cz) });
            }
        }
    }
    out
}

/// Structure `id` started at chunk (10, 10); its `count` chunks nearest to the start chunk
/// (within the reference box) that get a beard.
pub(super) fn placed_chunks(dg: &DimGen, id: &str, count: usize) -> (Start, Vec<BeardChunk>) {
    let sg = dg.structures.as_ref().expect("structures");
    let start = dg.structure_start_anywhere(id, 10, 10).unwrap_or_else(|| panic!("{id}: no start at (10, 10)"));
    let refs = vec![(start.structure, vec![start.clone()])];
    let b = start.ref_box(&sg.defs);
    let mut chunks: Vec<(i32, i32)> = (b.min[0] >> 4..=b.max[0] >> 4)
        .flat_map(|cx| (b.min[2] >> 4..=b.max[2] >> 4).map(move |cz| (cx, cz)))
        .collect();
    chunks.sort_by_key(|&(cx, cz)| ((cx - 10).pow(2) + (cz - 10).pow(2), cx, cz));
    let out = chunks
        .into_iter()
        .filter_map(|(cx, cz)| {
            let beard = Beard::new(cx, cz, &refs, &sg.defs)?;
            Some(BeardChunk { cx, cz, refs: refs.clone(), beard, volume: fill_volume(dg, cx, cz) })
        })
        .take(count)
        .collect();
    (start, out)
}

/// `paths` over every chunk of a scene: (points changed, fingerprint).
fn scene_fingerprint(chunks: &[BeardChunk], paths: BeardPaths) -> (usize, u64) {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut changed = 0;
    for c in chunks {
        let before = prefilled_density(volume_len(&c.volume), chunk_seed(c));
        let mut out = before.clone();
        c.beard.add_to_volume_with(&mut out, &c.volume, paths);
        changed += out.iter().zip(&before).filter(|(a, b)| a.to_bits() != b.to_bits()).count();
        fnv_bits(&mut h, &out);
    }
    (changed, h)
}

pub(super) fn volume_len(v: &Volume) -> usize {
    (v.sx * v.sy * v.sz) as usize
}

pub(super) fn chunk_seed(c: &BeardChunk) -> u64 {
    (i64::from(c.cx) * 31 + i64::from(c.cz)) as u64
}

/// Scenes of the real-structure tests: the village of the test world and one placed structure per
/// terrain adaptation.
pub(super) fn scenes(dg: &DimGen, placed_count: usize) -> Vec<(String, Vec<BeardChunk>)> {
    let mut v = vec![("village".to_owned(), referenced_chunks(dg, VILLAGE_CHUNK, 2))];
    for id in PLACED {
        v.push((id.to_owned(), placed_chunks(dg, id, placed_count).1));
    }
    v
}

/// Every combination of switches that takes a column path.
fn column_paths() -> Vec<BeardPaths> {
    (0..16u8)
        .map(|bits| BeardPaths {
            column_lists: bits & 1 != 0,
            column_buffer: bits & 2 != 0,
            kernel_tables: bits & 4 != 0,
            parallel: bits & 8 != 0,
        })
        .filter(|p| p.column_lists || p.column_buffer)
        .collect()
}

/// Every column path gives the point-by-point volume bit for bit (rows forced onto the threads
/// when `parallel` is on); the message names the first difference.
fn assert_paths_match(beard: &Beard, v: &Volume, seed: u64, what: &str) {
    let before = prefilled_density(volume_len(v), seed);
    let mut reference = before.clone();
    beard.add_with_parallel_threshold(&mut reference, v, BeardPaths::OLD, 0);
    for paths in column_paths() {
        let mut out = before.clone();
        beard.add_with_parallel_threshold(&mut out, v, paths, 0);
        if let Some(i) = (0..out.len()).find(|&i| out[i].to_bits() != reference[i].to_bits()) {
            let (iy, column) = (i as i32 % v.sy, i as i32 / v.sy);
            panic!(
                "{what}: {paths:?} gives {:e} instead of {:e} at x {}, y {}, z {}",
                out[i],
                reference[i],
                v.bx(column % v.sx),
                v.by(iy),
                v.bz(column / v.sx)
            );
        }
    }
}

/// Adaptations met among the rigid pieces of the chunks, and junction count.
fn coverage(chunks: &[BeardChunk]) -> (Vec<Adapt>, usize) {
    let mut adapts = Vec::new();
    for r in chunks.iter().flat_map(|c| &c.beard.rigids) {
        if !adapts.contains(&r.a) {
            adapts.push(r.a);
        }
    }
    (adapts, chunks.iter().map(|c| c.beard.junctions.len()).sum())
}

/// Stored fingerprints of the old path, and of the paths selected by the environment (default:
/// every new path); every adaptation and junctions are met.
#[test]
fn beard_fingerprints_match_the_point_by_point_path() {
    let Some(w) = world_data() else { return };
    let dg = dim(w, Some(SEED));
    let scenes = scenes(&dg, 12);
    let mut adapts = Vec::new();
    let mut junctions = 0;
    let mut bad = Vec::new();
    for ((name, chunks), want) in scenes.iter().zip(EXPECTED) {
        assert_eq!(name, want.0);
        let (a, j) = coverage(chunks);
        adapts.extend(a);
        junctions += j;
        for paths in [BeardPaths::OLD, BeardPaths::current()] {
            let (changed, h) = scene_fingerprint(chunks, paths);
            eprintln!("{paths:?}: (\"{name}\", {}, {changed}, {h:#018x}),", chunks.len());
            if (chunks.len(), changed, h) != (want.1, want.2, want.3) {
                bad.push(format!("{name} {paths:?}"));
            }
        }
    }
    for a in [Adapt::Bury, Adapt::BeardThin, Adapt::BeardBox, Adapt::Encapsulate] {
        assert!(adapts.contains(&a), "{a:?} not met by the scenes");
    }
    assert!(junctions > 0, "no junction in the scenes");
    assert!(bad.is_empty(), "beard fingerprint changed for {bad:?}");
}

/// Real structures (village of the test world; ancient city, stronghold, trail ruins and trial
/// chambers placed): every column path against the point-by-point path, chunk by chunk.
#[test]
fn beard_paths_match_bit_for_bit() {
    let Some(w) = world_data() else { return };
    let dg = dim(w, Some(SEED));
    for (name, chunks) in scenes(&dg, 12) {
        for c in &chunks {
            assert_paths_match(&c.beard, &c.volume, chunk_seed(c), &format!("{name} chunk ({}, {})", c.cx, c.cz));
        }
    }
}

const ADAPTS: [Adapt; 5] = [Adapt::None, Adapt::Bury, Adapt::BeardThin, Adapt::BeardBox, Adapt::Encapsulate];

fn random_rigid(rng: &mut Rng, x0: i32, y0: i32, z0: i32, height: i32) -> Rigid {
    let min = [rng.range(x0 - 20, x0 + 32), rng.range(y0 - 14, y0 + height + 10), rng.range(z0 - 20, z0 + 32)];
    let max = [min[0] + rng.range(0, 6), min[1] + rng.range(0, 6), min[2] + rng.range(0, 6)];
    Rigid { b: BBox { min, max }, a: ADAPTS[rng.range(0, 4) as usize], delta: rng.range(-8, 8) }
}

fn random_junction(rng: &mut Rng, x0: i32, y0: i32, z0: i32, height: i32) -> Junction {
    Junction {
        x: rng.range(x0 - 14, x0 + 29),
        ground_y: rng.range(y0 - 14, y0 + height + 13),
        z: rng.range(z0 - 14, z0 + 29),
        dy: 0,
        projection: Projection::TerrainMatching,
    }
}

/// Elements scattered across the edges of the chunk at (x0, z0) and of the y range, each reach
/// bound falling inside or just outside the volume; the zone is the elements' box inflated by 0 to
/// 24 (it may cut through the reaches).
fn random_beard(rng: &mut Rng, x0: i32, y0: i32, z0: i32, height: i32) -> Beard {
    let rigids: Vec<Rigid> = (0..rng.range(0, 6)).map(|_| random_rigid(rng, x0, y0, z0, height)).collect();
    let junctions: Vec<Junction> = (0..rng.range(0, 8)).map(|_| random_junction(rng, x0, y0, z0, height)).collect();
    let boxes = rigids.iter().map(|r| r.b).chain(junctions.iter().map(|j| {
        let p = [j.x, j.ground_y, j.z];
        BBox::new(p, p)
    }));
    let zone = boxes.reduce(|mut a, b| {
        a.encapsulate(&b);
        a
    });
    let inflate = rng.range(0, 24);
    Beard { rigids, junctions, zone: zone.map(|z| z.inflated(inflate)) }
}

/// Random elements of every adaptation on the reach bounds and the chunk edges, unit-step volume
/// and a stepped one (point-by-point path kept there).
#[test]
fn beard_paths_match_on_synthetic_bounds() {
    let mut rng = Rng::new(0xbea7d);
    for case in 0..600 {
        let (x0, z0, y0) = (16 * rng.range(-3, 3), 16 * rng.range(-3, 3), rng.range(-70, 60));
        let beard = random_beard(&mut rng, x0, y0, z0, 40);
        let volume = Volume::new(16, 40, 16, x0, y0, z0);
        assert_paths_match(&beard, &volume, case, &format!("synthetic case {case}"));
        let stepped = Volume::stepped(5, 11, 5, x0, y0, z0, 4, 4, 4);
        assert_paths_match(&beard, &stepped, case, &format!("synthetic stepped case {case}"));
    }
}

/// The reach of every element kind is exact: its term is +0.0 (bits) everywhere outside, and
/// non-zero somewhere on each of the six faces.
#[test]
fn reach_is_exact_for_every_adaptation() {
    let mut rng = Rng::new(7);
    for case in 0..200 {
        let a = ADAPTS[1 + case % 4];
        let min = [rng.range(-4, 4), rng.range(-4, 4), rng.range(-4, 4)];
        let max = [min[0] + rng.range(0, 5), min[1] + rng.range(0, 5), min[2] + rng.range(0, 5)];
        let rigid = Rigid { b: BBox { min, max }, a, delta: rng.range(-3, 3) };
        let reach = rigid.reach().expect("adapted piece");
        check_reach(&format!("{a:?} {min:?} {max:?} delta {}", rigid.delta), reach, |x, y, z| rigid.term(x, y, z));
        let j = Junction { x: min[0], ground_y: min[1], z: min[2], dy: 0, projection: Projection::TerrainMatching };
        check_reach(&format!("junction {min:?}"), junction_reach(&j), |x, y, z| junction_term(&j, x, y, z));
    }
    let none = Rigid { b: BBox { min: [0; 3], max: [3; 3] }, a: Adapt::None, delta: 0 };
    assert!(none.reach().is_none());
}

fn check_reach(what: &str, reach: BBox, term: impl Fn(i32, i32, i32) -> f32) {
    let mut faces = [false; 6];
    for x in reach.min[0] - 3..=reach.max[0] + 3 {
        for y in reach.min[1] - 3..=reach.max[1] + 3 {
            for z in reach.min[2] - 3..=reach.max[2] + 3 {
                let t = term(x, y, z);
                if !reach.contains([x, y, z]) {
                    assert_eq!(t.to_bits(), 0, "{what}: term {t:e} at ({x}, {y}, {z}) outside the reach {reach:?}");
                    continue;
                }
                assert!(t.to_bits() != (-0.0f32).to_bits(), "{what}: -0.0 term at ({x}, {y}, {z})");
                if t != 0.0 {
                    let p = [x, y, z];
                    for axis in 0..3 {
                        faces[axis] |= p[axis] == reach.min[axis];
                        faces[axis + 3] |= p[axis] == reach.max[axis];
                    }
                }
            }
        }
    }
    assert!(faces.iter().all(|&f| f), "{what}: reach {reach:?} wider than the terms (faces met {faces:?})");
}

/// A beard behind given paths, for the terrain bench.
struct BeardWithPaths<'a>(&'a Beard, BeardPaths);

impl StructureDensity for BeardWithPaths<'_> {
    fn add_to_volume(&self, out: &mut [f32], v: &Volume) {
        self.0.add_to_volume_with(out, v, self.1);
    }
}

/// Best of `passes` timings of `f`, in ms.
fn best_ms(passes: usize, mut f: impl FnMut()) -> f64 {
    (0..passes)
        .map(|_| {
            let t = Instant::now();
            f();
            t.elapsed().as_secs_f64() * 1e3
        })
        .fold(f64::INFINITY, f64::min)
}

fn pool(threads: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("thread pool")
}

/// Thread counts of the bench: 1, then powers of 2, then every logical core.
fn thread_counts() -> Vec<usize> {
    let n = std::thread::available_parallelism().map_or(1, usize::from);
    let mut v: Vec<usize> = (0..).map(|k| 1 << k).take_while(|&t| t < n).collect();
    v.push(n);
    v
}

fn add_all(chunks: &[BeardChunk], buffers: &mut [Vec<f32>], paths: BeardPaths) {
    for (c, out) in chunks.iter().zip(buffers.iter_mut()) {
        c.beard.add_to_volume_with(out, &c.volume, paths);
    }
}

fn bench_scene(name: &str, chunks: &[BeardChunk], defs: &[crate::structure::StructureDef]) {
    let elements: usize = chunks.iter().map(|c| c.beard.rigids.len() + c.beard.junctions.len()).sum();
    eprintln!("{name}: {} chunks, {:.0} elements per chunk", chunks.len(), elements as f64 / chunks.len() as f64);
    let per_chunk = |ms: f64| ms / chunks.len() as f64;
    let mut buffers: Vec<Vec<f32>> = chunks.iter().map(|c| vec![0f32; volume_len(&c.volume)]).collect();
    let new = BeardPaths { column_lists: true, column_buffer: true, kernel_tables: true, parallel: false };
    let configs = [
        ("old (point by point)", BeardPaths::OLD),
        ("column lists", BeardPaths { column_lists: true, ..BeardPaths::OLD }),
        ("column buffer", BeardPaths { column_buffer: true, ..BeardPaths::OLD }),
        ("lists + buffer", BeardPaths { column_lists: true, column_buffer: true, ..BeardPaths::OLD }),
        ("buffer + tables", BeardPaths { column_buffer: true, kernel_tables: true, ..BeardPaths::OLD }),
        ("lists + buffer + tables", new),
    ];
    pool(1).install(|| {
        let ms = best_ms(5, || {
            for c in chunks {
                std::hint::black_box(Beard::new(c.cx, c.cz, &c.refs, defs));
            }
        });
        eprintln!("  1 thread, Beard::new: {:.4} ms per chunk", per_chunk(ms));
        for pass in 0..3 {
            for (label, paths) in configs {
                let ms = best_ms(9, || add_all(chunks, &mut buffers, paths));
                eprintln!("  1 thread, pass {pass}, {label}: {:.4} ms per chunk", per_chunk(ms));
            }
        }
    });
    for threads in thread_counts() {
        pool(threads).install(|| {
            use rayon::prelude::*;
            let rows = BeardPaths { parallel: true, ..new };
            for (label, paths) in [("old", BeardPaths::OLD), ("new", new), ("new, rows in parallel", rows)] {
                let ms = best_ms(5, || {
                    chunks.par_iter().zip(buffers.par_iter_mut()).for_each(|(c, out)| {
                        c.beard.add_to_volume_with(out, &c.volume, paths);
                    });
                });
                eprintln!("  {threads} threads, chunks in parallel, {label}: {:.4} ms per chunk", per_chunk(ms));
            }
            for (label, threshold) in
                [("rows in parallel above the threshold", super::PARALLEL_MIN_WORK), ("rows always in parallel", 0)]
            {
                let paths = BeardPaths { parallel: true, ..new };
                let ms = best_ms(5, || {
                    for (c, out) in chunks.iter().zip(buffers.iter_mut()) {
                        c.beard.add_with_parallel_threshold(out, &c.volume, paths, threshold);
                    }
                });
                eprintln!("  {threads} threads, one chunk at a time, {label}: {:.4} ms per chunk", per_chunk(ms));
            }
        });
    }
    let work: Vec<usize> = chunks
        .iter()
        .filter_map(|c| {
            let zone = c.beard.zone?;
            Some(super::ColumnPass::new(&c.beard, &zone, &c.volume, new)?.work())
        })
        .collect();
    // The chunks with the most elements, where listing them per column should matter most.
    let mut heavy: Vec<&BeardChunk> = chunks.iter().collect();
    heavy.sort_by_key(|c| std::cmp::Reverse(c.beard.rigids.len() + c.beard.junctions.len()));
    heavy.truncate(4);
    let counts: Vec<usize> = heavy.iter().map(|c| c.beard.rigids.len() + c.beard.junctions.len()).collect();
    let mut out = vec![0f32; heavy.first().map_or(0, |c| volume_len(&c.volume))];
    pool(1).install(|| {
        for pass in 0..2 {
            for (label, lists) in [("without lists", false), ("with lists", true)] {
                let paths = BeardPaths { column_lists: lists, ..new };
                let ms =
                    best_ms(15, || heavy.iter().for_each(|c| c.beard.add_to_volume_with(&mut out, &c.volume, paths)));
                eprintln!(
                    "  heaviest chunks {counts:?}, pass {pass}, buffer + tables {label}: {:.4} ms per chunk",
                    ms / 4.0
                );
            }
        }
    });
    let above = work.iter().filter(|&&w| w >= super::PARALLEL_MIN_WORK).count();
    let total: usize = work.iter().sum();
    let ms = pool(1).install(|| best_ms(5, || add_all(chunks, &mut buffers, new)));
    eprintln!(
        "  column slots per chunk: mean {}, max {:?}, {above} chunks above the threshold; {:.2} ns per slot (1 thread)",
        total / work.len().max(1),
        work.iter().max(),
        ms * 1e6 / total.max(1) as f64
    );
}

/// Share of the beard in the terrain of the village chunks (1 thread), old path against new.
fn bench_terrain_share(dg: &DimGen, chunks: &[BeardChunk]) {
    let t = dg.terrain_generator().expect("noise world");
    let mut cache = BiomeCache::default();
    let targets: Vec<(i32, i32)> = chunks.iter().map(|c| (c.cx, c.cz)).collect();
    t.prepare_biomes(&mut cache, &targets);
    pool(1).install(|| {
        for pass in 0..3 {
            for (label, paths) in [("old", BeardPaths::OLD), ("new", BeardPaths::current())] {
                let ms = best_ms(3, || {
                    for c in chunks {
                        let beard = BeardWithPaths(&c.beard, paths);
                        std::hint::black_box(t.generate(c.cx, c.cz, &cache, Some(&beard as &dyn StructureDensity)));
                    }
                });
                eprintln!("village terrain, pass {pass}, beard {label}: {:.3} ms per chunk", ms / chunks.len() as f64);
            }
        }
    });
}

/// Micro-bench: `cargo test --release -p rc-worldgen --lib bench_beard -- --ignored --nocapture`.
/// Chunks of the test world's village (up to 64), 32 of each placed structure, best of 5.
#[test]
#[ignore = "benchmark"]
fn bench_beard() {
    let Some(w) = world_data() else { return };
    let dg = dim(w, Some(SEED));
    let defs = &dg.structures.as_ref().expect("structures").defs;
    let mut village = referenced_chunks(&dg, VILLAGE_CHUNK, 4);
    village.truncate(64);
    bench_scene("village", &village, defs);
    for id in PLACED {
        bench_scene(id, &placed_chunks(&dg, id, 32).1, defs);
    }
    bench_terrain_share(&dg, &village);
}
