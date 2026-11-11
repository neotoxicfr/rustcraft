//! Structures (`generation-decoration.md` § 7–9): sets and placements, starts per chunk,
//! references, contribution to the terrain ("beard"). The structure types are in
//! `structure_*.rs`; piece placement during decoration in `structure_place.rs`.

use hashbrown::{HashMap, HashSet};
use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use rayon::prelude::*;

use crate::Error;
use crate::biome::Searcher;
use crate::carver::LargeFeatureSeed;
use crate::data::full_id;
use crate::decor_blocks::Blocks;
use crate::decor_data::DecorData;
use crate::decor_pred::GenCtx;
use crate::decor_rng::DecoRng;
use crate::jigsaw::{JigsawCfg, JigsawPiece, Pools};
use crate::json::Json;
use crate::sampler::Ctx;
use crate::template::{BBox, Templates};
use crate::terrain::TerrainGenerator;

/// Named decoration steps (`step` of the structures).
pub const STEP_NAMES: [&str; 11] = [
    "raw_generation",
    "lakes",
    "local_modifications",
    "underground_structures",
    "surface_structures",
    "strongholds",
    "underground_ores",
    "underground_decoration",
    "fluid_springs",
    "vegetal_decoration",
    "top_layer_modification",
];

/// Terrain adaptation (§ 9.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Adapt {
    None,
    Bury,
    BeardThin,
    BeardBox,
    Encapsulate,
}

/// Structure type and its settings.
#[derive(Debug)]
pub enum Kind {
    Jigsaw(Box<JigsawCfg>),
    BuriedTreasure,
    Mineshaft {
        mesa: bool,
    },
    RuinedPortal(Vec<crate::structure_templ::PortalSetup>),
    Shipwreck {
        beached: bool,
    },
    OceanRuin {
        warm: bool,
        large: f32,
        cluster: f32,
    },
    DesertPyramid,
    JungleTemple,
    SwampHut,
    Igloo,
    Stronghold,
    Fortress,
    Mansion,
    EndCity,
    Monument(Vec<bool>),
    NetherFossil(crate::decor_pred::HeightP),
    /// Type recognised but not reproduced yet (no start).
    Unsupported(String),
}

#[derive(Debug)]
pub struct StructureDef {
    pub id: String,
    pub kind: Kind,
    /// Valid biomes (generator indices).
    pub biomes: Vec<bool>,
    pub step: usize,
    pub adapt: Adapt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reducer {
    Default,
    Legacy1,
    Legacy2,
    Legacy3,
}

#[derive(Debug)]
pub enum Placement {
    Random {
        spacing: i32,
        separation: i32,
        triangular: bool,
        salt: i32,
        frequency: f32,
        reducer: Reducer,
        /// (excluded set, radius in chunks).
        exclusion: Option<(String, i32)>,
    },
    Rings {
        distance: i32,
        spread: i32,
        count: i32,
        salt: i32,
        frequency: f32,
        reducer: Reducer,
        preferred: Vec<bool>,
        /// Ring draws (without noise) and positions resolved on demand.
        jobs: RingJobs,
        /// Exclusion zone, common to the game's two distributions.
        exclusion: Option<(String, i32)>,
    },
    /// Origin chunk of the dimension only (`StructureGen::origin`).
    Origin,
}

/// Optimisation switch `RUSTCRAFT_OPT_<name>` read once: `0` old path, `1` new,
/// other or absent: `default`.
fn opt_switch(cell: &'static OnceLock<bool>, name: &str, default: bool) -> bool {
    *cell.get_or_init(|| match rc_data::env_flags::read_os(name).and_then(|v| v.into_string().ok()).as_deref() {
        Some("0") => false,
        Some("1") => true,
        _ => default,
    })
}

/// Rings resolved stronghold by stronghold, at the first nearby chunk (`RUSTCRAFT_OPT_RINGS_LAZY`, default 1).
fn rings_lazy() -> bool {
    static C: OnceLock<bool> = OnceLock::new();
    opt_switch(&C, "RUSTCRAFT_OPT_RINGS_LAZY", true)
}

/// Start kept: the first inserted serves everyone (`RUSTCRAFT_OPT_STARTS_ORINSERT`, default 1).
pub(crate) fn starts_or_insert() -> bool {
    static C: OnceLock<bool> = OnceLock::new();
    opt_switch(&C, "RUSTCRAFT_OPT_STARTS_ORINSERT", true)
}

/// Chunks without start: one shared empty list (`RUSTCRAFT_OPT_STARTS_EMPTYARC`, default 1).
fn starts_empty_arc() -> bool {
    static C: OnceLock<bool> = OnceLock::new();
    opt_switch(&C, "RUSTCRAFT_OPT_STARTS_EMPTYARC", true)
}

/// Biomes of a start attempt with a kept context (`RUSTCRAFT_OPT_GENSTART_CTX`, default 1).
fn genstart_ctx() -> bool {
    static C: OnceLock<bool> = OnceLock::new();
    opt_switch(&C, "RUSTCRAFT_OPT_GENSTART_CTX", true)
}

/// References: neighbour skipped without touching the cache when no set places a structure there
/// (`RUSTCRAFT_OPT_STARTS_PREFILTRE`, default 1).
fn starts_prefilter() -> bool {
    static C: OnceLock<bool> = OnceLock::new();
    opt_switch(&C, "RUSTCRAFT_OPT_STARTS_PREFILTRE", true)
}

/// Maximum difference (in chunks, per axis) between a stronghold's draw and its position: the
/// biome search covers 112 blocks around the centre of the drawn chunk, in quarts of 4 blocks, i.e. blocks
/// 16·x0 − 104 to 16·x0 + 120, hence chunks x0 − 7 to x0 + 7; the fallback stays at x0.
pub const RING_REACH: u32 = 7;

/// Draws of the concentric rings (drawn centre and search seed of each stronghold) and
/// positions resolved only once per stronghold, on demand.
#[derive(Debug, Default)]
pub struct RingJobs {
    jobs: Vec<(i32, i32, i64)>,
    cells: Vec<OnceLock<(i32, i32)>>,
    all: OnceLock<Arc<Vec<(i32, i32)>>>,
}

impl RingJobs {
    /// Game draws (§ 7.2): angle, distance per ring, search seed per stronghold.
    fn draw(seed: i64, distance: i32, spread: i32, count: i32) -> RingJobs {
        let mut jobs: Vec<(i32, i32, i64)> = Vec::new();
        if count > 0 {
            let mut r = DecoRng::legacy(0);
            r.set_seed(seed);
            let mut a = r.next_double() * std::f64::consts::PI * 2.0;
            let (mut k, mut c, mut s) = (0, 0, spread);
            for i in 0..count {
                let d =
                    f64::from(4 * distance + distance * c * 6) + (r.next_double() - 0.5) * (f64::from(distance) * 2.5);
                // Java rounding: half towards +∞ (Rust's `round`: half away from zero).
                let x0 = (crate::jmath::cos(a) * d + 0.5).floor() as i32;
                let z0 = (crate::jmath::sin(a) * d + 0.5).floor() as i32;
                let fork = r.next_long();
                jobs.push((x0, z0, fork));
                a += std::f64::consts::PI * 2.0 / f64::from(s);
                k += 1;
                if k == s {
                    c += 1;
                    k = 0;
                    s += 2 * s / (c + 1);
                    s = s.min(count - i);
                    a += r.next_double() * std::f64::consts::PI * 2.0;
                }
            }
        }
        let cells = jobs.iter().map(|_| OnceLock::new()).collect();
        RingJobs { jobs, cells, all: OnceLock::new() }
    }

    /// Draws (centre, seed), in the game's order.
    pub fn jobs(&self) -> &[(i32, i32, i64)] {
        &self.jobs
    }
}

#[derive(Debug)]
pub struct StructureSet {
    pub id: String,
    /// (structure index, weight).
    pub structures: Vec<(usize, i32)>,
    pub placement: Placement,
}

/// Piece of a start.
#[derive(Debug, Clone)]
pub enum Piece {
    Jigsaw(Box<JigsawPiece>),
    /// Coded pieces (types being reproduced): box only.
    Coded {
        id: &'static str,
        bbox: BBox,
        data: Arc<crate::structure_coded::CodedPiece>,
    },
}

impl Piece {
    pub fn bbox(&self) -> BBox {
        match self {
            Piece::Jigsaw(p) => p.bbox,
            Piece::Coded { bbox, .. } => *bbox,
        }
    }
}

/// Valid structure start in a chunk.
#[derive(Debug, Clone)]
pub struct Start {
    pub structure: usize,
    pub cx: i32,
    pub cz: i32,
    pub pieces: Vec<Piece>,
    /// Bounding box of the pieces (without the adaptation enlargement).
    pub bbox: BBox,
}

impl Start {
    /// Box of the references: enlarged by 12 if the structure adapts the terrain.
    pub fn ref_box(&self, defs: &[StructureDef]) -> BBox {
        if defs[self.structure].adapt != Adapt::None { self.bbox.inflated(12) } else { self.bbox }
    }
}

type Rings = Arc<Vec<(i32, i32)>>;
type Starts = Arc<Vec<Start>>;

/// Structure generator of a dimension.
pub struct StructureGen {
    pub defs: Vec<StructureDef>,
    pub sets: Vec<StructureSet>,
    pub pools: Pools,
    pub templates: Arc<Templates>,
    pub seed: i64,
    /// Ring positions per set, old path (`RUSTCRAFT_OPT_RINGS_LAZY=0`).
    rings: Mutex<HashMap<usize, Rings>>,
    starts: start_cache::StartCache,
    /// Stronghold biome searches launched (both paths): bench and proofs.
    ring_searches: AtomicUsize,
    /// Clears of the start cache (threshold of 200,000 chunks reached).
    starts_clears: AtomicUsize,
    /// Paths of the reference walk (switches read at construction; the equivalence tests set them).
    pub reference_paths: ReferencePaths,
    /// What the "large structure" seed of a chunk draws from the world seed alone, drawn once.
    large_feature_seed: LargeFeatureSeed,
    /// `REDUCER_SEED_DRAWN_ONCE`: the frequency reducer seeded that way (mineshafts) reads
    /// [`Self::large_feature_seed`] instead of drawing it again for every chunk.
    pub reducer_seed_drawn_once: bool,
    /// `STARTS_COMPUTED_ONCE`: a chunk's start list missing from the cache is computed by the
    /// first thread that asks, the others wait for it.
    pub starts_computed_once: bool,
    /// Bench counters, when this generator counts (tests only).
    #[cfg(test)]
    pub(crate) walk_counts: Option<Box<WalkCounts>>,
    /// Structures per step (registry order).
    pub by_step: Vec<Vec<usize>>,
    pub flat: bool,
    pub procs: Arc<crate::structure_place::ProcLists>,
    /// Biomes where mineshafts are not placed.
    pub mineshaft_blocking: Vec<bool>,
    /// Decorator (biome temperature: "cold" portals).
    pub deco: Option<Arc<crate::decor::Decorator>>,
    /// Villager type per biome (generator indices), for the template entities.
    pub villager_types: Arc<[&'static str]>,
    /// Origin chunk of the dimension (placement "dimension_origin"): that of the spawn
    /// point found on the noise, (0, 0) without spawn target or in a flat world.
    pub origin: (i32, i32),
}

impl std::fmt::Debug for StructureGen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "StructureGen({} structures, {} sets)", self.defs.len(), self.sets.len())
    }
}

/// Context of a start generation (§ 7.3).
pub struct GenStart<'a> {
    pub sg: &'a StructureGen,
    pub t: &'a TerrainGenerator,
    pub b: &'a Blocks,
    pub cx: i32,
    pub cz: i32,
    pub rng: DecoRng,
    pub hctx: GenCtx,
    pub def: &'a StructureDef,
    /// Kept biome context of the attempt (`RUSTCRAFT_OPT_GENSTART_CTX`), otherwise a new one per point.
    pub bctx: Option<RefCell<Ctx>>,
    /// Every biome accepted (`place structure`, jigsaw block: the game's test accepts all).
    pub any_biome: bool,
}

impl GenStart<'_> {
    /// Raw biome (without zoom) at the point. New searcher at each call (its tie-breaking
    /// depends on the last result: do not share it).
    pub fn biome_at(&self, x: i32, y: i32, z: i32) -> u16 {
        #[cfg(test)]
        self.sg.count(|c| &c.start_biome_reads, 1);
        let (qx, qy, qz) = (x >> 2, y >> 2, z >> 2);
        match &self.bctx {
            Some(c) => self.t.source.biome_at(&self.t.g, &mut c.borrow_mut(), &mut Searcher::default(), qx, qy, qz),
            None => self.t.source.biome_at(&self.t.g, &mut Ctx::uncached(), &mut Searcher::default(), qx, qy, qz),
        }
    }

    pub fn valid_biome(&self, p: [i32; 3]) -> bool {
        self.biome_ok(p[0], p[1], p[2])
    }

    /// The structure accepts the biome at the point (every one for a placement by command).
    pub fn biome_ok(&self, x: i32, y: i32, z: i32) -> bool {
        self.any_biome || self.def.biomes.get(self.biome_at(x, y, z) as usize).copied().unwrap_or(false)
    }

    /// Ground height (first free y) without decoration.
    pub fn first_free(&self, x: i32, z: i32, ocean_floor: bool) -> i32 {
        self.t.base_height(x, z, ocean_floor)
    }
}

/// Biomes of a list or a tag (`#ns:name`), resolved into generator indices.
pub fn biome_set(d: &DecorData, j: Option<&Json>, names: &[String]) -> Vec<bool> {
    let mut out = vec![false; names.len()];
    let mut ids: Vec<String> = Vec::new();
    let mut stack: Vec<Json> = j.cloned().into_iter().collect();
    let mut seen = HashSet::new();
    while let Some(v) = stack.pop() {
        match v {
            Json::Arr(a) => stack.extend(a),
            Json::Str(s) => {
                if let Some(tag) = s.strip_prefix('#') {
                    if seen.insert(tag.to_owned())
                        && let Ok(t) = d.json("tags_worldgen_biome", tag)
                        && let Some(vals) = t.get("values")
                    {
                        stack.push(vals.clone());
                    }
                } else {
                    ids.push(full_id(&s));
                }
            }
            Json::Obj(m) => {
                if let Some(id) = m.get("id") {
                    stack.push(id.clone());
                }
            }
            _ => {}
        }
    }
    for id in ids {
        if let Some(i) = names.iter().position(|n| *n == id) {
            out[i] = true;
        }
    }
    out
}

/// [`biome_set`] for the many lists of one generator (34 structures, the rings, the mineshafts):
/// each biome tag file is parsed once instead of once per list that reaches it, and a biome name
/// is found by a hashed lookup instead of a walk of the names (`RUSTCRAFT_OPT_BIOME_TAG_CACHE`).
struct BiomeLists<'a> {
    d: &'a DecorData,
    names: &'a [String],
    /// Off: every list goes through [`biome_set`] (old path).
    cached: bool,
    /// Entries (`values`) of each tag file already read; `None`: no such file, or without entries.
    tag_entries: HashMap<String, Option<Json>>,
    index_of_name: HashMap<&'a str, usize>,
}

impl<'a> BiomeLists<'a> {
    fn new(d: &'a DecorData, names: &'a [String], cached: bool) -> Self {
        // Reversed: a name listed twice keeps its first index, the one a walk of the names finds.
        let index_of_name =
            if cached { names.iter().enumerate().rev().map(|(i, n)| (n.as_str(), i)).collect() } else { HashMap::new() };
        BiomeLists { d, names, cached, tag_entries: HashMap::new(), index_of_name }
    }

    /// Biomes of a list or a tag, as generator indices: the same walk as [`biome_set`].
    fn resolve(&mut self, j: Option<&Json>) -> Vec<bool> {
        if !self.cached {
            return biome_set(self.d, j, self.names);
        }
        let mut out = vec![false; self.names.len()];
        let mut stack: Vec<Json> = j.cloned().into_iter().collect();
        let mut seen = HashSet::new();
        while let Some(v) = stack.pop() {
            match v {
                Json::Arr(a) => stack.extend(a),
                Json::Str(s) => match s.strip_prefix('#') {
                    Some(tag) => {
                        if seen.insert(tag.to_owned())
                            && let Some(entries) = self.entries_of_tag(tag)
                        {
                            stack.push(entries.clone());
                        }
                    }
                    None => {
                        if let Some(&i) = self.index_of_name.get(full_id(&s).as_str()) {
                            out[i] = true;
                        }
                    }
                },
                Json::Obj(m) => {
                    if let Some(id) = m.get("id") {
                        stack.push(id.clone());
                    }
                }
                _ => {}
            }
        }
        out
    }

    fn entries_of_tag(&mut self, tag: &str) -> Option<&Json> {
        if !self.tag_entries.contains_key(tag) {
            let entries = self.d.json("tags_worldgen_biome", tag).ok().and_then(|t| t.get("values").cloned());
            self.tag_entries.insert(tag.to_owned(), entries);
        }
        self.tag_entries.get(tag)?.as_ref()
    }
}

/// Exclusion zone (excluded set, radius in chunks from 1 to 16).
fn exclusion(p: &Json) -> Option<(String, i32)> {
    let e = p.get("exclusion_zone")?;
    Some((full_id(e.get("other_set")?.as_str()?), e.get("chunk_count")?.as_i32()?))
}

fn reducer(j: &Json) -> Reducer {
    match j.get("frequency_reduction_method").and_then(Json::as_str) {
        Some("legacy_type_1") => Reducer::Legacy1,
        Some("legacy_type_2") => Reducer::Legacy2,
        Some("legacy_type_3") => Reducer::Legacy3,
        _ => Reducer::Default,
    }
}

impl StructureGen {
    /// `flat`: flat world (ring seed zero); `procs`: processor lists of the data (shared table).
    pub fn new(
        d: &DecorData,
        b: &Blocks,
        templates: Arc<Templates>,
        procs: Arc<crate::structure_place::ProcLists>,
        biome_names: &[String],
        seed: i64,
        flat: bool,
    ) -> Result<StructureGen, Error> {
        let pools = Pools::new(d, b);
        let mut biome_lists = BiomeLists::new(d, biome_names, crate::opt::biome_tag_cache());
        let mut defs = Vec::new();
        let mut index = HashMap::new();
        for id in d.ids("structure") {
            let j = d.json("structure", &id)?;
            let typ = j.get("type").and_then(Json::as_str).unwrap_or("").to_owned();
            let step =
                j.get("step").and_then(Json::as_str).and_then(|s| STEP_NAMES.iter().position(|n| *n == s)).unwrap_or(4);
            let adapt = match j.get("terrain_adaptation").and_then(Json::as_str) {
                Some("bury") => Adapt::Bury,
                Some("beard_thin") => Adapt::BeardThin,
                Some("beard_box") => Adapt::BeardBox,
                Some("encapsulate") => Adapt::Encapsulate,
                _ => Adapt::None,
            };
            let kind = match typ.as_str() {
                "minecraft:jigsaw" => Kind::Jigsaw(Box::new(JigsawCfg::parse(&j)?)),
                "minecraft:buried_treasure" => Kind::BuriedTreasure,
                "minecraft:mineshaft" => {
                    Kind::Mineshaft { mesa: j.get("mineshaft_type").and_then(Json::as_str) == Some("mesa") }
                }
                "minecraft:ruined_portal" => Kind::RuinedPortal(crate::structure_templ::PortalSetup::parse_all(&j)),
                "minecraft:shipwreck" => {
                    Kind::Shipwreck { beached: j.get("is_beached").and_then(Json::as_bool).unwrap_or(false) }
                }
                "minecraft:ocean_ruin" => Kind::OceanRuin {
                    warm: j.get("biome_temp").and_then(Json::as_str) == Some("warm"),
                    large: j.get("large_probability").and_then(Json::as_f32).unwrap_or(0.0),
                    cluster: j.get("cluster_probability").and_then(Json::as_f32).unwrap_or(0.0),
                },
                "minecraft:desert_pyramid" => Kind::DesertPyramid,
                "minecraft:jungle_temple" => Kind::JungleTemple,
                "minecraft:swamp_hut" => Kind::SwampHut,
                "minecraft:igloo" => Kind::Igloo,
                "minecraft:stronghold" => Kind::Stronghold,
                "minecraft:fortress" => Kind::Fortress,
                "minecraft:woodland_mansion" => Kind::Mansion,
                "minecraft:end_city" => Kind::EndCity,
                "minecraft:ocean_monument" => Kind::Monument(
                    biome_lists.resolve(Some(&Json::Str("#minecraft:required_ocean_monument_surrounding".into()))),
                ),
                "minecraft:nether_fossil" => match j.get("height").map(crate::decor_pred::HeightP::parse) {
                    Some(Ok(h)) => Kind::NetherFossil(h),
                    _ => Kind::Unsupported("minecraft:nether_fossil".into()),
                },
                other => Kind::Unsupported(other.to_owned()),
            };
            index.insert(id.clone(), defs.len());
            defs.push(StructureDef { biomes: biome_lists.resolve(j.get("biomes")), id, kind, step, adapt });
        }
        let mut sets = Vec::new();
        for id in d.ids("structure_set") {
            let j = d.json("structure_set", &id)?;
            let structures: Vec<(usize, i32)> = j
                .get("structures")
                .and_then(Json::as_array)
                .unwrap_or(&[])
                .iter()
                .filter_map(|e| {
                    let s = full_id(e.get("structure")?.as_str()?);
                    Some((*index.get(&s)?, e.get("weight").and_then(Json::as_i32).unwrap_or(1)))
                })
                .collect();
            let Some(p) = j.get("placement") else { continue };
            let freq = p.get("frequency").and_then(Json::as_f32).unwrap_or(1.0);
            let salt = p.get("salt").and_then(Json::as_i32).unwrap_or(0);
            let placement = match p.get("type").and_then(Json::as_str) {
                Some("minecraft:random_spread") => Placement::Random {
                    spacing: p.get("spacing").and_then(Json::as_i32).unwrap_or(1),
                    separation: p.get("separation").and_then(Json::as_i32).unwrap_or(0),
                    triangular: p.get("spread_type").and_then(Json::as_str) == Some("triangular"),
                    salt,
                    frequency: freq,
                    reducer: reducer(p),
                    exclusion: exclusion(p),
                },
                Some("minecraft:concentric_rings") => {
                    let distance = p.get("distance").and_then(Json::as_i32).unwrap_or(32);
                    let spread = p.get("spread").and_then(Json::as_i32).unwrap_or(3);
                    let count = p.get("count").and_then(Json::as_i32).unwrap_or(128);
                    Placement::Rings {
                        distance,
                        spread,
                        count,
                        salt,
                        frequency: freq,
                        reducer: reducer(p),
                        preferred: biome_lists.resolve(p.get("preferred_biomes")),
                        // Flat world: ring seed zero.
                        jobs: RingJobs::draw(if flat { 0 } else { seed }, distance, spread, count),
                        exclusion: exclusion(p),
                    }
                }
                Some("minecraft:dimension_origin") => Placement::Origin,
                // Unknown type: the game would refuse the data; the set is ignored.
                _ => continue,
            };
            sets.push(StructureSet { id, structures, placement });
        }
        // Possible sets: at least one structure with a possible biome. All the
        // structures stay in the draw (a structure without biome here fails at the attempt).
        sets.retain(|s| s.structures.iter().any(|(i, _)| defs[*i].biomes.iter().any(|&b| b)));
        let mut by_step = vec![Vec::new(); STEP_NAMES.len()];
        for (i, d) in defs.iter().enumerate() {
            by_step[d.step].push(i);
        }
        Ok(StructureGen {
            defs,
            sets,
            pools,
            templates,
            seed,
            rings: Mutex::new(HashMap::new()),
            starts: start_cache::StartCache::new(crate::opt::sharded_starts_cache()),
            large_feature_seed: LargeFeatureSeed::new(seed, 0),
            reducer_seed_drawn_once: crate::opt::reducer_seed_drawn_once(),
            starts_computed_once: crate::opt::starts_computed_once(),
            ring_searches: AtomicUsize::new(0),
            starts_clears: AtomicUsize::new(0),
            reference_paths: ReferencePaths::from_env(),
            #[cfg(test)]
            walk_counts: None,
            by_step,
            flat,
            procs,
            mineshaft_blocking: biome_lists.resolve(Some(&Json::Str("#minecraft:mineshaft_blocking".into()))),
            deco: None,
            villager_types: villager_types(biome_names),
            origin: (0, 0),
        })
    }

    /// Flat world: allowed sets (`overrides`, in their order; identifiers or `#tag`),
    /// all otherwise; then only those where a structure accepts a biome of the source (the game
    /// filters the sets of the two generators this way; here, only the flat one has a reduced source).
    pub fn keep_flat_sets(&mut self, d: &DecorData, overrides: Option<&[String]>) {
        if let Some(list) = overrides {
            let mut ids: Vec<String> = Vec::new();
            for o in list {
                match o.strip_prefix('#') {
                    Some(tag) => {
                        if let Ok(t) = d.json("tags_worldgen_structure_set", tag)
                            && let Some(Json::Arr(v)) = t.get("values")
                        {
                            ids.extend(v.iter().filter_map(Json::as_str).map(full_id));
                        }
                    }
                    None => ids.push(full_id(o)),
                }
            }
            let mut kept = Vec::new();
            for id in &ids {
                if let Some(i) = self.sets.iter().position(|s| s.id == *id) {
                    kept.push(self.sets.swap_remove(i));
                }
            }
            self.sets = kept;
        }
        let defs = &self.defs;
        self.sets.retain(|s| s.structures.iter().any(|(k, _)| defs[*k].biomes.iter().any(|b| *b)));
    }

    fn set_index(&self, id: &str) -> Option<usize> {
        self.sets.iter().position(|s| s.id == id)
    }

    /// Frequency reducer (§ 7.2).
    fn reduce(&self, r: Reducer, salt: i32, cx: i32, cz: i32, p: f32) -> bool {
        #[cfg(test)]
        self.count(|c| &c.reducer_draws, 1);
        let g = self.seed;
        let mut rng = DecoRng::legacy(0);
        match r {
            Reducer::Default => {
                rng.set_seed(
                    i64::from(salt)
                        .wrapping_mul(341873128712)
                        .wrapping_add(i64::from(cx).wrapping_mul(132897987541))
                        .wrapping_add(g)
                        .wrapping_add(i64::from(cz)),
                );
                rng.next_float() < p
            }
            Reducer::Legacy1 => {
                rng.set_seed(i64::from(cx >> 4) ^ (i64::from(cz >> 4) << 4) ^ g);
                rng.next_int();
                rng.next_int_bounded((1.0 / p) as i32) == 0
            }
            Reducer::Legacy2 => {
                rng.set_seed(
                    i64::from(cx)
                        .wrapping_mul(341873128712)
                        .wrapping_add(i64::from(cz).wrapping_mul(132897987541))
                        .wrapping_add(g)
                        .wrapping_add(10387320),
                );
                rng.next_float() < p
            }
            Reducer::Legacy3 => {
                if self.reducer_seed_drawn_once {
                    // Two of the three seedings and four of the six draws only depend on the
                    // world seed: they are kept, a walk asks this for each of its 289 chunks.
                    return self.large_feature_seed.at(cx, cz).next_double() < f64::from(p);
                }
                rng.set_large_feature_seed(g, cx, cz);
                rng.next_double() < f64::from(p)
            }
        }
    }

    /// `is_structure_chunk` (§ 7.2), without biome check.
    pub fn is_structure_chunk(&self, set: usize, cx: i32, cz: i32, t: &TerrainGenerator) -> bool {
        self.is_structure_chunk_with(set, cx, cz, t, rings_lazy())
    }

    /// [`Self::is_structure_chunk`]; `lazy`: rings resolved only near the chunk.
    fn is_structure_chunk_with(&self, set: usize, cx: i32, cz: i32, t: &TerrainGenerator, lazy: bool) -> bool {
        let s = &self.sets[set];
        #[cfg(test)]
        self.count(|c| &c.per_chunk_tests, 1);
        match &s.placement {
            Placement::Random { spacing, separation, triangular, salt, frequency, reducer, exclusion } => {
                #[cfg(test)]
                self.count(|c| &c.region_draws, 1);
                let (gx, gz) = (cx.div_euclid(*spacing), cz.div_euclid(*spacing));
                let mut rng = DecoRng::legacy(0);
                rng.set_seed(
                    i64::from(gx)
                        .wrapping_mul(341873128712)
                        .wrapping_add(i64::from(gz).wrapping_mul(132897987541))
                        .wrapping_add(self.seed)
                        .wrapping_add(i64::from(*salt)),
                );
                let l = spacing - separation;
                let spread = |r: &mut DecoRng| {
                    if *triangular {
                        (r.next_int_bounded(l) + r.next_int_bounded(l)) / 2
                    } else {
                        r.next_int_bounded(l)
                    }
                };
                let ox = spread(&mut rng);
                let oz = spread(&mut rng);
                if (gx * spacing + ox, gz * spacing + oz) != (cx, cz) {
                    return false;
                }
                if *frequency < 1.0 && !self.reduce(*reducer, *salt, cx, cz, *frequency) {
                    return false;
                }
                !self.excluded(exclusion, cx, cz, t, lazy)
            }
            Placement::Rings { salt, frequency, reducer, preferred, jobs, exclusion, .. } => {
                #[cfg(test)]
                self.count(|c| &c.ring_draws_compared, if lazy { jobs.jobs.len() as u64 } else { 0 });
                let hit = if lazy {
                    // Only the strongholds drawn at ≤ RING_REACH chunks can land here.
                    jobs.jobs.iter().enumerate().any(|(i, &(x0, z0, _))| {
                        x0.abs_diff(cx) <= RING_REACH
                            && z0.abs_diff(cz) <= RING_REACH
                            && self.ring_cell(jobs, preferred, i, t) == (cx, cz)
                    })
                } else {
                    self.ring_positions_old(set, t).contains(&(cx, cz))
                };
                if !hit {
                    return false;
                }
                (*frequency >= 1.0 || self.reduce(*reducer, *salt, cx, cz, *frequency))
                    && !self.excluded(exclusion, cx, cz, t, lazy)
            }
            Placement::Origin => (cx, cz) == self.origin,
        }
    }

    /// A chunk of the excluded set falls at `n` chunks or fewer (square): placement refused.
    fn excluded(&self, exclusion: &Option<(String, i32)>, cx: i32, cz: i32, t: &TerrainGenerator, lazy: bool) -> bool {
        let Some((other, n)) = exclusion else { return false };
        let Some(o) = self.set_index(other) else { return false };
        (cx - n..=cx + n).any(|x| (cz - n..=cz + n).any(|z| self.is_structure_chunk_with(o, x, z, t, lazy)))
    }

    /// Biome search of a stronghold (seed `fork`, around the drawn chunk): chunk kept.
    fn ring_search(&self, t: &TerrainGenerator, preferred: &[bool], (x0, z0, fork): (i32, i32, i64)) -> (i32, i32) {
        self.ring_searches.fetch_add(1, Ordering::Relaxed);
        let mut rr = DecoRng::legacy(fork);
        match biome_search(t, 16 * x0 + 8, 0, 16 * z0 + 8, 112, preferred, &mut rr) {
            Some((x, _, z)) => (x >> 4, z >> 4),
            None => (x0, z0),
        }
    }

    /// Position of stronghold `i`, resolved only once. The initialisation is purely sequential
    /// (no call to the pool inside): a thread waiting for this stronghold cannot be the one that
    /// computes it, even by task stealing, hence no deadlock.
    fn ring_cell(&self, rj: &RingJobs, preferred: &[bool], i: usize, t: &TerrainGenerator) -> (i32, i32) {
        *rj.cells[i].get_or_init(|| self.ring_search(t, preferred, rj.jobs[i]))
    }

    /// Stronghold biome searches launched since creation.
    pub fn ring_searches(&self) -> usize {
        self.ring_searches.load(Ordering::Relaxed)
    }

    /// Clears of the start cache since creation.
    pub fn starts_clears(&self) -> usize {
        self.starts_clears.load(Ordering::Relaxed)
    }

    /// Positions of the concentric rings (§ 7.2), computed once (all the strongholds: /locate,
    /// eye of Ender).
    pub fn ring_positions(&self, set: usize, t: &TerrainGenerator) -> Arc<Vec<(i32, i32)>> {
        if rings_lazy() { self.ring_positions_lazy(set, t) } else { self.ring_positions_old(set, t) }
    }

    /// Complete computation per stronghold: each stronghold on the pool, each through its cell (safe inside the pool
    /// as outside, see [`Self::ring_cell`]).
    fn ring_positions_lazy(&self, set: usize, t: &TerrainGenerator) -> Arc<Vec<(i32, i32)>> {
        let Placement::Rings { preferred, jobs, .. } = &self.sets[set].placement else {
            return Arc::new(Vec::new());
        };
        if let Some(v) = jobs.all.get() {
            return v.clone();
        }
        let v: Vec<(i32, i32)> =
            (0..jobs.jobs.len()).into_par_iter().map(|i| self.ring_cell(jobs, preferred, i, t)).collect();
        let _ = jobs.all.set(Arc::new(v));
        jobs.all.get().cloned().unwrap_or_default()
    }

    /// Old path: read, release, compute everything, insert (each thread that misses the cache redoes
    /// the 128 searches).
    fn ring_positions_old(&self, set: usize, t: &TerrainGenerator) -> Arc<Vec<(i32, i32)>> {
        if let Some(v) = self.rings.lock().ok().and_then(|m| m.get(&set).cloned()) {
            return v;
        }
        let Placement::Rings { preferred, jobs, .. } = &self.sets[set].placement else {
            return Arc::new(Vec::new());
        };
        let out: Vec<(i32, i32)> = jobs.jobs.par_iter().map(|&j| self.ring_search(t, preferred, j)).collect();
        let v = Arc::new(out);
        if let Ok(mut m) = self.rings.lock() {
            m.insert(set, v.clone());
        }
        v
    }

    /// Computes all the ring positions right away (`RUSTCRAFT_OPT_RINGS_WARM`, default 1,
    /// outside the pool: generation thread, after creation).
    pub fn warm_rings(&self, t: &TerrainGenerator) {
        for set in 0..self.sets.len() {
            if matches!(self.sets[set].placement, Placement::Rings { .. }) {
                self.ring_positions(set, t);
            }
        }
    }

    /// Starts of the chunk (status `structure_starts`), kept.
    pub fn starts(&self, cx: i32, cz: i32, t: &TerrainGenerator, b: &Blocks) -> Arc<Vec<Start>> {
        self.starts_with(cx, cz, t, b, starts_or_insert(), starts_empty_arc())
    }

    /// [`Self::starts`], `None` for a chunk without start: its list, the one empty list shared by
    /// all such chunks, is then not handed out. The reference walk mostly meets such chunks, and
    /// each handle taken on that list writes its count, a memory line every thread would fight for.
    fn starts_if_any(&self, cx: i32, cz: i32, t: &TerrainGenerator, b: &Blocks) -> Option<Starts> {
        let (or_insert, empty_arc) = (starts_or_insert(), starts_empty_arc());
        self.cached_starts(cx, cz, t, b, or_insert, empty_arc, |list| (!list.is_empty()).then(|| list.clone()))
    }

    /// Lock of the chunk's table of the start cache (`None`: poisoned). A generator that counts
    /// for the benches also counts the locks taken and those that had to wait behind another thread.
    fn lock_start_cache(&self, cx: i32, cz: i32) -> Option<std::sync::MutexGuard<'_, start_cache::Table>> {
        let table = self.starts.table_of(cx, cz);
        #[cfg(test)]
        if let Some(counts) = &self.walk_counts {
            counts.start_cache_locks.fetch_add(1, Ordering::Relaxed);
            match table.try_lock() {
                Ok(guard) => return Some(guard),
                Err(std::sync::TryLockError::WouldBlock) => counts.start_cache_lock_waits.fetch_add(1, Ordering::Relaxed),
                Err(std::sync::TryLockError::Poisoned(_)) => return None,
            };
        }
        table.lock().ok()
    }

    /// [`Self::starts`]; `or_insert`: the first inserted wins (same `Arc` for everyone);
    /// `empty_arc`: shared empty list, without allocation.
    fn starts_with(&self, cx: i32, cz: i32, t: &TerrainGenerator, b: &Blocks, or_insert: bool, empty_arc: bool) -> Starts {
        self.cached_starts(cx, cz, t, b, or_insert, empty_arc, Arc::clone)
    }

    /// What `read` makes of the chunk's list in the start cache (under the cache's lock), the list
    /// computed and inserted first when the cache does not hold it.
    #[allow(clippy::too_many_arguments)]
    fn cached_starts<R>(
        &self,
        cx: i32,
        cz: i32,
        t: &TerrainGenerator,
        b: &Blocks,
        or_insert: bool,
        empty_arc: bool,
        read: impl Fn(&Starts) -> R,
    ) -> R {
        if let Some(read_from_cache) = self.lock_start_cache(cx, cz).and_then(|m| m.ready.get(&(cx, cz)).map(&read)) {
            return read_from_cache;
        }
        let compute = || {
            let out = self.compute_starts(cx, cz, t, b, genstart_ctx());
            if empty_arc && out.is_empty() { empty_starts() } else { Arc::new(out) }
        };
        if or_insert && self.waits_for_a_start_being_computed() {
            return self.computed_once(cx, cz, compute, read);
        }
        let v = compute();
        if let Some(mut m) = self.lock_start_cache(cx, cz) {
            self.make_room_in(&mut m);
            if or_insert {
                return read(m.ready.entry((cx, cz)).or_insert(v));
            }
            m.ready.insert((cx, cz), v.clone());
        }
        read(&v)
    }

    /// Empties a table of the start cache that reached its capacity, before an insertion.
    fn make_room_in(&self, table: &mut start_cache::Table) {
        if table.ready.len() > self.starts.table_capacity {
            table.ready.clear();
            self.starts_clears.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// `STARTS_COMPUTED_ONCE` in effect. Threads then wait for one another, which is only safe
    /// while a start computation hands no work to the thread pool (a waiting pool thread could
    /// otherwise be the one that must run it): the two switches that put pool work inside a
    /// start computation, the old ring path and the state tables built across the pool, turn the
    /// wait off.
    fn waits_for_a_start_being_computed(&self) -> bool {
        self.starts_computed_once && rings_lazy() && !crate::opt::parallel_transforms()
    }

    /// Miss of the start cache settled by a single computation: the first thread that asks for a
    /// chunk computes its list, the threads that ask meanwhile wait for that list instead of each
    /// computing their own (first walks of a new world on 32 threads: 2.3 to 2.7 lists computed
    /// per chunk in the main world, 3.9 to 4.2 in the Nether, see `docs/perf.md`). Everyone gets
    /// the first list: it needs "first inserted wins".
    ///
    /// A thread never waits for itself: should it come back, inside its computation, to the very
    /// chunk it is computing, it computes a second list of its own, which is not kept.
    fn computed_once<R>(&self, cx: i32, cz: i32, compute: impl FnOnce() -> Starts, read: impl Fn(&Starts) -> R) -> R {
        let key = (cx, cz);
        let Some(mut table) = self.lock_start_cache(cx, cz) else { return read(&compute()) };
        // Inserted since the miss above.
        if let Some(list) = table.ready.get(&key) {
            return read(list);
        }
        let pending = table.in_progress.entry(key).or_default().clone();
        drop(table);
        let this_thread = std::thread::current().id();
        if pending.computing_thread() == Some(this_thread) {
            return read(&compute());
        }
        let list = pending.list.get_or_init(|| {
            pending.set_computing_thread(this_thread);
            compute()
        });
        let Some(mut table) = self.lock_start_cache(cx, cz) else { return read(list) };
        if table.in_progress.get(&key).is_some_and(|p| Arc::ptr_eq(p, &pending)) {
            table.in_progress.remove(&key);
        }
        // Every thread that waited comes here with the same list: the first one inserts it, and
        // all of them read what the cache holds.
        if !table.ready.contains_key(&key) {
            self.make_room_in(&mut table);
        }
        read(table.ready.entry(key).or_insert_with(|| list.clone()))
    }

    /// Replaces the start cache by an empty one, sharded or not (tests and benches).
    #[doc(hidden)]
    pub fn reset_start_cache(&mut self, sharded: bool) {
        self.starts = start_cache::StartCache::new(sharded);
    }

    /// `ctx`: biome context kept per start attempt.
    fn compute_starts(&self, cx: i32, cz: i32, t: &TerrainGenerator, b: &Blocks, ctx: bool) -> Vec<Start> {
        #[cfg(test)]
        self.count(|c| &c.starts_computed, 1);
        let mut out: Vec<Start> = Vec::new();
        for (si, set) in self.sets.iter().enumerate() {
            if set.structures.iter().any(|(s, _)| out.iter().any(|st| st.structure == *s)) {
                continue;
            }
            if !self.is_structure_chunk(si, cx, cz, t) {
                continue;
            }
            if set.structures.len() == 1 {
                if let Some(s) = self.try_start(set.structures[0].0, cx, cz, t, b, ctx) {
                    out.push(s);
                }
                continue;
            }
            let mut rng = DecoRng::legacy(0);
            rng.set_large_feature_seed(self.seed, cx, cz);
            let mut options = set.structures.clone();
            let mut total: i32 = options.iter().map(|o| o.1).sum();
            while !options.is_empty() {
                let mut tr = rng.next_int_bounded(total);
                let mut pick = 0;
                for (i, o) in options.iter().enumerate() {
                    tr -= o.1;
                    if tr < 0 {
                        pick = i;
                        break;
                    }
                }
                let (s, w) = options[pick];
                if let Some(st) = self.try_start(s, cx, cz, t, b, ctx) {
                    out.push(st);
                    break;
                }
                total -= w;
                options.remove(pick);
            }
        }
        out
    }

    fn try_start(&self, s: usize, cx: i32, cz: i32, t: &TerrainGenerator, b: &Blocks, ctx: bool) -> Option<Start> {
        self.try_start_biome(s, cx, cz, t, b, ctx, false)
    }

    #[allow(clippy::too_many_arguments)]
    fn try_start_biome(
        &self,
        s: usize,
        cx: i32,
        cz: i32,
        t: &TerrainGenerator,
        b: &Blocks,
        ctx: bool,
        any_biome: bool,
    ) -> Option<Start> {
        let def = &self.defs[s];
        let mut rng = DecoRng::legacy(0);
        rng.set_large_feature_seed(self.seed, cx, cz);
        let bctx = ctx.then(|| RefCell::new(Ctx::cached()));
        let mut g = GenStart { sg: self, t, b, cx, cz, rng, hctx: t.gen_ctx(), def, bctx, any_biome };
        let (pos, pieces) = match &def.kind {
            Kind::Jigsaw(cfg) => crate::jigsaw::generate(&mut g, cfg)?,
            Kind::BuriedTreasure => crate::structure_coded::buried_treasure(&mut g)?,
            Kind::Mineshaft { mesa } => crate::structure_coded::mineshaft(&mut g, *mesa)?,
            Kind::RuinedPortal(setups) => crate::structure_templ::ruined_portal(&mut g, setups)?,
            Kind::Shipwreck { beached } => crate::structure_templ::shipwreck(&mut g, *beached)?,
            Kind::OceanRuin { warm, large, cluster } => {
                crate::structure_templ::ocean_ruin(&mut g, *warm, *large, *cluster)?
            }
            Kind::DesertPyramid => crate::structure_scattered::desert_pyramid(&mut g)?,
            Kind::JungleTemple => crate::structure_scattered::jungle_temple(&mut g)?,
            Kind::SwampHut => crate::structure_scattered::swamp_hut(&mut g)?,
            Kind::Igloo => crate::structure_scattered::igloo(&mut g)?,
            Kind::Stronghold => crate::structure_stronghold::stronghold(&mut g)?,
            Kind::Fortress => crate::structure_fortress::fortress(&mut g)?,
            Kind::Mansion => crate::structure_mansion::mansion(&mut g)?,
            Kind::EndCity => crate::structure_endcity::end_city(&mut g)?,
            Kind::Monument(around) => crate::structure_monument::monument(&mut g, around)?,
            Kind::NetherFossil(h) => crate::structure_templ::nether_fossil(&mut g, h)?,
            Kind::Unsupported(_) => return None,
        };
        if pieces.is_empty() || !g.valid_biome(pos) {
            return None;
        }
        let mut bbox = pieces[0].bbox();
        for p in &pieces[1..] {
            bbox.encapsulate(&p.bbox());
        }
        Some(Start { structure: s, cx, cz, pieces, bbox })
    }

    /// Start of the structure `id` generated at chunk (cx, cz) whatever the placement rules and
    /// the biome (`place structure`).
    pub fn start_anywhere(&self, id: &str, cx: i32, cz: i32, t: &TerrainGenerator, b: &Blocks) -> Option<Start> {
        let id = crate::data::full_id(id);
        let s = self.defs.iter().position(|d| d.id == id)?;
        self.try_start_biome(s, cx, cz, t, b, genstart_ctx(), true)
    }

    /// Jigsaw assembly from the pool `pool` at `pos` (jigsaw block, `place jigsaw`): the start
    /// piece joins by its jigsaw named `target`, `depth` levels, within 128 blocks, randomness of
    /// the chunk of `pos`. The start carries no structure (index `usize::MAX`).
    #[allow(clippy::too_many_arguments)]
    pub fn jigsaw_at(
        &self,
        pool: &str,
        target: &str,
        depth: i32,
        pos: [i32; 3],
        t: &TerrainGenerator,
        b: &Blocks,
    ) -> Option<Start> {
        let def = self.defs.first()?;
        let (cx, cz) = (pos[0] >> 4, pos[2] >> 4);
        let mut rng = DecoRng::legacy(0);
        rng.set_large_feature_seed(self.seed, cx, cz);
        let mut g = GenStart { sg: self, t, b, cx, cz, rng, hctx: t.gen_ctx(), def, bctx: None, any_biome: true };
        let cfg = crate::jigsaw::JigsawCfg {
            start_pool: crate::data::full_id(pool),
            start_jigsaw_name: Some(crate::data::full_id(target)),
            size: depth,
            start_height: crate::decor_pred::HeightP::Const(crate::decor_pred::Anchor::Absolute(pos[1])),
            expansion_hack: false,
            project: None,
            max_h: 128,
            max_v: Some(128),
            aliases: Vec::new(),
            pad_bottom: 0,
            pad_top: 0,
            waterlog: true,
        };
        let (_, pieces) = crate::jigsaw::generate_from(&mut g, &cfg, pos)?;
        let mut bbox = pieces.first()?.bbox();
        for p in &pieces[1..] {
            bbox.encapsulate(&p.bbox());
        }
        Some(Start { structure: usize::MAX, cx, cz, pieces, bbox })
    }

    /// Adds `n` to one of the bench counters, when this generator counts.
    #[cfg(test)]
    pub(crate) fn count(&self, counter: impl Fn(&WalkCounts) -> &std::sync::atomic::AtomicU64, n: u64) {
        if let Some(counts) = &self.walk_counts {
            counter(counts).fetch_add(n, Ordering::Relaxed);
        }
    }
}

/// Work of the placement tests and of the start cache, counted for the benches (tests only: a
/// generator counts once `walk_counts` is set, and nothing of this is compiled into the game).
#[cfg(test)]
#[derive(Debug, Default)]
pub(crate) struct WalkCounts {
    /// Placement questions asked chunk by chunk (one set, one chunk).
    pub per_chunk_tests: std::sync::atomic::AtomicU64,
    /// Regions of a random-spread set seeded and drawn.
    pub region_draws: std::sync::atomic::AtomicU64,
    /// Frequency reducers seeded and drawn.
    pub reducer_draws: std::sync::atomic::AtomicU64,
    /// Stronghold draws compared with a chunk or a window.
    pub ring_draws_compared: std::sync::atomic::AtomicU64,
    /// Locks of the start cache taken, and those that had to wait for another thread.
    pub start_cache_locks: std::sync::atomic::AtomicU64,
    pub start_cache_lock_waits: std::sync::atomic::AtomicU64,
    /// Biomes read from the source by the start attempts.
    pub start_biome_reads: std::sync::atomic::AtomicU64,
    /// Start lists computed (a chunk computed by two threads at once counts twice).
    pub starts_computed: std::sync::atomic::AtomicU64,
}

/// Shared empty list of the chunks without start.
fn empty_starts() -> Starts {
    static E: OnceLock<Starts> = OnceLock::new();
    E.get_or_init(|| Arc::new(Vec::new())).clone()
}

pub(crate) fn chunk_key(x: i32, z: i32) -> i64 {
    (i64::from(x) & 0xFFFF_FFFF) | ((i64::from(z) & 0xFFFF_FFFF) << 32)
}

fn unkey(k: i64) -> (i32, i32) {
    (k as i32, (k >> 32) as i32)
}

/// Open set of long integers the game's way (iteration order reproduced, § 7.4).
#[derive(Debug, Default)]
struct LongSet {
    table: Vec<Option<i64>>,
    has_zero: bool,
    size: usize,
}

fn mix(k: i64) -> u64 {
    let mut h = (k as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h >> 32;
    h ^ (h >> 16)
}

impl LongSet {
    fn add(&mut self, k: i64) {
        if k == 0 {
            if !self.has_zero {
                self.has_zero = true;
                self.size += 1;
            }
            return;
        }
        if self.table.is_empty() {
            self.table = vec![None; 32];
        }
        if self.insert(k) {
            self.size += 1;
            if self.size >= self.table.len() * 3 / 4 {
                let n = self.table.len() * 2;
                let old = std::mem::replace(&mut self.table, vec![None; n]);
                for v in old.into_iter().flatten() {
                    self.insert(v);
                }
            }
        }
    }

    fn insert(&mut self, k: i64) -> bool {
        let n = self.table.len();
        let mut i = (mix(k) as usize) & (n - 1);
        loop {
            match self.table[i] {
                None => {
                    self.table[i] = Some(k);
                    return true;
                }
                Some(v) if v == k => return false,
                _ => i = (i + 1) & (n - 1),
            }
        }
    }

    /// Key 0 first, then slots by decreasing index.
    fn iter(&self) -> impl Iterator<Item = i64> + '_ {
        self.has_zero.then_some(0).into_iter().chain(self.table.iter().rev().flatten().copied())
    }
}

/// Biome search (§ 7.2): reservoir sampling of the valid quarts of a square.
pub fn biome_search(
    t: &TerrainGenerator,
    x: i32,
    y: i32,
    z: i32,
    r: i32,
    pred: &[bool],
    rng: &mut DecoRng,
) -> Option<(i32, i32, i32)> {
    let (qx, qz, qr, qy) = (x >> 2, z >> 2, r >> 2, y >> 2);
    let mut found = 0;
    let mut res = None;
    let mut ctx = Ctx::cached();
    let mut s = Searcher::default();
    for dz in -qr..=qr {
        for dx in -qr..=qr {
            let b = t.source.biome_at(&t.g, &mut ctx, &mut s, qx + dx, qy, qz + dz);
            if pred.get(b as usize).copied().unwrap_or(false) {
                if res.is_none() || rng.next_int_bounded(found + 1) == 0 {
                    res = Some((4 * (qx + dx), y, 4 * (qz + dz)));
                }
                found += 1;
            }
        }
    }
    res
}

/// Villager type of each biome (game table, plains by default).
fn villager_types(names: &[String]) -> Arc<[&'static str]> {
    names
        .iter()
        .map(|n| match n.strip_prefix("minecraft:").unwrap_or(n) {
            "badlands" | "desert" | "eroded_badlands" | "wooded_badlands" => "minecraft:desert",
            "bamboo_jungle" | "jungle" | "sparse_jungle" => "minecraft:jungle",
            "savanna_plateau" | "savanna" | "windswept_savanna" => "minecraft:savanna",
            "deep_frozen_ocean" | "frozen_ocean" | "frozen_river" | "ice_spikes" | "snowy_beach" | "snowy_taiga"
            | "snowy_plains" | "grove" | "snowy_slopes" | "frozen_peaks" | "jagged_peaks" => "minecraft:snow",
            "swamp" | "mangrove_swamp" => "minecraft:swamp",
            "old_growth_spruce_taiga"
            | "old_growth_pine_taiga"
            | "windswept_gravelly_hills"
            | "windswept_hills"
            | "taiga"
            | "windswept_forest" => "minecraft:taiga",
            _ => "minecraft:plains",
        })
        .collect()
}

/// Shared access to the structure generator (once per dimension).
pub type Shared = Arc<OnceLock<StructureGen>>;

#[path = "structure_start_cache.rs"]
mod start_cache;

#[path = "structure_references.rs"]
mod references;
pub use references::{
    REFERENCE_REACH, ReferencePaths, ReferenceSabotage, References, referenced_starts, referenced_starts_cloned,
};

#[cfg(test)]
#[path = "structure_tests.rs"]
pub(crate) mod tests;

#[cfg(test)]
#[path = "templ_tests.rs"]
mod templ_tests;
