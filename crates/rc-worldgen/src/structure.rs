//! Structures (`generation-decoration.md` § 7–9): sets and placements, starts per chunk,
//! references, contribution to the terrain ("beard"). The structure types are in
//! `structure_*.rs`; piece placement during decoration in `structure_place.rs`.

use hashbrown::{HashMap, HashSet};
use std::collections::BTreeMap;
use std::cell::RefCell;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use rayon::prelude::*;

use crate::Error;
use crate::biome::Searcher;
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
fn starts_or_insert() -> bool {
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
    starts: Mutex<HashMap<(i32, i32), Starts>>,
    /// Stronghold biome searches launched (both paths): bench and proofs.
    ring_searches: AtomicUsize,
    /// Clears of the start cache (threshold of 200,000 chunks reached).
    starts_clears: AtomicUsize,
    /// Structures per step (registry order).
    pub by_step: Vec<Vec<usize>>,
    pub flat: bool,
    pub procs: crate::structure_place::ProcLists,
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
}

impl GenStart<'_> {
    /// Raw biome (without zoom) at the point. New searcher at each call (its tie-breaking
    /// depends on the last result: do not share it).
    pub fn biome_at(&self, x: i32, y: i32, z: i32) -> u16 {
        let (qx, qy, qz) = (x >> 2, y >> 2, z >> 2);
        match &self.bctx {
            Some(c) => self.t.source.biome_at(&self.t.g, &mut c.borrow_mut(), &mut Searcher::default(), qx, qy, qz),
            None => self.t.source.biome_at(&self.t.g, &mut Ctx::uncached(), &mut Searcher::default(), qx, qy, qz),
        }
    }

    pub fn valid_biome(&self, p: [i32; 3]) -> bool {
        self.def.biomes.get(self.biome_at(p[0], p[1], p[2]) as usize).copied().unwrap_or(false)
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
    /// `flat`: flat world (ring seed zero).
    pub fn new(
        d: &DecorData,
        b: &Blocks,
        templates: Arc<Templates>,
        biome_names: &[String],
        seed: i64,
        flat: bool,
    ) -> Result<StructureGen, Error> {
        let pools = Pools::new(d, b);
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
                "minecraft:ocean_monument" => Kind::Monument(biome_set(
                    d,
                    Some(&Json::Str("#minecraft:required_ocean_monument_surrounding".into())),
                    biome_names,
                )),
                "minecraft:nether_fossil" => match j.get("height").map(crate::decor_pred::HeightP::parse) {
                    Some(Ok(h)) => Kind::NetherFossil(h),
                    _ => Kind::Unsupported("minecraft:nether_fossil".into()),
                },
                other => Kind::Unsupported(other.to_owned()),
            };
            index.insert(id.clone(), defs.len());
            defs.push(StructureDef { biomes: biome_set(d, j.get("biomes"), biome_names), id, kind, step, adapt });
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
                        preferred: biome_set(d, p.get("preferred_biomes"), biome_names),
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
            starts: Mutex::new(HashMap::new()),
            ring_searches: AtomicUsize::new(0),
            starts_clears: AtomicUsize::new(0),
            by_step,
            flat,
            procs: crate::structure_place::ProcLists::new(d),
            mineshaft_blocking: biome_set(d, Some(&Json::Str("#minecraft:mineshaft_blocking".into())), biome_names),
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
        match &s.placement {
            Placement::Random { spacing, separation, triangular, salt, frequency, reducer, exclusion } => {
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

    /// [`Self::starts`]; `or_insert`: the first inserted wins (same `Arc` for everyone);
    /// `empty_arc`: shared empty list, without allocation.
    fn starts_with(&self, cx: i32, cz: i32, t: &TerrainGenerator, b: &Blocks, or_insert: bool, empty_arc: bool) -> Starts {
        if let Some(v) = self.starts.lock().ok().and_then(|m| m.get(&(cx, cz)).cloned()) {
            return v;
        }
        let out = self.compute_starts(cx, cz, t, b, genstart_ctx());
        let v = if empty_arc && out.is_empty() { empty_starts() } else { Arc::new(out) };
        if let Ok(mut m) = self.starts.lock() {
            if m.len() > 200_000 {
                m.clear();
                self.starts_clears.fetch_add(1, Ordering::Relaxed);
            }
            if or_insert {
                return m.entry((cx, cz)).or_insert(v).clone();
            }
            m.insert((cx, cz), v.clone());
        }
        v
    }

    /// `ctx`: biome context kept per start attempt.
    fn compute_starts(&self, cx: i32, cz: i32, t: &TerrainGenerator, b: &Blocks, ctx: bool) -> Vec<Start> {
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
        let def = &self.defs[s];
        let mut rng = DecoRng::legacy(0);
        rng.set_large_feature_seed(self.seed, cx, cz);
        let bctx = ctx.then(|| RefCell::new(Ctx::cached()));
        let mut g = GenStart { sg: self, t, b, cx, cz, rng, hctx: t.gen_ctx(), def, bctx };
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

    /// References of the chunk (§ 7.4): starts of the chunks within 8 whose box intersects its
    /// columns, per structure (registry order), in the game's iteration order.
    pub fn references(
        &self,
        cx: i32,
        cz: i32,
        t: &TerrainGenerator,
        b: &Blocks,
    ) -> BTreeMap<usize, Vec<Arc<Vec<Start>>>> {
        self.references_with(cx, cz, t, b, starts_prefilter())
    }

    /// Is the neighbour a candidate chunk of at least one set? Otherwise its starts are
    /// surely empty: each attempt of [`Self::compute_starts`] goes through this same test.
    fn any_structure_chunk(&self, cx: i32, cz: i32, t: &TerrainGenerator) -> bool {
        (0..self.sets.len()).any(|si| self.is_structure_chunk(si, cx, cz, t))
    }

    /// [`Self::references`]; `prefilter`: neighbours without candidate skipped (neither lock nor entry).
    fn references_with(
        &self,
        cx: i32,
        cz: i32,
        t: &TerrainGenerator,
        b: &Blocks,
        prefilter: bool,
    ) -> BTreeMap<usize, Vec<Arc<Vec<Start>>>> {
        let mut keys: BTreeMap<usize, LongSet> = BTreeMap::new();
        let mut found: HashMap<(i32, i32), Arc<Vec<Start>>> = HashMap::new();
        let (x0, z0) = (cx * 16, cz * 16);
        for sx in cx - 8..=cx + 8 {
            for sz in cz - 8..=cz + 8 {
                if prefilter && !self.any_structure_chunk(sx, sz, t) {
                    continue;
                }
                let st = self.starts(sx, sz, t, b);
                if st.is_empty() {
                    continue;
                }
                for s in st.iter() {
                    if s.ref_box(&self.defs).intersects_xz(x0, z0, x0 + 15, z0 + 15) {
                        keys.entry(s.structure).or_default().add(chunk_key(sx, sz));
                        found.insert((sx, sz), st.clone());
                    }
                }
            }
        }
        keys.into_iter()
            .map(|(s, set)| (s, set.iter().filter_map(|k| found.get(&unkey(k)).cloned()).collect()))
            .collect()
    }

    /// Starts of a structure referenced by the chunk, in reference order.
    pub fn referenced(&self, cx: i32, cz: i32, t: &TerrainGenerator, b: &Blocks) -> Vec<(usize, Vec<Start>)> {
        self.references(cx, cz, t, b)
            .into_iter()
            .map(|(s, lists)| {
                let v = lists.iter().flat_map(|l| l.iter().filter(|st| st.structure == s).cloned()).collect();
                (s, v)
            })
            .collect()
    }
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

#[cfg(test)]
#[path = "structure_tests.rs"]
pub(crate) mod tests;

#[cfg(test)]
#[path = "templ_tests.rs"]
mod templ_tests;
