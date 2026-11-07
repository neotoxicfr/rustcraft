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
    /// The all-zero light sheet shared by the world (`rc_world::zero_layer`), recognised by its
    /// address (`LightPaths::zero_layers`).
    zero_sheet: std::sync::Arc<[u8; rc_world::LIGHT_BYTES]>,
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
        LightView { job, info, cols, zero_sheet: rc_world::zero_layer() }
    }

    /// Light properties of a state (0 outside the table).
    #[inline]
    fn info_of(&self, s: u32) -> u16 {
        self.info.get(s as usize).copied().unwrap_or(0)
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
        let inf = self.info_of(s);
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

/// [`pack`] applied to raw levels already packed without emission (block × 16, sky × 4096):
/// emissive render → full light, otherwise block raised to the state's emission.
#[inline]
fn with_emission(raw: u16, inf: u16) -> u16 {
    if inf & info::EMISSIVE != 0 {
        return 0xf0f0;
    }
    let block = ((raw >> 4) & 15).max(inf & info::EMISSION);
    block << 4 | (raw & 0xf000)
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

    /// Same reads with the shared all-zero sheet replaced by its constant: its block light reads
    /// as "no data" (0) and its sky as level 0, without touching the sheet. Underground, most
    /// sections have neither torch nor sky: their reads then cost no memory access.
    fn without_zero_sheet(self, zero: &[u8; rc_world::LIGHT_BYTES]) -> Self {
        let is_zero = |l: &[u8; rc_world::LIGHT_BYTES]| std::ptr::eq(l, zero);
        let block = self.block.filter(|l| !is_zero(l));
        let sky = match self.sky {
            Sky::Layer(l) | Sky::Bottom(l) if is_zero(l) => Sky::Level(0),
            sky => sky,
        };
        Slot { block, sky }
    }

    /// Raw levels at (x, y, z) packed without emission (block × 16, sky × 4096).
    #[inline]
    fn packed(&self, x: usize, y: usize, z: usize) -> u16 {
        let (b, k) = self.read(x, y, z);
        u16::from(b) << 4 | u16::from(k) << 12
    }

    /// The 16 raw levels of row (y, z), x from 0 to 15, packed like [`Slot::packed`].
    #[inline(always)]
    fn unpack_row(&self, y: usize, z: usize, out: &mut [u16; 16]) {
        // 8 bytes per row: the 16 half-bytes of x = 0..15.
        let row = |l: &[u8; rc_world::LIGHT_BYTES], y: usize| -> [u8; 8] {
            let at = (y << 7) | (z << 3);
            l[at..at + 8].try_into().expect("8 bytes")
        };
        let sky = match self.sky {
            Sky::Level(v) if self.block.is_none() => {
                out.fill(u16::from(v) << 12);
                return;
            }
            Sky::Level(v) => [v | v << 4; 8],
            Sky::Layer(l) => row(l, y),
            Sky::Bottom(l) => row(l, 0),
        };
        let block = self.block.map_or([0; 8], |l| row(l, y));
        unpack_light_row(&block, &sky, out);
    }
}

/// 16 raw levels packed like [`Slot::packed`] from the 8 bytes of block light and of sky light
/// of a row (two half-bytes per byte, even x in the low half). Same work on every byte and no
/// branch, so that the compiler turns the loop into vector shuffles.
#[inline(always)]
fn unpack_light_row(block: &[u8; 8], sky: &[u8; 8], out: &mut [u16; 16]) {
    for k in 0..8 {
        let (b, s) = (u16::from(block[k]), u16::from(sky[k]));
        out[2 * k] = (b & 0x0f) << 4 | (s & 0x0f) << 12;
        out[2 * k + 1] = (b & 0xf0) | (s & 0xf0) << 8;
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
    /// Reads of a face at fixed offsets from its block, one table per (direction, glued): no
    /// per-read position test (`RUSTCRAFT_OPT_AOOFFSETS`).
    pub fixed_offsets: bool,
    /// Properties of the cells of the section's 18³ grids read from the mesher's property word
    /// and state grid instead of the cache cells (`RUSTCRAFT_OPT_AOPROPBITS`).
    pub prop_bits: bool,
    /// Raw light of the 18³ grid unpacked once per section, by rows (`RUSTCRAFT_OPT_AORAWGRID`),
    /// when the section has at least `raw_grid_min` candidate blocks (`RUSTCRAFT_OPT_AORAWGRIDMIN`).
    pub raw_grid: bool,
    pub raw_grid_min: u16,
    /// The shared all-zero light sheet read as a constant (`RUSTCRAFT_OPT_AOZEROLAYER`).
    pub zero_layers: bool,
}

/// Candidate blocks from which a section's raw light grid is filled (`raw_grid_min`): a fill
/// costs about 2 µs (5,832 cells, loaded machine), some 400 lazy reads, or a few tens of blocks
/// with faces, hence 64. On the loaded machine 0, 64, 256 and 1,024 stayed inside the bench's own
/// noise (the same setting repeated moves up to 8 %): to measure again on a calm machine
/// (`docs/perf.md`).
pub const RAW_GRID_MIN: u16 = 64;

impl LightPaths {
    pub const ALL: LightPaths = LightPaths {
        cell: true,
        corners: true,
        facetab: true,
        scratch: true,
        cell8: true,
        fluid: true,
        neigh: true,
        fixed_offsets: true,
        prop_bits: true,
        raw_grid: true,
        raw_grid_min: RAW_GRID_MIN,
        zero_layers: true,
    };
    pub const OLD: LightPaths = LightPaths {
        cell: false,
        corners: false,
        facetab: false,
        scratch: false,
        cell8: false,
        fluid: false,
        neigh: false,
        fixed_offsets: false,
        prop_bits: false,
        raw_grid: false,
        raw_grid_min: RAW_GRID_MIN,
        zero_layers: false,
    };
    /// Paths active by default: D + A + B, best combination proven in the micro-bench, with the
    /// addressed reads (offsets, property bits, raw grid). C, E, F, G and the zero sheets showed
    /// no gain (C loses once combined): turned off until the calm-machine bench, see `docs/perf.md`.
    pub const DEFAULT: LightPaths = LightPaths {
        cell: true,
        corners: true,
        scratch: true,
        fixed_offsets: true,
        prop_bits: true,
        raw_grid: true,
        ..Self::OLD
    };

    /// Paths chosen by the environment (`DEFAULT`; "0" turns off, "1" activates).
    pub fn from_env() -> Self {
        use rc_data::opt_switch;
        static P: std::sync::OnceLock<LightPaths> = std::sync::OnceLock::new();
        *P.get_or_init(|| LightPaths {
            cell: opt_switch!("RUSTCRAFT_OPT_AOCELL"),
            corners: opt_switch!("RUSTCRAFT_OPT_AOCORNERS"),
            facetab: opt_switch!("RUSTCRAFT_OPT_AOFACETAB", default_off),
            scratch: opt_switch!("RUSTCRAFT_OPT_AOSCRATCH"),
            cell8: opt_switch!("RUSTCRAFT_OPT_AOCELL8", default_off),
            fluid: opt_switch!("RUSTCRAFT_OPT_AOFLUID", default_off),
            neigh: opt_switch!("RUSTCRAFT_OPT_AONEIGH", default_off),
            fixed_offsets: opt_switch!("RUSTCRAFT_OPT_AOOFFSETS"),
            prop_bits: opt_switch!("RUSTCRAFT_OPT_AOPROPBITS"),
            raw_grid: opt_switch!("RUSTCRAFT_OPT_AORAWGRID"),
            raw_grid_min: rc_data::env_str!("RUSTCRAFT_OPT_AORAWGRIDMIN")
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(RAW_GRID_MIN),
            zero_layers: opt_switch!("RUSTCRAFT_OPT_AOZEROLAYER", default_off),
        })
    }

    /// The face corners go through the addressed reader ([`face_corners_by_address`]) rather
    /// than the coordinate one; `cell` and `neigh` only act on the latter.
    fn addressed(&self) -> bool {
        self.fixed_offsets || self.prop_bits || self.raw_grid
    }
}

/// Cache cell: light properties of the state and light (read with the cell's state), valid
/// for one generation.
pub trait CellKind: Copy + Default {
    /// Largest representable generation (beyond: cells emptied, back to 1).
    const MAX_EPOCH: u32;
    fn valid(&self, epoch: u32) -> bool;
    fn info(&self) -> u16;
    fn fill(&mut self, epoch: u32, info: u16);
    fn lum(&self, epoch: u32) -> Option<u16>;
    fn set_lum(&mut self, epoch: u32, l: u16);
}

/// Original cell (12 B): 32-bit generations. It used to keep the state too (16 B), written and
/// never read.
#[derive(Clone, Copy, Default)]
pub struct Wide {
    epoch: u32,
    lum_epoch: u32,
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
    fn fill(&mut self, epoch: u32, info: u16) {
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
    fn fill(&mut self, epoch: u32, info: u16) {
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

/// Side of the section's grids with their one-block border, laid out like the mesher's state and
/// property grids (step x 1, z 18, y 324).
const GSIDE: i32 = 18;
const GRID_CELLS: usize = (GSIDE * GSIDE * GSIDE) as usize;

/// The mesher's 18³ grids of the section being meshed: states (`blocks::OUTSIDE` outside the
/// columns) and property words (`cull`, light properties in bits 29-31).
#[derive(Clone, Copy)]
pub struct SectionGrids<'g> {
    pub states: &'g [u32],
    pub props: &'g [u32],
}

/// Cells kept by the thread, in both formats, and the raw light grid (`LightPaths::raw_grid`,
/// 11.4 KiB once filled).
#[derive(Default)]
pub struct LightStores {
    wide: CellStore<Wide>,
    narrow: CellStore<Narrow>,
    raw_light: Vec<u16>,
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
    /// Raw light of the section's 18³ grid packed without emission ([`Slot::packed`]), filled at
    /// the reset when `raw_ready`.
    raw_light: Vec<u16>,
    raw_ready: bool,
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
    fn new(paths: LightPaths, store: CellStore<C>, raw_light: Vec<u16>) -> Self {
        LightCache {
            base_y: 0,
            store,
            raw_light,
            raw_ready: false,
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
    /// cells will be used (allocated at first need); `candidates`: blocks of the section that can
    /// produce a face (decides the raw light grid).
    fn reset(&mut self, v: &LightView<'a>, base_y: i32, cells: bool, candidates: usize) {
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
        if self.paths.zero_layers {
            self.slots = self.slots.map(|s| s.without_zero_sheet(&v.zero_sheet));
        }
        // Unpacking the whole grid costs about as much as 400 lazy reads: only for the sections
        // with enough blocks to light.
        self.raw_ready = self.paths.raw_grid && cells && candidates >= usize::from(self.paths.raw_grid_min);
        if self.raw_ready {
            fill_raw_grid(&self.slots, &mut self.raw_light);
        }
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

    /// Cell of the section's 18³ grids (the cache without its second ring).
    #[inline]
    fn grid_index(&self, p: [i32; 3]) -> Option<usize> {
        let (x, y, z) = (p[0] + 1, p[1] - self.base_y + 1, p[2] + 1);
        ((0..GSIDE).contains(&x) && (0..GSIDE).contains(&y) && (0..GSIDE).contains(&z))
            .then_some((y * GSIDE * GSIDE + z * GSIDE + x) as usize)
    }

    /// Cache cell `i`, filled with the properties of `state()` if it is not valid.
    #[inline]
    fn fill(&mut self, v: &LightView<'_>, i: usize, state: impl FnOnce() -> u32) -> &mut C {
        let epoch = self.store.epoch;
        let c = &mut self.store.cells[i];
        if !c.valid(epoch) {
            c.fill(epoch, v.info_of(state()));
        }
        c
    }

    /// Light properties of cache cell `i` (its state is `state()`).
    #[inline]
    fn info_in_cell(&mut self, v: &LightView<'_>, i: usize, state: impl FnOnce() -> u32) -> u16 {
        self.fill(v, i, state).info()
    }

    /// Light and properties of cache cell `i` (position `p`, state `state()`), the light read from
    /// the resolved layers at its first use.
    #[inline]
    fn light_and_info_in_cell(
        &mut self,
        v: &LightView<'_>,
        i: usize,
        p: [i32; 3],
        state: impl FnOnce() -> u32,
    ) -> (u16, u16) {
        let epoch = self.store.epoch;
        let c = *self.fill(v, i, state);
        if let Some(l) = c.lum(epoch) {
            return (l, c.info());
        }
        let inf = c.info();
        let l = if inf & info::EMISSIVE != 0 { 0xf0f0 } else { pack(self.raw(p), inf) };
        self.store.cells[i].set_lum(epoch, l);
        (l, inf)
    }

    /// Light properties of the cell's state (cache only).
    #[inline]
    fn cached_info(&mut self, v: &LightView<'_>, p: [i32; 3], state_at: &impl Fn(i32, i32, i32) -> u32) -> u16 {
        let state = || state_at(p[0], p[1], p[2]);
        match self.index(p) {
            Some(i) => self.info_in_cell(v, i, state),
            None => v.info_of(state()),
        }
    }

    /// Light of the cell (cache only).
    #[inline]
    fn cached_light(&mut self, v: &LightView<'_>, p: [i32; 3], state_at: &impl Fn(i32, i32, i32) -> u32) -> u16 {
        self.cached_cell(v, p, state_at).0
    }

    /// Light and properties of the cell in one read (cache only).
    #[inline]
    fn cached_cell(
        &mut self,
        v: &LightView<'_>,
        p: [i32; 3],
        state_at: &impl Fn(i32, i32, i32) -> u32,
    ) -> (u16, u16) {
        let state = || state_at(p[0], p[1], p[2]);
        let Some(i) = self.index(p) else {
            let s = state();
            return (v.lum(p[0], p[1], p[2], s), v.info_of(s));
        };
        self.light_and_info_in_cell(v, i, p, state)
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
    fn info(&mut self, v: &LightView<'_>, p: [i32; 3], state_at: &impl Fn(i32, i32, i32) -> u32) -> u16 {
        if self.paths.neigh
            && let Some(k) = self.near_slot(p)
        {
            if self.near_info >> k & 1 == 0 {
                self.near[k].1 = self.cached_info(v, p, state_at);
                self.near_info |= 1 << k;
            }
            return self.near[k].1;
        }
        self.cached_info(v, p, state_at)
    }

    /// Light and properties of the cell (neighbourhood memorised first if `neigh`).
    #[inline]
    fn cell(&mut self, v: &LightView<'_>, p: [i32; 3], state_at: &impl Fn(i32, i32, i32) -> u32) -> (u16, u16) {
        if self.paths.neigh
            && let Some(k) = self.near_slot(p)
        {
            if self.near_lum >> k & 1 == 0 {
                self.near[k] = self.cached_cell(v, p, state_at);
                self.near_lum |= 1 << k;
                self.near_info |= 1 << k;
            }
            return self.near[k];
        }
        self.cached_cell(v, p, state_at)
    }

    /// Light of the cell.
    #[inline]
    fn lum(&mut self, v: &LightView<'_>, p: [i32; 3], state_at: &impl Fn(i32, i32, i32) -> u32) -> u16 {
        if self.paths.neigh {
            return self.cell(v, p, state_at).0;
        }
        self.cached_light(v, p, state_at)
    }

    /// Light of a position read with the given state `s` (flat path): resolved layers of the
    /// section if the position is in the cache, otherwise full read.
    #[inline]
    fn lum_as(&self, v: &LightView<'_>, p: [i32; 3], s: u32) -> u16 {
        if self.index(p).is_none() {
            return v.lum(p[0], p[1], p[2], s);
        }
        let inf = v.info_of(s);
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
    /// otherwise allocated for the task; the raw light grid is always the thread's.
    pub fn new(paths: LightPaths, stores: &mut LightStores) -> Self {
        let raw_light = std::mem::take(&mut stores.raw_light);
        if paths.cell8 {
            let s = if paths.scratch { std::mem::take(&mut stores.narrow) } else { CellStore::filled() };
            AnyCache::Narrow(LightCache::new(paths, s, raw_light))
        } else {
            let s = if paths.scratch { std::mem::take(&mut stores.wide) } else { CellStore::filled() };
            AnyCache::Wide(LightCache::new(paths, s, raw_light))
        }
    }

    /// Gives the cells (`paths.scratch`) and the raw light grid back to the thread.
    pub fn finish(self, stores: &mut LightStores) {
        match self {
            AnyCache::Wide(c) => {
                stores.raw_light = c.raw_light;
                if c.paths.scratch {
                    stores.wide = c.store;
                }
            }
            AnyCache::Narrow(c) => {
                stores.raw_light = c.raw_light;
                if c.paths.scratch {
                    stores.narrow = c.store;
                }
            }
        }
    }

    /// Empties the cache for the section whose bottom is at `base_y`; `cells`: the cells will be
    /// used; `candidates`: blocks of the section that can produce a face.
    pub fn reset(&mut self, v: &LightView<'a>, base_y: i32, cells: bool, candidates: usize) {
        each!(self, c => c.reset(v, base_y, cells, candidates))
    }

    /// Light of a position read with its own state (fluids, `LightPaths::fluid`).
    pub fn lum(&mut self, v: &LightView<'_>, p: [i32; 3], state_at: &impl Fn(i32, i32, i32) -> u32) -> u16 {
        each!(self, c => c.lum(v, p, state_at))
    }

    /// [`quad_light`] in this cache.
    pub fn quad_light<F: Fn(i32, i32, i32) -> u32>(
        &mut self,
        src: &Sources<'_, '_, F>,
        p: [i32; 3],
        s: u32,
        q: &QuadGeom<'_>,
        smooth: bool,
    ) -> Vertices {
        each!(self, c => quad_light(src, c, p, s, q, smooth))
    }
}

/// What the reads of a block look at: the light view, the state of a position, and the
/// section's 18³ grids when the mesher has them.
pub struct Sources<'s, 'v, F> {
    pub view: &'s LightView<'v>,
    pub state_at: &'s F,
    pub grids: Option<SectionGrids<'s>>,
}

/// Offset of a read from its block: as a vector, and as steps in the 20³ cache and in the
/// 18³ grids.
#[derive(Clone, Copy)]
struct CellOffset {
    delta: [i32; 3],
    cache: isize,
    grid: isize,
}

impl CellOffset {
    const fn new(delta: [i32; 3]) -> Self {
        let [x, y, z] = delta;
        CellOffset {
            delta,
            cache: (y * CSIDE * CSIDE + z * CSIDE + x) as isize,
            grid: (y * GSIDE * GSIDE + z * GSIDE + x) as isize,
        }
    }
}

/// Reads of one face of a block for one (direction, glued) pair (§ 2.4), the base being the
/// block, or the block in front for a glued face.
#[derive(Clone, Copy)]
struct FaceReads {
    /// Base + each side: light and properties.
    edges: [CellOffset; 4],
    /// Each edge one step further along the normal: does it let the light through?
    beyond: [CellOffset; 4],
    /// Base + both sides of each corner: light and properties.
    diagonals: [CellOffset; 4],
    front: CellOffset,
    base: CellOffset,
}

const fn step(d: Dir) -> [i32; 3] {
    match d {
        Dir::Down => [0, -1, 0],
        Dir::Up => [0, 1, 0],
        Dir::North => [0, 0, -1],
        Dir::South => [0, 0, 1],
        Dir::West => [-1, 0, 0],
        Dir::East => [1, 0, 0],
    }
}

const fn plus(a: [i32; 3], b: [i32; 3]) -> [i32; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

const fn face_reads(d: Dir, glued: bool) -> FaceReads {
    let front = step(d);
    let base = if glued { front } else { [0; 3] };
    let sides = SIDES[d as usize];
    let none = CellOffset::new([0; 3]);
    let (mut edges, mut beyond, mut diagonals) = ([none; 4], [none; 4], [none; 4]);
    let mut k = 0;
    while k < 4 {
        let edge = plus(base, step(sides[k]));
        edges[k] = CellOffset::new(edge);
        beyond[k] = CellOffset::new(plus(edge, front));
        let (i, j) = CORNERS[k];
        diagonals[k] = CellOffset::new(plus(plus(base, step(sides[i])), step(sides[j])));
        k += 1;
    }
    FaceReads { edges, beyond, diagonals, front: CellOffset::new(front), base: CellOffset::new(base) }
}

/// [`FaceReads`] per direction (`Dir` order) and glued (index 1).
const FACE_READS: [[FaceReads; 2]; 6] = {
    let mut t = [[face_reads(Dir::Down, false); 2]; 6];
    let mut d = 0;
    while d < 6 {
        t[d] = [face_reads(Dir::ALL[d], false), face_reads(Dir::ALL[d], true)];
        d += 1;
    }
    t
};

/// Where a read lands: its position, its cell in the 20³ cache, and its cell in the 18³ grids
/// when it is inside them.
#[derive(Clone, Copy)]
struct CellAddress {
    pos: [i32; 3],
    cache: usize,
    grid: Option<usize>,
}

/// Light properties that the face corners test (solid render, lets the light through, dark
/// shade), rebuilt from bits 29-31 of a cell's property word (`cull::light_bits`).
#[inline]
fn info_of_prop_word(word: u32) -> u16 {
    use crate::cull::{DARK_SHADE_BIT, LIGHT_CLOSED_BIT, SOLID_RENDER_BIT};
    let mut i = 0;
    if word & SOLID_RENDER_BIT != 0 {
        i |= info::SOLID_RENDER;
    }
    // A solid render cell that does not close the light is one that does not dampen it.
    if word & LIGHT_CLOSED_BIT == 0 {
        i |= info::NO_DAMPENING;
    }
    if word & DARK_SHADE_BIT != 0 {
        i |= info::DARK_SHADE;
    }
    i
}

/// State of a cell of the mesher's state grid as the state closure gives it: outside the
/// columns, air (state 0). Today no reader of its properties would see the difference (only the
/// dampening bit differs); kept so that both sources give the same state.
#[inline]
fn state_of_grid_cell(v: u32) -> u32 {
    if v == crate::blocks::OUTSIDE { 0 } else { v }
}

impl<C: CellKind> LightCache<'_, C> {
    /// Cells of block `p` in the 20³ cache and in the 18³ grids.
    #[inline]
    fn block_cells(&self, p: [i32; 3]) -> (usize, usize) {
        let (x, y, z) = (p[0], p[1] - self.base_y, p[2]);
        let cache = (y + 2) * CSIDE * CSIDE + (z + 2) * CSIDE + x + 2;
        let grid = (y + 1) * GSIDE * GSIDE + (z + 1) * GSIDE + x + 1;
        (cache as usize, grid as usize)
    }

    /// Address of the read at `o` from block `p` (cells `block`). With `fixed_offsets`, plain
    /// additions (`in_grid`: the read is known to be inside the 18³ grids); otherwise the
    /// position is located like any other.
    #[inline]
    fn address(&self, p: [i32; 3], block: (usize, usize), o: CellOffset, in_grid: bool) -> CellAddress {
        let pos = plus(p, o.delta);
        if self.paths.fixed_offsets {
            let grid = in_grid.then(|| block.1.wrapping_add_signed(o.grid));
            return CellAddress { pos, cache: block.0.wrapping_add_signed(o.cache), grid };
        }
        // Every read of a face lies within two blocks of the section: always in the cache.
        let cache = self.index(pos).expect("face read within the two-block border");
        CellAddress { pos, cache, grid: self.grid_index(pos) }
    }

    /// Light and properties of the cell at `a`.
    #[inline]
    fn cell_at<F: Fn(i32, i32, i32) -> u32>(&mut self, src: &Sources<'_, '_, F>, a: CellAddress) -> (u16, u16) {
        let grid_state = match (src.grids, a.grid) {
            (Some(g), Some(c)) if self.paths.prop_bits => Some(state_of_grid_cell(g.states[c])),
            _ => None,
        };
        let state = || grid_state.unwrap_or_else(|| (src.state_at)(a.pos[0], a.pos[1], a.pos[2]));
        if self.raw_ready
            && let Some(c) = a.grid
        {
            let inf = match grid_state {
                Some(s) => src.view.info_of(s),
                None => self.info_in_cell(src.view, a.cache, state),
            };
            return (with_emission(self.raw_light[c], inf), inf);
        }
        self.light_and_info_in_cell(src.view, a.cache, a.pos, state)
    }

    /// Light properties of the cell at `a` (only the bits that the face corners test when they
    /// come from the property word).
    #[inline]
    fn flags_at<F: Fn(i32, i32, i32) -> u32>(&mut self, src: &Sources<'_, '_, F>, a: CellAddress) -> u16 {
        if self.paths.prop_bits
            && let (Some(g), Some(c)) = (src.grids, a.grid)
        {
            return info_of_prop_word(g.props[c]);
        }
        self.info_in_cell(src.view, a.cache, || (src.state_at)(a.pos[0], a.pos[1], a.pos[2]))
    }
}

/// Raw light of the 18³ grid around the section (index like the mesher's grids), unpacked from
/// the resolved layers row by row: for each (y, z) the cell x = −1 of the western column, the 16
/// cells of the middle one in one go, and the cell x = 16 of the eastern one. AVX2 version when
/// the processor has it.
#[allow(unsafe_code)]
fn fill_raw_grid(slots: &[Slot<'_>; 27], out: &mut Vec<u16>) {
    out.resize(GRID_CELLS, 0);
    #[cfg(target_arch = "x86_64")]
    {
        #[target_feature(enable = "avx2")]
        fn avx2(slots: &[Slot<'_>; 27], out: &mut [u16]) {
            fill_raw_rows(slots, out);
        }
        if std::arch::is_x86_feature_detected!("avx2") {
            // SAFETY: AVX2 has just been detected on this processor.
            return unsafe { avx2(slots, out) };
        }
    }
    fill_raw_rows(slots, out);
}

/// Body of [`fill_raw_grid`], compiled once per instruction set.
#[inline(always)]
fn fill_raw_rows(slots: &[Slot<'_>; 27], out: &mut [u16]) {
    // Grid coordinate (−1 to 16 shifted by one) → (section index 0..3, coordinate in it).
    let split = |g: usize| match g {
        0 => (0, 15),
        17 => (2, 0),
        g => (1, g - 1),
    };
    for gy in 0..GSIDE as usize {
        let (sy, y) = split(gy);
        for gz in 0..GSIDE as usize {
            let (sz, z) = split(gz);
            let row = &mut out[(gy * 18 + gz) * 18..][..18];
            let slot = |sx: usize| &slots[sy * 9 + sz * 3 + sx];
            row[0] = slot(0).packed(15, y, z);
            slot(1).unpack_row(y, z, (&mut row[1..17]).try_into().expect("16 cells"));
            row[17] = slot(2).packed(0, y, z);
        }
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

/// Occlusion gray of a cell from its properties.
#[inline]
fn shade(i: u16) -> f32 {
    if i & info::DARK_SHADE != 0 { 0.2 } else { 1.0 }
}

/// The light goes through the cell towards a corner (not solid render, or not dampening).
#[inline]
fn lets_light_through(i: u16) -> bool {
    i & info::SOLID_RENDER == 0 || i & info::NO_DAMPENING != 0
}

/// Corners A, B, C, D from the reads of the face: light and gray of the 4 edges, of the 4
/// diagonals and of the centre.
#[inline]
fn mix_corners(l: [u16; 4], o: [f32; 4], diags: [(u16, f32); 4], lc: u16, oc: f32) -> Corners {
    std::array::from_fn(|k| {
        let ((i, j), (lij, oij)) = (CORNERS[k], diags[k]);
        (mix([l[j], l[i], lij], lc), (o[j] + o[i] + oij + oc) * 0.25)
    })
}

/// Corners A, B, C, D of a face of direction `d` of block `p` (§ 2.4), each read located from
/// its coordinates.
#[inline]
fn face_corners<C: CellKind>(
    v: &LightView<'_>,
    cache: &mut LightCache<C>,
    state_at: &impl Fn(i32, i32, i32) -> u32,
    p: [i32; 3],
    d: Dir,
    is_stuck: bool,
) -> Corners {
    let base = if is_stuck { add(p, d) } else { p };
    let sides = SIDES[d as usize];
    let pos_i: [[i32; 3]; 4] = sides.map(|c| add(base, c));
    let (l, o, diags, lc, oc);
    if cache.paths.cell {
        // One read per cell: light and properties together; the block light is only read
        // if it is used.
        let li: [(u16, u16); 4] = pos_i.map(|q| cache.cell(v, q, state_at));
        l = li.map(|c| c.0);
        o = li.map(|c| shade(c.1));
        let perm: [bool; 4] = pos_i.map(|q| lets_light_through(cache.info(v, add(q, d), state_at)));
        diags = CORNERS.map(|(i, j)| {
            if !perm[i] && !perm[j] {
                (l[0], o[0])
            } else {
                let (lq, iq) = cache.cell(v, add(pos_i[i], sides[j]), state_at);
                (lq, shade(iq))
            }
        });
        let front = add(p, d);
        if is_stuck {
            let (lf, inf) = cache.cell(v, front, state_at);
            (lc, oc) = (lf, shade(inf));
        } else {
            let inf = cache.info(v, front, state_at);
            lc = if inf & info::SOLID_RENDER == 0 { cache.lum(v, front, state_at) } else { cache.lum(v, p, state_at) };
            oc = shade(cache.info(v, base, state_at));
        }
    } else {
        l = pos_i.map(|q| cache.lum(v, q, state_at));
        o = pos_i.map(|q| shade(cache.info(v, q, state_at)));
        let perm: [bool; 4] = pos_i.map(|q| lets_light_through(cache.info(v, add(q, d), state_at)));
        diags = CORNERS.map(|(i, j)| {
            if !perm[i] && !perm[j] {
                (l[0], o[0])
            } else {
                let q = add(pos_i[i], sides[j]);
                (cache.lum(v, q, state_at), shade(cache.info(v, q, state_at)))
            }
        });
        let mut c = cache.lum(v, p, state_at);
        let front = add(p, d);
        if is_stuck || cache.info(v, front, state_at) & info::SOLID_RENDER == 0 {
            c = cache.lum(v, front, state_at);
        }
        lc = c;
        oc = shade(cache.info(v, base, state_at));
    }
    mix_corners(l, o, diags, lc, oc)
}

/// [`face_corners`] read through addresses (`fixed_offsets`, `prop_bits`, `raw_grid`): the same
/// cells in the same roles, one read of light and properties per cell.
#[inline]
fn face_corners_by_address<C: CellKind, F: Fn(i32, i32, i32) -> u32>(
    src: &Sources<'_, '_, F>,
    cache: &mut LightCache<C>,
    p: [i32; 3],
    d: Dir,
    is_stuck: bool,
) -> Corners {
    let reads = &FACE_READS[d as usize][usize::from(is_stuck)];
    let block = cache.block_cells(p);
    // Only the cells beyond the edges of a glued face can leave the 18³ grids (block on the
    // section boundary, face looking out): one test on the normal axis for the four of them.
    let beyond_in_grid = !is_stuck || cache.grid_index(plus(p, reads.beyond[0].delta)).is_some();
    let at = |cache: &LightCache<C>, o: CellOffset| cache.address(p, block, o, true);
    let edges: [(u16, u16); 4] = reads.edges.map(|o| {
        let a = at(cache, o);
        cache.cell_at(src, a)
    });
    let l = edges.map(|c| c.0);
    let o = edges.map(|c| shade(c.1));
    let open: [bool; 4] = reads.beyond.map(|o| {
        let a = cache.address(p, block, o, beyond_in_grid);
        lets_light_through(cache.flags_at(src, a))
    });
    let diags: [(u16, f32); 4] = std::array::from_fn(|k| {
        let (i, j) = CORNERS[k];
        if !open[i] && !open[j] {
            return (l[0], o[0]);
        }
        let a = at(cache, reads.diagonals[k]);
        let (lq, iq) = cache.cell_at(src, a);
        (lq, shade(iq))
    });
    let front = at(cache, reads.front);
    let (lc, oc) = if is_stuck {
        // The base is the block in front.
        let (lf, inf) = cache.cell_at(src, front);
        (lf, shade(inf))
    } else {
        let centre = at(cache, reads.base);
        let lit_from = if cache.flags_at(src, front) & info::SOLID_RENDER == 0 { front } else { centre };
        (cache.cell_at(src, lit_from).0, shade(cache.flags_at(src, centre)))
    };
    mix_corners(l, o, diags, lc, oc)
}

/// Light per vertex of a quad of block `p` (state `s`); `src.state_at` gives the state of a
/// position.
pub fn quad_light<C: CellKind, F: Fn(i32, i32, i32) -> u32>(
    src: &Sources<'_, '_, F>,
    cache: &mut LightCache<C>,
    p: [i32; 3],
    s: u32,
    q: &QuadGeom<'_>,
    smooth: bool,
) -> Vertices {
    let v = src.view;
    let me = v.info_of(s);
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
    let corners_of = |cache: &mut LightCache<C>| {
        if cache.paths.addressed() {
            face_corners_by_address(src, cache, p, d, is_stuck)
        } else {
            face_corners(v, cache, src.state_at, p, d, is_stuck)
        }
    };
    let corner = if cache.paths.corners || cache.paths.neigh {
        cache.begin_block(p);
        let key = d as usize * 2 + usize::from(is_stuck);
        if cache.paths.corners && cache.corner_ok >> key & 1 == 1 {
            cache.corners[key]
        } else {
            let c = corners_of(cache);
            if cache.paths.corners {
                cache.corners[key] = c;
                cache.corner_ok |= 1 << key;
            }
            c
        }
    } else {
        corners_of(cache)
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

    #[test]
    fn wide_cell_is_twelve_bytes() {
        assert_eq!(size_of::<Wide>(), 12);
        assert_eq!(size_of::<Narrow>(), 6);
    }

    /// Light sheet with pseudo-random levels (fixed seed).
    fn random_sheet(seed: u64) -> std::sync::Arc<[u8; rc_world::LIGHT_BYTES]> {
        let mut x = seed | 1;
        let mut s = [0u8; rc_world::LIGHT_BYTES];
        for b in &mut s {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *b = x as u8;
        }
        std::sync::Arc::new(s)
    }

    /// Cache with every kind of slot around the section: own sheets, sky read from the bottom of
    /// a section above, levels, no block data, and the shared zero sheet.
    fn cache_with_mixed_slots<'a>(
        sheets: &'a [std::sync::Arc<[u8; rc_world::LIGHT_BYTES]>],
        zero: &'a [u8; rc_world::LIGHT_BYTES],
    ) -> LightCache<'a, Wide> {
        let mut c = LightCache::new(LightPaths::OLD, CellStore::default(), Vec::new());
        c.base_y = 32;
        c.slots = std::array::from_fn(|i| {
            let sheet = |k: usize| &*sheets[(i * 7 + k) % sheets.len()];
            let block = match i % 4 {
                0 => None,
                1 => Some(zero),
                _ => Some(sheet(0)),
            };
            let sky = match i % 5 {
                0 => Sky::Layer(sheet(1)),
                1 => Sky::Bottom(sheet(2)),
                2 => Sky::Level((i % 16) as u8),
                3 => Sky::Layer(zero),
                _ => Sky::Bottom(zero),
            };
            Slot { block, sky }
        });
        c
    }

    /// Every cell of the raw grid equals the lazy read of the same position, with the scalar body
    /// and with the processor's best version, with and without the zero sheet shortcut.
    #[test]
    fn raw_grid_matches_lazy_reads() {
        let zero = rc_world::zero_layer();
        let sheets: Vec<_> = (1..6).map(random_sheet).collect();
        let mut c = cache_with_mixed_slots(&sheets, &zero);
        for zero_shortcut in [false, true] {
            if zero_shortcut {
                c.slots = c.slots.map(|s| s.without_zero_sheet(&zero));
            }
            let mut best = Vec::new();
            fill_raw_grid(&c.slots, &mut best);
            let mut scalar = vec![0u16; GRID_CELLS];
            fill_raw_rows(&c.slots, &mut scalar);
            for gy in 0..GSIDE {
                for gz in 0..GSIDE {
                    for gx in 0..GSIDE {
                        let p = [gx - 1, c.base_y + gy - 1, gz - 1];
                        let i = c.grid_index(p).expect("inside the grid");
                        let want = pack(c.raw(p), 0);
                        assert_eq!(scalar[i], want, "scalar, cell {p:?}, zero shortcut {zero_shortcut}");
                        assert_eq!(best[i], want, "best version, cell {p:?}, zero shortcut {zero_shortcut}");
                    }
                }
            }
        }
    }

    /// Cost of one raw grid fill (best of 9 × 2,000), mixed slots and all-dark slots, baseline
    /// x86-64 body and AVX2 version: `cargo test --release -p rc-mesh --lib raw_grid_fill_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn raw_grid_fill_cost() {
        let zero = rc_world::zero_layer();
        let sheets: Vec<_> = (1..6).map(random_sheet).collect();
        let mixed = cache_with_mixed_slots(&sheets, &zero);
        let dark = [Slot { block: None, sky: Sky::Level(0) }; 27];
        let mut out = vec![0u16; GRID_CELLS];
        let time = |f: &mut dyn FnMut()| {
            (0..9)
                .map(|_| {
                    let s = std::time::Instant::now();
                    (0..2000).for_each(|_| f());
                    s.elapsed().as_secs_f64() * 1e9 / 2000.0
                })
                .fold(f64::MAX, f64::min)
        };
        for (name, slots) in [("mixed", mixed.slots), ("dark", dark)] {
            let best = time(&mut || fill_raw_grid(std::hint::black_box(&slots), &mut out));
            let scalar = time(&mut || fill_raw_rows(std::hint::black_box(&slots), &mut out));
            eprintln!("raw grid fill, {name} slots: {best:.0} ns (AVX2 when present), {scalar:.0} ns (baseline x86-64)");
        }
    }

    /// The zero sheet shortcut leaves every read of every cell of the cache unchanged.
    #[test]
    fn zero_sheet_reads_as_its_constant() {
        let zero = rc_world::zero_layer();
        let sheets: Vec<_> = (7..10).map(random_sheet).collect();
        let c = cache_with_mixed_slots(&sheets, &zero);
        for s in c.slots {
            let n = s.without_zero_sheet(&zero);
            for i in 0..4096 {
                let (x, y, z) = (i & 15, i >> 8, (i >> 4) & 15);
                assert_eq!(n.read(x, y, z), s.read(x, y, z), "cell {i}");
            }
        }
    }

    #[test]
    fn emission_on_packed_raw_matches_the_cell_read() {
        let props = [0, 3, 15, info::EMISSIVE, info::EMISSIVE | 7, info::SOLID_RENDER | 9];
        for b in 0..16u8 {
            for k in 0..16u8 {
                for inf in props {
                    let raw = u16::from(b) << 4 | u16::from(k) << 12;
                    // The lazy cell read: emissive render first, then the packed levels.
                    let want = if inf & info::EMISSIVE != 0 { 0xf0f0 } else { pack((b, k), inf) };
                    assert_eq!(with_emission(raw, inf), want, "block {b}, sky {k}, info {inf:#x}");
                }
            }
        }
    }

    /// The property word gives the face corners the same answers as the light table, for every
    /// combination of the light bits.
    #[test]
    fn prop_word_flags_match_the_light_table() {
        let bits = [info::SOLID_RENDER, info::NO_DAMPENING, info::DARK_SHADE, info::EMISSIVE, info::FULL_COLLISION];
        for m in 0..1u32 << bits.len() {
            let inf = bits.iter().enumerate().filter(|(k, _)| m >> k & 1 == 1).fold(5u16, |a, (_, &b)| a | b);
            let from_word = info_of_prop_word(crate::cull::light_bits(inf));
            assert_eq!(lets_light_through(from_word), lets_light_through(inf), "info {inf:#x}");
            assert_eq!(shade(from_word), shade(inf), "info {inf:#x}");
            assert_eq!(from_word & info::SOLID_RENDER, inf & info::SOLID_RENDER, "info {inf:#x}");
        }
        // Outside the columns (word 0): air, which lets the light through.
        assert!(lets_light_through(info_of_prop_word(0)));
    }

    /// The fixed offsets are the positions that the coordinate walk reads, and their cache and
    /// grid steps land on the cells that locating those positions gives.
    #[test]
    fn face_reads_match_the_coordinate_walk() {
        let fixed = LightPaths { fixed_offsets: true, ..LightPaths::OLD };
        let mut c = LightCache::new(fixed, CellStore::<Wide>::default(), Vec::new());
        c.base_y = 48;
        for d in Dir::ALL {
            for glued in [false, true] {
                let r = &FACE_READS[d as usize][usize::from(glued)];
                let base = if glued { add([0; 3], d) } else { [0; 3] };
                let sides = SIDES[d as usize];
                for k in 0..4 {
                    let edge = add(base, sides[k]);
                    assert_eq!(r.edges[k].delta, edge, "{d:?} glued {glued} edge {k}");
                    assert_eq!(r.beyond[k].delta, add(edge, d), "{d:?} glued {glued} beyond {k}");
                    let (i, j) = CORNERS[k];
                    assert_eq!(r.diagonals[k].delta, add(add(base, sides[i]), sides[j]), "{d:?} diagonal {k}");
                }
                assert_eq!(r.front.delta, add([0; 3], d));
                assert_eq!(r.base.delta, base);
                // Every block of the section: the offsets land where the coordinates do.
                let all = r.edges.iter().chain(&r.beyond).chain(&r.diagonals).chain([&r.front, &r.base]);
                for o in all {
                    for i in (0..4096i32).step_by(7) {
                        let p = [i & 15, c.base_y + (i >> 8), (i >> 4) & 15];
                        let block = c.block_cells(p);
                        let q = plus(p, o.delta);
                        let fixed = c.address(p, block, *o, true);
                        assert_eq!(Some(fixed.cache), c.index(q), "{d:?} {q:?}");
                        if let Some(g) = c.grid_index(q) {
                            assert_eq!(fixed.grid, Some(g), "{d:?} {q:?}");
                        }
                    }
                }
            }
        }
    }
}
