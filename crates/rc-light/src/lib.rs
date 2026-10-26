//! Light engine (`docs/specs/lighting.md` § 1.4), shared by the client and the singleplayer
//! integrated server: after a block change,
//! block and sky light are recomputed over the zone it can touch, up to the fixed
//! point described by the spec (stable rule). Exact resolution of the zone: its cells restart from
//! their sources (emission, sky columns), the neighbouring cells outside the zone keep their value
//! and serve as seeds, then propagation by decreasing levels.

mod fast;
mod full;
mod received;
pub use full::Columns;
pub use received::Received;

/// Throughput counters (measurements, `docs/perf.md`): chunks lit in full and
/// relights after block changes, with their cumulative time, for the whole process
/// (client and integrated server). Two clock reads per call, nothing else.
pub mod stats {
    use std::sync::atomic::{AtomicU64, Ordering::Relaxed};

    pub(crate) static CHUNKS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static CHUNK_NS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static RELIGHTS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static CHANGES: AtomicU64 = AtomicU64::new(0);
    pub(crate) static RELIGHT_NS: AtomicU64 = AtomicU64::new(0);
    /// Sky and block light zone resolutions (one per group of changes and
    /// per kind of light) and their time: separate the two costs in a relight.
    pub(crate) static SKY_SOLVES: AtomicU64 = AtomicU64::new(0);
    pub(crate) static SKY_NS: AtomicU64 = AtomicU64::new(0);
    pub(crate) static BLOCK_SOLVES: AtomicU64 = AtomicU64::new(0);
    pub(crate) static BLOCK_NS: AtomicU64 = AtomicU64::new(0);

    /// Totals since startup.
    #[derive(Clone, Copy, Debug, Default)]
    pub struct Totals {
        pub chunks: u64,
        pub chunk_ns: u64,
        pub relights: u64,
        pub changes: u64,
        pub relight_ns: u64,
        pub sky_solves: u64,
        pub sky_ns: u64,
        pub block_solves: u64,
        pub block_ns: u64,
    }

    impl Totals {
        fn zip(&self, o: &Totals, f: impl Fn(u64, u64) -> u64) -> Totals {
            Totals {
                chunks: f(self.chunks, o.chunks),
                chunk_ns: f(self.chunk_ns, o.chunk_ns),
                relights: f(self.relights, o.relights),
                changes: f(self.changes, o.changes),
                relight_ns: f(self.relight_ns, o.relight_ns),
                sky_solves: f(self.sky_solves, o.sky_solves),
                sky_ns: f(self.sky_ns, o.sky_ns),
                block_solves: f(self.block_solves, o.block_solves),
                block_ns: f(self.block_ns, o.block_ns),
            }
        }

        /// Difference between two readings (`self` after `before`).
        pub fn since(&self, before: &Totals) -> Totals {
            self.zip(before, |a, b| a - b)
        }

        /// Sum of two differences.
        pub fn plus(&self, other: &Totals) -> Totals {
            self.zip(other, |a, b| a + b)
        }
    }

    pub fn totals() -> Totals {
        Totals {
            chunks: CHUNKS.load(Relaxed),
            chunk_ns: CHUNK_NS.load(Relaxed),
            relights: RELIGHTS.load(Relaxed),
            changes: CHANGES.load(Relaxed),
            relight_ns: RELIGHT_NS.load(Relaxed),
            sky_solves: SKY_SOLVES.load(Relaxed),
            sky_ns: SKY_NS.load(Relaxed),
            block_solves: BLOCK_SOLVES.load(Relaxed),
            block_ns: BLOCK_NS.load(Relaxed),
        }
    }
}

/// Frozen copy of the resolutions (reference for the benches and equivalence tests): never
/// optimised, it gives the expected result, byte for byte, of any new path.
#[doc(hidden)]
pub mod reference;

/// Full light of the central chunk (see `full`), counted in `stats`.
pub fn light_chunk(t: &LightTables, cols: Columns, height: i32, has_sky: bool) -> (Vec<LightLayer>, Vec<LightLayer>) {
    light_chunk_scanned(t, cols, [None; 9], height, has_sky)
}

/// Like [`light_chunk`], with the scan of each column already done ([`scan_column`] on the
/// same states): `None` for a column redoes it on the fly. Switches: see `full`.
pub fn light_chunk_scanned(
    t: &LightTables,
    cols: Columns,
    scans: Scans,
    height: i32,
    has_sky: bool,
) -> (Vec<LightLayer>, Vec<LightLayer>) {
    use std::sync::atomic::Ordering::Relaxed;
    let t0 = std::time::Instant::now();
    let o = full::FullOpts::current();
    let out = if o == full::FullOpts::OLD {
        full::light_chunk_inner(t, cols, height, has_sky)
    } else {
        full::light_chunk_opt(t, cols, scans, height, has_sky, o)
    };
    stats::CHUNK_NS.fetch_add(t0.elapsed().as_nanos() as u64, Relaxed);
    stats::CHUNKS.fetch_add(1, Relaxed);
    out
}

pub use full::{ColumnScan, Scans, scan_column};

/// Benches: [`light_chunk_scanned`] with given switches (`skip`, `noemit`, `layout`)
/// instead of the process's, to compare the variants in one executable (alternating
/// passes). All false: old path. Not counted in `stats`.
#[doc(hidden)]
pub fn light_chunk_with(
    t: &LightTables,
    cols: Columns,
    scans: Scans,
    height: i32,
    has_sky: bool,
    [skip, noemit, layout]: [bool; 3],
) -> (Vec<LightLayer>, Vec<LightLayer>) {
    let o = full::FullOpts { skip, noemit, layout };
    if o == full::FullOpts::OLD {
        full::light_chunk_inner(t, cols, height, has_sky)
    } else {
        full::light_chunk_opt(t, cols, scans, height, has_sky, o)
    }
}

use hashbrown::HashSet;
use std::sync::Arc;

use rc_data::{VersionData, flags};
use rc_world::{ChunkGrid, LIGHT_BYTES, LightLayer, Section};

/// Columns seen by the engine: blocks (sections from bottom to top) and light sheets
/// (`sections + 2` positions, one below the world). `Sync`: the parallel resolutions
/// (`LIGHTJOIN`, `LIGHTPARSOLVE`, `LIGHTGROUPS`) read the grid from several threads.
pub trait LightGrid: Sync {
    /// Sections, sky sheets and block sheets of a loaded column.
    fn column(&self, cx: i32, cz: i32) -> Option<(&[Section], &[LightLayer], &[LightLayer])>;
    fn layers_mut(&mut self, cx: i32, cz: i32, sky: bool) -> Option<&mut [LightLayer]>;
}

impl LightGrid for ChunkGrid {
    fn column(&self, cx: i32, cz: i32) -> Option<(&[Section], &[LightLayer], &[LightLayer])> {
        let c = self.get(cx, cz)?;
        Some((&c.sections, &c.sky_light, &c.block_light))
    }

    fn layers_mut(&mut self, cx: i32, cz: i32, sky: bool) -> Option<&mut [LightLayer]> {
        let c = self.get_mut(cx, cz)?;
        Some(if sky { &mut c.sky_light } else { &mut c.block_light })
    }
}

/// Light properties per state.
pub struct LightTables {
    emission: Vec<u8>,
    dampening: Vec<u8>,
    /// Index of the face shapes in `masks` (0: no occluding face).
    face: Vec<u32>,
    /// Per direction (order down, up, north, south, west, east), 16 × 16 square of the face covered.
    masks: Vec<[[u64; 4]; 6]>,
    /// Occluded edge per pair of shapes `(a, b)` (index `a · n + b`, n = `masks.len()`): bit
    /// `dir` set if the passage from `a` to its neighbour `b` in direction `dir` is blocked.
    /// Empty if there are too many shapes (direct computation on the masks).
    occ: Vec<u8>,
    /// "Neutral" state for the full light: zero emission and attenuation (known state),
    /// no occluding face (air, cave air, void air…).
    neutral: Vec<bool>,
}

/// Shapes at most for the table of occluded edges (n² bytes: 256 KiB at 512).
const OCC_MAX: usize = 512;

const FULL: [u64; 4] = [u64::MAX; 4];
const OPPOSITE: [usize; 6] = [1, 0, 3, 2, 5, 4];
const STEP: [[i32; 3]; 6] = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];

impl LightTables {
    pub fn new(d: &VersionData) -> Self {
        let mut masks = vec![[[0u64; 4]; 6]];
        let mut by_shape = hashbrown::HashMap::new();
        let face = d
            .states
            .iter()
            .map(|s| {
                if !s.has(flags::CAN_OCCLUDE) || !s.has(flags::SHAPE_LIGHT_OCCLUSION) {
                    return 0;
                }
                *by_shape.entry(s.occlusion).or_insert_with(|| {
                    let (a, b) = d.shapes.get(s.occlusion as usize).copied().unwrap_or((0, 0));
                    let boxes = &d.aabbs[a as usize..b as usize];
                    let m = face_masks(boxes);
                    if m.iter().all(|f| f.iter().all(|&w| w == 0)) {
                        return 0;
                    }
                    masks.push(m);
                    (masks.len() - 1) as u32
                })
            })
            .collect();
        LightTables::from_parts(
            d.states.iter().map(|s| s.light_emission).collect(),
            d.states.iter().map(|s| s.light_dampening).collect(),
            face,
            masks,
        )
    }

    /// Tables from their per-state properties (tests), with the table of occluded edges.
    fn from_parts(emission: Vec<u8>, dampening: Vec<u8>, face: Vec<u32>, masks: Vec<[[u64; 4]; 6]>) -> Self {
        let n = masks.len();
        let mut occ = Vec::new();
        if n <= OCC_MAX {
            occ = vec![0u8; n * n];
            for a in 0..n {
                for b in 0..n {
                    for dir in 0..6 {
                        let (ma, mb) = (masks[a][dir], masks[b][OPPOSITE[dir]]);
                        if (a != 0 || b != 0) && (0..4).all(|i| ma[i] | mb[i] == FULL[i]) {
                            occ[a * n + b] |= 1 << dir;
                        }
                    }
                }
            }
        }
        let neutral = (0..emission.len().min(dampening.len()).min(face.len()))
            .map(|s| emission[s] == 0 && dampening[s] == 0 && face[s] == 0)
            .collect();
        LightTables { emission, dampening, face, masks, occ, neutral }
    }

    /// See the `neutral` field (unknown state: not neutral).
    #[inline]
    fn neutral(&self, s: u32) -> bool {
        self.neutral.get(s as usize).copied().unwrap_or(false)
    }

    /// The state change touches the sky light (different attenuation or shape): the
    /// sky resolution only reads the state through them, emission does not enter into it.
    pub fn matters_sky(&self, old: u32, new: u32) -> bool {
        self.opacity(old) != self.opacity(new) || self.damps(old) != self.damps(new) || self.face(old) != self.face(new)
    }

    /// Non-zero attenuation (sky column rule; unknown state: none).
    fn damps(&self, s: u32) -> bool {
        self.dampening.get(s as usize).copied().unwrap_or(0) != 0
    }

    /// The sky goes through the state with no loss or occluded edge, whatever the neighbouring cell
    /// below or above of the same kind (zero attenuation, no occluding face).
    fn sky_clear(&self, s: u32) -> bool {
        !self.damps(s) && self.face(s) == 0
    }

    /// [`Self::occluded`] on shape indices; `tab`: read of the pair table.
    #[inline]
    fn occluded_faces(&self, fa: u32, fb: u32, dir: usize, tab: bool) -> bool {
        if fa == 0 && fb == 0 {
            return false;
        }
        let n = self.masks.len();
        if tab && !self.occ.is_empty() {
            return self.occ[fa as usize * n + fb as usize] >> dir & 1 != 0;
        }
        let (ma, mb) = (self.masks[fa as usize][dir], self.masks[fb as usize][OPPOSITE[dir]]);
        (0..4).all(|i| ma[i] | mb[i] == FULL[i])
    }

    /// The state change touches the light (different emission, attenuation or shape): the
    /// resolution only reads the state through these three properties, so two states that
    /// share them (dirt turned to grass, update of an unchanged block) change nothing.
    pub fn matters(&self, old: u32, new: u32) -> bool {
        let g = |v: &[u8], s: u32| v.get(s as usize).copied().unwrap_or(0);
        g(&self.emission, old) != g(&self.emission, new)
            || g(&self.dampening, old) != g(&self.dampening, new)
            || self.face(old) != self.face(new)
    }

    fn face(&self, s: u32) -> u32 {
        self.face.get(s as usize).copied().unwrap_or(0)
    }

    fn opacity(&self, s: u32) -> u8 {
        self.dampening.get(s as usize).copied().unwrap_or(15).max(1)
    }

    /// Blocked passage between `a` and its neighbour `b` in direction `dir`.
    fn occluded(&self, a: u32, b: u32, dir: usize) -> bool {
        let (fa, fb) = (self.face(a), self.face(b));
        if fa == 0 && fb == 0 {
            return false;
        }
        let (ma, mb) = (self.masks[fa as usize][dir], self.masks[fb as usize][OPPOSITE[dir]]);
        (0..4).all(|i| ma[i] | mb[i] == FULL[i])
    }
}

/// Faces covered by a shape: boxes that touch the face plane, projected onto a
/// 16 × 16 grid (block shapes are in sixteenths).
fn face_masks(boxes: &[rc_data::Aabb]) -> [[u64; 4]; 6] {
    let mut out = [[0u64; 4]; 6];
    const E: f64 = 1.0e-6;
    for b in boxes {
        for (dir, mask) in out.iter_mut().enumerate() {
            let (axis, at_max) = match dir {
                0 => (1, false),
                1 => (1, true),
                2 => (2, false),
                3 => (2, true),
                4 => (0, false),
                _ => (0, true),
            };
            let touches = if at_max { b.max[axis] >= 1.0 - E } else { b.min[axis] <= E };
            if !touches {
                continue;
            }
            let (u, v) = match axis {
                0 => (1, 2),
                1 => (0, 2),
                _ => (0, 1),
            };
            let cell = |x: f64| (x * 16.0).round().clamp(0.0, 16.0) as usize;
            for i in cell(b.min[u])..cell(b.max[u]) {
                for j in cell(b.min[v])..cell(b.max[v]) {
                    let k = j * 16 + i;
                    mask[k >> 6] |= 1 << (k & 63);
                }
            }
        }
    }
    out
}

/// Work zone: cells of the zone and a one-cell margin (fixed seeds).
struct Zone {
    lo: [i32; 3],
    size: [i32; 3],
    state: Vec<u32>,
    /// Section carrying data (writing allowed, propagation allowed).
    data: Vec<bool>,
    value: Vec<u8>,
    inner: Vec<bool>,
}

impl Zone {
    fn index(&self, p: [i32; 3]) -> Option<usize> {
        let r = [p[0] - self.lo[0], p[1] - self.lo[1], p[2] - self.lo[2]];
        (0..3)
            .all(|a| (0..self.size[a]).contains(&r[a]))
            .then(|| ((r[1] * self.size[2] + r[2]) * self.size[0] + r[0]) as usize)
    }
}

fn nibble(l: &[u8; LIGHT_BYTES], x: usize, y: usize, z: usize) -> u8 {
    let i = (y << 8) | (z << 4) | x;
    (l[i >> 1] >> ((i & 1) * 4)) & 15
}

fn set_nibble(l: &mut [u8; LIGHT_BYTES], x: usize, y: usize, z: usize, v: u8) {
    let i = (y << 8) | (z << 4) | x;
    let sh = (i & 1) * 4;
    l[i >> 1] = (l[i >> 1] & !(15 << sh)) | (v << sh);
}

/// Recomputes the light around the changed positions (world coordinates, y relative to the bottom
/// of the world); returns the columns whose light changed.
pub fn relight(
    grid: &mut impl LightGrid,
    height: i32,
    t: &LightTables,
    has_sky: bool,
    changes: &[[i32; 3]],
) -> HashSet<(i32, i32)> {
    relight_sections(grid, height, t, has_sky, changes).into_keys().map(|(x, _, z)| (x, z)).collect()
}

/// Sections whose light changed: `(column x, section, column z)` (section from the bottom
/// of the world, −1 and `height / 16` for the sheets outside the world) → edge bits of the section
/// where at least one changed cell is (`rc_world::edge`).
pub type Touched = hashbrown::HashMap<(i32, i32, i32), u8>;

/// Like [`relight`], with the touched sections and their edges (re-meshing per section).
pub fn relight_sections(
    grid: &mut impl LightGrid,
    height: i32,
    t: &LightTables,
    has_sky: bool,
    changes: &[[i32; 3]],
) -> Touched {
    timed(changes.len(), || {
        let o = fast::Opts::current();
        if o == fast::Opts::OLD {
            return relight_zones(grid, height, t, has_sky, changes, true);
        }
        let mut out = [Touched::new()];
        fast::relight_zones(grid, height, t, has_sky, changes, None, o, &mut out);
        let [touched] = out;
        touched
    })
}

/// Benches: [`relight_sections`] with the incremental relight forced (`Some(threshold)`: groups
/// of at most `threshold` changes) or off (`None`), the other switches being those of the
/// process, to compare the two in one executable. Not counted in the relights.
#[doc(hidden)]
pub fn relight_sections_inc(
    grid: &mut impl LightGrid,
    height: i32,
    t: &LightTables,
    has_sky: bool,
    changes: &[[i32; 3]],
    inc: Option<usize>,
) -> Touched {
    let o = fast::Opts { inc: inc.is_some(), inc_max: inc.unwrap_or(usize::MAX), ..fast::Opts::current() };
    if o == fast::Opts::OLD {
        return relight_zones(grid, height, t, has_sky, changes, true);
    }
    let mut out = [Touched::new()];
    fast::relight_zones(grid, height, t, has_sky, changes, None, o, &mut out);
    let [touched] = out;
    touched
}

/// Like [`relight_sections`], the sky being recomputed only around `sky_changes` (the
/// changes that touch attenuation or shape, [`LightTables::matters_sky`]): a
/// lamp that turns on only changes the block light. `RUSTCRAFT_OPT_SKYBIT=0`: sky
/// recomputed around all the changes.
pub fn relight_masked(
    grid: &mut impl LightGrid,
    height: i32,
    t: &LightTables,
    has_sky: bool,
    changes: &[[i32; 3]],
    sky_changes: &[[i32; 3]],
) -> Touched {
    let (sky, block) = relight_sections_split(grid, height, t, has_sky, changes, Some(sky_changes));
    let mut out = block;
    for (k, v) in sky {
        *out.entry(k).or_insert(0) |= v;
    }
    out
}

/// Touched sections split: `(sky, block)` (`LightUpdate` masks). `sky_changes`:
/// see [`relight_masked`] (`None`: all the changes).
pub fn relight_sections_split(
    grid: &mut impl LightGrid,
    height: i32,
    t: &LightTables,
    has_sky: bool,
    changes: &[[i32; 3]],
    sky_changes: Option<&[[i32; 3]]>,
) -> (Touched, Touched) {
    timed(changes.len(), || {
        let o = fast::Opts::current();
        let mut out = [Touched::new(), Touched::new()];
        if o == fast::Opts::OLD {
            relight_zones_split(grid, height, t, has_sky, changes, &mut out);
        } else {
            fast::relight_zones(grid, height, t, has_sky, changes, sky_changes, o, &mut out);
        }
        let [block, sky] = out;
        (sky, block)
    })
}

/// Relight counted in `stats`.
fn timed<R>(changes: usize, f: impl FnOnce() -> R) -> R {
    use std::sync::atomic::Ordering::Relaxed;
    let t0 = std::time::Instant::now();
    let out = f();
    stats::RELIGHT_NS.fetch_add(t0.elapsed().as_nanos() as u64, Relaxed);
    stats::RELIGHTS.fetch_add(1, Relaxed);
    stats::CHANGES.fetch_add(changes as u64, Relaxed);
    out
}

/// Old path of [`relight_sections_split`]: same groups and same resolutions as
/// [`relight_zones`], block sections in `out[0]`, sky ones in `out[1]`.
fn relight_zones_split(
    grid: &mut impl LightGrid,
    height: i32,
    t: &LightTables,
    has_sky: bool,
    changes: &[[i32; 3]],
    out: &mut [Touched; 2],
) {
    for members in fast::groups(changes) {
        solve(grid, height, t, &members, false, true, &mut out[0]);
        if has_sky {
            solve(grid, height, t, &members, true, true, &mut out[1]);
        }
    }
}

/// `bounded` false: sky zone over the whole height (reference for the tests).
fn relight_zones(
    grid: &mut impl LightGrid,
    height: i32,
    t: &LightTables,
    has_sky: bool,
    changes: &[[i32; 3]],
    bounded: bool,
) -> Touched {
    let mut touched = Touched::new();
    if !bounded {
        for &c in changes {
            solve(grid, height, t, &[c], false, bounded, &mut touched);
            if has_sky {
                solve(grid, height, t, &[c], true, bounded, &mut touched);
            }
        }
        return touched;
    }
    for (_, _, members) in &change_groups(changes) {
        solve(grid, height, t, members, false, bounded, &mut touched);
        if has_sky {
            solve(grid, height, t, members, true, bounded, &mut touched);
        }
    }
    touched
}

/// Maximum gap (per axis) between a change and the box of a group for it to join.
const GROUP: i32 = 8;

/// Group of changes resolved in one go: (low corner, high corner, changes).
pub type Group = ([i32; 3], [i32; 3], Vec<[i32; 3]>);

/// Neighbouring changes (liquid flow…): a single zone, their bounding box,
/// resolved in one go instead of one zone per change. Each zone reads the box of its group
/// at ±16 cells horizontally (the benches' trace uses this to write those columns).
pub fn change_groups(changes: &[[i32; 3]]) -> Vec<Group> {
    let mut groups: Vec<Group> = Vec::new();
    for &c in changes {
        let near = |lo: &[i32; 3], hi: &[i32; 3]| (0..3).all(|a| c[a] >= lo[a] - GROUP && c[a] <= hi[a] + GROUP);
        match groups.iter_mut().find(|(lo, hi, _)| near(lo, hi)) {
            Some((lo, hi, members)) => {
                for a in 0..3 {
                    lo[a] = lo[a].min(c[a]);
                    hi[a] = hi[a].max(c[a]);
                }
                members.push(c);
            }
            None => groups.push((c, c, vec![c])),
        }
    }
    groups
}

/// State of a position (`y` relative to the bottom of the world); air (0) outside the world or without column,
/// like the zone's cells.
fn state_at(grid: &impl LightGrid, x: i32, y: i32, z: i32, height: i32) -> u32 {
    if !(0..height).contains(&y) {
        return 0;
    }
    grid.column(x >> 4, z >> 4).map_or(0, |(sections, _, _)| {
        sections
            .get((y >> 4) as usize)
            .map_or(0, |s| s.blocks.get(rc_world::block_index((x & 15) as usize, (y & 15) as usize, (z & 15) as usize)))
    })
}

/// The sky descends without an occluded edge from the top of the world to `from` included (rule of
/// [`source_height`] applied to the edges `(y + 1, y)` for `y ≥ from`).
fn clear_above(grid: &impl LightGrid, t: &LightTables, x: i32, z: i32, from: i32, height: i32) -> bool {
    let Some((sections, _, _)) = grid.column(x >> 4, z >> 4) else { return true };
    let (lx, lz) = ((x & 15) as usize, (z & 15) as usize);
    let at = |y: i32| {
        if (0..height).contains(&y) {
            sections
                .get((y >> 4) as usize)
                .map_or(0, |s| s.blocks.get(rc_world::block_index(lx, (y & 15) as usize, lz)))
        } else {
            0
        }
    };
    let mut hi = at(height);
    for y in (from.max(-1)..height).rev() {
        let lo = at(y);
        if t.dampening.get(lo as usize).copied().unwrap_or(0) != 0 || t.occluded(hi, lo, 0) {
            return false;
        }
        hi = lo;
    }
    true
}

fn solve(
    grid: &mut impl LightGrid,
    height: i32,
    t: &LightTables,
    cs: &[[i32; 3]],
    sky: bool,
    bounded: bool,
    touched: &mut Touched,
) {
    let t0 = std::time::Instant::now();
    solve_inner(grid, height, t, cs, sky, bounded, touched);
    use std::sync::atomic::Ordering::Relaxed;
    let ns = t0.elapsed().as_nanos() as u64;
    let (count, time) =
        if sky { (&stats::SKY_SOLVES, &stats::SKY_NS) } else { (&stats::BLOCK_SOLVES, &stats::BLOCK_NS) };
    count.fetch_add(1, Relaxed);
    time.fetch_add(ns, Relaxed);
}

fn solve_inner(
    grid: &mut impl LightGrid,
    height: i32,
    t: &LightTables,
    cs: &[[i32; 3]],
    sky: bool,
    bounded: bool,
    touched: &mut Touched,
) {
    // Box of the changed blocks.
    let (mut bmin, mut bmax) = (cs[0], cs[0]);
    for c in cs {
        for a in 0..3 {
            bmin[a] = bmin[a].min(c[a]);
            bmax[a] = bmax[a].max(c[a]);
        }
    }
    // Zone: ±15 (block) or ±15 horizontally (sky) around the changes, plus a margin.
    // Sky, in height: propagation loses at least 1 per step, only the column of sources
    // (level 15 without loss) passing through a changed block can go further, downwards,
    // through the cells the sky crosses beneath it. Zone: from the bottom of these columns − 15
    // to the highest change + 15 (everything else is out of reach and keeps its value).
    let (ylo, yhi) = if sky && !bounded {
        (0, height - 1)
    } else if sky {
        let mut bottom = bmin[1];
        for c in cs {
            let mut b = c[1] - 1;
            while b >= 0 {
                let s = state_at(grid, c[0], b, c[2], height);
                let edge_ok = b == c[1] - 1 || !t.occluded(state_at(grid, c[0], b + 1, c[2], height), s, 0);
                if t.dampening.get(s as usize).copied().unwrap_or(0) != 0 || !edge_ok {
                    break;
                }
                b -= 1;
            }
            bottom = bottom.min(b + 1);
        }
        ((bottom - 15).max(0), (bmax[1] + 15).min(height - 1))
    } else {
        ((bmin[1] - 15).max(0), (bmax[1] + 15).min(height - 1))
    };
    let lo = [bmin[0] - 16, ylo - 1, bmin[2] - 16];
    let size = [bmax[0] - bmin[0] + 33, yhi - ylo + 3, bmax[2] - bmin[2] + 33];
    let n = (size[0] * size[1] * size[2]) as usize;
    let mut z = Zone { lo, size, state: vec![0; n], data: vec![false; n], value: vec![0; n], inner: vec![false; n] };
    // Read: states, data present, stored values.
    for dz in 0..size[2] {
        for dx in 0..size[0] {
            let (x, wz) = (lo[0] + dx, lo[2] + dz);
            let col = grid.column(x >> 4, wz >> 4);
            let (lx, lz) = ((x & 15) as usize, (wz & 15) as usize);
            for dy in 0..size[1] {
                let y = lo[1] + dy;
                let i = ((dy * size[2] + dz) * size[0] + dx) as usize;
                let inner =
                    (1..size[0] - 1).contains(&dx) && (1..size[2] - 1).contains(&dz) && (1..size[1] - 1).contains(&dy);
                z.inner[i] = inner;
                let Some((sections, sky_layers, block_layers)) = col else { continue };
                if !(0..height).contains(&y) {
                    // Outside the world: above, full sky; below, nothing.
                    z.value[i] = if sky && y >= height { 15 } else { 0 };
                    continue;
                }
                z.state[i] = sections
                    .get((y >> 4) as usize)
                    .map_or(0, |s| s.blocks.get(rc_world::block_index(lx, (y & 15) as usize, lz)));
                let layers = if sky { sky_layers } else { block_layers };
                if let Some(Some(l)) = layers.get(((y >> 4) + 1) as usize) {
                    z.data[i] = true;
                    z.value[i] = nibble(l, lx, (y & 15) as usize, lz);
                }
            }
        }
    }
    // Sources of the zone's cells: emission, or sky columns above their source height.
    let mut init = vec![0u8; n];
    // Sky above the zone: the stack only carries sources if the sky descends without an occluded
    // edge from the top of the world to the top margin.
    let top_margin = lo[1] + size[1] - 1;
    for dz in 1..size[2] - 1 {
        for dx in 1..size[0] - 1 {
            let clear = || clear_above(&*grid, t, lo[0] + dx, lo[2] + dz, top_margin, height);
            let source_from = if sky && clear() { source_height(&z, t, dx, dz, height) } else { i32::MAX };
            for dy in 1..size[1] - 1 {
                let i = ((dy * size[2] + dz) * size[0] + dx) as usize;
                if !z.data[i] {
                    continue;
                }
                init[i] = if sky {
                    if lo[1] + dy >= source_from { 15 } else { 0 }
                } else {
                    t.emission.get(z.state[i] as usize).copied().unwrap_or(0)
                };
            }
        }
    }
    // Propagation by levels: seeds = zone sources and margin cells.
    let mut level = vec![0u8; n];
    let mut buckets: Vec<Vec<usize>> = vec![Vec::new(); 16];
    for i in 0..n {
        let v = if z.inner[i] {
            init[i]
        } else if z.data[i] || !sky || z.value[i] > 0 {
            z.value[i]
        } else {
            0
        };
        level[i] = v;
        if v > 1 && (z.inner[i] || z.data[i] || z.value[i] == 15) {
            buckets[v as usize].push(i);
        }
    }
    for l in (2..16usize).rev() {
        while let Some(i) = buckets[l].pop() {
            if level[i] as usize != l {
                continue;
            }
            let p = [
                lo[0] + (i as i32 % size[0]),
                lo[1] + (i as i32 / (size[0] * size[2])),
                lo[2] + ((i as i32 / size[0]) % size[2]),
            ];
            for (dir, s) in STEP.iter().enumerate() {
                let q = [p[0] + s[0], p[1] + s[1], p[2] + s[2]];
                let Some(j) = z.index(q) else { continue };
                if !z.inner[j] || !z.data[j] || t.occluded(z.state[i], z.state[j], dir) {
                    continue;
                }
                let nv = l as i32 - i32::from(t.opacity(z.state[j]));
                if nv > i32::from(level[j]) {
                    level[j] = nv as u8;
                    buckets[nv as usize].push(j);
                }
            }
        }
    }
    // Writing of the changed cells.
    for dz in 1..size[2] - 1 {
        for dx in 1..size[0] - 1 {
            let (x, wz) = (lo[0] + dx, lo[2] + dz);
            let (lx, lz) = ((x & 15) as usize, (wz & 15) as usize);
            let changed: Vec<(i32, u8)> = (1..size[1] - 1)
                .filter_map(|dy| {
                    let i = ((dy * size[2] + dz) * size[0] + dx) as usize;
                    (z.data[i] && level[i] != z.value[i]).then_some((lo[1] + dy, level[i]))
                })
                .collect();
            if changed.is_empty() {
                continue;
            }
            let Some(layers) = grid.layers_mut(x >> 4, wz >> 4, sky) else { continue };
            // Edges of the changed cells, accumulated per section along the stack (increasing y) then
            // recorded once per section (rather than one table entry per changed cell).
            let flat = rc_world::edge::ANY
                | rc_world::edge::bits(lx, rc_world::edge::X0)
                | rc_world::edge::bits(lz, rc_world::edge::Z0);
            let mut run: Option<(i32, u8)> = None;
            for (y, v) in changed {
                if let Some(Some(l)) = layers.get_mut(((y >> 4) + 1) as usize) {
                    set_nibble(Arc::make_mut(l), lx, (y & 15) as usize, lz, v);
                }
                let bits = flat | rc_world::edge::bits((y & 15) as usize, rc_world::edge::Y0);
                match &mut run {
                    Some((s, acc)) if *s == y >> 4 => *acc |= bits,
                    _ => {
                        if let Some((s, acc)) = run.replace((y >> 4, bits)) {
                            *touched.entry((x >> 4, s, wz >> 4)).or_insert(0) |= acc;
                        }
                    }
                }
            }
            if let Some((s, acc)) = run {
                *touched.entry((x >> 4, s, wz >> 4)).or_insert(0) |= acc;
            }
        }
    }
}

/// Sky source height of a stack of the zone (§ 1.4): the smallest y whose
/// positions up to the top are all sources.
fn source_height(z: &Zone, t: &LightTables, dx: i32, dz: i32, height: i32) -> i32 {
    let at = |dy: i32| z.state[((dy * z.size[2] + dz) * z.size[0] + dx) as usize];
    // From the top downwards: first occluded edge (bottom attenuates, or joined faces full).
    let mut top = z.size[1] - 2;
    while top >= 1 && z.lo[1] + top >= height {
        top -= 1;
    }
    for dy in (1..=top).rev() {
        let (hi, lo) = (at(dy + 1), at(dy));
        let damp = t.dampening.get(lo as usize).copied().unwrap_or(0);
        if damp != 0 || t.occluded(hi, lo, 0) {
            return z.lo[1] + dy + 1;
        }
    }
    i32::MIN
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Air, stone, dirt (same properties as stone), torch, slab (other shape).
    fn tables() -> LightTables {
        LightTables::from_parts(
            vec![0, 0, 0, 14, 0],
            vec![0, 15, 15, 0, 0],
            vec![0, 1, 1, 0, 2],
            vec![[[0; 4]; 6], [FULL; 6], [FULL, [0; 4], [0; 4], [0; 4], [0; 4], [0; 4]]],
        )
    }

    #[test]
    fn only_changes_of_light_properties_matter() {
        let t = tables();
        // Dirt turned to grass, or full block returned as is: no relight.
        assert!(!t.matters(1, 2));
        assert!(!t.matters(1, 1));
        assert!(!t.matters(0, 0));
        // Placing or removing a full block, torch, different shape: relight.
        assert!(t.matters(0, 1));
        assert!(t.matters(1, 0));
        assert!(t.matters(0, 3));
        assert!(t.matters(1, 4));
    }

    /// Test grid: 3 × 3 columns of 3 sections, full sky at the start (everything is air).
    type TestColumn = (Vec<Section>, Vec<LightLayer>, Vec<LightLayer>);
    struct TestGrid(hashbrown::HashMap<(i32, i32), TestColumn>);

    impl LightGrid for TestGrid {
        fn column(&self, cx: i32, cz: i32) -> Option<(&[Section], &[LightLayer], &[LightLayer])> {
            self.0.get(&(cx, cz)).map(|(s, k, b)| (&s[..], &k[..], &b[..]))
        }
        fn layers_mut(&mut self, cx: i32, cz: i32, sky: bool) -> Option<&mut [LightLayer]> {
            self.0.get_mut(&(cx, cz)).map(|(_, k, b)| if sky { &mut k[..] } else { &mut b[..] })
        }
    }

    fn grid() -> TestGrid {
        let mut m = hashbrown::HashMap::new();
        for cx in -1..=1 {
            for cz in -1..=1 {
                let sky = vec![Some(Arc::new([0xffu8; LIGHT_BYTES])); 5];
                let block = vec![Some(Arc::new([0u8; LIGHT_BYTES])); 5];
                m.insert((cx, cz), (vec![Section::empty(0, 0); 3], sky, block));
            }
        }
        TestGrid(m)
    }

    fn set(g: &mut TestGrid, p: [i32; 3], s: u32) {
        let (sections, _, _) = g.0.get_mut(&(p[0] >> 4, p[2] >> 4)).expect("column");
        let i = rc_world::block_index((p[0] & 15) as usize, (p[1] & 15) as usize, (p[2] & 15) as usize);
        sections[(p[1] >> 4) as usize].set_blocks(&[(i as u16, s)], 4, |s| s == 0);
    }

    /// Height-bounded sky zone: same light, cell for cell, as the zone over the whole
    /// height, on a random series of placements and removals (sky wells, roofs, leaves,
    /// torches, slabs).
    #[test]
    fn bounded_sky_zone_matches_full_height() {
        let t = LightTables::from_parts(
            vec![0, 0, 0, 14, 0, 0],
            vec![0, 15, 1, 0, 0, 0],
            vec![0, 1, 0, 0, 2, 0],
            vec![[[0; 4]; 6], [FULL; 6], [FULL, [0; 4], [0; 4], [0; 4], [0; 4], [0; 4]]],
        );
        let (mut a, mut b) = (grid(), grid());
        let mut seed = 0x2545_f491_u64;
        let mut rnd = |n: u64| {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            ((seed >> 33) % n) as i32
        };
        for step in 0..400 {
            // 1 to 4 changes in the same frame, grouped or not (same zone or separate zones).
            let center = [rnd(40) - 12, rnd(48), rnd(40) - 12];
            let mut ps = Vec::new();
            for _ in 0..1 + rnd(4) {
                let spread: i32 = if rnd(2) == 0 { 4 } else { 30 };
                let p = [
                    (center[0] + rnd(spread as u64) - spread / 2).clamp(-12, 27),
                    (center[1] + rnd(spread as u64) - spread / 2).clamp(0, 47),
                    (center[2] + rnd(spread as u64) - spread / 2).clamp(-12, 27),
                ];
                let s = [0, 1, 1, 1, 2, 3, 4, 5][rnd(8) as usize] as u32;
                set(&mut a, p, s);
                set(&mut b, p, s);
                ps.push(p);
            }
            let before: std::collections::HashMap<_, _> =
                a.0.iter().map(|(&k, (_, s, bl))| (k, (s.clone(), bl.clone()))).collect();
            let touched = relight_zones(&mut a, 48, &t, true, &ps, true);
            relight_zones(&mut b, 48, &t, true, &ps, false);
            // Sections returned: all those where a cell changed, with the edges of those cells.
            let mut expect = Touched::new();
            for (&(cx, cz), (_, sky, block)) in &a.0 {
                let (osky, oblock) = &before[&(cx, cz)];
                for (new, old) in [(sky, osky), (block, oblock)] {
                    for (li, (n, o)) in new.iter().zip(old).enumerate() {
                        let (Some(n), Some(o)) = (n, o) else { continue };
                        for i in 0..4096 {
                            let (x, y, z) = (i & 15, i >> 8, (i >> 4) & 15);
                            if nibble(n, x, y, z) != nibble(o, x, y, z) {
                                let bits = rc_world::edge::of_index(i);
                                *expect.entry((cx, li as i32 - 1, cz)).or_insert(0) |= bits;
                            }
                        }
                    }
                }
            }
            for (k, bits) in &expect {
                let got = touched.get(k).copied().unwrap_or(0);
                assert_eq!(got & bits, *bits, "section {k:?} at step {step}");
            }
            for (k, (_, sa, ba)) in &a.0 {
                let (_, sb, bb) = &b.0[k];
                assert!(sa == sb && ba == bb, "difference at step {step} (changes {ps:?}), column {k:?}");
            }
        }
    }

    /// Tables of the equivalence test: air, stone, leaves (attenuation 1), torch, slab,
    /// glass (no occlusion), sea lantern (15).
    fn tables_eq() -> LightTables {
        LightTables::from_parts(
            vec![0, 0, 0, 14, 0, 0, 15],
            vec![0, 15, 1, 0, 0, 0, 15],
            vec![0, 1, 0, 0, 2, 0, 1],
            vec![[[0; 4]; 6], [FULL; 6], [FULL, [0; 4], [0; 4], [0; 4], [0; 4], [0; 4]]],
        )
    }

    /// Working path of the relight = frozen copy (`reference`), byte for byte (sheets) and
    /// section by section (touched edges), over 600 steps of random placements and removals.
    #[test]
    fn relight_matches_frozen_reference() {
        let t = tables_eq();
        let (mut a, mut b) = (grid(), grid());
        let mut seed = 0x9e37_79b9_u64;
        let mut rnd = |n: u64| {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            ((seed >> 33) % n) as i32
        };
        for step in 0..600 {
            let center = [rnd(40) - 12, rnd(48), rnd(40) - 12];
            let mut ps = Vec::new();
            for _ in 0..1 + rnd(5) {
                let spread: i32 = if rnd(2) == 0 { 4 } else { 30 };
                let p = [
                    (center[0] + rnd(spread as u64) - spread / 2).clamp(-12, 27),
                    (center[1] + rnd(spread as u64) - spread / 2).clamp(0, 47),
                    (center[2] + rnd(spread as u64) - spread / 2).clamp(-12, 27),
                ];
                let s = [0, 1, 1, 1, 2, 3, 4, 5, 6][rnd(9) as usize] as u32;
                set(&mut a, p, s);
                set(&mut b, p, s);
                ps.push(p);
            }
            let ta = relight_sections(&mut a, 48, &t, step % 7 != 0, &ps);
            let tb = reference::relight_sections(&mut b, 48, &t, step % 7 != 0, &ps);
            assert_eq!(ta, tb, "touched sections at step {step}");
            for (k, (_, sa, ba)) in &a.0 {
                let (_, sb, bb) = &b.0[k];
                assert!(sa == sb && ba == bb, "difference from the reference at step {step}, column {k:?}");
            }
        }
    }

    /// Full light of a chunk = frozen copy, on random columns (terrain, caves,
    /// sources, slabs, absent columns), with and without sky.
    #[test]
    fn light_chunk_matches_frozen_reference() {
        let t = tables_eq();
        let height = 64;
        let mut seed = 0x1234_5678_u64;
        let mut rnd = |n: u64| {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (seed >> 33) % n
        };
        for case in 0..12 {
            let cols: Vec<Option<Vec<u32>>> = (0..9)
                .map(|i| {
                    if case % 4 == 3 && i % 4 == 1 {
                        return None;
                    }
                    let mut v = vec![0u32; 256 * height];
                    for (k, s) in v.iter_mut().enumerate() {
                        let y = k >> 8;
                        *s = match rnd(100) {
                            _ if y > 40 + (k & 7) => 0,
                            0..=2 => 3,
                            3 => 6,
                            4..=6 => 4,
                            7..=9 => 2,
                            10..=35 => 0,
                            36..=38 => 5,
                            _ => 1,
                        };
                    }
                    Some(v)
                })
                .collect();
            let refs: Columns = std::array::from_fn(|i| cols[i].as_deref());
            let has_sky = case % 3 != 2;
            assert_eq!(
                light_chunk(&t, refs, height as i32, has_sky),
                reference::light_chunk(&t, refs, height as i32, has_sky),
                "case {case}"
            );
        }
    }
}
