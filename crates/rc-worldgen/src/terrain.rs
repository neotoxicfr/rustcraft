//! `terrain` step of a chunk (spec generation-terrain § 1, § 3, § 4, § 7): filling by the
//! noise and the aquifers, surface rules, carved caves. Same seed → same blocks as the game.
//!
//! A [`TerrainGenerator`] is prepared once per dimension and seed; it is shareable between
//! threads. Each chunk has its own sampling context (caches) and its calls follow
//! the game's order, which guarantees bit-for-bit identical values.

use hashbrown::HashMap;

use rayon::prelude::*;

use crate::aquifer::{Aquifer, AquiferConfig, FluidPicker};
use crate::biome::{BiomeSource, ChunkBiomes, Searcher};
use crate::carver::{Carver, CarvingMask, Target, large_feature_random};
use crate::data::{DataPack, full_id};
use crate::density::{Binary, Df, Resolver, Simple};
use crate::json::Json;
use crate::material::{BiomeView, Env, GenContext, MaterialSystem, possible_biomes};
use crate::palette::{AIR, BlockId, BlockTable};
use crate::proto::ProtoTerrain;
use crate::sampler::Ctx;
use crate::volume::Volume;
use crate::{Error, Generator};

/// Contribution of the structures to the final density ("beardifier", spec § 9): interface for the
/// structures part, not implemented here. It adds to the density buffer of the filling
/// volume (indices of [`Volume::index`]); without structure, nothing is added (value 0).
pub trait StructureDensity: Sync {
    fn add_to_volume(&self, out: &mut [f32], v: &Volume);
}

/// Sub-steps of `terrain`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Fill,
    Surface,
    Carvers,
}

/// World height of a dimension (dimension type): `min_y`, `height`.
pub fn world_height(dimension: &str) -> (i32, i32) {
    crate::data::with_full_id(dimension, |d| match d {
        "minecraft:the_nether" | "minecraft:the_end" => (0, 256),
        _ => (-64, 384),
    })
}

/// Optimised paths of the terrain. Default: `RUSTCRAFT_OPT_*` switches read at startup
/// ([`crate::opt`]); the equivalence tests set them to compare old and new path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerrainOpts {
    /// Identifiers dirt, grass, mycelium, water and lava resolved once.
    pub ids: bool,
    /// Filling: window of the aquifer cells per column.
    pub aq_colwin: bool,
    /// Filling: 4 nearest through branchless network (with `aq_colwin`).
    pub aq_net: bool,
    /// Filling: positive density tested before the aquifer.
    pub aq_pretest: bool,
    /// Base height: density by slices, stop at the first block found.
    pub baseh_lazy: bool,
    /// Base height: shared memo.
    pub baseh_memo: bool,
    /// Caves: vertical difference per y precomputed.
    pub carver_yd: bool,
    /// Canyons: width buffer reused.
    pub carver_widths: bool,
    /// Caves: carvings recorded per source and replayed.
    pub carver_replay: bool,
}

impl TerrainOpts {
    pub const OLD: TerrainOpts = TerrainOpts {
        ids: false,
        aq_colwin: false,
        aq_net: false,
        aq_pretest: false,
        baseh_lazy: false,
        baseh_memo: false,
        carver_yd: false,
        carver_widths: false,
        carver_replay: false,
    };
    pub const NEW: TerrainOpts = TerrainOpts {
        ids: true,
        aq_colwin: true,
        aq_net: true,
        aq_pretest: true,
        baseh_lazy: true,
        baseh_memo: true,
        carver_yd: true,
        carver_widths: true,
        carver_replay: true,
    };

    pub fn from_env() -> Self {
        use crate::opt;
        TerrainOpts {
            ids: opt::terrain_ids(),
            aq_colwin: opt::aq_colwin(),
            aq_net: opt::aq_simd(),
            aq_pretest: opt::aq_pretest(),
            baseh_lazy: opt::baseh_lazy(),
            baseh_memo: opt::baseh_memo(),
            carver_yd: opt::carver_yd(),
            carver_widths: opt::carver_widths(),
            carver_replay: opt::carver_replay(),
        }
    }
}

/// Identifiers resolved once (`TerrainOpts::ids`).
#[derive(Debug, Clone, Copy)]
struct TerrainIds {
    lava: BlockId,
    water: BlockId,
    dirt: Option<BlockId>,
    grass: Option<BlockId>,
    mycelium: Option<BlockId>,
}

impl TerrainIds {
    fn resolve(b: &BlockTable) -> Self {
        TerrainIds {
            lava: b.get("minecraft:lava").unwrap_or(AIR),
            water: b.get("minecraft:water").unwrap_or(AIR),
            dirt: b.get("minecraft:dirt"),
            grass: b.get("minecraft:grass_block"),
            mycelium: b.get("minecraft:mycelium"),
        }
    }
}

/// Memo of the base heights: 16 tables under lock, cleared beyond `BASEH_MEMO_CAP` entries
/// each (≈ 16 × 4,096 × 16 bytes at most).
const BASEH_MEMO_CAP: usize = 4096;

type BaseHeightShard = HashMap<(i32, i32, bool), i32>;

#[derive(Debug, Default)]
struct BaseHeightMemo {
    shards: [std::sync::Mutex<BaseHeightShard>; 16],
    calls: std::sync::atomic::AtomicU64,
    hits: std::sync::atomic::AtomicU64,
    probes: std::sync::atomic::AtomicU64,
    probe_hits: std::sync::atomic::AtomicU64,
}

impl BaseHeightMemo {
    /// The top probe is worth it: fewer than 64 attempts, or at least one success in 4.
    fn probe_worth(&self) -> bool {
        use std::sync::atomic::Ordering::Relaxed;
        let n = self.probes.load(Relaxed);
        n < 64 || self.probe_hits.load(Relaxed) * 4 >= n
    }

    fn probe(&self, hit: bool) {
        use std::sync::atomic::Ordering::Relaxed;
        self.probes.fetch_add(1, Relaxed);
        if hit {
            self.probe_hits.fetch_add(1, Relaxed);
        }
    }
}

/// Terrain generator of a dimension.
#[derive(Debug)]
pub struct TerrainGenerator {
    pub g: Generator,
    pub blocks: BlockTable,
    pub source: BiomeSource,
    pub biome_names: Vec<String>,
    pub zoom_seed: i64,
    pub world_min_y: i32,
    pub world_height: i32,
    pub gc: GenContext,
    picker: FluidPicker,
    aquifer: Option<AquiferConfig>,
    pub material: MaterialSystem,
    carvers: Vec<Carver>,
    biome_carvers: Vec<Vec<u16>>,
    default_block: BlockId,
    beard_ok: bool,
    /// Flat world: column of the layers from the world bottom (heights and base columns of the
    /// structures); `None` for a noise world.
    flat: Option<Vec<BlockId>>,
    /// Blocks protected at the top of the generator against the caves (7; 0 for the regeneration
    /// below y = 0 of a converted chunk).
    protect_top: i32,
    /// Optimised paths (default switches).
    pub opts: TerrainOpts,
    ids: TerrainIds,
    /// Height of the slices of the lazy base height: common multiple of the interpolation
    /// cell heights (aligned slices = same values as the full volume).
    lazy_step: i32,
    /// Cells per slice of the lazy base height (setting measured on the micro-bench).
    #[doc(hidden)]
    pub lazy_cells: i32,
    /// Splitting of the lazy base height (0 fixed, 1 doubling, 2 probe then rest,
    /// 3 adaptive probe).
    #[doc(hidden)]
    pub lazy_mode: u8,
    baseh: BaseHeightMemo,
    replay: crate::carver::ReplayCache,
}

/// Biomes needed by the terrain of a set of chunks: biomes of the chunks (and neighbours) and
/// "cave" biome of the source chunks (within ±8).
#[derive(Debug, Default)]
pub struct BiomeCache {
    pub chunks: HashMap<(i32, i32), ChunkBiomes>,
    pub carver: HashMap<(i32, i32), u16>,
}

impl TerrainGenerator {
    /// From a world preset (`minecraft:normal`, `large_biomes`, `amplified`…) and a
    /// dimension (`minecraft:overworld`, `the_nether`, `the_end`).
    pub fn from_preset(pack: &DataPack, preset: &str, dimension: &str, seed: i64) -> Result<Self, Error> {
        let (source, settings) = BiomeSource::from_preset(pack, preset, dimension)?;
        let (min_y, height) = world_height(dimension);
        Self::new(pack, &settings, source, seed, min_y, height)
    }

    pub fn new(
        pack: &DataPack,
        settings_id: &str,
        source: BiomeSource,
        seed: i64,
        world_min_y: i32,
        world_height: i32,
    ) -> Result<Self, Error> {
        let mut g = Generator::new(pack, settings_id, seed)?;
        let mut blocks = BlockTable::new(pack);
        let s = g.settings.clone();
        let default_block = blocks.intern(&s.default_block);
        let default_fluid = blocks.intern(&s.default_fluid);
        let lava = blocks.intern("minecraft:lava");
        blocks.intern("minecraft:water");
        let gc = GenContext { min_y: world_min_y.max(s.min_y), depth: world_height.min(s.height), sea: s.sea_level };
        let aquifer = match &s.aquifers {
            Some(a) => {
                let mut f = |k: &str| -> Result<crate::sampler::Sid, Error> {
                    let d = crate::density::parse(a.get(k).ok_or_else(|| Error::Format(format!("aquifers.{k}")))?)?;
                    g.compile_function(&d)
                };
                Some(AquiferConfig {
                    barrier: f("barrier")?,
                    floodedness: f("fluid_level_floodedness")?,
                    spread: f("fluid_level_spread")?,
                    lava: f("lava")?,
                    exclusion: f("exclusion")?,
                    surface_level: f("surface_level")?,
                    random: g.positional().from_hash_of("minecraft:aquifer").fork_positional(),
                })
            }
            None => None,
        };
        let biome_names = source.names();
        let rule = s.material_rule.clone().ok_or_else(|| Error::Missing("material_rule".into()))?;
        let material = MaterialSystem::new(pack, &mut g, &mut blocks, &rule, &biome_names, gc, default_block)?;
        let mut carvers = Vec::new();
        let mut carver_ids: HashMap<String, u16> = HashMap::new();
        let mut biome_carvers = Vec::with_capacity(biome_names.len());
        for name in &biome_names {
            let mut list = Vec::new();
            if let Ok(j) = pack.json("biome", name) {
                let ids: Vec<String> = match j.get("carvers") {
                    Some(Json::Str(s)) => vec![s.clone()],
                    Some(Json::Arr(a)) => a.iter().filter_map(Json::as_str).map(str::to_owned).collect(),
                    _ => Vec::new(),
                };
                for id in ids {
                    if id.starts_with('#') {
                        return Err(Error::Format(format!("unhandled cave tag: {id}")));
                    }
                    let id = full_id(&id);
                    let k = match carver_ids.get(&id) {
                        Some(k) => *k,
                        None => {
                            carvers.push(Carver::load(pack, &id, &gc)?);
                            let k = (carvers.len() - 1) as u16;
                            carver_ids.insert(id, k);
                            k
                        }
                    };
                    list.push(k);
                }
            }
            biome_carvers.push(list);
        }
        // The contribution of the structures can only be added afterwards if the final density is
        // `x + beardifier` at the top.
        let mut top = g.settings.router[7].clone();
        while let Df::Ref(id) = &*top {
            match g.defs().function(id) {
                Some(f) => top = f,
                None => break,
            }
        }
        let beard_ok = matches!(&*top, Df::Binary(Binary::Add, _, r) if **r == Df::Simple(Simple::Beardifier));
        let lazy_step = g.program.nodes.iter().fold(8i64, |l, n| match n {
            crate::sampler::S::Interpolated { cy, .. } => {
                let (mut a, mut b) = (l, i64::from(*cy));
                while b != 0 {
                    (a, b) = (b, a % b);
                }
                l / a * i64::from(*cy)
            }
            _ => l,
        });
        let lazy_step = i32::try_from(lazy_step).unwrap_or(i32::MAX);
        let ids = TerrainIds::resolve(&blocks);
        Ok(TerrainGenerator {
            opts: TerrainOpts::from_env(),
            ids,
            lazy_step,
            lazy_cells: 1,
            lazy_mode: 3,
            baseh: BaseHeightMemo::default(),
            replay: crate::carver::ReplayCache::default(),
            zoom_seed: crate::rng::biome_zoom_seed(seed),
            picker: FluidPicker::new(s.sea_level, default_fluid, lava),
            g,
            blocks,
            source,
            biome_names,
            world_min_y,
            world_height,
            gc,
            aquifer,
            material,
            carvers,
            biome_carvers,
            default_block,
            beard_ok,
            flat: None,
            protect_top: 7,
        })
    }

    /// Generator of the regeneration below y = 0 of an old converted chunk: generation height
    /// −64..−1 (noise, surface, aquifers, caves bounded to the band, no protection at
    /// the top), same settings and same seed.
    pub fn retro(pack: &DataPack, settings_id: &str, source: BiomeSource, seed: i64) -> Result<Self, Error> {
        let (lo, hi) = crate::decor_region::RETRO_BAND;
        let mut t = Self::new(pack, settings_id, source, seed, lo, hi - lo)?;
        t.protect_top = 0;
        Ok(t)
    }

    /// Generator seen by the structures of a flat world: single biome, given layers (from bottom
    /// to top), generator starting at y 0 over 384 depth, sea at −63 (fixed values of the
    /// game for this generator). The noise is never sampled (settings of the main world
    /// kept only for the seed).
    pub fn flat(
        pack: &DataPack,
        layers: &[String],
        biome: &str,
        seed: i64,
        world_min_y: i32,
        world_height: i32,
    ) -> Result<Self, Error> {
        let mut t = Self::new(
            pack,
            "minecraft:overworld",
            BiomeSource::Fixed(full_id(biome)),
            seed,
            world_min_y,
            world_height,
        )?;
        let col: Vec<BlockId> = layers.iter().take(world_height as usize).map(|l| t.blocks.intern(l)).collect();
        t.gc = GenContext { min_y: world_min_y.max(0), depth: world_height.min(384), sea: -63 };
        t.g.settings.min_y = 0;
        t.g.settings.sea_level = -63;
        t.flat = Some(col);
        t.ids = TerrainIds::resolve(&t.blocks);
        Ok(t)
    }

    pub fn seed(&self) -> i64 {
        self.g.seed
    }

    /// Height context (generator bottom and depth bounded to the world, sea).
    pub fn gen_ctx(&self) -> crate::decor_pred::GenCtx {
        crate::decor_pred::GenCtx { min_y: self.gc.min_y, depth: self.gc.depth, sea_level: self.gc.sea }
    }

    /// Biomes of a chunk over the world height (step `biomes`).
    pub fn chunk_biomes(&self, cx: i32, cz: i32) -> ChunkBiomes {
        self.source.chunk_range(&self.g, cx, cz, self.world_min_y, self.world_height, &mut Searcher::default())
    }

    /// Biome of a source chunk for its caves: quart (min_x, 0, min_z), source without cache.
    pub fn carver_biome(&self, cx: i32, cz: i32) -> u16 {
        self.source.biome_at(&self.g, &mut Ctx::uncached(), &mut Searcher::default(), cx * 4, 0, cz * 4)
    }

    /// Fills the biome cache to generate `targets` (in parallel, without recomputation).
    pub fn prepare_biomes(&self, cache: &mut BiomeCache, targets: &[(i32, i32)]) {
        let mut need_c: Vec<(i32, i32)> = Vec::new();
        let mut need_k: Vec<(i32, i32)> = Vec::new();
        let mut seen_c = hashbrown::HashSet::new();
        let mut seen_k = hashbrown::HashSet::new();
        for &(cx, cz) in targets {
            for dx in -1..=1 {
                for dz in -1..=1 {
                    let p = (cx + dx, cz + dz);
                    if !cache.chunks.contains_key(&p) && seen_c.insert(p) {
                        need_c.push(p);
                    }
                }
            }
            for dx in -8..=8 {
                for dz in -8..=8 {
                    let p = (cx + dx, cz + dz);
                    if !cache.carver.contains_key(&p) && seen_k.insert(p) {
                        need_k.push(p);
                    }
                }
            }
        }
        let c: Vec<_> = need_c.par_iter().map(|&(x, z)| ((x, z), self.chunk_biomes(x, z))).collect();
        cache.chunks.extend(c);
        let k: Vec<_> = need_k.par_iter().map(|&(x, z)| ((x, z), self.carver_biome(x, z))).collect();
        cache.carver.extend(k);
    }

    /// Filling volume of the chunk (noise height bounded to the world), `None` if it is empty.
    pub fn fill_volume(&self, cx: i32, cz: i32) -> Option<Volume> {
        let s = &self.g.settings;
        let min_y = s.min_y.max(self.world_min_y);
        let h = (s.min_y + s.height).min(self.world_min_y + self.world_height) - min_y;
        (h > 0).then(|| Volume::new(16, h, 16, cx * 16, min_y, cz * 16))
    }

    /// Terrain of a chunk (end of the `terrain` step, before decoration). The biomes missing from the
    /// cache are computed on the fly.
    pub fn generate(
        &self,
        cx: i32,
        cz: i32,
        cache: &BiomeCache,
        structures: Option<&dyn StructureDensity>,
    ) -> ProtoTerrain {
        self.generate_until(cx, cz, cache, structures, Stage::Carvers)
    }

    /// Like [`Self::generate`], stopped after the sub-step `stop` (verification, debugging).
    pub fn generate_until(
        &self,
        cx: i32,
        cz: i32,
        cache: &BiomeCache,
        structures: Option<&dyn StructureDensity>,
        stop: Stage,
    ) -> ProtoTerrain {
        let mut chunk = ProtoTerrain::new(cx, cz, self.world_min_y, self.world_height);
        let Some(vol) = self.fill_volume(cx, cz) else { return chunk };
        let mut ctx = Ctx::cached();
        let mut aquifer = Aquifer::new(&self.g, &mut ctx, self.aquifer, self.picker, self.fluid_ids(), &vol);
        let mut density = self.g.volume(self.g.router.final_density, &mut ctx, &vol);
        if let Some(s) = structures {
            assert!(self.beard_ok, "final density without beardifier term at the top");
            s.add_to_volume(&mut density, &vol);
        }
        self.fill(&mut chunk, &mut aquifer, &mut ctx, &density, &vol);
        if stop == Stage::Fill {
            return chunk;
        }

        let owned: Vec<ChunkBiomes>;
        let mut refs: Vec<&ChunkBiomes> = Vec::with_capacity(9);
        let missing = (-1..=1).any(|dx| (-1..=1).any(|dz| !cache.chunks.contains_key(&(cx + dx, cz + dz))));
        if missing {
            owned = (-1..=1)
                .flat_map(|dx| (-1..=1).map(move |dz| (dx, dz)))
                .map(|(dx, dz)| self.chunk_biomes(cx + dx, cz + dz))
                .collect();
            refs.extend(owned.iter());
        } else {
            for dx in -1..=1 {
                for dz in -1..=1 {
                    refs.push(&cache.chunks[&(cx + dx, cz + dz)]);
                }
            }
        }
        let possible = possible_biomes(self.biome_names.len(), &refs);
        let chunks: [&ChunkBiomes; 9] = std::array::from_fn(|i| refs[i]);
        let mut view = BiomeView::Region {
            chunks,
            cx,
            cz,
            zoom_seed: self.zoom_seed,
            min_qy: self.world_min_y >> 2,
            max_qy: (self.world_min_y >> 2) + (self.world_height >> 2) - 1,
        };
        {
            let mut env = Env { g: &self.g, ctx: &mut ctx, biomes: &mut view };
            self.material.build_surface(&mut chunk, &self.blocks, &mut env, vol, Some(&possible));
        }
        if stop == Stage::Surface {
            return chunk;
        }
        self.carve(&mut chunk, &mut aquifer, &mut ctx, cache);
        chunk
    }

    /// "Base" column at (x, z): filling by the noise alone (density, aquifers), without
    /// surface or caves, from top to bottom; `stop` stops at the first block that satisfies it.
    /// Returns the blocks of the top of the generator up to the stop (index 0 = the highest) and the y of the
    /// highest block read.
    fn noise_column(&self, x: i32, z: i32, lazy: bool, stop: impl Fn(BlockId) -> bool) -> (Vec<BlockId>, i32) {
        let s = &self.g.settings;
        let min_y = s.min_y.max(self.world_min_y);
        let top = (s.min_y + s.height).min(self.world_min_y + self.world_height);
        let h = top - min_y;
        if h <= 0 {
            return (Vec::new(), top - 1);
        }
        let vol = Volume::new(1, h, 1, x, min_y, z);
        let mut ctx = Ctx::cached();
        let mut aquifer = Aquifer::new(&self.g, &mut ctx, self.aquifer, self.picker, self.fluid_ids(), &vol);
        // Adaptive probe (mode 3): attempted as long as it finds the block often enough.
        let adaptive = self.lazy_mode == 3;
        let lazy = lazy && (!adaptive || self.baseh.probe_worth());
        if lazy {
            // Slices aligned on the interpolation cells, from top to bottom: each block
            // gets the same density as in the full volume.
            // Splitting (`lazy_mode`): 0 = fixed slices of `lazy_cells` cells; 1 = slices
            // that double; 2 = a probe of `lazy_cells` cells then the rest at once; 3 = like 2,
            // probe abandoned when it succeeds less than one time in 4 (after 64 attempts).
            let mut seg = self.lazy_step.saturating_mul(self.lazy_cells.max(1));
            let mut out = Vec::new();
            let mut hi = top;
            let mut first = true;
            while hi > min_y {
                let lo = self.lazy_bottom(hi, min_y, &mut seg, first);
                let probing = first;
                first = false;
                let v = Volume::new(1, hi - lo, 1, x, lo, z);
                let density = self.g.volume(self.g.router.final_density, &mut ctx, &v);
                for y in (lo..hi).rev() {
                    let (b, _) = aquifer.compute(&self.g, &mut ctx, x, y, z, f64::from(density[(y - lo) as usize]));
                    let b = b.unwrap_or(self.default_block);
                    out.push(b);
                    if stop(b) {
                        if adaptive && probing {
                            self.baseh.probe(true);
                        }
                        return (out, top - 1);
                    }
                }
                if adaptive && probing {
                    self.baseh.probe(false);
                }
                hi = lo;
            }
            return (out, top - 1);
        }
        let density = self.g.volume(self.g.router.final_density, &mut ctx, &vol);
        let mut out = Vec::with_capacity(h as usize);
        for iy in (0..h).rev() {
            let y = min_y + iy;
            let (b, _) = aquifer.compute(&self.g, &mut ctx, x, y, z, f64::from(density[iy as usize]));
            let b = b.unwrap_or(self.default_block);
            out.push(b);
            if stop(b) {
                break;
            }
        }
        (out, top - 1)
    }

    /// Bottom of the next slice (`hi` excluded) of the lazy base height: aligned on the
    /// interpolation cells, bounded to the generator bottom.
    fn lazy_bottom(&self, hi: i32, min_y: i32, seg: &mut i32, first: bool) -> i32 {
        if !first && self.lazy_mode == 1 {
            *seg = seg.saturating_mul(2);
        }
        if !first && self.lazy_mode >= 2 {
            return min_y;
        }
        let step = self.lazy_step;
        hi.saturating_sub(*seg).div_euclid(step).saturating_mul(step).max(min_y)
    }

    /// Slice check: final density of the column (x, z) assembled slice by
    /// slice like the lazy base height, and the same through the full volume.
    #[doc(hidden)]
    pub fn lazy_density_check(&self, x: i32, z: i32) -> (Vec<f32>, Vec<f32>) {
        let s = &self.g.settings;
        let min_y = s.min_y.max(self.world_min_y);
        let top = (s.min_y + s.height).min(self.world_min_y + self.world_height);
        if top <= min_y {
            return (Vec::new(), Vec::new());
        }
        let mut ctx = Ctx::cached();
        let full = self.g.volume(self.g.router.final_density, &mut ctx, &Volume::new(1, top - min_y, 1, x, min_y, z));
        let mut ctx = Ctx::cached();
        let mut parts = Vec::new();
        let mut seg = self.lazy_step.saturating_mul(self.lazy_cells.max(1));
        let (mut hi, mut first) = (top, true);
        while hi > min_y {
            let lo = self.lazy_bottom(hi, min_y, &mut seg, first);
            first = false;
            parts.push(self.g.volume(self.g.router.final_density, &mut ctx, &Volume::new(1, hi - lo, 1, x, lo, z)));
            hi = lo;
        }
        (parts.into_iter().rev().flatten().collect(), full)
    }

    /// "Base" height (first free y) of a column without decoration or surface: non-air
    /// (`WORLD_SURFACE_WG`) or blocking movement (`OCEAN_FLOOR_WG`); world bottom if nothing.
    pub fn base_height(&self, x: i32, z: i32, ocean_floor: bool) -> i32 {
        let b = &self.blocks;
        let pred = |id: BlockId| if ocean_floor { b.blocks_motion(id) } else { !b.is_air(id) };
        if let Some(layers) = &self.flat {
            return match layers.iter().rposition(|&id| pred(id)) {
                Some(i) => self.world_min_y + i as i32 + 1,
                None => self.world_min_y,
            };
        }
        use std::sync::atomic::Ordering::Relaxed;
        self.baseh.calls.fetch_add(1, Relaxed);
        let key = (x, z, ocean_floor);
        let shard = &self.baseh.shards[(x.wrapping_mul(31).wrapping_add(z) & 15) as usize];
        if self.opts.baseh_memo
            && let Some(&h) = shard.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get(&key)
        {
            self.baseh.hits.fetch_add(1, Relaxed);
            return h;
        }
        let (col, top) = self.noise_column(x, z, self.opts.baseh_lazy, pred);
        let h = match col.last() {
            Some(&id) if pred(id) => top - (col.len() as i32 - 1) + 1,
            _ => self.world_min_y,
        };
        if self.opts.baseh_memo {
            let mut m = shard.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            if m.len() >= BASEH_MEMO_CAP {
                m.clear();
            }
            m.insert(key, h);
        }
        h
    }

    /// Bytes and number of cave carvings kept for the replay.
    pub fn carver_replay_usage(&self) -> (usize, usize) {
        self.replay.usage()
    }

    /// Calls to [`Self::base_height`] (noise world) and answers served by the memo.
    pub fn base_height_stats(&self) -> (u64, u64) {
        use std::sync::atomic::Ordering::Relaxed;
        (self.baseh.calls.load(Relaxed), self.baseh.hits.load(Relaxed))
    }

    /// Identifiers (lava, water) given to the aquifers.
    fn fluid_ids(&self) -> (BlockId, BlockId) {
        if self.opts.ids {
            (self.ids.lava, self.ids.water)
        } else {
            (self.blocks.get("minecraft:lava").unwrap_or(AIR), self.blocks.get("minecraft:water").unwrap_or(AIR))
        }
    }

    /// Aquifer of the volume `v` of this generator (equivalence tests of the aquifer paths).
    pub fn new_aquifer(&self, ctx: &mut Ctx, v: &Volume) -> Aquifer {
        Aquifer::new(&self.g, ctx, self.aquifer, self.picker, self.fluid_ids(), v)
    }

    /// Whole base column (from world bottom to top), air outside the generator.
    pub fn base_column(&self, x: i32, z: i32) -> Vec<BlockId> {
        let mut out = vec![AIR; self.world_height as usize];
        if let Some(layers) = &self.flat {
            out[..layers.len()].copy_from_slice(layers);
            return out;
        }
        let (col, top) = self.noise_column(x, z, false, |_| false);
        for (i, &id) in col.iter().enumerate() {
            let y = top - i as i32;
            if let Some(o) = out.get_mut((y - self.world_min_y) as usize) {
                *o = id;
            }
        }
        out
    }

    /// Terrain of several chunks in parallel (biomes prepared first).
    pub fn generate_many(&self, targets: &[(i32, i32)]) -> Vec<ProtoTerrain> {
        let mut cache = BiomeCache::default();
        self.prepare_biomes(&mut cache, targets);
        targets.par_iter().map(|&(cx, cz)| self.generate(cx, cz, &cache, None)).collect()
    }

    fn fill(&self, chunk: &mut ProtoTerrain, aquifer: &mut Aquifer, ctx: &mut Ctx, density: &[f32], vol: &Volume) {
        let b = &self.blocks;
        let o = self.opts;
        for z in 0..vol.sz {
            let bz = vol.bz(z);
            for x in 0..vol.sx {
                let bx = vol.bx(x);
                let base = vol.index(x, 0, z);
                for y in (0..vol.sy).rev() {
                    let by = vol.by(y);
                    let d = density[base + y as usize];
                    let (s, update) = if o.aq_pretest && d > 0.0 {
                        (None, false)
                    } else if o.aq_colwin {
                        aquifer.compute_column(&self.g, ctx, bx, by, bz, f64::from(d), o.aq_net)
                    } else {
                        aquifer.compute(&self.g, ctx, bx, by, bz, f64::from(d))
                    };
                    let s = s.unwrap_or(self.default_block);
                    if s != AIR {
                        chunk.set_raw(b, x, by, z, s);
                        chunk.update_heightmaps(b, x, by, z, s);
                        if update && b.has_fluid(s) {
                            chunk.mark_post_processing(x, by, z);
                        }
                    }
                }
            }
        }
    }

    /// Carved-cave mask of the chunk (cx, cz): carvings of the 17 × 17 neighbouring sources.
    pub fn carving_mask(&self, cx: i32, cz: i32, cache: &BiomeCache) -> CarvingMask {
        let gc = self.gc;
        let max_y = gc.min_y + gc.depth - 1 - self.protect_top;
        let mut mask = CarvingMask::new(gc.min_y + 1, max_y);
        {
            let o = self.opts;
            let mut t = Target::new(cx * 16, cz * 16, &mut mask, o.carver_yd, o.carver_widths);
            for dx in -8..=8 {
                for dz in -8..=8 {
                    let (sx, sz) = (cx + dx, cz + dz);
                    let biome = cache.carver.get(&(sx, sz)).copied().unwrap_or_else(|| self.carver_biome(sx, sz));
                    let Some(list) = self.biome_carvers.get(biome as usize) else { continue };
                    for (k, &ci) in list.iter().enumerate() {
                        let carver = &self.carvers[ci as usize];
                        if o.carver_replay {
                            let rec = self.replay.get((ci, k as u16, sx, sz), || {
                                carver.record(&gc, &mut large_feature_random(self.g.seed, k as i64, sx, sz), sx, sz)
                            });
                            rec.replay(&mut t);
                        } else {
                            let mut r = large_feature_random(self.g.seed, k as i64, sx, sz);
                            carver.run(&gc, &mut r, sx, sz, &mut t);
                        }
                    }
                }
            }
        }
        mask
    }

    fn carve(&self, chunk: &mut ProtoTerrain, aquifer: &mut Aquifer, ctx: &mut Ctx, cache: &BiomeCache) {
        let mask = self.carving_mask(chunk.cx, chunk.cz, cache);
        if mask.is_empty() {
            return;
        }
        let b = &self.blocks;
        let (dirt, grass, mycelium) = if self.opts.ids {
            (self.ids.dirt, self.ids.grass, self.ids.mycelium)
        } else {
            (b.get("minecraft:dirt"), b.get("minecraft:grass_block"), b.get("minecraft:mycelium"))
        };
        let mut view =
            BiomeView::Source { src: &self.source, zoom_seed: self.zoom_seed, searcher: Searcher::default() };
        let (bx0, bz0) = (chunk.min_x(), chunk.min_z());
        mask.visit(|x, z, bottom, top| {
            let mut has_grass = false;
            let (wx, wz) = (bx0 + x, bz0 + z);
            let mut y = top;
            while y >= bottom {
                let old = chunk.get(x, y, z);
                if !b.uncarvable(old) {
                    if Some(old) == grass || Some(old) == mycelium {
                        has_grass = true;
                    }
                    let (s, update) = aquifer.compute(&self.g, ctx, wx, y, wz, 0.0);
                    if let Some(s) = s {
                        chunk.set(b, x, y, z, s);
                        if update && b.has_fluid(s) {
                            chunk.mark_post_processing(x, y, z);
                        }
                        if has_grass && dirt.is_some() && Some(chunk.get(x, y - 1, z)) == dirt {
                            let mut env = Env { g: &self.g, ctx, biomes: &mut view };
                            if let Some(top) = self.material.top_material(chunk, &mut env, x, y - 1, z, b.has_fluid(s))
                            {
                                chunk.set(b, x, y - 1, z, top);
                                if b.has_fluid(top) {
                                    chunk.mark_post_processing(x, y - 1, z);
                                }
                            }
                        }
                    }
                }
                y -= 1;
            }
        });
    }
}
