//! Aquifers (spec generation-terrain § 3.2–3.3): global fluid picker, noise aquifers on
//! 16 × 12 × 16 cells (level, water/lava type, barriers between neighbouring aquifers).

use hashbrown::HashMap;

use crate::Generator;
use crate::math::floor_div;
use crate::palette::{AIR, BlockId};
use crate::rng::Positional;
use crate::sampler::{Ctx, Sid};
use crate::volume::Volume;

/// "Very low" level: no fluid.
pub const WAY_BELOW_MIN_Y: i32 = -32512;

/// Fluid status: `kind` under `level`, air above.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FluidStatus {
    pub level: i32,
    pub kind: BlockId,
}

impl FluidStatus {
    #[inline]
    pub fn at(self, y: i32) -> BlockId {
        if y < self.level { self.kind } else { AIR }
    }
}

/// Global picker: lava under `min(−54, sea)`, default fluid under the sea.
#[derive(Debug, Clone, Copy)]
pub struct FluidPicker {
    pub lava: FluidStatus,
    pub sea: FluidStatus,
}

impl FluidPicker {
    pub fn new(sea_level: i32, default_fluid: BlockId, lava: BlockId) -> Self {
        FluidPicker {
            lava: FluidStatus { level: -54, kind: lava },
            sea: FluidStatus { level: sea_level, kind: default_fluid },
        }
    }

    #[inline]
    pub fn at(&self, y: i32) -> FluidStatus {
        if y < (-54).min(self.sea.level) { self.lava } else { self.sea }
    }
}

/// Functions and randomness of the aquifers (shared by all the chunks).
#[derive(Debug, Clone, Copy)]
pub struct AquiferConfig {
    pub barrier: Sid,
    pub floodedness: Sid,
    pub spread: Sid,
    pub lava: Sid,
    pub exclusion: Sid,
    pub surface_level: Sid,
    pub random: Positional,
}

#[inline]
fn grid_x(b: i32) -> i32 {
    b >> 4
}
#[inline]
fn grid_y(b: i32) -> i32 {
    floor_div(b, 12)
}
#[inline]
fn similarity(d1: i32, d2: i32) -> f64 {
    1.0 - f64::from(d2.wrapping_sub(d1)) / 25.0
}

const SURFACE_OFFSETS: [(i32, i32); 13] =
    [(0, 0), (-2, -1), (-1, -1), (0, -1), (1, -1), (-3, 0), (-2, 0), (-1, 0), (1, 0), (-2, 1), (-1, 1), (0, 1), (1, 1)];

/// Cache of the preliminary surface heights per quart: dense window around the chunk
/// (everything the aquifer reads) and table fallback for the rest.
#[derive(Debug)]
struct SurfaceCache {
    qx0: i32,
    qz0: i32,
    dense: Vec<i32>,
    far: HashMap<(i32, i32), i32>,
}

const SC_W: i32 = 48;
const EMPTY: i32 = i32::MIN;

impl SurfaceCache {
    fn new(min_x: i32, min_z: i32) -> Self {
        SurfaceCache {
            qx0: (min_x >> 2) - 24,
            qz0: (min_z >> 2) - 20,
            dense: vec![EMPTY; (SC_W * SC_W) as usize],
            far: HashMap::new(),
        }
    }

    #[inline]
    fn slot(&self, qx: i32, qz: i32) -> Option<usize> {
        let (a, b) = (qx - self.qx0, qz - self.qz0);
        (a >= 0 && b >= 0 && a < SC_W && b < SC_W).then(|| (a + b * SC_W) as usize)
    }

    fn get(&self, qx: i32, qz: i32) -> Option<i32> {
        match self.slot(qx, qz) {
            Some(i) => (self.dense[i] != EMPTY).then_some(self.dense[i]),
            None => self.far.get(&(qx, qz)).copied(),
        }
    }

    fn put(&mut self, qx: i32, qz: i32, v: i32) {
        match self.slot(qx, qz) {
            Some(i) => self.dense[i] = v,
            None => {
                self.far.insert((qx, qz), v);
            }
        }
    }
}

/// Noise aquifer of a chunk (cell caches private to the chunk).
#[derive(Debug)]
pub struct NoiseAquifer {
    cfg: AquiferConfig,
    picker: FluidPicker,
    lava_id: BlockId,
    water_id: BlockId,
    min_gx: i32,
    min_gy: i32,
    min_gz: i32,
    size_x: i32,
    size_z: i32,
    /// Cell centres (x, y, z), `valid` false until drawn.
    loc: Vec<[i32; 3]>,
    loc_valid: Vec<bool>,
    status: Vec<Option<FluidStatus>>,
    skip_above: i32,
    surface: SurfaceCache,
    /// Column window (`compute_fill`): key (x, z, y cell), indices of the 12
    /// neighbouring cells in the generic walk order, dx² + dz² of their centre and y of the centre.
    win_key: (i32, i32, i32),
    win_idx: [usize; 12],
    win_dxz: [i32; 12],
    win_ly: [i32; 12],
}

/// 4 nearest out of 12 in the generic walk order (insertion with `>=`: at equal
/// distance, the cell seen last goes first). Key = (distance, 15 − rank): the 4
/// smallest keys, sorted, give the same result. Branchless comparison network: three
/// sorts of 4, then two bitonic merges keeping the 4 smallest.
#[inline]
fn nearest4_net(nd: &[i32; 12]) -> [i64; 4] {
    #[inline(always)]
    fn cs(a: &mut [i64; 4], i: usize, j: usize) {
        let (x, y) = (a[i], a[j]);
        a[i] = x.min(y);
        a[j] = x.max(y);
    }
    #[inline(always)]
    fn sort4(a: &mut [i64; 4]) {
        cs(a, 0, 1);
        cs(a, 2, 3);
        cs(a, 0, 2);
        cs(a, 1, 3);
        cs(a, 1, 2);
    }
    #[inline(always)]
    fn merge(a: &[i64; 4], b: &[i64; 4]) -> [i64; 4] {
        let mut c = [a[0].min(b[3]), a[1].min(b[2]), a[2].min(b[1]), a[3].min(b[0])];
        cs(&mut c, 0, 2);
        cs(&mut c, 1, 3);
        cs(&mut c, 0, 1);
        cs(&mut c, 2, 3);
        c
    }
    let key = |k: usize| (i64::from(nd[k]) << 4) | (15 - k as i64);
    let mut g: [[i64; 4]; 3] = std::array::from_fn(|q| std::array::from_fn(|i| key(q * 4 + i)));
    for a in &mut g {
        sort4(a);
    }
    merge(&merge(&g[0], &g[1]), &g[2])
}

/// Aquifer of a chunk: noise-based, or disabled (global fluid under the zero density).
#[derive(Debug)]
pub enum Aquifer {
    Disabled(FluidPicker),
    Noise(Box<NoiseAquifer>),
}

impl Aquifer {
    /// Creates the aquifer of volume `v` (samples the surface maximum first, like the game).
    pub fn new(
        g: &Generator,
        ctx: &mut Ctx,
        cfg: Option<AquiferConfig>,
        picker: FluidPicker,
        fluids: (BlockId, BlockId),
        v: &Volume,
    ) -> Self {
        let Some(cfg) = cfg else { return Aquifer::Disabled(picker) };
        let (lava_id, water_id) = fluids;
        let min_gx = grid_x(v.x0 - 5);
        let max_gx = grid_x(v.max_x() - 5) + 1;
        let min_gy = grid_y(v.y0 + 1) - 1;
        let max_gy = grid_y(v.max_y() + 1) + 1;
        let min_gz = grid_x(v.z0 - 5);
        let max_gz = grid_x(v.max_z() - 5) + 1;
        let (size_x, size_y, size_z) = (max_gx - min_gx + 1, max_gy - min_gy + 1, max_gz - min_gz + 1);
        let total = (size_x * size_y * size_z) as usize;
        let mut a = NoiseAquifer {
            cfg,
            picker,
            lava_id,
            water_id,
            min_gx,
            min_gy,
            min_gz,
            size_x,
            size_z,
            loc: vec![[0; 3]; total],
            loc_valid: vec![false; total],
            status: vec![None; total],
            skip_above: 0,
            surface: SurfaceCache::new(v.x0, v.z0),
            win_key: (i32::MIN, i32::MIN, i32::MIN),
            win_idx: [0; 12],
            win_dxz: [0; 12],
            win_ly: [0; 12],
        };
        let max_surface = a.max_surface_level(g, ctx, min_gx << 4, min_gz << 4, (max_gx << 4) + 9, (max_gz << 4) + 9);
        let adjusted = max_surface.wrapping_add(8);
        let skip_gy = grid_y(adjusted + 12) + 1;
        a.skip_above = skip_gy * 12 + 11 - 1;
        Aquifer::Noise(Box::new(a))
    }

    /// Substance at the block for a density (§ 3.3): `None` = solid (default block or barrier),
    /// and the "fluid update requested" flag.
    #[inline]
    pub fn compute(
        &mut self,
        g: &Generator,
        ctx: &mut Ctx,
        x: i32,
        y: i32,
        z: i32,
        density: f64,
    ) -> (Option<BlockId>, bool) {
        match self {
            Aquifer::Disabled(p) => {
                if density > 0.0 {
                    (None, false)
                } else {
                    (Some(p.at(y).at(y)), false)
                }
            }
            Aquifer::Noise(a) => a.compute(g, ctx, x, y, z, density),
        }
    }

    /// Like [`Self::compute`], for a column walk (x, z fixed, y variable): the 12
    /// neighbouring cells and their dx² + dz² are kept as long as x, z and the y cell do not
    /// change; `net` picks the 4 nearest through the branchless network. Identical result.
    #[allow(clippy::too_many_arguments)]
    #[inline]
    pub fn compute_column(
        &mut self,
        g: &Generator,
        ctx: &mut Ctx,
        x: i32,
        y: i32,
        z: i32,
        density: f64,
        net: bool,
    ) -> (Option<BlockId>, bool) {
        match self {
            Aquifer::Disabled(_) => self.compute(g, ctx, x, y, z, density),
            Aquifer::Noise(a) => a.compute_window(g, ctx, x, y, z, density, net),
        }
    }
}

impl NoiseAquifer {
    fn max_surface_level(&mut self, g: &Generator, ctx: &mut Ctx, x0: i32, z0: i32, x1: i32, z1: i32) -> i32 {
        let (qx0, qx1, qz0, qz1) = (x0 >> 2, x1 >> 2, z0 >> 2, z1 >> 2);
        let v = Volume::stepped(qx1 - qx0 + 1, 1, qz1 - qz0 + 1, qx0 << 2, 0, qz0 << 2, 4, 1, 4);
        let buf = g.volume(self.cfg.surface_level, ctx, &v);
        let mut max = i32::MIN;
        for z in 0..v.sz {
            for x in 0..v.sx {
                let s = crate::math::floor_f(buf[v.index(x, 0, z)]);
                self.surface.put(v.bx(x) >> 2, v.bz(z) >> 2, s);
                max = max.max(s);
            }
        }
        max
    }

    fn surface_level(&mut self, g: &Generator, ctx: &mut Ctx, x: i32, z: i32) -> i32 {
        let (qx, qz) = (x >> 2, z >> 2);
        if let Some(v) = self.surface.get(qx, qz) {
            return v;
        }
        let v = crate::math::floor_f(g.value(self.cfg.surface_level, ctx, qx << 2, 0, qz << 2));
        self.surface.put(qx, qz, v);
        v
    }

    #[inline]
    fn index(&self, gx: i32, gy: i32, gz: i32) -> usize {
        (((gy - self.min_gy) * self.size_z + (gz - self.min_gz)) * self.size_x + (gx - self.min_gx)) as usize
    }

    fn compute(
        &mut self,
        g: &Generator,
        ctx: &mut Ctx,
        x: i32,
        y: i32,
        z: i32,
        density: f64,
    ) -> (Option<BlockId>, bool) {
        if let Some(r) = self.early(y, density) {
            return r;
        }
        let (ax, ay, az) = (grid_x(x - 5), grid_y(y + 1), grid_x(z - 5));
        let mut d = [i32::MAX; 4];
        let mut c = [0usize; 4];
        for ox in 0..=1 {
            for oy in -1..=1 {
                for oz in 0..=1 {
                    let (gx, gy, gz) = (ax + ox, ay + oy, az + oz);
                    let i = self.center(gx, gy, gz);
                    let [lx, ly, lz] = self.loc[i];
                    let (dx, dy, dz) = (lx - x, ly - y, lz - z);
                    let nd = dx * dx + dy * dy + dz * dz;
                    insert4(&mut d, &mut c, nd, i);
                }
            }
        }
        self.resolve(g, ctx, x, y, z, density, d, c)
    }

    /// Common early exits: positive density, above the aquifers, global lava.
    #[inline]
    fn early(&self, y: i32, density: f64) -> Option<(Option<BlockId>, bool)> {
        if density > 0.0 {
            return Some((None, false));
        }
        let global = self.picker.at(y);
        if y > self.skip_above {
            return Some((Some(global.at(y)), false));
        }
        if global.at(y) == self.lava_id && self.lava_id != AIR {
            return Some((Some(self.lava_id), false));
        }
        None
    }

    /// Index of the cell, centre drawn as needed (pure: depends only on the cell).
    #[inline]
    fn center(&mut self, gx: i32, gy: i32, gz: i32) -> usize {
        let i = self.index(gx, gy, gz);
        if !self.loc_valid[i] {
            let mut r = self.cfg.random.at(gx, gy, gz);
            let lx = (gx << 4) + r.next_int_bounded(10);
            let ly = gy * 12 + r.next_int_bounded(9);
            let lz = (gz << 4) + r.next_int_bounded(10);
            self.loc[i] = [lx, ly, lz];
            self.loc_valid[i] = true;
        }
        i
    }

    #[allow(clippy::too_many_arguments)]
    fn compute_window(
        &mut self,
        g: &Generator,
        ctx: &mut Ctx,
        x: i32,
        y: i32,
        z: i32,
        density: f64,
        net: bool,
    ) -> (Option<BlockId>, bool) {
        if let Some(r) = self.early(y, density) {
            return r;
        }
        let ay = grid_y(y + 1);
        if self.win_key != (x, z, ay) {
            let (ax, az) = (grid_x(x - 5), grid_x(z - 5));
            let mut k = 0;
            for ox in 0..=1 {
                for oy in -1..=1 {
                    for oz in 0..=1 {
                        let i = self.center(ax + ox, ay + oy, az + oz);
                        let [lx, ly, lz] = self.loc[i];
                        let (dx, dz) = (lx - x, lz - z);
                        self.win_idx[k] = i;
                        self.win_dxz[k] = dx * dx + dz * dz;
                        self.win_ly[k] = ly;
                        k += 1;
                    }
                }
            }
            self.win_key = (x, z, ay);
        }
        let nd: [i32; 12] = std::array::from_fn(|k| {
            let dy = self.win_ly[k] - y;
            self.win_dxz[k] + dy * dy
        });
        let mut d = [i32::MAX; 4];
        let mut c = [0usize; 4];
        if net {
            let best = nearest4_net(&nd);
            for j in 0..4 {
                d[j] = (best[j] >> 4) as i32;
                c[j] = self.win_idx[(15 - (best[j] & 15)) as usize];
            }
        } else {
            for (k, &v) in nd.iter().enumerate() {
                insert4(&mut d, &mut c, v, self.win_idx[k]);
            }
        }
        self.resolve(g, ctx, x, y, z, density, d, c)
    }

    /// Rest of the computation once the 4 nearest cells are known (increasing distances).
    #[allow(clippy::too_many_arguments)]
    fn resolve(
        &mut self,
        g: &Generator,
        ctx: &mut Ctx,
        x: i32,
        y: i32,
        z: i32,
        density: f64,
        d: [i32; 4],
        c: [usize; 4],
    ) -> (Option<BlockId>, bool) {
        let flowing = similarity(100, 144);
        let s1 = self.status_of(g, ctx, c[0]);
        let sim12 = similarity(d[0], d[1]);
        let fluid = s1.at(y);
        if sim12 <= 0.0 {
            let update = if sim12 >= flowing { s1 != self.status_of(g, ctx, c[1]) } else { false };
            return (Some(fluid), update);
        }
        if fluid == self.water_id
            && fluid != AIR
            && self.picker.at(y - 1).at(y - 1) == self.lava_id
            && self.lava_id != AIR
        {
            return (Some(fluid), true);
        }
        let mut barrier = f64::NAN;
        let s2 = self.status_of(g, ctx, c[1]);
        let b12 = sim12 * self.pressure(g, ctx, x, y, z, &mut barrier, s1, s2);
        if density + b12 > 0.0 {
            return (None, false);
        }
        let s3 = self.status_of(g, ctx, c[2]);
        let sim13 = similarity(d[0], d[2]);
        if sim13 > 0.0 {
            let b13 = sim12 * sim13 * self.pressure(g, ctx, x, y, z, &mut barrier, s1, s3);
            if density + b13 > 0.0 {
                return (None, false);
            }
        }
        let sim23 = similarity(d[1], d[2]);
        if sim23 > 0.0 {
            let b23 = sim12 * sim23 * self.pressure(g, ctx, x, y, z, &mut barrier, s2, s3);
            if density + b23 > 0.0 {
                return (None, false);
            }
        }
        let flow12 = s1 != s2;
        let flow23 = sim23 >= flowing && s2 != s3;
        let flow13 = sim13 >= flowing && s1 != s3;
        let update = if !flow12 && !flow23 && !flow13 {
            sim13 >= flowing && similarity(d[0], d[3]) >= flowing && s1 != self.status_of(g, ctx, c[3])
        } else {
            true
        };
        (Some(fluid), update)
    }

    #[allow(clippy::too_many_arguments)]
    fn pressure(
        &self,
        g: &Generator,
        ctx: &mut Ctx,
        x: i32,
        y: i32,
        z: i32,
        barrier: &mut f64,
        a: FluidStatus,
        b: FluidStatus,
    ) -> f64 {
        let (ta, tb) = (a.at(y), b.at(y));
        let lava_water = |p: BlockId, q: BlockId| p == self.lava_id && q == self.water_id && p != AIR && q != AIR;
        if lava_water(ta, tb) || lava_water(tb, ta) {
            return 2.0;
        }
        let diff = (a.level - b.level).abs();
        if diff == 0 {
            return 0.0;
        }
        let avg = 0.5 * f64::from(a.level + b.level);
        let above = f64::from(y) + 0.5 - avg;
        let base = f64::from(diff) / 2.0;
        let toward = base - above.abs();
        let gradient = if above > 0.0 {
            let c = toward;
            if c > 0.0 { c / 1.5 } else { c / 2.5 }
        } else {
            let c = 3.0 + toward;
            if c > 0.0 { c / 3.0 } else { c / 10.0 }
        };
        let noise = if (-2.0..=2.0).contains(&gradient) {
            if barrier.is_nan() {
                *barrier = f64::from(g.value(self.cfg.barrier, ctx, x, y, z));
            }
            *barrier
        } else {
            0.0
        };
        2.0 * (noise + gradient)
    }

    fn status_of(&mut self, g: &Generator, ctx: &mut Ctx, i: usize) -> FluidStatus {
        if let Some(s) = self.status[i] {
            return s;
        }
        let [x, y, z] = self.loc[i];
        let s = self.compute_fluid(g, ctx, x, y, z);
        self.status[i] = Some(s);
        s
    }

    fn compute_fluid(&mut self, g: &Generator, ctx: &mut Ctx, x: i32, y: i32, z: i32) -> FluidStatus {
        let global = self.picker.at(y);
        let mut lowest = i32::MAX;
        let top = y + 12;
        let bottom = y - 12;
        let mut center_under = false;
        for (ox, oz) in SURFACE_OFFSETS {
            let (sx, sz) = (x + (ox << 4), z + (oz << 4));
            let s = self.surface_level(g, ctx, sx, sz);
            let adj = s + 8;
            let start = ox == 0 && oz == 0;
            if start && bottom > adj {
                return global;
            }
            let pokes = top > adj;
            if pokes || start {
                let at_surface = self.picker.at(adj);
                if at_surface.at(adj) != AIR {
                    if start {
                        center_under = true;
                    }
                    if pokes {
                        return at_surface;
                    }
                }
            }
            lowest = lowest.min(s);
        }
        let level = self.surface_fluid_level(g, ctx, x, y, z, global, lowest, center_under);
        FluidStatus { level, kind: self.fluid_type(g, ctx, x, y, z, global, level) }
    }

    #[allow(clippy::too_many_arguments)]
    fn surface_fluid_level(
        &self,
        g: &Generator,
        ctx: &mut Ctx,
        x: i32,
        y: i32,
        z: i32,
        global: FluidStatus,
        lowest: i32,
        center_under: bool,
    ) -> i32 {
        let (partial, full) = if g.value(self.cfg.exclusion, ctx, x, y, z) > 0.0 {
            (-1.0, -1.0)
        } else {
            let below = (lowest + 8) - y;
            let factor = if center_under { clamped_map(f64::from(below), 0.0, 64.0, 1.0, 0.0) } else { 0.0 };
            let noise = f64::from(g.value(self.cfg.floodedness, ctx, x, y, z)).clamp(-1.0, 1.0);
            let full_t = map(factor, 1.0, 0.0, -0.3, 0.8);
            let part_t = map(factor, 1.0, 0.0, -0.8, 0.4);
            (noise - part_t, noise - full_t)
        };
        if full > 0.0 {
            global.level
        } else if partial > 0.0 {
            let (cx, cy, cz) = (floor_div(x, 16), floor_div(y, 40), floor_div(z, 16));
            let middle = cy * 40 + 20;
            let spread = f64::from(g.value(self.cfg.spread, ctx, cx, cy, cz) * 10.0);
            let q = (spread / 3.0).floor() as i32 * 3;
            lowest.min(middle + q)
        } else {
            WAY_BELOW_MIN_Y
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn fluid_type(
        &self,
        g: &Generator,
        ctx: &mut Ctx,
        x: i32,
        y: i32,
        z: i32,
        global: FluidStatus,
        level: i32,
    ) -> BlockId {
        if level <= -10 && level != WAY_BELOW_MIN_Y && global.kind != self.lava_id {
            let (cx, cy, cz) = (floor_div(x, 64), floor_div(y, 40), floor_div(z, 64));
            if f64::from(g.value(self.cfg.lava, ctx, cx, cy, cz)).abs() > 0.3 {
                return self.lava_id;
            }
        }
        global.kind
    }
}

/// Insertion into the 4 nearest (game order: `>=`, the new cell goes first at
/// equal distance).
#[inline(always)]
fn insert4(d: &mut [i32; 4], c: &mut [usize; 4], nd: i32, i: usize) {
    if d[0] >= nd {
        *c = [i, c[0], c[1], c[2]];
        *d = [nd, d[0], d[1], d[2]];
    } else if d[1] >= nd {
        *c = [c[0], i, c[1], c[2]];
        *d = [d[0], nd, d[1], d[2]];
    } else if d[2] >= nd {
        c[3] = c[2];
        c[2] = i;
        d[3] = d[2];
        d[2] = nd;
    } else if d[3] >= nd {
        c[3] = i;
        d[3] = nd;
    }
}

#[inline]
fn inverse_lerp(v: f64, a: f64, b: f64) -> f64 {
    (v - a) / (b - a)
}

#[inline]
fn lerp(t: f64, a: f64, b: f64) -> f64 {
    a + t * (b - a)
}

#[inline]
pub(crate) fn map(v: f64, a: f64, b: f64, c: f64, d: f64) -> f64 {
    lerp(inverse_lerp(v, a, b), c, d)
}

#[inline]
pub(crate) fn clamped_map(v: f64, a: f64, b: f64, c: f64, d: f64) -> f64 {
    let t = inverse_lerp(v, a, b);
    if t < 0.0 {
        c
    } else if t > 1.0 {
        d
    } else {
        lerp(t, c, d)
    }
}

#[cfg(test)]
mod tests {
    use super::{insert4, nearest4_net};

    /// Network against insertion: distances drawn in small ranges (many ties)
    /// and large ones; indices = rank. Fails if the tie-breaking order changes.
    #[test]
    fn net_matches_insertion() {
        let mut s = 0x1234_5678_9ABC_DEF0u64;
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        let mut ties = 0;
        for n in 0..200_000 {
            let span = [3u64, 6, 40, 3000][n % 4];
            let nd: [i32; 12] = std::array::from_fn(|_| (next() % span) as i32);
            let (mut d, mut c) = ([i32::MAX; 4], [0usize; 4]);
            for (k, &v) in nd.iter().enumerate() {
                insert4(&mut d, &mut c, v, k);
            }
            let best = nearest4_net(&nd);
            let got_d: [i32; 4] = std::array::from_fn(|j| (best[j] >> 4) as i32);
            let got_c: [usize; 4] = std::array::from_fn(|j| (15 - (best[j] & 15)) as usize);
            assert_eq!((got_d, got_c), (d, c), "{nd:?}");
            ties += usize::from(d[0] == d[1] || d[1] == d[2] || d[2] == d[3]);
        }
        assert!(ties > 50_000, "too few ties ({ties})");
    }
}
