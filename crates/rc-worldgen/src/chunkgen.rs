//! Complete chain of a dimension for singleplayer (`generation-decoration.md` § 1): terrain → decoration
//! chunk (network states) → decoration of the 3×3 area. "Noise" generators (the game's terrain)
//! and "flat" (layers). Initial spawn point (noise search, § spawn).
//!
//! Everything is prepared once per world and dimension; the per-chunk calls are `&self` and
//! shareable between threads (parallelism is the caller's business).

use std::borrow::Borrow;
use std::path::Path;
use std::sync::Arc;

use crate::Error;
use crate::biome::{BiomeSource, ClimateList, quantize};
use crate::data::{DataPack, full_id};
use crate::decor::{DecorStats, Decorator};
use crate::decor_blocks::{Blocks, State};
use crate::decor_data::DecorData;
use crate::decor_region::{DecoChunk, FINAL_HMS, Hm, Region};
use crate::json::Json;
use crate::palette::{AIR, BlockId};
use crate::proto::ProtoTerrain;
use crate::sampler::{Ctx, Sid};
use crate::structure::{References, Start, StructureGen, referenced_starts, referenced_starts_cloned};
use crate::structure_beard::Beard;
use crate::structure_place::ProcLists;
use crate::terrain::StructureDensity;
use crate::terrain::{BiomeCache, TerrainGenerator, world_height};

/// Game data useful to generation, read once from the jar (and the official reports).
pub struct WorldData {
    pub pack: Arc<DataPack>,
    pub decor: DecorData,
    pub blocks: Arc<Blocks>,
    /// Structure templates (read on demand).
    pub templates: Arc<crate::template::Templates>,
    /// Biome parameter lists already parsed, per preset (`RUSTCRAFT_OPT_SHARED_CLIMATE_LISTS`).
    climate_lists: std::sync::Mutex<Vec<(String, ClimateList)>>,
    /// Processor lists shared by every generator of these data (`RUSTCRAFT_OPT_SHARED_PROC_LISTS`).
    proc_lists: std::sync::OnceLock<Arc<ProcLists>>,
}

impl std::fmt::Debug for WorldData {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "WorldData({} files, {:?})", self.pack.len(), self.blocks)
    }
}

impl WorldData {
    /// `jar`: client (or server) jar; `reports`: `reports/` folder of the official data
    /// generator (biome parameter lists), if it exists.
    pub fn load(jar: &Path, reports: Option<&Path>, v: &rc_data::VersionData) -> Result<Self, Error> {
        let mut pack = DataPack::from_jar(jar)?;
        if let Some(r) = reports {
            pack.add_reports(r)?;
        }
        let decor = DecorData::from_jar(jar)?;
        let blocks = Arc::new(Blocks::new(v, &decor)?);
        let templates =
            Arc::new(crate::template::Templates::for_world(decor.files_of("template"), &blocks, blocks_key(jar, v)));
        Ok(WorldData {
            pack: Arc::new(pack),
            decor,
            blocks,
            templates,
            climate_lists: Default::default(),
            proc_lists: Default::default(),
        })
    }

    /// Biome parameter list of a preset (report of the data generator): parsed once per world
    /// data, then copied for each generator, which is far cheaper than parsing the report again.
    fn climate_list(&self, preset: &str) -> Result<ClimateList, Error> {
        if !crate::opt::shared_climate_lists() {
            return ClimateList::from_report(&self.pack, preset);
        }
        let mut lists = self.climate_lists.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some((_, list)) = lists.iter().find(|(p, _)| p == preset) {
            return Ok(list.clone());
        }
        let list = ClimateList::from_report(&self.pack, preset)?;
        lists.push((preset.to_owned(), list.clone()));
        Ok(list)
    }

    /// Processor lists of the data, analysed on demand: one table shared by every decorator and
    /// structure generator of these data (old path: a new table for each).
    pub fn proc_lists(&self) -> Arc<ProcLists> {
        if !crate::opt::shared_proc_lists() {
            return Arc::new(ProcLists::new(&self.decor));
        }
        self.proc_lists.get_or_init(|| Arc::new(ProcLists::new(&self.decor))).clone()
    }

    /// Data shared by all the users of the same jar in the process (dimension generators,
    /// spawn point search, `/locate`, `/place`): built once as long as
    /// a user holds them (the cache only keeps a weak reference: nothing stays in
    /// memory once the world is closed). The data depend only on the jar, the reports and the
    /// version, never on the world (its data packs do not touch generation here).
    /// `RUSTCRAFT_OPT_WORLDDATA_SHARE=0`: each call reloads (old path).
    pub fn shared(jar: &Path, reports: Option<&Path>, v: &rc_data::VersionData) -> Result<Arc<Self>, Error> {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        if !*ON.get_or_init(|| rc_data::env_str!("RUSTCRAFT_OPT_WORLDDATA_SHARE") != Some("0")) {
            return Self::load(jar, reports, v).map(Arc::new);
        }
        type Key = (std::path::PathBuf, Option<std::path::PathBuf>, String, usize, usize);
        static CACHE: std::sync::Mutex<Vec<(Key, std::sync::Weak<WorldData>)>> = std::sync::Mutex::new(Vec::new());
        let key: Key = (
            std::fs::canonicalize(jar).unwrap_or_else(|_| jar.to_path_buf()),
            reports.map(Path::to_path_buf),
            v.version.clone(),
            v.states.len(),
            v.blocks.len(),
        );
        // Lock held during loading: two threads that need it at the same time only make
        // one.
        let mut c = CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        c.retain(|(_, w)| w.strong_count() > 0);
        if let Some(wd) = c.iter().find(|(k, _)| *k == key).and_then(|(_, w)| w.upgrade()) {
            return Ok(wd);
        }
        let wd = Arc::new(Self::load(jar, reports, v)?);
        c.push((key, Arc::downgrade(&wd)));
        Ok(wd)
    }
}

/// Names what the blocks of the generation data come from (jar and game version): the key of the
/// state tables the process keeps (`RUSTCRAFT_OPT_KEEP_TRANSFORMS`).
fn blocks_key(jar: &Path, v: &rc_data::VersionData) -> String {
    let jar = std::fs::canonicalize(jar).unwrap_or_else(|_| jar.to_path_buf());
    format!("{}|{}|{}|{}", jar.display(), v.version, v.states.len(), v.blocks.len())
}

/// Generator of a dimension as described by the world settings.
#[derive(Debug, Clone, PartialEq)]
pub enum GenSpec {
    /// Noise settings (`minecraft:overworld`…) and biome source.
    Noise {
        settings: String,
        source: SourceSpec,
    },
    Flat(FlatSpec),
    /// Demo world of all the states (not generated here: empty).
    Debug,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SourceSpec {
    /// Named parameter list (`minecraft:overworld`, `minecraft:nether`).
    MultiNoise(String),
    TheEnd,
    Fixed(String),
    /// Checkerboard: biomes (identifiers or `#tag`, in order) and scale.
    Checkerboard {
        biomes: Vec<String>,
        scale: i32,
    },
}

/// Flat world: layers from bottom to top (state, thickness), biome, decoration.
#[derive(Debug, Clone, PartialEq)]
pub struct FlatSpec {
    pub layers: Vec<(String, i32)>,
    pub biome: String,
    pub features: bool,
    pub lakes: bool,
    /// Allowed structure sets (`structure_overrides`, identifiers or `#tag`);
    /// `None`: all those for which a biome is possible.
    pub structure_overrides: Option<Vec<String>>,
}

impl FlatSpec {
    /// Default flat settings of the game (fallback when the preset is not readable):
    /// bedrock, 2 dirt, grass, plains, strongholds and villages.
    pub fn classic() -> Self {
        FlatSpec {
            layers: vec![
                ("minecraft:bedrock".into(), 1),
                ("minecraft:dirt".into(), 2),
                ("minecraft:grass_block".into(), 1),
            ],
            biome: "minecraft:plains".into(),
            features: false,
            lakes: false,
            structure_overrides: Some(vec!["minecraft:strongholds".into(), "minecraft:villages".into()]),
        }
    }
}

enum Kind {
    Noise(Box<TerrainGenerator>),
    /// `spec`: settings (structures); `t`: "generator" view of the structures, prepared by
    /// [`DimGen::enable_structures`].
    Flat {
        layers: Vec<State>,
        spec: Box<FlatSpec>,
        t: Option<Box<TerrainGenerator>>,
    },
    /// "Debug" world: empty terrain, then at decoration a barrier floor at y = 60 and
    /// all the block states in a grid (every other cell) at y = 70.
    Debug,
}

/// State of the "debug" world grid at (x, z): air outside the odd positive cells.
pub fn debug_state_at(x: i32, z: i32, state_count: usize, air: State) -> State {
    if x <= 0 || z <= 0 || x % 2 == 0 || z % 2 == 0 {
        return air;
    }
    let n = state_count as f32;
    let ceil = |v: f32| {
        let i = v as i32;
        if v > i as f32 { i + 1 } else { i }
    };
    let width = ceil(n.sqrt());
    let height = ceil(n / width as f32);
    let (gx, gz) = (x / 2, z / 2);
    if gx > width || gz > height {
        return air;
    }
    let index = (gx * width + gz).unsigned_abs() as usize;
    if index < state_count { index as State } else { air }
}

/// Spawn target point: (function, quantised (min, max)) per parameter.
type SpawnTarget = Vec<(Sid, (i64, i64))>;

/// Optimised paths of the conversion of a terrain proto-chunk into a decoration chunk. Default:
/// `RUSTCRAFT_OPT_*` switches read at startup; the equivalence tests set them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConversionOpts {
    /// `SKIP_AIR_SECTIONS`: a section of plain air is not converted cell by cell, and the highest
    /// other section bounds the walk of the final height maps.
    pub skip_air_sections: bool,
    /// `CONVERT_WITH_HEIGHTMAPS`: conversion layer by layer from the top, finding the four final
    /// height maps on the way.
    pub heightmaps_while_converting: bool,
    /// `REUSE_PROTO_BUFFER`: the proto-chunk's block buffer goes back to its thread for the next one.
    pub reuse_proto_buffer: bool,
}

impl ConversionOpts {
    pub const OLD: ConversionOpts =
        ConversionOpts { skip_air_sections: false, heightmaps_while_converting: false, reuse_proto_buffer: false };
    pub const NEW: ConversionOpts =
        ConversionOpts { skip_air_sections: true, heightmaps_while_converting: true, reuse_proto_buffer: true };

    pub fn from_env() -> Self {
        ConversionOpts {
            skip_air_sections: crate::opt::skip_air_sections(),
            heightmaps_while_converting: crate::opt::convert_with_heightmaps(),
            reuse_proto_buffer: crate::opt::reuse_proto_buffer(),
        }
    }
}

/// How the structure references of a chunk reach its terrain and its decoration. Default:
/// `RUSTCRAFT_OPT_*` switches read at startup; the equivalence tests set them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReferenceUse {
    /// `REFERENCES_ONCE`: one walk serves the beard and the saved compound of a new chunk.
    pub one_walk_for_terrain: bool,
    /// `REUSE_TERRAIN_REFERENCES`: the decoration uses the references of that walk again (only
    /// with `one_walk_for_terrain`, which hands them out).
    pub kept_for_decoration: bool,
    /// `REFERENCED_WITHOUT_CLONE`: the referenced starts are borrowed, not copied.
    pub borrowed_starts: bool,
}

impl ReferenceUse {
    pub const OLD: ReferenceUse =
        ReferenceUse { one_walk_for_terrain: false, kept_for_decoration: false, borrowed_starts: false };
    pub const NEW: ReferenceUse =
        ReferenceUse { one_walk_for_terrain: true, kept_for_decoration: true, borrowed_starts: true };

    pub fn from_env() -> Self {
        ReferenceUse {
            one_walk_for_terrain: crate::opt::references_once(),
            kept_for_decoration: crate::opt::reuse_terrain_references(),
            borrowed_starts: crate::opt::referenced_without_clone(),
        }
    }
}

/// References of a chunk walked for its terrain ([`DimGen::terrain_with_structures`]), which its
/// owner keeps until the chunk is decorated ([`DimGen::decorate_reusing`]).
#[derive(Debug)]
pub struct TerrainReferences {
    chunk: (i32, i32),
    references: References,
    /// Clears of the start cache counted before the walk.
    start_cache_clears: usize,
}

impl TerrainReferences {
    /// The start lists held are still the ones the start cache hands out for the chunk `center`.
    /// After a clear the cache builds new start objects, and the pieces placed must be the cache's:
    /// a scattered piece (pyramid, temple, hut) records its state in them. Without "first inserted
    /// wins", a list can also be replaced in the cache with no clear to tell.
    fn still_current(&self, sg: &StructureGen, center: (i32, i32)) -> bool {
        crate::structure::starts_or_insert() && self.chunk == center && self.start_cache_clears == sg.starts_clears()
    }

    /// The references kept, for the tests.
    #[doc(hidden)]
    pub fn references_mut(&mut self) -> &mut References {
        &mut self.references
    }
}

/// What the conversion of the blocks leaves to do for the final height maps.
enum FinalMapsToDo {
    /// Found during the conversion.
    Nothing,
    /// Every cell from this y up is plain air.
    WalkBelow(i32),
    /// The whole chunk to walk.
    WalkAll,
}

/// Ready generator of a dimension.
pub struct DimGen {
    kind: Kind,
    pub deco: Option<Arc<Decorator>>,
    pub blocks: Arc<Blocks>,
    /// Terrain block identifier → network state.
    to_state: Vec<State>,
    /// Terrain block identifier → height map bits of its network state.
    final_map_bits: Vec<u8>,
    /// Optimised paths of the conversion into decoration chunks.
    pub conversion: ConversionOpts,
    /// Paths of the structure references through terrain and decoration.
    pub reference_use: ReferenceUse,
    pub dimension: String,
    pub seed: i64,
    pub min_y: i32,
    pub height: i32,
    pub sea_level: i32,
    gen_min_y: i32,
    gen_depth: i32,
    /// Names of the biomes, indexed by the chunk values.
    pub biome_names: Vec<String>,
    spawn_targets: Vec<SpawnTarget>,
    /// Structures (world with structures, noise generator).
    pub structures: Option<Arc<crate::structure::StructureGen>>,
    /// What it takes to build the regeneration generator below y = 0 (noise world going down
    /// below 0): data, settings, biome source.
    retro_parts: Option<Box<(Arc<DataPack>, String, BiomeSource)>>,
    /// Regeneration generator (and its state table), built at the first converted chunk.
    retro_gen: std::sync::OnceLock<Option<RetroGen>>,
}

type RetroGen = (Box<TerrainGenerator>, Vec<State>);

/// Cave biomes kept by the regeneration below y = 0; the other quarts take the old
/// biome of the column at y = 0.
const RETAINED_RETRO_BIOMES: [&str; 3] = ["minecraft:lush_caves", "minecraft:dripstone_caves", "minecraft:deep_dark"];

impl std::fmt::Debug for DimGen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "DimGen({}, seed {}, {} biomes)", self.dimension, self.seed, self.biome_names.len())
    }
}

/// Distance of a quantised value to a climate parameter interval.
fn param_distance((min, max): (i64, i64), v: i64) -> i64 {
    let above = v - max;
    if above > 0 { above } else { (min - v).max(0) }
}

/// Points visited by one spiral of the spawn search around `origin`: rings of `step` blocks up
/// to `max_r`, the angle advancing by `step / radius` (single precision, like the game).
fn spawn_spiral(origin: (i32, i32), max_r: f32, step: f32) -> Vec<(i32, i32)> {
    let mut points = Vec::new();
    let mut angle = 0.0f32;
    let mut radius = step;
    while radius <= max_r {
        let x = origin.0 + (crate::jmath::sin(f64::from(angle)) * f64::from(radius)) as i32;
        let z = origin.1 + (crate::jmath::cos(f64::from(angle)) * f64::from(radius)) as i32;
        points.push((x, z));
        angle += step / radius;
        if f64::from(angle) > std::f64::consts::PI * 2.0 {
            angle = 0.0;
            radius += step;
        }
    }
    points
}

fn parse_param(j: &Json) -> Option<(i64, i64)> {
    match j {
        Json::Arr(a) if a.len() == 2 => Some((quantize(a[0].as_f32()?), quantize(a[1].as_f32()?))),
        Json::Num(_) => {
            let q = quantize(j.as_f32()?);
            Some((q, q))
        }
        _ => None,
    }
}

impl DimGen {
    /// `dimension`: `minecraft:overworld`… (world height); `decorate`: decoration active
    /// (disabled for flat worlds without features).
    pub fn new(w: &WorldData, spec: &GenSpec, dimension: &str, seed: i64) -> Result<Self, Error> {
        let (min_y, height) = world_height(dimension);
        let dimension = full_id(dimension);
        match spec {
            GenSpec::Noise { settings, source } => {
                let src = match source {
                    SourceSpec::MultiNoise(p) => BiomeSource::MultiNoise(w.climate_list(p)?),
                    SourceSpec::TheEnd => BiomeSource::TheEnd,
                    SourceSpec::Fixed(b) => BiomeSource::Fixed(full_id(b)),
                    SourceSpec::Checkerboard { biomes, scale } => {
                        BiomeSource::checkerboard(&crate::biome::biome_list(&w.decor, biomes), *scale)?
                    }
                };
                let retro_parts = (min_y < crate::decor_region::RETRO_BAND.1)
                    .then(|| Box::new((w.pack.clone(), full_id(settings), src.clone())));
                let mut t = TerrainGenerator::new(&w.pack, &full_id(settings), src, seed, min_y, height)?;
                let to_state = (0..t.blocks.len())
                    .map(|i| {
                        let name = t.blocks.name(i as u16);
                        w.blocks.state_from_str(name).ok_or_else(|| Error::Missing(format!("state {name}")))
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let final_map_bits = to_state.iter().map(|&s| w.blocks.facts(s).hm).collect();
                let spawn_targets = Self::spawn_targets(&w.pack, settings, &mut t)?;
                let biome_names = t.biome_names.clone();
                let mut deco = Decorator::new(&w.decor, &w.blocks, &biome_names, seed)?;
                deco.templates = Some(w.templates.clone());
                deco.procs = Some(w.proc_lists());
                let s = &t.g.settings;
                let (sea_level, gen_min_y, gen_depth) = (s.sea_level, s.min_y, s.height);
                Ok(DimGen {
                    kind: Kind::Noise(Box::new(t)),
                    deco: Some(Arc::new(deco)),
                    blocks: w.blocks.clone(),
                    to_state,
                    final_map_bits,
                    conversion: ConversionOpts::from_env(),
                    reference_use: ReferenceUse::from_env(),
                    dimension,
                    seed,
                    min_y,
                    height,
                    sea_level,
                    gen_min_y,
                    gen_depth,
                    biome_names,
                    spawn_targets,
                    structures: None,
                    retro_parts,
                    retro_gen: std::sync::OnceLock::new(),
                })
            }
            GenSpec::Flat(f) => {
                let mut layers = Vec::new();
                for (name, n) in &f.layers {
                    let s = crate::data::with_full_id(name, |n| w.blocks.state_from_str(n))
                        .ok_or_else(|| Error::Missing(format!("state {name}")))?;
                    for _ in 0..*n {
                        layers.push(s);
                    }
                }
                // The game removes from the filling the layers that do not block movement and
                // places them as the last decoration step (where air remains).
                let air = w.blocks.default_state(w.blocks.k.air);
                let void_gen = layers.iter().all(|&s| w.blocks.is(s, w.blocks.k.air));
                let mut fill = Vec::new();
                for (i, s) in layers.iter_mut().enumerate() {
                    if w.blocks.facts(*s).hm & Hm::MotionBlocking.bit() == 0 {
                        fill.push((i as i32, *s));
                        *s = air;
                    }
                }
                layers.truncate(height as usize);
                let biome_names = vec![full_id(&f.biome)];
                let adj = crate::decor::FlatAdjust {
                    biome: f.biome.clone(),
                    lakes: f.lakes,
                    decoration: f.features,
                    void_gen,
                    fill,
                };
                let deco = Some(Arc::new(Decorator::new_flat(&w.decor, &w.blocks, &adj, seed)?));
                Ok(DimGen {
                    kind: Kind::Flat { layers, spec: Box::new(f.clone()), t: None },
                    deco,
                    blocks: w.blocks.clone(),
                    to_state: Vec::new(),
                    final_map_bits: Vec::new(),
                    conversion: ConversionOpts::from_env(),
                    reference_use: ReferenceUse::from_env(),
                    dimension,
                    seed,
                    min_y,
                    height,
                    sea_level: -63,
                    gen_min_y: 0,
                    gen_depth: 384,
                    biome_names,
                    spawn_targets: Vec::new(),
                    structures: None,
                    retro_parts: None,
                    retro_gen: std::sync::OnceLock::new(),
                })
            }
            GenSpec::Debug => Ok(DimGen {
                kind: Kind::Debug,
                deco: None,
                blocks: w.blocks.clone(),
                to_state: Vec::new(),
                final_map_bits: Vec::new(),
                conversion: ConversionOpts::from_env(),
                reference_use: ReferenceUse::from_env(),
                dimension,
                seed,
                min_y,
                height,
                sea_level: 63,
                gen_min_y: min_y,
                gen_depth: height,
                biome_names: vec!["minecraft:plains".into()],
                spawn_targets: Vec::new(),
                structures: None,
                retro_parts: None,
                retro_gen: std::sync::OnceLock::new(),
            }),
        }
    }

    fn spawn_targets(pack: &DataPack, settings: &str, t: &mut TerrainGenerator) -> Result<Vec<SpawnTarget>, Error> {
        let targets = if crate::opt::noise_settings_read_once() {
            t.g.settings.spawn_target.clone()
        } else {
            pack.json("noise_settings", settings)?.get("spawn_target").cloned()
        };
        let mut out = Vec::new();
        for point in targets.as_ref().and_then(Json::as_array).unwrap_or(&[]) {
            let Json::Obj(m) = point else { continue };
            let mut v = Vec::new();
            for (id, p) in m {
                let sid = t.g.compile_named(id)?;
                let range = parse_param(p).ok_or_else(|| Error::Format(format!("spawn_target {id}")))?;
                v.push((sid, range));
            }
            out.push(v);
        }
        Ok(out)
    }

    /// Enables the structures (world setting `generate_structures`); no effect outside noise.
    pub fn enable_structures(&mut self, w: &WorldData) -> Result<(), Error> {
        let flat = match &mut self.kind {
            Kind::Noise(_) => None,
            Kind::Flat { spec, t, .. } => {
                // "Generator" view of the flat world (heights, columns, single biome) and
                // decorator reduced to structures if the world has no decoration.
                // Non-blocking layers seen as air (removed from the filling).
                let names: Vec<String> = spec
                    .layers
                    .iter()
                    .flat_map(|(n, k)| {
                        let blocking = crate::data::with_full_id(n, |f| w.blocks.state_from_str(f))
                            .is_some_and(|s| w.blocks.facts(s).hm & Hm::MotionBlocking.bit() != 0);
                        let n = if blocking { full_id(n) } else { "minecraft:air".to_owned() };
                        std::iter::repeat_n(n, (*k).max(0) as usize)
                    })
                    .collect();
                *t = Some(Box::new(TerrainGenerator::flat(
                    &w.pack,
                    &names,
                    &spec.biome,
                    self.seed,
                    self.min_y,
                    self.height,
                )?));
                if let Some(d) = self.deco.as_mut().and_then(Arc::get_mut) {
                    d.templates = Some(w.templates.clone());
                    d.procs = Some(w.proc_lists());
                }
                Some(spec.structure_overrides.clone())
            }
            Kind::Debug => return Ok(()),
        };
        let mut sg = crate::structure::StructureGen::new(
            &w.decor,
            &w.blocks,
            w.templates.clone(),
            w.proc_lists(),
            &self.biome_names,
            self.seed,
            flat.is_some(),
        )?;
        if let Some(overrides) = flat {
            sg.keep_flat_sets(&w.decor, overrides.as_deref());
        }
        sg.deco = self.deco.clone();
        // Spawn search on the noise done only if a set is placed at the origin
        // (none in the game data: data packs only).
        if sg.sets.iter().any(|s| matches!(s.placement, crate::structure::Placement::Origin)) {
            sg.origin = self.spawn_chunk();
        }
        self.structures = Some(Arc::new(sg));
        Ok(())
    }

    /// Computes all the stronghold ring positions now (to call outside the pool).
    pub fn warm_rings(&self) {
        if let (Some(sg), Some(t)) = (&self.structures, self.structure_terrain()) {
            sg.warm_rings(t);
        }
    }

    /// Generator seen by the structures (noise, or flat world view).
    pub(crate) fn structure_terrain(&self) -> Option<&TerrainGenerator> {
        match &self.kind {
            Kind::Noise(t) => Some(t),
            Kind::Flat { t, .. } => t.as_deref(),
            Kind::Debug => None,
        }
    }

    /// Start of the structure `id` at chunk (cx, cz), placement rules and biome ignored
    /// (`place structure`); `None` without structures or if it cannot start there.
    pub fn structure_start_anywhere(&self, id: &str, cx: i32, cz: i32) -> Option<crate::structure::Start> {
        let (sg, t) = (self.structures.as_ref()?, self.structure_terrain()?);
        sg.start_anywhere(id, cx, cz, t, &self.blocks)
    }

    /// Jigsaw assembly from a pool at a position (jigsaw block, `place jigsaw`).
    pub fn jigsaw_start(&self, pool: &str, target: &str, depth: i32, pos: [i32; 3]) -> Option<crate::structure::Start> {
        let (sg, t) = (self.structures.as_ref()?, self.structure_terrain()?);
        sg.jigsaw_at(pool, target, depth, pos, t, &self.blocks)
    }

    /// Structure starts of a chunk (empty without structures).
    pub fn structure_starts(&self, cx: i32, cz: i32) -> Arc<Vec<crate::structure::Start>> {
        match (&self.structures, self.structure_terrain()) {
            (Some(sg), Some(t)) => sg.starts(cx, cz, t, &self.blocks),
            _ => Arc::new(Vec::new()),
        }
    }

    /// Disk-composed `structures` of a chunk (empty without structures).
    pub fn structures_nbt(&self, cx: i32, cz: i32) -> rc_proto::nbt::Nbt {
        use rc_proto::nbt::Nbt;
        match (&self.structures, self.structure_terrain()) {
            (Some(sg), Some(t)) => crate::structure_nbt::chunk_structures(sg, cx, cz, t, &self.blocks),
            _ => Nbt::Compound(vec![
                ("References".into(), Nbt::Compound(Vec::new())),
                ("starts".into(), Nbt::Compound(Vec::new())),
            ]),
        }
    }

    /// Writes into the chunk's `structures` compound the current state of its starts' pieces that
    /// placement changed (kept height, chest or spawner placed), as the game saves its live start.
    /// Called after a decoration for the region and before the chunk leaves as a full chunk: a
    /// stronghold corridor far from the starting chunk is placed outside its region. Not when a
    /// proto-chunk is written: that moment depends on evictions, and the file would with it.
    pub fn refresh_placement_state(&self, c: &mut DecoChunk) {
        let (Some(sg), Some(t)) = (&self.structures, self.structure_terrain()) else { return };
        let (cx, cz) = (c.cx, c.cz);
        if let Some(s) = &mut c.structures {
            crate::structure_nbt::refresh_placement_state(sg, cx, cz, t, &self.blocks, s);
        }
    }

    /// Like `structures_nbt` for a converted chunk (`below_zero_retrogen`): the starts
    /// whose box leaves the −64..−1 band are rejected.
    pub fn structures_nbt_retro(&self, cx: i32, cz: i32) -> rc_proto::nbt::Nbt {
        match (&self.structures, self.structure_terrain()) {
            (Some(sg), Some(t)) => crate::structure_nbt::chunk_structures_in(
                sg,
                cx,
                cz,
                t,
                &self.blocks,
                Some(crate::decor_region::RETRO_BAND),
            ),
            _ => self.structures_nbt(cx, cz),
        }
    }

    /// Terrain generator (noise worlds).
    pub fn terrain_generator(&self) -> Option<&TerrainGenerator> {
        match &self.kind {
            Kind::Noise(t) => Some(t),
            _ => None,
        }
    }

    /// Prepares the biomes of several chunks (in parallel) before their terrain.
    pub fn prepare_biomes(&self, cache: &mut BiomeCache, targets: &[(i32, i32)]) {
        if let Kind::Noise(t) = &self.kind {
            t.prepare_biomes(cache, targets);
        }
    }

    /// Chunk at status `terrain` (blocks, biomes, primed generation and final height maps,
    /// post-processing marks).
    pub fn terrain(
        &self,
        cx: i32,
        cz: i32,
        cache: &BiomeCache,
        structures: Option<&dyn StructureDensity>,
    ) -> DecoChunk {
        let flat_layers = match &self.kind {
            Kind::Noise(t) => return self.noise_terrain(t, cx, cz, cache, structures),
            Kind::Flat { layers, .. } => Some(layers),
            Kind::Debug => None,
        };
        let mut c = DecoChunk::new(cx, cz, self.min_y, self.height);
        c.blocks.fill(self.blocks.default_state(self.blocks.k.air));
        if let Some(layers) = flat_layers {
            for (i, &s) in layers.iter().enumerate() {
                let base = i * 256;
                c.blocks[base..base + 256].fill(s);
            }
            c.prime_maps(&self.blocks, &[Hm::WorldSurfaceWg, Hm::OceanFloorWg]);
        }
        c.prime_final_heightmaps(&self.blocks);
        c
    }

    /// [`Self::terrain`] of the noise generator `t`.
    fn noise_terrain(
        &self,
        t: &TerrainGenerator,
        cx: i32,
        cz: i32,
        cache: &BiomeCache,
        structures: Option<&dyn StructureDensity>,
    ) -> DecoChunk {
        // Structures active: beard of the referenced starts (except forced density).
        let own = match (&self.structures, structures) {
            (Some(sg), None) => self.beard(sg, cx, cz, &sg.references(cx, cz, t, &self.blocks)),
            _ => None,
        };
        let structures = structures.or_else(|| own.as_ref().map(|b| b as &dyn StructureDensity));
        self.noise_terrain_under(t, cx, cz, cache, structures)
    }

    /// Noise terrain of the chunk under the given structure density (beard), converted.
    fn noise_terrain_under(
        &self,
        t: &TerrainGenerator,
        cx: i32,
        cz: i32,
        cache: &BiomeCache,
        structures: Option<&dyn StructureDensity>,
    ) -> DecoChunk {
        let p = t.generate(cx, cz, cache, structures);
        let c = self.convert_noise_terrain(t, &p, cache);
        if self.conversion.reuse_proto_buffer {
            p.recycle_blocks();
        }
        c
    }

    /// Beard of the chunk's terrain, from its references.
    fn beard(&self, sg: &StructureGen, cx: i32, cz: i32, references: &References) -> Option<Beard> {
        if self.reference_use.borrowed_starts {
            Beard::new(cx, cz, &referenced_starts(references), &sg.defs)
        } else {
            Beard::new(cx, cz, &referenced_starts_cloned(references), &sg.defs)
        }
    }

    /// New chunk at status `terrain` with its `structures` compound (`saved`: the compound of a
    /// proto-chunk read from disk, kept as is). Its references are walked once, for the beard of
    /// the terrain and for the compound, then handed back for [`Self::decorate_reusing`].
    pub fn terrain_with_structures(
        &self,
        cx: i32,
        cz: i32,
        cache: &BiomeCache,
        saved: Option<rc_proto::nbt::Nbt>,
    ) -> (DecoChunk, Option<TerrainReferences>) {
        let (Some(sg), Some(t), true) =
            (&self.structures, self.structure_terrain(), self.reference_use.one_walk_for_terrain)
        else {
            // Old path: a walk inside the terrain, another for the compound.
            let mut c = self.terrain(cx, cz, cache, None);
            c.structures = Some(saved.unwrap_or_else(|| self.structures_nbt(cx, cz)));
            return (c, None);
        };
        // Counted before the walk: a clear during it must leave these references stale.
        let start_cache_clears = sg.starts_clears();
        let references = sg.references(cx, cz, t, &self.blocks);
        let mut c = match &self.kind {
            Kind::Noise(noise) => {
                let beard = self.beard(sg, cx, cz, &references);
                self.noise_terrain_under(noise, cx, cz, cache, beard.as_ref().map(|b| b as &dyn StructureDensity))
            }
            // Flat and debug worlds: the structures do not shape the terrain.
            Kind::Flat { .. } | Kind::Debug => self.terrain(cx, cz, cache, None),
        };
        c.structures = Some(saved.unwrap_or_else(|| {
            crate::structure_nbt::chunk_structures_from(sg, cx, cz, t, &self.blocks, None, &references)
        }));
        let kept = self
            .reference_use
            .kept_for_decoration
            .then_some(TerrainReferences { chunk: (cx, cz), references, start_cache_clears });
        (c, kept)
    }

    /// Decoration chunk of the proto-chunk `p` of the noise generator `t`: network states,
    /// generation maps, biomes, fluid marks and final maps (public for the conversion bench).
    #[doc(hidden)]
    pub fn convert_noise_terrain(&self, t: &TerrainGenerator, p: &ProtoTerrain, cache: &BiomeCache) -> DecoChunk {
        let (cx, cz) = (p.cx, p.cz);
        let mut c = DecoChunk::new(cx, cz, self.min_y, self.height);
        let final_maps = self.convert_noise_blocks(&p.blocks, &mut c);
        for i in 0..256 {
            c.heights[Hm::WorldSurfaceWg as usize][i] = p.world_surface_wg[i] as i16;
            c.heights[Hm::OceanFloorWg as usize][i] = p.ocean_floor_wg[i] as i16;
        }
        match cache.chunks.get(&(cx, cz)) {
            Some(b) => c.biomes.copy_from_slice(&b.data),
            None => c.biomes.copy_from_slice(&t.chunk_biomes(cx, cz).data),
        }
        for &m in &p.post_processing {
            let sec = (m >> 16) as i32;
            let (x, y, z) = ((m & 15) as i32, ((m >> 4) & 15) as i32, ((m >> 8) & 15) as i32);
            c.post_process.push([cx * 16 + x, self.min_y + sec * 16 + y, cz * 16 + z]);
        }
        match final_maps {
            FinalMapsToDo::Nothing => {}
            FinalMapsToDo::WalkBelow(top) if crate::opt::prime_one_walk() => {
                c.prime_many(&self.blocks, &FINAL_HMS, top);
            }
            FinalMapsToDo::WalkBelow(_) | FinalMapsToDo::WalkAll => c.prime_final_heightmaps(&self.blocks),
        }
        c
    }

    /// Network states of the terrain blocks `terrain` written into the new chunk `c`, on the
    /// paths of [`Self::conversion`].
    fn convert_noise_blocks(&self, terrain: &[BlockId], c: &mut DecoChunk) -> FinalMapsToDo {
        let o = self.conversion;
        let air = self.blocks.default_state(self.blocks.k.air);
        // Skipping a section of plain air is exact while plain air counts in no height map (true
        // with the game's tags, checked here once).
        let skip_air = o.skip_air_sections && self.final_map_bits[AIR as usize] == 0;
        if o.heightmaps_while_converting {
            c.convert_terrain_finding_final_maps(terrain, &self.to_state, &self.final_map_bits, air, skip_air, true);
            FinalMapsToDo::Nothing
        } else if skip_air {
            FinalMapsToDo::WalkBelow(c.convert_terrain_skipping_air(terrain, &self.to_state, air, true))
        } else {
            for (dst, &b) in c.blocks.iter_mut().zip(terrain) {
                *dst = self.to_state[b as usize];
            }
            FinalMapsToDo::WalkAll
        }
    }

    /// Regeneration generator below y = 0 (built at first need), and its state table.
    fn retro_gen(&self) -> Option<&RetroGen> {
        self.retro_gen
            .get_or_init(|| {
                let p = self.retro_parts.as_ref()?;
                let t = TerrainGenerator::retro(&p.0, &p.1, p.2.clone(), self.seed).ok()?;
                if t.biome_names != self.biome_names {
                    return None;
                }
                let to_state = (0..t.blocks.len())
                    .map(|i| self.blocks.state_from_str(t.blocks.name(i as u16)))
                    .collect::<Option<Vec<_>>>()?;
                Some((Box::new(t), to_state))
            })
            .as_ref()
    }

    /// Regeneration below y = 0 of an old converted chunk (`below_zero_retrogen`): biomes of
    /// the −64..−1 band (cave biomes of the noise kept, otherwise the old biome of the column at
    /// y = 0), terrain of the band (noise, surface, aquifers, caves) under the kept old blocks,
    /// old bedrock (y 0 to 4) changed to deepslate, columns without bedrock emptied over
    /// the band, height maps redone. `None`: not a converted chunk, or generator without
    /// band below 0. The blend with the old terrain is not reproduced.
    pub fn retro_terrain(&self, old: &DecoChunk) -> Option<DecoChunk> {
        let retro = old.retro.as_ref()?;
        let (t, to_state) = self.retro_gen()?;
        let (lo, hi) = crate::decor_region::RETRO_BAND;
        if old.min_y != lo {
            return None;
        }
        let (cx, cz) = (old.cx, old.cz);
        let mut cache = BiomeCache::default();
        t.prepare_biomes(&mut cache, &[(cx, cz)]);
        let retained: Vec<bool> = t.biome_names.iter().map(|n| RETAINED_RETRO_BIOMES.contains(&n.as_str())).collect();
        let qy0 = (0 - old.min_y) >> 2;
        let band_biomes = {
            let cb = cache.chunks.get_mut(&(cx, cz))?;
            for qy in 0..cb.height_q {
                for qz in 0..4 {
                    for qx in 0..4 {
                        let i = ((qy * 4 + qz) * 4 + qx) as usize;
                        if !retained.get(cb.data[i] as usize).copied().unwrap_or(false) {
                            cb.data[i] = old.biomes[((qy0 * 4 + qz) * 4 + qx) as usize];
                        }
                    }
                }
            }
            cb.data.clone()
        };
        let p = t.generate(cx, cz, &cache, None);
        let mut c = old.clone();
        let b = &self.blocks;
        if self.conversion.skip_air_sections {
            // The old blocks of the band are overwritten, plain air sections included.
            c.convert_terrain_skipping_air(&p.blocks, to_state, b.default_state(b.k.air), false);
        } else {
            for (dst, &id) in c.blocks.iter_mut().zip(&p.blocks) {
                *dst = to_state[id as usize];
            }
        }
        c.biomes[..band_biomes.len()].copy_from_slice(&band_biomes);
        let deepslate = b.default_state(b.block_id("deepslate")?);
        let bedrock = b.block_id("bedrock")?;
        for y in 0..=4 {
            for z in 0..16 {
                for x in 0..16 {
                    let i = c.index(x, y, z);
                    if b.is(c.blocks[i], bedrock) {
                        c.blocks[i] = deepslate;
                    }
                }
            }
        }
        let air = b.default_state(b.k.air);
        for z in 0..16 {
            for x in 0..16 {
                if retro.hole(x, z) {
                    for y in lo..hi {
                        let i = c.index(x, y, z);
                        c.blocks[i] = air;
                    }
                }
            }
        }
        for i in 0..256 {
            c.heights[Hm::WorldSurfaceWg as usize][i] = p.world_surface_wg[i] as i16;
            c.heights[Hm::OceanFloorWg as usize][i] = p.ocean_floor_wg[i] as i16;
        }
        for &m in &p.post_processing {
            let sec = (m >> 16) as i32;
            let (x, y, z) = ((m & 15) as i32, ((m >> 4) & 15) as i32, ((m >> 8) & 15) as i32);
            c.post_process.push([cx * 16 + x, lo + sec * 16 + y, cz * 16 + z]);
        }
        if self.conversion.reuse_proto_buffer {
            p.recycle_blocks();
        }
        c.prime_final_heightmaps(b);
        Some(c)
    }

    /// Decorates the central chunk of the 3×3 (z then x increasing order, centre at 4).
    pub fn decorate(&self, chunks: [&mut DecoChunk; 9]) -> DecorStats {
        self.decorate_reusing(chunks, None)
    }

    /// [`Self::decorate`]; `kept`: references of the central chunk from
    /// [`Self::terrain_with_structures`], used instead of a new walk while they are still current.
    pub fn decorate_reusing(&self, chunks: [&mut DecoChunk; 9], kept: Option<&TerrainReferences>) -> DecorStats {
        if let Kind::Debug = self.kind {
            let mut r = Region::new(chunks, &self.blocks, self.sea_level, self.seed);
            let (bx, bz) = (r.cx * 16, r.cz * 16);
            let b = &self.blocks;
            let (air, barrier) = (b.default_state(b.k.air), b.default_state(b.block_id("barrier").unwrap_or(b.k.air)));
            for x in 0..16 {
                for z in 0..16 {
                    let (wx, wz) = (bx + x, bz + z);
                    // "Debug" world: the game refuses any placement outside generation, post-processing
                    // therefore changes nothing (no mark).
                    r.set(wx, 60, wz, barrier, 2);
                    r.set(wx, 70, wz, debug_state_at(wx, wz, b.state_count(), air), 2 | 16);
                }
            }
            return DecorStats::default();
        }
        let Some(d) = &self.deco else { return DecorStats::default() };
        let retro_mask: [bool; 9] = std::array::from_fn(|i| chunks[i].retro.is_some());
        // Scattered pieces and shipwrecks only occupy their starting chunk and its neighbours,
        // always in the region of whoever places them: their recorded state (kept height) is
        // re-read before, the current state rewritten after.
        if let (Some(sg), Some(t)) = (&self.structures, self.structure_terrain()) {
            for c in &chunks {
                if let Some(s) = &c.structures {
                    crate::structure_nbt::restore_placement_state(sg, c.cx, c.cz, t, &self.blocks, s);
                }
            }
        }
        let mut r =
            Region::new(chunks, &self.blocks, self.sea_level, self.seed).with_generator(self.gen_min_y, self.gen_depth);
        match (&self.structures, self.structure_terrain()) {
            (Some(sg), Some(t)) => {
                // Checked here, after the scattered pieces were re-read above: reading them can
                // clear the start cache.
                let kept = kept
                    .filter(|k| self.reference_use.kept_for_decoration && k.still_current(sg, (r.cx, r.cz)));
                let walked;
                let references = match kept {
                    Some(k) => &k.references,
                    None => {
                        walked = sg.references(r.cx, r.cz, t, &self.blocks);
                        &walked
                    }
                };
                let stats = if self.reference_use.borrowed_starts {
                    place_structures_while_decorating(d, &mut r, sg, t, referenced_starts(references), &retro_mask)
                } else {
                    place_structures_while_decorating(d, &mut r, sg, t, referenced_starts_cloned(references), &retro_mask)
                };
                for c in r.chunks.iter_mut() {
                    self.refresh_placement_state(c);
                }
                stats
            }
            _ => d.decorate(&mut r),
        }
    }

    /// Origin chunk of the spawn (search on the climate noise; (0, 0) without target).
    pub fn spawn_chunk(&self) -> (i32, i32) {
        let Kind::Noise(t) = &self.kind else { return (0, 0) };
        if self.spawn_targets.is_empty() {
            return (0, 0);
        }
        let fitness = |x: i32, z: i32| -> i64 {
            let (qx, qz) = ((x >> 2) << 2, (z >> 2) << 2);
            let mut ctx = Ctx::uncached();
            let mut best = i64::MAX;
            for point in &self.spawn_targets {
                let mut f = 0i64;
                for &(sid, range) in point {
                    let v = quantize(t.g.value(sid, &mut ctx, qx, 0, qz));
                    let d = param_distance(range, v);
                    f = f.wrapping_add(d.wrapping_mul(d));
                }
                best = best.min(f);
            }
            let bias = i64::from(x) * i64::from(x) + i64::from(z) * i64::from(z);
            best.wrapping_mul(2048 * 2048).wrapping_add(bias)
        };
        let parallel = crate::opt::parallel_spawn_search();
        let mut best = ((0, 0), fitness(0, 0));
        for (max_r, step) in [(2048.0f32, 512.0f32), (512.0, 32.0)] {
            let points = spawn_spiral(best.0, max_r, step);
            // The points of a spiral do not depend on the scores: they can all be scored at once,
            // the strict comparison in spiral order below keeps the first best one either way.
            let scores: Vec<i64> = if parallel {
                use rayon::prelude::*;
                points.par_iter().map(|&(x, z)| fitness(x, z)).collect()
            } else {
                points.iter().map(|&(x, z)| fitness(x, z)).collect()
            };
            for (p, f) in points.into_iter().zip(scores) {
                if f < best.1 {
                    best = (p, f);
                }
            }
        }
        (best.0.0 >> 4, best.0.1 >> 4)
    }

    /// Fallback spawn height of the generator (the game: 64; flat world: top of the layers).
    pub fn spawn_height(&self) -> i32 {
        match &self.kind {
            Kind::Flat { layers, .. } => self.min_y + layers.len() as i32,
            _ => 64,
        }
    }
}

/// Decoration of the region's central chunk with the pieces of its referenced starts placed at
/// each step (`retro_mask`: converted chunks of the 3×3, see [`retro_reject_starts`]).
fn place_structures_while_decorating<S: Borrow<Start>>(
    d: &Decorator,
    r: &mut Region,
    sg: &StructureGen,
    t: &TerrainGenerator,
    mut referenced: Vec<(usize, Vec<S>)>,
    retro_mask: &[bool; 9],
) -> DecorStats {
    retro_reject_starts(&mut referenced, (r.cx, r.cz), retro_mask);
    let mut place = |step: usize, cx: &mut crate::decor::Ctx, rng: &mut crate::decor_rng::DecoRng, seed: i64| {
        crate::structure_place::place_step(cx, sg, t, &referenced, step, rng, seed);
    };
    d.decorate_with(r, Some(&mut place))
}

/// Rejection of the starts of a converted chunk (`below_zero_retrogen`): the game refuses any start
/// whose box leaves the −64..−1 band. `mask`: converted chunks
/// of the 3×3 (centre at 4). A start born outside the 3×3 follows the state of the central chunk
/// (old converted worlds are in one piece; the state of the distant ones is not known here).
fn retro_reject_starts<S: Borrow<Start>>(refs: &mut [(usize, Vec<S>)], center: (i32, i32), mask: &[bool; 9]) {
    let (lo, hi) = crate::decor_region::RETRO_BAND;
    for (_, v) in refs.iter_mut() {
        v.retain(|s| {
            let s: &Start = s.borrow();
            let (dx, dz) = (s.cx - center.0, s.cz - center.1);
            let retro = if dx.abs() <= 1 && dz.abs() <= 1 { mask[((dz + 1) * 3 + dx + 1) as usize] } else { mask[4] };
            !retro || (s.bbox.min[1] >= lo && s.bbox.max[1] < hi)
        });
    }
}

#[cfg(test)]
#[path = "generator_sharing_tests.rs"]
mod generator_sharing_tests;

#[cfg(test)]
mod tests {
    use super::*;

    fn start_at(cx: i32, cz: i32, y0: i32, y1: i32) -> crate::structure::Start {
        crate::structure::Start {
            structure: 0,
            cx,
            cz,
            pieces: Vec::new(),
            bbox: crate::template::BBox::new([cx * 16, y0, cz * 16], [cx * 16 + 5, y1, cz * 16 + 5]),
        }
    }

    /// Converted chunk: a start whose box leaves −64..−1 is rejected, a start born
    /// in a non-converted neighbour of the 3×3 stays, a non-converted chunk rejects nothing.
    /// Borrowed starts (the decoration's path) and owned copies (old path) alike.
    #[test]
    fn retro_chunks_reject_starts_outside_the_band() {
        let mut mask = [false; 9];
        mask[4] = true;
        mask[5] = true; // (1, 0) converted
        let starts = vec![
            start_at(0, 0, -60, -2),  // in the band: kept
            start_at(0, 0, -60, 10),  // exceeds y = −1: rejected
            start_at(1, 0, -70, -2),  // under −64: rejected
            start_at(-1, 0, -60, 40), // non-converted neighbour: kept
            start_at(5, 5, -60, 40),  // outside the 3×3: follows the centre, rejected
        ];
        let mut borrowed: Vec<(usize, Vec<&Start>)> = vec![(0, starts.iter().collect())];
        retro_reject_starts(&mut borrowed, (0, 0), &mask);
        let kept: Vec<(i32, i32)> = borrowed[0].1.iter().map(|s| (s.cx, s.cz)).collect();
        assert_eq!(kept, vec![(0, 0), (-1, 0)]);
        let mut owned = vec![(0usize, starts.clone())];
        retro_reject_starts(&mut owned, (0, 0), &mask);
        let kept: Vec<(i32, i32)> = owned[0].1.iter().map(|s| (s.cx, s.cz)).collect();
        assert_eq!(kept, vec![(0, 0), (-1, 0)]);
        let mut plain = vec![(0usize, vec![start_at(0, 0, -60, 10)])];
        retro_reject_starts(&mut plain, (0, 0), &[false; 9]);
        assert_eq!(plain[0].1.len(), 1);
    }

    /// Grid of the "debug" world: odd positive cells, index x·width + z, air elsewhere
    /// and past the last state (width and height rounded up in floating point).
    #[test]
    fn debug_grid_layout() {
        let n = 30_000;
        let width = (n as f32).sqrt().ceil() as i32;
        assert_eq!(width, 174);
        let air = 0;
        assert_eq!(debug_state_at(1, 1, n, air), 0);
        assert_eq!(debug_state_at(1, 3, n, air), 1);
        assert_eq!(debug_state_at(3, 1, n, air), width as State);
        assert_eq!(debug_state_at(2, 3, n, air), air);
        assert_eq!(debug_state_at(-1, 3, n, air), air);
        assert_eq!(debug_state_at(0, 3, n, air), air);
        // Last partial row: indices outside the list = air.
        let last = (n as i32 - 1) / width;
        assert_eq!(debug_state_at(last * 2 + 1, (n as i32 - 1 - last * width) * 2 + 1, n, air), (n - 1) as State);
        assert_eq!(debug_state_at(last * 2 + 1, (width - 1) * 2 + 1, n, air), air);
    }
}
