//! Carved caves (spec generation-terrain § 7): `cave` caves and `canyon` canyons, size mask of
//! the target chunk, then mask application (aquifers, top material on grass).

use std::sync::OnceLock;

use crate::data::DataPack;
use crate::json::Json;
use crate::material::GenContext;
use crate::rng::LegacyRandom;
use crate::{Error, data::full_id};

// ---------------------------------------------------------------- table sine

fn sin_table() -> &'static [f32] {
    static T: OnceLock<Vec<f32>> = OnceLock::new();
    T.get_or_init(|| (0..65536).map(|i| (f64::from(i) / 10_430.378_350_470_453).sin() as f32).collect())
}

/// The game's table sine (argument converted to f64).
#[inline]
pub fn sin_t(a: f64) -> f32 {
    sin_table()[((a * 10_430.378_350_470_453) as i64 & 65535) as usize]
}

#[inline]
pub fn cos_t(a: f64) -> f32 {
    sin_table()[((a * 10_430.378_350_470_453 + 16384.0) as i64 & 65535) as usize]
}

// ---------------------------------------------------------------- value providers

#[derive(Debug, Clone, Copy)]
pub enum FloatProvider {
    Const(f32),
    Uniform(f32, f32),
    Trapezoid { min: f32, max: f32, plateau: f32 },
}

impl FloatProvider {
    fn parse(j: &Json) -> Result<Self, Error> {
        if let Some(v) = j.as_f32() {
            return Ok(FloatProvider::Const(v));
        }
        let f = |k: &str| j.get(k).and_then(Json::as_f32).ok_or_else(|| Error::Format(format!("provider: {k}")));
        let t = j.get("type").and_then(Json::as_str).unwrap_or_default();
        match t.strip_prefix("minecraft:").unwrap_or(t) {
            "constant" => Ok(FloatProvider::Const(f("value")?)),
            "uniform" => Ok(FloatProvider::Uniform(f("min_inclusive")?, f("max_exclusive")?)),
            "trapezoid" => Ok(FloatProvider::Trapezoid { min: f("min")?, max: f("max")?, plateau: f("plateau")? }),
            other => Err(Error::Format(format!("float provider {other}"))),
        }
    }

    pub fn sample(&self, r: &mut LegacyRandom) -> f32 {
        match *self {
            FloatProvider::Const(v) => v,
            FloatProvider::Uniform(a, b) => r.next_float() * (b - a) + a,
            FloatProvider::Trapezoid { min, max, plateau } => {
                let range = max - min;
                let start = (range - plateau) / 2.0;
                let end = range - start;
                min + r.next_float() * end + r.next_float() * start
            }
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub enum IntProvider {
    Const(i32),
    Uniform(i32, i32),
    BiasedToBottom(i32, i32),
    VeryBiasedToBottom(i32, i32),
}

impl IntProvider {
    fn parse(j: &Json) -> Result<Self, Error> {
        if let Some(v) = j.as_i32() {
            return Ok(IntProvider::Const(v));
        }
        let f = |k: &str| j.get(k).and_then(Json::as_i32).ok_or_else(|| Error::Format(format!("provider: {k}")));
        let t = j.get("type").and_then(Json::as_str).unwrap_or_default();
        match t.strip_prefix("minecraft:").unwrap_or(t) {
            "constant" => Ok(IntProvider::Const(f("value")?)),
            "uniform" => Ok(IntProvider::Uniform(f("min_inclusive")?, f("max_inclusive")?)),
            "biased_to_bottom" => Ok(IntProvider::BiasedToBottom(f("min_inclusive")?, f("max_inclusive")?)),
            "very_biased_to_bottom" => Ok(IntProvider::VeryBiasedToBottom(f("min_inclusive")?, f("max_inclusive")?)),
            other => Err(Error::Format(format!("integer provider {other}"))),
        }
    }

    pub fn sample(&self, r: &mut LegacyRandom) -> i32 {
        match *self {
            IntProvider::Const(v) => v,
            IntProvider::Uniform(a, b) => r.next_int_bounded(b - a + 1) + a,
            IntProvider::BiasedToBottom(a, b) => {
                let n = r.next_int_bounded(b - a + 1) + 1;
                a + r.next_int_bounded(n)
            }
            IntProvider::VeryBiasedToBottom(a, b) => {
                let n = r.next_int_bounded(b - a + 1) + 1;
                let m = r.next_int_bounded(n) + 1;
                a + r.next_int_bounded(m)
            }
        }
    }
}

/// Uniform height between two resolved anchors (or constant).
#[derive(Debug, Clone, Copy)]
pub struct HeightProvider {
    pub min: i32,
    pub max: i32,
}

impl HeightProvider {
    fn parse(j: &Json, gc: &GenContext) -> Result<Self, Error> {
        let t = j.get("type").and_then(Json::as_str).unwrap_or_default();
        match t.strip_prefix("minecraft:").unwrap_or(t) {
            "uniform" => Ok(HeightProvider {
                min: gc.anchor(j.get("min_inclusive").ok_or_else(|| Error::Format("height".into()))?)?,
                max: gc.anchor(j.get("max_inclusive").ok_or_else(|| Error::Format("height".into()))?)?,
            }),
            "constant" => {
                let v = gc.anchor(j.get("value").ok_or_else(|| Error::Format("height".into()))?)?;
                Ok(HeightProvider { min: v, max: v })
            }
            "" => {
                let v = gc.anchor(j)?;
                Ok(HeightProvider { min: v, max: v })
            }
            other => Err(Error::Format(format!("height {other}"))),
        }
    }

    pub fn sample(&self, r: &mut LegacyRandom) -> i32 {
        if self.min > self.max { self.min } else { r.next_int_bounded(self.max - self.min + 1) + self.min }
    }
}

// ---------------------------------------------------------------- definitions

#[derive(Debug, Clone)]
pub struct CaveDef {
    pub y: HeightProvider,
    pub count: IntProvider,
    pub thickness: FloatProvider,
    pub weird: bool,
    pub room_vertical: FloatProvider,
    pub horizontal: FloatProvider,
    pub vertical: FloatProvider,
    pub start_vertical: FloatProvider,
    pub floor: FloatProvider,
}

#[derive(Debug, Clone)]
pub struct CanyonDef {
    pub y: HeightProvider,
    pub vertical_rotation: FloatProvider,
    pub distance_factor: FloatProvider,
    pub thickness: FloatProvider,
    pub width_smoothness: i32,
    pub horizontal_radius_factor: FloatProvider,
    pub vertical_default: f32,
    pub vertical_center: f32,
    pub y_scale: FloatProvider,
}

#[derive(Debug, Clone)]
pub enum CarverKind {
    Cave(CaveDef),
    Canyon(CanyonDef),
}

#[derive(Debug, Clone)]
pub struct Carver {
    pub probability: f32,
    pub kind: CarverKind,
}

impl Carver {
    pub fn load(pack: &DataPack, id: &str, gc: &GenContext) -> Result<Self, Error> {
        let j = pack.json("carver", &full_id(id))?;
        let bad = |k: &str| Error::Format(format!("cave {id}: {k}"));
        let fp = |j: &Json, k: &str| FloatProvider::parse(j.get(k).ok_or_else(|| bad(k))?);
        let probability = j.get("probability").and_then(Json::as_f32).ok_or_else(|| bad("probability"))?;
        let y = HeightProvider::parse(j.get("y").ok_or_else(|| bad("y"))?, gc)?;
        let t = j.get("type").and_then(Json::as_str).unwrap_or_default();
        let kind = match t.strip_prefix("minecraft:").unwrap_or(t) {
            "cave" | "nether_cave" => CarverKind::Cave(CaveDef {
                y,
                count: IntProvider::parse(j.get("count").ok_or_else(|| bad("count"))?)?,
                thickness: fp(&j, "thickness")?,
                weird: j.get("weird_thickness_bias").and_then(Json::as_bool).unwrap_or(false),
                room_vertical: fp(&j, "room_vertical_radius_multiplier")?,
                horizontal: fp(&j, "horizontal_radius_multiplier")?,
                vertical: fp(&j, "vertical_radius_multiplier")?,
                start_vertical: match j.get("start_vertical_radius_multiplier") {
                    Some(v) => FloatProvider::parse(v)?,
                    None => FloatProvider::Const(1.0),
                },
                floor: fp(&j, "floor_level")?,
            }),
            "canyon" => {
                let s = j.get("shape").ok_or_else(|| bad("shape"))?;
                CarverKind::Canyon(CanyonDef {
                    y,
                    vertical_rotation: fp(&j, "vertical_rotation")?,
                    distance_factor: fp(s, "distance_factor")?,
                    thickness: fp(s, "thickness")?,
                    width_smoothness: s
                        .get("width_smoothness")
                        .and_then(Json::as_i32)
                        .ok_or_else(|| bad("width_smoothness"))?,
                    horizontal_radius_factor: fp(s, "horizontal_radius_factor")?,
                    vertical_default: s
                        .get("vertical_radius_default_factor")
                        .and_then(Json::as_f32)
                        .ok_or_else(|| bad("vertical_radius_default_factor"))?,
                    vertical_center: s
                        .get("vertical_radius_center_factor")
                        .and_then(Json::as_f32)
                        .ok_or_else(|| bad("vertical_radius_center_factor"))?,
                    y_scale: fp(s, "y_scale")?,
                })
            }
            other => return Err(bad(&format!("type {other}"))),
        };
        Ok(Carver { probability, kind })
    }
}

// ---------------------------------------------------------------- mask

/// Size mask of a chunk: bit `(y − min) + (z + 16·x)·height`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CarvingMask {
    pub min_y: i32,
    pub max_y: i32,
    height: i32,
    bits: Vec<u64>,
    any: bool,
}

impl CarvingMask {
    pub fn new(min_y: i32, max_y: i32) -> Self {
        let height = (max_y - min_y + 1).max(0);
        CarvingMask { min_y, max_y, height, bits: vec![0; ((256 * height) as usize).div_ceil(64)], any: false }
    }

    #[inline]
    pub fn carve(&mut self, x: i32, y: i32, z: i32) {
        let i = (y - self.min_y + (z + (x << 4)) * self.height) as usize;
        self.bits[i >> 6] |= 1 << (i & 63);
        self.any = true;
    }

    pub fn is_empty(&self) -> bool {
        !self.any
    }

    #[inline]
    fn get(&self, i: usize) -> bool {
        self.bits[i >> 6] >> (i & 63) & 1 != 0
    }

    /// Contiguous ranges per column (increasing index: x major, z minor, y rising):
    /// `f(x, z, bottom, top)`.
    pub fn visit(&self, mut f: impl FnMut(i32, i32, i32, i32)) {
        let h = self.height as usize;
        if h == 0 {
            return;
        }
        for column in 0..256usize {
            let base = column * h;
            let mut y = 0usize;
            while y < h {
                // fast skip of empty words
                let i = base + y;
                if i & 63 == 0 && self.bits[i >> 6] == 0 && y + 64 <= h {
                    y += 64;
                    continue;
                }
                if !self.get(i) {
                    y += 1;
                    continue;
                }
                let start = y;
                while y < h && self.get(base + y) {
                    y += 1;
                }
                f(
                    (column >> 4) as i32 & 15,
                    column as i32 & 15,
                    start as i32 + self.min_y,
                    (y - 1) as i32 + self.min_y,
                );
            }
        }
    }
}

// ---------------------------------------------------------------- carving

/// Exclusion test of a block of the ellipsoid (normalised coordinates, y of the block).
#[derive(Debug, Clone, Copy)]
pub enum Skip<'a> {
    /// Cave: under the floor or outside the unit sphere.
    Cave { floor: f64 },
    /// Canyon: width per y (from the bottom of the generator).
    Canyon { widths: &'a [f32], min_gen_y: i32 },
}

/// Destination of a carving: the target chunk, or the replay recorder.
trait Sink {
    fn begin(&mut self) {}
    fn end(&mut self) {}
    fn can_reach(&mut self, x: f64, z: f64, step: i32, total: i32, thickness: f32) -> bool;
    #[allow(clippy::too_many_arguments)]
    fn ellipsoid(&mut self, x: f64, y: f64, z: f64, hr: f64, vr: f64, skip: Skip<'_>);
    fn take_widths(&mut self, n: usize) -> Vec<f32>;
    fn give_widths(&mut self, w: Vec<f32>);
}

/// Target chunk: corner, mask and options of the optimised paths.
pub struct Target<'a> {
    pub min_x: i32,
    pub min_z: i32,
    pub mask: &'a mut CarvingMask,
    /// `RUSTCRAFT_OPT_CARVER_YD`: vertical difference per y computed once per ellipsoid.
    pub yd_once: bool,
    /// `RUSTCRAFT_OPT_CARVER_WIDTHS`: canyon width buffer reused.
    pub reuse_widths: bool,
    yd: Vec<f64>,
    widths: Vec<f32>,
    /// Stack of tunnel ends of the replay (reused).
    ends: Vec<u32>,
}

impl<'a> Target<'a> {
    pub fn new(min_x: i32, min_z: i32, mask: &'a mut CarvingMask, yd_once: bool, reuse_widths: bool) -> Self {
        Target { min_x, min_z, mask, yd_once, reuse_widths, yd: Vec::new(), widths: Vec::new(), ends: Vec::new() }
    }

    #[inline]
    fn mid_x(&self) -> f64 {
        f64::from(self.min_x + 8)
    }
    #[inline]
    fn mid_z(&self) -> f64 {
        f64::from(self.min_z + 8)
    }

    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    fn ellipsoid_with(&mut self, x: f64, y: f64, z: f64, hr: f64, vr: f64, skip: impl Fn(f64, f64, f64, i32) -> bool) {
        let max_delta = 16.0 + hr * 2.0;
        if (x - self.mid_x()).abs() > max_delta || (z - self.mid_z()).abs() > max_delta {
            return;
        }
        let (cx0, cz0) = (self.min_x, self.min_z);
        let min_xi = ((x - hr).floor() as i32 - cx0 - 1).max(0);
        let max_xi = ((x + hr).floor() as i32 - cx0).min(15);
        let min_y = ((y - vr).floor() as i32 - 1).max(self.mask.min_y);
        let max_y = ((y + vr).floor() as i32 + 1).min(self.mask.max_y);
        let min_zi = ((z - hr).floor() as i32 - cz0 - 1).max(0);
        let max_zi = ((z + hr).floor() as i32 - cz0).min(15);
        let mut yd_ready = false;
        for xi in min_xi..=max_xi {
            let xd = (f64::from(cx0 + xi) + 0.5 - x) / hr;
            for zi in min_zi..=max_zi {
                let zd = (f64::from(cz0 + zi) + 0.5 - z) / hr;
                if xd * xd + zd * zd >= 1.0 {
                    continue;
                }
                if self.yd_once {
                    // Same expression, once per y: index wy − min_y − 1.
                    if !yd_ready {
                        self.yd.clear();
                        self.yd.extend((min_y + 1..=max_y).map(|wy| (f64::from(wy) - 0.5 - y) / vr));
                        yd_ready = true;
                    }
                    let mut wy = max_y;
                    while wy > min_y {
                        if !skip(xd, self.yd[(wy - min_y - 1) as usize], zd, wy) {
                            self.mask.carve(xi, wy, zi);
                        }
                        wy -= 1;
                    }
                } else {
                    let mut wy = max_y;
                    while wy > min_y {
                        let yd = (f64::from(wy) - 0.5 - y) / vr;
                        if !skip(xd, yd, zd, wy) {
                            self.mask.carve(xi, wy, zi);
                        }
                        wy -= 1;
                    }
                }
            }
        }
    }
}

impl Sink for Target<'_> {
    fn can_reach(&mut self, x: f64, z: f64, step: i32, total: i32, thickness: f32) -> bool {
        let (dx, dz) = (x - self.mid_x(), z - self.mid_z());
        let rem = f64::from(total - step);
        let rr = f64::from(thickness + 2.0 + 16.0);
        dx * dx + dz * dz - rem * rem <= rr * rr
    }

    #[inline]
    fn ellipsoid(&mut self, x: f64, y: f64, z: f64, hr: f64, vr: f64, skip: Skip<'_>) {
        match skip {
            Skip::Cave { floor } => self.ellipsoid_with(x, y, z, hr, vr, move |xd: f64, yd: f64, zd: f64, _y: i32| {
                yd <= floor || xd * xd + yd * yd + zd * zd >= 1.0
            }),
            Skip::Canyon { widths, min_gen_y } => {
                self.ellipsoid_with(x, y, z, hr, vr, |xd: f64, yd: f64, zd: f64, wy: i32| {
                    let yi = wy - min_gen_y;
                    (xd * xd + zd * zd) * f64::from(widths[(yi - 1) as usize]) + yd * yd / 6.0 >= 1.0
                })
            }
        }
    }

    fn take_widths(&mut self, n: usize) -> Vec<f32> {
        if self.reuse_widths {
            let mut w = std::mem::take(&mut self.widths);
            w.clear();
            w.resize(n, 0.0);
            w
        } else {
            vec![0f32; n]
        }
    }

    fn give_widths(&mut self, w: Vec<f32>) {
        if self.reuse_widths {
            self.widths = w;
        }
    }
}

const PI_F: f32 = std::f32::consts::PI;

impl CaveDef {
    fn carve(&self, r: &mut LegacyRandom, src_x: i32, src_z: i32, t: &mut impl Sink) {
        let max_distance = (4 * 2 - 1) * 16;
        let count = self.count.sample(r);
        for _ in 0..count {
            let x = f64::from(src_x * 16 + r.next_int_bounded(16));
            let y = f64::from(self.y.sample(r));
            let z = f64::from(src_z * 16 + r.next_int_bounded(16));
            let hm = f64::from(self.horizontal.sample(r));
            let vm = f64::from(self.vertical.sample(r));
            let sm = f64::from(self.start_vertical.sample(r));
            let floor = f64::from(self.floor.sample(r));
            let skip = Skip::Cave { floor };
            let mut tunnels = 1;
            if r.next_int_bounded(4) == 0 {
                let ys = f64::from(self.room_vertical.sample(r));
                let th = 1.0 + r.next_float() * 6.0;
                let hr = 1.5 + f64::from(sin_t(f64::from(PI_F / 2.0)) * th);
                let vr = hr * ys;
                t.ellipsoid(x + 1.0, y, z, hr, vr, skip);
                tunnels += r.next_int_bounded(4);
            }
            for _ in 0..tunnels {
                let rot_h = r.next_float() * (PI_F * 2.0);
                let rot_v = (r.next_float() - 0.5) / 4.0;
                let th = self.thickness(r);
                let dist = max_distance - r.next_int_bounded(max_distance / 4);
                let seed = r.next_long();
                tunnel(t, seed, x, y, z, hm, vm, th, rot_h, rot_v, 0, dist, sm, skip);
            }
        }
    }

    fn thickness(&self, r: &mut LegacyRandom) -> f32 {
        let mut th = self.thickness.sample(r);
        if self.weird && r.next_int_bounded(10) == 0 {
            th *= r.next_float() * r.next_float() * 3.0 + 1.0;
        }
        th
    }
}

#[allow(clippy::too_many_arguments)]
fn tunnel(
    t: &mut impl Sink,
    seed: i64,
    x: f64,
    y: f64,
    z: f64,
    hm: f64,
    vm: f64,
    th: f32,
    rot_h: f32,
    rot_v: f32,
    step: i32,
    dist: i32,
    y_scale: f64,
    skip: Skip<'_>,
) {
    t.begin();
    tunnel_body(t, seed, x, y, z, hm, vm, th, rot_h, rot_v, step, dist, y_scale, skip);
    t.end();
}

#[allow(clippy::too_many_arguments)]
fn tunnel_body(
    t: &mut impl Sink,
    seed: i64,
    mut x: f64,
    mut y: f64,
    mut z: f64,
    hm: f64,
    vm: f64,
    th: f32,
    mut rot_h: f32,
    mut rot_v: f32,
    step: i32,
    dist: i32,
    y_scale: f64,
    skip: Skip<'_>,
) {
    let mut r = LegacyRandom::new(seed);
    let split = r.next_int_bounded(dist / 2) + dist / 4;
    let steep = r.next_int_bounded(6) == 0;
    let mut y_rota = 0.0f32;
    let mut x_rota = 0.0f32;
    for p in step..dist {
        let hr = 1.5 + f64::from(sin_t(f64::from(PI_F * p as f32 / dist as f32)) * th);
        let vr = hr * y_scale;
        let cos_x = cos_t(f64::from(rot_v));
        x += f64::from(cos_t(f64::from(rot_h)) * cos_x);
        y += f64::from(sin_t(f64::from(rot_v)));
        z += f64::from(sin_t(f64::from(rot_h)) * cos_x);
        rot_v *= if steep { 0.92 } else { 0.7 };
        rot_v += x_rota * 0.1;
        rot_h += y_rota * 0.1;
        x_rota *= 0.9;
        y_rota *= 0.75;
        x_rota += (r.next_float() - r.next_float()) * r.next_float() * 2.0;
        y_rota += (r.next_float() - r.next_float()) * r.next_float() * 4.0;
        if p == split && th > 1.0 {
            for sign in [-1.0f32, 1.0] {
                let s = r.next_long();
                let bt = r.next_float() * 0.5 + 0.5;
                tunnel(t, s, x, y, z, hm, vm, bt, rot_h + sign * (PI_F / 2.0), rot_v / 3.0, p, dist, 1.0, skip);
            }
            return;
        }
        if r.next_int_bounded(4) != 0 {
            if !t.can_reach(x, z, p, dist, th) {
                return;
            }
            t.ellipsoid(x, y, z, hr * hm, vr * vm, skip);
        }
    }
}

impl CanyonDef {
    fn carve(&self, gc: &GenContext, r: &mut LegacyRandom, src_x: i32, src_z: i32, t: &mut impl Sink) {
        let max_distance = (4 * 2 - 1) * 16;
        let x = f64::from(src_x * 16 + r.next_int_bounded(16));
        let y = self.y.sample(r);
        let z = f64::from(src_z * 16 + r.next_int_bounded(16));
        let rot_h = r.next_float() * (PI_F * 2.0);
        let rot_v = self.vertical_rotation.sample(r);
        let y_scale = f64::from(self.y_scale.sample(r));
        let th = self.thickness.sample(r);
        let distance = (max_distance as f32 * self.distance_factor.sample(r)) as i32;
        let seed = r.next_long();
        let widths = t.take_widths(gc.depth.max(0) as usize);
        t.begin();
        let widths = self.do_carve(gc, t, widths, seed, x, f64::from(y), z, th, rot_h, rot_v, distance, y_scale);
        t.end();
        t.give_widths(widths);
    }

    #[allow(clippy::too_many_arguments)]
    fn do_carve(
        &self,
        gc: &GenContext,
        t: &mut impl Sink,
        mut widths: Vec<f32>,
        seed: i64,
        mut x: f64,
        mut y: f64,
        mut z: f64,
        th: f32,
        mut rot_h: f32,
        mut rot_v: f32,
        distance: i32,
        y_scale: f64,
    ) -> Vec<f32> {
        let mut r = LegacyRandom::new(seed);
        let mut w = 1.0f32;
        for (i, v) in widths.iter_mut().enumerate() {
            if i == 0 || r.next_int_bounded(self.width_smoothness) == 0 {
                w = 1.0 + r.next_float() * r.next_float();
            }
            *v = w * w;
        }
        let skip = Skip::Canyon { widths: &widths, min_gen_y: gc.min_y };
        let mut y_rota = 0.0f32;
        let mut x_rota = 0.0f32;
        for p in 0..distance {
            let mut hr = 1.5 + f64::from(sin_t(f64::from(p as f32 * PI_F / distance as f32)) * th);
            let mut vr = hr * y_scale;
            hr *= f64::from(self.horizontal_radius_factor.sample(&mut r));
            let m = 1.0 - (0.5 - p as f32 / distance as f32).abs() * 2.0;
            let factor = self.vertical_default + self.vertical_center * m;
            vr = f64::from(factor) * vr * f64::from(r.next_float() * (1.0 - 0.75) + 0.75);
            let xc = cos_t(f64::from(rot_v));
            let xs = sin_t(f64::from(rot_v));
            x += f64::from(cos_t(f64::from(rot_h)) * xc);
            y += f64::from(xs);
            z += f64::from(sin_t(f64::from(rot_h)) * xc);
            rot_v *= 0.7;
            rot_v += x_rota * 0.05;
            rot_h += y_rota * 0.05;
            x_rota *= 0.8;
            y_rota *= 0.5;
            x_rota += (r.next_float() - r.next_float()) * r.next_float() * 2.0;
            y_rota += (r.next_float() - r.next_float()) * r.next_float() * 4.0;
            if r.next_int_bounded(4) != 0 {
                if !t.can_reach(x, z, p, distance, th) {
                    break;
                }
                t.ellipsoid(x, y, z, hr, vr, skip);
            }
        }
        widths
    }
}

/// "Large structure" randomness of a source chunk for the cave of rank `k`.
pub fn large_feature_random(seed: i64, k: i64, sx: i32, sz: i32) -> LegacyRandom {
    let s = seed.wrapping_add(k);
    let mut r = LegacyRandom::new(s);
    let a = r.next_long();
    let b = r.next_long();
    r.set_seed(i64::from(sx).wrapping_mul(a) ^ i64::from(sz).wrapping_mul(b) ^ s);
    r
}

impl Carver {
    /// Starting draw then carving into the mask of the target chunk.
    pub fn run(&self, gc: &GenContext, r: &mut LegacyRandom, src_x: i32, src_z: i32, t: &mut Target) {
        self.run_into(gc, r, src_x, src_z, t);
    }

    fn run_into(&self, gc: &GenContext, r: &mut LegacyRandom, src_x: i32, src_z: i32, t: &mut impl Sink) {
        if r.next_float() > self.probability {
            return;
        }
        match &self.kind {
            CarverKind::Cave(c) => c.carve(r, src_x, src_z, t),
            CarverKind::Canyon(c) => c.carve(gc, r, src_x, src_z, t),
        }
    }

    /// Complete carving of a source, independent of the target chunk (all the range tests
    /// assumed to succeed), to replay it afterwards on each target ([`Record::replay`]).
    pub fn record(&self, gc: &GenContext, r: &mut LegacyRandom, src_x: i32, src_z: i32) -> Record {
        let mut rec = Record::default();
        self.run_into(gc, r, src_x, src_z, &mut rec);
        rec.events.shrink_to_fit();
        rec
    }
}

// ---------------------------------------------------------------- replay

/// Event of a recorded carving.
#[derive(Debug, Clone, Copy)]
enum Ev {
    /// Start of a tunnel; `end` = index of its `End`.
    Begin {
        end: u32,
    },
    End,
    /// Range test: on failure, the current tunnel stops (jump to its `End`).
    Check {
        x: f64,
        z: f64,
        step: i32,
        total: i32,
        th: f32,
    },
    /// Ellipsoid; `skip` = index into the carving's exclusion tests.
    Ell {
        x: f64,
        y: f64,
        z: f64,
        hr: f64,
        vr: f64,
        skip: u32,
    },
}

#[derive(Debug, Clone)]
enum SkipRec {
    Cave(f64),
    Canyon(Vec<f32>, i32),
}

/// Recorded carving of a source (a cave of a given rank).
#[derive(Debug, Clone, Default)]
pub struct Record {
    events: Vec<Ev>,
    skips: Vec<SkipRec>,
    open: Vec<usize>,
    last_canyon: Option<(usize, u32)>,
}

impl Record {
    /// Bytes occupied (estimate, for the cache ceiling).
    pub fn bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + self.events.capacity() * std::mem::size_of::<Ev>()
            + self.skips.iter().map(|s| 16 + if let SkipRec::Canyon(w, _) = s { w.len() * 4 } else { 0 }).sum::<usize>()
    }

    /// Replays the carving on a target chunk: same range tests, same stops, same
    /// ellipsoids as the direct carving.
    pub fn replay(&self, t: &mut Target) {
        let mut ends = std::mem::take(&mut t.ends);
        ends.clear();
        let mut i = 0usize;
        while i < self.events.len() {
            match self.events[i] {
                Ev::Begin { end } => ends.push(end),
                Ev::End => {
                    ends.pop();
                }
                Ev::Check { x, z, step, total, th } => {
                    if !t.can_reach(x, z, step, total, th) {
                        i = *ends.last().expect("test outside tunnel") as usize;
                        continue;
                    }
                }
                Ev::Ell { x, y, z, hr, vr, skip } => {
                    let s = match &self.skips[skip as usize] {
                        SkipRec::Cave(floor) => Skip::Cave { floor: *floor },
                        SkipRec::Canyon(w, min_gen_y) => Skip::Canyon { widths: w, min_gen_y: *min_gen_y },
                    };
                    t.ellipsoid(x, y, z, hr, vr, s);
                }
            }
            i += 1;
        }
        t.ends = ends;
    }
}

impl Sink for Record {
    fn begin(&mut self) {
        self.open.push(self.events.len());
        self.events.push(Ev::Begin { end: 0 });
    }

    fn end(&mut self) {
        let at = self.open.pop().expect("tunnel end without start");
        let end = self.events.len() as u32;
        self.events[at] = Ev::Begin { end };
        self.events.push(Ev::End);
        if self.open.is_empty() {
            self.last_canyon = None;
        }
    }

    fn can_reach(&mut self, x: f64, z: f64, step: i32, total: i32, th: f32) -> bool {
        self.events.push(Ev::Check { x, z, step, total, th });
        true
    }

    fn ellipsoid(&mut self, x: f64, y: f64, z: f64, hr: f64, vr: f64, skip: Skip<'_>) {
        let id = match skip {
            Skip::Cave { floor } => match self.skips.last() {
                Some(SkipRec::Cave(f)) if f.to_bits() == floor.to_bits() => self.skips.len() as u32 - 1,
                _ => {
                    self.skips.push(SkipRec::Cave(floor));
                    self.skips.len() as u32 - 1
                }
            },
            Skip::Canyon { widths, min_gen_y } => match self.last_canyon {
                Some((p, id)) if p == widths.as_ptr() as usize => id,
                _ => {
                    self.skips.push(SkipRec::Canyon(widths.to_vec(), min_gen_y));
                    let id = self.skips.len() as u32 - 1;
                    self.last_canyon = Some((widths.as_ptr() as usize, id));
                    id
                }
            },
        };
        self.events.push(Ev::Ell { x, y, z, hr, vr, skip: id });
    }

    fn take_widths(&mut self, n: usize) -> Vec<f32> {
        vec![0f32; n]
    }

    fn give_widths(&mut self, _w: Vec<f32>) {}
}

/// Table of a piece of the replay cache and its bytes.
type ReplayShard = (hashbrown::HashMap<(u16, u16, i32, i32), std::sync::Arc<Record>>, usize);

/// Shared cache of the recorded carvings of a generator, key (cave, rank, source x, source z).
/// 16 tables under lock; a table that exceeds `cap / 16` bytes is emptied.
#[derive(Debug)]
pub struct ReplayCache {
    shards: [std::sync::Mutex<ReplayShard>; 16],
    cap: usize,
}

impl Default for ReplayCache {
    fn default() -> Self {
        ReplayCache { shards: Default::default(), cap: 16 << 20 }
    }
}

impl ReplayCache {
    /// Recorded carving of the source (computed as needed).
    pub fn get(&self, key: (u16, u16, i32, i32), make: impl FnOnce() -> Record) -> std::sync::Arc<Record> {
        let h = (key.2.wrapping_mul(31).wrapping_add(key.3) & 15) as usize;
        let shard = &self.shards[h];
        if let Some(r) = shard.lock().unwrap_or_else(std::sync::PoisonError::into_inner).0.get(&key) {
            return r.clone();
        }
        let rec = std::sync::Arc::new(make());
        let mut g = shard.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if g.1 + rec.bytes() > self.cap / 16 {
            g.0.clear();
            g.1 = 0;
        }
        g.1 += rec.bytes();
        g.0.insert(key, rec.clone());
        rec
    }

    /// Bytes occupied (estimate) and number of carvings kept.
    pub fn usage(&self) -> (usize, usize) {
        self.shards.iter().fold((0, 0), |(b, n), s| {
            let g = s.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            (b + g.1, n + g.0.len())
        })
    }
}
