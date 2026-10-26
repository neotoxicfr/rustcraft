//! Visual environment on the app side: state (`rc_env`), world reads it needs
//! (biomes around the camera, fluid in the camera, sky light) and frame fog.
//! Rules: `docs/specs/environment.md`.

use rc_env::{Attr, Env, FogInput, FogKind, Probe, WaterVision};
use rc_physics::Fluid;
use rc_world::ChunkGrid;

use crate::render::FrameEnv;

/// Hashed seed of the current world (login packet), for the biome blur zoom. The official
/// server sends the same one for all the dimensions of a world; respawning keeps it.
static BIOME_ZOOM_SEED: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

pub fn set_biome_zoom_seed(seed: i64) {
    BIOME_ZOOM_SEED.store(seed, std::sync::atomic::Ordering::Relaxed);
}

pub fn biome_zoom_seed() -> i64 {
    BIOME_ZOOM_SEED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Effects of the local player that change the light and the fog.
#[derive(Default, Clone, Copy, Debug, PartialEq)]
pub struct CameraEffects {
    /// Night vision: ticks left (−1: infinite).
    pub night_vision: Option<i32>,
    pub conduit_power: bool,
    pub fire_resistance: bool,
}

impl CameraEffects {
    pub fn read(effects: &[crate::hud_extra::Effect], names: &[String]) -> Self {
        let find = |n: &str| {
            effects.iter().find(|e| usize::try_from(e.id).ok().and_then(|i| names.get(i)).is_some_and(|s| s == n))
        };
        CameraEffects {
            night_vision: find("minecraft:night_vision").map(|e| e.duration),
            conduit_power: find("minecraft:conduit_power").is_some(),
            fire_resistance: find("minecraft:fire_resistance").is_some(),
        }
    }

    /// Night vision intensity: full beyond 200 ticks left (or infinite),
    /// then flicker 0.7 ± 0.3 with a period of 10 ticks.
    pub fn night_vision_scale(&self, pt: f32) -> Option<f32> {
        let d = self.night_vision?;
        Some(if d == -1 || d > 200 {
            1.0
        } else {
            0.7 + crate::sky_mesh::table_sin((d as f32 - pt) * std::f32::consts::PI * 0.2) * 0.3
        })
    }
}

/// Reads of the current world.
pub struct WorldView<'a> {
    pub grid: &'a ChunkGrid,
    pub min_y: i32,
    pub height: i32,
}

impl WorldView<'_> {
    pub fn state(&self, x: i32, y: i32, z: i32) -> u32 {
        let ly = y - self.min_y;
        if !(0..self.height).contains(&ly) {
            return 0;
        }
        self.grid
            .get(x >> 4, z >> 4)
            .and_then(|c| c.block((x & 15) as usize, ly as usize, (z & 15) as usize))
            .unwrap_or(0)
    }

    /// Biome at the quart (qx, qy, qz); column absent: `missing`.
    pub(crate) fn biome(&self, qx: i32, qy: i32, qz: i32, missing: u32) -> u32 {
        let Some(c) = self.grid.get(qx >> 2, qz >> 2) else { return missing };
        let (lo, hi) = (self.min_y >> 2, ((self.min_y + self.height) >> 2) - 1);
        let ly = qy.clamp(lo, hi) - lo;
        let Some(s) = c.sections.get((ly >> 2) as usize) else { return missing };
        s.biomes.get((((ly & 3) as usize) << 4) | (((qz & 3) as usize) << 2) | ((qx & 3) as usize))
    }

    /// Biome at the block (x, y, z) after the game's blur zoom (tints, precipitation, rain
    /// fog, F3 screen); column absent: `missing`.
    pub(crate) fn biome_full(&self, x: i32, y: i32, z: i32, missing: u32) -> u32 {
        let (qx, qy, qz) = rc_mesh::blocks::biome_zoom(biome_zoom_seed(), x, y, z);
        self.biome(qx, qy, qz, missing)
    }

    /// Biome colors of the block (x, y, z) after the blur zoom, without blending (grass, foliage,
    /// dry foliage, water; RGB), swamp grass by (x, z): tint of a moving or falling
    /// block, which reads only one biome. Biome outside the palette: white.
    pub(crate) fn biome_tints(
        &self,
        palette: &rc_mesh::blocks::BiomePalette,
        x: i32,
        y: i32,
        z: i32,
        missing: u32,
    ) -> [u32; 4] {
        let id = self.biome_full(x, y, z, missing);
        let mut c = palette.get(id as usize).copied().unwrap_or([0xff_ffff; 4]);
        if c[0] == rc_mesh::blocks::SWAMP_GRASS {
            c[0] = rc_mesh::blocks::swamp_grass(x, z);
        }
        c
    }

    /// Raw sky light at the block (0 without data).
    fn sky_light(&self, x: i32, y: i32, z: i32) -> u8 {
        self.light(true, x, y, z)
    }

    /// Raw light (block, sky) at the block.
    pub fn light_pair(&self, x: i32, y: i32, z: i32) -> (u8, u8) {
        (self.light(false, x, y, z), self.light(true, x, y, z))
    }

    fn light(&self, sky: bool, x: i32, y: i32, z: i32) -> u8 {
        let Some(c) = self.grid.get(x >> 4, z >> 4) else { return 0 };
        let sy = ((y - self.min_y) >> 4) + 1;
        let layers = if sky { &c.sky_light } else { &c.block_light };
        let Ok(mut s) = usize::try_from(sy) else { return 0 };
        let mut y = y;
        if sky {
            // Sky section without data (never stored by the server above the terrain):
            // bottom row of the first section above that has some, 15 if there is none.
            while layers.get(s).is_some_and(Option::is_none) {
                s += 1;
                y = 0;
            }
            if s >= layers.len() {
                return 15;
            }
        }
        let Some(Some(layer)) = layers.get(s) else { return 0 };
        let i = (((y & 15) as usize) << 8) | (((z & 15) as usize) << 4) | ((x & 15) as usize);
        (layer[i >> 1] >> ((i & 1) * 4)) & 15
    }

    /// Maximum raw light at the block: `max(block, sky − darkening)`; above the
    /// world the sky is 15.
    pub fn raw_brightness(&self, x: i32, y: i32, z: i32, has_sky: bool, darken: i32) -> i32 {
        let block = i32::from(self.light(false, x, y, z));
        let sky = match (has_sky, y >= self.min_y + self.height) {
            (false, _) => 0,
            (true, true) => 15,
            (true, false) => i32::from(self.sky_light(x, y, z)),
        };
        block.max(sky - darken)
    }
}

/// Cloud range in chunks (default value of the option, before the settings are read).
pub const CLOUD_RANGE: i32 = 128;

#[derive(Default)]
pub struct EnvState {
    pub env: Env,
    probe: Probe,
    rain_fog: f32,
    pub water_vision: WaterVision,
    flat: bool,
    skybox: u8,
    plains: u32,
    /// Server view distance (chunks), 0 if unknown.
    pub server_view: i32,
    pub game_mode: i32,
    /// Camera of the last render (the biome probe sits there at the tick).
    cam: [f64; 3],
    powder_snow: Option<u16>,
    /// The dimension has sky light.
    pub has_sky: bool,
    ambient_light: f32,
    flicker: rc_env::Flicker,
    /// Lightmap to recompute (once per tick, at the first frame that follows).
    lightmap_dirty: bool,
    /// Combined flags of the boss bars (1 dark sky, 2 music, 4 fog), set at
    /// every tick before `tick`.
    pub boss_flags: u8,
    /// Darkening of the world by the boss bars: (previous, current), +0.05 per tick
    /// with the flag, −0.0125 otherwise.
    boss_dark: (f32, f32),
    /// "Brightness" option (`None`: default 0.5).
    pub gamma: Option<f32>,
    /// Darkness effect (`video.rs`): brightness drop, darkening of the lightmap.
    pub darkness: (f32, f32),
    /// Blindness (ticks left, −1 infinite) and camera darkness fade: tightened fog,
    /// sky not drawn.
    pub eye_effects: (Option<i32>, Option<f32>),
    /// Cloud options: shown, range in chunks, "fast" (flat) (`None`: yes,
    /// `CLOUD_RANGE`, detailed).
    pub clouds: Option<(bool, i32, bool)>,
    /// Default clock of the dimension (End flashes).
    default_clock: String,
    /// End flash: drawn period, offset, duration, angles, previous and current intensities.
    flash: EndFlash,
    /// Thunderstorm flash in progress (lightning counter > 0, "hide lightning" option off).
    pub sky_flash: bool,
    /// "Hide lightning" option: the End flash no longer lights the world (the sky
    /// still draws it, and its sound is always played).
    pub hide_flashes: bool,
    /// Frozen world (`/tick freeze`): the world clocks no longer advance.
    pub frozen: bool,
    /// Effects of the local player (night vision, conduit, fire resistance), set every frame.
    pub effects: CameraEffects,
    /// Eyes in water at the last tick (conduit power in the lightmap).
    eyes_in_water: bool,
    /// Reads of the world from `frame` kept from one frame to the next.
    cache: ViewCache,
}

/// Caches of `frame` (`RUSTCRAFT_OPT_ENVIRONCACHE`, `=0`: everything re-read on every frame).
struct ViewCache {
    on: bool,
    /// Blur zoom of the eye's block: (block, seed) → retained quart (pure function of both).
    zoom: Option<ZoomMemo>,
    /// 3×3×3 neighborhood of the eye's block with no water, lava or powder snow.
    clear: Option<(ClearKey, bool)>,
}

/// (block, seed, retained quart).
type ZoomMemo = ([i32; 3], i64, (i32, i32, i32));

/// What the neighborhood depends on: block, buffers of the columns that contain it, world bounds,
/// tables and data (addresses), state of the powder snow.
#[derive(Clone, Copy, PartialEq)]
struct ClearKey {
    block: [i32; 3],
    stamps: [u64; 4],
    min_y: i32,
    height: i32,
    tables: usize,
    data: usize,
    powder_snow: Option<u16>,
}

impl Default for ViewCache {
    fn default() -> Self {
        static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let on = *ON.get_or_init(|| rc_data::env_str!("RUSTCRAFT_OPT_ENVIRONCACHE") != Some("0"));
        ViewCache { on, zoom: None, clear: None }
    }
}

impl ViewCache {
    /// Quart of the blur zoom at block `b` (kept as long as the block and the seed do not change).
    fn zoom(&mut self, seed: i64, b: [i32; 3]) -> (i32, i32, i32) {
        match self.zoom {
            Some((kb, ks, q)) if kb == b && ks == seed => q,
            _ => {
                let q = rc_mesh::blocks::biome_zoom(seed, b[0], b[1], b[2]);
                self.zoom = Some((b, seed, q));
                q
            }
        }
    }

    /// True if the 27 blocks around `b` have no water, lava or powder snow: the camera
    /// is then necessarily in the air (the five points of the near plane stay within one block
    /// of the eye, checked by `reach`).
    fn clear_around(
        &mut self,
        w: &WorldView<'_>,
        t: &rc_physics::Tables,
        d: &rc_data::VersionData,
        powder_snow: Option<u16>,
        b: [i32; 3],
    ) -> bool {
        let col = |x: i32, z: i32| w.grid.stamp(x >> 4, z >> 4);
        let key = ClearKey {
            block: b,
            stamps: [col(b[0] - 1, b[2] - 1), col(b[0] + 1, b[2] - 1), col(b[0] - 1, b[2] + 1), col(b[0] + 1, b[2] + 1)],
            min_y: w.min_y,
            height: w.height,
            tables: std::ptr::from_ref(t) as usize,
            data: std::ptr::from_ref(d) as usize,
            powder_snow,
        };
        if let Some((k, clear)) = self.clear
            && k == key
        {
            return clear;
        }
        let mut clear = true;
        'scan: for y in b[1] - 1..=b[1] + 1 {
            for z in b[2] - 1..=b[2] + 1 {
                for x in b[0] - 1..=b[0] + 1 {
                    let s = w.state(x, y, z);
                    if matches!(t.fluids.fluid(s), Fluid::Water | Fluid::Lava)
                        || (powder_snow.is_some() && d.states.get(s as usize).map(|st| st.block) == powder_snow)
                    {
                        clear = false;
                        break 'scan;
                    }
                }
            }
        }
        self.clear = Some((key, clear));
        clear
    }
}

/// Half-height of the camera's near plane (fov 70°, plane at 0.05).
fn near_half_height() -> f64 {
    (70.0f64.to_radians() / 2.0).tan() * 0.05
}

/// True if the near-plane points stay within one block of the eye on each axis.
fn reach(aspect: f32) -> bool {
    let h = near_half_height();
    0.05 + h + h * f64::from(aspect).abs() < 0.99
}

#[derive(Default, Clone, Copy)]
struct EndFlash {
    /// Period of the drawn parameters: 0 at the start like the game, which therefore draws nothing during
    /// the first 600 ticks of the clock (offset and duration zero: no flash).
    period: i64,
    offset: i32,
    duration: i32,
    angle_x: f32,
    angle_y: f32,
    old: f32,
    now: f32,
}

impl EndFlash {
    /// Advances one tick with `t` ticks of the default clock (`docs/specs/sky.md` § 6.3).
    fn tick(&mut self, t: i64) {
        let g = t.div_euclid(600);
        if self.period != g {
            let mut r = crate::sky_mesh::JavaRandom::new(g);
            r.float();
            self.offset = r.next_int(201);
            self.duration = 100 + r.next_int((380.min(600 - self.offset)) - 100 + 1);
            self.angle_x = r.float() * 70.0 - 60.0;
            self.angle_y = r.float() * 360.0 - 180.0;
            self.period = g;
        }
        self.old = self.now;
        let u = t.rem_euclid(600) as i32;
        self.now = if (self.offset..=self.offset + self.duration).contains(&u) {
            crate::sky_mesh::table_sin((u - self.offset) as f32 * std::f32::consts::PI / self.duration as f32)
        } else {
            0.0
        };
    }

    fn at(&self, pt: f32) -> f32 {
        self.old + pt * (self.now - self.old)
    }

    /// The flash starts at this tick (intensity became positive).
    fn started(&self) -> bool {
        self.now > 0.0 && self.old <= 0.0
    }
}

/// Direction (pitch, yaw in degrees) toward the End flash's sound point, with the
/// game's tabulated sines: yaw shifted by a half turn, pitch cosine sign-flipped.
pub fn end_flash_direction(x_angle: f32, y_angle: f32) -> [f64; 3] {
    use crate::sky_mesh::{table_cos, table_sin};
    let deg = std::f32::consts::PI / 180.0;
    let ry = -y_angle * deg - std::f32::consts::PI;
    let (yc, ys) = (table_cos(ry), table_sin(ry));
    let (xc, xs) = (-table_cos(-x_angle * deg), table_sin(-x_angle * deg));
    [f64::from(ys * xc), f64::from(xs), f64::from(yc * xc)]
}

impl EnvState {
    /// Id of the plains (biome of absent columns, like the game's empty level).
    pub fn plains(&self) -> u32 {
        self.plains
    }

    /// Entering a world (login or respawn).
    pub fn join(&mut self, d: &rc_data::VersionData, dimension: &str, biomes: &[String], flat: bool) {
        self.env = Env::new(d, dimension, biomes);
        self.probe.reset();
        self.rain_fog = 0.0;
        self.flat = flat;
        let dt = d.dimension_types.iter().find(|t| t.name == dimension);
        self.skybox = dt.map_or(1, |t| t.skybox);
        self.default_clock = dt.map_or(String::new(), |t| t.default_clock.clone());
        self.flash = EndFlash::default();
        self.has_sky = dt.is_none_or(|t| t.has_skylight);
        self.ambient_light = dt.map_or(0.0, |t| t.ambient_light);
        self.lightmap_dirty = true;
        self.plains = biomes.iter().position(|b| b == "minecraft:plains").unwrap_or(0) as u32;
        self.powder_snow = d.blocks.iter().position(|b| b.name == "minecraft:powder_snow").map(|i| i as u16);
        self.cache = ViewCache { on: self.cache.on, ..ViewCache::default() };
    }

    /// Tick of the client world: clocks, underwater vision, biome probe.
    pub fn tick(&mut self, w: &WorldView<'_>, eyes_in_water: bool) {
        if !self.frozen {
            self.env.clocks.tick();
        }
        self.water_vision.tick(eyes_in_water, self.game_mode == 3);
        self.eyes_in_water = eyes_in_water;
        self.flicker.tick();
        self.lightmap_dirty = true;
        if !self.frozen {
            self.boss_dark.0 = self.boss_dark.1;
            self.boss_dark.1 = if self.boss_flags & 1 != 0 {
                (self.boss_dark.1 + 0.05).min(1.0)
            } else {
                (self.boss_dark.1 - 0.0125).max(0.0)
            };
        }
        if self.skybox == 2 {
            let t = self.env.clocks.total_of(&self.default_clock);
            self.flash.tick(t);
        }
        let plains = self.plains;
        let weights = rc_env::biome_weights(self.cam, |x, y, z| w.biome(x, y, z, plains));
        self.probe.tick(weights);
    }

    /// End flash added to the sky light factor: nothing if flashes are hidden,
    /// a third with a boss bar's fog.
    fn end_flash_light(&self, pt: f32) -> f32 {
        let flash = if self.skybox == 2 && !self.hide_flashes { self.flash.at(pt) } else { 0.0 };
        if self.boss_flags & 4 != 0 { flash / 3.0 } else { flash }
    }

    /// Angles (pitch, yaw) of the End flash if it started at the last tick.
    pub fn end_flash_started(&self) -> Option<(f32, f32)> {
        (self.skybox == 2 && self.flash.started()).then_some((self.flash.angle_x, self.flash.angle_y))
    }

    /// Biome of the quart containing `pos` (non-interpolated attributes: audio).
    pub fn biome_at(&self, w: &WorldView<'_>, pos: [f64; 3]) -> u32 {
        let q = pos.map(|v| (v.floor() as i32) >> 2);
        w.biome(q[0], q[1], q[2], self.plains)
    }

    /// Biome of the block containing `pos` after the blur zoom (F3 screen, like the game).
    pub fn biome_full_at(&self, w: &WorldView<'_>, pos: [f64; 3]) -> u32 {
        let b = pos.map(|v| v.floor() as i32);
        w.biome_full(b[0], b[1], b[2], self.plains)
    }

    /// Sun angle (degrees), for the clock hands.
    pub fn sun_angle(&mut self) -> f32 {
        self.env.eval_global(Attr::SunAngle)[0]
    }

    /// Entity shadow parameters: sky darkening, sky present, `ambient_light`.
    pub fn shadow_params(&mut self) -> (i32, bool, f32) {
        let darken = (15.0 - self.env.eval_global(Attr::SkyLightLevel)[0]) as i32;
        (darken, self.has_sky, self.ambient_light)
    }

    /// Brightness of the dimension at the eye's block, for the vignette.
    pub fn eye_brightness(&mut self, w: &WorldView<'_>, eye: [f64; 3]) -> f32 {
        let darken = (15.0 - self.env.eval_global(Attr::SkyLightLevel)[0]) as i32;
        let b = eye.map(|v| v.floor() as i32);
        let level = w.raw_brightness(b[0], b[1], b[2], self.has_sky, darken);
        let v = level as f32 / 15.0;
        let curved = v / (4.0 - 3.0 * v);
        self.ambient_light + (1.0 - self.ambient_light) * curved
    }

    /// Lightmap to send to the GPU if a tick has passed since the last one (`lighting.md` § 4).
    pub fn lightmap(&mut self, pt: f32) -> Option<Vec<u8>> {
        if !self.lightmap_dirty {
            return None;
        }
        self.lightmap_dirty = false;
        let flash = self.end_flash_light(pt);
        // Night vision, otherwise conduit power with the eyes in water (underwater vision).
        let water = self.water_vision.value(self.eyes_in_water);
        let night_vision = self
            .effects
            .night_vision_scale(pt)
            .unwrap_or(if water > 0.0 && self.effects.conduit_power { water } else { 0.0 });
        let mut get = |a: Attr| self.probe.get(&mut self.env, a, pt);
        let input = rc_env::LightmapInput {
            // Thunderstorm flash: sky factor forced to 1 (`environment.md` § 1.5, point 5).
            sky_factor: if self.sky_flash { 1.0 } else { get(Attr::SkyLightFactor)[0] } + flash,
            sky_color: get(Attr::SkyLightColor),
            block_tint: get(Attr::BlockLightTint),
            ambient: get(Attr::AmbientLightColor),
            night_vision_color: get(Attr::NightVisionColor),
            night_vision,
            block_factor: 1.4 + self.flicker.value,
            brightness: (self.gamma.unwrap_or(0.5) - self.darkness.0).max(0.0),
            boss_darken: self.boss_dark.0 + pt * (self.boss_dark.1 - self.boss_dark.0),
            darkness: self.darkness.1,
        };
        Some(rc_env::lightmap(&input))
    }

    /// Fluid or block in the camera (spec § 5.1): water at the eye, then lava or powder snow
    /// at five points of the near plane.
    #[allow(clippy::too_many_arguments)]
    fn camera_kind(
        &self,
        w: &WorldView<'_>,
        t: &rc_physics::Tables,
        d: &rc_data::VersionData,
        eye: [f64; 3],
        yaw: f32,
        pitch: f32,
        aspect: f32,
    ) -> FogKind {
        let top =
            |s: u32, x: i32, y: i32, z: i32| f64::from(y) + f64::from(t.fluids.camera_height(s, w.state(x, y + 1, z)));
        let b = eye.map(|v| v.floor() as i32);
        let s = w.state(b[0], b[1], b[2]);
        if t.fluids.fluid(s) == Fluid::Water && eye[1] < top(s, b[0], b[1], b[2]) {
            return FogKind::Water;
        }
        let (yr, pr) = (f64::from(yaw.to_radians()), f64::from(pitch.to_radians()));
        let f = [-yr.sin() * pr.cos(), -pr.sin(), yr.cos() * pr.cos()];
        let l = [yr.cos(), 0.0, yr.sin()];
        let u = [f[1] * l[2] - f[2] * l[1], f[2] * l[0] - f[0] * l[2], f[0] * l[1] - f[1] * l[0]];
        let half_h = near_half_height();
        let half_w = half_h * f64::from(aspect);
        for (su, sl) in [(0.0, 0.0), (1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)] {
            let p: [f64; 3] = std::array::from_fn(|a| eye[a] + f[a] * 0.05 + u[a] * half_h * su + l[a] * half_w * sl);
            let b = p.map(|v| v.floor() as i32);
            let s = w.state(b[0], b[1], b[2]);
            if t.fluids.fluid(s) == Fluid::Lava && p[1] <= top(s, b[0], b[1], b[2]) {
                return FogKind::Lava;
            }
            if self.powder_snow.is_some() && d.states.get(s as usize).map(|st| st.block) == self.powder_snow {
                return FogKind::PowderSnow;
            }
        }
        FogKind::Atmospheric
    }

    /// Fog and sky of the frame (`pt`: tick fraction, `dt`: frame duration in s).
    #[allow(clippy::too_many_arguments)]
    pub fn frame(
        &mut self,
        w: &WorldView<'_>,
        t: &rc_physics::Tables,
        d: &rc_data::VersionData,
        eye: [f64; 3],
        yaw: f32,
        pitch: f32,
        aspect: f32,
        pt: f32,
        dt: f64,
        option_view: i32,
        eyes_in_water: bool,
    ) -> FrameEnv {
        self.cam = eye;
        let b = eye.map(|v| v.floor() as i32);
        let (kind, biome) = if self.cache.on {
            let clear = reach(aspect) && self.cache.clear_around(w, t, d, self.powder_snow, b);
            let kind =
                if clear { FogKind::Atmospheric } else { self.camera_kind(w, t, d, eye, yaw, pitch, aspect) };
            let q = self.cache.zoom(biome_zoom_seed(), b);
            (kind, w.biome(q.0, q.1, q.2, self.plains))
        } else {
            (self.camera_kind(w, t, d, eye, yaw, pitch, aspect), w.biome_full(b[0], b[1], b[2], self.plains))
        };
        let render_chunks = if self.server_view > 0 { option_view.min(self.server_view) } else { option_view };
        let (yr, pr) = (yaw.to_radians(), pitch.to_radians());
        let input = FogInput {
            kind,
            pt,
            render_chunks,
            eye_y: eye[1],
            min_y: w.min_y,
            flat: self.flat,
            forward_x: -yr.sin() * pr.cos(),
            water_vision: self.water_vision.value(eyes_in_water),
            sky_light: w.sky_light(b[0], b[1], b[2]),
            biome_rain: self.env.has_precipitation(biome),
            dt_ticks: (dt * 20.0) as f32,
            blindness: self.eye_effects.0,
            darkness_blend: self.eye_effects.1,
            boss_darken: self.boss_dark.0 + pt * (self.boss_dark.1 - self.boss_dark.0),
            boss_fog: self.boss_flags & 4 != 0,
            // Darkness cancels the brightening of night vision.
            night_vision: if self.eye_effects.1.is_some() {
                0.0
            } else {
                self.effects.night_vision_scale(pt).unwrap_or(0.0)
            },
            spectator: self.game_mode == 3,
            fire_resistance: self.effects.fire_resistance,
            cloud_range: self.clouds.map_or(CLOUD_RANGE, |c| c.1),
        };
        // The sky is not drawn with blindness, darkness or boss fog.
        let sky_blocked = self.eye_effects.0.is_some() || self.eye_effects.1.is_some() || self.boss_flags & 4 != 0;
        let fog = rc_env::fog(&mut self.env, &mut self.probe, &mut self.rain_fog, &input);
        let mut sky = self.probe.get(&mut self.env, Attr::SkyColor, pt);
        if self.sky_flash {
            for (c, f) in sky.iter_mut().zip([0.8, 0.8, 1.0]) {
                *c += 0.22 * (f - *c);
            }
        }
        let dome = self.skybox == 1 && !matches!(kind, FogKind::Lava | FogKind::PowderSnow) && !sky_blocked;
        let horizon = if self.flat { f64::from(w.min_y) } else { 63.0 };
        let (game_time, rain) = (self.env.clocks.game_time, self.env.weather.rain);
        let mut get = |a: Attr| self.probe.get(&mut self.env, a, pt);
        let (sun_deg, moon_deg, star_deg) = (get(Attr::SunAngle)[0], get(Attr::MoonAngle)[0], get(Attr::StarAngle)[0]);
        let (star_brightness, sunrise, moon_phase) =
            (get(Attr::StarBrightness)[0], get(Attr::SunriseSunsetColor), get(Attr::MoonPhase)[0] as u32);
        // Cloud color brought down to 8 bits per channel; height re-read only if visible.
        let cloud = get(Attr::CloudColor).map(|v| (v * 255.0).floor().clamp(0.0, 255.0) / 255.0);
        let (clouds_on, cloud_range, flat) = self.clouds.unwrap_or((true, CLOUD_RANGE, false));
        let clouds = (cloud[3] > 0.0 && clouds_on).then(|| crate::sky::CloudFrame {
            color: cloud,
            height: get(Attr::CloudHeight)[0],
            game_time,
            pt,
            range_chunks: cloud_range,
            cloud_end: fog.cloud_end,
            flat,
        });
        let flash = self.flash.at(pt);
        let end = (self.skybox == 2 && !matches!(kind, FogKind::Lava | FogKind::PowderSnow) && !sky_blocked)
            .then_some((flash, self.flash.angle_x, self.flash.angle_y));
        let bodies = crate::sky::SkyFrame {
            end,
            sun_deg,
            moon_deg,
            star_deg,
            star_brightness,
            rain,
            moon_phase,
            sunrise,
            clouds,
        };
        let far = ((64 * render_chunks) as f32).max((16 * cloud_range) as f32);
        FrameEnv {
            fog,
            sky,
            dome,
            disc: dome && kind != FogKind::Water && eye[1] - horizon < 0.0,
            bodies,
            far,
            fluid: matches!(kind, FogKind::Water | FogKind::Lava),
        }
    }
}

#[cfg(test)]
mod cache_tests;

#[cfg(test)]
mod end_flash_tests;
