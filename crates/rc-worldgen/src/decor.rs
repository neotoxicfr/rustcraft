//! Decoration of a chunk (`generation-decoration.md` § 2): order of the 11 steps, topological sort of the
//! placed features, decoration seeds, per-chunk loop.

use hashbrown::HashMap;
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use crate::Error;
use crate::decor_blocks::{Blocks, State};
use crate::decor_data::DecorData;
use crate::decor_pred::{ParseCtx, jclamp_f};
use crate::decor_region::Region;
use crate::decor_rng::{DecoRng, SinTable};
use crate::feature::{PlacedId, Registry};
use crate::json::Json;
use crate::noise::Simplex;
use crate::rng::Rng;

pub const STEPS: usize = 11;

/// Debug journal: draws (bits, value) and placements (position, state).
pub type RngTrace = (Vec<(u32, i32)>, Vec<([i32; 3], u32)>);

/// Climate of a biome (freezing and snow).
#[derive(Debug, Clone, Copy, Default)]
pub struct BiomeClimate {
    pub temperature: f32,
    pub has_precipitation: bool,
    pub frozen: bool,
}

/// Data prepared once per world (seed, dimension).
pub struct Decorator {
    pub reg: Registry,
    /// "Pale moss patch" feature resolved once (`RUSTCRAFT_OPT_DECOR_IDS`).
    pub pale_moss_patch: Option<crate::feature::FeatureId>,
    pub biome_names: Vec<String>,
    pub climate: Vec<BiomeClimate>,
    /// List L_s of each step (topological order).
    pub steps: Vec<Vec<PlacedId>>,
    /// Per biome and per step: indices (into L_s) of its features.
    biome_steps: Vec<Vec<Vec<u32>>>,
    /// Per biome: top-level features (bits per placed feature identifier).
    biome_has: Vec<Vec<u64>>,
    pub info_noise: Simplex,
    pub temperature_noise: Simplex,
    pub frozen_noise: [(Simplex, f64, f32); 3],
    pub sin: SinTable,
    pub seed: i64,
    geode: crate::noise::NoiseStack,
    /// Templates and processor lists ("template" features and fossils).
    pub templates: Option<Arc<crate::template::Templates>>,
    pub procs: Option<Arc<crate::structure_place::ProcLists>>,
}

impl std::fmt::Debug for Decorator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Decorator({} placed features, {} biomes)", self.reg.placed.len(), self.biome_names.len())
    }
}

/// Context of a decoration: region, data, reused buffers.
pub struct Ctx<'c, 'a> {
    pub r: &'c mut Region<'a>,
    pub d: &'c Decorator,
    stacks: Vec<Vec<([i32; 3], u16)>>,
    positions: Vec<Vec<[i32; 3]>>,
    pub missing: BTreeMap<String, u32>,
    pub tree: crate::tree::Scratch,
}

impl<'c, 'a> Ctx<'c, 'a> {
    pub fn new(r: &'c mut Region<'a>, d: &'c Decorator) -> Self {
        Ctx { r, d, stacks: Vec::new(), positions: Vec::new(), missing: BTreeMap::new(), tree: Default::default() }
    }

    pub fn take_stack(&mut self) -> Vec<([i32; 3], u16)> {
        self.stacks.pop().unwrap_or_default()
    }

    pub fn give_stack(&mut self, mut v: Vec<([i32; 3], u16)>) {
        v.clear();
        self.stacks.push(v);
    }

    pub fn take_positions(&mut self) -> Vec<[i32; 3]> {
        self.positions.pop().unwrap_or_default()
    }

    pub fn give_positions(&mut self, mut v: Vec<[i32; 3]>) {
        v.clear();
        self.positions.push(v);
    }

    pub fn note_missing(&mut self, name: &str) {
        *self.missing.entry(name.to_owned()).or_insert(0) += 1;
    }
}

/// Placement of the structures of a step: (step, context, decoration randomness, decoration seed).
pub type StructureHook<'h> = &'h mut dyn FnMut(usize, &mut Ctx, &mut DecoRng, i64);

/// Result of a decoration (statistics).
#[derive(Debug, Default, Clone)]
pub struct DecorStats {
    pub features_run: u32,
    /// Feature types met but not reproduced (name → number of attempted placements).
    pub missing: BTreeMap<String, u32>,
}

/// Settings of a flat world that recompose the features of its biome (`generation-decoration.md`
/// § 1, flat world): lava lakes, biome decoration without its structures, non-blocking
/// layers placed as the last step.
#[derive(Debug, Clone)]
pub struct FlatAdjust {
    pub biome: String,
    pub lakes: bool,
    pub decoration: bool,
    /// All the layers are air.
    pub void_gen: bool,
    /// Layers removed from the filling: (index from the bottom of the world, state).
    pub fill: Vec<(i32, State)>,
}

impl FlatAdjust {
    fn apply(
        &self,
        steps: Vec<Vec<PlacedId>>,
        reg: &mut Registry,
        c: &mut ParseCtx,
    ) -> Result<Vec<Vec<PlacedId>>, Error> {
        let mut out: Vec<Vec<PlacedId>> = Vec::new();
        fn add(out: &mut Vec<Vec<PlacedId>>, s: usize, f: PlacedId) {
            if out.len() <= s {
                out.resize(s + 1, Vec::new());
            }
            out[s].push(f);
        }
        const LAKES: usize = 1;
        const UNDERGROUND_STRUCTURES: usize = 3;
        const SURFACE_STRUCTURES: usize = 4;
        const TOP_LAYER: usize = 10;
        if self.lakes {
            for n in ["minecraft:lake_lava_underground", "minecraft:lake_lava_surface"] {
                let f = reg.placed_ref(&Json::Str(n.into()), c)?;
                add(&mut out, LAKES, f);
            }
        }
        if (!self.void_gen || crate::data::with_full_id(&self.biome, |b| b == "minecraft:the_void")) && self.decoration {
            for (s, list) in steps.iter().enumerate() {
                if s != UNDERGROUND_STRUCTURES && s != SURFACE_STRUCTURES && !(self.lakes && s == LAKES) {
                    for &f in list {
                        add(&mut out, s, f);
                    }
                }
            }
        }
        for &(height, state) in &self.fill {
            let f = reg.push_inline(crate::feature::Feature::FillLayer { height, state });
            add(&mut out, TOP_LAYER, f);
        }
        Ok(out)
    }
}

impl Decorator {
    /// `biome_names`: possible biomes of the source, in its order (that of the chunk indices).
    pub fn new(data: &DecorData, blocks: &Blocks, biome_names: &[String], seed: i64) -> Result<Decorator, Error> {
        Self::new_with(data, blocks, biome_names, seed, None)
    }

    /// Decorator of a flat world: features of the biome recomposed by `adj`.
    pub fn new_flat(data: &DecorData, blocks: &Blocks, adj: &FlatAdjust, seed: i64) -> Result<Decorator, Error> {
        Self::new_with(data, blocks, &[crate::data::full_id(&adj.biome)], seed, Some(adj))
    }

    fn new_with(
        data: &DecorData,
        blocks: &Blocks,
        biome_names: &[String],
        seed: i64,
        flat: Option<&FlatAdjust>,
    ) -> Result<Decorator, Error> {
        let biome_index: HashMap<String, u16> =
            biome_names.iter().enumerate().map(|(i, n)| (crate::data::full_id(n), i as u16)).collect();
        let mut c = ParseCtx { b: blocks, data, biome_index: &biome_index, providers: HashMap::new() };
        let mut reg = Registry::default();
        let mut per_biome: Vec<Vec<Vec<PlacedId>>> = Vec::new();
        let mut climate = Vec::new();
        for name in biome_names {
            let j = data.json("biome", name)?;
            let mut steps = Vec::new();
            for s in j.get("features").and_then(Json::as_array).unwrap_or(&[]) {
                let mut list = Vec::new();
                for f in s.as_array().ok_or_else(|| Error::Format(format!("{name} : features")))? {
                    list.push(reg.placed_ref(f, &mut c).map_err(|e| Error::Format(format!("{name} : {e}")))?);
                }
                steps.push(list);
            }
            if let Some(adj) = flat {
                steps = adj.apply(steps, &mut reg, &mut c)?;
            }
            per_biome.push(steps);
            climate.push(BiomeClimate {
                temperature: j.get("temperature").and_then(Json::as_f32).unwrap_or(0.5),
                has_precipitation: j.get("has_precipitation").and_then(Json::as_bool).unwrap_or(true),
                frozen: j.get("temperature_modifier").and_then(Json::as_str) == Some("frozen"),
            });
        }
        // Features called by the code (pale moss decorator).
        if data.raw("feature", "minecraft:pale_moss_patch").is_some() {
            reg.feature_ref(&Json::Str("minecraft:pale_moss_patch".into()), &mut c)?;
        }
        // Features placed by structure pieces (trees, village haystacks…).
        fn pool_features(j: &Json, out: &mut Vec<String>) {
            if j.get("element_type").and_then(Json::as_str) == Some("minecraft:feature_pool_element")
                && let Some(f) = j.get("feature").and_then(Json::as_str)
            {
                out.push(f.to_owned());
            }
            for k in ["elements", "element"] {
                match j.get(k) {
                    Some(Json::Arr(a)) => a.iter().for_each(|e| pool_features(e, out)),
                    Some(e @ Json::Obj(_)) => pool_features(e, out),
                    _ => {}
                }
            }
        }
        let mut named = Vec::new();
        for id in data.ids("template_pool") {
            if let Ok(j) = data.json("template_pool", &id) {
                pool_features(&j, &mut named);
            }
        }
        for f in named {
            reg.placed_ref(&Json::Str(f), &mut c)?;
        }
        // Topological sort (§ 2.2).
        let mut number: HashMap<PlacedId, u32> = HashMap::new();
        let mut edges: BTreeMap<(usize, u32), BTreeSet<(usize, u32)>> = BTreeMap::new();
        let mut node_feature: HashMap<(usize, u32), PlacedId> = HashMap::new();
        let mut max_step = 0;
        for steps in &per_biome {
            max_step = max_step.max(steps.len());
            let mut flat = Vec::new();
            for (s, list) in steps.iter().enumerate() {
                for &f in list {
                    let n = number.len() as u32;
                    let idx = *number.entry(f).or_insert(n);
                    flat.push((s, idx));
                    node_feature.insert((s, idx), f);
                }
            }
            for i in 0..flat.len() {
                let e = edges.entry(flat[i]).or_default();
                if i + 1 < flat.len() {
                    e.insert(flat[i + 1]);
                }
            }
        }
        let mut discovered = BTreeSet::new();
        let mut visiting = BTreeSet::new();
        let mut sorted = Vec::new();
        let keys: Vec<(usize, u32)> = edges.keys().copied().collect();
        for k in keys {
            if !discovered.contains(&k) && dfs(&edges, &mut discovered, &mut visiting, &mut sorted, k) {
                return Err(Error::Format("cycle in the order of the features".into()));
            }
        }
        sorted.reverse();
        let mut steps: Vec<Vec<PlacedId>> = vec![Vec::new(); max_step];
        let mut index: HashMap<(usize, PlacedId), u32> = HashMap::new();
        for node in &sorted {
            let f = node_feature[node];
            let list = &mut steps[node.0];
            index.entry((node.0, f)).or_insert(list.len() as u32);
            list.push(f);
        }
        let words = reg.placed.len().div_ceil(64);
        let mut biome_steps = Vec::new();
        let mut biome_has = Vec::new();
        for b_steps in &per_biome {
            let mut has = vec![0u64; words];
            let mut idx_steps = Vec::new();
            for (s, list) in b_steps.iter().enumerate() {
                let mut v: Vec<u32> = list.iter().map(|f| index[&(s, *f)]).collect();
                v.sort_unstable();
                v.dedup();
                idx_steps.push(v);
                for f in list {
                    has[f.0 as usize >> 6] |= 1 << (f.0 & 63);
                }
            }
            biome_steps.push(idx_steps);
            biome_has.push(has);
        }
        let info_noise = Simplex::new_without_offset(&mut Rng::legacy(2345));
        let temperature_noise = Simplex::new_without_offset(&mut Rng::legacy(1234));
        let mut fr = Rng::legacy(3456);
        let frozen_noise = [(1.0, 0.142_857_15f32), (0.5, 0.285_714_3), (0.25, 0.571_428_6)]
            .map(|(freq, amp)| (Simplex::new_without_offset(&mut fr), freq, amp));
        let pale_moss_patch = reg.feature_id("minecraft:pale_moss_patch");
        Ok(Decorator {
            pale_moss_patch,
            reg,
            biome_names: biome_names.to_vec(),
            climate,
            steps,
            biome_steps,
            biome_has,
            info_noise,
            temperature_noise,
            frozen_noise,
            sin: SinTable::new(),
            seed,
            geode: crate::feature_geode::geode_noise(seed),
            templates: None,
            procs: None,
        })
    }

    /// Templates and processors of the jar ("template" features and fossils).
    pub fn with_templates(mut self, d: &DecorData, b: &Blocks) -> Self {
        self.templates = Some(Arc::new(crate::template::Templates::new(d.files_of("template"), b)));
        self.procs = Some(Arc::new(crate::structure_place::ProcLists::new(d)));
        self
    }

    /// Without any biome feature (flat world without decoration: only the structures and the
    /// features they call remain).
    pub fn without_biome_features(mut self) -> Self {
        for steps in &mut self.biome_steps {
            steps.iter_mut().for_each(Vec::clear);
        }
        for has in &mut self.biome_has {
            has.fill(0);
        }
        self
    }

    pub fn geode_noise(&self) -> &crate::noise::NoiseStack {
        &self.geode
    }

    #[inline]
    pub fn biome_has_feature(&self, biome: u16, f: PlacedId) -> bool {
        self.biome_has.get(biome as usize).is_some_and(|w| w[f.0 as usize >> 6] >> (f.0 & 63) & 1 != 0)
    }

    /// Decorates the central chunk of the region (steps of the features; structures are elsewhere).
    pub fn decorate(&self, r: &mut Region) -> DecorStats {
        self.decorate_with(r, None)
    }

    /// Like [`Self::decorate`], with the placement of the structures at the start of each step:
    /// `structures(step, ctx, randomness, decoration seed)` (§ 2.3).
    pub fn decorate_with(&self, r: &mut Region, mut structures: Option<StructureHook>) -> DecorStats {
        let (bx, bz) = (r.cx * 16, r.cz * 16);
        let origin = [bx, r.min_y, bz];
        let mut rng = DecoRng::xoroshiro(0);
        let deco_seed = rng.set_decoration_seed(self.seed, bx, bz);
        // Possible biomes: present in the 3×3 chunks.
        let mut present = vec![false; self.biome_names.len()];
        for c in r.chunks.iter() {
            for &b in &c.biomes {
                if let Some(p) = present.get_mut(b as usize) {
                    *p = true;
                }
            }
        }
        let mut stats = DecorStats::default();
        let mut ctx = Ctx::new(r, self);
        let mut indices: Vec<u32> = Vec::new();
        for s in 0..STEPS.max(self.steps.len()) {
            if let Some(h) = structures.as_mut() {
                h(s, &mut ctx, &mut rng, deco_seed);
            }
            if s >= self.steps.len() {
                continue;
            }
            indices.clear();
            for (b, &p) in present.iter().enumerate() {
                if p && let Some(list) = self.biome_steps[b].get(s) {
                    indices.extend_from_slice(list);
                }
            }
            indices.sort_unstable();
            indices.dedup();
            for &i in &indices {
                let f = self.steps[s][i as usize];
                rng.set_feature_seed(deco_seed, i as i32, s as i32);
                crate::placement::place(&mut ctx, f, &mut rng, origin, Some(f));
                stats.features_run += 1;
            }
        }
        stats.missing = std::mem::take(&mut ctx.missing);
        stats
    }

    /// Forced placement (verification): `n` attempts of the named placed feature, feature seeds
    /// (decoration, 1000 + k, 99), biome check active.
    pub fn force_traced(
        &self,
        r: &mut Region,
        name: &str,
        n: u32,
        mut after: impl FnMut(&mut Region, PlacedId),
    ) -> DecorStats {
        let mut stats = DecorStats::default();
        let Some(f) = self.reg.placed_id(name) else { return stats };
        let (bx, bz) = (r.cx * 16, r.cz * 16);
        let origin = [bx, r.min_y, bz];
        let mut rng = DecoRng::xoroshiro(0);
        let deco_seed = rng.set_decoration_seed(self.seed, bx, bz);
        for k in 0..n {
            rng.set_feature_seed(deco_seed, 1000 + k as i32, 99);
            {
                let mut ctx = Ctx::new(r, self);
                crate::placement::place(&mut ctx, f, &mut rng, origin, Some(f));
                for (key, v) in std::mem::take(&mut ctx.missing) {
                    *stats.missing.entry(key).or_insert(0) += v;
                }
            }
            stats.features_run += 1;
            after(r, f);
        }
        stats
    }

    /// Complete decoration journaling the draws of the named feature (debug).
    pub fn decorate_rng_trace(&self, r: &mut Region, name: &str) -> RngTrace {
        let mut sets = Vec::new();
        let mut log = Vec::new();
        let target = self.reg.placed_id(name);
        let (bx, bz) = (r.cx * 16, r.cz * 16);
        let origin = [bx, r.min_y, bz];
        let mut rng = DecoRng::xoroshiro(0);
        let deco_seed = rng.set_decoration_seed(self.seed, bx, bz);
        let mut present = vec![false; self.biome_names.len()];
        for c in r.chunks.iter() {
            for &b in &c.biomes {
                if let Some(p) = present.get_mut(b as usize) {
                    *p = true;
                }
            }
        }
        let mut indices: Vec<u32> = Vec::new();
        for s in 0..self.steps.len() {
            indices.clear();
            for (b, &p) in present.iter().enumerate() {
                if p && let Some(list) = self.biome_steps[b].get(s) {
                    indices.extend_from_slice(list);
                }
            }
            indices.sort_unstable();
            indices.dedup();
            for &i in &indices {
                let f = self.steps[s][i as usize];
                rng.set_feature_seed(deco_seed, i as i32, s as i32);
                if Some(f) == target {
                    rng.trace = Some(Vec::new());
                    r.set_log = Some(Vec::new());
                }
                let mut ctx = Ctx::new(r, self);
                crate::placement::place(&mut ctx, f, &mut rng, origin, Some(f));
                if let Some(t) = rng.trace.take() {
                    log.extend(t);
                }
                if let Some(s) = r.set_log.take() {
                    sets.extend(s);
                }
            }
        }
        (log, sets)
    }

    /// Decoration with a callback after each top-level feature (verification traces).
    pub fn decorate_traced(&self, r: &mut Region, mut after: impl FnMut(&mut Region, usize, PlacedId)) -> DecorStats {
        let (bx, bz) = (r.cx * 16, r.cz * 16);
        let origin = [bx, r.min_y, bz];
        let mut rng = DecoRng::xoroshiro(0);
        let deco_seed = rng.set_decoration_seed(self.seed, bx, bz);
        let mut present = vec![false; self.biome_names.len()];
        for c in r.chunks.iter() {
            for &b in &c.biomes {
                if let Some(p) = present.get_mut(b as usize) {
                    *p = true;
                }
            }
        }
        let mut stats = DecorStats::default();
        let mut indices: Vec<u32> = Vec::new();
        for s in 0..self.steps.len() {
            indices.clear();
            for (b, &p) in present.iter().enumerate() {
                if p && let Some(list) = self.biome_steps[b].get(s) {
                    indices.extend_from_slice(list);
                }
            }
            indices.sort_unstable();
            indices.dedup();
            for &i in &indices {
                let f = self.steps[s][i as usize];
                rng.set_feature_seed(deco_seed, i as i32, s as i32);
                {
                    let mut ctx = Ctx::new(r, self);
                    crate::placement::place(&mut ctx, f, &mut rng, origin, Some(f));
                    for (k, v) in std::mem::take(&mut ctx.missing) {
                        *stats.missing.entry(k).or_insert(0) += v;
                    }
                }
                stats.features_run += 1;
                after(r, s, f);
            }
        }
        stats
    }

    // ---- Climate (freezing, snow) ----

    /// Temperature adjusted to the height.
    pub fn temperature(&self, biome: u16, p: [i32; 3], sea_level: i32) -> f32 {
        let c = self.climate[biome as usize];
        let mut t = c.temperature;
        if c.frozen {
            let x = f64::from(p[0]);
            let z = f64::from(p[2]);
            let mut large = 0.0f32;
            for (n, freq, amp) in &self.frozen_noise {
                large += amp * n.get2(x * 0.05 * freq, z * 0.05 * freq);
            }
            let g = f64::from(large * 7.0) + f64::from(self.info_noise.get2(x * 0.2, z * 0.2));
            if g < 0.3 && f64::from(self.info_noise.get2(x * 0.09, z * 0.09)) < 0.8 {
                t = 0.2;
            }
        }
        let snow = sea_level + 17;
        if p[1] > snow {
            let v = self.temperature_noise.get2(f64::from(p[0] as f32 / 8.0), f64::from(p[2] as f32 / 8.0)) * 8.0;
            t - (v + p[1] as f32 - snow as f32) * 0.05 / 40.0
        } else {
            t
        }
    }

    pub fn warm_enough_to_rain(&self, biome: u16, p: [i32; 3], sea_level: i32) -> bool {
        if crate::opt::freeze_threshold() && self.surely_warm(biome, p[1], sea_level) {
            return true;
        }
        self.temperature(biome, p, sea_level) >= 0.15
    }

    /// Warm enough to rain whatever the noise (`RUSTCRAFT_OPT_FREEZE_THRESHOLD`): biome not
    /// frozen and temperature still >= 0.15 with the altitude noise at its bound. Same sequence
    /// of f32 operations as the exact computation, increasing in noise: noise <= bound ⇒ result >=.
    #[doc(hidden)]
    pub fn surely_warm(&self, biome: u16, y: i32, sea_level: i32) -> bool {
        let c = self.climate[biome as usize];
        if c.frozen {
            return false;
        }
        let snow = sea_level + 17;
        if y <= snow {
            return c.temperature >= 0.15;
        }
        let bound = 8.0 * crate::material::TEMPERATURE_NOISE_BOUND;
        c.temperature - (bound + y as f32 - snow as f32) * 0.05 / 40.0 >= 0.15
    }

    /// [`Self::warm_enough_to_rain`] at the heights `ys` of a single column (`RUSTCRAFT_OPT_FREEZE_COL`):
    /// frozen part and altitude noise computed once, altitude term in the same f32 order.
    pub fn warm_column(&self, biome: u16, x: i32, z: i32, ys: [i32; 2], sea_level: i32) -> [bool; 2] {
        let threshold = crate::opt::freeze_threshold();
        let c = self.climate[biome as usize];
        let snow = sea_level + 17;
        let mut base: Option<f32> = None;
        let mut v: Option<f32> = None;
        let mut out = [false; 2];
        for (k, &y) in ys.iter().enumerate() {
            if threshold && self.surely_warm(biome, y, sea_level) {
                out[k] = true;
                continue;
            }
            let t = *base.get_or_insert_with(|| {
                let mut t = c.temperature;
                if c.frozen {
                    let (xf, zf) = (f64::from(x), f64::from(z));
                    let mut large = 0.0f32;
                    for (n, freq, amp) in &self.frozen_noise {
                        large += amp * n.get2(xf * 0.05 * freq, zf * 0.05 * freq);
                    }
                    let g = f64::from(large * 7.0) + f64::from(self.info_noise.get2(xf * 0.2, zf * 0.2));
                    if g < 0.3 && f64::from(self.info_noise.get2(xf * 0.09, zf * 0.09)) < 0.8 {
                        t = 0.2;
                    }
                }
                t
            });
            let t = if y > snow {
                let v = *v.get_or_insert_with(|| {
                    self.temperature_noise.get2(f64::from(x as f32 / 8.0), f64::from(z as f32 / 8.0)) * 8.0
                });
                t - (v + y as f32 - snow as f32) * 0.05 / 40.0
            } else {
                t
            };
            out[k] = t >= 0.15;
        }
        out
    }

    pub fn clamp(v: f32) -> f32 {
        jclamp_f(v, 0.0, 1.0)
    }
}

fn dfs(
    edges: &BTreeMap<(usize, u32), BTreeSet<(usize, u32)>>,
    discovered: &mut BTreeSet<(usize, u32)>,
    visiting: &mut BTreeSet<(usize, u32)>,
    out: &mut Vec<(usize, u32)>,
    node: (usize, u32),
) -> bool {
    if discovered.contains(&node) {
        return false;
    }
    if visiting.contains(&node) {
        return true;
    }
    visiting.insert(node);
    if let Some(next) = edges.get(&node) {
        for &n in next {
            if dfs(edges, discovered, visiting, out, n) {
                return true;
            }
        }
    }
    visiting.remove(&node);
    discovered.insert(node);
    out.push(node);
    false
}
