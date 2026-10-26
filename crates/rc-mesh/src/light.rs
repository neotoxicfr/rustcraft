//! Light at meshing: reading of the received levels (spec `docs/specs/lighting.md` § 1.3) and
//! per-vertex light, smoothed with ambient occlusion or flat (§ 2).
//!
//! Light packed into 16 bits: low byte = block × 16 (0–240), high byte = sky × 16.

use rc_assets::geom::Dir;
use rc_data::{VersionData, flags, light_bits};
use rc_world::Column;

use crate::wire::MeshJob;

/// Light properties of a state (bits of [`info`]).
pub mod info {
    /// Emission (0–15) on the 4 low bits.
    pub const EMISSION: u16 = 0xf;
    pub const EMISSIVE: u16 = 1 << 4;
    pub const DARK_SHADE: u16 = 1 << 5;
    pub const FULL_COLLISION: u16 = 1 << 6;
    pub const NO_DAMPENING: u16 = 1 << 7;
    pub const SOLID_RENDER: u16 = 1 << 8;
}

/// Table of light properties per state.
pub fn state_info(d: &VersionData) -> Vec<u16> {
    d.states
        .iter()
        .map(|s| {
            let mut v = u16::from(s.light_emission.min(15));
            if s.light & light_bits::EMISSIVE != 0 {
                v |= info::EMISSIVE;
            }
            if s.light & light_bits::DARK_SHADE != 0 {
                v |= info::DARK_SHADE;
            }
            if s.has(flags::FULL_COLLISION) {
                v |= info::FULL_COLLISION;
            }
            if s.light_dampening == 0 {
                v |= info::NO_DAMPENING;
            }
            if s.has(flags::SOLID_RENDER) {
                v |= info::SOLID_RENDER;
            }
            v
        })
        .collect()
}

/// Reading of the light levels around the column to mesh.
pub struct LightView<'a> {
    pub job: &'a MeshJob,
    pub info: &'a [u16],
    /// Columns from -1 to +1 in (x, z) around the centre, index (dz + 1) · 3 + dx + 1.
    cols: [Option<&'a Column>; 9],
}

#[inline]
fn nibble(layer: &[u8; rc_world::LIGHT_BYTES], x: usize, y: usize, z: usize) -> u8 {
    let i = (y << 8) | (z << 4) | x;
    (layer[i >> 1] >> ((i & 1) * 4)) & 15
}

impl<'a> LightView<'a> {
    pub fn new(job: &'a MeshJob, info: &'a [u16]) -> Self {
        let cols = std::array::from_fn(|i| {
            let (dx, dz) = (i as i32 % 3 - 1, i as i32 / 3 - 1);
            crate::blocks::locate(job, dx * 16, dz * 16).map(|(c, _, _)| c)
        });
        LightView { job, info, cols }
    }

    /// Layers of a section (index `idx` in the light arrays, 0 = below the world) of a
    /// column: block, and where to read the sky from. Section without data (no array received): block 0;
    /// sky read from the bottom layer of the first section above that has some, 15 if there is
    /// none; dimension without sky: 0.
    fn slot(&self, c: Option<&'a Column>, idx: i32) -> Slot<'a> {
        fn layer(l: &[rc_world::LightLayer], i: i32) -> Option<&[u8; rc_world::LIGHT_BYTES]> {
            usize::try_from(i).ok().and_then(|i| l.get(i)).and_then(|v| v.as_deref())
        }
        let Some(c) = c else { return Slot { block: None, sky: Sky::Level(if self.job.has_sky { 15 } else { 0 }) } };
        let block = layer(&c.block_light, idx);
        if !self.job.has_sky {
            return Slot { block, sky: Sky::Level(0) };
        }
        let sky = match layer(&c.sky_light, idx) {
            Some(l) => Sky::Layer(l),
            None => {
                let above = (idx.max(-1) + 1..c.sky_light.len() as i32).find_map(|i| layer(&c.sky_light, i));
                above.map_or(Sky::Level(15), Sky::Bottom)
            }
        };
        Slot { block, sky }
    }

    /// Raw levels (block, sky) at (x, y, z), y relative to the bottom of the world.
    pub fn raw(&self, x: i32, y: i32, z: i32) -> (u8, u8) {
        let (cx, cz) = (x.div_euclid(16), z.div_euclid(16));
        let col = if (-1..=1).contains(&cx) && (-1..=1).contains(&cz) {
            self.cols[((cz + 1) * 3 + cx + 1) as usize]
        } else {
            None
        };
        self.slot(col, y.div_euclid(16) + 1).read(
            x.rem_euclid(16) as usize,
            y.rem_euclid(16) as usize,
            z.rem_euclid(16) as usize,
        )
    }

    /// Packed light of a position read with state `s` (§ 2.1): emissive render → full
    /// light; otherwise block raised to the state's emission.
    pub fn lum(&self, x: i32, y: i32, z: i32, s: u32) -> u16 {
        let inf = self.info.get(s as usize).copied().unwrap_or(0);
        if inf & info::EMISSIVE != 0 {
            return 0xf0f0;
        }
        pack(self.raw(x, y, z), inf)
    }
}

/// Packed light of the raw levels, block raised to the state's emission.
#[inline]
fn pack((b, k): (u8, u8), inf: u16) -> u16 {
    let b = b.max((inf & info::EMISSION) as u8);
    u16::from(b) << 4 | u16::from(k) << 12
}

/// Sky source of a section: its layer, the bottom layer of a section above, or a level.
#[derive(Clone, Copy)]
enum Sky<'a> {
    Layer(&'a [u8; rc_world::LIGHT_BYTES]),
    Bottom(&'a [u8; rc_world::LIGHT_BYTES]),
    Level(u8),
}

/// Resolved layers of a section of a column.
#[derive(Clone, Copy)]
struct Slot<'a> {
    block: Option<&'a [u8; rc_world::LIGHT_BYTES]>,
    sky: Sky<'a>,
}

impl Slot<'_> {
    const EMPTY: Slot<'static> = Slot { block: None, sky: Sky::Level(0) };

    #[inline]
    fn read(&self, x: usize, y: usize, z: usize) -> (u8, u8) {
        let block = self.block.map_or(0, |l| nibble(l, x, y, z));
        let sky = match self.sky {
            Sky::Layer(l) => nibble(l, x, y, z),
            Sky::Bottom(l) => nibble(l, x, 0, z),
            Sky::Level(v) => v,
        };
        (block, sky)
    }
}

/// Smoothed light paths (same output bit for bit; `RUSTCRAFT_OPT_AO*` switches read
/// at startup, set in the task to compare the paths in one process).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LightPaths {
    /// A single cell read returns light and properties (`RUSTCRAFT_OPT_AOCELL`).
    pub cell: bool,
    /// Corners of a face memorised per (direction, glued) during the block (`RUSTCRAFT_OPT_AOCORNERS`).
    pub corners: bool,
    /// Geometry of each baked face precomputed: planar, at the edge, full, corner carrying each
    /// vertex, bilinear weights (`RUSTCRAFT_OPT_AOFACETAB`).
    pub facetab: bool,
    /// Cache cells kept by the thread, generation continued from one task to the next, allocated at
    /// first need (`RUSTCRAFT_OPT_AOSCRATCH`).
    pub scratch: bool,
    /// Cells of 6 B instead of 16 (`RUSTCRAFT_OPT_AOCELL8`).
    pub cell8: bool,
    /// Fluids read through the section cache, flat light through its resolved layers
    /// (`RUSTCRAFT_OPT_AOFLUID`).
    pub fluid: bool,
    /// 3 × 3 × 3 neighbourhood of the block memorised at the first read (`RUSTCRAFT_OPT_AONEIGH`).
    pub neigh: bool,
}

impl LightPaths {
    pub const ALL: LightPaths =
        LightPaths { cell: true, corners: true, facetab: true, scratch: true, cell8: true, fluid: true, neigh: true };
    pub const OLD: LightPaths = LightPaths {
        cell: false,
        corners: false,
        facetab: false,
        scratch: false,
        cell8: false,
        fluid: false,
        neigh: false,
    };
    /// Paths active by default: D + A + B, best combination proven in the micro-bench. C, E, F
    /// and G showed no gain (C loses once combined): turned off until the calm-machine bench,
    /// see `docs/perf.md`.
    pub const DEFAULT: LightPaths = LightPaths { cell: true, corners: true, scratch: true, ..Self::OLD };

    /// Paths chosen by the environment (`DEFAULT`; "0" turns off, "1" activates).
    pub fn from_env() -> Self {
        static P: std::sync::OnceLock<LightPaths> = std::sync::OnceLock::new();
        *P.get_or_init(|| {
            let d = Self::DEFAULT;
            let on = |v: Option<&std::ffi::OsStr>, def: bool| v.map_or(def, |v| v != "0");
            LightPaths {
                cell: on(rc_data::env_os!("RUSTCRAFT_OPT_AOCELL"), d.cell),
                corners: on(rc_data::env_os!("RUSTCRAFT_OPT_AOCORNERS"), d.corners),
                facetab: on(rc_data::env_os!("RUSTCRAFT_OPT_AOFACETAB"), d.facetab),
                scratch: on(rc_data::env_os!("RUSTCRAFT_OPT_AOSCRATCH"), d.scratch),
                cell8: on(rc_data::env_os!("RUSTCRAFT_OPT_AOCELL8"), d.cell8),
                fluid: on(rc_data::env_os!("RUSTCRAFT_OPT_AOFLUID"), d.fluid),
                neigh: on(rc_data::env_os!("RUSTCRAFT_OPT_AONEIGH"), d.neigh),
            }
        })
    }
}

/// Cache cell: light properties of the state and light (read with the cell's state), valid
/// for one generation.
pub trait CellKind: Copy + Default {
    /// Largest representable generation (beyond: cells emptied, back to 1).
    const MAX_EPOCH: u32;
    fn valid(&self, epoch: u32) -> bool;
    fn info(&self) -> u16;
    fn fill(&mut self, epoch: u32, state: u32, info: u16);
    fn lum(&self, epoch: u32) -> Option<u16>;
    fn set_lum(&mut self, epoch: u32, l: u16);
}

/// Original cell (16 B): 32-bit generations, state kept.
#[derive(Clone, Copy, Default)]
pub struct Wide {
    epoch: u32,
    lum_epoch: u32,
    state: u32,
    info: u16,
    lum: u16,
}

impl CellKind for Wide {
    const MAX_EPOCH: u32 = u32::MAX;
    #[inline]
    fn valid(&self, epoch: u32) -> bool {
        self.epoch == epoch
    }
    #[inline]
    fn info(&self) -> u16 {
        self.info
    }
    #[inline]
    fn fill(&mut self, epoch: u32, state: u32, info: u16) {
        self.state = state;
        self.info = info;
        self.epoch = epoch;
    }
    #[inline]
    fn lum(&self, epoch: u32) -> Option<u16> {
        (self.lum_epoch == epoch).then_some(self.lum)
    }
    #[inline]
    fn set_lum(&mut self, epoch: u32, l: u16) {
        self.lum = l;
        self.lum_epoch = epoch;
    }
}

/// Compact cell (6 B): 16-bit generation, absent light denoted by a value that no packed
/// light takes (at most 0xf0f0).
#[derive(Clone, Copy, Default)]
pub struct Narrow {
    epoch: u16,
    info: u16,
    lum: u16,
}

const NO_LUM: u16 = 0xffff;

impl CellKind for Narrow {
    const MAX_EPOCH: u32 = u16::MAX as u32;
    #[inline]
    fn valid(&self, epoch: u32) -> bool {
        u32::from(self.epoch) == epoch
    }
    #[inline]
    fn info(&self) -> u16 {
        self.info
    }
    #[inline]
    fn fill(&mut self, epoch: u32, _state: u32, info: u16) {
        self.info = info;
        self.lum = NO_LUM;
        self.epoch = epoch as u16;
    }
    #[inline]
    fn lum(&self, _epoch: u32) -> Option<u16> {
        (self.lum != NO_LUM).then_some(self.lum)
    }
    #[inline]
    fn set_lum(&mut self, _epoch: u32, l: u16) {
        self.lum = l;
    }
}

const CSIDE: i32 = 20;
const CELLS: usize = (CSIDE * CSIDE * CSIDE) as usize;

/// Cache cells and current generation: a cell is only valid if its generation is the
/// current one (emptying the cache = changing generation, without rewriting the cells). Kept by the thread
/// (`LightPaths::scratch`), they serve from one task to the next: the generation then continues
/// (resetting it to 1 would make the cells of a previous task valid).
pub struct CellStore<C> {
    epoch: u32,
    cells: Vec<C>,
}

impl<C> Default for CellStore<C> {
    fn default() -> Self {
        CellStore { epoch: 1, cells: Vec::new() }
    }
}

impl<C: CellKind> CellStore<C> {
    /// Cells allocated up front (original path: one allocation of 8,000 cells per task).
    fn filled() -> Self {
        CellStore { epoch: 1, cells: vec![C::default(); CELLS] }
    }
}

/// Cells kept by the thread, in both formats.
#[derive(Default)]
pub struct LightStores {
    wide: CellStore<Wide>,
    narrow: CellStore<Narrow>,
}

impl LightStores {
    /// Sets the current generation (tests of the wrap to the maximum).
    #[doc(hidden)]
    pub fn set_epoch(&mut self, e: u32) {
        self.wide.epoch = e;
        self.narrow.epoch = e.min(Narrow::MAX_EPOCH);
    }
}

/// Per-section cache of the lighting reads: properties and light (read with the cell's
/// state) of each cell of the section and of its two-block border, computed at the first
/// read; and memos of the current block (face corners, neighbourhood).
pub struct LightCache<'a, C: CellKind = Wide> {
    base_y: i32,
    store: CellStore<C>,
    /// Layers of the 3 × 3 columns × 3 sections (below, the section, above) that cover the
    /// cache, resolved at each section: index (dy + 1) · 9 + (dz + 1) · 3 + dx + 1.
    slots: [Slot<'a>; 27],
    paths: LightPaths,
    /// Block of the memos (`at`, none before the first block of the section); bit (direction · 2 +
    /// glued) of `corner_ok`: corners memorised.
    at: Option<[i32; 3]>,
    corner_ok: u16,
    corners: [Corners; 12],
    /// Neighbourhood of `at` (index (dy + 1) · 9 + (dz + 1) · 3 + dx + 1): (light, properties),
    /// bits of the known properties and lights.
    near_info: u32,
    near_lum: u32,
    near: [(u16, u16); 27],
}

/// Light and occlusion gray (0–1) of the corners A, B, C, D of a face.
type Corners = [(u16, f32); 4];

impl<'a, C: CellKind> LightCache<'a, C> {
    fn new(paths: LightPaths, store: CellStore<C>) -> Self {
        LightCache {
            base_y: 0,
            store,
            slots: [Slot::EMPTY; 27],
            paths,
            at: None,
            corner_ok: 0,
            corners: [[(0, 0.0); 4]; 12],
            near_info: 0,
            near_lum: 0,
            near: [(0, 0); 27],
        }
    }

    /// Empties the cache for the section whose bottom is at `base_y` (multiple of 16); `cells`: the
    /// cells will be used (allocated at first need).
    fn reset(&mut self, v: &LightView<'a>, base_y: i32, cells: bool) {
        self.base_y = base_y;
        let s = &mut self.store;
        s.epoch = s.epoch.wrapping_add(1);
        if s.epoch == 0 || s.epoch > C::MAX_EPOCH {
            s.cells.fill(C::default());
            s.epoch = 1;
        }
        if cells && s.cells.is_empty() {
            s.cells.resize(CELLS, C::default());
        }
        let sec = base_y.div_euclid(16);
        self.slots = std::array::from_fn(|i| v.slot(v.cols[i % 9], sec + (i / 9) as i32));
        self.at = None;
        self.corner_ok = 0;
        self.near_info = 0;
        self.near_lum = 0;
    }

    /// Raw levels of a cache cell (within the two-block border: x, z ∈ [−2, 18),
    /// y − bottom ∈ [−2, 18)), read from the resolved layers.
    #[inline]
    fn raw(&self, p: [i32; 3]) -> (u8, u8) {
        let (x, y, z) = (p[0] + 16, p[1] - self.base_y + 16, p[2] + 16);
        let s = &self.slots[((y >> 4) * 9 + (z >> 4) * 3 + (x >> 4)) as usize];
        s.read((x & 15) as usize, (y & 15) as usize, (z & 15) as usize)
    }

    #[inline]
    fn index(&self, p: [i32; 3]) -> Option<usize> {
        let (x, y, z) = (p[0] + 2, p[1] - self.base_y + 2, p[2] + 2);
        ((0..CSIDE).contains(&x) && (0..CSIDE).contains(&y) && (0..CSIDE).contains(&z))
            .then_some((y * CSIDE * CSIDE + z * CSIDE + x) as usize)
    }

    #[inline]
    fn fill(&mut self, v: &LightView<'_>, i: usize, p: [i32; 3], st: &impl Fn(i32, i32, i32) -> u32) -> &mut C {
        let epoch = self.store.epoch;
        let c = &mut self.store.cells[i];
        if !c.valid(epoch) {
            let s = st(p[0], p[1], p[2]);
            c.fill(epoch, s, v.info.get(s as usize).copied().unwrap_or(0));
        }
        c
    }

    /// Light properties of the cell's state (cache only).
    #[inline]
    fn info_c(&mut self, v: &LightView<'_>, p: [i32; 3], st: &impl Fn(i32, i32, i32) -> u32) -> u16 {
        match self.index(p) {
            Some(i) => self.fill(v, i, p, st).info(),
            None => v.info.get(st(p[0], p[1], p[2]) as usize).copied().unwrap_or(0),
        }
    }

    /// Light of the cell (cache only).
    #[inline]
    fn lum_c(&mut self, v: &LightView<'_>, p: [i32; 3], st: &impl Fn(i32, i32, i32) -> u32) -> u16 {
        self.cell_c(v, p, st).0
    }

    /// Light and properties of the cell in one read (cache only).
    #[inline]
    fn cell_c(&mut self, v: &LightView<'_>, p: [i32; 3], st: &impl Fn(i32, i32, i32) -> u32) -> (u16, u16) {
        let Some(i) = self.index(p) else {
            let s = st(p[0], p[1], p[2]);
            return (v.lum(p[0], p[1], p[2], s), v.info.get(s as usize).copied().unwrap_or(0));
        };
        let epoch = self.store.epoch;
        let c = *self.fill(v, i, p, st);
        if let Some(l) = c.lum(epoch) {
            return (l, c.info());
        }
        let inf = c.info();
        let l = if inf & info::EMISSIVE != 0 { 0xf0f0 } else { pack(self.raw(p), inf) };
        self.store.cells[i].set_lum(epoch, l);
        (l, inf)
    }

    /// Cell of the 3 × 3 × 3 neighbourhood of the memo block.
    #[inline]
    fn near_slot(&self, p: [i32; 3]) -> Option<usize> {
        let a = self.at?;
        let d = [p[0] - a[0] + 1, p[1] - a[1] + 1, p[2] - a[2] + 1];
        (d.iter().all(|&c| (0..3).contains(&c))).then(|| (d[1] * 9 + d[2] * 3 + d[0]) as usize)
    }

    /// Properties of the cell (neighbourhood memorised first if `neigh`).
    #[inline]
    fn info(&mut self, v: &LightView<'_>, p: [i32; 3], st: &impl Fn(i32, i32, i32) -> u32) -> u16 {
        if self.paths.neigh
            && let Some(k) = self.near_slot(p)
        {
            if self.near_info >> k & 1 == 0 {
                self.near[k].1 = self.info_c(v, p, st);
                self.near_info |= 1 << k;
            }
            return self.near[k].1;
        }
        self.info_c(v, p, st)
    }

    /// Light and properties of the cell (neighbourhood memorised first if `neigh`).
    #[inline]
    fn cell(&mut self, v: &LightView<'_>, p: [i32; 3], st: &impl Fn(i32, i32, i32) -> u32) -> (u16, u16) {
        if self.paths.neigh
            && let Some(k) = self.near_slot(p)
        {
            if self.near_lum >> k & 1 == 0 {
                self.near[k] = self.cell_c(v, p, st);
                self.near_lum |= 1 << k;
                self.near_info |= 1 << k;
            }
            return self.near[k];
        }
        self.cell_c(v, p, st)
    }

    /// Light of the cell.
    #[inline]
    fn lum(&mut self, v: &LightView<'_>, p: [i32; 3], st: &impl Fn(i32, i32, i32) -> u32) -> u16 {
        if self.paths.neigh {
            return self.cell(v, p, st).0;
        }
        self.lum_c(v, p, st)
    }

    /// Light of a position read with the given state `s` (flat path): resolved layers of the
    /// section if the position is in the cache, otherwise full read.
    #[inline]
    fn lum_as(&self, v: &LightView<'_>, p: [i32; 3], s: u32) -> u16 {
        if self.index(p).is_none() {
            return v.lum(p[0], p[1], p[2], s);
        }
        let inf = v.info.get(s as usize).copied().unwrap_or(0);
        if inf & info::EMISSIVE != 0 {
            return 0xf0f0;
        }
        pack(self.raw(p), inf)
    }

    /// Changes block for the memos.
    #[inline]
    fn begin_block(&mut self, p: [i32; 3]) {
        if self.at != Some(p) {
            self.at = Some(p);
            self.corner_ok = 0;
            self.near_info = 0;
            self.near_lum = 0;
        }
    }
}

/// Task cache in the chosen cell format (`LightPaths::cell8`).
pub enum AnyCache<'a> {
    Wide(LightCache<'a, Wide>),
    Narrow(LightCache<'a, Narrow>),
}

macro_rules! each {
    ($s:expr, $c:ident => $e:expr) => {
        match $s {
            AnyCache::Wide($c) => $e,
            AnyCache::Narrow($c) => $e,
        }
    };
}

impl<'a> AnyCache<'a> {
    /// Cache of a task: cells taken from `stores` (kept by the thread) if `paths.scratch`,
    /// otherwise allocated for the task.
    pub fn new(paths: LightPaths, stores: &mut LightStores) -> Self {
        if paths.cell8 {
            let s = if paths.scratch { std::mem::take(&mut stores.narrow) } else { CellStore::filled() };
            AnyCache::Narrow(LightCache::new(paths, s))
        } else {
            let s = if paths.scratch { std::mem::take(&mut stores.wide) } else { CellStore::filled() };
            AnyCache::Wide(LightCache::new(paths, s))
        }
    }

    /// Gives the cells back to the thread (`paths.scratch`).
    pub fn finish(self, stores: &mut LightStores) {
        match self {
            AnyCache::Wide(c) if c.paths.scratch => stores.wide = c.store,
            AnyCache::Narrow(c) if c.paths.scratch => stores.narrow = c.store,
            _ => {}
        }
    }

    /// Empties the cache for the section whose bottom is at `base_y`; `cells`: the cells will be used.
    pub fn reset(&mut self, v: &LightView<'a>, base_y: i32, cells: bool) {
        each!(self, c => c.reset(v, base_y, cells))
    }

    /// Light of a position read with its own state (fluids, `LightPaths::fluid`).
    pub fn lum(&mut self, v: &LightView<'_>, p: [i32; 3], st: &impl Fn(i32, i32, i32) -> u32) -> u16 {
        each!(self, c => c.lum(v, p, st))
    }

    /// [`quad_light`] in this cache.
    pub fn quad_light(
        &mut self,
        v: &LightView<'_>,
        st: &impl Fn(i32, i32, i32) -> u32,
        p: [i32; 3],
        s: u32,
        q: &QuadGeom<'_>,
        smooth: bool,
    ) -> Vertices {
        each!(self, c => quad_light(v, c, st, p, s, q, smooth))
    }
}

/// Average of four lights (§ 2.4): black neighbours replaced by the centre if it is bright enough.
fn mix(n: [u16; 3], c: u16) -> u16 {
    let (cb, cs) = (c & 0xff, c >> 8);
    let n = if cs > 0x20 || cb > 0x20 {
        n.map(|v| {
            if v == 0 {
                c
            } else if v >> 8 == 0 {
                v | cs << 8
            } else {
                v
            }
        })
    } else {
        n
    };
    let b = (u32::from(n[0] & 0xff) + u32::from(n[1] & 0xff) + u32::from(n[2] & 0xff) + u32::from(cb)) >> 2;
    let s = (u32::from(n[0] >> 8) + u32::from(n[1] >> 8) + u32::from(n[2] >> 8) + u32::from(cs)) >> 2;
    (b | s << 8) as u16
}

/// Lateral directions c0..c3 of each face (§ 2.4).
const SIDES: [[Dir; 4]; 6] = [
    [Dir::West, Dir::East, Dir::North, Dir::South],
    [Dir::East, Dir::West, Dir::North, Dir::South],
    [Dir::Up, Dir::Down, Dir::East, Dir::West],
    [Dir::West, Dir::East, Dir::Down, Dir::Up],
    [Dir::Up, Dir::Down, Dir::North, Dir::South],
    [Dir::Down, Dir::Up, Dir::North, Dir::South],
];

/// Corners A, B, C, D: pairs of sides (indices into `SIDES`).
const CORNERS: [(usize, usize); 4] = [(0, 3), (0, 2), (1, 2), (1, 3)];

fn add(p: [i32; 3], d: Dir) -> [i32; 3] {
    let o = d.offset();
    [p[0] + o[0], p[1] + o[1], p[2] + o[2]]
}

/// Light (packed) and occlusion gray (0–255) of the 4 vertices of a quad.
pub type Vertices = [(u16, u8); 4];
const EPS: f32 = 1.0e-4;

fn axis(d: Dir) -> usize {
    match d {
        Dir::West | Dir::East => 0,
        Dir::Down | Dir::Up => 1,
        Dir::North | Dir::South => 2,
    }
}

fn positive(d: Dir) -> bool {
    matches!(d, Dir::Up | Dir::South | Dir::East)
}

/// "Glued" quad (§ 2.3): plane on the normal axis, and on the edge of the cube on the normal side or block
/// with a cubic collision.
fn stuck(q: &QuadGeom<'_>, full_collision: bool) -> bool {
    let a = axis(q.dir);
    let (lo, hi) = q.pos.iter().fold((f32::MAX, f32::MIN), |(l, h), p| (l.min(p[a]), h.max(p[a])));
    if lo != hi {
        return false;
    }
    let on_edge = if positive(q.dir) { hi > 1.0 - EPS } else { lo < EPS };
    on_edge || full_collision
}

/// Emission floor of the element (§ 2.6).
fn floor_emission(l: u16, e: u8) -> u16 {
    if e == 0 {
        return l;
    }
    let e = u16::from(e);
    let b = ((l & 0xff) >> 4).max(e) << 4;
    let s = ((l >> 8) >> 4).max(e) << 4;
    b | s << 8
}

/// Baked quad as seen by the lighting.
pub struct QuadGeom<'a> {
    pub pos: &'a [[f32; 3]; 4],
    pub dir: Dir,
    pub cull: Option<Dir>,
    pub emission: u8,
    /// Precomputed geometry of the face (`LightPaths::facetab`), otherwise recomputed.
    pub tab: Option<(&'a FaceLight, &'a FaceTables)>,
}

/// Geometry of a baked face as seen by the lighting, computed once at model load:
/// same tests and same floating-point products as the per-quad computation.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceLight {
    /// Planar on the axis of its normal; and on the edge of the cube on the normal side.
    flat: bool,
    edge: bool,
    /// Each vertex is at a corner of the face (weight 0 or 1).
    full: bool,
    /// Full face: corner that carries each vertex.
    corner_of: [u8; 4],
    /// Non-full face: index of its weights in `FaceTables::weights`.
    weights: u32,
}

/// Geometry of all the baked faces (index = quad index), and bilinear weights
/// (vertex, corner) of the non-full faces.
#[derive(Default)]
pub struct FaceTables {
    pub faces: Vec<FaceLight>,
    weights: Vec<[[f32; 4]; 4]>,
}

impl FaceTables {
    pub fn new<'q>(quads: impl Iterator<Item = (&'q [[f32; 3]; 4], Dir)>) -> Self {
        let mut t = FaceTables::default();
        let mut seen: hashbrown::HashMap<[[u32; 4]; 4], u32> = hashbrown::HashMap::new();
        for (pos, dir) in quads {
            let a = axis(dir);
            let (lo, hi) = pos.iter().fold((f32::MAX, f32::MIN), |(l, h), p| (l.min(p[a]), h.max(p[a])));
            let flat = lo == hi;
            let edge = if positive(dir) { hi > 1.0 - EPS } else { lo < EPS };
            let sides = SIDES[dir as usize];
            let (ax, bx) = (axis(sides[0]), axis(sides[2]));
            let full = pos.iter().all(|p| matches!(p[ax], 0.0 | 1.0) && matches!(p[bx], 0.0 | 1.0));
            let mut f = FaceLight { flat, edge, full, corner_of: [0; 4], weights: u32::MAX };
            if full {
                f.corner_of = std::array::from_fn(|k| {
                    let p = &pos[k];
                    CORNERS.iter().position(|&(i, j)| weight(sides[i], p) == 1.0 && weight(sides[j], p) == 1.0).unwrap_or(0)
                        as u8
                });
            } else {
                let w: [[f32; 4]; 4] = std::array::from_fn(|k| {
                    let p = &pos[k];
                    std::array::from_fn(|c| {
                        let (i, j) = CORNERS[c];
                        weight(sides[i], p) * weight(sides[j], p)
                    })
                });
                // Bit-identical weights shared (many faces have the same geometry).
                let n = t.weights.len() as u32;
                f.weights = *seen.entry(w.map(|r| r.map(f32::to_bits))).or_insert(n);
                if f.weights == n {
                    t.weights.push(w);
                }
            }
            t.faces.push(f);
        }
        t.faces.shrink_to_fit();
        t.weights.shrink_to_fit();
        t
    }

    /// Bytes used.
    pub fn heap_bytes(&self) -> usize {
        self.faces.capacity() * size_of::<FaceLight>() + self.weights.capacity() * size_of::<[[f32; 4]; 4]>()
    }
}

/// Weight of a corner on the axis of a side at the position of a vertex.
#[inline]
fn weight(side: Dir, pos: &[f32; 3]) -> f32 {
    let u = pos[axis(side)];
    if positive(side) { u } else { 1.0 - u }
}

/// Corners A, B, C, D of a face of direction `d` of block `p` (§ 2.4).
#[inline]
fn face_corners<C: CellKind>(
    v: &LightView<'_>,
    cache: &mut LightCache<C>,
    st: &impl Fn(i32, i32, i32) -> u32,
    p: [i32; 3],
    d: Dir,
    is_stuck: bool,
) -> Corners {
    let shade = |i: u16| if i & info::DARK_SHADE != 0 { 0.2f32 } else { 1.0 };
    let base = if is_stuck { add(p, d) } else { p };
    let sides = SIDES[d as usize];
    let pos_i: [[i32; 3]; 4] = sides.map(|c| add(base, c));
    let perm_of = |i: u16| i & info::SOLID_RENDER == 0 || i & info::NO_DAMPENING != 0;
    let (l, o, diags, lc, oc);
    if cache.paths.cell {
        // One read per cell: light and properties together; the block light is only read
        // if it is used.
        let li: [(u16, u16); 4] = pos_i.map(|q| cache.cell(v, q, st));
        l = li.map(|c| c.0);
        o = li.map(|c| shade(c.1));
        let perm: [bool; 4] = pos_i.map(|q| perm_of(cache.info(v, add(q, d), st)));
        diags = CORNERS.map(|(i, j)| {
            if !perm[i] && !perm[j] {
                (l[0], o[0])
            } else {
                let (lq, iq) = cache.cell(v, add(pos_i[i], sides[j]), st);
                (lq, shade(iq))
            }
        });
        let front = add(p, d);
        if is_stuck {
            let (lf, inf) = cache.cell(v, front, st);
            (lc, oc) = (lf, shade(inf));
        } else {
            let inf = cache.info(v, front, st);
            lc = if inf & info::SOLID_RENDER == 0 { cache.lum(v, front, st) } else { cache.lum(v, p, st) };
            oc = shade(cache.info(v, base, st));
        }
    } else {
        l = pos_i.map(|q| cache.lum(v, q, st));
        o = pos_i.map(|q| shade(cache.info(v, q, st)));
        let perm: [bool; 4] = pos_i.map(|q| perm_of(cache.info(v, add(q, d), st)));
        diags = CORNERS.map(|(i, j)| {
            if !perm[i] && !perm[j] {
                (l[0], o[0])
            } else {
                let q = add(pos_i[i], sides[j]);
                (cache.lum(v, q, st), shade(cache.info(v, q, st)))
            }
        });
        let mut c = cache.lum(v, p, st);
        let front = add(p, d);
        if is_stuck || cache.info(v, front, st) & info::SOLID_RENDER == 0 {
            c = cache.lum(v, front, st);
        }
        lc = c;
        oc = shade(cache.info(v, base, st));
    }
    std::array::from_fn(|k| {
        let ((i, j), (lij, oij)) = (CORNERS[k], diags[k]);
        (mix([l[j], l[i], lij], lc), (o[j] + o[i] + oij + oc) * 0.25)
    })
}

/// Light per vertex of a quad of block `p` (state `s`); `st` gives the state of a position.
pub fn quad_light<C: CellKind>(
    v: &LightView<'_>,
    cache: &mut LightCache<C>,
    st: &impl Fn(i32, i32, i32) -> u32,
    p: [i32; 3],
    s: u32,
    q: &QuadGeom<'_>,
    smooth: bool,
) -> Vertices {
    let inf = |s: u32| v.info.get(s as usize).copied().unwrap_or(0);
    let me = inf(s);
    let full_collision = me & info::FULL_COLLISION != 0;
    let is_stuck = match q.tab {
        Some((t, _)) => t.flat && (t.edge || full_collision),
        None => stuck(q, full_collision),
    };
    if !smooth {
        // Flat path: one light for the quad, read with the block's own state.
        let at = match q.cull {
            Some(d) => add(p, d),
            None if is_stuck => add(p, q.dir),
            None => p,
        };
        let l = if cache.paths.fluid { cache.lum_as(v, at, s) } else { v.lum(at[0], at[1], at[2], s) };
        return [(floor_emission(l, q.emission), 255); 4];
    }
    let d = q.dir;
    let corner = if cache.paths.corners || cache.paths.neigh {
        cache.begin_block(p);
        let key = d as usize * 2 + usize::from(is_stuck);
        if cache.paths.corners && cache.corner_ok >> key & 1 == 1 {
            cache.corners[key]
        } else {
            let c = face_corners(v, cache, st, p, d, is_stuck);
            if cache.paths.corners {
                cache.corners[key] = c;
                cache.corner_ok |= 1 << key;
            }
            c
        }
    } else {
        face_corners(v, cache, st, p, d, is_stuck)
    };
    if let Some((t, tabs)) = q.tab {
        if t.full {
            return std::array::from_fn(|k| {
                let (lv, ov) = corner[t.corner_of[k] as usize];
                (floor_emission(lv, q.emission), (ov.clamp(0.0, 1.0) * 255.0) as u8)
            });
        }
        let w = &tabs.weights[t.weights as usize];
        return std::array::from_fn(|k| blend(&corner, &w[k], q.emission));
    }
    let sides = SIDES[d as usize];
    // Full quad: each vertex is at a corner of the face, weight 0 or 1; the corner that carries it
    // is taken directly (same result as the weighted sum, without the products).
    let (ax, bx) = (axis(sides[0]), axis(sides[2]));
    let full = q.pos.iter().all(|pos| matches!(pos[ax], 0.0 | 1.0) && matches!(pos[bx], 0.0 | 1.0));
    if full {
        return std::array::from_fn(|k| {
            let pos = &q.pos[k];
            let c = CORNERS
                .iter()
                .position(|&(i, j)| weight(sides[i], pos) == 1.0 && weight(sides[j], pos) == 1.0)
                .unwrap_or(0);
            let (lv, ov) = corner[c];
            let gray = (ov.clamp(0.0, 1.0) * 255.0) as u8;
            (floor_emission(lv, q.emission), gray)
        });
    }
    std::array::from_fn(|k| {
        let pos = &q.pos[k];
        let w: [f32; 4] = std::array::from_fn(|c| {
            let (i, j) = CORNERS[c];
            weight(sides[i], pos) * weight(sides[j], pos)
        });
        blend(&corner, &w, q.emission)
    })
}

/// Vertex of a non-full face: weighted sum of the corners (bilinear weights, exact at the corners).
#[inline]
fn blend(corner: &Corners, w: &[f32; 4], emission: u8) -> (u16, u8) {
    let (mut b, mut sk, mut ao) = (0.0f32, 0.0f32, 0.0f32);
    for (c, &w) in w.iter().enumerate() {
        let (lv, ov) = corner[c];
        b += w * f32::from(lv & 0xff);
        sk += w * f32::from(lv >> 8);
        ao += w * ov;
    }
    let light = (b as u16) | (sk as u16) << 8;
    let gray = (ao.clamp(0.0, 1.0) * 255.0) as u8;
    (floor_emission(light, emission), gray)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mix_replaces_dark_neighbours() {
        // Sky-15 centre: an opaque neighbour (0) takes the centre's value.
        let c = 0xf000;
        assert_eq!(mix([0, 0xf000, 0xf000], c), 0xf000);
        // Dark centre: no replacement.
        assert_eq!(mix([0, 0x2000, 0x2000], 0x2000) >> 8, 0x18);
        // Sky alone at zero: replaced by the centre's, block kept.
        assert_eq!(mix([0x0050, 0xf000, 0xf000], c), 0xf014);
    }

    #[test]
    fn emission_floor_requantizes() {
        assert_eq!(floor_emission(0x7834, 10), 0xa0a0);
        assert_eq!(floor_emission(0xf034, 0), 0xf034);
    }
}
