//! Noise settings, router and "random state" of a world (spec § 1.5, § 3.2, § 4):
//! preparation of the functions (references expanded, caches deduplicated, axis slices),
//! compilation into samplers, final density of a chunk.

use hashbrown::HashMap;
use std::sync::Arc;

use rayon::prelude::*;

use crate::Error;
use crate::data::DataPack;
use crate::density::{
    self, AX_ALL, Axis, Binary, Df, DfRef, LIMIT_FACTOR, Registry, Resolver, Rewriter, ShiftKind, SliceAxes, Spline,
    Tiling, axes, range, rewrite_children,
};
use crate::json::Json;
use crate::math::Interval;
use crate::noise::{NormalNoise, Simplex, blended_fbm};
use crate::rng::{Positional, Rng};
use crate::sampler::{Ctx, Program, S, SSpline, Sid};
use crate::volume::Volume;

/// Named functions and noise parameters, read once.
#[derive(Debug, Default)]
pub struct Defs {
    pub functions: Registry,
    pub noises: HashMap<String, NormalNoise>,
}

impl Defs {
    pub fn load(pack: &DataPack) -> Result<Self, Error> {
        let mut d = Defs::default();
        for id in pack.ids("density_function") {
            let j = pack.json("density_function", &id)?;
            d.functions.insert(id.clone(), density::parse(&j).map_err(|e| Error::Format(format!("{id} : {e}")))?);
        }
        for id in pack.ids("noise") {
            let j = pack.json("noise", &id)?;
            d.noises.insert(id.clone(), NormalNoise::from_json(&j).map_err(|e| Error::Format(format!("{id} : {e}")))?);
        }
        Ok(d)
    }
}

impl Resolver for Defs {
    fn function(&self, id: &str) -> Option<DfRef> {
        self.functions.get(id).cloned()
    }
    fn noise_range(&self, id: &str) -> Interval {
        self.noises.get(id).map_or(Interval::NAI, |n| n.range)
    }
    fn prepared(&self, _: u32) -> (Interval, u8) {
        unreachable!("prepared cache outside compilation")
    }
}

#[derive(Debug, Clone, Copy)]
struct PreparedInfo {
    range: Interval,
    axes: u8,
    sampler: Sid,
}

struct View<'a> {
    defs: &'a Defs,
    prepared: &'a [PreparedInfo],
}

impl Resolver for View<'_> {
    fn function(&self, id: &str) -> Option<DfRef> {
        self.defs.function(id)
    }
    fn noise_range(&self, id: &str) -> Interval {
        self.defs.noise_range(id)
    }
    fn prepared(&self, id: u32) -> (Interval, u8) {
        let p = self.prepared[id as usize];
        (p.range, p.axes)
    }
}

/// Noise settings (`worldgen/noise_settings/*.json`).
#[derive(Debug, Clone)]
pub struct NoiseSettings {
    pub min_y: i32,
    pub height: i32,
    pub sea_level: i32,
    pub legacy_random: bool,
    pub default_block: String,
    pub default_fluid: String,
    /// Router in order: temperature, vegetation, continents, erosion, depth, ridges,
    /// chunk_surface_level, final_density.
    pub router: [DfRef; 8],
    /// Fields for part 2 (aquifers, material rules), raw.
    pub aquifers: Option<Json>,
    pub material_rule: Option<Json>,
}

pub const ROUTER_FIELDS: [&str; 8] =
    ["temperature", "vegetation", "continents", "erosion", "depth", "ridges", "chunk_surface_level", "final_density"];

impl NoiseSettings {
    pub fn load(pack: &DataPack, id: &str) -> Result<Self, Error> {
        let j = pack.json("noise_settings", id)?;
        let bad = |w: &str| Error::Format(format!("noise_settings {id} : {w}"));
        let noise = j.get("noise").ok_or_else(|| bad("noise"))?;
        let r = j.get("noise_router").ok_or_else(|| bad("noise_router"))?;
        let field = |k: &str| -> Result<DfRef, Error> { density::parse(r.get(k).ok_or_else(|| bad(k))?) };
        // Block state: string (26.3) or object `{Name}` (older formats).
        let block = |k: &str| {
            let b = j.get(k);
            b.and_then(Json::as_str)
                .or_else(|| b.and_then(|b| b.get("Name")).and_then(Json::as_str))
                .map_or_else(|| "minecraft:air".to_owned(), crate::data::full_id)
        };
        Ok(NoiseSettings {
            min_y: noise.get("min_y").and_then(Json::as_i32).ok_or_else(|| bad("min_y"))?,
            height: noise.get("height").and_then(Json::as_i32).ok_or_else(|| bad("height"))?,
            sea_level: j.get("sea_level").and_then(Json::as_i32).ok_or_else(|| bad("sea_level"))?,
            legacy_random: j.get("legacy_random_source").and_then(Json::as_bool).unwrap_or(false),
            default_block: block("default_block"),
            default_fluid: block("default_fluid"),
            router: [
                field(ROUTER_FIELDS[0])?,
                field(ROUTER_FIELDS[1])?,
                field(ROUTER_FIELDS[2])?,
                field(ROUTER_FIELDS[3])?,
                field(ROUTER_FIELDS[4])?,
                field(ROUTER_FIELDS[5])?,
                field(ROUTER_FIELDS[6])?,
                field(ROUTER_FIELDS[7])?,
            ],
            aquifers: j.get("aquifers").cloned(),
            material_rule: j.get("material_rule").cloned(),
        })
    }
}

/// Samplers of the router.
#[derive(Debug, Clone, Copy)]
pub struct Router {
    pub temperature: Sid,
    pub vegetation: Sid,
    pub continents: Sid,
    pub erosion: Sid,
    pub depth: Sid,
    pub ridges: Sid,
    pub chunk_surface_level: Sid,
    pub final_density: Sid,
}

/// Generation state of a world for a noise setting and a seed.
#[derive(Debug)]
pub struct Generator {
    pub seed: i64,
    pub settings: NoiseSettings,
    pub program: Program,
    pub router: Router,
    defs: Arc<Defs>,
    positional: Positional,
    noise_index: HashMap<String, u32>,
    prepared: Vec<PreparedInfo>,
    prepared_map: HashMap<DfRef, u32>,
    compiled: HashMap<DfRef, Sid>,
}

impl Generator {
    pub fn new(pack: &DataPack, settings_id: &str, seed: i64) -> Result<Self, Error> {
        Self::with_defs(Arc::new(Defs::load(pack)?), NoiseSettings::load(pack, settings_id)?, seed)
    }

    pub fn with_defs(defs: Arc<Defs>, settings: NoiseSettings, seed: i64) -> Result<Self, Error> {
        let positional = Rng::for_world(seed, settings.legacy_random).fork_positional();
        let mut g = Generator {
            seed,
            settings,
            program: Program::default(),
            router: Router {
                temperature: 0,
                vegetation: 0,
                continents: 0,
                erosion: 0,
                depth: 0,
                ridges: 0,
                chunk_surface_level: 0,
                final_density: 0,
            },
            defs,
            positional,
            noise_index: HashMap::new(),
            prepared: Vec::new(),
            prepared_map: HashMap::new(),
            compiled: HashMap::new(),
        };
        let r = g.settings.router.clone();
        let s: Vec<Sid> = r.iter().map(|f| g.compile_function(f)).collect::<Result<_, _>>()?;
        g.router = Router {
            temperature: s[0],
            vegetation: s[1],
            continents: s[2],
            erosion: s[3],
            depth: s[4],
            ridges: s[5],
            chunk_surface_level: s[6],
            final_density: s[7],
        };
        Ok(g)
    }

    pub fn defs(&self) -> &Arc<Defs> {
        &self.defs
    }

    /// Root positional factory of the world.
    pub fn positional(&self) -> Positional {
        self.positional
    }

    /// Prepares and compiles a function (memoised by structural equality, like the game).
    pub fn compile_function(&mut self, f: &DfRef) -> Result<Sid, Error> {
        if let Some(s) = self.compiled.get(f) {
            return Ok(*s);
        }
        let opt = self.optimize(f);
        let s = self.compile(&opt)?;
        self.compiled.insert(f.clone(), s);
        Ok(s)
    }

    /// Compiles the named function `id` (`worldgen/density_function/<id>`).
    pub fn compile_named(&mut self, id: &str) -> Result<Sid, Error> {
        let f =
            self.defs.function(&crate::data::full_id(id)).ok_or_else(|| Error::Missing(format!("function {id}")))?;
        self.compile_function(&f)
    }

    /// Declared range and axes of a named function.
    pub fn declared(&self, id: &str) -> Option<(Interval, u8)> {
        let f = self.defs.function(&crate::data::full_id(id))?;
        Some(self.declared_of(&f))
    }

    /// Declared range and axes of any function.
    pub fn declared_of(&self, f: &DfRef) -> (Interval, u8) {
        let v = View { defs: &self.defs, prepared: &self.prepared };
        (range(f, &v), axes(f, &v))
    }

    fn optimize(&mut self, f: &DfRef) -> DfRef {
        let inlined = Inline { g: self }.rewrite(f);
        let defs = self.defs.clone();
        let v = View { defs: &defs, prepared: &self.prepared };
        SliceAxes { parent: AX_ALL, resolver: &v }.rewrite(&inlined)
    }

    fn prepare_cache(&mut self, input: &DfRef) -> DfRef {
        if let Some(id) = self.prepared_map.get(input) {
            return Arc::new(Df::Prepared(*id));
        }
        let id = self.prepared.len() as u32;
        self.prepared.push(PreparedInfo { range: Interval::NAI, axes: 0, sampler: 0 });
        let opt = self.optimize(input);
        let s = self.compile(&opt).unwrap_or_else(|e| panic!("cache : {e}"));
        let defs = self.defs.clone();
        let v = View { defs: &defs, prepared: &self.prepared };
        let (r, a) = (range(&opt, &v), axes(&opt, &v));
        let cs = self.push(S::Cache { id, input: s });
        self.prepared[id as usize] = PreparedInfo { range: r, axes: a, sampler: cs };
        self.prepared_map.insert(input.clone(), id);
        Arc::new(Df::Prepared(id))
    }

    fn push(&mut self, s: S) -> Sid {
        self.program.push(s)
    }

    /// Instance of the noise `id` (shared with the density functions), created as needed.
    pub fn noise(&mut self, id: &str) -> Result<u32, Error> {
        let params = self.defs.noises.get(id).ok_or_else(|| Error::Missing(format!("noise {id}")))?.clone();
        let legacy_offset = match id {
            "minecraft:nether/temperature" => Some(0),
            "minecraft:nether/vegetation" => Some(1),
            _ => None,
        };
        if let Some(o) = legacy_offset {
            let stack = params.create_legacy_nether(&mut Rng::legacy(self.seed.wrapping_add(o)));
            self.program.noises.push(stack);
            return Ok((self.program.noises.len() - 1) as u32);
        }
        if let Some(i) = self.noise_index.get(id) {
            return Ok(*i);
        }
        let stack = params.create(&mut self.positional.from_hash_of(id));
        self.program.noises.push(stack);
        let i = (self.program.noises.len() - 1) as u32;
        self.noise_index.insert(id.to_owned(), i);
        Ok(i)
    }

    fn compile(&mut self, f: &DfRef) -> Result<Sid, Error> {
        let defs = self.defs.clone();
        let s = match &**f {
            Df::Const(v) => S::Const(v.0),
            Df::Simple(density::Simple::BlendAlpha) => S::Const(1.0),
            Df::Simple(_) => S::Const(0.0),
            Df::Noise { noise, xz, y, sx, sy, sz } => {
                let n = self.noise(noise)?;
                let is_zero = |d: &DfRef| d.is_const().is_some_and(|v| v.to_bits() == 0);
                if is_zero(sx) && is_zero(sy) && is_zero(sz) {
                    S::Noise { noise: n, xz: xz.0, y: y.0 }
                } else {
                    let a = self.compile(sx)?;
                    let c = self.compile(sz)?;
                    if is_zero(sy) {
                        S::ShiftedXz { sx: a, sz: c, noise: n, xz: xz.0, y: y.0 }
                    } else {
                        let b = self.compile(sy)?;
                        S::ShiftedXyz { sx: a, sy: b, sz: c, noise: n, xz: xz.0, y: y.0 }
                    }
                }
            }
            Df::EndIslands => {
                let mut r = Rng::legacy(self.seed);
                r.consume(17_292);
                self.program.simplex.push(Simplex::new_without_offset(&mut r));
                S::EndIslands { simplex: (self.program.simplex.len() - 1) as u32 }
            }
            Df::Distance { p, metric } => S::Distance { p: *p, metric: *metric },
            Df::Gradient { axis, tiling, from_c, to_c, from_v, to_v } => {
                let range = to_c.wrapping_sub(*from_c);
                let factor = (to_v.0 - from_v.0) / range as f32;
                match tiling {
                    Tiling::Clamp => S::GradClamp {
                        axis: *axis,
                        from: *from_c,
                        min: (*from_c).min(*to_c),
                        max: (*from_c).max(*to_c),
                        from_v: from_v.0,
                        factor,
                    },
                    Tiling::Repeat => S::GradRepeat { axis: *axis, from: *from_c, range, from_v: from_v.0, factor },
                    Tiling::Mirrored => S::GradMirror { axis: *axis, from: *from_c, range, from_v: from_v.0, factor },
                }
            }
            Df::Shift(kind, noise) => {
                let n = self.noise(noise)?;
                match kind {
                    ShiftKind::B => S::ShiftB { noise: n },
                    ShiftKind::A => {
                        let inner = self.push(S::Noise { noise: n, xz: 0.25, y: 0.0 });
                        S::ConstMul(inner, 4.0)
                    }
                    ShiftKind::Shift => {
                        let inner = self.push(S::Noise { noise: n, xz: 0.25, y: 0.25 });
                        S::ConstMul(inner, 4.0)
                    }
                }
            }
            Df::Unary(u, i) => S::Unary(*u, self.compile(i)?),
            Df::Round(k, i, m) => {
                let a = self.compile(i)?;
                if m.is_const() == Some(1.0) { S::RoundInt(*k, a) } else { S::Round(*k, a, self.compile(m)?) }
            }
            Df::Binary(b, l, r) => {
                let ls = self.compile(l)?;
                let rs = self.compile(r)?;
                let (lc, rc) = (l.is_const(), r.is_const());
                match b {
                    Binary::Add => match (lc, rc) {
                        (Some(c), _) => S::ConstAdd(rs, c),
                        (None, Some(c)) => S::ConstAdd(ls, c),
                        _ => S::Add(ls, rs),
                    },
                    Binary::Sub => match (lc, rc) {
                        (Some(c), _) => S::ConstSub(c, rs),
                        (None, Some(c)) => S::ConstAdd(ls, -c),
                        _ => S::Sub(ls, rs),
                    },
                    Binary::Mul => match (lc, rc) {
                        (Some(c), _) => S::ConstMul(rs, c),
                        (None, Some(c)) => S::ConstMul(ls, c),
                        _ => S::Mul(ls, rs),
                    },
                    Binary::Div => match (lc, rc) {
                        (Some(c), _) => S::ConstDiv(c, rs),
                        (None, Some(c)) => S::ConstMul(ls, 1.0 / c),
                        _ => S::Div(ls, rs),
                    },
                    Binary::Min | Binary::Max => {
                        let v = View { defs: &defs, prepared: &self.prepared };
                        let (lr, rr) = (range(l, &v), range(r, &v));
                        let is_min = *b == Binary::Min;
                        // Disjoint ranges: the winning side only (§ 3.2 ⚠).
                        if is_min && lr.max < rr.min || !is_min && lr.min > rr.max {
                            return Ok(ls);
                        }
                        if is_min && rr.max < lr.min || !is_min && rr.min > lr.max {
                            return Ok(rs);
                        }
                        match (lc, rc, is_min) {
                            (Some(c), _, true) => S::ConstMin(rs, c),
                            (None, Some(c), true) => S::ConstMin(ls, c),
                            (None, None, true) => S::Min(ls, rs, rr.min),
                            (Some(c), _, false) => S::ConstMax(rs, c),
                            (None, Some(c), false) => S::ConstMax(ls, c),
                            (None, None, false) => S::Max(ls, rs, rr.max),
                        }
                    }
                }
            }
            Df::Pow(b, e) => {
                let bs = self.compile(b)?;
                let es = self.compile(e)?;
                if let Some(c) = b.is_const() {
                    S::PowConstBase(f64::from(c), es)
                } else if let Some(c) = e.is_const() {
                    let special = match c.abs() {
                        0.5 => Some(S::Unary(density::Unary::Sqrt, bs)),
                        1.0 => None,
                        2.0 => Some(S::Unary(density::Unary::Square, bs)),
                        3.0 => Some(S::Unary(density::Unary::Cube, bs)),
                        _ => return Ok(self.push(S::PowConstExp(bs, f64::from(c)))),
                    };
                    let inner = match special {
                        Some(s) => self.push(s),
                        None => bs,
                    };
                    if c >= 0.0 {
                        return Ok(inner);
                    }
                    S::Unary(density::Unary::Reciprocal, inner)
                } else {
                    S::Pow(bs, es)
                }
            }
            Df::Spline(sp) => {
                let mut coords: Vec<Sid> = Vec::new();
                let mut index: HashMap<DfRef, usize> = HashMap::new();
                let compiled = self.compile_spline(sp, &mut coords, &mut index)?;
                S::Spline { spline: Box::new(compiled), coords }
            }
            Df::Lerp(a, f1, s2) => {
                let a_s = self.compile(a)?;
                let f_s = self.compile(f1)?;
                let s_s = self.compile(s2)?;
                if let Some(c) = f1.is_const() {
                    S::LerpConstFirst(a_s, c, s_s)
                } else if let Some(c) = s2.is_const() {
                    S::LerpConstSecond(a_s, f_s, c)
                } else {
                    S::Lerp(a_s, f_s, s_s)
                }
            }
            Df::Clamp(i, mn, mx) => S::Clamp(self.compile(i)?, mn.0, mx.0),
            Df::RangeChoice { input, min, max, inside, outside } => {
                let is = self.compile(input)?;
                match (inside.is_const(), outside.is_const()) {
                    (Some(a), Some(b)) => S::RangeConst { input: is, min: min.0, max: max.0, inside: a, outside: b },
                    _ => {
                        let a = self.compile(inside)?;
                        let b = self.compile(outside)?;
                        S::Range { input: is, min: min.0, max: max.0, inside: a, outside: b }
                    }
                }
            }
            Df::IntervalSelect { input, thresholds, functions } => {
                let is = self.compile(input)?;
                if thresholds.len() == 1 {
                    let below = self.compile(&functions[0])?;
                    let above = self.compile(&functions[functions.len() - 1])?;
                    S::IntervalSingle { input: is, threshold: thresholds[0].0, below, above }
                } else {
                    let fs: Vec<Sid> = functions.iter().map(|f| self.compile(f)).collect::<Result<_, _>>()?;
                    S::IntervalSelect { input: is, thresholds: thresholds.iter().map(|t| t.0).collect(), functions: fs }
                }
            }
            Df::Cache(_) => unreachable!("unprepared cache"),
            Df::Prepared(id) => return Ok(self.prepared[*id as usize].sampler),
            Df::BlendDensity(i) => return self.compile(i),
            Df::Interpolated { input, cxz, cy } => S::Interpolated {
                input: self.compile(input)?,
                cxz: *cxz,
                cy: *cy,
                ixz: 1.0 / *cxz as f32,
                iy: 1.0 / *cy as f32,
            },
            Df::Slice(axis, c, input) => {
                let pair = match (&**input, axis) {
                    (Df::Slice(Axis::Z, cz, inner), Axis::X) => Some((*c, *cz, inner)),
                    (Df::Slice(Axis::X, cx, inner), Axis::Z) => Some((*cx, *c, inner)),
                    _ => None,
                };
                match pair {
                    Some((x, z, inner)) => S::SliceXz(self.compile(inner)?, x, z),
                    None => {
                        let i = self.compile(input)?;
                        match axis {
                            Axis::X => S::SliceX(i, *c),
                            Axis::Y => S::SliceY(i, *c),
                            Axis::Z => S::SliceZ(i, *c),
                        }
                    }
                }
            }
            Df::FindTop { density, upper, lower, cell } => {
                let d = self.compile(density)?;
                let u = self.compile(upper)?;
                let ft = self.push(S::FindTop { density: d, upper: u, lower: *lower, cell: *cell });
                S::SliceY(ft, 0)
            }
            Df::OldBlended { xz, y, xz_factor, y_factor, smear } => {
                let mut r = if self.settings.legacy_random {
                    Rng::legacy(self.seed)
                } else {
                    self.positional.from_hash_of("minecraft:terrain")
                };
                let xz_mult = 684.412 * xz.0;
                let y_mult = 684.412 * y.0;
                let limit_smear = y_mult * smear.0;
                let main_smear = limit_smear / y_factor.0;
                let min = blended_fbm(&mut r, -15, limit_smear, f64::from(LIMIT_FACTOR));
                let max = blended_fbm(&mut r, -15, limit_smear, f64::from(LIMIT_FACTOR));
                let main = blended_fbm(&mut r, -7, main_smear, 12.75);
                let base = self.program.noises.len() as u32;
                self.program.noises.extend([min, max, main]);
                let min_s = self.push(S::Noise { noise: base, xz: xz_mult, y: y_mult });
                let max_s = self.push(S::Noise { noise: base + 1, xz: xz_mult, y: y_mult });
                let main_s = self.push(S::Noise { noise: base + 2, xz: xz_mult / xz_factor.0, y: y_mult / y_factor.0 });
                let add = self.push(S::ConstAdd(main_s, 0.5));
                let choice = self.push(S::Clamp(add, 0.0, 1.0));
                S::Lerp(choice, min_s, max_s)
            }
            Df::Ref(id) => {
                let v = self.defs.function(id).ok_or_else(|| Error::Missing(format!("function {id}")))?;
                return self.compile(&v);
            }
        };
        Ok(self.push(s))
    }

    fn compile_spline(
        &mut self,
        sp: &Spline<DfRef>,
        coords: &mut Vec<Sid>,
        index: &mut HashMap<DfRef, usize>,
    ) -> Result<SSpline, Error> {
        Ok(match sp {
            Spline::Const(v) => SSpline::Const(v.0),
            Spline::Multi { coord, locs, values, ders } => {
                let ci = match index.get(coord) {
                    Some(i) => *i,
                    None => {
                        let s = self.compile(coord)?;
                        coords.push(s);
                        index.insert(coord.clone(), coords.len() - 1);
                        coords.len() - 1
                    }
                };
                let mut vs = Vec::with_capacity(values.len());
                for v in values {
                    vs.push(self.compile_spline(v, coords, index)?);
                }
                SSpline::Multi {
                    coord: ci,
                    locs: locs.iter().map(|l| l.0).collect(),
                    values: vs,
                    ders: ders.iter().map(|d| d.0).collect(),
                }
            }
        })
    }

    // ---------------------------------------------------------------- evaluation

    pub fn value(&self, s: Sid, ctx: &mut Ctx, x: i32, y: i32, z: i32) -> f32 {
        self.program.value(s, ctx, x, y, z)
    }

    /// Values on the grid `v`; the buffer comes from the context when it has a free one
    /// (`RUSTCRAFT_OPT_VOL_POOL`), and can be handed back to it by [`Ctx::recycle`].
    pub fn volume(&self, s: Sid, ctx: &mut Ctx, v: &Volume) -> Vec<f32> {
        let mut out = ctx.output(v.size());
        self.program.volume(s, ctx, &mut out, v);
        out
    }

    /// Volume of a chunk: 16 × height from `(16·cx, min_y, 16·cz)`.
    pub fn chunk_volume(&self, cx: i32, cz: i32) -> Volume {
        Volume::new(16, self.settings.height, 16, cx * 16, self.settings.min_y, cz * 16)
    }

    /// Final density of a chunk (volume path, context with fresh caches).
    pub fn final_density(&self, cx: i32, cz: i32) -> Vec<f32> {
        self.volume(self.router.final_density, &mut Ctx::cached(), &self.chunk_volume(cx, cz))
    }

    /// Final density of several chunks in parallel (one chunk per task).
    pub fn final_density_many(&self, chunks: &[(i32, i32)]) -> Vec<Vec<f32>> {
        chunks.par_iter().map(|&(cx, cz)| self.final_density(cx, cz)).collect()
    }
}

/// Rule "expand the references, deduplicate and prepare the caches".
struct Inline<'g> {
    g: &'g mut Generator,
}

impl Rewriter for Inline<'_> {
    fn rewrite(&mut self, f: &DfRef) -> DfRef {
        let defs = self.g.defs.clone();
        let f = match &**f {
            Df::Ref(id) => defs.function(id).unwrap_or_else(|| panic!("unknown density function: {id}")),
            _ => f.clone(),
        };
        if let Df::Cache(input) = &*f {
            return self.g.prepare_cache(input);
        }
        rewrite_children(&f, self, &*defs)
    }
}
