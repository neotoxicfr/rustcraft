//! Equivalence proofs of the references through a chunk's terrain and its decoration: one walk
//! for the beard and the saved compound, the walk's references used again at decoration, starts
//! borrowed instead of copied. Game data required (jar 26.3 and rc-data cache); absent: tests
//! skipped, except `RUSTCRAFT_OPT_REQUIRE=1` (explicit failure).

use std::sync::atomic::Ordering;

use hashbrown::HashMap;
use rayon::prelude::*;

use super::tests::{SEEDS, VILLAGE_CHUNK, WalkLeads, chunks_of, parts, square};
use crate::chunkgen::{DimGen, ReferenceUse, TerrainReferences, WorldData};
use crate::decor_region::DecoChunk;
use crate::structure::tests::{dim, nether, world_data};
use crate::structure::{Adapt, Start};
use crate::structure_beard::Beard;
use crate::terrain::BiomeCache;

/// Each lead of the use of the references alone on top of the old path, then all together (the
/// kept references need the single walk that hands them out).
const USES: [(&str, ReferenceUse); 4] = [
    ("one walk for the terrain", ReferenceUse { one_walk_for_terrain: true, ..ReferenceUse::OLD }),
    ("kept for the decoration", ReferenceUse { one_walk_for_terrain: true, kept_for_decoration: true, ..ReferenceUse::OLD }),
    ("borrowed starts", ReferenceUse { borrowed_starts: true, ..ReferenceUse::OLD }),
    ("all", ReferenceUse::NEW),
];

/// Generator of the old paths everywhere (walk, use of the references).
fn old_generator(dg: DimGen) -> DimGen {
    let mut dg = WalkLeads::OLD.applied_to(dg);
    dg.reference_use = ReferenceUse::OLD;
    dg
}

/// Generator of the new walk with the given use of the references.
fn new_generator(dg: DimGen, reference_use: ReferenceUse) -> DimGen {
    let mut dg = WalkLeads::NEW.applied_to(dg);
    dg.reference_use = reference_use;
    dg
}

/// Builds the generator of a world of the tests.
type MakeWorld = fn(&WorldData) -> DimGen;

fn main_world(w: &WorldData) -> DimGen {
    dim(w, Some(SEEDS[0]))
}

/// Everything a chunk holds (blocks, maps, biomes, marks, ticks, entities, saved compound).
fn chunk_text(c: &DecoChunk) -> String {
    format!("{c:?}")
}

fn fnv(text: &str) -> u64 {
    text.bytes().fold(0xcbf2_9ce4_8422_2325, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// A new chunk the old way: terrain (its own walk for the beard), then the compound (another walk).
fn terrain_then_compound(dg: &DimGen, (cx, cz): (i32, i32), cache: &BiomeCache) -> DecoChunk {
    let mut c = dg.terrain(cx, cz, cache, None);
    c.structures = Some(dg.structures_nbt(cx, cz));
    c
}

fn biomes_of(dg: &DimGen, chunks: &[(i32, i32)]) -> BiomeCache {
    let mut cache = BiomeCache::default();
    dg.prepare_biomes(&mut cache, chunks);
    cache
}

/// Chunks of the list whose terrain a structure shapes (a beard exists).
fn chunks_with_a_beard(dg: &DimGen, chunks: &[(i32, i32)]) -> usize {
    let (sg, t) = parts(dg);
    chunks
        .iter()
        .filter(|&&(cx, cz)| {
            let referenced = sg.referenced(cx, cz, t, &dg.blocks);
            let adapted = referenced.iter().any(|(s, starts)| sg.defs[*s].adapt != Adapt::None && !starts.is_empty());
            adapted && Beard::new(cx, cz, &referenced, &sg.defs).is_some()
        })
        .count()
}

/// New chunk through `terrain_with_structures` (each lead alone, then all) = terrain then
/// compound of the old path, everything the chunk holds, around the village of the test world
/// (beard), in the Nether and in the flat world (no beard); a compound read from disk is kept.
#[test]
fn terrain_with_structures_matches_terrain_then_compound() {
    let Some(w) = world_data() else { return };
    let worlds: [(&str, MakeWorld, (i32, i32)); 3] = [
        ("main world", main_world, VILLAGE_CHUNK),
        ("Nether", |w| nether(w, SEEDS[0]), (0, 0)),
        ("flat world", |w| dim(w, None), (0, 0)),
    ];
    for (world, make, center) in worlds {
        let chunks: Vec<(i32, i32)> = chunks_of(square(center, 5)).collect();
        let old = old_generator(make(w));
        let cache = biomes_of(&old, &chunks);
        let expected: Vec<String> = chunks.iter().map(|&p| chunk_text(&terrain_then_compound(&old, p, &cache))).collect();
        if world == "main world" {
            let beards = chunks_with_a_beard(&old, &chunks);
            eprintln!("{world}: {beards} of {} chunks have a beard", chunks.len());
            assert!(beards > 0, "no beard around the village: pointless comparison");
        }
        for (lead, reference_use) in USES {
            let new = new_generator(make(w), reference_use);
            let cache = biomes_of(&new, &chunks);
            for (&(cx, cz), expected) in chunks.iter().zip(&expected) {
                let (c, kept) = new.terrain_with_structures(cx, cz, &cache, None);
                assert!(chunk_text(&c) == *expected, "{world}, {lead}: chunk ({cx}, {cz}) differs");
                assert_eq!(kept.is_some(), reference_use.kept_for_decoration, "{world}, {lead}: references handed out");
            }
        }
        let new = new_generator(make(w), ReferenceUse::NEW);
        let saved = rc_proto::nbt::Nbt::Compound(vec![("starts".into(), rc_proto::nbt::Nbt::Compound(Vec::new()))]);
        let (c, _) = new.terrain_with_structures(center.0, center.1, &biomes_of(&new, &chunks), Some(saved.clone()));
        assert_eq!(c.structures.as_ref(), Some(&saved), "{world}: compound read from disk replaced");
    }
}

/// Stored fingerprint of the `structures` compounds (starts and references) of the 64 × 64
/// chunks around the origin of the test world, taken with the old path, before the fields the
/// game reads back from the template, stronghold and fortress pieces were added.
const COMPOUNDS_64X64_BEFORE_OFFICIAL_FIELDS: u64 = 0x69f9d6415271330f;

/// Same, full compounds (retaken when those fields were added).
const COMPOUNDS_64X64: u64 = 0x9b146eb8c13131e5;

/// Compounds of 64 × 64 new chunks through `terrain_with_structures` = those of
/// `structures_nbt` (old walk), chunk by chunk, and their fingerprint = the stored one; seen
/// without the added piece fields, the fingerprint taken before them.
#[test]
fn structures_compounds_match_over_64x64_chunks() {
    let Some(w) = world_data() else { return };
    let chunks: Vec<(i32, i32)> = chunks_of(square((0, 0), 64)).collect();
    let (old, new) = (old_generator(main_world(w)), new_generator(main_world(w), ReferenceUse::NEW));
    let cache = biomes_of(&new, &chunks);
    let compounds: Vec<(String, String, String)> = chunks
        .par_iter()
        .map(|&(cx, cz)| {
            let (c, _) = new.terrain_with_structures(cx, cz, &cache, None);
            let old_compound = old.structures_nbt(cx, cz);
            let before = crate::structure::tests::without_official_reader_fields(&old_compound);
            (format!("{:?}", c.structures.expect("compound")), format!("{old_compound:?}"), format!("{before:?}"))
        })
        .collect();
    let mut with_references = 0;
    let (mut all, mut all_before) = (String::new(), String::new());
    for (&(cx, cz), (new_compound, old_compound, before)) in chunks.iter().zip(&compounds) {
        assert_eq!(new_compound, old_compound, "chunk ({cx}, {cz})");
        with_references += usize::from(old_compound.contains("LongArray"));
        all += old_compound;
        all_before += before;
    }
    let (fingerprint, before) = (fnv(&all), fnv(&all_before));
    eprintln!(
        "{with_references} of {} compounds hold references, fingerprint 0x{fingerprint:016x} \
         (0x{before:016x} without the added piece fields)",
        chunks.len()
    );
    assert!(with_references > 0, "no reference: pointless comparison");
    assert_eq!(before, COMPOUNDS_64X64_BEFORE_OFFICIAL_FIELDS, "compounds moved beyond the added piece fields");
    assert_eq!(fingerprint, COMPOUNDS_64X64, "compounds differ from the stored fingerprint");
}

/// First start of a structure whose identifier `wanted` accepts, in the square of `radius`
/// chunks around the origin.
fn find_start(dg: &DimGen, wanted: impl Fn(&str) -> bool, radius: i32) -> Option<Start> {
    let sg = dg.structures.as_ref()?;
    chunks_of(square((0, 0), 2 * radius + 1))
        .find_map(|(x, z)| dg.structure_starts(x, z).iter().find(|s| wanted(&sg.defs[s.structure].id)).cloned())
}

/// Up to 6 chunks of the start's box, the start's own chunk first.
fn chunks_to_decorate(s: &Start) -> Vec<(i32, i32)> {
    let mut chunks: Vec<(i32, i32)> = (s.bbox.min[0] >> 4..=s.bbox.max[0] >> 4)
        .flat_map(|x| (s.bbox.min[2] >> 4..=s.bbox.max[2] >> 4).map(move |z| (x, z)))
        .collect();
    chunks.sort_by_key(|&(x, z)| ((x - s.cx).abs() + (z - s.cz).abs(), x, z));
    chunks.truncate(6);
    chunks
}

/// The chunks of a site as the generation thread holds them: terrain, and the references kept
/// from it.
struct Site {
    chunks: HashMap<(i32, i32), DecoChunk>,
    kept: HashMap<(i32, i32), TerrainReferences>,
}

impl Site {
    /// Terrain of `centers` and of their neighbours.
    fn new(dg: &DimGen, centers: &[(i32, i32)]) -> Site {
        let mut all: Vec<(i32, i32)> =
            centers.iter().flat_map(|&p| chunks_of(square(p, 3))).collect();
        all.sort_unstable();
        all.dedup();
        let cache = biomes_of(dg, &all);
        let (mut chunks, mut kept) = (HashMap::new(), HashMap::new());
        for &(cx, cz) in &all {
            let (c, references) = dg.terrain_with_structures(cx, cz, &cache, None);
            chunks.insert((cx, cz), c);
            kept.extend(references.map(|r| ((cx, cz), r)));
        }
        Site { chunks, kept }
    }

    /// Decorates the chunk `center` with the references `kept` (`None`: walked again).
    fn decorate_with(&mut self, dg: &DimGen, center: (i32, i32), kept: Option<&TerrainReferences>) {
        let mut region: Vec<DecoChunk> =
            (0..9).map(|i| self.chunks.remove(&(center.0 + i % 3 - 1, center.1 + i / 3 - 1)).expect("terrain")).collect();
        {
            let mut it = region.iter_mut();
            dg.decorate_reusing(std::array::from_fn(|_| it.next().expect("9 chunks")), kept);
        }
        self.chunks.extend(region.into_iter().map(|c| ((c.cx, c.cz), c)));
    }

    /// Decorates `center` the way the generation thread does: with its kept references, taken.
    fn decorate(&mut self, dg: &DimGen, center: (i32, i32)) {
        let kept = self.kept.remove(&center);
        self.decorate_with(dg, center, kept.as_ref());
    }

    /// Everything the chunks hold, in position order.
    fn text(&self) -> Vec<((i32, i32), String)> {
        let mut text: Vec<((i32, i32), String)> = self.chunks.iter().map(|(&p, c)| (p, chunk_text(c))).collect();
        text.sort_unstable_by_key(|(p, _)| *p);
        text
    }
}

/// A place decorated by the tests: the chunks to decorate, in order, in the world of `seed`.
struct DecoratedSite {
    name: String,
    seed: i64,
    centers: Vec<(i32, i32)>,
}

/// Sites decorated: the village of the test world (jigsaw pieces over many chunks, beard), a
/// desert pyramid and a swamp hut (scattered pieces: their state is kept in the start objects
/// and written back to the compound), found on the first seed that has them.
fn decorated_sites(w: &WorldData) -> Vec<DecoratedSite> {
    let village = chunks_of(square(VILLAGE_CHUNK, 3)).take(6).collect();
    let mut sites = vec![DecoratedSite { name: "village".to_owned(), seed: SEEDS[0], centers: village }];
    for id in ["minecraft:desert_pyramid", "minecraft:swamp_hut"] {
        let found = [12345, SEEDS[0], SEEDS[1], 42, SEEDS[2], 7]
            .into_iter()
            .find_map(|seed| find_start(&dim(w, Some(seed)), |s| s == id, 160).map(|s| (seed, s)));
        let (seed, start) = found.unwrap_or_else(|| panic!("{id} not found"));
        sites.push(DecoratedSite { name: id.to_owned(), seed, centers: chunks_to_decorate(&start) });
    }
    sites
}

/// Decoration through the new uses of the references (each lead alone, then all) = decoration of
/// the old path, everything the chunks hold, on a village, a desert pyramid and a swamp hut.
#[test]
fn decoration_matches_through_every_reference_use() {
    let Some(w) = world_data() else { return };
    for DecoratedSite { name: site, seed, centers } in decorated_sites(w) {
        let old = old_generator(dim(w, Some(seed)));
        let mut expected = Site::new(&old, &centers);
        let terrain_text = expected.text();
        for &center in &centers {
            expected.decorate(&old, center);
        }
        let expected = expected.text();
        assert!(expected != terrain_text, "{site}: decoration changed nothing");
        for (lead, reference_use) in USES {
            let new = new_generator(dim(w, Some(seed)), reference_use);
            let mut got = Site::new(&new, &centers);
            assert_eq!(got.kept.len(), if reference_use.kept_for_decoration { got.chunks.len() } else { 0 }, "{site}, {lead}");
            for &center in &centers {
                got.decorate(&new, center);
            }
            for ((p, got), (_, expected)) in got.text().iter().zip(&expected) {
                assert!(got == expected, "{site}, {lead}: chunk {p:?} differs after decoration");
            }
        }
    }
}

/// The kept references are what the placement reads while they are current (emptied by hand: the
/// village is not built), and are walked again once stale: after a clear of the start cache, or
/// handed to another chunk than theirs.
#[test]
fn kept_references_used_only_while_current() {
    let Some(w) = world_data() else { return };
    let dg = new_generator(main_world(w), ReferenceUse::NEW);
    let (sg, _) = parts(&dg);
    let center = VILLAGE_CHUNK;
    let decorated = |emptied: bool, clear_cache: bool, kept_of: (i32, i32)| {
        let mut site = Site::new(&dg, &[center]);
        let mut kept = site.kept.remove(&kept_of).expect("kept references");
        if emptied {
            assert!(!kept.references_mut().is_empty(), "the village chunk references nothing");
            kept.references_mut().clear();
        }
        if clear_cache {
            sg.starts.clear();
            sg.starts_clears.fetch_add(1, Ordering::Relaxed);
        }
        site.decorate_with(&dg, center, Some(&kept));
        site.text()
    };
    let walked_again = {
        let mut site = Site::new(&dg, &[center]);
        site.decorate_with(&dg, center, None);
        site.text()
    };
    assert!(decorated(false, false, center) == walked_again, "kept references: other decoration than a new walk");
    assert!(decorated(true, false, center) != walked_again, "emptied references not seen: the kept ones are not read");
    assert!(decorated(true, true, center) == walked_again, "stale references used after a clear of the start cache");
    let neighbour = (center.0 + 1, center.1);
    assert!(decorated(true, false, neighbour) == walked_again, "references of another chunk used");
}
