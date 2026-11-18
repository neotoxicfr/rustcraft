//! Value and height providers, block predicates, rule tests, state providers
//! (`generation-decoration.md` § 3.5, 3.6, 4).

use hashbrown::HashMap;

use crate::Error;
use crate::decor_blocks::{BlockId, BlockSet, Blocks, Dir, Prop, State};
use crate::decor_data::DecorData;
use crate::decor_region::Region;
use crate::decor_rng::DecoRng;
use crate::decor_rules;
use crate::json::Json;
use crate::noise::{NoiseStack, NormalNoise};
use crate::rng::Rng;

fn bad(w: impl Into<String>) -> Error {
    Error::Format(w.into())
}

fn ty(j: &Json) -> Option<&str> {
    j.get("type").and_then(Json::as_str).map(|t| t.strip_prefix("minecraft:").unwrap_or(t))
}

pub fn get_i32(j: &Json, k: &str) -> Result<i32, Error> {
    j.get(k).and_then(Json::as_i32).ok_or_else(|| bad(format!("integer {k}")))
}

pub fn get_f32(j: &Json, k: &str) -> Result<f32, Error> {
    j.get(k).and_then(Json::as_f32).ok_or_else(|| bad(format!("float {k}")))
}

pub fn opt_f32(j: &Json, k: &str, d: f32) -> f32 {
    j.get(k).and_then(Json::as_f32).unwrap_or(d)
}

pub fn opt_i32(j: &Json, k: &str, d: i32) -> i32 {
    j.get(k).and_then(Json::as_i32).unwrap_or(d)
}

pub fn opt_bool(j: &Json, k: &str, d: bool) -> bool {
    j.get(k).and_then(Json::as_bool).unwrap_or(d)
}

pub fn vec3(j: Option<&Json>) -> Result<[i32; 3], Error> {
    match j {
        None => Ok([0; 3]),
        Some(a) => {
            let a = a.as_array().ok_or_else(|| bad("vector"))?;
            if a.len() != 3 {
                return Err(bad("3-component vector"));
            }
            Ok([
                a[0].as_i32().ok_or_else(|| bad("vector"))?,
                a[1].as_i32().ok_or_else(|| bad("vector"))?,
                a[2].as_i32().ok_or_else(|| bad("vector"))?,
            ])
        }
    }
}

// ---------------------------------------------------------------- Weighted list

/// Weighted choice: `next_int(total)` then first cumulative sum that exceeds it.
#[derive(Debug, Clone)]
pub struct Weighted<T> {
    pub items: Vec<(T, i32)>,
    pub total: i32,
}

impl<T> Weighted<T> {
    pub fn new(items: Vec<(T, i32)>) -> Self {
        let total = items.iter().map(|i| i.1).sum();
        Weighted { items, total }
    }

    pub fn pick(&self, r: &mut DecoRng) -> Option<&T> {
        if self.total <= 0 {
            return None;
        }
        let mut t = r.next_int_bounded(self.total);
        for (v, w) in &self.items {
            t -= *w;
            if t < 0 {
                return Some(v);
            }
        }
        None
    }
}

// ---------------------------------------------------------------- Integers

#[derive(Debug, Clone)]
pub enum IntP {
    Const(i32),
    Uniform(i32, i32),
    Biased(i32, i32),
    VeryBiased(i32, i32),
    Clamped(Box<IntP>, i32, i32),
    ClampedNormal { mean: f32, dev: f32, min: i32, max: i32 },
    Weighted(Weighted<IntP>),
    Trapezoid { min: i32, max: i32, plateau: i32 },
}

impl IntP {
    pub fn parse(j: &Json) -> Result<IntP, Error> {
        if let Some(v) = j.as_i32() {
            return Ok(IntP::Const(v));
        }
        let t = ty(j).ok_or_else(|| bad("integer provider"))?;
        Ok(match t {
            "constant" => IntP::Const(get_i32(j, "value")?),
            "uniform" => IntP::Uniform(get_i32(j, "min_inclusive")?, get_i32(j, "max_inclusive")?),
            "biased_to_bottom" => IntP::Biased(get_i32(j, "min_inclusive")?, get_i32(j, "max_inclusive")?),
            "very_biased_to_bottom" => IntP::VeryBiased(get_i32(j, "min_inclusive")?, get_i32(j, "max_inclusive")?),
            "clamped" => IntP::Clamped(
                Box::new(IntP::parse(j.get("source").ok_or_else(|| bad("source"))?)?),
                get_i32(j, "min_inclusive")?,
                get_i32(j, "max_inclusive")?,
            ),
            "clamped_normal" => IntP::ClampedNormal {
                mean: get_f32(j, "mean")?,
                dev: get_f32(j, "deviation")?,
                min: get_i32(j, "min_inclusive")?,
                max: get_i32(j, "max_inclusive")?,
            },
            "weighted_list" => {
                let mut items = Vec::new();
                for e in j.get("distribution").and_then(Json::as_array).ok_or_else(|| bad("distribution"))? {
                    items.push((IntP::parse(e.get("data").ok_or_else(|| bad("data"))?)?, get_i32(e, "weight")?));
                }
                IntP::Weighted(Weighted::new(items))
            }
            "trapezoid" => {
                IntP::Trapezoid { min: get_i32(j, "min")?, max: get_i32(j, "max")?, plateau: opt_i32(j, "plateau", 0) }
            }
            t => return Err(bad(format!("integer provider {t}"))),
        })
    }

    pub fn sample(&self, r: &mut DecoRng) -> i32 {
        match self {
            IntP::Const(v) => *v,
            IntP::Uniform(a, b) => r.between(*a, *b),
            IntP::Biased(a, b) => {
                let k = r.next_int_bounded(b - a + 1) + 1;
                a + r.next_int_bounded(k)
            }
            IntP::VeryBiased(a, b) => {
                let k = r.next_int_bounded(b - a + 1) + 1;
                let k = r.next_int_bounded(k) + 1;
                a + r.next_int_bounded(k)
            }
            IntP::Clamped(s, a, b) => s.sample(r).clamp(*a, *b),
            IntP::ClampedNormal { mean, dev, min, max } => {
                let v = mean + r.next_gaussian() as f32 * dev;
                jclamp_f(v, *min as f32, *max as f32) as i32
            }
            IntP::Weighted(w) => match w.pick(r) {
                Some(p) => p.clone_sample(r),
                None => 0,
            },
            IntP::Trapezoid { min, max, plateau } => {
                if *plateau == 0 && *max == -*min {
                    let a = r.next_int_bounded(max + 1);
                    return a - r.next_int_bounded(max + 1);
                }
                let range = max - min;
                if *plateau == range {
                    return r.between(*min, *max);
                }
                let ps = (range - plateau) / 2;
                let pe = range - ps;
                let a = r.between(0, pe);
                min + a + r.between(0, ps)
            }
        }
    }

    fn clone_sample(&self, r: &mut DecoRng) -> i32 {
        self.sample(r)
    }

    pub fn min_value(&self) -> i32 {
        match self {
            IntP::Const(v) => *v,
            IntP::Uniform(a, _) | IntP::Biased(a, _) | IntP::VeryBiased(a, _) => *a,
            IntP::Clamped(s, a, _) => (*a).max(s.min_value()),
            IntP::ClampedNormal { min, .. } => *min,
            IntP::Weighted(w) => w.items.iter().map(|i| i.0.min_value()).min().unwrap_or(0),
            IntP::Trapezoid { min, .. } => *min,
        }
    }

    pub fn max_value(&self) -> i32 {
        match self {
            IntP::Const(v) => *v,
            IntP::Uniform(_, b) | IntP::Biased(_, b) | IntP::VeryBiased(_, b) => *b,
            IntP::Clamped(s, _, b) => (*b).min(s.max_value()),
            IntP::ClampedNormal { max, .. } => *max,
            IntP::Weighted(w) => w.items.iter().map(|i| i.0.max_value()).max().unwrap_or(0),
            IntP::Trapezoid { max, .. } => *max,
        }
    }
}

/// The bound of a float value (NaN kept as in Java: comparison false → value).
#[inline]
pub fn jclamp_f(v: f32, lo: f32, hi: f32) -> f32 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

#[inline]
pub fn jclamp_d(v: f64, lo: f64, hi: f64) -> f64 {
    if v < lo {
        lo
    } else if v > hi {
        hi
    } else {
        v
    }
}

// ---------------------------------------------------------------- Floats

#[derive(Debug, Clone)]
pub enum FloatP {
    Const(f32),
    Uniform(f32, f32),
    ClampedNormal { mean: f32, dev: f32, min: f32, max: f32 },
    Trapezoid { min: f32, max: f32, plateau: f32 },
}

impl FloatP {
    pub fn parse(j: &Json) -> Result<FloatP, Error> {
        if let Some(v) = j.as_f32() {
            return Ok(FloatP::Const(v));
        }
        let t = ty(j).ok_or_else(|| bad("float provider"))?;
        Ok(match t {
            "constant" => FloatP::Const(get_f32(j, "value")?),
            "uniform" => FloatP::Uniform(get_f32(j, "min_inclusive")?, get_f32(j, "max_exclusive")?),
            "clamped_normal" => FloatP::ClampedNormal {
                mean: get_f32(j, "mean")?,
                dev: get_f32(j, "deviation")?,
                min: get_f32(j, "min")?,
                max: get_f32(j, "max")?,
            },
            "trapezoid" => {
                FloatP::Trapezoid { min: get_f32(j, "min")?, max: get_f32(j, "max")?, plateau: get_f32(j, "plateau")? }
            }
            t => return Err(bad(format!("float provider {t}"))),
        })
    }

    pub fn sample(&self, r: &mut DecoRng) -> f32 {
        match self {
            FloatP::Const(v) => *v,
            FloatP::Uniform(a, b) => r.next_float() * (b - a) + a,
            FloatP::ClampedNormal { mean, dev, min, max } => {
                jclamp_f(mean + r.next_gaussian() as f32 * dev, *min, *max)
            }
            FloatP::Trapezoid { min, max, plateau } => {
                let range = max - min;
                let ps = (range - plateau) / 2.0;
                let pe = range - ps;
                let a = r.next_float() * pe;
                min + a + r.next_float() * ps
            }
        }
    }

    pub fn min_value(&self) -> f32 {
        match self {
            FloatP::Const(v) => *v,
            FloatP::Uniform(a, _) => *a,
            FloatP::ClampedNormal { min, .. } => *min,
            FloatP::Trapezoid { min, .. } => *min,
        }
    }
}

// ---------------------------------------------------------------- Heights

#[derive(Debug, Clone, Copy)]
pub enum Anchor {
    Absolute(i32),
    AboveBottom(i32),
    BelowTop(i32),
    SeaLevel(i32),
}

/// Generation context (min y and generation depth, sea level).
#[derive(Debug, Clone, Copy)]
pub struct GenCtx {
    pub min_y: i32,
    pub depth: i32,
    pub sea_level: i32,
}

impl Anchor {
    pub fn parse(j: &Json) -> Result<Anchor, Error> {
        if let Some(v) = j.get("absolute").and_then(Json::as_i32) {
            Ok(Anchor::Absolute(v))
        } else if let Some(v) = j.get("above_bottom").and_then(Json::as_i32) {
            Ok(Anchor::AboveBottom(v))
        } else if let Some(v) = j.get("below_top").and_then(Json::as_i32) {
            Ok(Anchor::BelowTop(v))
        } else if let Some(v) = j.get("relative_to_sea_level").and_then(Json::as_i32) {
            Ok(Anchor::SeaLevel(v))
        } else {
            Err(bad("vertical anchor"))
        }
    }

    pub fn resolve(self, c: GenCtx) -> i32 {
        match self {
            Anchor::Absolute(v) => v,
            Anchor::AboveBottom(o) => c.min_y + o,
            Anchor::BelowTop(o) => c.min_y + c.depth - 1 - o,
            Anchor::SeaLevel(o) => c.sea_level + o,
        }
    }
}

#[derive(Debug, Clone)]
pub enum HeightP {
    Const(Anchor),
    Uniform(Anchor, Anchor),
    Biased(Anchor, Anchor, i32),
    VeryBiased(Anchor, Anchor, i32),
    Trapezoid(Anchor, Anchor, i32),
    Weighted(Weighted<HeightP>),
}

impl HeightP {
    pub fn parse(j: &Json) -> Result<HeightP, Error> {
        let Some(t) = ty(j) else { return Ok(HeightP::Const(Anchor::parse(j)?)) };
        let a = |k: &str| Anchor::parse(j.get(k).ok_or_else(|| bad(k.to_owned()))?);
        Ok(match t {
            "constant" => HeightP::Const(a("value")?),
            "uniform" => HeightP::Uniform(a("min_inclusive")?, a("max_inclusive")?),
            "biased_to_bottom" => HeightP::Biased(a("min_inclusive")?, a("max_inclusive")?, opt_i32(j, "inner", 1)),
            "very_biased_to_bottom" => {
                HeightP::VeryBiased(a("min_inclusive")?, a("max_inclusive")?, opt_i32(j, "inner", 1))
            }
            "trapezoid" => HeightP::Trapezoid(a("min_inclusive")?, a("max_inclusive")?, opt_i32(j, "plateau", 0)),
            "weighted_list" => {
                let mut items = Vec::new();
                for e in j.get("distribution").and_then(Json::as_array).ok_or_else(|| bad("distribution"))? {
                    items.push((HeightP::parse(e.get("data").ok_or_else(|| bad("data"))?)?, get_i32(e, "weight")?));
                }
                HeightP::Weighted(Weighted::new(items))
            }
            t => return Err(bad(format!("height provider {t}"))),
        })
    }

    pub fn sample(&self, r: &mut DecoRng, c: GenCtx) -> i32 {
        match self {
            HeightP::Const(a) => a.resolve(c),
            HeightP::Uniform(a, b) => {
                let (lo, hi) = (a.resolve(c), b.resolve(c));
                if lo > hi { lo } else { r.between(lo, hi) }
            }
            HeightP::Biased(a, b, inner) => {
                let (lo, hi) = (a.resolve(c), b.resolve(c));
                if hi - lo - inner < 0 {
                    return lo;
                }
                let k = r.next_int_bounded(hi - lo - inner + 1);
                r.next_int_bounded(k + inner) + lo
            }
            HeightP::VeryBiased(a, b, inner) => {
                let (lo, hi) = (a.resolve(c), b.resolve(c));
                if hi - lo - inner < 0 {
                    return lo;
                }
                let between =
                    |r: &mut DecoRng, u: i32, v: i32| if u >= v { u } else { u + r.next_int_bounded(v - u + 1) };
                let x = between(r, lo + inner, hi);
                let y = between(r, lo, x - 1);
                between(r, lo, y - 1 + inner)
            }
            HeightP::Trapezoid(a, b, plateau) => {
                let (lo, hi) = (a.resolve(c), b.resolve(c));
                if lo > hi {
                    return lo;
                }
                let range = hi - lo;
                if *plateau >= range {
                    return r.between(lo, hi);
                }
                let ps = (range - plateau) / 2;
                let pe = range - ps;
                let x = r.between(0, pe);
                lo + x + r.between(0, ps)
            }
            HeightP::Weighted(w) => match w.pick(r) {
                Some(h) => h.sample(r, c),
                None => c.min_y,
            },
        }
    }
}

// ---------------------------------------------------------------- Block sets

/// Ordered set of blocks (list or tag resolved in order).
#[derive(Debug, Clone, Default)]
pub struct Blockset {
    pub set: BlockSet,
    pub list: Vec<BlockId>,
}

impl Blockset {
    #[inline]
    pub fn contains(&self, b: BlockId) -> bool {
        self.set.contains(b)
    }

    pub fn parse(j: &Json, b: &Blocks) -> Result<Blockset, Error> {
        let mut out = Blockset::default();
        let add = |name: &str, out: &mut Blockset| -> Result<(), Error> {
            if let Some(t) = name.strip_prefix('#') {
                for &x in b.tag_list(t).ok_or_else(|| Error::Missing(format!("tag {t}")))? {
                    if !out.set.contains(x) {
                        out.set.insert(x);
                        out.list.push(x);
                    }
                }
            } else {
                let x = b.block_id(name).ok_or_else(|| Error::Missing(format!("block {name}")))?;
                if !out.set.contains(x) {
                    out.set.insert(x);
                    out.list.push(x);
                }
            }
            Ok(())
        };
        match j {
            Json::Str(s) => add(s, &mut out)?,
            Json::Arr(a) => {
                for e in a {
                    add(e.as_str().ok_or_else(|| bad("block list"))?, &mut out)?;
                }
            }
            _ => return Err(bad("block set")),
        }
        Ok(out)
    }
}

/// Fluids: bits 1 source water, 2 flowing water, 4 source lava, 8 flowing lava, 16 none.
pub fn fluid_bits(b: &Blocks, s: State) -> u8 {
    match (b.fluid(s), b.is_source(s)) {
        (1, true) => 1,
        (1, false) => 2,
        (2, true) => 4,
        (2, false) => 8,
        _ => 16,
    }
}

pub fn parse_fluids(j: &Json, b: &Blocks) -> Result<u8, Error> {
    let one = |n: &str| -> Result<u8, Error> {
        if let Some(t) = n.strip_prefix('#') {
            return b.fluid_tag(t).ok_or_else(|| Error::Missing(format!("fluid tag {t}")));
        }
        Ok(match n.strip_prefix("minecraft:").unwrap_or(n) {
            "water" => 1,
            "flowing_water" => 2,
            "lava" => 4,
            "flowing_lava" => 8,
            "empty" => 16,
            x => return Err(bad(format!("fluid {x}"))),
        })
    };
    match j {
        Json::Str(s) => one(s),
        Json::Arr(a) => {
            let mut m = 0;
            for e in a {
                m |= one(e.as_str().ok_or_else(|| bad("fluid"))?)?;
            }
            Ok(m)
        }
        _ => Err(bad("fluids")),
    }
}

// ---------------------------------------------------------------- Block predicates

#[derive(Debug, Clone)]
pub enum Pred {
    True,
    Blocks([i32; 3], Blockset),
    Tag([i32; 3], u16),
    Fluids([i32; 3], u8),
    Biomes(Vec<u16>),
    Sturdy([i32; 3], Dir),
    Solid([i32; 3]),
    Replaceable([i32; 3]),
    Survive([i32; 3], State),
    InWorld([i32; 3]),
    Height(Anchor, Anchor),
    Volume([i32; 3], [i32; 3], Box<Pred>),
    All(Vec<Pred>),
    Any(Vec<Pred>),
    Not(Box<Pred>),
}

pub struct ParseCtx<'a> {
    pub b: &'a Blocks,
    pub data: &'a DecorData,
    pub biome_index: &'a HashMap<String, u16>,
    pub providers: HashMap<String, Provider>,
}

impl Pred {
    pub fn parse(j: &Json, c: &ParseCtx) -> Result<Pred, Error> {
        let b = c.b;
        let t = ty(j).ok_or_else(|| bad("predicate without type"))?;
        let off = || vec3(j.get("offset"));
        let list = |k: &str| -> Result<Vec<Pred>, Error> {
            j.get(k)
                .and_then(Json::as_array)
                .ok_or_else(|| bad(k.to_owned()))?
                .iter()
                .map(|p| Pred::parse(p, c))
                .collect()
        };
        Ok(match t {
            // "Unobstructed": the game tests the entities in the cell itself (the offset is read
            // but never applied) and the generation region has no entity: always true.
            "true" | "unobstructed" => Pred::True,
            "matching_blocks" => {
                Pred::Blocks(off()?, Blockset::parse(j.get("blocks").ok_or_else(|| bad("blocks"))?, b)?)
            }
            "matching_block_tag" => {
                let tag = j.get("tag").and_then(Json::as_str).ok_or_else(|| bad("tag"))?;
                Pred::Tag(off()?, b.tag(tag).ok_or_else(|| Error::Missing(format!("tag {tag}")))?)
            }
            "matching_fluids" => Pred::Fluids(off()?, parse_fluids(j.get("fluids").ok_or_else(|| bad("fluids"))?, b)?),
            "matching_biomes" => {
                let mut v = Vec::new();
                let add = |n: &str, v: &mut Vec<u16>| {
                    if let Some(&i) = crate::data::with_full_id(n, |f| c.biome_index.get(f)) {
                        v.push(i);
                    }
                };
                match j.get("biomes") {
                    Some(Json::Str(s)) => add(s, &mut v),
                    Some(Json::Arr(a)) => a.iter().filter_map(Json::as_str).for_each(|s| add(s, &mut v)),
                    _ => return Err(bad("biomes")),
                }
                Pred::Biomes(v)
            }
            "has_sturdy_face" => Pred::Sturdy(
                off()?,
                Dir::from_name(j.get("direction").and_then(Json::as_str).ok_or_else(|| bad("direction"))?)
                    .ok_or_else(|| bad("direction"))?,
            ),
            "solid" => Pred::Solid(off()?),
            "replaceable" => Pred::Replaceable(off()?),
            "would_survive" => Pred::Survive(off()?, b.parse_state(j.get("state").ok_or_else(|| bad("state"))?)?),
            "inside_world_bounds" => Pred::InWorld(off()?),
            "height_range" => Pred::Height(
                Anchor::parse(j.get("min_inclusive").ok_or_else(|| bad("min"))?)?,
                Anchor::parse(j.get("max_inclusive").ok_or_else(|| bad("max"))?)?,
            ),
            "volume_match" => Pred::Volume(
                vec3(j.get("min"))?,
                vec3(j.get("max"))?,
                Box::new(Pred::parse(j.get("match").ok_or_else(|| bad("match"))?, c)?),
            ),
            "all_of" => Pred::All(list("predicates")?),
            "any_of" => Pred::Any(list("predicates")?),
            "not" => Pred::Not(Box::new(Pred::parse(j.get("predicate").ok_or_else(|| bad("predicate"))?, c)?)),
            t => return Err(bad(format!("predicate {t}"))),
        })
    }

    pub fn test(&self, r: &Region, p: [i32; 3]) -> bool {
        let b = r.b;
        let at = |o: &[i32; 3]| r.get(p[0] + o[0], p[1] + o[1], p[2] + o[2]);
        match self {
            Pred::True => true,
            Pred::Blocks(o, set) => set.contains(b.block(at(o))),
            Pred::Tag(o, t) => b.in_tag(at(o), *t),
            Pred::Fluids(o, m) => fluid_bits(b, at(o)) & m != 0,
            Pred::Biomes(v) => {
                let bi = r.biome(p[0], p[1], p[2]);
                v.contains(&bi)
            }
            Pred::Sturdy(o, d) => b.sturdy(at(o), *d),
            Pred::Solid(o) => b.legacy_solid(at(o)),
            Pred::Replaceable(o) => b.replaceable(at(o)),
            Pred::Survive(o, s) => decor_rules::can_survive(r, *s, [p[0] + o[0], p[1] + o[1], p[2] + o[2]]),
            Pred::InWorld(o) => !r.outside_height(p[1] + o[1]),
            Pred::Height(lo, hi) => {
                let c = r.gen_ctx();
                p[1] >= lo.resolve(c) && p[1] <= hi.resolve(c)
            }
            Pred::Volume(mn, mx, m) => {
                for ox in mn[0]..=mx[0] {
                    for oz in mn[2]..=mx[2] {
                        for oy in mn[1]..=mx[1] {
                            if !m.test(r, [p[0] + ox, p[1] + oy, p[2] + oz]) {
                                return false;
                            }
                        }
                    }
                }
                true
            }
            Pred::All(v) => v.iter().all(|x| x.test(r, p)),
            Pred::Any(v) => v.iter().any(|x| x.test(r, p)),
            Pred::Not(x) => !x.test(r, p),
        }
    }
}

// ---------------------------------------------------------------- Rule tests

#[derive(Debug, Clone)]
pub enum Rule {
    Always,
    Block(BlockId),
    State(State),
    Tag(u16),
    RandomBlock(BlockId, f32),
    RandomState(State, f32),
    Height(i32, i32),
    All(Vec<Rule>),
    Any(Vec<Rule>),
    Not(Box<Rule>),
}

impl Rule {
    pub fn parse(j: &Json, b: &Blocks) -> Result<Rule, Error> {
        let t = j.get("predicate_type").and_then(Json::as_str).ok_or_else(|| bad("rule test"))?;
        let t = t.strip_prefix("minecraft:").unwrap_or(t);
        let block = |k: &str| -> Result<BlockId, Error> {
            let n = j.get(k).and_then(Json::as_str).ok_or_else(|| bad(k.to_owned()))?;
            b.block_id(n).ok_or_else(|| Error::Missing(format!("block {n}")))
        };
        let list = |k: &str| -> Result<Vec<Rule>, Error> {
            j.get(k)
                .and_then(Json::as_array)
                .ok_or_else(|| bad(k.to_owned()))?
                .iter()
                .map(|x| Rule::parse(x, b))
                .collect()
        };
        Ok(match t {
            "always_true" => Rule::Always,
            "block_match" => Rule::Block(block("block")?),
            "blockstate_match" => Rule::State(b.parse_state(j.get("block_state").ok_or_else(|| bad("block_state"))?)?),
            "tag_match" => {
                let n = j.get("tag").and_then(Json::as_str).ok_or_else(|| bad("tag"))?;
                Rule::Tag(b.tag(n).ok_or_else(|| Error::Missing(format!("tag {n}")))?)
            }
            "random_block_match" => Rule::RandomBlock(block("block")?, get_f32(j, "probability")?),
            "random_blockstate_match" => Rule::RandomState(
                b.parse_state(j.get("block_state").ok_or_else(|| bad("block_state"))?)?,
                get_f32(j, "probability")?,
            ),
            "height_match" => Rule::Height(get_i32(j, "min_inclusive")?, get_i32(j, "max_inclusive")?),
            "all_of" => Rule::All(list("rules")?),
            "any_of" => Rule::Any(list("rules")?),
            "not" => Rule::Not(Box::new(Rule::parse(j.get("rule").ok_or_else(|| bad("rule"))?, b)?)),
            t => return Err(bad(format!("rule test {t}"))),
        })
    }

    #[inline]
    pub fn test(&self, b: &Blocks, s: State, y: i32, r: &mut DecoRng) -> bool {
        match self {
            Rule::Always => true,
            Rule::Block(x) => b.is(s, *x),
            Rule::State(x) => s == *x,
            Rule::Tag(t) => b.in_tag(s, *t),
            Rule::RandomBlock(x, p) => b.is(s, *x) && r.next_float() < *p,
            Rule::RandomState(x, p) => s == *x && r.next_float() < *p,
            Rule::Height(lo, hi) => y >= *lo && y <= *hi,
            Rule::All(v) => v.iter().all(|x| x.test(b, s, y, r)),
            Rule::Any(v) => v.iter().any(|x| x.test(b, s, y, r)),
            Rule::Not(x) => !x.test(b, s, y, r),
        }
    }
}

// ---------------------------------------------------------------- State providers

#[derive(Debug, Clone)]
pub struct NoiseP {
    pub noise: NoiseStack,
    pub scale: f32,
    pub states: Vec<State>,
}

impl NoiseP {
    fn value(&self, p: [i32; 3], scale: f64) -> f32 {
        self.noise.get(f64::from(p[0]) * scale, f64::from(p[1]) * scale, f64::from(p[2]) * scale)
    }

    fn pick(states: &[State], v: f32) -> State {
        let t = jclamp_f((1.0 + v) / 2.0, 0.0, 0.9999);
        states[(t * states.len() as f32) as usize]
    }
}

#[derive(Debug, Clone)]
pub enum Provider {
    Simple(State),
    Weighted(Weighted<State>),
    RandomBlock(Vec<BlockId>),
    /// `pp`: property resolved at read time (`RUSTCRAFT_OPT_DECOR_IDS`).
    RandomizedInt { source: Box<Provider>, prop: String, pp: Option<Prop>, values: IntP },
    Rotated { source: Box<Provider>, dir: Option<Dir> },
    CopyProps(Box<Provider>),
    RuleBased { rules: Vec<(Pred, Provider)>, fallback: Option<Box<Provider>> },
    Noise(NoiseP),
    NoiseThreshold { n: NoiseP, threshold: f32, high_chance: f32, default: State, low: Vec<State>, high: Vec<State> },
    DualNoise { n: NoiseP, variety: (i32, i32), slow: NoiseStack, slow_scale: f32 },
}

fn states_list(j: &Json, b: &Blocks) -> Result<Vec<State>, Error> {
    j.as_array().ok_or_else(|| bad("state list"))?.iter().map(|s| b.parse_state(s)).collect()
}

fn noise_p(j: &Json, b: &Blocks) -> Result<NoiseP, Error> {
    let seed = j.get("seed").and_then(|v| match v {
        Json::Num(n) => n.parse::<i64>().ok(),
        _ => None,
    });
    let seed = seed.ok_or_else(|| bad("seed"))?;
    let params = NormalNoise::from_json(j.get("noise").ok_or_else(|| bad("noise"))?)?;
    let noise = params.create(&mut Rng::legacy(seed));
    Ok(NoiseP {
        noise,
        scale: get_f32(j, "scale")?,
        states: j.get("states").map(|s| states_list(s, b)).transpose()?.unwrap_or_default(),
    })
}

impl Provider {
    pub fn parse(j: &Json, c: &mut ParseCtx) -> Result<Provider, Error> {
        if let Json::Str(s) = j {
            // Identifier of the provider registry, otherwise bare state.
            let full = crate::data::full_id(s);
            if let Some(p) = c.providers.get(&full) {
                return Ok(p.clone());
            }
            if c.data.raw("block_state_provider", &full).is_some() {
                let pj = c.data.json("block_state_provider", &full)?;
                let p = Provider::parse(&pj, c)?;
                c.providers.insert(full, p.clone());
                return Ok(p);
            }
            return Ok(Provider::Simple(c.b.parse_state(j)?));
        }
        let Some(t) = ty(j) else { return Ok(Provider::Simple(c.b.parse_state(j)?)) };
        let b = c.b;
        Ok(match t {
            "simple" | "simple_state_provider" => {
                Provider::Simple(b.parse_state(j.get("state").ok_or_else(|| bad("state"))?)?)
            }
            "weighted" | "weighted_state_provider" => {
                let mut items = Vec::new();
                for e in j.get("entries").and_then(Json::as_array).ok_or_else(|| bad("entries"))? {
                    items.push((b.parse_state(e.get("data").ok_or_else(|| bad("data"))?)?, get_i32(e, "weight")?));
                }
                Provider::Weighted(Weighted::new(items))
            }
            "random_block" => {
                Provider::RandomBlock(Blockset::parse(j.get("blocks").ok_or_else(|| bad("blocks"))?, b)?.list)
            }
            "randomized_int" | "randomized_int_state_provider" => Provider::RandomizedInt {
                source: Box::new(Provider::parse(j.get("source").ok_or_else(|| bad("source"))?, c)?),
                pp: c.b.prop(j.get("property").and_then(Json::as_str).ok_or_else(|| bad("property"))?),
                prop: j.get("property").and_then(Json::as_str).ok_or_else(|| bad("property"))?.to_owned(),
                values: IntP::parse(j.get("values").ok_or_else(|| bad("values"))?)?,
            },
            "rotated" | "rotated_block_provider" => Provider::Rotated {
                source: Box::new(Provider::parse(j.get("state").ok_or_else(|| bad("state"))?, c)?),
                dir: j.get("direction").and_then(Json::as_str).and_then(Dir::from_name),
            },
            "copy_properties" => {
                Provider::CopyProps(Box::new(Provider::parse(j.get("source").ok_or_else(|| bad("source"))?, c)?))
            }
            "rule_based" | "rule_based_state_provider" => {
                let mut rules = Vec::new();
                for r in j.get("rules").and_then(Json::as_array).ok_or_else(|| bad("rules"))? {
                    let p = Pred::parse(r.get("if_true").ok_or_else(|| bad("if_true"))?, c)?;
                    let t = Provider::parse(r.get("then").ok_or_else(|| bad("then"))?, c)?;
                    rules.push((p, t));
                }
                let fallback = match j.get("fallback") {
                    Some(f) => Some(Box::new(Provider::parse(f, c)?)),
                    None => None,
                };
                Provider::RuleBased { rules, fallback }
            }
            "noise" | "noise_provider" => Provider::Noise(noise_p(j, b)?),
            "noise_threshold" | "noise_threshold_provider" => Provider::NoiseThreshold {
                n: noise_p(j, b)?,
                threshold: get_f32(j, "threshold")?,
                high_chance: get_f32(j, "high_chance")?,
                default: b.parse_state(j.get("default_state").ok_or_else(|| bad("default_state"))?)?,
                low: states_list(j.get("low_states").ok_or_else(|| bad("low_states"))?, b)?,
                high: states_list(j.get("high_states").ok_or_else(|| bad("high_states"))?, b)?,
            },
            "dual_noise" | "dual_noise_provider" => {
                let n = noise_p(j, b)?;
                let seed = n_seed(j)?;
                let slow = NormalNoise::from_json(j.get("slow_noise").ok_or_else(|| bad("slow_noise"))?)?
                    .create(&mut Rng::legacy(seed));
                let v = j.get("variety").ok_or_else(|| bad("variety"))?;
                let variety = match v {
                    Json::Arr(a) if a.len() == 2 => {
                        (a[0].as_i32().ok_or_else(|| bad("variety"))?, a[1].as_i32().ok_or_else(|| bad("variety"))?)
                    }
                    Json::Num(_) => {
                        let x = v.as_i32().ok_or_else(|| bad("variety"))?;
                        (x, x)
                    }
                    _ => (get_i32(v, "min_inclusive")?, get_i32(v, "max_inclusive")?),
                };
                Provider::DualNoise { n, variety, slow, slow_scale: get_f32(j, "slow_scale")? }
            }
            t => return Err(bad(format!("state provider {t}"))),
        })
    }

    /// State provided (never empty: otherwise the current state at p).
    pub fn get(&self, r: &Region, rng: &mut DecoRng, p: [i32; 3]) -> State {
        match self.get_opt(r, rng, p) {
            Some(s) => s,
            None => r.get_p(p),
        }
    }

    pub fn get_opt(&self, r: &Region, rng: &mut DecoRng, p: [i32; 3]) -> Option<State> {
        let b = r.b;
        Some(match self {
            Provider::Simple(s) => *s,
            Provider::Weighted(w) => *w.pick(rng)?,
            Provider::RandomBlock(list) => {
                if list.is_empty() {
                    return None;
                }
                b.default_state(list[rng.next_int_bounded(list.len() as i32) as usize])
            }
            Provider::RandomizedInt { source, prop, pp, values } => {
                let s = source.get(r, rng, p);
                match if crate::opt::decor_ids() { *pp } else { b.prop(prop) } {
                    Some(pp) if b.get_int(s, pp).is_some() => {
                        let v = values.sample(rng);
                        b.with_int(s, pp, v)
                    }
                    _ => s,
                }
            }
            Provider::Rotated { source, dir } => {
                let d = match dir {
                    Some(d) => *d,
                    None => Dir::ALL[rng.next_int_bounded(6) as usize],
                };
                let mut s = source.get(r, rng, p);
                s = b.with_axis(s, b.p.axis, d.axis());
                s = b.with_dir(s, b.p.facing, d);
                if d.is_horizontal() {
                    s = b.with_dir(s, b.p.horizontal_facing, d);
                }
                s
            }
            Provider::CopyProps(source) => {
                let s = source.get(r, rng, p);
                b.copy_props(r.get_p(p), s)
            }
            Provider::RuleBased { rules, fallback } => {
                for (pred, then) in rules {
                    if pred.test(r, p)
                        && let Some(s) = then.get_opt(r, rng, p)
                    {
                        return Some(s);
                    }
                }
                return fallback.as_ref().and_then(|f| f.get_opt(r, rng, p));
            }
            Provider::Noise(n) => NoiseP::pick(&n.states, n.value(p, f64::from(n.scale))),
            Provider::NoiseThreshold { n, threshold, high_chance, default, low, high } => {
                let v = f64::from(n.value(p, f64::from(n.scale)));
                if v < f64::from(*threshold) {
                    low[rng.next_int_bounded(low.len() as i32) as usize]
                } else if rng.next_float() < *high_chance {
                    high[rng.next_int_bounded(high.len() as i32) as usize]
                } else {
                    *default
                }
            }
            Provider::DualNoise { n, variety, slow, slow_scale } => {
                let ss = f64::from(*slow_scale);
                let sv = |q: [i32; 3]| slow.get(f64::from(q[0]) * ss, f64::from(q[1]) * ss, f64::from(q[2]) * ss);
                let v = f64::from(sv(p));
                let local = clamped_map_d(v, -1.0, 1.0, f64::from(variety.0), f64::from(variety.1 + 1)) as i32;
                let mut cand: [State; 16] = [0; 16];
                let k = local.clamp(0, 16) as usize;
                for (i, c) in cand.iter_mut().enumerate().take(k) {
                    let ii = i as i32;
                    *c = NoiseP::pick(&n.states, sv([p[0] + ii * 54545, p[1], p[2] + ii * 34234]));
                }
                NoiseP::pick(&cand[..k], n.value(p, f64::from(n.scale)))
            }
        })
    }

    /// Can the provider give nothing (rules without fallback, empty random block)?
    pub fn first_state(&self) -> Option<State> {
        match self {
            Provider::Simple(s) => Some(*s),
            Provider::Weighted(w) => w.items.first().map(|i| i.0),
            _ => None,
        }
    }
}

fn n_seed(j: &Json) -> Result<i64, Error> {
    j.get("seed")
        .and_then(|v| match v {
            Json::Num(n) => n.parse::<i64>().ok(),
            _ => None,
        })
        .ok_or_else(|| bad("seed"))
}

pub fn clamped_map_d(v: f64, a: f64, b: f64, c: f64, d: f64) -> f64 {
    let t = (v - a) / (b - a);
    if t < 0.0 {
        c
    } else if t > 1.0 {
        d
    } else {
        c + t * (d - c)
    }
}

pub fn clamped_map_f(v: f32, a: f32, b: f32, c: f32, d: f32) -> f32 {
    let t = (v - a) / (b - a);
    if t < 0.0 {
        c
    } else if t > 1.0 {
        d
    } else {
        c + t * (d - c)
    }
}

/// Property of dynamic name (read).
pub fn prop_of(b: &Blocks, name: &str) -> Option<Prop> {
    b.prop(name)
}
