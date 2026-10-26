// Direct read allowed (tests and benches: settings read at launch, outside the measured path); the game goes through rc_data::env_*!.
#![allow(clippy::disallowed_methods)]
//! Equivalence proofs of the concentric rings and the start cache (C1-M01).
//! Game data required (jar 26.3 and rc-data cache); absent: tests skipped, except
//! `RUSTCRAFT_OPT_REQUIRE=1` (explicit failure). The expected fingerprints were taken with the
//! code from before laziness (read / release / compute / insert pattern).

use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use crate::chunkgen::{DimGen, FlatSpec, GenSpec, SourceSpec, WorldData};
use crate::structure::{Placement, StructureGen};
use crate::terrain::TerrainGenerator;

fn required() -> bool {
    std::env::var("RUSTCRAFT_OPT_REQUIRE").is_ok_and(|v| v == "1")
}

pub(crate) fn world_data() -> Option<&'static WorldData> {
    static W: OnceLock<Option<WorldData>> = OnceLock::new();
    let w = W
        .get_or_init(|| {
            let mc = std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join(".minecraft"))?;
            let jar = mc.join("versions").join("26.3").join("26.3.jar");
            if !jar.is_file() {
                return None;
            }
            let v = rc_data::load_cached("26.3")?;
            let reports = rc_data::cache_root().join("26.3").join("generated").join("reports");
            WorldData::load(&jar, Some(&reports), &v).ok()
        })
        .as_ref();
    if w.is_none() {
        assert!(!required(), "game data missing (RUSTCRAFT_OPT_REQUIRE=1)");
        eprintln!("game data missing: test skipped");
    }
    w
}

/// Seeds of the main world (noise) and classic flat world (ring seed zero).
const SEEDS: [i64; 4] = [20261011, 1, 42, -4172144997902289642];

pub(crate) fn dim(w: &WorldData, seed: Option<i64>) -> DimGen {
    let (spec, s) = match seed {
        Some(s) => (
            GenSpec::Noise {
                settings: "minecraft:overworld".into(),
                source: SourceSpec::MultiNoise("minecraft:overworld".into()),
            },
            s,
        ),
        None => (GenSpec::Flat(FlatSpec::classic()), 0),
    };
    let mut dg = DimGen::new(w, &spec, "minecraft:overworld", s).unwrap();
    dg.enable_structures(w).unwrap();
    dg
}

fn parts(dg: &DimGen) -> (Arc<StructureGen>, &TerrainGenerator, usize) {
    let sg = dg.structures.clone().expect("structures");
    let t = dg.structure_terrain().expect("terrain");
    let set = sg.sets.iter().position(|s| matches!(s.placement, Placement::Rings { .. })).expect("ring set");
    (sg, t, set)
}

fn fnv(v: impl IntoIterator<Item = i32>) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for x in v {
        for b in x.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    }
    h
}

fn ring_digest(r: &[(i32, i32)]) -> u64 {
    fnv(r.iter().flat_map(|&(x, z)| [x, z]))
}

/// Chunks (cx, cz) of the 600 × 600 square around the origin where the ring set says yes.
fn sweep(sg: &StructureGen, set: usize, t: &TerrainGenerator, lazy: bool) -> Vec<(i32, i32)> {
    let mut v = Vec::new();
    for cx in -300..300 {
        for cz in -300..300 {
            if sg.is_structure_chunk_with(set, cx, cz, t, lazy) {
                v.push((cx, cz));
            }
        }
    }
    v
}

/// (seed or None = flat, fingerprint of the 128 positions, count, fingerprint of the sweep, count).
const EXPECTED: [(Option<i64>, u64, usize, u64, usize); 5] = [
    (Some(20261011), 0x2dbe9552a6e8b65a, 128, 0x91ea81ba946fc2ec, 6),
    (Some(1), 0xcda75a78bc3c58c5, 128, 0xb59540c4cfa18305, 6),
    (Some(42), 0x615091db1c9ded44, 128, 0x080d62c958389bb5, 7),
    (Some(-4172144997902289642), 0xc866e471e7b178e7, 128, 0x03566e31b15e62b9, 7),
    (None, 0x7ed58547ac941c28, 128, 0x6b6dae48738ef09d, 6),
];

/// Ring positions and 600 × 600 sweep = values taken with the old code, through the
/// two paths (lazy: sweep first, strongholds resolved one by one, then full computation).
#[test]
fn rings_match_stored_values() {
    let Some(w) = world_data() else { return };
    for lazy in [false, true] {
        let mut got = Vec::new();
        for &(seed, ..) in &EXPECTED {
            let dg = dim(w, seed);
            let (sg, t, set) = parts(&dg);
            let s = sweep(&sg, set, t, lazy);
            let r = if lazy { sg.ring_positions_lazy(set, t) } else { sg.ring_positions_old(set, t) };
            got.push((seed, ring_digest(&r), r.len(), ring_digest(&s), s.len()));
        }
        for g in &got {
            eprintln!("    ({:?}, 0x{:016x}, {}, 0x{:016x}, {}),", g.0, g.1, g.2, g.3, g.4);
        }
        assert_eq!(got.as_slice(), EXPECTED.as_slice(), "rings differ from the stored values (laziness {lazy})");
    }
}

/// Window of 31 × 31 chunks around each draw (more than twice the bound): the two
/// paths say the same everywhere; each position stays within RING_REACH of its draw.
#[test]
fn rings_lazy_matches_old_around_every_job() {
    let Some(w) = world_data() else { return };
    for seed in SEEDS.iter().copied().map(Some).chain([None]) {
        let dg = dim(w, seed);
        let (sg, t, set) = parts(&dg);
        let Placement::Rings { jobs, .. } = &sg.sets[set].placement else { unreachable!() };
        let full = sg.ring_positions_old(set, t);
        let (mut dup, mut fallback, mut far) = (0, 0, 0u32);
        for (i, &(x0, z0, _)) in jobs.jobs().iter().enumerate() {
            let (x, z) = full[i];
            far = far.max(x.abs_diff(x0)).max(z.abs_diff(z0));
            fallback += usize::from((x, z) == (x0, z0));
            dup += usize::from(full[..i].contains(&(x, z)));
            for cx in x0 - 15..=x0 + 15 {
                for cz in z0 - 15..=z0 + 15 {
                    assert_eq!(
                        sg.is_structure_chunk_with(set, cx, cz, t, true),
                        sg.is_structure_chunk_with(set, cx, cz, t, false),
                        "seed {seed:?}, stronghold {i}, chunk ({cx}, {cz})"
                    );
                }
            }
        }
        assert!(far <= crate::structure::RING_REACH, "seed {seed:?}: difference {far}");
        eprintln!("seed {seed:?}: max difference {far}, positions at the draw {fallback}, duplicates {dup}");
    }
}

/// No preferred biome: each search fails, fallback on the drawn chunk, same answers.
#[test]
fn rings_fallback_without_preferred_biome() {
    let Some(w) = world_data() else { return };
    let mut dg = dim(w, Some(20261011));
    let sg = Arc::get_mut(dg.structures.as_mut().unwrap()).unwrap();
    let set = sg.sets.iter().position(|s| matches!(s.placement, Placement::Rings { .. })).unwrap();
    if let Placement::Rings { preferred, .. } = &mut sg.sets[set].placement {
        preferred.iter_mut().for_each(|b| *b = false);
    }
    let (sg, t, set) = parts(&dg);
    let Placement::Rings { jobs, .. } = &sg.sets[set].placement else { unreachable!() };
    let drawn: Vec<(i32, i32)> = jobs.jobs().iter().map(|&(x, z, _)| (x, z)).collect();
    assert_eq!(*sg.ring_positions_old(set, t), drawn);
    for &(x, z) in &drawn {
        assert!(sg.is_structure_chunk_with(set, x, z, t, true));
    }
    assert_eq!(*sg.ring_positions_lazy(set, t), drawn);
}

/// Construction: no noise computation; parallel generation of 1,000 chunks around
/// the spawn then of chunks near strongholds: each stronghold searched only once.
#[test]
fn rings_lazy_one_search_per_job() {
    let Some(w) = world_data() else { return };
    let dg = dim(w, Some(20261011));
    let (sg, t, set) = parts(&dg);
    assert_eq!(sg.ring_searches(), 0, "search at construction");
    let pool = rayon::ThreadPoolBuilder::new().num_threads(32).build().unwrap();
    let spawn: Vec<(i32, i32)> = (-16..16).flat_map(|x| (-16..16).map(move |z| (x, z))).take(1000).collect();
    pool.install(|| {
        use rayon::prelude::*;
        spawn.par_iter().for_each(|&(x, z)| {
            sg.is_structure_chunk_with(set, x, z, t, true);
        });
    });
    // No stronghold falls within 7 chunks of a 32 square around the origin.
    assert_eq!(sg.ring_searches(), 0, "searches launched far from any stronghold");
    let Placement::Rings { jobs, .. } = &sg.sets[set].placement else { unreachable!() };
    let near: Vec<(i32, i32)> = jobs.jobs()[..3]
        .iter()
        .flat_map(|&(x0, z0, _)| (-8..8).flat_map(move |dx| (-8..8).map(move |dz| (x0 + dx, z0 + dz))))
        .collect();
    pool.install(|| {
        use rayon::prelude::*;
        near.par_iter().for_each(|&(x, z)| {
            sg.is_structure_chunk_with(set, x, z, t, true);
        });
    });
    assert_eq!(sg.ring_searches(), 3, "searches for 3 neighbouring strongholds");
    pool.install(|| {
        use rayon::prelude::*;
        (0..64).into_par_iter().for_each(|_| {
            sg.ring_positions_lazy(set, t);
        });
    });
    assert_eq!(sg.ring_searches(), 128, "a single computation per stronghold");
}

/// Reentrancy: full computation launched from 32 pool tasks while other tasks ask
/// the question chunk by chunk; must finish (300 s safeguard).
#[test]
fn rings_lazy_reentrant_in_pool() {
    let Some(w) = world_data() else { return };
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let dg = dim(w, Some(42));
        let (sg, t, set) = parts(&dg);
        let Placement::Rings { jobs, .. } = &sg.sets[set].placement else { unreachable!() };
        let pool = rayon::ThreadPoolBuilder::new().num_threads(32).build().unwrap();
        let out: Vec<usize> = pool.install(|| {
            use rayon::prelude::*;
            (0..64usize)
                .into_par_iter()
                .map(|i| {
                    if i % 2 == 0 {
                        sg.ring_positions_lazy(set, t).len()
                    } else {
                        let (x0, z0, _) = jobs.jobs()[i];
                        (x0 - 7..=x0 + 7)
                            .flat_map(|x| (z0 - 7..=z0 + 7).map(move |z| (x, z)))
                            .filter(|&(x, z)| sg.is_structure_chunk_with(set, x, z, t, true))
                            .count()
                    }
                })
                .collect()
        });
        let _ = tx.send((out, sg.ring_searches()));
    });
    let (out, searches) = rx.recv_timeout(std::time::Duration::from_secs(300)).expect("ring deadlock");
    assert!(out.iter().step_by(2).all(|&n| n == 128));
    assert!(out.iter().skip(1).step_by(2).all(|&n| n >= 1));
    assert_eq!(searches, 128);
}

/// Micro-bench: disk compound of the structures (starts + references) of 1,000 chunks around
/// the spawn, new world, at 1, 2, 4… threads up to the number of logical cores. Compare
/// `RUSTCRAFT_OPT_RINGS_LAZY=0` and `=1` (read once per process):
/// `cargo test --release -p rc-worldgen --lib structure::tests::bench_rings -- --ignored --nocapture`.
#[test]
#[ignore = "benchmark"]
fn bench_rings_first_chunks() {
    use rayon::prelude::*;
    let Some(w) = world_data() else { return };
    let max = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut counts: Vec<usize> = std::iter::successors(Some(1usize), |n| Some(n * 2)).take_while(|&n| n <= max).collect();
    if counts.last() != Some(&max) {
        counts.push(max);
    }
    let chunks: Vec<(i32, i32)> = (-16..16).flat_map(|x| (-16..16).map(move |z| (x, z))).take(1000).collect();
    eprintln!("RUSTCRAFT_OPT_RINGS_LAZY={}", std::env::var("RUSTCRAFT_OPT_RINGS_LAZY").unwrap_or_else(|_| "default".into()));
    for &n in &counts {
        let dg = dim(w, Some(20261011));
        let sg = dg.structures.clone().unwrap();
        let pool = rayon::ThreadPoolBuilder::new().num_threads(n).build().unwrap();
        let t0 = std::time::Instant::now();
        let first = pool.install(|| {
            // First batch: 32 chunks, like the first send to the generator.
            chunks[..32].par_iter().for_each(|&(x, z)| {
                std::hint::black_box(dg.structures_nbt(x, z));
            });
            t0.elapsed()
        });
        pool.install(|| {
            chunks[32..].par_iter().for_each(|&(x, z)| {
                std::hint::black_box(dg.structures_nbt(x, z));
            });
        });
        let total = t0.elapsed();
        eprintln!(
            "{n:>3} threads: first batch {:>8.1} ms, 1,000 chunks {:>8.1} ms, stronghold searches {}",
            first.as_secs_f64() * 1e3,
            total.as_secs_f64() * 1e3,
            sg.ring_searches()
        );
    }
}

/// Survey: calls to the base height and answers served by the memo (repetitions) during the
/// disk compound of the structures of 1,024 chunks around the spawn, all threads.
/// Compare `RUSTCRAFT_OPT_BASEH_MEMO=0` and `=1`, `RUSTCRAFT_OPT_BASEH_LAZY=0` and `=1`:
/// `cargo test --release -p rc-worldgen --lib structure::tests::base_height_repeats -- --ignored --nocapture`.
#[test]
#[ignore = "benchmark"]
fn base_height_repeats() {
    use rayon::prelude::*;
    let Some(w) = world_data() else { return };
    let chunks: Vec<(i32, i32)> = (-16..16).flat_map(|x| (-16..16).map(move |z| (x, z))).collect();
    for seed in [20261011, 1, -6_214_586_474_519_478_921] {
        let dg = dim(w, Some(seed));
        let t = dg.structure_terrain().expect("terrain");
        let t0 = std::time::Instant::now();
        chunks.par_iter().for_each(|&(x, z)| {
            std::hint::black_box(dg.structures_nbt(x, z));
        });
        let (calls, hits) = t.base_height_stats();
        eprintln!(
            "seed {seed}: {:.1} ms, {calls} calls, {hits} served by the memo (opts {:?})",
            t0.elapsed().as_secs_f64() * 1e3,
            t.opts
        );
    }
}

/// Old path in the pool: the race recomputes (survey for the bench, not a requirement).
#[test]
fn rings_old_path_duplicates_under_contention() {
    let Some(w) = world_data() else { return };
    let dg = dim(w, Some(1));
    let (sg, t, set) = parts(&dg);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(32).build().unwrap();
    pool.install(|| {
        use rayon::prelude::*;
        (0..32).into_par_iter().for_each(|_| {
            sg.ring_positions_old(set, t);
        });
    });
    eprintln!("old path, 32 threads: {} searches for 128 strongholds", sg.ring_searches());
}

fn nether(w: &WorldData, seed: i64) -> DimGen {
    let spec = GenSpec::Noise {
        settings: "minecraft:nether".into(),
        source: SourceSpec::MultiNoise("minecraft:nether".into()),
    };
    let mut dg = DimGen::new(w, &spec, "minecraft:the_nether", seed).unwrap();
    dg.enable_structures(w).unwrap();
    dg
}

/// Stable form of a list of starts (the full debug form goes through tables with random
/// hashing in the template elements).
fn stable(st: &[crate::structure::Start]) -> String {
    use crate::structure::Piece;
    let mut s = String::new();
    for x in st {
        s += &format!("[{} {},{} {:?}", x.structure, x.cx, x.cz, x.bbox);
        for p in &x.pieces {
            match p {
                Piece::Jigsaw(j) => {
                    s += &format!(
                        " J{:?} {} {:?} {:?} {:?} {}",
                        j.pos, j.ground_delta, j.rot, j.bbox, j.junctions, j.waterlog
                    )
                }
                Piece::Coded { id, bbox, .. } => s += &format!(" C{id} {bbox:?}"),
            }
        }
        s += "]";
    }
    s
}

/// Fingerprint of the starts (types, pieces, boxes) of a square of
/// 2,000 blocks (125 × 125 chunks) and number of starts.
/// Disk compound seen as before G6-F19: without the fields added to the scattered and jigsaw pieces
/// (dimensions, height, flags, liquid settings), empty junctions. Used to prove that the
/// starts themselves have not moved.
fn legacy_view(n: &rc_proto::nbt::Nbt) -> rc_proto::nbt::Nbt {
    use rc_proto::nbt::Nbt;
    const ADDED: [&str; 17] = [
        "Width",
        "Height",
        "Depth",
        "HPos",
        "hasPlacedChest0",
        "hasPlacedChest1",
        "hasPlacedChest2",
        "hasPlacedChest3",
        "placedMainChest",
        "placedHiddenChest",
        "placedTrap1",
        "placedTrap2",
        "Witch",
        "Cat",
        "liquid_settings",
        "override_liquid_settings",
        "junctions",
    ];
    match n {
        Nbt::Compound(f) => Nbt::Compound(
            f.iter()
                .filter(|(k, _)| k == "junctions" || !ADDED.contains(&k.as_str()))
                .map(|(k, v)| (k.clone(), if k == "junctions" { Nbt::List(Vec::new()) } else { legacy_view(v) }))
                .collect(),
        ),
        Nbt::List(l) => Nbt::List(l.iter().map(legacy_view).collect()),
        o => o.clone(),
    }
}

fn starts_digest_with(dg: &DimGen, legacy: bool) -> (u64, usize) {
    let sg = dg.structures.clone().expect("structures");
    let t = dg.structure_terrain().expect("terrain");
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut n = 0;
    for cx in -62..63 {
        for cz in -62..63 {
            let st = sg.starts(cx, cz, t, &dg.blocks);
            n += st.len();
            // Disk compound (starts and references), plus the stable form of the pieces.
            let nbt = dg.structures_nbt(cx, cz);
            let nbt = if legacy { legacy_view(&nbt) } else { nbt };
            for b in format!("{cx},{cz}:{};{nbt:?}", stable(&st)).bytes() {
                h ^= u64::from(b);
                h = h.wrapping_mul(0x0100_0000_01b3);
            }
        }
    }
    (h, n)
}

/// (seed, main world fingerprint, starts, Nether fingerprint, starts).
const EXPECTED_STARTS: [(i64, u64, usize, u64, usize); 3] = [
    (20261011, 0x9865810b7562e1b1, 139, 0xd081054f684115bc, 462),
    (1, 0x516cae3251dee064, 152, 0x181b29d3f38ad641, 552),
    (-4172144997902289642, 0x54a70f38ee068b4e, 169, 0xedd2e3c5cd1a05d8, 369),
];

/// Same fingerprints, full disk compound (fields of the scattered and jigsaw pieces of G6-F19).
const EXPECTED_STARTS_NBT: [(i64, u64, usize, u64, usize); 3] = [
    (20261011, 0x380dc891f5798769, 139, 0x6436685cd040f166, 462),
    (1, 0x6d8ebb7b162e710f, 152, 0x5d6dcc2cae50bfd8, 552),
    (-4172144997902289642, 0x5223aed5180a3acb, 169, 0xbc02d866035efcbd, 369),
];

/// Starts of a 2,000-block square, main world and Nether = values of the old code
/// (compound seen without the added fields), then full compound = stored values.
#[test]
fn starts_match_stored_values() {
    let Some(w) = world_data() else { return };
    for (legacy, expected) in [(true, &EXPECTED_STARTS), (false, &EXPECTED_STARTS_NBT)] {
        let mut got = Vec::new();
        for &(seed, ..) in expected {
            let (a, na) = starts_digest_with(&dim(w, Some(seed)), legacy);
            let (b, nb) = starts_digest_with(&nether(w, seed), legacy);
            eprintln!("    ({seed}, 0x{a:016x}, {na}, 0x{b:016x}, {nb}),");
            got.push((seed, a, na, b, nb));
        }
        assert_eq!(got.as_slice(), expected.as_slice(), "starts differ from the stored values (old view {legacy})");
    }
}

/// Biome context kept per attempt = new context per point: same starts, chunk by
/// chunk, main world and Nether.
#[test]
fn genstart_ctx_matches_uncached() {
    let Some(w) = world_data() else { return };
    for &(seed, ..) in &EXPECTED_STARTS {
        for dg in [dim(w, Some(seed)), nether(w, seed)] {
            let sg = dg.structures.clone().unwrap();
            let t = dg.structure_terrain().unwrap();
            for cx in -62..63 {
                for cz in -62..63 {
                    let a = stable(&sg.compute_starts(cx, cz, t, &dg.blocks, false));
                    let b = stable(&sg.compute_starts(cx, cz, t, &dg.blocks, true));
                    assert_eq!(a, b, "seed {seed}, chunk ({cx}, {cz})");
                }
            }
        }
    }
}

/// 32 threads, each chunk of a 64 × 64 square requested twice: a single `Arc` per chunk,
/// a single empty list for all; then the disk compound = stored values.
#[test]
fn starts_parallel_share_one_arc() {
    use rayon::prelude::*;
    let Some(w) = world_data() else { return };
    let dg = dim(w, Some(20261011));
    let sg = dg.structures.clone().unwrap();
    let t = dg.structure_terrain().unwrap();
    let mut list: Vec<(i32, i32)> = (-32..32).flat_map(|x| (-32..32).map(move |z| (x, z))).collect();
    let back: Vec<(i32, i32)> = list.iter().rev().copied().collect();
    list.extend(back);
    let pool = rayon::ThreadPoolBuilder::new().num_threads(32).build().unwrap();
    let got: Vec<((i32, i32), super::Starts)> =
        pool.install(|| list.par_iter().map(|&(x, z)| ((x, z), sg.starts_with(x, z, t, &dg.blocks, true, true))).collect());
    let mut first: hashbrown::HashMap<(i32, i32), super::Starts> = hashbrown::HashMap::new();
    let mut empties = hashbrown::HashSet::new();
    for (k, v) in got {
        if v.is_empty() {
            empties.insert(Arc::as_ptr(&v) as usize);
        }
        let f = first.entry(k).or_insert_with(|| v.clone());
        assert!(Arc::ptr_eq(f, &v), "two lists for the chunk {k:?}");
    }
    assert_eq!(empties.len(), 1, "distinct empty lists");
    let (h, n) = starts_digest_with(&dg, true);
    assert_eq!((h, n), (EXPECTED_STARTS[0].1, EXPECTED_STARTS[0].2));
}

/// Stable form of the references of a chunk.
fn refs_stable(r: &std::collections::BTreeMap<usize, Vec<super::Starts>>) -> String {
    r.iter().map(|(s, l)| format!("{s}:{}", l.iter().map(|v| stable(v)).collect::<Vec<_>>().join("|"))).collect()
}

/// Reference prefilter: same references as without, chunk by chunk (48 × 48 square,
/// main world and Nether, two seeds), and fewer entries in the start cache.
#[test]
fn references_prefilter_matches_full() {
    let Some(w) = world_data() else { return };
    for seed in [20261011, -4172144997902289642] {
        for mk in [dim as fn(&WorldData, Option<i64>) -> DimGen, |w, s| nether(w, s.unwrap())] {
            let (a, b) = (mk(w, Some(seed)), mk(w, Some(seed)));
            let (sa, sb) = (a.structures.clone().unwrap(), b.structures.clone().unwrap());
            let (ta, tb) = (a.structure_terrain().unwrap(), b.structure_terrain().unwrap());
            let mut refs = 0;
            for cx in -24..24 {
                for cz in -24..24 {
                    let ra = sa.references_with(cx, cz, ta, &a.blocks, false);
                    let rb = sb.references_with(cx, cz, tb, &b.blocks, true);
                    refs += ra.len();
                    assert_eq!(refs_stable(&ra), refs_stable(&rb), "seed {seed}, chunk ({cx}, {cz})");
                }
            }
            let (na, nb) = (sa.starts.lock().unwrap().len(), sb.starts.lock().unwrap().len());
            eprintln!("seed {seed}: {refs} references, cache entries {na} without prefilter, {nb} with");
            assert!(refs > 0, "no reference: pointless test");
            assert!(nb < na, "the prefilter avoids no entry");
        }
    }
}
