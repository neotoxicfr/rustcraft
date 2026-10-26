//! Visual environment: environment attributes (dimension, biome, timelines, weather),
//! world clocks, camera probe and fog. Rules: `docs/specs/environment.md`.
//! Computations in `f32` like the game (colours in raw sRGB, no linearisation).

pub use rc_data::{AmbientMood, AmbientSounds, AudioAttrs, BackgroundMusic, MusicTrack};
use rc_data::{Ease, EnvEntry, EnvOp, Timeline, VersionData};

/// Attribute value: colour (r, g, b, a), or float in [0].
pub type V = [f32; 4];

/// Optimisation switches of the crate (read once; `=0` restores the old path).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnvOpts {
    /// `RUSTCRAFT_OPT_ENV_NOALLOC`: evaluation without allocation (clock indices kept,
    /// track sampling without a segment array).
    pub noalloc: bool,
    /// `RUSTCRAFT_OPT_ENV_KEYCACHE`: global value kept per (time, clocks, weather).
    pub keycache: bool,
    /// `RUSTCRAFT_OPT_FOG_MEMO`: sky/fog blend kept, sun sign without a sine, angle
    /// without `fmod` when the offset is under one turn.
    pub fog_memo: bool,
}

impl EnvOpts {
    pub const OLD: EnvOpts = EnvOpts { noalloc: false, keycache: false, fog_memo: false };
    pub const NEW: EnvOpts = EnvOpts { noalloc: true, keycache: true, fog_memo: true };

    pub fn from_env() -> EnvOpts {
        static O: std::sync::OnceLock<EnvOpts> = std::sync::OnceLock::new();
        *O.get_or_init(|| EnvOpts {
            noalloc: rc_data::env_str!("RUSTCRAFT_OPT_ENV_NOALLOC") != Some("0"),
            keycache: rc_data::env_str!("RUSTCRAFT_OPT_ENV_KEYCACHE") != Some("0"),
            fog_memo: rc_data::env_str!("RUSTCRAFT_OPT_FOG_MEMO") != Some("0"),
        })
    }
}

impl Default for EnvOpts {
    fn default() -> Self {
        EnvOpts::from_env()
    }
}

/// Unique stamp in the process (never 0): marks a clock state with no risk of
/// confusion between two objects, even replaced or cloned.
fn stamp() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Rgb,
    Argb,
    Float,
    Angle,
    /// Non-interpolated value (moon phase): stepwise everywhere.
    Step,
}

/// Attributes tracked by the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attr {
    FogColor,
    FogStart,
    FogEnd,
    SkyFogEnd,
    CloudFogEnd,
    WaterFogColor,
    WaterFogStart,
    WaterFogEnd,
    SkyColor,
    SunriseSunsetColor,
    SunAngle,
    SkyLightColor,
    SkyLightFactor,
    BlockLightTint,
    AmbientLightColor,
    NightVisionColor,
    MoonAngle,
    StarAngle,
    StarBrightness,
    MoonPhase,
    CloudColor,
    CloudHeight,
    SkyLightLevel,
    /// `audio/music_volume`: target volume of the music, not interpolated in space.
    MusicVolume,
    /// `audio/firefly_bush_sounds`: boolean (0/1), day timeline.
    FireflyBushSounds,
}

pub const ATTRS: usize = 25;

impl Attr {
    pub const ALL: [Attr; ATTRS] = [
        Attr::FogColor,
        Attr::FogStart,
        Attr::FogEnd,
        Attr::SkyFogEnd,
        Attr::CloudFogEnd,
        Attr::WaterFogColor,
        Attr::WaterFogStart,
        Attr::WaterFogEnd,
        Attr::SkyColor,
        Attr::SunriseSunsetColor,
        Attr::SunAngle,
        Attr::SkyLightColor,
        Attr::SkyLightFactor,
        Attr::BlockLightTint,
        Attr::AmbientLightColor,
        Attr::NightVisionColor,
        Attr::MoonAngle,
        Attr::StarAngle,
        Attr::StarBrightness,
        Attr::MoonPhase,
        Attr::CloudColor,
        Attr::CloudHeight,
        Attr::SkyLightLevel,
        Attr::MusicVolume,
        Attr::FireflyBushSounds,
    ];

    fn def(self) -> &'static Def {
        &DEFS[self as usize]
    }

    pub fn from_name(name: &str) -> Option<Attr> {
        Attr::ALL.into_iter().find(|a| a.def().name == name)
    }
}

struct Def {
    name: &'static str,
    kind: Kind,
    default: V,
    min: f32,
    max: f32,
}

const fn rgb(c: u32) -> V {
    [((c >> 16) & 0xff) as f32 / 255.0, ((c >> 8) & 0xff) as f32 / 255.0, (c & 0xff) as f32 / 255.0, 1.0]
}

const fn float(v: f32) -> V {
    [v, 0.0, 0.0, 0.0]
}

const DEFS: [Def; ATTRS] = [
    Def { name: "visual/fog_color", kind: Kind::Rgb, default: rgb(0), min: f32::MIN, max: f32::MAX },
    Def { name: "visual/fog_start_distance", kind: Kind::Float, default: float(0.0), min: f32::MIN, max: f32::MAX },
    Def { name: "visual/fog_end_distance", kind: Kind::Float, default: float(1024.0), min: 0.0, max: f32::MAX },
    Def { name: "visual/sky_fog_end_distance", kind: Kind::Float, default: float(512.0), min: 0.0, max: f32::MAX },
    Def { name: "visual/cloud_fog_end_distance", kind: Kind::Float, default: float(2048.0), min: 0.0, max: f32::MAX },
    Def { name: "visual/water_fog_color", kind: Kind::Rgb, default: rgb(0x050533), min: f32::MIN, max: f32::MAX },
    Def {
        name: "visual/water_fog_start_distance",
        kind: Kind::Float,
        default: float(-8.0),
        min: f32::MIN,
        max: f32::MAX,
    },
    Def { name: "visual/water_fog_end_distance", kind: Kind::Float, default: float(96.0), min: 0.0, max: f32::MAX },
    Def { name: "visual/sky_color", kind: Kind::Rgb, default: rgb(0), min: f32::MIN, max: f32::MAX },
    Def { name: "visual/sunrise_sunset_color", kind: Kind::Argb, default: [0.0; 4], min: f32::MIN, max: f32::MAX },
    Def { name: "visual/sun_angle", kind: Kind::Angle, default: float(0.0), min: f32::MIN, max: f32::MAX },
    Def { name: "visual/sky_light_color", kind: Kind::Rgb, default: rgb(0xffffff), min: f32::MIN, max: f32::MAX },
    Def { name: "visual/sky_light_factor", kind: Kind::Float, default: float(1.0), min: 0.0, max: 1.0 },
    Def { name: "visual/block_light_tint", kind: Kind::Rgb, default: rgb(0xffd88c), min: f32::MIN, max: f32::MAX },
    Def { name: "visual/ambient_light_color", kind: Kind::Rgb, default: rgb(0), min: f32::MIN, max: f32::MAX },
    Def { name: "visual/night_vision_color", kind: Kind::Rgb, default: rgb(0x999999), min: f32::MIN, max: f32::MAX },
    Def { name: "visual/moon_angle", kind: Kind::Angle, default: float(0.0), min: f32::MIN, max: f32::MAX },
    Def { name: "visual/star_angle", kind: Kind::Angle, default: float(0.0), min: f32::MIN, max: f32::MAX },
    Def { name: "visual/star_brightness", kind: Kind::Float, default: float(0.0), min: 0.0, max: 1.0 },
    Def { name: "visual/moon_phase", kind: Kind::Step, default: float(0.0), min: f32::MIN, max: f32::MAX },
    Def { name: "visual/cloud_color", kind: Kind::Argb, default: [0.0; 4], min: f32::MIN, max: f32::MAX },
    Def { name: "visual/cloud_height", kind: Kind::Float, default: float(192.33), min: f32::MIN, max: f32::MAX },
    Def { name: "gameplay/sky_light_level", kind: Kind::Float, default: float(15.0), min: 0.0, max: 15.0 },
    Def { name: "audio/music_volume", kind: Kind::Float, default: float(1.0), min: 0.0, max: 1.0 },
    Def { name: "audio/firefly_bush_sounds", kind: Kind::Step, default: float(0.0), min: 0.0, max: 1.0 },
];

#[inline]
pub fn lerp(t: f32, a: f32, b: f32) -> f32 {
    a + t * (b - a)
}

fn lerp4(t: f32, a: V, b: V) -> V {
    std::array::from_fn(|i| lerp(t, a[i], b[i]))
}

/// Applies a modifier to value `s` (spec § 1.3).
pub fn apply(kind: Kind, op: EnvOp, s: V, a: V) -> V {
    let color = matches!(kind, Kind::Rgb | Kind::Argb);
    match (op, color) {
        (EnvOp::Override, _) => a,
        (EnvOp::Add, false) => float(s[0] + a[0]),
        (EnvOp::Subtract, false) => float(s[0] - a[0]),
        (EnvOp::Multiply, false) => float(s[0] * a[0]),
        (EnvOp::Minimum, false) => float(s[0].min(a[0])),
        (EnvOp::Maximum, false) => float(s[0].max(a[0])),
        (EnvOp::AlphaBlend, false) => float(lerp(a[1], s[0], a[0])),
        // Booleans (0/1).
        (EnvOp::And | EnvOp::Or | EnvOp::Nand | EnvOp::Nor | EnvOp::Xor | EnvOp::Xnor, false) => {
            let (x, y) = (s[0] > 0.5, a[0] > 0.5);
            let r = match op {
                EnvOp::And => x && y,
                EnvOp::Or => x || y,
                EnvOp::Nand => !(x && y),
                EnvOp::Nor => !(x || y),
                EnvOp::Xor => x != y,
                _ => x == y,
            };
            float(f32::from(u8::from(r)))
        }
        (EnvOp::Add, true) => [(s[0] + a[0]).min(1.0), (s[1] + a[1]).min(1.0), (s[2] + a[2]).min(1.0), s[3]],
        (EnvOp::Subtract, true) => [(s[0] - a[0]).max(0.0), (s[1] - a[1]).max(0.0), (s[2] - a[2]).max(0.0), s[3]],
        (EnvOp::Multiply, true) => {
            if kind == Kind::Argb {
                std::array::from_fn(|i| s[i] * a[i])
            } else {
                [s[0] * a[0], s[1] * a[1], s[2] * a[2], s[3]]
            }
        }
        (EnvOp::AlphaBlend, true) => {
            if kind == Kind::Rgb {
                if a[3] == 0.0 {
                    s
                } else {
                    [lerp(a[3], s[0], a[0]), lerp(a[3], s[1], a[1]), lerp(a[3], s[2], a[2]), s[3]]
                }
            } else if a[3] == 1.0 {
                a
            } else if a[3] == 0.0 {
                s
            } else {
                let al = a[3] + s[3] * (1.0 - a[3]);
                let c = |i: usize| (a[i] * a[3] + s[i] * (al - a[3])) / al;
                [c(0), c(1), c(2), al]
            }
        }
        (EnvOp::BlendToGray, true) => {
            let g = (0.3 * s[0] + 0.59 * s[1] + 0.11 * s[2]) * a[0];
            let g = g.clamp(0.0, 1.0);
            [lerp(a[1], s[0], g), lerp(a[1], s[1], g), lerp(a[1], s[2], g), s[3]]
        }
        _ => s,
    }
}

/// Cubic Bézier curve (spec § 2.2): y for a given x.
pub fn bezier(c: [f32; 4], x: f32) -> f32 {
    let coef = |c1: f32, c2: f32| (3.0 * c1 - 3.0 * c2 + 1.0, -6.0 * c1 + 3.0 * c2, 3.0 * c1);
    let (ax, bx, cx) = coef(c[0], c[2]);
    let (ay, by, cy) = coef(c[1], c[3]);
    let f = |a: f32, b: f32, c: f32, t: f32| ((a * t + b) * t + c) * t;
    let mut t = x;
    let mut done = false;
    for _ in 0..4 {
        let e = f(ax, bx, cx, t) - x;
        if e.abs() < 1.0e-5 {
            done = true;
            break;
        }
        let d = (3.0 * ax * t + 2.0 * bx) * t + cx;
        if d < 1.0e-5 {
            break;
        }
        t -= (e / d).clamp(-0.25, 0.25);
    }
    if !done {
        let (mut lo, mut hi) = (0.0f32, 1.0f32);
        for _ in 0..64 {
            let e = f(ax, bx, cx, t) - x;
            if e.abs() < 1.0e-5 {
                break;
            }
            if e < 0.0 {
                lo = t;
            } else {
                hi = t;
            }
            t = (lo + hi) * 0.5;
        }
    }
    f(ay, by, cy, t)
}

/// Compiled timeline track for a tracked attribute.
#[derive(Debug, Clone)]
struct Track {
    clock: usize,
    period: u32,
    step: bool,
    op: EnvOp,
    ease: Ease,
    keys: Vec<(u32, V)>,
}

impl Track {
    /// Like `sample`, without a segment array: the same segments, in the same order, are
    /// walked on the fly (first whose end exceeds `t`, otherwise the last).
    fn sample_scan(&self, ticks: i64) -> V {
        let k = &self.keys;
        if k.len() == 1 {
            return k[0].1;
        }
        let (first, last) = (k[0], k[k.len() - 1]);
        let p = i64::from(self.period);
        let t = if p > 0 { ticks.rem_euclid(p) } else { ticks };
        let seg = (|| {
            if p > 0 {
                let s = (i64::from(last.0) - p, i64::from(first.0), last.1, first.1);
                if t < s.1 {
                    return s;
                }
            }
            for (j, w) in k.windows(2).enumerate() {
                let s = (i64::from(w[0].0), i64::from(w[1].0), w[0].1, w[1].1);
                // Without a period, the last segment is the last pair of keys.
                if t < s.1 || (p == 0 && j == k.len() - 2) {
                    return s;
                }
            }
            (i64::from(last.0), i64::from(first.0) + p, last.1, first.1)
        })();
        self.finish(t, seg)
    }

    fn finish(&self, t: i64, (a, b, va, vb): (i64, i64, V, V)) -> V {
        if t <= a {
            return va;
        }
        if t >= b {
            return vb;
        }
        if self.step {
            return va;
        }
        let alpha = (t - a) as f32 / (b - a) as f32;
        let eased = match self.ease {
            Ease::Linear => alpha,
            Ease::Constant => 0.0,
            Ease::Bezier(c) => bezier(c, alpha),
        };
        lerp4(eased, va, vb)
    }

    /// Argument of the track at the whole tick `ticks` of its clock (spec § 2.2).
    fn sample(&self, ticks: i64) -> V {
        let k = &self.keys;
        if k.len() == 1 {
            return k[0].1;
        }
        // Segments (start, end, start value, end value).
        let mut segs: Vec<(i64, i64, V, V)> = Vec::with_capacity(k.len() + 1);
        let (first, last) = (k[0], k[k.len() - 1]);
        let p = i64::from(self.period);
        if p > 0 {
            segs.push((i64::from(last.0) - p, i64::from(first.0), last.1, first.1));
        }
        for w in k.windows(2) {
            segs.push((i64::from(w[0].0), i64::from(w[1].0), w[0].1, w[1].1));
        }
        if p > 0 {
            segs.push((i64::from(last.0), i64::from(first.0) + p, last.1, first.1));
        }
        let t = if p > 0 { ticks.rem_euclid(p) } else { ticks };
        let &seg = segs.iter().find(|s| t < s.1).unwrap_or(&segs[segs.len() - 1]);
        self.finish(t, seg)
    }
}

/// World clocks on the client side (spec § 2.4).
#[derive(Debug, Clone, Default)]
pub struct Clocks {
    pub game_time: i64,
    last: i64,
    /// (name, total ticks, fraction, speed).
    clocks: Vec<(String, i64, f64, f32)>,
    /// Stamp of the clock state (changed at each modification) and of their layout
    /// (changed at each creation); 0 for a default state (no clock).
    rev: u64,
    layout: u64,
}

impl Clocks {
    fn index(&mut self, name: &str) -> usize {
        match self.clocks.iter().position(|c| c.0 == name) {
            Some(i) => i,
            None => {
                self.clocks.push((name.to_owned(), 0, 0.0, 1.0));
                (self.rev, self.layout) = (stamp(), stamp());
                self.clocks.len() - 1
            }
        }
    }

    fn advance(&mut self) {
        self.rev = stamp();
        let d = self.game_time - self.last;
        self.last = self.game_time;
        for c in &mut self.clocks {
            let x = c.2 + d as f64 * f64::from(c.3);
            let n = x.floor();
            c.2 = x - n;
            c.1 += n as i64;
        }
    }

    /// Client world tick.
    pub fn tick(&mut self) {
        self.game_time += 1;
        self.advance();
    }

    /// Time packet: game time, then listed clock states.
    pub fn set_time(&mut self, game_time: i64, updates: &[(String, i64, f32, f32)]) {
        self.game_time = game_time;
        self.advance();
        for (name, total, partial, rate) in updates {
            let i = self.index(name);
            self.clocks[i] = (name.clone(), *total, f64::from(*partial), *rate);
        }
        self.rev = stamp();
    }

    pub fn total(&self, i: usize) -> i64 {
        self.clocks.get(i).map_or(0, |c| c.1)
    }

    /// Total ticks of a clock by its name (0 if unknown).
    pub fn total_of(&self, name: &str) -> i64 {
        self.clocks.iter().find(|c| c.0 == name).map_or(0, |c| c.1)
    }
}

/// Rain and thunder levels received from the server (spec § 2.5).
#[derive(Debug, Clone, Copy, Default)]
pub struct Weather {
    pub rain: f32,
    /// Value received (the effective level is multiplied by the rain).
    pub thunder_raw: f32,
}

impl Weather {
    /// Game event: 1 and 2 set the rain, 7 and 8 the levels.
    pub fn event(&mut self, kind: u8, param: f32) {
        match kind {
            1 => self.rain = 0.0,
            2 => self.rain = 1.0,
            7 => self.rain = param.clamp(0.0, 1.0),
            8 => self.thunder_raw = param.clamp(0.0, 1.0),
            _ => {}
        }
    }

    pub fn thunder(&self) -> f32 {
        self.thunder_raw * self.rain
    }
}

/// Weather modifiers built into the game (spec § 1.5): (rain, thunder).
fn weather_mods(a: Attr) -> Option<[(EnvOp, V); 2]> {
    let c = |r: f32, g: f32, b: f32| [r, g, b, 1.0];
    let gray = |b: f32, f: f32| (EnvOp::BlendToGray, [b, f, 0.0, 0.0]);
    let b = |x: u8| f32::from(x) / 255.0;
    Some(match a {
        Attr::SkyColor => [gray(0.6, 0.75), gray(0.24, 0.94)],
        Attr::FogColor => [(EnvOp::Multiply, c(b(127), b(127), b(153))), (EnvOp::Multiply, c(b(63), b(63), b(76)))],
        Attr::SunriseSunsetColor => {
            [(EnvOp::Multiply, [1.0, b(127), b(127), b(153)]), (EnvOp::Multiply, [1.0, b(63), b(63), b(76)])]
        }
        Attr::SkyLightColor => {
            [(EnvOp::AlphaBlend, [b(0x7a), b(0x7a), 1.0, b(79)]), (EnvOp::AlphaBlend, [b(0x7a), b(0x7a), 1.0, b(134)])]
        }
        Attr::StarBrightness => [(EnvOp::Override, float(0.0)), (EnvOp::Override, float(0.0))],
        Attr::CloudColor => [gray(0.24, 0.5), gray(0.095, 0.94)],
        Attr::SkyLightFactor => {
            [(EnvOp::AlphaBlend, [0.24, 0.3125, 0.0, 0.0]), (EnvOp::AlphaBlend, [0.24, 0.527_343_75, 0.0, 0.0])]
        }
        Attr::SkyLightLevel => {
            [(EnvOp::AlphaBlend, [4.0, 0.3125, 0.0, 0.0]), (EnvOp::AlphaBlend, [4.0, 0.527_343_75, 0.0, 0.0])]
        }
        _ => return None,
    })
}

/// Environment of a dimension: layers compiled per tracked attribute.
#[derive(Debug, Clone, Default)]
pub struct Env {
    dim: Vec<Vec<(EnvOp, V)>>,
    /// Per biome (network identifier), then per attribute.
    biomes: Vec<Vec<Vec<(EnvOp, V)>>>,
    precipitation: Vec<bool>,
    tracks: Vec<Vec<Track>>,
    clock_names: Vec<String>,
    weather_ok: bool,
    /// Discrete audio attributes: dimension, then per biome (network identifier).
    audio_dim: AudioAttrs,
    audio_biomes: Vec<AudioAttrs>,
    pub clocks: Clocks,
    pub weather: Weather,
    pub opts: EnvOpts,
    /// World clocks of the timelines (indices of `clock_names`), resolved for the clock
    /// layout `clock_layout`.
    clock_ids: Vec<usize>,
    clock_layout: u64,
    /// Global values kept per attribute, with their key.
    global: [Option<(GlobalKey, V)>; ATTRS],
}

/// What a global value depends on: time, clock state, weather (bit for bit).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct GlobalKey {
    game_time: i64,
    clocks: u64,
    rain: u32,
    thunder_raw: u32,
}

fn layer(entries: &[EnvEntry]) -> Vec<Vec<(EnvOp, V)>> {
    let mut out = vec![Vec::new(); ATTRS];
    for e in entries {
        if let Some(a) = Attr::from_name(&e.attr) {
            out[a as usize].push((e.op, e.arg));
        }
    }
    out
}

impl Env {
    /// Layers of dimension `dimension` (type name), biomes in network order.
    pub fn new(d: &VersionData, dimension: &str, biomes: &[String]) -> Env {
        let dt = d.dimension_types.iter().find(|t| t.name == dimension);
        let mut env = Env {
            dim: layer(dt.map_or(&[][..], |t| &t.attributes)),
            biomes: Vec::with_capacity(biomes.len()),
            precipitation: Vec::with_capacity(biomes.len()),
            tracks: vec![Vec::new(); ATTRS],
            weather_ok: dt.is_some_and(|t| t.has_skylight && !t.has_ceiling && t.skybox != 2),
            audio_dim: dt.map(|t| t.audio.clone()).unwrap_or_default(),
            ..Default::default()
        };
        for n in biomes {
            let b = d.biomes.iter().find(|b| &b.name == n);
            env.audio_biomes.push(b.map(|b| b.audio.clone()).unwrap_or_default());
            env.biomes.push(layer(b.map_or(&[][..], |b| &b.attributes)));
            env.precipitation.push(b.is_none_or(|b| b.has_precipitation));
        }
        let names = dt.map_or(&[][..], |t| &t.timelines);
        for tl in names.iter().filter_map(|n| d.timelines.iter().find(|t| &t.name == n)) {
            env.add_timeline(tl);
        }
        env
    }

    fn add_timeline(&mut self, tl: &Timeline) {
        let clock = match self.clock_names.iter().position(|c| c == &tl.clock) {
            Some(i) => i,
            None => {
                self.clock_names.push(tl.clock.clone());
                self.clock_names.len() - 1
            }
        };
        for t in &tl.tracks {
            if let Some(a) = Attr::from_name(&t.attr)
                && !t.keys.is_empty()
            {
                self.tracks[a as usize].push(Track {
                    clock,
                    period: tl.period,
                    step: a.def().kind == Kind::Step,
                    op: t.op,
                    ease: t.ease,
                    keys: t.keys.clone(),
                });
            }
        }
    }

    pub fn has_precipitation(&self, biome: u32) -> bool {
        self.precipitation.get(biome as usize).copied().unwrap_or(true)
    }

    /// Attribute value for biome weights (in order of appearance, spec § 4.2),
    /// then timelines, weather and bounds (spec § 1.5).
    pub fn eval(&mut self, a: Attr, weights: &[(u32, f32)]) -> V {
        let def = a.def();
        let mut v = def.default;
        for &(op, arg) in self.dim.get(a as usize).map_or(&[][..], |l| &l[..]) {
            v = apply(def.kind, op, v, arg);
        }
        let v0 = v;
        let biome = |b: u32| {
            let mut vb = v0;
            if let Some(l) = self.biomes.get(b as usize) {
                for &(op, arg) in &l[a as usize] {
                    vb = apply(def.kind, op, vb, arg);
                }
            }
            vb
        };
        if def.kind == Kind::Step {
            if let Some(&(b, _)) = weights.iter().max_by(|x, y| x.1.total_cmp(&y.1)) {
                v = biome(b);
            }
        } else if let Some((&(b0, w0), rest)) = weights.split_first() {
            v = biome(b0);
            let mut total = w0;
            for &(b, w) in rest {
                total += w;
                v = lerp4(w / total, v, biome(b));
            }
        }
        // Timeline clocks: local indices → world clocks.
        if self.opts.noalloc {
            // All named clocks are created (in order) from the first evaluation,
            // like the old path; afterwards only if their layout changed.
            if self.clock_layout != self.clocks.layout || self.clock_ids.len() != self.clock_names.len() {
                self.clock_ids.clear();
                for n in &self.clock_names {
                    self.clock_ids.push(self.clocks.index(n));
                }
                self.clock_layout = self.clocks.layout;
            }
            for t in self.tracks.get(a as usize).map_or(&[][..], |l| &l[..]) {
                let arg = t.sample_scan(self.clocks.total(self.clock_ids[t.clock]));
                v = apply(def.kind, t.op, v, arg);
            }
        } else {
            let ids: Vec<usize> = self.clock_names.clone().iter().map(|n| self.clocks.index(n)).collect();
            for t in self.tracks.get(a as usize).map_or(&[][..], |l| &l[..]) {
                let arg = t.sample(self.clocks.total(ids[t.clock]));
                v = apply(def.kind, t.op, v, arg);
            }
        }
        if self.weather_ok
            && let Some([rain_mod, thunder_mod]) = weather_mods(a)
        {
            let thunder = self.weather.thunder();
            let rain = self.weather.rain - thunder;
            if rain > 0.0 {
                v = lerp4(rain, v, apply(def.kind, rain_mod.0, v, rain_mod.1));
            }
            if thunder > 0.0 {
                v = lerp4(thunder, v, apply(def.kind, thunder_mod.0, v, thunder_mod.1));
            }
        }
        if def.kind == Kind::Float {
            v[0] = v[0].clamp(def.min, def.max);
        }
        v
    }

    /// Attribute without position (biome layer ignored), for the lightmap outside the probe.
    pub fn eval_global(&mut self, a: Attr) -> V {
        if !self.opts.keycache {
            return self.eval(a, &[]);
        }
        if let Some((k, v)) = self.global[a as usize]
            && k == self.global_key()
        {
            return v;
        }
        let v = self.eval(a, &[]);
        // Key re-read after the evaluation: it may have created clocks.
        self.global[a as usize] = Some((self.global_key(), v));
        v
    }

    fn global_key(&self) -> GlobalKey {
        GlobalKey {
            game_time: self.clocks.game_time,
            clocks: self.clocks.rev,
            rain: self.weather.rain.to_bits(),
            thunder_raw: self.weather.thunder_raw.to_bits(),
        }
    }

    /// Attribute not interpolated in space: layer of the single biome `biome` (biome of the quarter
    /// containing the position, like the game's audio attributes).
    pub fn eval_in_biome(&mut self, a: Attr, biome: u32) -> V {
        self.eval(a, &[(biome, 1.0)])
    }

    /// `audio/ambient_sounds` in `biome`: the biome layer replaces the dimension's.
    pub fn ambient_sounds(&self, biome: u32) -> AmbientSounds {
        self.audio_biomes
            .get(biome as usize)
            .and_then(|b| b.ambient.clone())
            .or_else(|| self.audio_dim.ambient.clone())
            .unwrap_or_default()
    }

    /// `audio/background_music` in `biome`: the biome layer replaces the dimension's.
    pub fn background_music(&self, biome: u32) -> BackgroundMusic {
        self.audio_biomes
            .get(biome as usize)
            .and_then(|b| b.music.clone())
            .or_else(|| self.audio_dim.music.clone())
            .unwrap_or_default()
    }

    /// [`ambient_sounds`](Self::ambient_sounds) borrowed, without a copy (`RUSTCRAFT_OPT_SOUNDIDS`).
    pub fn ambient_sounds_ref(&self, biome: u32) -> &AmbientSounds {
        static NONE: AmbientSounds = AmbientSounds { loop_sound: None, mood: None, additions: Vec::new() };
        self.audio_biomes
            .get(biome as usize)
            .and_then(|b| b.ambient.as_ref())
            .or(self.audio_dim.ambient.as_ref())
            .unwrap_or(&NONE)
    }

    /// [`background_music`](Self::background_music) borrowed, without a copy.
    pub fn background_music_ref(&self, biome: u32) -> &BackgroundMusic {
        static NONE: BackgroundMusic = BackgroundMusic { default: None, creative: None, underwater: None };
        self.audio_biomes.get(biome as usize).and_then(|b| b.music.as_ref()).or(self.audio_dim.music.as_ref()).unwrap_or(&NONE)
    }
}

/// Biome weights around a position (spec § 4.1): 6×6×6 kernel on the quarter grid,
/// accumulated per biome in order of first appearance. `biome_at(qx, qy, qz)`: biome at the quarter.
pub fn biome_weights(pos: [f64; 3], biome_at: impl Fn(i32, i32, i32) -> u32) -> Vec<(u32, f32)> {
    let q = pos.map(|p| p / 4.0 - 0.5);
    let i = q.map(|v| v.floor() as i32);
    let f: [f32; 3] = std::array::from_fn(|a| (q[a] - f64::from(i[a])) as f32);
    const K: [f32; 7] = [0.0, 1.0, 4.0, 6.0, 4.0, 1.0, 0.0];
    let w = |k: usize, f: f32| lerp(f, K[k + 1], K[k]);
    let mut out: Vec<(u32, f32)> = Vec::new();
    for z in 0..6 {
        for x in 0..6 {
            for y in 0..6 {
                let weight = w(x, f[0]) * w(y, f[1]) * w(z, f[2]);
                let b = biome_at(i[0] - 2 + x as i32, i[1] - 2 + y as i32, i[2] - 2 + z as i32);
                match out.iter_mut().find(|e| e.0 == b) {
                    Some(e) => e.1 += weight,
                    None => out.push((b, weight)),
                }
            }
        }
    }
    out
}

/// Smoothing of attributes read by the camera (spec § 4.3): value of the previous tick and of this
/// tick, interpolated by the tick fraction; the new one is computed at the first read.
#[derive(Debug, Clone, Default)]
pub struct Probe {
    old: [Option<V>; ATTRS],
    new: [Option<V>; ATTRS],
    pub weights: Vec<(u32, f32)>,
    /// Sky/fog blend of `fog` kept for its input `sf` (bits).
    sky_mix: Option<(u32, f32)>,
}

impl Probe {
    /// Tick: the current value becomes the old one; biome weights recomputed.
    pub fn tick(&mut self, weights: Vec<(u32, f32)>) {
        for i in 0..ATTRS {
            if let Some(n) = self.new[i].take() {
                self.old[i] = Some(n);
            } else {
                self.old[i] = None;
            }
        }
        self.weights = weights;
    }

    pub fn reset(&mut self) {
        *self = Probe::default();
    }

    pub fn get(&mut self, env: &mut Env, a: Attr, pt: f32) -> V {
        let i = a as usize;
        let n = match self.new[i] {
            Some(n) => n,
            None => {
                let n = env.eval(a, &self.weights);
                self.new[i] = Some(n);
                n
            }
        };
        let o = *self.old[i].get_or_insert(n);
        if a.def().kind == Kind::Step {
            return n;
        }
        if a.def().kind == Kind::Angle {
            let d = n[0] - o[0];
            // Under one turn, the remainder equals the operand bit for bit (−0 included); NaN and ±∞ go
            // through the remainder.
            let d = if env.opts.fog_memo && d.abs() < 360.0 { d } else { d % 360.0 };
            let d = if d >= 180.0 {
                d - 360.0
            } else if d < -180.0 {
                d + 360.0
            } else {
                d
            };
            return if d.abs() >= 90.0 { n } else { float(o[0] + pt * d) };
        }
        lerp4(pt, o, n)
    }
}

/// Fluid (or block) the camera is in (spec § 5.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FogKind {
    Atmospheric,
    Water,
    Lava,
    PowderSnow,
}

/// Fog uniforms (spec § 7): colour, then distances.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Fog {
    pub color: V,
    pub env_start: f32,
    pub env_end: f32,
    pub render_start: f32,
    pub render_end: f32,
    pub sky_end: f32,
    pub cloud_end: f32,
}

/// Underwater vision of the local player (spec § 5.5).
#[derive(Debug, Clone, Copy, Default)]
pub struct WaterVision {
    pub time: i32,
}

impl WaterVision {
    pub fn tick(&mut self, eyes_in_water: bool, spectator: bool) {
        if eyes_in_water {
            self.time = (self.time + if spectator { 10 } else { 1 }).clamp(0, 600);
        } else if self.time > 0 {
            self.time = (self.time - 10).clamp(0, 600);
        }
    }

    pub fn value(&self, eyes_in_water: bool) -> f32 {
        if !eyes_in_water {
            return 0.0;
        }
        let t = self.time as f32;
        if t >= 600.0 {
            return 1.0;
        }
        let a = (t / 100.0).clamp(0.0, 1.0);
        let b = if t < 100.0 { 0.0 } else { ((t - 100.0) / 500.0).clamp(0.0, 1.0) };
        a * 0.6 + b * 0.399_999_98
    }
}

/// Fog inputs of a frame.
pub struct FogInput {
    pub kind: FogKind,
    pub pt: f32,
    /// Effective render distance in chunks.
    pub render_chunks: i32,
    /// Eye height, world bottom, flat world.
    pub eye_y: f64,
    pub min_y: i32,
    pub flat: bool,
    /// x of the unit look vector.
    pub forward_x: f32,
    pub water_vision: f32,
    /// Raw sky light at the camera block (0..15) and biome precipitation at the block.
    pub sky_light: u8,
    pub biome_rain: bool,
    /// Frame duration in ticks (for the rain multiplier).
    pub dt_ticks: f32,
    /// Blindness of the camera entity: remaining duration in ticks (−1 = infinite).
    pub blindness: Option<i32>,
    /// Darkness of the camera entity: frame fade factor (0 to 1).
    pub darkness_blend: Option<f32>,
    /// Darkening of the world by a boss bar (0 to 1, interpolated) and boss fog.
    pub boss_darken: f32,
    pub boss_fog: bool,
    /// Night vision intensity of the camera (0 without the effect or with Darkness):
    /// brightens the colour outside water.
    pub night_vision: f32,
    /// Camera as a spectator, with fire resistance: open view in lava and snow.
    pub spectator: bool,
    pub fire_resistance: bool,
    /// "Cloud range" option in chunks (end of the cloud fog: ×16).
    pub cloud_range: i32,
}

/// `sin(theta) > 0`. With `fast`, decided without a sine far from zeros (margin 0.01 rad, where the
/// computed sine cannot change sign); near a zero or outside ±2π, computed sine.
fn sin_positive(theta: f32, fast: bool) -> bool {
    use std::f32::consts::PI;
    const M: f32 = 0.01;
    if fast {
        let a = theta.abs();
        let pos = theta > 0.0;
        if (M..=PI - M).contains(&a) {
            return pos;
        }
        if (PI + M..=2.0 * PI - M).contains(&a) {
            return !pos;
        }
    }
    theta.sin() > 0.0
}

/// Fog of the frame (spec § 5–6). `rain_fog`: persistent rain multiplier.
pub fn fog(env: &mut Env, probe: &mut Probe, rain_fog: &mut f32, i: &FogInput) -> Fog {
    let pt = i.pt;
    let r = (i.render_chunks * 16) as f32;
    let (rain, thunder) = (env.weather.rain, env.weather.thunder());
    // Base colour.
    let mut c: V = match i.kind {
        FogKind::Lava => rgb(0x991900),
        FogKind::PowderSnow => rgb(0x9fbbcc),
        FogKind::Water => probe.get(env, Attr::WaterFogColor, pt),
        FogKind::Atmospheric => {
            let mut c = probe.get(env, Attr::FogColor, pt);
            if i.render_chunks >= 4 {
                let theta = probe.get(env, Attr::SunAngle, pt)[0] * (std::f32::consts::PI / 180.0);
                let sx = if sin_positive(theta, env.opts.fog_memo) { -1.0 } else { 1.0 };
                let f = i.forward_x * sx;
                if f > 0.0 {
                    let s = probe.get(env, Attr::SunriseSunsetColor, pt);
                    if s[3] > 0.0 {
                        c = lerp4(f * s[3], c, [s[0], s[1], s[2], 1.0]);
                    }
                }
            }
            let mut sky = probe.get(env, Attr::SkyColor, pt);
            if rain > 0.0 {
                let (m, mb) = (1.0 - rain * 0.5, 1.0 - rain * 0.4);
                sky = [(sky[0] * m).clamp(0.0, 1.0), (sky[1] * m).clamp(0.0, 1.0), (sky[2] * mb).clamp(0.0, 1.0), 1.0];
            }
            if thunder > 0.0 {
                let m = 1.0 - thunder * 0.5;
                sky = [(sky[0] * m).clamp(0.0, 1.0), (sky[1] * m).clamp(0.0, 1.0), (sky[2] * m).clamp(0.0, 1.0), 1.0];
            }
            let sf = (probe.get(env, Attr::SkyFogEnd, pt)[0] / 16.0).min(i.render_chunks as f32);
            let sky_mix = |sf: f32| {
                let u = (sf / 32.0).clamp(0.0, 1.0);
                let u = lerp(u, 0.25, 1.0);
                1.0 - u.powf(0.25)
            };
            let mix = if env.opts.fog_memo {
                match probe.sky_mix {
                    Some((k, m)) if k == sf.to_bits() => m,
                    _ => {
                        let m = sky_mix(sf);
                        probe.sky_mix = Some((sf.to_bits(), m));
                        m
                    }
                }
            } else {
                sky_mix(sf)
            };
            lerp4(mix, c, sky)
        }
    };
    // Void darkness.
    let range = if i.flat { 1.0 } else { 32.0 };
    let mut n = ((range + i.min_y as f32 - i.eye_y as f32) / range).clamp(0.0, 1.0);
    // Blindness, otherwise darkness: raise the darkness (the first effect present alone counts).
    if let Some(d) = i.blindness {
        n = if d != -1 && d <= 19 { (d as f32 / 20.0).max(n) } else { 1.0 };
    } else if let Some(b) = i.darkness_blend {
        n = b.max(n);
    }
    if n > 0.0 && !matches!(i.kind, FogKind::Lava | FogKind::PowderSnow) {
        let b = (1.0 - n) * (1.0 - n);
        c = [c[0] * b, c[1] * b, c[2] * b, c[3]];
    }
    // Boss bar: darkened colour (0.7; 0.6; 0.6) before the brightening.
    if i.boss_darken > 0.0 {
        c = [
            lerp(i.boss_darken, c[0], c[0] * 0.7),
            lerp(i.boss_darken, c[1], c[1] * 0.6),
            lerp(i.boss_darken, c[2], c[2] * 0.6),
            c[3],
        ];
    }
    // Brightening: underwater vision in water, night vision elsewhere.
    let e = if i.kind == FogKind::Water { i.water_vision } else { i.night_vision };
    if c[0] != 0.0 && c[1] != 0.0 && c[2] != 0.0 {
        let s = 1.0 / c[0].max(c[1].max(c[2]));
        c = [lerp(e, c[0], c[0] * s), lerp(e, c[1], c[1] * s), lerp(e, c[2], c[2] * s), 1.0];
    }
    c[3] = 1.0;
    // Distances.
    let (mut env_start, mut env_end, mut sky_end, mut cloud_end);
    match i.kind {
        FogKind::Lava => {
            (env_start, env_end) = if i.spectator {
                (-8.0, r * 0.5)
            } else if i.fire_resistance {
                (0.0, 5.0)
            } else {
                (0.25, 1.0)
            };
            (sky_end, cloud_end) = (env_end, env_end);
        }
        FogKind::PowderSnow => {
            (env_start, env_end) = if i.spectator { (-8.0, r * 0.5) } else { (0.0, 2.0) };
            (sky_end, cloud_end) = (env_end, env_end);
        }
        FogKind::Water => {
            env_start = probe.get(env, Attr::WaterFogStart, pt)[0];
            env_end = probe.get(env, Attr::WaterFogEnd, pt)[0] * i.water_vision.max(0.25);
            (sky_end, cloud_end) = (env_end, env_end);
        }
        FogKind::Atmospheric => {
            let sky = ((f32::from(i.sky_light) - 8.0) / 7.0).clamp(0.0, 1.0);
            let target = rain * sky * if i.biome_rain { 1.0 } else { 0.5 };
            *rain_fog += (target - *rain_fog) * i.dt_ticks * 0.2;
            env_start = probe.get(env, Attr::FogStart, pt)[0] + -160.0 * *rain_fog;
            let end0 = probe.get(env, Attr::FogEnd, pt)[0];
            env_end = 96.0f32.min(end0).max(end0 + -256.0 * *rain_fog);
            sky_end = r.min(probe.get(env, Attr::SkyFogEnd, pt)[0]);
            // Cloud range of the option (128 chunks by default: 2,048 blocks).
            cloud_end = ((i.cloud_range * 16) as f32).min(probe.get(env, Attr::CloudFogEnd, pt)[0]);
            // Boss bar with fog (dragon): start 10 at most, end 96 at most for all.
            if i.boss_fog {
                env_start = env_start.min(10.0);
                env_end = env_end.min(96.0);
                (sky_end, cloud_end) = (env_end, env_end);
            }
        }
    }
    // Blindness then darkness tighten the fog (except lava and powder snow, which take priority).
    if !matches!(i.kind, FogKind::Lava | FogKind::PowderSnow) {
        if let Some(d) = i.blindness {
            let dist = if d == -1 { 5.0 } else { lerp((d as f32 / 20.0).min(1.0), r, 5.0) };
            (env_start, env_end) = (dist * 0.25, dist);
            (sky_end, cloud_end) = (dist * 0.8, dist * 0.8);
        } else if let Some(b) = i.darkness_blend {
            let dist = lerp(b, r, 15.0);
            (env_start, env_end) = (dist * 0.75, dist);
            (sky_end, cloud_end) = (dist, dist);
        }
    }
    let span = (r / 10.0).clamp(4.0, 64.0);
    Fog { color: c, env_start, env_end, render_start: r - span, render_end: r, sky_end, cloud_end }
}

/// Lightmap inputs (`docs/specs/lighting.md` § 4.3).
#[derive(Debug, Clone, Copy)]
pub struct LightmapInput {
    pub sky_factor: f32,
    pub sky_color: V,
    pub block_tint: V,
    pub ambient: V,
    pub night_vision_color: V,
    pub night_vision: f32,
    /// 1.4 + flicker.
    pub block_factor: f32,
    /// "Brightness" setting (default 0.5) minus the darkness.
    pub brightness: f32,
    pub boss_darken: f32,
    pub darkness: f32,
}

/// 16 × 16 RGBA8 lightmap: column = block level, row = sky level (§ 4.2).
pub fn lightmap(i: &LightmapInput) -> Vec<u8> {
    let curve = |l: f32| l / (4.0 - 3.0 * l);
    let mut out = Vec::with_capacity(16 * 16 * 4);
    for sky in 0..16 {
        for block in 0..16 {
            let (b, s) = (block as f32 / 15.0, sky as f32 / 15.0);
            let fb = curve(b) * i.block_factor;
            let fc = curve(s) * i.sky_factor;
            let t = 0.9 * (2.0 * b - 1.0) * (2.0 * b - 1.0);
            let mut c = [0.0f32; 3];
            for k in 0..3 {
                let mut v = i.ambient[k].max(i.night_vision_color[k] * i.night_vision);
                v += i.sky_color[k] * fc;
                v += lerp(t, i.block_tint[k], 1.0) * fb;
                let dark = [0.7, 0.6, 0.6][k];
                v = lerp(i.boss_darken, v, v * dark);
                v -= i.darkness;
                c[k] = v.clamp(0.0, 1.0);
            }
            let m = c[0].max(c[1]).max(c[2]);
            for v in &mut c {
                let bright = if m > 0.0 { *v * (1.0 - (1.0 - m).powi(4)) / m } else { 0.0 };
                *v = lerp(i.brightness, *v, bright);
                out.push((*v * 255.0 + 0.5).floor().clamp(0.0, 255.0) as u8);
            }
            out.push(255);
        }
    }
    out
}

/// Flicker of block light (§ 4.3), advanced at each tick.
#[derive(Debug, Clone, Copy)]
pub struct Flicker {
    pub value: f32,
    seed: u64,
}

impl Default for Flicker {
    fn default() -> Self {
        Flicker { value: 0.0, seed: 0x9e37_79b9_7f4a_7c15 }
    }
}

impl Flicker {
    fn next(&mut self) -> f32 {
        self.seed ^= self.seed << 13;
        self.seed ^= self.seed >> 7;
        self.seed ^= self.seed << 17;
        (self.seed >> 40) as f32 / (1u64 << 24) as f32
    }

    pub fn tick(&mut self) {
        let (a, b, c, d) = (self.next(), self.next(), self.next(), self.next());
        self.value += (a - b) * c * d * 0.1;
        self.value *= 0.9;
    }
}

#[cfg(test)]
mod equiv_tests;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modifiers() {
        let s = [0.5, 0.25, 1.0, 1.0];
        assert_eq!(apply(Kind::Rgb, EnvOp::Multiply, s, [0.5, 1.0, 0.0, 1.0]), [0.25, 0.25, 0.0, 1.0]);
        assert_eq!(apply(Kind::Rgb, EnvOp::Add, s, [0.75, 0.0, 0.5, 1.0]), [1.0, 0.25, 1.0, 1.0]);
        assert_eq!(apply(Kind::Float, EnvOp::AlphaBlend, float(10.0), [20.0, 0.25, 0.0, 0.0])[0], 12.5);
        let g = apply(Kind::Rgb, EnvOp::BlendToGray, [1.0, 0.0, 0.0, 1.0], [1.0, 1.0, 0.0, 0.0]);
        assert_eq!(g, [0.3, 0.3, 0.3, 1.0]);
    }

    #[test]
    fn bezier_endpoints_and_symmetry() {
        let c = [0.362, 0.241, 0.638, 0.759];
        assert!(bezier(c, 0.0).abs() < 1e-4);
        assert!((bezier(c, 1.0) - 1.0).abs() < 1e-4);
        assert!((bezier(c, 0.5) - 0.5).abs() < 1e-4);
    }

    #[test]
    fn track_loops_and_holds() {
        let t = Track {
            clock: 0,
            period: 24000,
            step: false,
            op: EnvOp::Multiply,
            ease: Ease::Linear,
            keys: vec![(133, float(1.0)), (11867, float(1.0)), (13670, float(0.0)), (22330, float(0.0))],
        };
        assert_eq!(t.sample(6000)[0], 1.0);
        assert_eq!(t.sample(18000)[0], 0.0);
        // Looping: between 22330 and 133 + 24000, linear rise.
        let mid = (22330 + 24133) / 2;
        assert!((t.sample(mid)[0] - 0.5).abs() < 1e-3);
        assert_eq!(t.sample(mid + 24000 * 3)[0], t.sample(mid)[0]);
        // Sun angle ramp: two keys on the same tick.
        let sun = Track {
            clock: 0,
            period: 24000,
            step: false,
            op: EnvOp::Override,
            ease: Ease::Linear,
            keys: vec![(6000, float(360.0)), (6000, float(0.0))],
        };
        assert_eq!(sun.sample(6000)[0], 0.0);
        assert!((sun.sample(12000)[0] - 90.0).abs() < 1e-3);
        assert!((sun.sample(0)[0] - 270.0).abs() < 1e-3);
    }

    #[test]
    fn clocks_advance_with_rate() {
        let mut c = Clocks::default();
        c.set_time(100, &[("minecraft:overworld".into(), 6000, 0.0, 1.0)]);
        c.tick();
        c.tick();
        assert_eq!(c.total(0), 6002);
        c.set_time(110, &[]);
        assert_eq!(c.total(0), 6010);
        c.set_time(110, &[("minecraft:overworld".into(), 6010, 0.0, 0.0)]);
        c.tick();
        assert_eq!(c.total(0), 6010);
    }

    #[test]
    fn biome_weights_sum_to_4096() {
        let w = biome_weights([10.3, 64.0, -7.9], |x, _, _| u32::from(x > 2));
        let total: f32 = w.iter().map(|e| e.1).sum();
        assert!((total - 4096.0).abs() < 1e-2);
        assert_eq!(w.len(), 2);
    }

    #[test]
    fn real_audio_attributes() {
        let Some(d) = rc_testdata::version_data() else { return };
        let biomes: Vec<String> =
            ["plains", "crimson_forest", "pale_garden", "ocean"].iter().map(|b| format!("minecraft:{b}")).collect();
        let mut env = Env::new(&d, "minecraft:overworld", &biomes);
        // Plains: inherited from the dimension (cave mood, game music, creative).
        let a = env.ambient_sounds(0);
        assert_eq!(a.loop_sound, None);
        let m = a.mood.expect("mood");
        assert_eq!(
            (m.sound.as_str(), m.tick_delay, m.block_search_extent, m.offset),
            ("minecraft:ambient.cave", 6000, 8, 2.0)
        );
        let bm = env.background_music(0);
        assert_eq!(
            bm.default.as_ref().map(|t| (t.sound.as_str(), t.min_delay, t.max_delay)),
            Some(("minecraft:music.game", 12000, 24000))
        );
        assert_eq!(bm.creative.as_ref().map(|t| t.sound.as_str()), Some("minecraft:music.creative"));
        assert!(bm.underwater.is_none());
        // Crimson forest: own loop, addition, mood.
        let c = env.ambient_sounds(1);
        assert_eq!(c.loop_sound.as_deref(), Some("minecraft:ambient.crimson_forest.loop"));
        assert_eq!(c.additions, vec![("minecraft:ambient.crimson_forest.additions".to_owned(), 0.0111)]);
        // Pale garden: empty music, volume 0.
        assert_eq!(env.background_music(2), BackgroundMusic::default());
        assert_eq!(env.eval_in_biome(Attr::MusicVolume, 2)[0], 0.0);
        assert_eq!(env.eval_in_biome(Attr::MusicVolume, 0)[0], 1.0);
        assert!(env.background_music(3).underwater.is_some());
        // End: music that replaces.
        let end = Env::new(&d, "minecraft:the_end", &biomes[..1]);
        let t = end.background_music(0).default.expect("End music");
        assert_eq!((t.sound.as_str(), t.min_delay, t.replace), ("minecraft:music.end", 6000, true));
        // Borrowed reads (`RUSTCRAFT_OPT_SOUNDIDS`): same values as the copies, biome layer,
        // dimension layer and biome outside the list included.
        for dim in ["minecraft:overworld", "minecraft:the_nether", "minecraft:the_end"] {
            let e = Env::new(&d, dim, &biomes);
            for b in 0..6 {
                assert_eq!(*e.ambient_sounds_ref(b), e.ambient_sounds(b), "{dim} biome {b}");
                assert_eq!(*e.background_music_ref(b), e.background_music(b), "{dim} biome {b}");
            }
        }
        assert!(env.ambient_sounds_ref(1).loop_sound.is_some() && env.ambient_sounds_ref(0).loop_sound.is_none());
        // Fireflies: true at night (day timeline, "or" modifier).
        env.clocks.set_time(0, &[("minecraft:overworld".into(), 18000, 0.0, 1.0)]);
        assert_eq!(env.eval_global(Attr::FireflyBushSounds)[0], 1.0);
        env.clocks.set_time(0, &[("minecraft:overworld".into(), 6000, 0.0, 1.0)]);
        assert_eq!(env.eval_global(Attr::FireflyBushSounds)[0], 0.0);
    }

    #[test]
    fn blindness_and_darkness_tighten_the_fog() {
        let Some(d) = rc_testdata::version_data() else { return };
        let biomes = vec!["minecraft:plains".to_owned()];
        let mut env = Env::new(&d, "minecraft:overworld", &biomes);
        env.clocks.set_time(0, &[("minecraft:overworld".into(), 6000, 0.0, 1.0)]);
        let mut probe = Probe::default();
        let mut rain = 0.0;
        let input = |blindness, darkness_blend| FogInput {
            kind: FogKind::Atmospheric,
            pt: 0.0,
            render_chunks: 12,
            eye_y: 64.0,
            min_y: -64,
            flat: false,
            forward_x: 0.0,
            water_vision: 0.0,
            sky_light: 15,
            biome_rain: false,
            dt_ticks: 1.0,
            blindness,
            darkness_blend,
            boss_darken: 0.0,
            boss_fog: false,
            night_vision: 0.0,
            spectator: false,
            fire_resistance: false,
            cloud_range: 128,
        };
        let clear = fog(&mut env, &mut probe, &mut rain, &input(None, None));
        // Infinite blindness: 5-block fog, black colour.
        let blind = fog(&mut env, &mut probe, &mut rain, &input(Some(-1), None));
        assert_eq!((blind.env_start, blind.env_end), (1.25, 5.0));
        assert_eq!((blind.sky_end, blind.cloud_end), (4.0, 4.0));
        assert_eq!(&blind.color[..3], &[0.0, 0.0, 0.0]);
        // Last second: gradual return to the normal range.
        let fading = fog(&mut env, &mut probe, &mut rain, &input(Some(10), None));
        assert!(fading.env_end > blind.env_end && fading.env_end < 192.0);
        // Darkness: from render distance to 15 blocks depending on the fade.
        let dark = fog(&mut env, &mut probe, &mut rain, &input(None, Some(1.0)));
        assert_eq!((dark.env_start, dark.env_end), (11.25, 15.0));
        assert!(clear.env_end > dark.env_end);
        let half = fog(&mut env, &mut probe, &mut rain, &input(None, Some(0.5)));
        assert!(half.env_end > dark.env_end && half.color[2] < clear.color[2]);
    }

    #[test]
    fn boss_bar_darkens_and_closes_the_fog() {
        let Some(d) = rc_testdata::version_data() else { return };
        let biomes = vec!["minecraft:plains".to_owned()];
        let mut env = Env::new(&d, "minecraft:overworld", &biomes);
        env.clocks.set_time(0, &[("minecraft:overworld".into(), 6000, 0.0, 1.0)]);
        let mut probe = Probe::default();
        let mut rain = 0.0;
        let mut input = FogInput {
            kind: FogKind::Atmospheric,
            pt: 0.0,
            render_chunks: 12,
            eye_y: 64.0,
            min_y: -64,
            flat: false,
            forward_x: 0.0,
            water_vision: 0.0,
            sky_light: 15,
            biome_rain: false,
            dt_ticks: 1.0,
            blindness: None,
            darkness_blend: None,
            boss_darken: 0.0,
            boss_fog: false,
            night_vision: 0.0,
            spectator: false,
            fire_resistance: false,
            cloud_range: 128,
        };
        let clear = fog(&mut env, &mut probe, &mut rain, &input);
        // Full darkening: channels × (0.7; 0.6; 0.6), distances unchanged.
        input.boss_darken = 1.0;
        let dark = fog(&mut env, &mut probe, &mut rain, &input);
        for (k, f) in [0.7, 0.6, 0.6].into_iter().enumerate() {
            assert!((dark.color[k] - clear.color[k] * f).abs() < 1e-6);
        }
        assert_eq!(dark.env_end, clear.env_end);
        // Boss bar fog: start 10 at most, end 96 at most, sky and clouds at the same end.
        input.boss_darken = 0.0;
        input.boss_fog = true;
        let fogged = fog(&mut env, &mut probe, &mut rain, &input);
        assert!(fogged.env_start <= 10.0 && fogged.env_end <= 96.0);
        assert_eq!((fogged.sky_end, fogged.cloud_end), (fogged.env_end, fogged.env_end));
        assert_eq!(fogged.color, clear.color);
        // Cloud range: 128 chunks → 2,048 blocks (attribute bound), 32 → 512.
        input.boss_fog = false;
        assert_eq!(clear.cloud_end, 2048.0);
        input.cloud_range = 32;
        assert_eq!(fog(&mut env, &mut probe, &mut rain, &input).cloud_end, 512.0);
        input.cloud_range = 128;
        // Night vision: colour brought to a maximum of 1 outside water.
        input.night_vision = 1.0;
        let bright = fog(&mut env, &mut probe, &mut rain, &input);
        assert!((bright.color[0].max(bright.color[1]).max(bright.color[2]) - 1.0).abs() < 1e-6);
        input.night_vision = 0.0;
        // Lava: 0.25..1; fire resistance 0..5; spectator −8..half the render distance.
        input.kind = FogKind::Lava;
        let lava = fog(&mut env, &mut probe, &mut rain, &input);
        assert_eq!((lava.env_start, lava.env_end), (0.25, 1.0));
        input.fire_resistance = true;
        let resist = fog(&mut env, &mut probe, &mut rain, &input);
        assert_eq!((resist.env_start, resist.env_end, resist.cloud_end), (0.0, 5.0, 5.0));
        input.spectator = true;
        let spect = fog(&mut env, &mut probe, &mut rain, &input);
        assert_eq!((spect.env_start, spect.env_end), (-8.0, 96.0));
        // Powder snow: 0..2, spectator like lava.
        input.kind = FogKind::PowderSnow;
        assert_eq!(fog(&mut env, &mut probe, &mut rain, &input).env_end, 96.0);
        input.spectator = false;
        assert_eq!(fog(&mut env, &mut probe, &mut rain, &input).env_end, 2.0);
    }

    #[test]
    fn real_overworld_noon_and_midnight() {
        let Some(d) = rc_testdata::version_data() else { return };
        let biomes = vec!["minecraft:plains".to_owned()];
        let mut env = Env::new(&d, "minecraft:overworld", &biomes);
        env.clocks.set_time(0, &[("minecraft:overworld".into(), 6000, 0.0, 1.0)]);
        let w = [(0u32, 4096.0f32)];
        let sky = env.eval(Attr::SkyColor, &w);
        assert_eq!(sky, rgb(0x78a7ff));
        assert_eq!(env.eval(Attr::FogColor, &w), rgb(0xc0d8ff));
        env.clocks.set_time(0, &[("minecraft:overworld".into(), 18000, 0.0, 1.0)]);
        assert_eq!(env.eval(Attr::SkyColor, &w), [0.0, 0.0, 0.0, 1.0]);
        // Rain: sky grey.
        env.weather.event(7, 1.0);
        env.clocks.set_time(0, &[("minecraft:overworld".into(), 6000, 0.0, 1.0)]);
        let rainy = env.eval(Attr::SkyColor, &w);
        assert!(rainy[2] < sky[2]);
    }
}

#[cfg(test)]
#[test]
fn lightmap_matches_spec_examples() {
    let i = LightmapInput {
        sky_factor: 1.0,
        sky_color: rgb(0xffffff),
        block_tint: rgb(0xffd88c),
        ambient: rgb(0x0a0a0a),
        night_vision_color: rgb(0x999999),
        night_vision: 0.0,
        block_factor: 1.4,
        brightness: 0.5,
        boss_darken: 0.0,
        darkness: 0.0,
    };
    let m = lightmap(&i);
    // Sky 15, block 0: white; sky 0, block 0: about 0.093.
    assert_eq!(&m[(15 * 16) * 4..(15 * 16) * 4 + 3], &[255, 255, 255]);
    assert_eq!(m[0], (0.093f32 * 255.0).round() as u8);
}
