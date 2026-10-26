//! Density functions (spec § 3): tree read from the JSON, preparation (references expanded,
//! caches deduplicated, slices of the uniform axes), declared ranges, compilation into
//! samplers. Evaluation (point and volume) is in `sampler.rs`.

use hashbrown::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::Error;
use crate::json::Json;
use crate::math::{Interval, jmax, round_half_up};

pub const AX_X: u8 = 1;
pub const AX_Y: u8 = 2;
pub const AX_Z: u8 = 4;
pub const AX_ALL: u8 = 7;

/// f32 compared and hashed by its bits (structural equality of the functions).
#[derive(Debug, Clone, Copy)]
pub struct F32(pub f32);
impl PartialEq for F32 {
    fn eq(&self, o: &Self) -> bool {
        self.0.to_bits() == o.0.to_bits()
    }
}
impl Eq for F32 {}
impl Hash for F32 {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.0.to_bits().hash(h);
    }
}

#[derive(Debug, Clone, Copy)]
pub struct F64(pub f64);
impl PartialEq for F64 {
    fn eq(&self, o: &Self) -> bool {
        self.0.to_bits() == o.0.to_bits()
    }
}
impl Eq for F64 {}
impl Hash for F64 {
    fn hash<H: Hasher>(&self, h: &mut H) {
        self.0.to_bits().hash(h);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    pub fn bit(self) -> u8 {
        match self {
            Axis::X => AX_X,
            Axis::Y => AX_Y,
            Axis::Z => AX_Z,
        }
    }

    #[inline]
    pub fn choose(self, x: i32, y: i32, z: i32) -> i32 {
        match self {
            Axis::X => x,
            Axis::Y => y,
            Axis::Z => z,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Simple {
    BlendAlpha,
    BlendOffset,
    Beardifier,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tiling {
    Clamp,
    Repeat,
    Mirrored,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Metric {
    Euclidean,
    EuclideanSquared,
    Manhattan,
    Chebyshev,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ShiftKind {
    Shift,
    A,
    B,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Unary {
    Abs,
    Square,
    Cube,
    Sqrt,
    HalfNegative,
    QuarterNegative,
    Reciprocal,
    Negate,
    Squeeze,
    Log,
    Sign,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RoundKind {
    Floor,
    Round,
    Ceil,
    Truncate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Binary {
    Add,
    Sub,
    Mul,
    Div,
    Min,
    Max,
}

/// Cubic spline (§ 3.5), coordinates of type `C`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Spline<C> {
    Const(F32),
    Multi { coord: C, locs: Vec<F32>, values: Vec<Spline<C>>, ders: Vec<F32> },
}

pub type DfRef = Arc<Df>;

/// Density function as described by the data.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum Df {
    Const(F32),
    Simple(Simple),
    Noise {
        noise: String,
        xz: F64,
        y: F64,
        sx: DfRef,
        sy: DfRef,
        sz: DfRef,
    },
    EndIslands,
    Distance {
        p: [i32; 3],
        metric: Metric,
    },
    Gradient {
        axis: Axis,
        tiling: Tiling,
        from_c: i32,
        to_c: i32,
        from_v: F32,
        to_v: F32,
    },
    Shift(ShiftKind, String),
    Unary(Unary, DfRef),
    Round(RoundKind, DfRef, DfRef),
    Binary(Binary, DfRef, DfRef),
    Pow(DfRef, DfRef),
    Spline(Arc<Spline<DfRef>>),
    Lerp(DfRef, DfRef, DfRef),
    Clamp(DfRef, F32, F32),
    RangeChoice {
        input: DfRef,
        min: F32,
        max: F32,
        inside: DfRef,
        outside: DfRef,
    },
    IntervalSelect {
        input: DfRef,
        thresholds: Vec<F32>,
        functions: Vec<DfRef>,
    },
    Cache(DfRef),
    BlendDensity(DfRef),
    Interpolated {
        input: DfRef,
        cxz: i32,
        cy: i32,
    },
    Slice(Axis, i32, DfRef),
    FindTop {
        density: DfRef,
        upper: DfRef,
        lower: i32,
        cell: i32,
    },
    OldBlended {
        xz: F64,
        y: F64,
        xz_factor: F64,
        y_factor: F64,
        smear: F64,
    },
    /// Named reference to `worldgen/density_function/<id>`.
    Ref(String),
    /// Prepared cache (after deduplication).
    Prepared(u32),
}

impl Df {
    pub fn is_const(&self) -> Option<f32> {
        match self {
            Df::Const(v) => Some(v.0),
            _ => None,
        }
    }
}

fn zero() -> DfRef {
    Arc::new(Df::Const(F32(0.0)))
}

// ---------------------------------------------------------------- JSON reading

fn bad(what: &str) -> Error {
    Error::Format(format!("density function: {what}"))
}

fn f32_field(j: &Json, k: &str) -> Result<f32, Error> {
    j.get(k).and_then(Json::as_f32).ok_or_else(|| bad(k))
}

/// Noise value bounded like the game (±1e6, bounds included; NaN refused).
const NOISE_VALUE_MAX: f32 = 1_000_000.0;

fn noise_value(v: f32, k: &str) -> Result<f32, Error> {
    if (-NOISE_VALUE_MAX..=NOISE_VALUE_MAX).contains(&v) { Ok(v) } else { Err(bad(&format!("{k} outside ±1e6"))) }
}

fn noise_field(j: &Json, k: &str) -> Result<f32, Error> {
    noise_value(f32_field(j, k)?, k)
}

/// Bounds of `lower_bound` (`find_top_surface`): twice the height limits of a dimension.
const LOWER_BOUND_RANGE: std::ops::RangeInclusive<i32> = -4064..=4062;

fn f64_field(j: &Json, k: &str) -> Result<f64, Error> {
    j.get(k).and_then(Json::as_f64).ok_or_else(|| bad(k))
}

fn i32_field(j: &Json, k: &str) -> Result<i32, Error> {
    j.get(k).and_then(Json::as_i32).ok_or_else(|| bad(k))
}

fn axis_field(j: &Json, k: &str) -> Result<Axis, Error> {
    match j.get(k).and_then(Json::as_str) {
        Some("x") => Ok(Axis::X),
        Some("y") => Ok(Axis::Y),
        Some("z") => Ok(Axis::Z),
        _ => Err(bad(k)),
    }
}

fn strip_ns(s: &str) -> &str {
    s.strip_prefix("minecraft:").unwrap_or(s)
}

fn full(s: &str) -> String {
    crate::data::full_id(s)
}

pub fn parse(j: &Json) -> Result<DfRef, Error> {
    Ok(Arc::new(parse_df(j)?))
}

fn child(j: &Json, k: &str) -> Result<DfRef, Error> {
    parse(j.get(k).ok_or_else(|| bad(k))?)
}

fn child_or(j: &Json, k: &str, default: fn() -> DfRef) -> Result<DfRef, Error> {
    match j.get(k) {
        Some(v) => parse(v),
        None => Ok(default()),
    }
}

fn noise_ref(j: &Json, k: &str) -> Result<String, Error> {
    j.get(k).and_then(Json::as_str).map(full).ok_or_else(|| bad(&format!("{k} (noise reference expected)")))
}

fn parse_spline(j: &Json) -> Result<Spline<DfRef>, Error> {
    if let Some(v) = j.as_f32() {
        return Ok(Spline::Const(F32(v)));
    }
    let coord = child(j, "coordinate")?;
    let pts = j.get("points").and_then(Json::as_array).ok_or_else(|| bad("points"))?;
    if pts.is_empty() {
        return Err(bad("spline without point"));
    }
    let mut locs = Vec::new();
    let mut values = Vec::new();
    let mut ders = Vec::new();
    for p in pts {
        locs.push(F32(f32_field(p, "location")?));
        values.push(parse_spline(p.get("value").ok_or_else(|| bad("value"))?)?);
        ders.push(F32(f32_field(p, "derivative")?));
    }
    Ok(Spline::Multi { coord, locs, values, ders })
}

fn parse_df(j: &Json) -> Result<Df, Error> {
    if let Some(v) = j.as_f32() {
        return Ok(Df::Const(F32(noise_value(v, "constant")?)));
    }
    if let Some(s) = j.as_str() {
        return Ok(Df::Ref(full(s)));
    }
    let ty = j.get("type").and_then(Json::as_str).ok_or_else(|| bad("type"))?;
    let unary = |u| Ok(Df::Unary(u, child(j, "input")?));
    let binary = |b| Ok(Df::Binary(b, child(j, "left")?, child(j, "right")?));
    let round = |r| Ok(Df::Round(r, child(j, "input")?, child_or(j, "multiple", || Arc::new(Df::Const(F32(1.0))))?));
    match strip_ns(ty) {
        "constant" => Ok(Df::Const(F32(noise_field(j, "value")?))),
        "blend_alpha" => Ok(Df::Simple(Simple::BlendAlpha)),
        "blend_offset" => Ok(Df::Simple(Simple::BlendOffset)),
        "beardifier" => Ok(Df::Simple(Simple::Beardifier)),
        "noise" => Ok(Df::Noise {
            noise: noise_ref(j, "noise")?,
            xz: F64(f64_field(j, "xz_scale")?),
            y: F64(f64_field(j, "y_scale")?),
            sx: child_or(j, "shift_x", zero)?,
            sy: child_or(j, "shift_y", zero)?,
            sz: child_or(j, "shift_z", zero)?,
        }),
        "end_outer_islands" => Ok(Df::EndIslands),
        "distance_to_point" => {
            let p = j.get("point").and_then(Json::as_array).ok_or_else(|| bad("point"))?;
            let c = |i: usize| p.get(i).and_then(Json::as_i32).ok_or_else(|| bad("point"));
            let metric = match j.get("metric").and_then(Json::as_str) {
                Some("euclidean") => Metric::Euclidean,
                Some("euclidean_squared") => Metric::EuclideanSquared,
                Some("manhattan") => Metric::Manhattan,
                Some("chebyshev") => Metric::Chebyshev,
                _ => return Err(bad("metric")),
            };
            Ok(Df::Distance { p: [c(0)?, c(1)?, c(2)?], metric })
        }
        "gradient" => {
            let tiling = match j.get("tiling").and_then(Json::as_str) {
                None | Some("clamp_to_edge") => Tiling::Clamp,
                Some("repeat") => Tiling::Repeat,
                Some("mirrored_repeat") => Tiling::Mirrored,
                Some(_) => return Err(bad("tiling")),
            };
            let (from_c, to_c) = (i32_field(j, "from_coordinate")?, i32_field(j, "to_coordinate")?);
            if from_c == to_c {
                return Err(bad("from_coordinate = to_coordinate"));
            }
            Ok(Df::Gradient {
                axis: axis_field(j, "axis")?,
                tiling,
                from_c,
                to_c,
                from_v: F32(noise_field(j, "from_value")?),
                to_v: F32(noise_field(j, "to_value")?),
            })
        }
        "shift_a" => Ok(Df::Shift(ShiftKind::A, noise_ref(j, "noise")?)),
        "shift_b" => Ok(Df::Shift(ShiftKind::B, noise_ref(j, "noise")?)),
        "shift" => Ok(Df::Shift(ShiftKind::Shift, noise_ref(j, "noise")?)),
        "abs" => unary(Unary::Abs),
        "square" => unary(Unary::Square),
        "cube" => unary(Unary::Cube),
        "sqrt" => unary(Unary::Sqrt),
        "half_negative" => unary(Unary::HalfNegative),
        "quarter_negative" => unary(Unary::QuarterNegative),
        "reciprocal" => unary(Unary::Reciprocal),
        "negate" => unary(Unary::Negate),
        "squeeze" => unary(Unary::Squeeze),
        "log" => unary(Unary::Log),
        "sign" => unary(Unary::Sign),
        "floor" => round(RoundKind::Floor),
        "round" => round(RoundKind::Round),
        "ceil" => round(RoundKind::Ceil),
        "truncate" => round(RoundKind::Truncate),
        "add" => binary(Binary::Add),
        "sub" => binary(Binary::Sub),
        "mul" => binary(Binary::Mul),
        "div" => binary(Binary::Div),
        "min" => binary(Binary::Min),
        "max" => binary(Binary::Max),
        "pow" => Ok(Df::Pow(child(j, "base")?, child(j, "exponent")?)),
        "spline" => Ok(Df::Spline(Arc::new(parse_spline(j.get("spline").ok_or_else(|| bad("spline"))?)?))),
        "lerp" => Ok(Df::Lerp(child(j, "alpha")?, child(j, "first")?, child(j, "second")?)),
        "clamp" => {
            let (mn, mx) = (noise_field(j, "min")?, noise_field(j, "max")?);
            if mx < mn {
                return Err(bad("clamp : max < min"));
            }
            Ok(Df::Clamp(child(j, "input")?, F32(mn), F32(mx)))
        }
        "range_choice" => Ok(Df::RangeChoice {
            input: child(j, "input")?,
            min: F32(noise_field(j, "min_inclusive")?),
            max: F32(noise_field(j, "max_exclusive")?),
            inside: child(j, "when_in_range")?,
            outside: child(j, "when_out_of_range")?,
        }),
        "interval_select" => {
            let thresholds: Vec<F32> = j
                .get("thresholds")
                .and_then(Json::as_array)
                .ok_or_else(|| bad("thresholds"))?
                .iter()
                .map(|v| v.as_f32().ok_or_else(|| bad("thresholds")).and_then(|v| noise_value(v, "thresholds")).map(F32))
                .collect::<Result<_, _>>()?;
            let functions: Vec<DfRef> = j
                .get("functions")
                .and_then(Json::as_array)
                .ok_or_else(|| bad("functions"))?
                .iter()
                .map(parse)
                .collect::<Result<_, _>>()?;
            if functions.len() < 2 || thresholds.len() + 1 != functions.len() {
                return Err(bad("interval_select: number of thresholds"));
            }
            // Thresholds increasing in the wide sense.
            if thresholds.windows(2).any(|w| w[0].0.total_cmp(&w[1].0).is_gt()) {
                return Err(bad("interval_select: thresholds out of order"));
            }
            Ok(Df::IntervalSelect { input: child(j, "input")?, thresholds, functions })
        }
        "cache" => Ok(Df::Cache(child(j, "input")?)),
        "blend_density" => Ok(Df::BlendDensity(child(j, "input")?)),
        "interpolated" => {
            let (cxz, cy) = (i32_field(j, "cell_size_xz")?, i32_field(j, "cell_size_y")?);
            if cxz <= 0 || cy <= 0 {
                return Err(bad("cell size"));
            }
            Ok(Df::Interpolated { input: child(j, "input")?, cxz, cy })
        }
        "slice" => Ok(Df::Slice(axis_field(j, "axis")?, i32_field(j, "coordinate")?, child(j, "input")?)),
        "find_top_surface" => {
            let cell = i32_field(j, "cell_height")?;
            if cell <= 0 {
                return Err(bad("cell_height"));
            }
            let lower = i32_field(j, "lower_bound")?;
            if !LOWER_BOUND_RANGE.contains(&lower) {
                return Err(bad("lower_bound"));
            }
            Ok(Df::FindTop {
                density: child(j, "density")?,
                upper: child(j, "upper_bound")?,
                lower,
                cell,
            })
        }
        "old_blended_noise" => Ok(Df::OldBlended {
            xz: F64(f64_field(j, "xz_scale")?),
            y: F64(f64_field(j, "y_scale")?),
            xz_factor: F64(f64_field(j, "xz_factor")?),
            y_factor: F64(f64_field(j, "y_factor")?),
            smear: F64(f64_field(j, "smear_scale_multiplier")?),
        }),
        other => Err(bad(&format!("unknown type '{other}'"))),
    }
}

// ------------------------------------------------------- ranges and axes (§ 3.6)

/// What must be known for the ranges and the axes: named functions, noises, prepared caches.
pub trait Resolver {
    fn function(&self, id: &str) -> Option<DfRef>;
    fn noise_range(&self, id: &str) -> Interval;
    fn prepared(&self, id: u32) -> (Interval, u8);
}

fn resolve(r: &dyn Resolver, id: &str) -> DfRef {
    r.function(id).unwrap_or_else(|| panic!("unknown density function: {id}"))
}

pub fn axes(df: &Df, r: &dyn Resolver) -> u8 {
    match df {
        Df::Const(_) => 0,
        Df::Simple(Simple::Beardifier) => AX_ALL,
        Df::Simple(_) => AX_X | AX_Z,
        Df::Noise { xz, y, sx, sy, sz, .. } => {
            let mut a = AX_ALL;
            if y.0 == 0.0 {
                a &= !AX_Y;
            }
            if xz.0 == 0.0 {
                a &= !(AX_X | AX_Z);
            }
            a | axes(sx, r) | axes(sy, r) | axes(sz, r)
        }
        Df::EndIslands => AX_X | AX_Z,
        Df::Distance { .. } => AX_ALL,
        Df::Gradient { axis, .. } => axis.bit(),
        Df::Shift(ShiftKind::Shift, _) => AX_ALL,
        Df::Shift(_, _) => AX_X | AX_Z,
        Df::Unary(_, i) | Df::Cache(i) | Df::BlendDensity(i) | Df::Clamp(i, ..) => axes(i, r),
        Df::Interpolated { input, .. } => axes(input, r),
        Df::Round(_, a, b) | Df::Binary(_, a, b) | Df::Pow(a, b) => axes(a, r) | axes(b, r),
        Df::Spline(s) => spline_axes(s, r),
        Df::Lerp(a, b, c) => axes(a, r) | axes(b, r) | axes(c, r),
        Df::RangeChoice { input, inside, outside, .. } => axes(input, r) | axes(inside, r) | axes(outside, r),
        Df::IntervalSelect { input, functions, .. } => functions.iter().fold(axes(input, r), |a, f| a | axes(f, r)),
        Df::Slice(axis, _, i) => axes(i, r) & !axis.bit(),
        Df::FindTop { density, upper, .. } => (axes(density, r) | axes(upper, r)) & !AX_Y,
        Df::OldBlended { .. } => AX_ALL,
        Df::Ref(id) => axes(&resolve(r, id), r),
        Df::Prepared(id) => r.prepared(*id).1,
    }
}

fn spline_axes(s: &Spline<DfRef>, r: &dyn Resolver) -> u8 {
    match s {
        Spline::Const(_) => 0,
        Spline::Multi { coord, values, .. } => values.iter().fold(axes(coord, r), |a, v| a | spline_axes(v, r)),
    }
}

fn leaky(f: f32, v: f32) -> f32 {
    if v > 0.0 { v } else { v * f }
}

pub fn squeeze(v: f32) -> f32 {
    let c = crate::math::clamp(v, -1.0, 1.0);
    c / 2.0 - c * c * c / 24.0
}

pub fn round_to_integer(v: f32, k: RoundKind) -> f32 {
    match k {
        RoundKind::Floor => f64::from(v).floor() as f32,
        RoundKind::Round => round_half_up(v) as f32,
        RoundKind::Ceil => f64::from(v).ceil() as f32,
        RoundKind::Truncate => {
            if v > 0.0 {
                f64::from(v).floor() as f32
            } else {
                f64::from(v).ceil() as f32
            }
        }
    }
}

pub fn range(df: &Df, r: &dyn Resolver) -> Interval {
    match df {
        Df::Const(v) => Interval::exact(v.0),
        Df::Simple(Simple::BlendAlpha) => Interval::of(0.0, 1.0),
        Df::Simple(_) => Interval::INFINITE,
        Df::Noise { noise, .. } => r.noise_range(noise),
        Df::EndIslands => Interval::of(-0.84375, 0.5625),
        Df::Distance { .. } => Interval::of(0.0, f32::INFINITY),
        Df::Gradient { from_v, to_v, .. } => Interval::encapsulating2(from_v.0, to_v.0),
        Df::Shift(_, n) => Interval::mul(r.noise_range(n), Interval::exact(4.0)),
        Df::Unary(u, i) => {
            let i = range(i, r);
            match u {
                Unary::Abs => Interval::abs(i),
                Unary::Square => Interval::square(i),
                Unary::Cube => Interval::map_monotonic(i, |x| x * x * x),
                Unary::Sqrt => Interval::pow(i, Interval::exact(0.5)),
                Unary::HalfNegative => Interval::map_monotonic(i, |x| leaky(0.5, x)),
                Unary::QuarterNegative => Interval::map_monotonic(i, |x| leaky(0.25, x)),
                Unary::Reciprocal => Interval::reciprocal(i),
                Unary::Negate => Interval::sub(Interval::exact(0.0), i),
                Unary::Squeeze => Interval::map_monotonic(i, squeeze),
                Unary::Log => Interval::log(i),
                Unary::Sign => Interval::sign(i),
            }
        }
        Df::Round(k, i, m) => {
            let m = range(m, r);
            Interval::mul(Interval::map_monotonic(Interval::div(range(i, r), m), |v| round_to_integer(v, *k)), m)
        }
        Df::Binary(b, x, y) => {
            let (x, y) = (range(x, r), range(y, r));
            match b {
                Binary::Add => Interval::add(x, y),
                Binary::Sub => Interval::sub(x, y),
                Binary::Mul => Interval::mul(x, y),
                Binary::Div => Interval::div(x, y),
                Binary::Min => Interval::min(x, y),
                Binary::Max => Interval::max(x, y),
            }
        }
        Df::Pow(b, e) => Interval::pow(range(b, r), range(e, r)),
        Df::Spline(s) => spline_range(s, r),
        Df::Lerp(a, f, s) => Interval::lerp(range(a, r), range(f, r), range(s, r)),
        Df::Clamp(i, mn, mx) => Interval::clamp(range(i, r), mn.0, mx.0),
        Df::RangeChoice { inside, outside, .. } => Interval::encapsulating([range(inside, r), range(outside, r)]),
        Df::IntervalSelect { functions, .. } => Interval::encapsulating(functions.iter().map(|f| range(f, r))),
        Df::Cache(i) | Df::BlendDensity(i) | Df::Slice(_, _, i) => range(i, r),
        Df::Interpolated { input, .. } => range(input, r),
        Df::FindTop { upper, lower, .. } => {
            let lo = *lower as f32;
            Interval::of(lo, jmax(lo, range(upper, r).max))
        }
        Df::OldBlended { y, smear, .. } => {
            crate::noise::blended_fbm_range(-15, 684.412 * y.0 * smear.0, f64::from(LIMIT_FACTOR))
        }
        Df::Ref(id) => range(&resolve(r, id), r),
        Df::Prepared(id) => r.prepared(*id).0,
    }
}

pub const LIMIT_FACTOR: f32 = 0.999_984_74;

fn linear_extend(input: f32, locs: &[F32], value: f32, ders: &[F32], i: usize) -> f32 {
    let d = ders[i].0;
    if d == 0.0 { value } else { value + d * (input - locs[i].0) }
}

fn spline_range(s: &Spline<DfRef>, r: &dyn Resolver) -> Interval {
    use crate::math::jmin;
    let Spline::Multi { coord, locs, values, ders } = s else {
        let Spline::Const(v) = s else { unreachable!() };
        return Interval::exact(v.0);
    };
    let last = locs.len() - 1;
    let (mut mn, mut mx) = (f32::INFINITY, f32::NEG_INFINITY);
    let input = range(coord, r);
    if input.nai {
        return input;
    }
    if input.min < locs[0].0 {
        let fr = spline_range(&values[0], r);
        let e1 = linear_extend(input.min, locs, fr.min, ders, 0);
        let e2 = linear_extend(input.min, locs, fr.max, ders, 0);
        mn = jmin(mn, jmin(e1, e2));
        mx = jmax(mx, jmax(e1, e2));
    }
    if input.max > locs[last].0 {
        let lr = spline_range(&values[last], r);
        let e1 = linear_extend(input.max, locs, lr.min, ders, last);
        let e2 = linear_extend(input.max, locs, lr.max, ders, last);
        mn = jmin(mn, jmin(e1, e2));
        mx = jmax(mx, jmax(e1, e2));
    }
    let vr: Vec<Interval> = values.iter().map(|v| spline_range(v, r)).collect();
    for x in &vr {
        mn = jmin(mn, x.min);
        mx = jmax(mx, x.max);
    }
    for i in 0..last {
        let (x1, x2) = (locs[i].0, locs[i + 1].0);
        let xd = x2 - x1;
        let (min1, max1, min2, max2) = (vr[i].min, vr[i].max, vr[i + 1].min, vr[i + 1].max);
        let (d1, d2) = (ders[i].0, ders[i + 1].0);
        if d1 != 0.0 || d2 != 0.0 {
            let p1 = d1 * xd;
            let p2 = d2 * xd;
            let min_lerp1 = jmin(min1, min2);
            let max_lerp1 = jmax(max1, max2);
            let min_a = p1 - max2 + min1;
            let max_a = p1 - min2 + max1;
            let min_b = -p2 + min2 - max1;
            let max_b = -p2 + max2 - min1;
            let min_lerp2 = jmin(min_a, min_b);
            let max_lerp2 = jmax(max_a, max_b);
            mn = jmin(mn, min_lerp1 + 0.25 * min_lerp2);
            mx = jmax(mx, max_lerp1 + 0.25 * max_lerp2);
        }
    }
    Interval::of(mn, mx)
}

// ------------------------------------------------------------ preparation (§ 3.2)

/// Rewrite of a tree: the rule "expand and deduplicate the caches" or the rule of the uniform
/// axis slices.
pub trait Rewriter {
    fn rewrite(&mut self, f: &DfRef) -> DfRef;
}

/// Applies `rule` to the direct children (prepared caches and leaves stay as they are).
pub fn rewrite_children(f: &DfRef, rule: &mut dyn Rewriter, resolver: &dyn Resolver) -> DfRef {
    let mut c = |x: &DfRef| rule.rewrite(x);
    let n = match &**f {
        Df::Const(_)
        | Df::Simple(_)
        | Df::EndIslands
        | Df::Distance { .. }
        | Df::Gradient { .. }
        | Df::Shift(..)
        | Df::OldBlended { .. }
        | Df::Prepared(_) => return f.clone(),
        Df::Ref(id) => return c(&resolve(resolver, id)),
        Df::Noise { noise, xz, y, sx, sy, sz } => {
            Df::Noise { noise: noise.clone(), xz: *xz, y: *y, sx: c(sx), sy: c(sy), sz: c(sz) }
        }
        Df::Unary(u, i) => Df::Unary(*u, c(i)),
        Df::Round(k, i, m) => {
            let i = c(i);
            Df::Round(*k, i, c(m))
        }
        Df::Binary(b, x, y) => {
            let x = c(x);
            Df::Binary(*b, x, c(y))
        }
        Df::Pow(b, e) => {
            let b = c(b);
            Df::Pow(b, c(e))
        }
        Df::Spline(s) => Df::Spline(Arc::new(map_spline(s, &mut c))),
        Df::Lerp(a, f1, s) => {
            let a = c(a);
            let f1 = c(f1);
            Df::Lerp(a, f1, c(s))
        }
        Df::Clamp(i, mn, mx) => Df::Clamp(c(i), *mn, *mx),
        Df::RangeChoice { input, min, max, inside, outside } => {
            let input = c(input);
            let inside = c(inside);
            Df::RangeChoice { input, min: *min, max: *max, inside, outside: c(outside) }
        }
        Df::IntervalSelect { input, thresholds, functions } => {
            let input = c(input);
            Df::IntervalSelect {
                input,
                thresholds: thresholds.clone(),
                functions: functions.iter().map(&mut c).collect(),
            }
        }
        Df::Cache(i) => Df::Cache(c(i)),
        Df::BlendDensity(i) => Df::BlendDensity(c(i)),
        Df::Interpolated { input, cxz, cy } => Df::Interpolated { input: c(input), cxz: *cxz, cy: *cy },
        Df::Slice(a, k, i) => Df::Slice(*a, *k, c(i)),
        Df::FindTop { density, upper, lower, cell } => {
            let density = c(density);
            Df::FindTop { density, upper: c(upper), lower: *lower, cell: *cell }
        }
    };
    Arc::new(n)
}

fn map_spline(s: &Spline<DfRef>, c: &mut impl FnMut(&DfRef) -> DfRef) -> Spline<DfRef> {
    match s {
        Spline::Const(v) => Spline::Const(*v),
        Spline::Multi { coord, locs, values, ders } => {
            let coord = c(coord);
            Spline::Multi {
                coord,
                locs: locs.clone(),
                values: values.iter().map(|v| map_spline(v, c)).collect(),
                ders: ders.clone(),
            }
        }
    }
}

/// Slices rule: a child whose axes are strictly included in those of the parent is wrapped
/// in `slice` at 0 on the missing axes (x, then z, then y).
pub struct SliceAxes<'a> {
    pub parent: u8,
    pub resolver: &'a dyn Resolver,
}

impl Rewriter for SliceAxes<'_> {
    fn rewrite(&mut self, f: &DfRef) -> DfRef {
        if matches!(**f, Df::Const(_) | Df::Gradient { .. }) {
            return f.clone();
        }
        let r = self.resolver;
        let a = axes(f, r);
        if self.parent == a {
            return rewrite_children(f, self, r);
        }
        let nf = rewrite_children(f, &mut SliceAxes { parent: a, resolver: r }, r);
        let removed = self.parent & !a;
        let mut existing = 0u8;
        let mut cur = &nf;
        while let Df::Slice(ax, _, i) = &**cur {
            existing |= ax.bit();
            cur = i;
        }
        let filtered = removed & !existing;
        let mut out = nf.clone();
        if filtered & AX_X != 0 {
            out = Arc::new(Df::Slice(Axis::X, 0, out));
        }
        if filtered & AX_Z != 0 {
            out = Arc::new(Df::Slice(Axis::Z, 0, out));
        }
        if filtered & AX_Y != 0 {
            out = Arc::new(Df::Slice(Axis::Y, 0, out));
        }
        out
    }
}

/// Table of the named functions (JSON read once).
pub type Registry = HashMap<String, DfRef>;
