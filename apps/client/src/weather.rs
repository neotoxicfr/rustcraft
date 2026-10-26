//! Weather on the client (`docs/specs/weather-border.md` § 1-3): "blocks motion" heightmap,
//! temperature and precipitation at a point, selection of rain and snow
//! columns, ground drops and rain sound, lightning (sounds, sky flash).

use hashbrown::HashMap;
use std::sync::OnceLock;

use crate::environ::WorldView;
use crate::sky_mesh::JavaRandom;

// ---------- Generator with Gaussians ----------

/// The game's LCG with the queue of one Gaussian value (Marsaglia polar method).
pub struct Lcg {
    r: JavaRandom,
    gauss: Option<f64>,
}

impl Lcg {
    pub fn new(seed: i64) -> Self {
        Lcg { r: JavaRandom::new(seed), gauss: None }
    }

    pub fn int(&mut self, n: i32) -> i32 {
        self.r.next_int(n)
    }

    pub fn float(&mut self) -> f32 {
        self.r.float()
    }

    pub fn double(&mut self) -> f64 {
        self.r.double()
    }

    pub fn gaussian(&mut self) -> f64 {
        if let Some(g) = self.gauss.take() {
            return g;
        }
        loop {
            let x = 2.0 * self.r.double() - 1.0;
            let y = 2.0 * self.r.double() - 1.0;
            let s = x * x + y * y;
            if s < 1.0 && s != 0.0 {
                let f = (-2.0 * s.ln() / s).sqrt();
                self.gauss = Some(y * f);
                return x * f;
            }
        }
    }
}

// ---------- Temperature noises (§ 1.2) ----------

/// 2D simplex with permutation table (zero offsets).
struct Simplex {
    p: [u8; 256],
}

const GRAD: [(f64, f64); 12] = [
    (1.0, 1.0),
    (-1.0, 1.0),
    (1.0, -1.0),
    (-1.0, -1.0),
    (1.0, 0.0),
    (-1.0, 0.0),
    (1.0, 0.0),
    (-1.0, 0.0),
    (0.0, 1.0),
    (0.0, -1.0),
    (0.0, 1.0),
    (0.0, -1.0),
];

impl Simplex {
    fn new(r: &mut JavaRandom) -> Self {
        // Three offsets drawn then thrown away.
        for _ in 0..3 {
            r.double();
        }
        let mut p: [u8; 256] = std::array::from_fn(|i| i as u8);
        for i in 0..256 {
            let j = i + r.next_int(256 - i as i32) as usize;
            p.swap(i, j);
        }
        Simplex { p }
    }

    fn perm(&self, i: i32) -> i32 {
        i32::from(self.p[(i & 255) as usize])
    }

    fn get(&self, u: f64, v: f64) -> f32 {
        let f2 = 0.5 * (3f64.sqrt() - 1.0);
        let g2 = (3.0 - 3f64.sqrt()) / 6.0;
        let s = (u + v) * f2;
        let i = (u + s).floor() as i32;
        let j = (v + s).floor() as i32;
        let t = f64::from(i.wrapping_add(j)) * g2;
        let x0 = u - (f64::from(i) - t);
        let y0 = v - (f64::from(j) - t);
        let (i1, j1) = if x0 > y0 { (1, 0) } else { (0, 1) };
        let x1 = x0 - f64::from(i1) + g2;
        let y1 = y0 - f64::from(j1) + g2;
        let x2 = x0 - 1.0 + 2.0 * g2;
        let y2 = y0 - 1.0 + 2.0 * g2;
        let (ii, jj) = (i & 255, j & 255);
        let g0 = self.perm(ii + self.perm(jj)) % 12;
        let g1 = self.perm(ii + i1 + self.perm(jj + j1)) % 12;
        let gi2 = self.perm(ii + 1 + self.perm(jj + 1)) % 12;
        let corner = |g: i32, x: f64, y: f64| {
            let t = 0.5 - x * x - y * y;
            if t < 0.0 {
                0.0
            } else {
                let t = t * t;
                let (gx, gy) = GRAD[g as usize];
                t * t * (gx * x + gy * y)
            }
        };
        (70.0 * (corner(g0, x0, y0) + corner(g1, x1, y1) + corner(gi2, x2, y2))) as f32
    }
}

/// Climate noises: `T` (seed 1234), `B` (2345), stack `F` (3456, three layers).
struct Climate {
    t: Simplex,
    b: Simplex,
    f: [Simplex; 3],
}

fn climate() -> &'static Climate {
    static C: OnceLock<Climate> = OnceLock::new();
    C.get_or_init(|| {
        let mut rf = JavaRandom::new(3456);
        let f = [Simplex::new(&mut rf), Simplex::new(&mut rf), Simplex::new(&mut rf)];
        Climate { t: Simplex::new(&mut JavaRandom::new(1234)), b: Simplex::new(&mut JavaRandom::new(2345)), f }
    })
}

impl Climate {
    /// Stack `F`: amplitudes and frequencies of the spec, sum in float.
    fn frozen(&self, u: f64, v: f64) -> f32 {
        let mut s = 0.0f32;
        for (n, (freq, amp)) in self.f.iter().zip([(1.0, 0.142_857_15f32), (0.5, 0.285_714_3), (0.25, 0.571_428_6)]) {
            s += amp * n.get(u * freq, v * freq);
        }
        s
    }
}

/// Temperature at block `(x, y, z)` of a base biome `base` (§ 1.2).
pub fn temperature(base: f32, frozen: bool, x: i32, y: i32, z: i32, sea_level: i32) -> f32 {
    let c = climate();
    let mut t = base;
    if frozen {
        let large = f64::from(c.frozen(f64::from(x) * 0.05, f64::from(z) * 0.05) * 7.0);
        let edge = f64::from(c.b.get(f64::from(x) * 0.2, f64::from(z) * 0.2));
        if large + edge < 0.3 && f64::from(c.b.get(f64::from(x) * 0.09, f64::from(z) * 0.09)) < 0.8 {
            t = 0.2;
        }
    }
    let snow = sea_level + 17;
    if y > snow {
        let v = c.t.get(f64::from(x as f32 / 8.0), f64::from(z as f32 / 8.0)) * 8.0;
        t - (v + y as f32 - snow as f32) * 0.05 / 40.0
    } else {
        t
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Precip {
    None,
    Rain,
    Snow,
}

/// Climate of the current world's biomes, in network order.
#[derive(Default)]
pub struct BiomeClimate {
    temp: Vec<f32>,
    frozen: Vec<bool>,
    rain: Vec<bool>,
    pub sea_level: i32,
}

impl BiomeClimate {
    pub fn new(d: &rc_data::VersionData, biomes: &[String], sea_level: i32) -> Self {
        let mut c = BiomeClimate { sea_level, ..Default::default() };
        for n in biomes {
            let b = d.biomes.iter().find(|b| &b.name == n);
            c.temp.push(b.map_or(0.8, |b| b.temperature));
            c.frozen.push(b.is_some_and(|b| b.frozen));
            c.rain.push(b.is_none_or(|b| b.has_precipitation));
        }
        c
    }

    /// Column precipitation at the point (§ 1.2): chunk absent or dry biome → none.
    pub fn at(&self, w: &WorldView<'_>, x: i32, y: i32, z: i32) -> Precip {
        if w.grid.get(x >> 4, z >> 4).is_none() {
            return Precip::None;
        }
        let b = w.biome_full(x, y, z, 0) as usize;
        if !self.rain.get(b).copied().unwrap_or(true) {
            return Precip::None;
        }
        let base = self.temp.get(b).copied().unwrap_or(0.8);
        let frozen = self.frozen.get(b).copied().unwrap_or(false);
        if temperature(base, frozen, x, y, z, self.sea_level) >= 0.15 { Precip::Rain } else { Precip::Snow }
    }
}

// ---------- "Blocks motion" heightmap (§ 1.3) ----------

/// Id of the `MOTION_BLOCKING` map in the chunk packet.
const MOTION_BLOCKING: i32 = 4;

/// "First free y" per column of each loaded chunk, kept up to date on block changes.
#[derive(Default)]
pub struct Heights {
    cols: HashMap<(i32, i32), Box<[i16; 256]>>,
    min_y: i32,
    height: i32,
    /// Per block id: tag `minecraft:blocks_motion_in_heightmap`.
    tagged: Vec<bool>,
}

/// Blocking in the sense of the map: tagged or containing a fluid.
pub struct Blocking<'a> {
    pub data: &'a rc_data::VersionData,
    pub phys: &'a rc_physics::Tables,
}

impl Blocking<'_> {
    fn test(&self, tagged: &[bool], s: u32) -> bool {
        let block = self.data.states.get(s as usize).map_or(0, |st| usize::from(st.block));
        tagged.get(block).copied().unwrap_or(false) || self.phys.fluids.fluid(s) != rc_physics::Fluid::None
    }
}

impl Heights {
    pub fn reset(&mut self, min_y: i32, height: i32) {
        self.cols.clear();
        (self.min_y, self.height) = (min_y, height);
    }

    pub fn set_block_tags(&mut self, blocks: usize, tags: &[(String, Vec<i32>)]) {
        self.tagged = vec![false; blocks];
        if let Some((_, ids)) = tags.iter().find(|(n, _)| n == "minecraft:blocks_motion_in_heightmap") {
            for &i in ids {
                if let Some(t) = usize::try_from(i).ok().and_then(|i| self.tagged.get_mut(i)) {
                    *t = true;
                }
            }
        }
    }

    /// Chunk received: the packet's map, or computed from the blocks if absent or wrongly sized.
    pub fn on_chunk(&mut self, col: &rc_world::Column, blocking: &Blocking<'_>) {
        let bits = (32 - (self.height as u32).leading_zeros()).max(1) as usize;
        let per = 64 / bits;
        let longs = 256usize.div_ceil(per);
        let raw = col.heightmaps.iter().find(|h| h.0 == MOTION_BLOCKING).filter(|h| h.1.len() == longs);
        let mut out = Box::new([0i16; 256]);
        match raw {
            Some((_, data)) => {
                let mask = (1u64 << bits) - 1;
                for (i, o) in out.iter_mut().enumerate() {
                    let v = (data[i / per] as u64 >> ((i % per) * bits)) & mask;
                    *o = (v as i32 + self.min_y) as i16;
                }
            }
            None => {
                for (i, o) in out.iter_mut().enumerate() {
                    let (x, z) = (i & 15, i >> 4);
                    let top = (0..self.height as usize)
                        .rev()
                        .find(|&y| col.block(x, y, z).is_some_and(|s| blocking.test(&self.tagged, s)))
                        .map_or(self.min_y, |y| y as i32 + 1 + self.min_y);
                    *o = top as i16;
                }
            }
        }
        self.cols.insert((col.x, col.z), out);
    }

    pub fn forget(&mut self, x: i32, z: i32) {
        self.cols.remove(&(x, z));
    }

    /// Block changed (after the section update): game rule (§ 1.3).
    pub fn on_block(&mut self, w: &WorldView<'_>, pos: [i32; 3], state: u32, blocking: &Blocking<'_>) {
        let (tagged, min_y) = (&self.tagged, self.min_y);
        let Some(col) = self.cols.get_mut(&(pos[0] >> 4, pos[2] >> 4)) else { return };
        let i = ((pos[2] & 15) << 4 | (pos[0] & 15)) as usize;
        let h = i32::from(col[i]);
        let y = pos[1];
        if y <= h - 2 {
            return;
        }
        if blocking.test(tagged, state) {
            if y >= h {
                col[i] = (y + 1) as i16;
            }
        } else if y == h - 1 {
            let top = (min_y..y).rev().find(|&yy| blocking.test(tagged, w.state(pos[0], yy, pos[2])));
            col[i] = top.map_or(min_y, |t| t + 1) as i16;
        }
    }

    /// First free y of the column; chunk absent: minimum Y; beyond ±30000000: sea + 1.
    pub fn get(&self, x: i32, z: i32, sea_level: i32) -> i32 {
        if !(-30_000_000..30_000_000).contains(&x) || !(-30_000_000..30_000_000).contains(&z) {
            return sea_level + 1;
        }
        self.cols.get(&(x >> 4, z >> 4)).map_or(self.min_y, |c| i32::from(c[((z & 15) << 4 | (x & 15)) as usize]))
    }
}

// ---------- Border wall (§ 4.3) ----------

impl WeatherState {
    /// Wall of the frame: visibility, tint and alpha on every frame; geometry redone when the
    /// interpolated bounds change or the camera changes section. `view_blocks` = effective
    /// render distance × 16, `far` = far plane.
    pub fn prepare_wall(
        &mut self,
        out: &mut crate::weather_render::Wall,
        c: [f64; 3],
        pt: f32,
        view_blocks: f64,
        far: f32,
    ) {
        let b = &self.border;
        let k = b.bounds_at(f64::from(pt));
        let dd = view_blocks;
        let deep_inside = c[0] < k[1] - dd && c[0] > k[0] + dd && c[2] < k[3] - dd && c[2] > k[2] + dd;
        let far_out = c[0] < k[0] - dd || c[0] > k[1] + dd || c[2] < k[2] - dd || c[2] > k[3] + dd;
        out.order.clear();
        if deep_inside || far_out {
            return;
        }
        let alpha = (1.0 - b.distance(c[0], c[2]) / dd).powi(4).clamp(0.0, 1.0) as f32;
        let rgb = match b.status() {
            rc_physics::border::Status::Growing => 0x40_ff80,
            rc_physics::border::Status::Shrinking => 0xff_3030,
            rc_physics::border::Status::Stationary => 0x20_a0ff,
        };
        let ch = |s: u32| ((rgb >> s) & 255) as f32 / 255.0;
        let section = c.map(|v| (v.floor() as i32) >> 4);
        // Settings (render distance, far plane) in the key too: rare, but the geometry depends on them.
        let key = (k.map(f64::to_bits), section, [dd.to_bits(), u64::from(far.to_bits())]);
        if self.wall_key != Some(key) {
            self.wall_key = Some(key);
            let (bx0, bx1, bz0, bz1) = (k[0] + 0.01, k[1] - 0.01, k[2] + 0.01, k[3] - 0.01);
            let z0 = (c[2] - dd).floor().max(bz0);
            let z1 = (c[2] + dd).ceil().min(bz1);
            let x0 = (c[0] - dd).floor().max(bx0);
            let x1 = (c[0] + dd).ceil().min(bx1);
            self.wall_origin = [x0, z0];
            let h = f64::from(far);
            let v0 = -(c[1] * 0.5 - (c[1] * 0.5).floor());
            let v1 = v0 + h;
            let ux0 = ((x0.floor() as i64) & 1) as f64 * 0.5;
            let ux1 = (x1 - x0) / 2.0;
            let uz0 = ((z0.floor() as i64) & 1) as f64 * 0.5;
            let uz1 = (z1 - z0) / 2.0;
            let (sx, sz, ex, sz1) = (x1 - x0, bz1 - z0, bx1 - x0, z1 - z0);
            // Corners (x, y, z, u, v) of each quad.
            type Corner = (f64, f64, f64, f64, f64);
            let quads: [[Corner; 4]; 4] = [
                // South.
                [
                    (0.0, -h, sz, ux0, v1),
                    (sx, -h, sz, ux1 + ux0, v1),
                    (sx, h, sz, ux1 + ux0, v0),
                    (0.0, h, sz, ux0, v0),
                ],
                // West.
                [
                    (0.0, -h, 0.0, uz0, v1),
                    (0.0, -h, sz1, uz1 + uz0, v1),
                    (0.0, h, sz1, uz1 + uz0, v0),
                    (0.0, h, 0.0, uz0, v0),
                ],
                // North.
                [
                    (sx, -h, 0.0, ux0, v1),
                    (0.0, -h, 0.0, ux1 + ux0, v1),
                    (0.0, h, 0.0, ux1 + ux0, v0),
                    (sx, h, 0.0, ux0, v0),
                ],
                // East.
                [
                    (ex, -h, sz1, uz0, v1),
                    (ex, -h, 0.0, uz1 + uz0, v1),
                    (ex, h, 0.0, uz1 + uz0, v0),
                    (ex, h, sz1, uz0, v0),
                ],
            ];
            for (q, corners) in quads.iter().enumerate() {
                let v = |i: usize| {
                    let (x, y, z, u, vv) = corners[i];
                    crate::weather_render::WallVert { pos: [x as f32, y as f32, z as f32], uv: [u as f32, vv as f32] }
                };
                for (j, i) in [0, 1, 2, 0, 2, 3].into_iter().enumerate() {
                    out.verts[q * 6 + j] = v(i);
                }
            }
            out.dirty = true;
        }
        let ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() % 3000);
        let o = self.wall_origin;
        out.frame = crate::weather_render::WallFrame {
            off: [(o[0] - c[0].floor()) as f32, (o[1] - c[2].floor()) as f32, ms as f32 / 3000.0, 0.0],
            color: [ch(16), ch(8), ch(0), alpha],
        };
        // Faces sorted by increasing distance, kept under the render distance.
        let mut faces = [(c[2] - k[2], 2u32), (k[3] - c[2], 0), (c[0] - k[0], 1), (k[1] - c[0], 3)];
        faces.sort_by(|a, b| a.0.total_cmp(&b.0));
        out.order.extend(faces.iter().filter(|f| f.0 < dd).map(|f| f.1));
    }
}

// ---------- Ground effects and thunderstorm (§ 2.1, § 3) ----------

/// Lightning tracked on the client: life, remaining flashes, own generator.
struct Bolt {
    life: i32,
    flashes: i32,
    rng: Lcg,
}

/// What the weather tick reads and modifies in the app.
pub struct TickCtx<'a, 'w> {
    pub view: &'a WorldView<'w>,
    pub data: &'a rc_data::VersionData,
    pub phys: &'a rc_physics::Tables,
    pub particles: &'a mut crate::particles::Particles,
    pub audio: Option<&'a mut rc_audio::AudioEngine>,
    pub entities: &'a crate::entities::Entities,
    /// Camera position (eyes).
    pub eye: [f64; 3],
    pub rain: f32,
    pub game_time: i64,
    pub radius: i32,
    /// "Particles" setting: 0 all, 1 reduced, 2 minimal.
    pub particle_level: i32,
}

/// Top of the collision shape of state `s` under the local point `(fx, fz)`.
fn shape_top(phys: &rc_physics::Tables, s: u32, fx: f64, fz: f64) -> f64 {
    let mut boxes = Vec::new();
    phys.collision(s, &mut boxes);
    boxes
        .iter()
        .filter(|b| b.min[0] <= fx && fx <= b.max[0] && b.min[2] <= fz && fz <= b.max[2])
        .map(|b| b.max[1])
        .fold(0.0, f64::max)
}

impl WeatherState {
    /// Client world tick: ground drops and rain sound, then lightning (sounds, flash).
    /// Returns the extinguished lightning bolts, to remove from the client world.
    pub fn tick(&mut self, ctx: TickCtx<'_, '_>) -> Vec<i32> {
        self.border.tick();
        self.ground(&ctx);
        self.sky_flash = (self.sky_flash - 1).max(0);
        self.storm(ctx)
    }

    fn ground(&mut self, ctx: &TickCtx<'_, '_>) {
        let p = ctx.rain;
        if p <= 0.0 {
            return;
        }
        let w = ctx.view;
        let r = ctx.radius.clamp(3, 10);
        let mut rng = Lcg::new(ctx.game_time.wrapping_mul(312_987_231));
        let b = ctx.eye.map(|v| v.floor() as i32);
        let d = 2 * r + 1;
        let mut n = (0.225 * (d * d) as f32 * p * p) as i32;
        if ctx.particle_level == 1 {
            n /= 2;
        }
        let sea = self.climate.sea_level;
        let mut found = None;
        let mut spawns = Vec::new();
        for _ in 0..n {
            let (dx, dz) = (rng.int(d) - r, rng.int(d) - r);
            let (x, z) = (b[0] + dx, b[2] + dz);
            let y = self.heights.get(x, z, sea);
            if y <= w.min_y || (y - b[1]).abs() > 10 || self.climate.at(w, x, y, z) != Precip::Rain {
                continue;
            }
            let s = [x, y - 1, z];
            found = Some(s);
            if ctx.particle_level == 2 {
                break;
            }
            let (fx, fz) = (rng.double(), rng.double());
            let state = w.state(s[0], s[1], s[2]);
            let fluid = ctx.phys.fluids.fluid(state);
            let fh = if fluid == rc_physics::Fluid::None {
                0.0
            } else {
                f64::from(ctx.phys.fluids.height_at(state, w.state(s[0], s[1] + 1, s[2])))
            };
            let h = shape_top(ctx.phys, state, fx, fz).max(fh);
            let block = ctx.data.states.get(state as usize).and_then(|i| ctx.data.blocks.get(usize::from(i.block)));
            let name = block.map_or("", |b| b.name.as_str());
            let lit_campfire =
                name.ends_with("campfire") && ctx.data.state_properties(state).any(|p| p == ("lit", "true"));
            let smoke = fluid == rc_physics::Fluid::Lava || name == "minecraft:magma_block" || lit_campfire;
            spawns.push((smoke, [f64::from(s[0]) + fx, f64::from(s[1]) + h, f64::from(s[2]) + fz]));
        }
        // Particles outside the world borrow (the engine is passed by the caller).
        self.pending_drops = spawns;
        if let Some(s) = found {
            let hit = rng.int(3) < self.rain_sound;
            self.rain_sound += 1;
            if hit {
                self.rain_sound = 0;
                let above = s[1] > b[1] + 1 && self.heights.get(b[0], b[2], sea) > ctx.eye[1].floor() as i32;
                let (name, volume, pitch) = if above {
                    ("minecraft:weather.rain.above", 0.1, 0.5)
                } else {
                    ("minecraft:weather.rain", 0.2, 1.0)
                };
                self.pending_sound = Some((name, volume, pitch, s.map(|v| f64::from(v) + 0.5)));
            }
        }
    }

    fn storm(&mut self, ctx: TickCtx<'_, '_>) -> Vec<i32> {
        let TickCtx { particles, audio, entities, .. } = ctx;
        let opts = crate::particles::Options::default();
        for (smoke, pos) in self.pending_drops.drain(..) {
            let k = if smoke { crate::particles::Kind::Smoke } else { crate::particles::Kind::Rain };
            particles.spawn(k, &opts, pos, [0.0; 3]);
        }
        let mut audio = audio;
        if let (Some((name, volume, pitch, pos)), Some(a)) = (self.pending_sound.take(), audio.as_deref_mut()) {
            a.play(name, rc_audio::Category::Weather, Some(pos), volume, pitch, None);
        }
        // Lightning: tracked as long as the entity exists.
        self.bolts.retain(|id, _| entities.map.get(id).is_some_and(|e| e.kind == "lightning_bolt"));
        let mut gone = Vec::new();
        for (&id, e) in entities.map.iter().filter(|(_, e)| e.kind == "lightning_bolt") {
            let bolt = self.bolts.entry(id).or_insert_with(|| {
                let mut rng = Lcg::new((e.uuid as i64) ^ ((e.uuid >> 64) as i64));
                let flashes = rng.int(3) + 1;
                Bolt { life: 2, flashes, rng }
            });
            if bolt.life == 2
                && !e.meta_bool(4)
                && let Some(a) = audio.as_deref_mut()
            {
                let pitch = 0.8 + bolt.rng.float() * 0.2;
                a.play(
                    "minecraft:entity.lightning_bolt.thunder",
                    rc_audio::Category::Weather,
                    Some(e.pos),
                    10_000.0,
                    pitch,
                    None,
                );
                let pitch = 0.5 + bolt.rng.float() * 0.2;
                a.play(
                    "minecraft:entity.lightning_bolt.impact",
                    rc_audio::Category::Weather,
                    Some(e.pos),
                    2.0,
                    pitch,
                    None,
                );
            }
            bolt.life -= 1;
            if bolt.life < 0 {
                if bolt.flashes == 0 {
                    gone.push(id);
                    continue;
                }
                if bolt.life < -bolt.rng.int(10) {
                    bolt.flashes -= 1;
                    bolt.life = 1;
                }
            }
            if bolt.life >= 0 {
                self.sky_flash = 2;
            }
        }
        for id in &gone {
            self.bolts.remove(id);
        }
        gone
    }
}

// ---------- App state ----------

/// All the weather state of the client world (a single field in the app).
#[derive(Default)]
pub struct WeatherState {
    pub heights: Heights,
    pub climate: BiomeClimate,
    /// Client ticks elapsed.
    pub ticks: u64,
    /// World version (chunks, blocks, light): the columns are redone when it changes.
    pub rev: u64,
    /// Key of the columns sent: camera block, radius, world version.
    pub built: Option<([i32; 3], i32, u64)>,
    /// Column builds: count and cumulated time (s), for the log.
    pub build_stats: (u32, f64),
    /// Weather images, handed back to the renderer on its creation.
    pub images: Option<crate::weather_render::WeatherImages>,
    /// Rain sound counter, drops and sound waiting for the tick.
    rain_sound: i32,
    pending_drops: Vec<(bool, [f64; 3])>,
    pending_sound: Option<(&'static str, f32, f32, [f64; 3])>,
    bolts: HashMap<i32, Bolt>,
    /// World lightning counter: sky flash as long as it is positive.
    pub sky_flash: i32,
    /// World border (§ 4).
    pub border: rc_physics::WorldBorder,
    /// Wall: key of the last build (interpolated bounds, camera section).
    wall_key: Option<([u64; 4], [i32; 3], [u64; 2])>,
    wall_origin: [f64; 2],
}

/// Half-widths of the columns according to their offset from the camera block (the game's 32 × 32 table).
fn half_width(dx: i32, dz: i32) -> (f32, f32) {
    let d = ((dx * dx + dz * dz) as f32).sqrt();
    (-(dz as f32) / d / 2.0, dx as f32 / d / 2.0)
}

impl WeatherState {
    /// Rain and snow columns of the frame (§ 1.4-1.7). Redone only when the camera block,
    /// the radius or the world change: the rest (scrolling, alpha) is in the shader.
    #[allow(clippy::too_many_arguments)]
    pub fn prepare(
        &mut self,
        out: &mut crate::weather_render::WeatherRenderer,
        w: &WorldView<'_>,
        d: &rc_data::VersionData,
        eye: [f64; 3],
        rain: f32,
        game_time: i64,
        pt: f32,
        radius: i32,
    ) {
        let r = radius.clamp(3, 10);
        out.frame = crate::weather_render::Frame {
            ticks: [(game_time & 131_071) as u32, (game_time & 511) as u32, 0, 0],
            f: [pt, rain, (r * r) as f32, 0.0],
        };
        if rain <= 0.0 {
            out.counts = (0, 0);
            self.built = None;
            return;
        }
        let c = eye.map(|v| v.floor() as i32);
        let key = (c, r, self.rev);
        if self.built == Some(key) {
            return;
        }
        self.built = Some(key);
        let t0 = std::time::Instant::now();
        let (mut rains, mut snows) = (Vec::new(), Vec::new());
        let sea = self.climate.sea_level;
        for z in c[2] - r..=c[2] + r {
            for x in c[0] - r..=c[0] + r {
                let h = self.heights.get(x, z, sea);
                let (y0, y1) = ((c[1] - r).max(h), (c[1] + r).max(h));
                // Camera column: degenerate quad (NaN) in the game, hence invisible.
                if y0 == y1 || (x == c[0] && z == c[2]) {
                    continue;
                }
                let kind = self.climate.at(w, x, c[1], z);
                if kind == Precip::None {
                    continue;
                }
                let g = x.wrapping_mul(x).wrapping_mul(3121).wrapping_add(x.wrapping_mul(45_238_971))
                    ^ z.wrapping_mul(z).wrapping_mul(418_711).wrapping_add(z.wrapping_mul(13761));
                let mut rng = Lcg::new(i64::from(g));
                let ly = c[1].max(h);
                let state = w.state(x, ly, z);
                let info = d.states.get(state as usize);
                let (mut bl, mut sl) = w.light_pair(x, ly, z);
                if info.is_some_and(|i| i.light & rc_data::light_bits::EMISSIVE != 0) {
                    (bl, sl) = (15, 15);
                } else {
                    bl = bl.max(info.map_or(0, |i| i.light_emission));
                }
                let (hx, hz) = half_width(x - c[0], z - c[2]);
                let pos = [(x - c[0]) as f32 + 0.5, (z - c[2]) as f32 + 0.5, y0 as f32, y1 as f32];
                let pack = |b: u8, s: u8| (u32::from(b) * 16) | ((u32::from(s) * 16) << 8);
                if kind == Precip::Rain {
                    let sum = x
                        .wrapping_mul(x)
                        .wrapping_mul(3121)
                        .wrapping_add(x.wrapping_mul(45_238_971))
                        .wrapping_add(z.wrapping_mul(z).wrapping_mul(418_711))
                        .wrapping_add(z.wrapping_mul(13761));
                    let speed = 3.0 + rng.float();
                    let p = [speed, (sum & 255) as f32, 0.0, 0.0];
                    rains.push(crate::weather_render::Inst { pos, half: [hx, hz], p, light: pack(bl, sl), kind: 0 });
                } else {
                    let (d1, g1) = (rng.double(), rng.gaussian() as f32);
                    let (d2, g2) = (rng.double(), rng.gaussian() as f32);
                    let (bl, sl) = ((bl * 3 + 15) / 4, (sl * 3 + 15) / 4);
                    let p = [d1 as f32, g1, d2 as f32, g2];
                    snows.push(crate::weather_render::Inst { pos, half: [hx, hz], p, light: pack(bl, sl), kind: 1 });
                }
            }
        }
        out.counts = (rains.len() as u32, snows.len() as u32);
        rains.append(&mut snows);
        out.insts = rains;
        out.dirty = true;
        self.build_stats.0 += 1;
        self.build_stats.1 += t0.elapsed().as_secs_f64();
    }
}

impl WeatherState {
    pub fn with_images(images: crate::weather_render::WeatherImages) -> Self {
        WeatherState { images: Some(images), ..Default::default() }
    }

    /// Entering a world; `sea_level`: `None` keeps the previous one (respawn).
    pub fn join(
        &mut self,
        d: &rc_data::VersionData,
        biomes: &[String],
        sea_level: Option<i32>,
        min_y: i32,
        height: i32,
    ) {
        let sea = sea_level.unwrap_or(self.climate.sea_level);
        self.climate = BiomeClimate::new(d, biomes, sea);
        self.heights.reset(min_y, height);
        // New client world: default border until the initialization packet.
        self.border = rc_physics::WorldBorder::default();
        self.bolts.clear();
        self.sky_flash = 0;
    }

    /// Border packet (§ 4.1).
    pub fn on_border(&mut self, p: rc_proto_v777::border::BorderPacket) {
        use rc_proto_v777::border::BorderPacket as P;
        let b = &mut self.border;
        match p {
            P::Init { x, z, old_size, new_size, ticks, max_size, warn_blocks, warn_ticks } => {
                b.init(x, z, old_size, new_size, ticks, max_size, warn_blocks, warn_ticks)
            }
            P::Center { x, z } => b.set_center(x, z),
            P::Lerp { old_size, new_size, ticks } => b.lerp(old_size, new_size, ticks),
            P::Size(s) => b.set_size(s),
            P::WarningDelay(t) => b.warn_ticks = t,
            P::WarningDistance(d) => b.warn_blocks = d,
        }
    }

    /// Check line (`RUSTCRAFT_WEATHERLOG`): height and precipitation at the camera block.
    pub fn debug_line(&self, w: &WorldView<'_>, eye: [f64; 3]) -> String {
        let b = eye.map(|v| v.floor() as i32);
        let h = self.heights.get(b[0], b[2], self.climate.sea_level);
        let p = self.climate.at(w, b[0], b[1], b[2]);
        let (n, s) = self.build_stats;
        let ms = if n > 0 { s * 1e3 / f64::from(n) } else { 0.0 };
        format!(
            "weather: block {b:?}, height {h}, precipitation {p:?}, {n} column builds ({ms:.3} ms on average)"
        )
    }

    pub fn on_chunk(&mut self, col: &rc_world::Column, b: &Blocking<'_>) {
        self.rev += 1;
        self.heights.on_chunk(col, b);
    }

    pub fn forget(&mut self, x: i32, z: i32) {
        self.rev += 1;
        self.heights.forget(x, z);
    }

    /// Block changes of a section (section indices, after they are applied).
    pub fn on_blocks(&mut self, w: &WorldView<'_>, base: [i32; 3], changes: &[(u16, u32)], b: &Blocking<'_>) {
        self.rev += 1;
        for &(i, s) in changes {
            let i = i32::from(i);
            let pos = [base[0] + (i & 15), base[1] + (i >> 8), base[2] + ((i >> 4) & 15)];
            self.heights.on_block(w, pos, s, b);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Values recorded on the game (probe outside the repository, public API): point, `T`, `B`, stack `F`
    /// (float bits).
    #[test]
    fn noises_match_game_values() {
        let pts: [(f64, f64, i32, i32, i32); 6] = [
            (0.0, 0.0, 0, 0, 0),
            (153.8125, 3.75, -1096907821, 1044601049, 1040188135),
            (-17.25, 93.75, 1063521153, 1054891352, -1093238174),
            (150.125, -3.5, 1055340843, 1054988726, -1116919085),
            (154320.875, -956790.125, 1055540991, -1092241773, 1043682922),
            (60.05, 1.5, -1094011404, 1058022017, 1043909660),
        ];
        let c = climate();
        for (u, v, t, b, f) in pts {
            assert_eq!(c.t.get(u, v).to_bits() as i32, t, "T({u}, {v})");
            assert_eq!(c.b.get(u, v).to_bits() as i32, b, "B({u}, {v})");
            assert_eq!(c.frozen(u, v).to_bits() as i32, f, "F({u}, {v})");
        }
        let mut g = Lcg::new(99);
        assert_eq!(g.gaussian().to_bits() as i64, 4608510154852676553);
        assert_eq!(g.gaussian().to_bits() as i64, -4617188881996588056);
    }

    #[test]
    fn gaussian_pairs_share_one_draw() {
        let mut a = Lcg::new(42);
        let (g1, g2) = (a.gaussian(), a.gaussian());
        let mut b = Lcg::new(42);
        b.gaussian();
        assert_eq!(b.gauss, Some(g2));
        assert!(g1.is_finite() && g2.is_finite());
    }
}
