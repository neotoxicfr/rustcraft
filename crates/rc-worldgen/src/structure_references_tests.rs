//! Equivalence proofs of the reference walk (candidates found per window against the questions
//! chunk by chunk, borrowed starts against copies) and its bench.
//! Game data required (jar 26.3 and rc-data cache); absent: tests skipped, except
//! `RUSTCRAFT_OPT_REQUIRE=1` (explicit failure).

use std::sync::Arc;
use std::sync::atomic::Ordering;

use super::{REFERENCE_REACH, ReferencePaths, ReferenceSabotage, References, referenced_starts, referenced_starts_cloned};
use crate::chunkgen::{DimGen, WorldData};
use crate::structure::tests::{dim, nether, refs_stable, stable, world_data};
use crate::structure::{Placement, Reducer, Start, StructureGen, WalkCounts};
use crate::template::BBox;
use crate::terrain::{ChunkRect, TerrainGenerator};

/// Seeds of the main world compared (the first is the test world of the other structure tests).
pub(super) const SEEDS: [i64; 3] = [20261011, 1, -4172144997902289642];

/// Every combination of the two per-window paths, the prefilter on.
const WINDOW_PATHS: [ReferencePaths; 3] = [
    ReferencePaths { candidates_by_region: true, ..ReferencePaths::PER_CHUNK },
    ReferencePaths { ring_window_filter: true, ..ReferencePaths::PER_CHUNK },
    ReferencePaths::NEW,
];

pub(super) fn parts(dg: &DimGen) -> (&StructureGen, &TerrainGenerator) {
    (dg.structures.as_deref().expect("structures"), dg.structure_terrain().expect("terrain"))
}

pub(super) fn square(center: (i32, i32), side: i32) -> ChunkRect {
    ChunkRect { min_x: center.0 - side / 2, min_z: center.1 - side / 2, width: side, depth: side }
}

pub(super) fn chunks_of(area: ChunkRect) -> impl Iterator<Item = (i32, i32)> {
    (0..area.area()).map(move |i| area.position(i))
}

/// Chunk of the first stronghold (rings resolved in full); a world without rings (Nether): any
/// other place.
fn first_stronghold(sg: &StructureGen, t: &TerrainGenerator) -> (i32, i32) {
    match sg.sets.iter().position(|s| matches!(s.placement, Placement::Rings { .. })) {
        Some(rings) => sg.ring_positions(rings, t)[0],
        None => (5_000, -3_000),
    }
}

/// Answers of the chunk-by-chunk question (the reference) over `area` grown by the reach of a
/// reference window, asked once per chunk.
struct PerChunkAnswers {
    grown: ChunkRect,
    candidate: Vec<bool>,
}

impl PerChunkAnswers {
    fn over(sg: &StructureGen, t: &TerrainGenerator, area: ChunkRect) -> Self {
        let grown = ChunkRect {
            min_x: area.min_x - REFERENCE_REACH,
            min_z: area.min_z - REFERENCE_REACH,
            width: area.width + 2 * REFERENCE_REACH,
            depth: area.depth + 2 * REFERENCE_REACH,
        };
        let candidate = chunks_of(grown).map(|(x, z)| sg.any_structure_chunk(x, z, t)).collect();
        PerChunkAnswers { grown, candidate }
    }

    fn is_candidate(&self, x: i32, z: i32) -> bool {
        self.candidate[self.grown.index(x, z).expect("inside the grown area")]
    }

    /// Window chunks whose per-window answer differs from the chunk-by-chunk one, over every
    /// centre of `area`; also returns the candidates met (a test without any proves nothing).
    fn differences(&self, sg: &StructureGen, t: &TerrainGenerator, area: ChunkRect, paths: ReferencePaths) -> (usize, usize) {
        let (mut differences, mut candidates) = (0, 0);
        for (cx, cz) in chunks_of(area) {
            let window = sg.candidate_window(cx, cz, t, paths);
            for (x, z) in chunks_of(square((cx, cz), 2 * REFERENCE_REACH + 1)) {
                let expected = self.is_candidate(x, z);
                candidates += usize::from(expected);
                differences += usize::from(window.contains(x, z) != expected);
            }
        }
        (differences, candidates)
    }
}

/// [`PerChunkAnswers::differences`] of one path over one area.
fn window_differences(sg: &StructureGen, t: &TerrainGenerator, area: ChunkRect, paths: ReferencePaths) -> usize {
    PerChunkAnswers::over(sg, t, area).differences(sg, t, area, paths).0
}

/// Areas compared in a world: around the origin (negative coordinates, several regions of every
/// set), around the first stronghold, and far away. 10,001 chunks in all.
fn compared_areas(sg: &StructureGen, t: &TerrainGenerator) -> [ChunkRect; 3] {
    [square((0, 0), 70), square(first_stronghold(sg, t), 50), square((-100_003, 77_777), 51)]
}

/// Worlds compared: main world on three seeds, classic flat world, Nether.
fn compared_worlds(w: &WorldData) -> Vec<(String, DimGen)> {
    let mut worlds: Vec<(String, DimGen)> =
        SEEDS.iter().map(|&seed| (format!("main world, seed {seed}"), dim(w, Some(seed)))).collect();
    worlds.push(("flat world".into(), dim(w, None)));
    worlds.push(("Nether".into(), nether(w, SEEDS[0])));
    worlds
}

/// Candidate windows found per window (regions, ring filter, both) = the chunk-by-chunk answers,
/// bit for bit, on 10,001 chunks of each world: 3 seeds of the main world (16 random-spread sets,
/// one with an exclusion zone, and the rings), flat world, Nether (spacing 2).
#[test]
fn candidates_by_region_match_per_chunk() {
    let Some(w) = world_data() else { return };
    for (name, dg) in compared_worlds(w) {
        let (sg, t) = parts(&dg);
        let mut candidates_met = 0;
        for area in compared_areas(sg, t) {
            let reference = PerChunkAnswers::over(sg, t, area);
            for paths in WINDOW_PATHS {
                let (differences, candidates) = reference.differences(sg, t, area, paths);
                assert_eq!(differences, 0, "{name}, area {area:?}, {paths:?}");
                candidates_met += candidates;
            }
        }
        eprintln!("{name}: {candidates_met} candidate answers compared");
        assert!(candidates_met > 0, "{name}: no candidate chunk, pointless comparison");
    }
}

/// Small deterministic generator of the synthetic placements.
struct TestRng(u64);

impl TestRng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Integer in `lo..=hi`.
    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + (self.next() % (hi - lo + 1) as u64) as i32
    }

    fn pick<T: Copy>(&mut self, choices: &[T]) -> T {
        choices[self.next() as usize % choices.len()]
    }
}

/// Redraws every placement of the generator: spacings from 1 (every chunk its own region) to 40,
/// any separation, both spreads, the four frequency reducers, exclusion zones pointing at an
/// earlier set (random-spread or rings), and a frequency and an exclusion on the rings too.
fn redraw_placements(sg: &mut StructureGen, rng: &mut TestRng) {
    let ids: Vec<String> = sg.sets.iter().map(|s| s.id.clone()).collect();
    for i in 0..sg.sets.len() {
        let new_frequency = rng.pick(&[1.0, 1.0, 0.6, 0.25]);
        let new_reducer = rng.pick(&[Reducer::Default, Reducer::Legacy1, Reducer::Legacy2, Reducer::Legacy3]);
        // Only earlier sets: no exclusion cycle (the game's data have none either).
        let new_exclusion = (i > 0 && rng.range(0, 2) == 0).then(|| (ids[rng.range(0, i as i32 - 1) as usize].clone(), rng.range(1, 16)));
        match &mut sg.sets[i].placement {
            Placement::Random { spacing, separation, triangular, salt, frequency, reducer, exclusion } => {
                *spacing = rng.pick(&[1, 1, 2, 3, 5, 8, 16, 17, 24, 34, 40]);
                *separation = rng.range(0, *spacing - 1);
                *triangular = rng.range(0, 1) == 1;
                *salt = rng.range(0, 1_000_000);
                (*frequency, *reducer, *exclusion) = (new_frequency, new_reducer, new_exclusion);
            }
            Placement::Rings { frequency, reducer, exclusion, .. } => {
                (*frequency, *reducer, *exclusion) = (new_frequency, new_reducer, new_exclusion);
            }
            Placement::Origin => {}
        }
    }
}

/// Same comparison on placements drawn at random (what a data pack may bring): 9 draws of every
/// set of the main world, around the origin and around the first stronghold.
#[test]
fn candidates_by_region_match_per_chunk_on_random_placements() {
    let Some(w) = world_data() else { return };
    let mut rng = TestRng(0x5EED_0001);
    let mut candidates_met = 0;
    for round in 0..9 {
        let mut dg = dim(w, Some(SEEDS[round % SEEDS.len()]));
        redraw_placements(Arc::get_mut(dg.structures.as_mut().unwrap()).unwrap(), &mut rng);
        let (sg, t) = parts(&dg);
        for area in [square((0, 0), 24), square(first_stronghold(sg, t), 20)] {
            let reference = PerChunkAnswers::over(sg, t, area);
            for paths in WINDOW_PATHS {
                let (differences, candidates) = reference.differences(sg, t, area, paths);
                assert_eq!(differences, 0, "round {round}, area {area:?}, {paths:?}, sets {:?}", sg.sets);
                candidates_met += candidates;
            }
        }
    }
    assert!(candidates_met > 0, "no candidate chunk, pointless comparison");
}

/// Main world whose second random-spread set avoids the first within 4 chunks: about a third of
/// its chunks are excluded.
fn world_with_frequent_exclusion(w: &WorldData) -> DimGen {
    let mut dg = dim(w, Some(SEEDS[0]));
    let sg = Arc::get_mut(dg.structures.as_mut().unwrap()).unwrap();
    let spread: Vec<usize> =
        (0..sg.sets.len()).filter(|&i| matches!(sg.sets[i].placement, Placement::Random { .. })).take(2).collect();
    let avoided = sg.sets[spread[0]].id.clone();
    for (set, new_spacing, new_exclusion) in [(spread[0], 16, None), (spread[1], 6, Some((avoided, 4)))] {
        if let Placement::Random { spacing, separation, frequency, exclusion, .. } = &mut sg.sets[set].placement {
            (*spacing, *separation, *frequency, *exclusion) = (new_spacing, 0, 1.0, new_exclusion);
        }
    }
    dg
}

/// Each deliberate error of the per-window search is seen by the comparison above (game
/// placements for the region, spread and ring errors; a set that often excludes for the
/// exclusion, which the game data use for one rare set only).
#[test]
fn candidate_comparison_catches_sabotage() {
    let Some(w) = world_data() else { return };
    let dg = dim(w, Some(SEEDS[0]));
    let (sg, t) = parts(&dg);
    let sabotaged = |sabotage| ReferencePaths { sabotage, ..ReferencePaths::NEW };
    let origin = square((0, 0), 70);
    let stronghold = square(first_stronghold(sg, t), 50);
    for (sabotage, area) in [
        (ReferenceSabotage::RegionShiftedByOne, origin),
        (ReferenceSabotage::TriangularAsLinear, origin),
        (ReferenceSabotage::WindowTransposed, origin),
        (ReferenceSabotage::RingDrawsWithoutReach, stronghold),
    ] {
        let differences = window_differences(sg, t, area, sabotaged(sabotage));
        eprintln!("{sabotage:?}: {differences} window answers differ");
        assert!(differences > 0, "{sabotage:?} not seen");
    }

    let excluding = world_with_frequent_exclusion(w);
    let (sg, t) = parts(&excluding);
    let area = square((0, 0), 30);
    assert_eq!(window_differences(sg, t, area, ReferencePaths::NEW), 0, "frequent exclusion, no sabotage");
    let differences = window_differences(sg, t, area, sabotaged(ReferenceSabotage::ExclusionOmitted));
    eprintln!("ExclusionOmitted: {differences} window answers differ");
    assert!(differences > 0, "ExclusionOmitted not seen");
}

/// The leads of the reference walk, each switchable (the paths of the walk itself, and two
/// settings of the generator).
#[derive(Debug, Clone, Copy)]
pub(super) struct WalkLeads {
    paths: ReferencePaths,
    reducer_seed_drawn_once: bool,
    sharded_start_cache: bool,
    starts_computed_once: bool,
}

impl WalkLeads {
    pub(super) const OLD: WalkLeads = WalkLeads {
        paths: ReferencePaths::PER_CHUNK,
        reducer_seed_drawn_once: false,
        sharded_start_cache: false,
        starts_computed_once: false,
    };
    pub(super) const NEW: WalkLeads = WalkLeads {
        paths: ReferencePaths::NEW,
        reducer_seed_drawn_once: true,
        sharded_start_cache: true,
        starts_computed_once: true,
    };

    /// Each lead alone on top of the old path, then all together.
    fn each_alone_then_all() -> [(&'static str, WalkLeads); 7] {
        let old = ReferencePaths::PER_CHUNK;
        [
            ("candidates by region", WalkLeads { paths: ReferencePaths { candidates_by_region: true, ..old }, ..Self::OLD }),
            ("ring window filter", WalkLeads { paths: ReferencePaths { ring_window_filter: true, ..old }, ..Self::OLD }),
            ("empty lists not cloned", WalkLeads { paths: ReferencePaths { empty_lists_not_cloned: true, ..old }, ..Self::OLD }),
            ("reducer seed drawn once", WalkLeads { reducer_seed_drawn_once: true, ..Self::OLD }),
            ("sharded start cache", WalkLeads { sharded_start_cache: true, ..Self::OLD }),
            ("starts computed once", WalkLeads { starts_computed_once: true, ..Self::OLD }),
            ("all", Self::NEW),
        ]
    }

    /// The generator set to these leads (empty start cache).
    pub(super) fn applied_to(self, mut dg: DimGen) -> DimGen {
        let sg = Arc::get_mut(dg.structures.as_mut().expect("structures")).expect("generator not shared yet");
        sg.reference_paths = self.paths;
        sg.reducer_seed_drawn_once = self.reducer_seed_drawn_once;
        sg.starts_computed_once = self.starts_computed_once;
        sg.reset_start_cache(self.sharded_start_cache);
        dg
    }
}

/// References through each lead of the walk alone, then all together = references of the old
/// path, chunk by chunk (40 × 40 square, main world and Nether, two seeds), with the same chunks
/// in the start cache at the end.
#[test]
fn references_match_through_every_walk_lead() {
    let Some(w) = world_data() else { return };
    let area = square((0, 0), 40);
    for seed in [SEEDS[0], SEEDS[2]] {
        for make in [dim as fn(&WorldData, Option<i64>) -> DimGen, |w, s| nether(w, s.unwrap())] {
            let old = WalkLeads::OLD.applied_to(make(w, Some(seed)));
            let (old_sg, old_t) = parts(&old);
            let expected: Vec<String> =
                chunks_of(area).map(|(cx, cz)| refs_stable(&old_sg.references(cx, cz, old_t, &old.blocks))).collect();
            assert!(expected.iter().any(|r| !r.is_empty()), "no reference: pointless test");
            for (lead, leads) in WalkLeads::each_alone_then_all() {
                let new = leads.applied_to(make(w, Some(seed)));
                let (new_sg, new_t) = parts(&new);
                for ((cx, cz), expected) in chunks_of(area).zip(&expected) {
                    let got = refs_stable(&new_sg.references(cx, cz, new_t, &new.blocks));
                    assert_eq!(&got, expected, "{lead}, {}, seed {seed}, chunk ({cx}, {cz})", new.dimension);
                }
                assert_eq!(
                    new_sg.starts.chunks(),
                    old_sg.starts.chunks(),
                    "{lead}, seed {seed}: other chunks read from the start cache"
                );
            }
        }
    }
}

/// Reducer whose seed factors are kept = reducer that draws them again, on 200,000 chunks near
/// and far, both signs; a kept factor of another seed is seen.
#[test]
fn reducer_seed_drawn_once_matches_redrawn() {
    let Some(w) = world_data() else { return };
    let redrawn = WalkLeads::OLD.applied_to(dim(w, Some(SEEDS[2])));
    let kept = WalkLeads { reducer_seed_drawn_once: true, ..WalkLeads::OLD }.applied_to(dim(w, Some(SEEDS[2])));
    let mut wrong = WalkLeads { reducer_seed_drawn_once: true, ..WalkLeads::OLD }.applied_to(dim(w, Some(SEEDS[2])));
    let wrong_sg = Arc::get_mut(wrong.structures.as_mut().unwrap()).unwrap();
    wrong_sg.large_feature_seed = crate::carver::LargeFeatureSeed::new(wrong_sg.seed, 1);
    let (redrawn, kept, wrong) = (parts(&redrawn).0, parts(&kept).0, parts(&wrong).0);
    let mut rng = TestRng(0x5EED);
    let (mut passed, mut wrong_answers) = (0, 0);
    for i in 0..200_000 {
        let reach = if i % 2 == 0 { 300 } else { 1_875_000 };
        let (cx, cz) = (rng.range(-reach, reach), rng.range(-reach, reach));
        let probability = rng.pick(&[0.004f32, 0.3, 0.75]);
        let expected = redrawn.reduce(Reducer::Legacy3, 0, cx, cz, probability);
        assert_eq!(kept.reduce(Reducer::Legacy3, 0, cx, cz, probability), expected, "chunk ({cx}, {cz}), {probability}");
        passed += usize::from(expected);
        wrong_answers += usize::from(wrong.reduce(Reducer::Legacy3, 0, cx, cz, probability) != expected);
    }
    eprintln!("{passed} of 200,000 chunks pass; a kept factor of another seed changes {wrong_answers} answers");
    assert!(passed > 0 && wrong_answers > 0);
}

/// Start cache emptied on the way (table capacity lowered to 2 chunks), one table or 64: the
/// references are those of a cache never emptied, the clears are counted, and a chunk asked twice
/// in a row gets the very same list.
#[test]
fn start_cache_clears_change_no_reference() {
    let Some(w) = world_data() else { return };
    let area = square(VILLAGE_CHUNK, 24);
    let never_cleared = WalkLeads::OLD.applied_to(dim(w, Some(SEEDS[0])));
    let (reference_sg, reference_t) = parts(&never_cleared);
    for sharded in [false, true] {
        let mut dg = WalkLeads { sharded_start_cache: sharded, ..WalkLeads::NEW }.applied_to(dim(w, Some(SEEDS[0])));
        Arc::get_mut(dg.structures.as_mut().unwrap()).unwrap().starts.table_capacity = 2;
        let (sg, t) = parts(&dg);
        let mut referenced = 0;
        for (cx, cz) in chunks_of(area) {
            let expected = reference_sg.references(cx, cz, reference_t, &never_cleared.blocks);
            referenced += expected.len();
            assert_eq!(
                refs_stable(&sg.references(cx, cz, t, &dg.blocks)),
                refs_stable(&expected),
                "sharded {sharded}, chunk ({cx}, {cz})"
            );
            let (first, again) = (sg.starts(cx, cz, t, &dg.blocks), sg.starts(cx, cz, t, &dg.blocks));
            assert!(Arc::ptr_eq(&first, &again), "sharded {sharded}: two lists for the chunk ({cx}, {cz})");
        }
        eprintln!("sharded {sharded}: {} clears, {referenced} references", sg.starts_clears());
        assert!(referenced > 0 && sg.starts_clears() > 0, "sharded {sharded}: nothing referenced or never cleared");
        assert_eq!(reference_sg.starts_clears(), 0);
    }
}

/// 32 threads asking each chunk of a 48 × 48 square twice, one table or 64: one list per chunk,
/// one empty list for all, and `starts_if_any` hands out that same list or nothing.
#[test]
fn start_cache_tables_share_one_list_per_chunk() {
    use rayon::prelude::*;
    let Some(w) = world_data() else { return };
    let mut asked: Vec<(i32, i32)> = chunks_of(square((0, 0), 48)).collect();
    asked.extend(asked.clone().into_iter().rev());
    let pool = rayon::ThreadPoolBuilder::new().num_threads(32).build().unwrap();
    for sharded in [false, true] {
        let dg = WalkLeads { sharded_start_cache: sharded, ..WalkLeads::NEW }.applied_to(dim(w, Some(SEEDS[0])));
        let (sg, t) = parts(&dg);
        let got: Vec<((i32, i32), super::Starts)> =
            pool.install(|| asked.par_iter().map(|&(x, z)| ((x, z), sg.starts(x, z, t, &dg.blocks))).collect());
        let mut first: hashbrown::HashMap<(i32, i32), super::Starts> = hashbrown::HashMap::new();
        let mut empty_lists = hashbrown::HashSet::new();
        let mut with_starts = 0;
        for (chunk, list) in got {
            if list.is_empty() {
                empty_lists.insert(Arc::as_ptr(&list) as usize);
            }
            let kept = first.entry(chunk).or_insert_with(|| list.clone());
            assert!(Arc::ptr_eq(kept, &list), "sharded {sharded}: two lists for the chunk {chunk:?}");
            match sg.starts_if_any(chunk.0, chunk.1, t, &dg.blocks) {
                Some(same) => {
                    assert!(Arc::ptr_eq(&same, &list) && !list.is_empty(), "sharded {sharded}, chunk {chunk:?}");
                    with_starts += 1;
                }
                None => assert!(list.is_empty(), "sharded {sharded}: starts of {chunk:?} hidden"),
            }
        }
        assert_eq!(empty_lists.len(), 1, "sharded {sharded}: distinct empty lists");
        assert!(with_starts > 0, "sharded {sharded}: no start in the square");
    }
}

fn start_of(structure: usize, cx: i32) -> Start {
    Start { structure, cx, cz: 0, pieces: Vec::new(), bbox: BBox::new([cx * 16, 0, 0], [cx * 16 + 5, 5, 5]) }
}

/// Borrowed lists: only the starts of the referenced structure, in list order, never a copy; a
/// chunk list that also holds a start of another structure does not leak it (the filter left out
/// would add (1, 7) to structure 0 and (0, 7) to structure 1).
#[test]
fn referenced_starts_keep_only_their_structure() {
    let shared_chunk = Arc::new(vec![start_of(0, 7), start_of(1, 7)]);
    let other_chunk = Arc::new(vec![start_of(0, 9)]);
    let mut references = References::new();
    references.insert(0, vec![shared_chunk.clone(), other_chunk.clone()]);
    references.insert(1, vec![shared_chunk.clone()]);
    let borrowed = referenced_starts(&references);
    let seen: Vec<(usize, Vec<(usize, i32)>)> =
        borrowed.iter().map(|(s, starts)| (*s, starts.iter().map(|st| (st.structure, st.cx)).collect())).collect();
    assert_eq!(seen, vec![(0, vec![(0, 7), (0, 9)]), (1, vec![(1, 7)])]);
    assert!(std::ptr::eq(borrowed[0].1[0], &shared_chunk[0]), "start copied instead of borrowed");
    let cloned = referenced_starts_cloned(&references);
    assert_eq!(cloned.len(), borrowed.len());
    for ((s, owned), (_, lent)) in cloned.iter().zip(&borrowed) {
        let lent: Vec<Start> = lent.iter().map(|st| (*st).clone()).collect();
        assert_eq!(stable(owned), stable(&lent), "structure {s}");
    }
}

/// Borrowed lists = copied lists on real references (village of the test world, Nether).
#[test]
fn referenced_starts_borrowed_match_cloned() {
    let Some(w) = world_data() else { return };
    let mut referenced = 0;
    for (dg, center) in [(dim(w, Some(SEEDS[0])), VILLAGE_CHUNK), (nether(w, SEEDS[0]), (0, 0))] {
        let (sg, t) = parts(&dg);
        for (cx, cz) in chunks_of(square(center, 17)) {
            let references = sg.references(cx, cz, t, &dg.blocks);
            let borrowed = referenced_starts(&references);
            let cloned = sg.referenced(cx, cz, t, &dg.blocks);
            assert_eq!(borrowed.len(), cloned.len(), "chunk ({cx}, {cz})");
            for ((s, lent), (cloned_s, owned)) in borrowed.iter().zip(&cloned) {
                let lent: Vec<Start> = lent.iter().map(|st| (*st).clone()).collect();
                assert_eq!((s, stable(&lent)), (cloned_s, stable(owned)), "chunk ({cx}, {cz})");
                referenced += owned.len();
            }
        }
    }
    assert!(referenced > 0, "no referenced start: pointless test");
}

/// Chunk of the village of the test world (seed 20261011), at (-1408, 368).
pub(super) const VILLAGE_CHUNK: (i32, i32) = (-88, 23);

/// Counters of a generator, read and reset.
fn take_counts(sg: &StructureGen) -> [u64; 8] {
    let c: &WalkCounts = sg.walk_counts.as_deref().expect("counting generator");
    [
        &c.per_chunk_tests,
        &c.region_draws,
        &c.reducer_draws,
        &c.ring_draws_compared,
        &c.start_cache_locks,
        &c.start_cache_lock_waits,
        &c.start_biome_reads,
        &c.starts_computed,
    ]
    .map(|counter| counter.swap(0, Ordering::Relaxed))
}

/// Best of `passes` timings of `work`, in microseconds.
fn best_micros(passes: usize, mut work: impl FnMut()) -> f64 {
    (0..passes)
        .map(|_| {
            let started = std::time::Instant::now();
            work();
            started.elapsed().as_secs_f64() * 1e6
        })
        .fold(f64::INFINITY, f64::min)
}

/// Thread counts of a bench: 1, then powers of 2 up to the logical cores.
fn thread_counts() -> Vec<usize> {
    let max = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut counts: Vec<usize> = std::iter::successors(Some(1usize), |n| Some(n * 2)).take_while(|&n| n <= max).collect();
    if counts.last() != Some(&max) {
        counts.push(max);
    }
    counts
}

/// What the bench compares: the old walk, each lead alone, then the leads stacked.
fn bench_leads() -> Vec<(&'static str, WalkLeads)> {
    let regions_and_rings =
        ReferencePaths { candidates_by_region: true, ring_window_filter: true, ..ReferencePaths::PER_CHUNK };
    let mut leads = vec![("old (per chunk)", WalkLeads::OLD)];
    leads.extend(WalkLeads::each_alone_then_all().into_iter().take(5));
    leads.extend([
        ("regions + ring filter", WalkLeads { paths: regions_and_rings, ..WalkLeads::OLD }),
        ("  + reducer seed", WalkLeads { paths: regions_and_rings, reducer_seed_drawn_once: true, ..WalkLeads::OLD }),
        ("  + empty lists not cloned", WalkLeads { sharded_start_cache: false, ..WalkLeads::NEW }),
        ("  + sharded cache (all, new)", WalkLeads::NEW),
    ]);
    leads
}

/// Generator set to `leads`, rings resolved (as the generation thread does at creation) and
/// start cache filled over `chunks`. `counting`: it counts its placement tests and cache locks;
/// such a generator is never timed (every thread would write the same counters).
fn warm_generator(dg: DimGen, leads: WalkLeads, chunks: &[(i32, i32)], counting: bool) -> DimGen {
    let mut dg = leads.applied_to(dg);
    if counting {
        Arc::get_mut(dg.structures.as_mut().unwrap()).unwrap().walk_counts = Some(Box::default());
    }
    dg.warm_rings();
    let (sg, t) = parts(&dg);
    for &(cx, cz) in chunks {
        sg.references(cx, cz, t, &dg.blocks);
    }
    dg
}

/// References of every chunk, one after the other on the calling thread.
fn walk_in_turn(dg: &DimGen, chunks: &[(i32, i32)]) {
    let (sg, t) = parts(dg);
    for &(cx, cz) in chunks {
        std::hint::black_box(sg.references(cx, cz, t, &dg.blocks));
    }
}

/// References of every chunk, spread over the threads of `pool`.
fn walk_in_pool(dg: &DimGen, chunks: &[(i32, i32)], pool: &rayon::ThreadPool) {
    use rayon::prelude::*;
    let (sg, t) = parts(dg);
    pool.install(|| {
        chunks.par_iter().for_each(|&(cx, cz)| {
            std::hint::black_box(sg.references(cx, cz, t, &dg.blocks));
        });
    });
}

/// Micro-bench of the reference walk: 1,024 chunks around the spawn (main world and Nether, seed
/// 20261011), start cache warm (the starts themselves are not the subject). Per walk and per lead:
/// duration on one thread, questions asked chunk by chunk, regions drawn, reducers drawn,
/// stronghold draws compared, locks of the start cache, allocations; then the wall time of the
/// 1,024 walks on 1, 2, 4… threads with the cache locks that had to wait, per walk.
/// `cargo test --release -p rc-worldgen --lib bench_references -- --ignored --nocapture --test-threads=1`.
#[test]
#[ignore = "benchmark"]
fn bench_references() {
    let Some(w) = world_data() else { return };
    let chunks: Vec<(i32, i32)> = chunks_of(square((0, 0), 32)).collect();
    let pools: Vec<rayon::ThreadPool> =
        thread_counts().into_iter().map(|n| rayon::ThreadPoolBuilder::new().num_threads(n).build().unwrap()).collect();
    let per_walk = |n: u64| n as f64 / chunks.len() as f64;
    for (world, make) in [("main world", dim as fn(&WorldData, Option<i64>) -> DimGen), ("Nether", |w, s| nether(w, s.unwrap()))] {
        eprintln!("{world}, {} walks; one thread per walk, then wall time of all the walks per thread count (lock waits per walk):", chunks.len());
        for (label, leads) in bench_leads() {
            let timed = warm_generator(make(w, Some(SEEDS[0])), leads, &chunks, false);
            let counting = warm_generator(make(w, Some(SEEDS[0])), leads, &chunks, true);
            let counted = parts(&counting).0;
            let biome_reads_filling_the_cache = take_counts(counted)[6];
            if label.starts_with("old") {
                eprintln!(
                    "  filling the start cache ({} chunks held): {biome_reads_filling_the_cache} biomes read from the source by the start attempts",
                    counted.starts.chunks().len()
                );
            }
            let allocations_before = crate::test_alloc::allocations();
            walk_in_turn(&counting, &chunks);
            let allocations = crate::test_alloc::allocations() - allocations_before;
            let counts = take_counts(counted).map(per_walk);
            let micros = best_micros(15, || walk_in_turn(&timed, &chunks)) / chunks.len() as f64;
            eprintln!(
                "  {label:<30} {micros:>7.2} µs | per-chunk questions {:>6.0} | regions drawn {:>6.1} | reducers {:>5.0} | \
                 stronghold draws compared {:>6.0} | cache locks {:>5.1} | allocations {:>4.2}",
                counts[0],
                counts[1],
                counts[2],
                counts[3],
                counts[4],
                per_walk(allocations)
            );
            let mut line = String::from("      threads:");
            for pool in &pools {
                walk_in_pool(&counting, &chunks, pool);
                let waits = per_walk(take_counts(counted)[5]);
                let millis = best_micros(9, || walk_in_pool(&timed, &chunks, pool)) / 1e3;
                line += &format!(" {}: {millis:.2} ms ({waits:.2}) |", pool.current_num_threads());
            }
            eprintln!("{line}");
        }
    }
}

/// Micro-bench of what a new chunk does with its references, around the village of the test
/// world (9 × 9 chunks, start cache warm, one thread): the three walks with copied starts of the
/// old path (beard, saved compound, decoration) against one walk with borrowed starts; duration
/// and allocations per chunk. The beard itself, the terrain and the placement are not timed.
/// `cargo test --release -p rc-worldgen --lib bench_reference_use -- --ignored --nocapture --test-threads=1`.
#[test]
#[ignore = "benchmark"]
fn bench_reference_use_per_chunk() {
    let Some(w) = world_data() else { return };
    let chunks: Vec<(i32, i32)> = chunks_of(square(VILLAGE_CHUNK, 9)).collect();
    let old = warm_generator(dim(w, Some(SEEDS[0])), WalkLeads::OLD, &chunks, false);
    let new = warm_generator(dim(w, Some(SEEDS[0])), WalkLeads::NEW, &chunks, false);
    let three_walks_copied = |dg: &DimGen| {
        let (sg, t) = parts(dg);
        for &(cx, cz) in &chunks {
            let beard = referenced_starts_cloned(&sg.references(cx, cz, t, &dg.blocks));
            let compound = sg.references(cx, cz, t, &dg.blocks);
            let decoration = referenced_starts_cloned(&sg.references(cx, cz, t, &dg.blocks));
            std::hint::black_box((beard, compound, decoration));
        }
    };
    let one_walk = |dg: &DimGen, borrowed: bool| {
        let (sg, t) = parts(dg);
        for &(cx, cz) in &chunks {
            let references = sg.references(cx, cz, t, &dg.blocks);
            if borrowed {
                std::hint::black_box((referenced_starts(&references), referenced_starts(&references)));
            } else {
                std::hint::black_box((referenced_starts_cloned(&references), referenced_starts_cloned(&references)));
            }
        }
    };
    let report = |label: &str, work: &dyn Fn()| {
        let allocations_before = crate::test_alloc::allocations();
        work();
        let allocations = (crate::test_alloc::allocations() - allocations_before) as f64 / chunks.len() as f64;
        let micros = best_micros(15, work) / chunks.len() as f64;
        eprintln!("  {label:<48} {micros:>8.2} µs, {allocations:>7.1} allocations per chunk");
    };
    eprintln!("references of a new chunk around a village ({} chunks):", chunks.len());
    report("old: three old walks, starts copied", &|| three_walks_copied(&old));
    report("three new walks, starts copied", &|| three_walks_copied(&new));
    report("one old walk, starts copied", &|| one_walk(&old, false));
    report("one old walk, starts borrowed", &|| one_walk(&old, true));
    report("new: one new walk, starts borrowed", &|| one_walk(&new, true));
}

/// Micro-bench of the first walks of a new world (start cache empty, rings resolved): the
/// references of the 1,024 chunks around the spawn asked from 1, 2, 4… threads at once, as the
/// first terrain batches do. Wall time, start lists computed, and how many of those a thread
/// computed while another was already computing the same chunk.
/// `cargo test --release -p rc-worldgen --lib bench_cold_references -- --ignored --nocapture --test-threads=1`.
#[test]
#[ignore = "benchmark"]
fn bench_cold_references_by_threads() {
    let Some(w) = world_data() else { return };
    let chunks: Vec<(i32, i32)> = chunks_of(square((0, 0), 32)).collect();
    for (world, make) in [("main world", dim as fn(&WorldData, Option<i64>) -> DimGen), ("Nether", |w, s| nether(w, s.unwrap()))] {
        eprintln!("{world}, first walk of {} chunks (start cache empty):", chunks.len());
        for threads in thread_counts() {
            let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().unwrap();
            let mut line = format!("  {threads:>3} threads:");
            let all_but_computed_once = WalkLeads { starts_computed_once: false, ..WalkLeads::NEW };
            for (label, leads) in [("old", WalkLeads::OLD), ("new, each thread computes", all_but_computed_once), ("new", WalkLeads::NEW)] {
                let timed = warm_generator(make(w, Some(SEEDS[0])), leads, &[], false);
                let millis = best_micros(1, || walk_in_pool(&timed, &chunks, &pool)) / 1e3;
                let counting = warm_generator(make(w, Some(SEEDS[0])), leads, &[], true);
                let counted = parts(&counting).0;
                take_counts(counted);
                walk_in_pool(&counting, &chunks, &pool);
                let computed = take_counts(counted)[7];
                let held = counted.starts.chunks().len() as u64;
                line += &format!(" {label} {millis:>8.1} ms, {computed} lists computed for {held} chunks ({} twice or more) |", computed - held);
            }
            eprintln!("{line}");
        }
    }
}

/// First walk of 1,024 chunks from 32 threads at once, start cache empty: with "computed once"
/// every chunk's list is computed exactly once (without it, several threads compute the same
/// chunk), and the references are those of the old path walked on one thread.
#[test]
fn starts_computed_once_by_32_threads() {
    use rayon::prelude::*;
    let Some(w) = world_data() else { return };
    let chunks: Vec<(i32, i32)> = chunks_of(square((0, 0), 32)).collect();
    let pool = rayon::ThreadPoolBuilder::new().num_threads(32).build().unwrap();
    for make in [dim as fn(&WorldData, Option<i64>) -> DimGen, |w, s| nether(w, s.unwrap())] {
        let old = WalkLeads::OLD.applied_to(make(w, Some(SEEDS[0])));
        let (old_sg, old_t) = parts(&old);
        let expected: Vec<String> =
            chunks.iter().map(|&(cx, cz)| refs_stable(&old_sg.references(cx, cz, old_t, &old.blocks))).collect();
        for leads in [WalkLeads::NEW, WalkLeads { starts_computed_once: true, ..WalkLeads::OLD }] {
            let new = warm_generator(make(w, Some(SEEDS[0])), leads, &[], true);
            let (sg, t) = parts(&new);
            take_counts(sg);
            let got: Vec<String> = pool
                .install(|| chunks.par_iter().map(|&(cx, cz)| refs_stable(&sg.references(cx, cz, t, &new.blocks))).collect());
            let (computed, held) = (take_counts(sg)[7], sg.starts.chunks().len() as u64);
            assert_eq!(computed, held, "{}: a start list computed more than once", new.dimension);
            assert!(got == expected, "{}: references differ from the old path", new.dimension);
            assert_eq!(sg.starts.chunks(), old_sg.starts.chunks(), "{}: other chunks in the start cache", new.dimension);
        }
    }
}

/// Runs `work` on another thread and fails if it has not finished after 60 s (a wait that never
/// ends must fail the test, not hang it).
fn within_a_minute<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    let (done, result) = std::sync::mpsc::channel();
    std::thread::spawn(move || done.send(work()));
    result.recv_timeout(std::time::Duration::from_secs(60)).expect("still waiting after 60 s, or panicked")
}

/// A computation that asks for the chunk it is computing (work picked up from the pool inside
/// it) gets a list of its own instead of waiting for itself; the cache keeps the outer list.
/// After a computation that panicked, the next thread computes.
#[test]
fn start_computed_once_never_waits_for_itself() {
    let Some(w) = world_data() else { return };
    let (outer_list, inner_list) = within_a_minute(move || {
        let dg = WalkLeads::NEW.applied_to(dim(w, Some(SEEDS[0])));
        let sg = parts(&dg).0;
        let (outer, inner): (super::Starts, super::Starts) = (Arc::new(vec![start_of(0, 1)]), Arc::new(vec![start_of(0, 2)]));
        let got = sg.computed_once(
            900_000,
            900_000,
            || {
                let nested = sg.computed_once(900_000, 900_000, || inner.clone(), Arc::clone);
                assert!(Arc::ptr_eq(&nested, &inner), "the nested request did not get its own list");
                outer.clone()
            },
            Arc::clone,
        );
        assert!(Arc::ptr_eq(&got, &outer), "outer request: another list than the one it computed");
        let kept = sg.computed_once(900_000, 900_000, || unreachable!("already in the cache"), Arc::clone);
        assert!(Arc::ptr_eq(&kept, &outer), "the cache did not keep the outer list");
        (outer[0].cx, inner[0].cx)
    });
    assert_eq!((outer_list, inner_list), (1, 2));

    let after_panic = within_a_minute(move || {
        let dg = WalkLeads::NEW.applied_to(dim(w, Some(SEEDS[0])));
        let sg = parts(&dg).0;
        let list: super::Starts = Arc::new(vec![start_of(0, 3)]);
        std::thread::scope(|scope| {
            let panicked = scope.spawn(|| sg.computed_once(7, 7, || panic!("computation failed (expected by the test)"), Arc::clone));
            assert!(panicked.join().is_err(), "the failed computation did not propagate");
            let got = scope.spawn(|| sg.computed_once(7, 7, || list.clone(), Arc::clone)).join().expect("second computation");
            Arc::ptr_eq(&got, &list)
        })
    });
    assert!(after_panic, "after a failed computation, the next thread did not compute");
}
