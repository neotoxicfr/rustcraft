//! Compiled samplers and their evaluation (spec § 3.4–3.8), point path and volume
//! path, as the game uses them (they are not always bit-for-bit equal).

use crate::density::{Axis, Metric, RoundKind, Unary, round_to_integer, squeeze};
use crate::math::{clamp, floor_div, floor_mod, jmax, jmin, lerp, lerp3, signum};
use crate::noise::{NoiseStack, Simplex};
use crate::volume::Volume;

mod eval_counts;

use eval_counts::count;
pub use eval_counts::{EvalCounts, count_evaluations, take_eval_counts};

/// Index of a sampler in the generator's table.
pub type Sid = u32;

#[derive(Debug, Clone)]
pub enum SSpline {
    Const(f32),
    Multi { coord: usize, locs: Vec<f32>, values: Vec<SSpline>, ders: Vec<f32> },
}

#[derive(Debug, Clone)]
pub enum S {
    Const(f32),
    /// Unshifted noise: `noise(x·xz, y·y, z·xz)`.
    Noise {
        noise: u32,
        xz: f64,
        y: f64,
    },
    ShiftedXz {
        sx: Sid,
        sz: Sid,
        noise: u32,
        xz: f64,
        y: f64,
    },
    ShiftedXyz {
        sx: Sid,
        sy: Sid,
        sz: Sid,
        noise: u32,
        xz: f64,
        y: f64,
    },
    ShiftB {
        noise: u32,
    },
    EndIslands {
        simplex: u32,
    },
    Distance {
        p: [i32; 3],
        metric: Metric,
    },
    GradClamp {
        axis: Axis,
        from: i32,
        min: i32,
        max: i32,
        from_v: f32,
        factor: f32,
    },
    GradRepeat {
        axis: Axis,
        from: i32,
        range: i32,
        from_v: f32,
        factor: f32,
    },
    GradMirror {
        axis: Axis,
        from: i32,
        range: i32,
        from_v: f32,
        factor: f32,
    },
    Add(Sid, Sid),
    ConstAdd(Sid, f32),
    ConstSub(f32, Sid),
    Sub(Sid, Sid),
    Mul(Sid, Sid),
    ConstMul(Sid, f32),
    Div(Sid, Sid),
    ConstDiv(f32, Sid),
    Min(Sid, Sid, f32),
    ConstMin(Sid, f32),
    Max(Sid, Sid, f32),
    ConstMax(Sid, f32),
    Unary(Unary, Sid),
    RoundInt(RoundKind, Sid),
    Round(RoundKind, Sid, Sid),
    PowConstBase(f64, Sid),
    PowConstExp(Sid, f64),
    Pow(Sid, Sid),
    Spline {
        spline: Box<SSpline>,
        coords: Vec<Sid>,
    },
    LerpConstFirst(Sid, f32, Sid),
    LerpConstSecond(Sid, Sid, f32),
    Lerp(Sid, Sid, Sid),
    Clamp(Sid, f32, f32),
    RangeConst {
        input: Sid,
        min: f32,
        max: f32,
        inside: f32,
        outside: f32,
    },
    Range {
        input: Sid,
        min: f32,
        max: f32,
        inside: Sid,
        outside: Sid,
    },
    IntervalSingle {
        input: Sid,
        threshold: f32,
        below: Sid,
        above: Sid,
    },
    IntervalSelect {
        input: Sid,
        thresholds: Vec<f32>,
        functions: Vec<Sid>,
    },
    Cache {
        id: u32,
        input: Sid,
    },
    Interpolated {
        input: Sid,
        cxz: i32,
        cy: i32,
        ixz: f32,
        iy: f32,
    },
    SliceX(Sid, i32),
    SliceY(Sid, i32),
    SliceZ(Sid, i32),
    SliceXz(Sid, i32, i32),
    FindTop {
        density: Sid,
        upper: Sid,
        lower: i32,
        cell: i32,
    },
}

#[derive(Debug, Default)]
struct CacheCell {
    volume: Option<Volume>,
    buf: Option<Vec<f32>>,
    key: i64,
    value: f32,
}

/// Evaluator paths (switches of `crate::opt`, frozen at first use). The equivalence tests
/// pass their own values to compare old and new path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Opts {
    /// Branches of the choices computed only on the y rows where they are used.
    pub range_yvol: bool,
    /// Buffers taken back from the context, cache read without copy, cells pre-sized.
    pub vol_pool: bool,
    /// Point spline without allocation or dynamic call.
    pub spline_stack: bool,
    /// A buffer taken from the context keeps its old values instead of being zeroed.
    pub take_without_zeroing: bool,
    /// A buffer is taken from the spare one that fits it best, and the spares are capped in bytes.
    pub best_fit_buffers: bool,
    /// A cache whose input is exact serves any volume whose points it already holds (sub-lattice).
    pub sublattice_cache_read: bool,
    /// Volume spline: the coordinate buffers are held on the stack (up to 8 coordinates).
    pub spline_inputs_on_stack: bool,
    /// A cache whose input never reads y has one point value per column: where the point would be
    /// computed, the value already known from another height answers.
    pub column_memo_y_independent: bool,
    /// Scattered floors of the generator (surface levels, cell tops) through the shared floor,
    /// not the C library (`RUSTCRAFT_OPT_FLOOR_JAVA`).
    pub floor_java: bool,
    /// Shifted noise volumes: the points handed to the noise in batches, layers outside and points
    /// inside (`RUSTCRAFT_OPT_NOISE_BATCH_POINTS`).
    pub shifted_noise_in_batches: bool,
}

impl Opts {
    /// Code from before the optimisations.
    pub const OLD: Opts = Opts {
        range_yvol: false,
        vol_pool: false,
        spline_stack: false,
        take_without_zeroing: false,
        best_fit_buffers: false,
        sublattice_cache_read: false,
        spline_inputs_on_stack: false,
        column_memo_y_independent: false,
        floor_java: false,
        shifted_noise_in_batches: false,
    };
    /// All the tracks.
    pub const NEW: Opts = Opts {
        range_yvol: true,
        vol_pool: true,
        spline_stack: true,
        take_without_zeroing: true,
        best_fit_buffers: true,
        sublattice_cache_read: true,
        spline_inputs_on_stack: true,
        column_memo_y_independent: true,
        floor_java: true,
        shifted_noise_in_batches: true,
    };

    pub fn current() -> Opts {
        static P: std::sync::LazyLock<Opts> = std::sync::LazyLock::new(|| Opts {
            range_yvol: crate::opt::range_yvol(),
            vol_pool: crate::opt::vol_pool(),
            spline_stack: crate::opt::spline_stack(),
            take_without_zeroing: crate::opt::take_without_zeroing(),
            best_fit_buffers: crate::opt::best_fit_buffers(),
            sublattice_cache_read: crate::opt::sublattice_cache_read(),
            spline_inputs_on_stack: crate::opt::spline_inputs_on_stack(),
            column_memo_y_independent: crate::opt::column_memo_y_independent(),
            floor_java: crate::opt::floor_java(),
            shifted_noise_in_batches: crate::opt::noise_batch_points(),
        });
        *P
    }
}

impl Default for Opts {
    fn default() -> Self {
        Opts::current()
    }
}

/// Spare buffers kept by a context at most, the old way (count only).
const SPARE_COUNT_CAP: usize = 64;

/// Bytes of spare buffers kept by a context at most (`best_fit_buffers`). A context kept by a
/// thread from one chunk to the next must hold a bounded reserve: a chunk volume is 393 KB, and
/// the reserve settles at 2.4 to 3.5 MB on the overworld (micro-bench, 256 chunks).
const SPARE_BYTES_CAP: usize = 8 << 20;

/// Spare buffers kept by a context at most with `best_fit_buffers` (bounds the search of a fit).
const SPARE_COUNT_CAP_BEST_FIT: usize = 128;

/// Capacity of a new buffer of `n` elements (`best_fit_buffers`): the next size of the series
/// 2^k, 3·2^(k-1) (… 64 Ki, 96 Ki, 128 Ki …; a chunk volume is exactly 96 Ki). The row
/// sub-volumes of the choices have heights that change from chunk to chunk: rounded to a few
/// sizes, they find a spare that fits instead of adding a new buffer each time.
fn size_class(n: usize) -> usize {
    let power = n.max(4).next_power_of_two();
    let three_quarters = power / 4 * 3;
    if n <= three_quarters { three_quarters } else { power }
}

/// Evaluation context: cache cells (like a context with caches in the game) and buffers.
#[derive(Debug, Default)]
pub struct Ctx {
    cells: Option<Vec<CacheCell>>,
    pool: Vec<Vec<f32>>,
    /// Bytes held by `pool` (sum of the capacities).
    pool_bytes: usize,
    opts: Opts,
    /// Test hook: every taken buffer is filled with NaN (see [`Ctx::poison_taken_buffers`]).
    poison: bool,
}

impl Ctx {
    /// Context without cache (pure evaluation).
    pub fn uncached() -> Self {
        Self::uncached_with(Opts::current())
    }

    /// Context with caches (filling of a chunk, climate of a chunk).
    pub fn cached() -> Self {
        Self::cached_with(Opts::current())
    }

    /// Context without cache, paths forced (equivalence tests, benches).
    pub fn uncached_with(opts: Opts) -> Self {
        Ctx { cells: None, opts, ..Ctx::default() }
    }

    /// Context with caches, paths forced (equivalence tests, benches).
    pub fn cached_with(opts: Opts) -> Self {
        Ctx { cells: Some(Vec::new()), opts, ..Ctx::default() }
    }

    /// The game's floor of a float, through the switch kept in the context (read once per context:
    /// reading it at each floor cost 27% of the floor). `RUSTCRAFT_OPT_FLOOR_JAVA=0`: C library.
    #[inline]
    pub fn floor_f32_to_i32(&self, v: f32) -> i32 {
        crate::math::floor_f32_to_i32(v, self.opts.floor_java)
    }

    /// The game's rounding of a float (halves up), same switch as [`Ctx::floor_f32_to_i32`].
    #[inline]
    pub fn round_f32_to_i32(&self, v: f32) -> i32 {
        crate::math::round_f32_to_i32(v, self.opts.floor_java)
    }

    /// Hands a buffer (received from [`Generator::volume`](crate::Generator::volume)) back to the context, which
    /// will take it again for a later volume.
    pub fn recycle(&mut self, v: Vec<f32>) {
        self.give(v);
    }

    /// Makes a context reused for another chunk answer exactly like a fresh one: every cache
    /// cell forgets its volume and its last point, and its buffer goes back to the spare buffers.
    /// Without it, a point read inside the previous chunk's cached volume would return the volume
    /// value where a fresh context computes the point value, and the two differ below an
    /// interpolation or a noise whose scale is not a power of 2. Once reset, the context can
    /// also serve another program (another dimension or seed).
    pub fn reset_for_reuse(&mut self) {
        let Some(mut cells) = self.cells.take() else { return };
        for c in &mut cells {
            c.volume = None;
            c.key = 0;
            c.value = f32::NAN;
            if let Some(b) = c.buf.take() {
                self.give(b);
            }
        }
        self.cells = Some(cells);
    }

    /// Test hook: every buffer taken from now on is filled with NaN, so that a caller reading an
    /// element it did not write changes the result (proof that no caller relies on the zeroing).
    #[doc(hidden)]
    pub fn poison_taken_buffers(&mut self) {
        self.poison = true;
    }

    /// Bytes held by the spare buffers (bound of the reserve of a kept context).
    pub fn spare_bytes(&self) -> usize {
        self.pool_bytes
    }

    /// Bytes held by every buffer of the context: the spares plus the volumes kept by the cache
    /// cells (until [`Ctx::reset_for_reuse`] hands them back or drops them beyond the cap).
    pub fn held_bytes(&self) -> usize {
        let cells = self.cells.iter().flatten();
        let in_cells: usize = cells.filter_map(|c| c.buf.as_ref()).map(|b| b.capacity() * size_of::<f32>()).sum();
        self.pool_bytes + in_cells
    }

    /// Output buffer of a volume: taken from the context (`vol_pool`) or new.
    pub(crate) fn output(&mut self, n: usize) -> Vec<f32> {
        if self.opts.vol_pool { self.take(n) } else { vec![0.0; n] }
    }

    /// Buffer of `n` elements, from the spare buffers when one is free (hand it back with
    /// [`Ctx::recycle`]). Its values are unspecified with `take_without_zeroing`: the caller
    /// writes every element it reads.
    pub fn take(&mut self, n: usize) -> Vec<f32> {
        let mut v = self.pop_spare(n);
        if self.poison {
            v.clear();
            v.resize(n, f32::NAN);
        } else if self.opts.take_without_zeroing {
            // Only the part beyond the old length is written: the rest keeps old values,
            // which every caller overwrites (393 KB of zeros not written for a chunk volume).
            v.truncate(n);
            v.resize(n, 0.0);
        } else {
            v.clear();
            v.resize(n, 0.0);
        }
        v
    }

    fn pop_spare(&mut self, n: usize) -> Vec<f32> {
        if !self.opts.best_fit_buffers {
            let v = self.pool.pop().unwrap_or_default();
            self.pool_bytes -= v.capacity() * size_of::<f32>();
            return v;
        }
        let Some(at) = self.best_fit(n) else { return Vec::with_capacity(size_class(n)) };
        let v = self.pool.swap_remove(at);
        self.pool_bytes -= v.capacity() * size_of::<f32>();
        v
    }

    /// Smallest spare buffer that holds `n` elements without growing (`None`: a new one is
    /// allocated, the smaller spares stay for smaller requests). The last one given back (the old
    /// choice) is often a chunk volume: a tiny request would take it, and the next chunk-sized
    /// request would allocate again.
    fn best_fit(&self, n: usize) -> Option<usize> {
        let fits = self.pool.iter().enumerate().filter(|(_, b)| b.capacity() >= n);
        fits.min_by_key(|(_, b)| b.capacity()).map(|(i, _)| i)
    }

    fn give(&mut self, v: Vec<f32>) {
        let bytes = v.capacity() * size_of::<f32>();
        let keep = if self.opts.best_fit_buffers {
            self.pool.len() < SPARE_COUNT_CAP_BEST_FIT && self.pool_bytes + bytes <= SPARE_BYTES_CAP
        } else {
            self.pool.len() < SPARE_COUNT_CAP
        };
        if keep {
            self.pool_bytes += bytes;
            self.pool.push(v);
        }
    }

    /// Cell of the cache `id`; `hint` = number of caches of the program (sized up front if `vol_pool`).
    fn cell(&mut self, id: u32, hint: usize) -> Option<&mut CacheCell> {
        let pre = self.opts.vol_pool;
        let cells = self.cells.as_mut()?;
        let id = id as usize;
        if id >= cells.len() {
            let n = if pre { hint.max(id + 1) } else { (id + 16) & !15 };
            cells.resize_with(n, || CacheCell { value: f32::NAN, ..Default::default() });
        }
        Some(&mut cells[id])
    }
}

/// Smallest range of rows `[r0, r1[` of the volume containing an element of `buf` where `need` is
/// true (`None`: none).
pub fn rows_hull(v: &Volume, buf: &[f32], need: impl Fn(f32) -> bool) -> Option<(i32, i32)> {
    let sy = v.sy as usize;
    let (mut lo, mut hi) = (sy, 0usize);
    for col in buf.chunks_exact(sy) {
        if let Some(a) = col[..lo.min(sy)].iter().position(|x| need(*x)) {
            lo = a;
        }
        if hi < sy
            && let Some(b) = col[hi..].iter().rposition(|x| need(*x))
        {
            hi = hi + b + 1;
        }
        if lo == 0 && hi == sy {
            break;
        }
    }
    (lo < hi).then_some((lo as i32, hi as i32))
}

/// Rows to compute for a branch whose interpolated nodes have the mesh `cell` (0: none):
/// the start is brought back to a cell boundary (or to the bottom of the volume) so that interpolation there
/// restarts from the same corner as in the whole volume; a single row of a taller volume would take
/// another interpolation path, hence the whole volume in that case.
pub fn snap_rows(v: &Volume, cell: i32, (mut r0, r1): (i32, i32)) -> (i32, i32) {
    if cell < 0 {
        r0 = 0;
    }
    if cell > 1 {
        while r0 > 0 && floor_mod(v.by(r0), cell) != 0 {
            r0 -= 1;
        }
    }
    if cell != 0 && r1 - r0 == 1 && v.sy > 1 {
        return (0, v.sy);
    }
    (r0, r1)
}

/// Does the sub-volume `sub` (same columns and step, rows `[k, k + sub.sy[` of `full`) give, for
/// a branch of mesh `cell`, the same values as `full` on these rows? `Some(k)` if so.
fn rows_of(full: &Volume, sub: &Volume, cell: i32) -> Option<usize> {
    if (full.sx, full.sz, full.x0, full.z0, full.stx, full.sty, full.stz)
        != (sub.sx, sub.sz, sub.x0, sub.z0, sub.stx, sub.sty, sub.stz)
    {
        return None;
    }
    let dy = sub.y0.checked_sub(full.y0)?;
    if dy < 0 || dy % full.sty != 0 {
        return None;
    }
    let k = dy / full.sty;
    if k + sub.sy > full.sy {
        return None;
    }
    if cell != 0 && sub.sy == 1 && full.sy > 1 {
        return None;
    }
    if k > 0 && (cell < 0 || cell > 1 && floor_mod(sub.y0, cell) != 0) {
        return None;
    }
    Some(k as usize)
}

/// Copies into `out` (volume `v`) the rows `[k, k + v.sy[` of each column of `b` (volume `w`).
fn copy_rows(b: &[f32], w: &Volume, k: usize, out: &mut [f32], v: &Volume) {
    let (sy, len) = (w.sy as usize, v.sy as usize);
    for (col, dst) in out.chunks_exact_mut(len).enumerate() {
        let at = col * sy + k;
        dst.copy_from_slice(&b[at..at + len]);
    }
}

/// Where the points of a volume sit inside a larger volume that holds them all: per axis (x, y,
/// z), index in the larger volume of the first point and index stride between two points.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Sublattice {
    first: [i32; 3],
    stride: [i32; 3],
}

impl Sublattice {
    /// `Some` if every point of `v` is a point of `w`.
    fn of(w: &Volume, v: &Volume) -> Option<Sublattice> {
        let x = lattice_axis((w.x0, w.stx, w.sx), (v.x0, v.stx, v.sx))?;
        let y = lattice_axis((w.y0, w.sty, w.sy), (v.y0, v.sty, v.sy))?;
        let z = lattice_axis((w.z0, w.stz, w.sz), (v.z0, v.stz, v.sz))?;
        Some(Sublattice { first: [x.0, y.0, z.0], stride: [x.1, y.1, z.1] })
    }

    /// Copies the points of `v` from `b` (values of `w`) into `out`, in the order of `v`.
    fn copy(&self, b: &[f32], w: &Volume, out: &mut [f32], v: &Volume) {
        let ([fx, fy, fz], [rx, ry, rz]) = (self.first, self.stride);
        let len = v.sy as usize;
        for (col, dst) in out.chunks_exact_mut(len).enumerate() {
            let (ix, iz) = (col as i32 % v.sx, col as i32 / v.sx);
            let at = w.index(fx + ix * rx, fy, fz + iz * rz);
            if ry == 1 {
                dst.copy_from_slice(&b[at..at + len]);
            } else {
                dst.iter_mut().zip(b[at..].iter().step_by(ry as usize)).for_each(|(d, s)| *d = *s);
            }
        }
    }
}

/// One axis of [`Sublattice::of`]: `(origin, step, size)` of the larger volume, then of the
/// smaller one; `(first index, stride)` if the points of the smaller one are points of the larger.
fn lattice_axis((w0, wst, wn): (i32, i32, i32), (v0, vst, vn): (i32, i32, i32)) -> Option<(i32, i32)> {
    let offset = i64::from(v0) - i64::from(w0);
    let wst = i64::from(wst);
    if offset < 0 || offset % wst != 0 {
        return None;
    }
    let first = offset / wst;
    // A single point has no stride to match.
    let stride = if vn == 1 { 1 } else if i64::from(vst) % wst == 0 { i64::from(vst) / wst } else { return None };
    let last = first + i64::from(vn - 1) * stride;
    (last < i64::from(wn)).then_some((first as i32, stride as i32))
}

/// Common mesh (lcm) of two branches: 0 = no interpolation, -1 = too large (the
/// sub-volume then keeps the bottom of the volume).
fn cell_join(a: i32, b: i32) -> i32 {
    if a < 0 || b < 0 {
        return -1;
    }
    if a == 0 || b == 0 {
        return a.max(b);
    }
    let (mut x, mut y) = (i64::from(a), i64::from(b));
    while y != 0 {
        (x, y) = (y, x % y);
    }
    let l = i64::from(a) / x * i64::from(b);
    if l > 1 << 20 { -1 } else { l as i32 }
}

fn pack_pos(x: i32, y: i32, z: i32) -> i64 {
    ((i64::from(x) & 0x3FF_FFFF) << 38) | (i64::from(y) & 0xFFF) | ((i64::from(z) & 0x3FF_FFFF) << 12)
}

/// Table of samplers and the objects they reference (noises, simplex).
const EXACT: u8 = 1;
const PINNED: u8 = 2;
/// The point value may depend on y (see [`reads_y_itself`]).
const READS_Y: u8 = 4;

/// Does the node read y itself, children apart? Conservative: an interpolation is counted as
/// reading y (away from a cell corner its point path goes through a small volume, whose value may
/// differ from the point value of its input).
fn reads_y_itself(s: &S) -> bool {
    match s {
        S::Noise { y, .. } | S::ShiftedXz { y, .. } | S::ShiftedXyz { y, .. } => *y != 0.0,
        S::GradClamp { axis, .. } | S::GradRepeat { axis, .. } | S::GradMirror { axis, .. } => *axis == Axis::Y,
        S::Distance { .. } | S::Interpolated { .. } => true,
        _ => false,
    }
}

/// Does `child` pass its dependence on y to `parent`? A slice in y reads its input at a fixed y,
/// and a top surface search probes its density at its own heights.
fn passes_y(parent: &S, child: Sid) -> bool {
    match parent {
        S::SliceY(..) => false,
        S::FindTop { upper, .. } => child == *upper,
        _ => true,
    }
}

#[derive(Debug, Default)]
pub struct Program {
    pub nodes: Vec<S>,
    pub noises: Vec<NoiseStack>,
    pub simplex: Vec<Simplex>,
    /// Per node: common y mesh of the interpolated nodes of its subtree (see `cell_join`).
    cells: Vec<i32>,
    /// Per node: `EXACT` (the point path returns the volume value bit for bit everywhere below),
    /// `PINNED` (a cache below has a non-`EXACT` entry).
    flags: Vec<u8>,
    /// Number of caches (size of the cells of a context).
    n_caches: usize,
}

impl S {
    /// Direct children (samplers read by this node).
    pub fn children(&self, mut f: impl FnMut(Sid)) {
        match self {
            S::Const(_)
            | S::Noise { .. }
            | S::ShiftB { .. }
            | S::EndIslands { .. }
            | S::Distance { .. }
            | S::GradClamp { .. }
            | S::GradRepeat { .. }
            | S::GradMirror { .. } => {}
            S::ShiftedXz { sx, sz, .. } => {
                f(*sx);
                f(*sz);
            }
            S::ShiftedXyz { sx, sy, sz, .. } => {
                f(*sx);
                f(*sy);
                f(*sz);
            }
            S::Add(a, b)
            | S::Sub(a, b)
            | S::Mul(a, b)
            | S::Div(a, b)
            | S::Min(a, b, _)
            | S::Max(a, b, _)
            | S::Round(_, a, b)
            | S::Pow(a, b)
            | S::LerpConstFirst(a, _, b)
            | S::LerpConstSecond(a, b, _) => {
                f(*a);
                f(*b);
            }
            S::ConstAdd(a, _)
            | S::ConstSub(_, a)
            | S::ConstMul(a, _)
            | S::ConstDiv(_, a)
            | S::ConstMin(a, _)
            | S::ConstMax(a, _)
            | S::Unary(_, a)
            | S::RoundInt(_, a)
            | S::PowConstBase(_, a)
            | S::PowConstExp(a, _)
            | S::Clamp(a, ..)
            | S::RangeConst { input: a, .. }
            | S::Cache { input: a, .. }
            | S::Interpolated { input: a, .. }
            | S::SliceX(a, _)
            | S::SliceY(a, _)
            | S::SliceZ(a, _)
            | S::SliceXz(a, ..) => f(*a),
            S::Spline { coords, .. } => coords.iter().for_each(|c| f(*c)),
            S::Lerp(a, b, c) => {
                f(*a);
                f(*b);
                f(*c);
            }
            S::Range { input, inside, outside, .. } => {
                f(*input);
                f(*inside);
                f(*outside);
            }
            S::IntervalSingle { input, below, above, .. } => {
                f(*input);
                f(*below);
                f(*above);
            }
            S::IntervalSelect { input, functions, .. } => {
                f(*input);
                functions.iter().for_each(|c| f(*c));
            }
            S::FindTop { density, upper, .. } => {
                f(*density);
                f(*upper);
            }
        }
    }
}

/// Points of a shifted noise volume handed to the noise at once (on the stack: 3 × 512 bytes,
/// zeroed on every call, hence small). The 26.3 volumes are one row of 16 to 25 points (4×4, 5×5
/// climate grids) and fit in one batch; a larger one (data pack) empties its batch more often, each
/// point keeping its layers in the same order, so the result does not depend on this size.
const SHIFTED_NOISE_BATCH: usize = 64;

/// Shifted noise over a volume, its points handed to the noise in batches: `out` holds the shift
/// in x on entry and the noise on exit, `by` the shift in y (absent: none), `bz` the shift in z.
/// Each point gets the coordinates of the point-by-point path.
fn shifted_noise_in_batches(
    noise: &NoiseStack,
    v: &Volume,
    (xz_scale, y_scale): (f64, f64),
    out: &mut [f32],
    by: Option<&[f32]>,
    bz: &[f32],
) {
    let mut x_coords = [0.0f64; SHIFTED_NOISE_BATCH];
    let mut y_coords = [0.0f64; SHIFTED_NOISE_BATCH];
    let mut z_coords = [0.0f64; SHIFTED_NOISE_BATCH];
    let (mut i, mut start, mut filled) = (0, 0, 0);
    for iz in 0..v.sz {
        let base_z = f64::from(v.bz(iz)) * xz_scale;
        for ix in 0..v.sx {
            let base_x = f64::from(v.bx(ix)) * xz_scale;
            for iy in 0..v.sy {
                x_coords[filled] = base_x + f64::from(out[i]);
                y_coords[filled] = match by {
                    None => f64::from(v.by(iy)) * y_scale,
                    Some(by) => f64::from(v.by(iy)) * y_scale + f64::from(by[i]),
                };
                z_coords[filled] = base_z + f64::from(bz[i]);
                (i, filled) = (i + 1, filled + 1);
                // The shifts of these points are read: their places in `out` take the noise.
                if filled == SHIFTED_NOISE_BATCH || i == out.len() {
                    let batch = ..filled;
                    noise.get_many(&x_coords[batch], &y_coords[batch], &z_coords[batch], &mut out[start..i]);
                    (start, filled) = (i, 0);
                }
            }
        }
    }
}

/// Counts a volume of a shifted noise (inventory of the batch sizes, see `EvalCounts`).
fn count_shifted_noise_volume(v: &Volume) {
    count(|c| {
        c.shifted_noise_volumes += 1;
        c.shifted_noise_values += v.size() as u64;
        c.shifted_noise_volumes_taller_than_one_row += u64::from(v.sy > 1);
    });
}

fn island_height(n: &Simplex, sx: i32, sz: i32) -> f32 {
    let (cx, cz) = (sx / 2, sz / 2);
    let (rx, rz) = (sx % 2, sz % 2);
    let mut h = -100.0f32;
    for ox in -12..=12 {
        for oz in -12..=12 {
            let tx = i64::from(cx) + i64::from(ox);
            let tz = i64::from(cz) + i64::from(oz);
            if tx * tx + tz * tz > 4096 && n.get2(tx as f64, tz as f64) < -0.9 {
                let size = ((tx as f32).abs() * 3439.0 + (tz as f32).abs() * 147.0) % 13.0 + 9.0;
                let xd = (rx - ox * 2) as f32;
                let zd = (rz - oz * 2) as f32;
                let nd = 100.0 - ((xd * xd + zd * zd) as f64).sqrt() as f32 * size;
                h = jmax(h, clamp(nd, -100.0, 80.0));
            }
        }
    }
    h
}

fn metric(m: Metric, dx: f32, dy: f32, dz: f32) -> f32 {
    match m {
        Metric::Euclidean => f64::from(dx * dx + dy * dy + dz * dz).sqrt() as f32,
        Metric::EuclideanSquared => dx * dx + dy * dy + dz * dz,
        Metric::Manhattan => dx.abs() + dy.abs() + dz.abs(),
        Metric::Chebyshev => jmax(jmax(dx.abs(), dy.abs()), dz.abs()),
    }
}

#[inline]
fn unary(u: Unary, v: f32) -> f32 {
    match u {
        Unary::Abs => v.abs(),
        Unary::Square => v * v,
        Unary::Cube => v * v * v,
        Unary::Sqrt => f64::from(v).sqrt() as f32,
        Unary::HalfNegative => {
            if v > 0.0 {
                v
            } else {
                v * 0.5
            }
        }
        Unary::QuarterNegative => {
            if v > 0.0 {
                v
            } else {
                v * 0.25
            }
        }
        Unary::Reciprocal => 1.0 / v,
        Unary::Negate => -v,
        Unary::Squeeze => squeeze(v),
        Unary::Log => f64::from(v).ln() as f32,
        Unary::Sign => signum(v),
    }
}

#[inline]
fn round_apply(k: RoundKind, v: f32, m: f32, floor_java: bool) -> f32 {
    if m == 0.0 { v } else { round_to_integer(v / m, k, floor_java) * m }
}

#[inline]
fn lerp_choice(a: f32, f: f32, s: f32) -> f32 {
    if a == 0.0 {
        f
    } else if a == 1.0 {
        s
    } else {
        lerp(a, f, s)
    }
}

fn grad_clamp(c: i32, from: i32, min: i32, max: i32, from_v: f32, factor: f32) -> f32 {
    let rel = c.clamp(min, max) - from;
    from_v + rel as f32 * factor
}

fn grad_repeat(c: i32, from: i32, range: i32, from_v: f32, factor: f32) -> f32 {
    from_v + floor_mod(c.wrapping_sub(from), range) as f32 * factor
}

fn grad_mirror(c: i32, from: i32, range: i32, from_v: f32, factor: f32) -> f32 {
    let rel = c.wrapping_sub(from);
    let tile = floor_div(rel, range);
    let local = rel - tile.wrapping_mul(range);
    if tile & 1 == 0 { from_v + local as f32 * factor } else { from_v + (range - local) as f32 * factor }
}

/// Value of a spline; `input(i)` = coordinate `i` (direct call, or dynamic with `F = dyn FnMut`).
fn spline_sample<F: FnMut(usize) -> f32 + ?Sized>(s: &SSpline, input: &mut F) -> f32 {
    match s {
        SSpline::Const(v) => *v,
        SSpline::Multi { coord, locs, values, ders } => {
            let x = input(*coord);
            // First index such that x < loc, minus 1 (binary search).
            let (mut lo, mut len) = (0usize, locs.len());
            while len > 0 {
                let half = len / 2;
                let mid = lo + half;
                if x < locs[mid] {
                    len = half;
                } else {
                    lo = mid + 1;
                    len -= half + 1;
                }
            }
            let start = lo as isize - 1;
            let last = locs.len() - 1;
            let ext = |v: f32, i: usize| if ders[i] == 0.0 { v } else { v + ders[i] * (x - locs[i]) };
            if start < 0 {
                let v = spline_sample(&values[0], input);
                return ext(v, 0);
            }
            let s0 = start as usize;
            if s0 == last {
                let v = spline_sample(&values[last], input);
                return ext(v, last);
            }
            let (x1, x2) = (locs[s0], locs[s0 + 1]);
            let t = (x - x1) / (x2 - x1);
            let (d1, d2) = (ders[s0], ders[s0 + 1]);
            let y1 = spline_sample(&values[s0], input);
            let y2 = spline_sample(&values[s0 + 1], input);
            let a = d1 * (x2 - x1) - (y2 - y1);
            let b = -d2 * (x2 - x1) + (y2 - y1);
            lerp(t, y1, y2) + t * (1.0 - t) * lerp(t, a, b)
        }
    }
}

impl Program {
    #[inline]
    fn node(&self, s: Sid) -> &S {
        &self.nodes[s as usize]
    }

    /// Adds a node (its children are already in the table) and notes its interpolation mesh.
    pub fn push(&mut self, s: S) -> Sid {
        let mut cell = match s {
            S::Interpolated { cy, .. } => cy.max(1),
            _ => 0,
        };
        // Point and volume paths that differ: interpolation (lerp3 against accumulation),
        // noise with a scale that is not a power of 2 (spec generation-noise § 2.4).
        let pow2 = |k: f64| k == 0.0 || (k.is_normal() && k.to_bits() & ((1 << 52) - 1) == 0);
        let mut flags = match s {
            S::Interpolated { .. } => 0,
            S::Noise { xz, y, .. } if !(pow2(xz) && pow2(y)) => 0,
            // At a point, a zero left side skips the right one and the comparisons go through the
            // Java min and max; over a volume, the sides are multiplied and compared: −0, +0 and NaN
            // can come out differently (the game does the same).
            S::Mul(..) | S::Div(..) | S::Min(..) | S::Max(..) => 0,
            S::ConstMin(_, c) | S::ConstMax(_, c) if c == 0.0 => 0,
            _ => EXACT,
        };
        if let S::Cache { input, .. } = s
            && self.flags_of(input) & EXACT == 0
        {
            flags |= PINNED;
        }
        if reads_y_itself(&s) {
            flags |= READS_Y;
        }
        if matches!(s, S::Cache { .. }) {
            self.n_caches += 1;
        }
        s.children(|c| {
            cell = cell_join(cell, self.cell_of(c));
            let f = self.flags_of(c);
            let passed_y = if passes_y(&s, c) { f & READS_Y } else { 0 };
            flags = (flags & f & EXACT) | ((flags | f) & PINNED) | (flags & READS_Y) | passed_y;
        });
        self.cells.push(cell);
        self.flags.push(flags);
        self.nodes.push(s);
        (self.nodes.len() - 1) as Sid
    }

    /// Flags of `s` (worst case if the table was not filled by [`Program::push`]).
    fn flags_of(&self, s: Sid) -> u8 {
        if self.flags.len() == self.nodes.len() {
            self.flags.get(s as usize).copied().unwrap_or(PINNED)
        } else {
            PINNED
        }
    }

    /// Can the branches of a choice at `s` be computed on only part of the rows?
    /// No if a cache below has a non-exact entry: filled for a few rows only (or
    /// in another order), it would leave the following point reads outside those rows to the
    /// point path, which does not return the volume value bit for bit.
    fn restrictable(&self, s: Sid) -> bool {
        self.flags_of(s) & PINNED == 0
    }

    /// Can the point value of `s` depend on y? Yes below a cache with a non-exact entry too: its
    /// value then depends on which points its buffer holds.
    fn reads_y(&self, s: Sid) -> bool {
        self.flags_of(s) & (READS_Y | PINNED) != 0
    }

    /// y mesh of the interpolated nodes under `s` (0: none; -1: unknown or too large).
    pub fn cell_of(&self, s: Sid) -> i32 {
        if self.cells.len() == self.nodes.len() { self.cells.get(s as usize).copied().unwrap_or(-1) } else { -1 }
    }

    /// Values of `s` on the rows `rows` of the volume (the rest of `out` is not written).
    pub fn volume_rows(&self, s: Sid, ctx: &mut Ctx, out: &mut [f32], v: &Volume, rows: Option<(i32, i32)>) {
        let Some(rows) = rows else { return };
        let (r0, r1) = snap_rows(v, self.cell_of(s), rows);
        if r0 == 0 && r1 == v.sy {
            self.volume(s, ctx, out, v);
            return;
        }
        let sv = Volume::stepped(v.sx, r1 - r0, v.sz, v.x0, v.by(r0), v.z0, v.stx, v.sty, v.stz);
        let mut b = ctx.take(sv.size());
        self.volume(s, ctx, &mut b, &sv);
        let (n, sy, r0) = ((r1 - r0) as usize, v.sy as usize, r0 as usize);
        for (c, src) in b.chunks_exact(n).enumerate() {
            let at = c * sy + r0;
            out[at..at + n].copy_from_slice(src);
        }
        ctx.give(b);
    }

    /// Counts (counting on) what the branch `branch` of a choice restricted in y costs over `v`:
    /// the elements that read it (`reads` true on their selector value), the values computed over
    /// the rows of the hull of every column, and the values a hull per column would compute.
    fn count_branch(&self, branch: Sid, v: &Volume, selector: &[f32], reads: impl Fn(f32) -> bool) {
        count(|c| {
            let cell = self.cell_of(branch);
            let rows = |hull: Option<(i32, i32)>| {
                hull.map_or(0, |h| {
                    let (r0, r1) = snap_rows(v, cell, h);
                    (r1 - r0) as u64
                })
            };
            let whole = (v.sx * v.sz) as u64 * rows(rows_hull(v, selector, &reads));
            let columns = selector.chunks_exact(v.sy as usize);
            let per_column = columns.map(|col| rows(rows_hull(v, col, &reads))).sum::<u64>();
            // Only a branch without interpolation could be cut per column: an interpolated node
            // would compute the corners shared by neighbouring columns again for each one.
            let noises = if cell == 0 { self.noise_nodes_under(branch) } else { 0 };
            c.choice_branches += 1;
            c.branch_values_read += selector.iter().filter(|x| reads(**x)).count() as u64;
            c.branch_values_whole_hull += whole;
            c.branch_values_column_hulls += per_column;
            c.plain_branch_noise_values_whole_hull += whole * noises;
            c.plain_branch_noise_values_column_hulls += per_column * noises;
        });
    }

    /// Noise nodes evaluated by `s` (each cache counted once).
    fn noise_nodes_under(&self, s: Sid) -> u64 {
        let (mut stack, mut seen, mut n) = (vec![s], Vec::new(), 0);
        while let Some(x) = stack.pop() {
            let node = self.node(x);
            if let S::Cache { id, .. } = node {
                if seen.contains(id) {
                    continue;
                }
                seen.push(*id);
            }
            n += u64::from(matches!(node, S::Noise { .. } | S::ShiftedXz { .. } | S::ShiftedXyz { .. } | S::ShiftB { .. }));
            node.children(|c| stack.push(c));
        }
        n
    }

    /// Read-only buffer of the values of `s`: that of a cache already filled for this volume is
    /// lent without copy (`vol_pool`); hand it back through [`Program::unborrow`] before any other evaluation.
    fn borrow(&self, s: Sid, ctx: &mut Ctx, v: &Volume) -> (Vec<f32>, Option<u32>) {
        if ctx.opts.vol_pool
            && let S::Cache { id, .. } = self.node(s)
            && let Some(c) = ctx.cell(*id, self.n_caches)
            && c.volume == Some(*v)
            && let Some(b) = c.buf.take()
        {
            return (b, Some(*id));
        }
        let mut b = ctx.take(v.size());
        self.volume(s, ctx, &mut b, v);
        (b, None)
    }

    fn unborrow(&self, ctx: &mut Ctx, (b, id): (Vec<f32>, Option<u32>)) {
        match id.and_then(|id| ctx.cell(id, self.n_caches)) {
            Some(c) => c.buf = Some(b),
            None => ctx.give(b),
        }
    }

    /// Value at a point.
    pub fn value(&self, s: Sid, ctx: &mut Ctx, x: i32, y: i32, z: i32) -> f32 {
        match self.node(s) {
            S::Const(v) => *v,
            S::Noise { noise, xz, y: ys, .. } => {
                count(|c| c.point_noises += 1);
                self.noises[*noise as usize].get(f64::from(x) * xz, f64::from(y) * ys, f64::from(z) * xz)
            }
            S::ShiftedXz { sx, sz, noise, xz, y: ys } => {
                count(|c| c.point_noises += 1);
                let nx = f64::from(x) * xz + f64::from(self.value(*sx, ctx, x, y, z));
                let ny = f64::from(y) * ys;
                let nz = f64::from(z) * xz + f64::from(self.value(*sz, ctx, x, y, z));
                self.noises[*noise as usize].get(nx, ny, nz)
            }
            S::ShiftedXyz { sx, sy, sz, noise, xz, y: ys } => {
                count(|c| c.point_noises += 1);
                let nx = f64::from(x) * xz + f64::from(self.value(*sx, ctx, x, y, z));
                let ny = f64::from(y) * ys + f64::from(self.value(*sy, ctx, x, y, z));
                let nz = f64::from(z) * xz + f64::from(self.value(*sz, ctx, x, y, z));
                self.noises[*noise as usize].get(nx, ny, nz)
            }
            S::ShiftB { noise } => {
                count(|c| c.point_noises += 1);
                self.noises[*noise as usize].get(f64::from(z) * 0.25, f64::from(x) * 0.25, 0.0) * 4.0
            }
            S::EndIslands { simplex } => {
                count(|c| c.point_noises += 1);
                (island_height(&self.simplex[*simplex as usize], x / 8, z / 8) - 8.0) / 128.0
            }
            S::Distance { p, metric: m } => metric(*m, (p[0] - x) as f32, (p[1] - y) as f32, (p[2] - z) as f32),
            S::GradClamp { axis, from, min, max, from_v, factor } => {
                grad_clamp(axis.choose(x, y, z), *from, *min, *max, *from_v, *factor)
            }
            S::GradRepeat { axis, from, range, from_v, factor } => {
                grad_repeat(axis.choose(x, y, z), *from, *range, *from_v, *factor)
            }
            S::GradMirror { axis, from, range, from_v, factor } => {
                grad_mirror(axis.choose(x, y, z), *from, *range, *from_v, *factor)
            }
            S::Add(a, b) => self.value(*a, ctx, x, y, z) + self.value(*b, ctx, x, y, z),
            S::ConstAdd(a, c) => self.value(*a, ctx, x, y, z) + c,
            S::ConstSub(c, a) => c - self.value(*a, ctx, x, y, z),
            S::Sub(a, b) => self.value(*a, ctx, x, y, z) - self.value(*b, ctx, x, y, z),
            S::Mul(a, b) => {
                let l = self.value(*a, ctx, x, y, z);
                if l == 0.0 { 0.0 } else { l * self.value(*b, ctx, x, y, z) }
            }
            S::ConstMul(a, c) => self.value(*a, ctx, x, y, z) * c,
            S::Div(a, b) => {
                let l = self.value(*a, ctx, x, y, z);
                if l == 0.0 { 0.0 } else { l / self.value(*b, ctx, x, y, z) }
            }
            S::ConstDiv(c, a) => c / self.value(*a, ctx, x, y, z),
            S::Min(a, b, rmin) => {
                let l = self.value(*a, ctx, x, y, z);
                if l <= *rmin { l } else { jmin(l, self.value(*b, ctx, x, y, z)) }
            }
            S::ConstMin(a, c) => jmin(self.value(*a, ctx, x, y, z), *c),
            S::Max(a, b, rmax) => {
                let l = self.value(*a, ctx, x, y, z);
                if l >= *rmax { l } else { jmax(l, self.value(*b, ctx, x, y, z)) }
            }
            S::ConstMax(a, c) => jmax(self.value(*a, ctx, x, y, z), *c),
            S::Unary(u, a) => unary(*u, self.value(*a, ctx, x, y, z)),
            S::RoundInt(k, a) => round_to_integer(self.value(*a, ctx, x, y, z), *k, ctx.opts.floor_java),
            S::Round(k, a, m) => {
                let v = self.value(*a, ctx, x, y, z);
                let m = self.value(*m, ctx, x, y, z);
                round_apply(*k, v, m, ctx.opts.floor_java)
            }
            S::PowConstBase(b, e) => b.powf(f64::from(self.value(*e, ctx, x, y, z))) as f32,
            S::PowConstExp(b, e) => f64::from(self.value(*b, ctx, x, y, z)).powf(*e) as f32,
            S::Pow(b, e) => {
                let b = self.value(*b, ctx, x, y, z);
                let e = self.value(*e, ctx, x, y, z);
                f64::from(b).powf(f64::from(e)) as f32
            }
            S::Spline { spline, coords } if ctx.opts.spline_stack && coords.len() <= 8 => {
                count(|c| c.point_splines += 1);
                let mut cache = [f32::NAN; 8];
                let mut input = |i: usize| {
                    if cache[i].is_nan() {
                        cache[i] = self.value(coords[i], ctx, x, y, z);
                    }
                    cache[i]
                };
                spline_sample(spline, &mut input)
            }
            S::Spline { spline, coords } => {
                count(|c| c.point_splines += 1);
                let mut cache = vec![f32::NAN; coords.len()];
                let mut input = |i: usize| {
                    if cache[i].is_nan() {
                        cache[i] = self.value(coords[i], ctx, x, y, z);
                    }
                    cache[i]
                };
                spline_sample::<dyn FnMut(usize) -> f32>(spline, &mut input)
            }
            S::LerpConstFirst(a, f, s) => {
                let al = self.value(*a, ctx, x, y, z);
                if al == 0.0 {
                    *f
                } else if al == 1.0 {
                    self.value(*s, ctx, x, y, z)
                } else {
                    lerp(al, *f, self.value(*s, ctx, x, y, z))
                }
            }
            S::LerpConstSecond(a, f, s) => {
                let al = self.value(*a, ctx, x, y, z);
                if al == 0.0 {
                    self.value(*f, ctx, x, y, z)
                } else if al == 1.0 {
                    *s
                } else {
                    lerp(al, self.value(*f, ctx, x, y, z), *s)
                }
            }
            S::Lerp(a, f, s) => {
                let al = self.value(*a, ctx, x, y, z);
                if al == 0.0 {
                    self.value(*f, ctx, x, y, z)
                } else if al == 1.0 {
                    self.value(*s, ctx, x, y, z)
                } else {
                    let fv = self.value(*f, ctx, x, y, z);
                    lerp(al, fv, self.value(*s, ctx, x, y, z))
                }
            }
            S::Clamp(a, mn, mx) => clamp(self.value(*a, ctx, x, y, z), *mn, *mx),
            S::RangeConst { input, min, max, inside, outside } => {
                let v = self.value(*input, ctx, x, y, z);
                if v >= *min && v < *max { *inside } else { *outside }
            }
            S::Range { input, min, max, inside, outside } => {
                let v = self.value(*input, ctx, x, y, z);
                if v >= *min && v < *max {
                    self.value(*inside, ctx, x, y, z)
                } else {
                    self.value(*outside, ctx, x, y, z)
                }
            }
            S::IntervalSingle { input, threshold, below, above } => {
                let v = self.value(*input, ctx, x, y, z);
                if v < *threshold { self.value(*below, ctx, x, y, z) } else { self.value(*above, ctx, x, y, z) }
            }
            S::IntervalSelect { input, thresholds, functions } => {
                let v = self.value(*input, ctx, x, y, z);
                let i = thresholds.iter().position(|t| v < *t).unwrap_or(functions.len() - 1);
                self.value(functions[i], ctx, x, y, z)
            }
            S::Cache { id, input } => self.value_cached(*id, *input, ctx, x, y, z),
            S::Interpolated { input, cxz, cy, .. } => {
                let (kx, ky, kz) = (floor_mod(x, *cxz), floor_mod(y, *cy), floor_mod(z, *cxz));
                if kx == 0 && ky == 0 && kz == 0 {
                    return self.value(*input, ctx, x, y, z);
                }
                let v = Volume::stepped(2, 2, 2, x - kx, y - ky, z - kz, *cxz, *cy, *cxz);
                let mut b = ctx.take(8);
                self.volume(*input, ctx, &mut b, &v);
                let r = lerp3(
                    kx as f32 / *cxz as f32,
                    ky as f32 / *cy as f32,
                    kz as f32 / *cxz as f32,
                    b[v.index(0, 0, 0)],
                    b[v.index(1, 0, 0)],
                    b[v.index(0, 1, 0)],
                    b[v.index(1, 1, 0)],
                    b[v.index(0, 0, 1)],
                    b[v.index(1, 0, 1)],
                    b[v.index(0, 1, 1)],
                    b[v.index(1, 1, 1)],
                );
                ctx.give(b);
                r
            }
            S::SliceX(a, c) => self.value(*a, ctx, *c, y, z),
            S::SliceY(a, c) => self.value(*a, ctx, x, *c, z),
            S::SliceZ(a, c) => self.value(*a, ctx, x, y, *c),
            S::SliceXz(a, cx, cz) => self.value(*a, ctx, *cx, y, *cz),
            S::FindTop { density, upper, lower, cell } => {
                let ub = self.value(*upper, ctx, x, y, z);
                self.find_top(*density, *lower, *cell, ctx, x, z, ub)
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn find_top(&self, density: Sid, lower: i32, cell: i32, ctx: &mut Ctx, x: i32, z: i32, upper: f32) -> f32 {
        let top = ctx.floor_f32_to_i32(upper / cell as f32).wrapping_mul(cell);
        if top <= lower {
            return lower as f32;
        }
        let mut p = top;
        while p >= lower {
            count(|c| c.top_probes += 1);
            if self.value(density, ctx, x, p, z) > 0.0 {
                return p as f32;
            }
            p -= cell;
        }
        lower as f32
    }

    fn value_cached(&self, id: u32, input: Sid, ctx: &mut Ctx, x: i32, y: i32, z: i32) -> f32 {
        let key = pack_pos(x, y, z);
        let column_memo = ctx.opts.column_memo_y_independent;
        match ctx.cell(id, self.n_caches) {
            None => return self.value(input, ctx, x, y, z),
            Some(c) => {
                // The game's order: the last point, then the buffer.
                if c.key == key && !c.value.is_nan() {
                    count(|n| n.cache_point_last += 1);
                    return c.value;
                }
                if let (Some(b), Some(v)) = (&c.buf, &c.volume)
                    && let Some(i) = v.index_of_block(x, y, z)
                {
                    count(|n| n.cache_point_buffer += 1);
                    return b[i];
                }
                // Where the game would compute the point: an input that never reads y has one
                // point value per column, already known from another height.
                if column_memo
                    && !self.reads_y(input)
                    && let Some(v) = self.column_value(c, input, (x, z), key)
                {
                    count(|n| n.cache_point_same_column += 1);
                    // Kept as the game keeps the point it computes: a later read back at the old
                    // height must miss the key and find the buffer (−0 where the point gave +0).
                    c.key = key;
                    c.value = v;
                    return v;
                }
            }
        }
        count(|n| n.cache_point_computed += 1);
        let v = self.value(input, ctx, x, y, z);
        if let Some(c) = ctx.cell(id, self.n_caches) {
            c.key = key;
            c.value = v;
        }
        v
    }

    /// Point value at the column (x, z) of a cache whose input never reads y, known from another
    /// height: the last point computed (`key` is the point read), or for an exact input (its
    /// volume value is its point value) the buffer at y = 0.
    fn column_value(&self, c: &CacheCell, input: Sid, (x, z): (i32, i32), key: i64) -> Option<f32> {
        // `pack_pos` keeps y in its 12 low bits.
        if c.key >> 12 == key >> 12 && !c.value.is_nan() {
            return Some(c.value);
        }
        let (b, v) = (c.buf.as_ref()?, c.volume.as_ref()?);
        if self.flags_of(input) & EXACT == 0 {
            return None;
        }
        v.index_of_block(x, 0, z).map(|i| b[i])
    }

    fn volume_cached(&self, id: u32, input: Sid, ctx: &mut Ctx, out: &mut [f32], v: &Volume) {
        let opts = ctx.opts;
        let Some(c) = ctx.cell(id, self.n_caches) else {
            self.volume(input, ctx, out, v);
            return;
        };
        if let (Some(b), Some(w)) = (&c.buf, &c.volume) {
            if w == v {
                count(|n| n.cache_volume_same += 1);
                out.copy_from_slice(b);
                return;
            }
            // Rows of a volume already in cache (sub-volume of a choice restricted in y).
            if opts.range_yvol
                && let Some(k) = rows_of(w, v, self.cell_of(input))
            {
                count(|n| n.cache_volume_rows += 1);
                copy_rows(b, w, k, out, v);
                return;
            }
            // An exact input gives each point the same value whatever the volume around it, so the
            // points already held serve as they are.
            if opts.sublattice_cache_read
                && self.flags_of(input) & EXACT != 0
                && let Some(lattice) = Sublattice::of(w, v)
            {
                count(|n| n.cache_volume_sublattice += 1);
                lattice.copy(b, w, out, v);
                // Then the cache holds `v`, as if it had computed it: the point reads that follow
                // find the buffer the game's cache holds.
                let mut held = self.retarget_cache_to_volume(id, ctx, v);
                held.copy_from_slice(out);
                self.keep_in_cache(id, ctx, held);
                return;
            }
        }
        count(|n| {
            n.cache_volume_computed += 1;
            n.cache_volume_values += v.size() as u64;
        });
        let mut held = self.retarget_cache_to_volume(id, ctx, v);
        self.volume(input, ctx, &mut held, v);
        out.copy_from_slice(&held);
        self.keep_in_cache(id, ctx, held);
    }

    /// Makes the cache `id` hold the volume `v` (its old buffer goes back to the spares) and returns a
    /// buffer for its values, to hand to [`Program::keep_in_cache`].
    fn retarget_cache_to_volume(&self, id: u32, ctx: &mut Ctx, v: &Volume) -> Vec<f32> {
        let old = ctx.cell(id, self.n_caches).and_then(|c| {
            c.volume = Some(*v);
            c.buf.take()
        });
        if let Some(o) = old {
            ctx.give(o);
        }
        ctx.take(v.size())
    }

    fn keep_in_cache(&self, id: u32, ctx: &mut Ctx, values: Vec<f32>) {
        if let Some(c) = ctx.cell(id, self.n_caches) {
            c.buf = Some(values);
        }
    }

    /// Values on a grid (buffer `out` of `v.size()` elements).
    #[allow(clippy::manual_memcpy)]
    pub fn volume(&self, s: Sid, ctx: &mut Ctx, out: &mut [f32], v: &Volume) {
        debug_assert_eq!(out.len(), v.size());
        match self.node(s) {
            S::Const(c) => out.fill(*c),
            S::Noise { noise, xz, y } => {
                out.fill(0.0);
                self.noises[*noise as usize].add_to_volume(out, v, *xz, *y, 1.0);
            }
            S::ShiftedXz { sx, sz, noise, xz, y: ys } if ctx.opts.shifted_noise_in_batches => {
                count_shifted_noise_volume(v);
                self.volume(*sx, ctx, out, v);
                let mut bz = ctx.take(v.size());
                self.volume(*sz, ctx, &mut bz, v);
                shifted_noise_in_batches(&self.noises[*noise as usize], v, (*xz, *ys), out, None, &bz);
                ctx.give(bz);
            }
            S::ShiftedXyz { sx, sy, sz, noise, xz, y: ys } if ctx.opts.shifted_noise_in_batches => {
                count_shifted_noise_volume(v);
                self.volume(*sx, ctx, out, v);
                let mut by = ctx.take(v.size());
                self.volume(*sy, ctx, &mut by, v);
                let mut bz = ctx.take(v.size());
                self.volume(*sz, ctx, &mut bz, v);
                shifted_noise_in_batches(&self.noises[*noise as usize], v, (*xz, *ys), out, Some(&by), &bz);
                ctx.give(by);
                ctx.give(bz);
            }
            S::ShiftedXz { sx, sz, noise, xz, y: ys } => {
                count_shifted_noise_volume(v);
                self.volume(*sx, ctx, out, v);
                let mut bz = ctx.take(v.size());
                self.volume(*sz, ctx, &mut bz, v);
                let n = &self.noises[*noise as usize];
                let mut i = 0;
                for iz in 0..v.sz {
                    let base_z = f64::from(v.bz(iz)) * xz;
                    for ix in 0..v.sx {
                        let base_x = f64::from(v.bx(ix)) * xz;
                        for iy in 0..v.sy {
                            let nx = base_x + f64::from(out[i]);
                            let ny = f64::from(v.by(iy)) * ys;
                            let nz = base_z + f64::from(bz[i]);
                            out[i] = n.get(nx, ny, nz);
                            i += 1;
                        }
                    }
                }
                ctx.give(bz);
            }
            S::ShiftedXyz { sx, sy, sz, noise, xz, y: ys } => {
                count_shifted_noise_volume(v);
                self.volume(*sx, ctx, out, v);
                let mut by = ctx.take(v.size());
                self.volume(*sy, ctx, &mut by, v);
                let mut bz = ctx.take(v.size());
                self.volume(*sz, ctx, &mut bz, v);
                let n = &self.noises[*noise as usize];
                let mut i = 0;
                for iz in 0..v.sz {
                    let base_z = f64::from(v.bz(iz)) * xz;
                    for ix in 0..v.sx {
                        let base_x = f64::from(v.bx(ix)) * xz;
                        for iy in 0..v.sy {
                            let nx = base_x + f64::from(out[i]);
                            let ny = f64::from(v.by(iy)) * ys + f64::from(by[i]);
                            let nz = base_z + f64::from(bz[i]);
                            out[i] = n.get(nx, ny, nz);
                            i += 1;
                        }
                    }
                }
                ctx.give(by);
                ctx.give(bz);
            }
            S::ShiftB { noise } => {
                let tv = Volume::stepped(v.sz, v.sx, 1, v.z0, v.x0, 0, v.stz, v.stx, 1);
                let mut tb = ctx.take(tv.size());
                // The noise adds to the buffer: it must start from zero, whatever the buffer held.
                tb.fill(0.0);
                self.noises[*noise as usize].add_to_volume(&mut tb, &tv, 0.25, 0.25, 4.0);
                for iz in 0..v.sz {
                    for ix in 0..v.sx {
                        let val = tb[tv.index(iz, ix, 0)];
                        let at = v.index(ix, 0, iz);
                        out[at..at + v.sy as usize].fill(val);
                    }
                }
                ctx.give(tb);
            }
            S::EndIslands { simplex } => {
                let n = &self.simplex[*simplex as usize];
                for iz in 0..v.sz {
                    let bz = v.bz(iz);
                    for ix in 0..v.sx {
                        let bx = v.bx(ix);
                        let val = (island_height(n, bx / 8, bz / 8) - 8.0) / 128.0;
                        let at = v.index(ix, 0, iz);
                        out[at..at + v.sy as usize].fill(val);
                    }
                }
            }
            S::Distance { .. } => self.naive(s, ctx, out, v),
            S::GradClamp { axis, from, min, max, from_v, factor } => {
                self.gradient_volume(*axis, out, v, |c| grad_clamp(c, *from, *min, *max, *from_v, *factor))
            }
            S::GradRepeat { axis, from, range, from_v, factor } => {
                self.gradient_volume(*axis, out, v, |c| grad_repeat(c, *from, *range, *from_v, *factor))
            }
            S::GradMirror { axis, from, range, from_v, factor } => {
                self.gradient_volume(*axis, out, v, |c| grad_mirror(c, *from, *range, *from_v, *factor))
            }
            S::Add(a, b) => self.binary_volume(*a, *b, ctx, out, v, |l, r| *l += r),
            S::ConstAdd(a, c) => {
                self.volume(*a, ctx, out, v);
                out.iter_mut().for_each(|o| *o += c);
            }
            S::ConstSub(c, a) => {
                self.volume(*a, ctx, out, v);
                out.iter_mut().for_each(|o| *o = c - *o);
            }
            S::Sub(a, b) => self.binary_volume(*a, *b, ctx, out, v, |l, r| *l += -r),
            S::Mul(a, b) => self.binary_volume(*a, *b, ctx, out, v, |l, r| *l *= r),
            S::ConstMul(a, c) => {
                self.volume(*a, ctx, out, v);
                out.iter_mut().for_each(|o| *o *= c);
            }
            S::Div(a, b) => self.binary_volume(*a, *b, ctx, out, v, |l, r| *l /= r),
            S::ConstDiv(c, a) => {
                self.volume(*a, ctx, out, v);
                out.iter_mut().for_each(|o| *o = c / *o);
            }
            S::Min(a, b, _) => self.binary_volume(*a, *b, ctx, out, v, |l, r| {
                if r < *l {
                    *l = r;
                }
            }),
            S::ConstMin(a, c) => {
                self.volume(*a, ctx, out, v);
                out.iter_mut().for_each(|o| {
                    if *c < *o {
                        *o = *c;
                    }
                });
            }
            S::Max(a, b, _) => self.binary_volume(*a, *b, ctx, out, v, |l, r| {
                if r > *l {
                    *l = r;
                }
            }),
            S::ConstMax(a, c) => {
                self.volume(*a, ctx, out, v);
                out.iter_mut().for_each(|o| {
                    if *c > *o {
                        *o = *c;
                    }
                });
            }
            S::Unary(u, a) => {
                self.volume(*a, ctx, out, v);
                out.iter_mut().for_each(|o| *o = unary(*u, *o));
            }
            S::RoundInt(k, a) => {
                self.volume(*a, ctx, out, v);
                let floor_java = ctx.opts.floor_java;
                out.iter_mut().for_each(|o| *o = round_to_integer(*o, *k, floor_java));
            }
            S::Round(k, a, m) => {
                let floor_java = ctx.opts.floor_java;
                self.binary_volume(*a, *m, ctx, out, v, |l, r| *l = round_apply(*k, *l, r, floor_java));
            }
            S::PowConstBase(b, e) => {
                self.volume(*e, ctx, out, v);
                out.iter_mut().for_each(|o| *o = b.powf(f64::from(*o)) as f32);
            }
            S::PowConstExp(b, e) => {
                self.volume(*b, ctx, out, v);
                out.iter_mut().for_each(|o| *o = f64::from(*o).powf(*e) as f32);
            }
            S::Pow(b, e) => {
                self.binary_volume(*b, *e, ctx, out, v, |l, r| *l = f64::from(*l).powf(f64::from(r)) as f32)
            }
            S::Spline { spline, coords } => self.spline_volume(spline, coords, ctx, out, v),
            // Choice: each branch only on the rows where an element reads it.
            S::LerpConstFirst(a, f, s2) if ctx.opts.range_yvol && self.restrictable(s) => {
                self.volume(*a, ctx, out, v);
                let mut sb = ctx.take(v.size());
                self.count_branch(*s2, v, out, |a| a != 0.0);
                self.volume_rows(*s2, ctx, &mut sb, v, rows_hull(v, out, |a| a != 0.0));
                for (o, sv) in out.iter_mut().zip(&sb) {
                    *o = lerp_choice(*o, *f, *sv);
                }
                ctx.give(sb);
            }
            S::LerpConstSecond(a, f, s2) if ctx.opts.range_yvol && self.restrictable(s) => {
                self.volume(*a, ctx, out, v);
                let mut fb = ctx.take(v.size());
                self.count_branch(*f, v, out, |a| a != 1.0);
                self.volume_rows(*f, ctx, &mut fb, v, rows_hull(v, out, |a| a != 1.0));
                for (o, fv) in out.iter_mut().zip(&fb) {
                    *o = lerp_choice(*o, *fv, *s2);
                }
                ctx.give(fb);
            }
            S::Lerp(a, f, s2) if ctx.opts.range_yvol && self.restrictable(s) => {
                self.volume(*a, ctx, out, v);
                let mut fb = ctx.take(v.size());
                self.count_branch(*f, v, out, |a| a != 1.0);
                self.volume_rows(*f, ctx, &mut fb, v, rows_hull(v, out, |a| a != 1.0));
                let mut sb = ctx.take(v.size());
                self.count_branch(*s2, v, out, |a| a != 0.0);
                self.volume_rows(*s2, ctx, &mut sb, v, rows_hull(v, out, |a| a != 0.0));
                for i in 0..out.len() {
                    out[i] = lerp_choice(out[i], fb[i], sb[i]);
                }
                ctx.give(fb);
                ctx.give(sb);
            }
            S::Range { input, min, max, inside, outside } if ctx.opts.range_yvol && self.restrictable(s) => {
                let (min, max) = (*min, *max);
                let inr = |x: f32| x >= min && x < max;
                let mut ib = ctx.take(v.size());
                self.volume(*input, ctx, &mut ib, v);
                self.count_branch(*inside, v, &ib, inr);
                self.count_branch(*outside, v, &ib, |x| !inr(x));
                self.volume_rows(*inside, ctx, out, v, rows_hull(v, &ib, inr));
                if let Some(h) = rows_hull(v, &ib, |x| !inr(x)) {
                    let mut ob = ctx.take(v.size());
                    self.volume_rows(*outside, ctx, &mut ob, v, Some(h));
                    for i in 0..out.len() {
                        if !inr(ib[i]) {
                            out[i] = ob[i];
                        }
                    }
                    ctx.give(ob);
                }
                ctx.give(ib);
            }
            S::IntervalSingle { input, threshold, below, above } if ctx.opts.range_yvol && self.restrictable(s) => {
                let t = *threshold;
                // NaN goes above, like the old path.
                let is_below = |x: f32| x < t;
                let mut ib = ctx.take(v.size());
                self.volume(*input, ctx, &mut ib, v);
                self.count_branch(*below, v, &ib, is_below);
                self.count_branch(*above, v, &ib, |x| !is_below(x));
                self.volume_rows(*below, ctx, out, v, rows_hull(v, &ib, is_below));
                if let Some(h) = rows_hull(v, &ib, |x| !is_below(x)) {
                    let mut ab = ctx.take(v.size());
                    self.volume_rows(*above, ctx, &mut ab, v, Some(h));
                    for i in 0..out.len() {
                        if !is_below(ib[i]) {
                            out[i] = ab[i];
                        }
                    }
                    ctx.give(ab);
                }
                ctx.give(ib);
            }
            S::IntervalSelect { input, thresholds, functions } if ctx.opts.range_yvol && self.restrictable(s) => {
                // Index of the function chosen per element (small integer, exact in f32).
                let mut kb = ctx.take(v.size());
                self.volume(*input, ctx, &mut kb, v);
                let last = functions.len() - 1;
                for o in kb.iter_mut() {
                    let x = *o;
                    *o = thresholds.iter().position(|t| x < *t).unwrap_or(last) as f32;
                }
                let mut fb = ctx.take(v.size());
                for (k, f) in functions.iter().enumerate() {
                    let kf = k as f32;
                    self.count_branch(*f, v, &kb, |x| x == kf);
                    let Some(h) = rows_hull(v, &kb, |x| x == kf) else { continue };
                    self.volume_rows(*f, ctx, &mut fb, v, Some(h));
                    for i in 0..out.len() {
                        if kb[i] == kf {
                            out[i] = fb[i];
                        }
                    }
                }
                ctx.give(fb);
                ctx.give(kb);
            }
            S::LerpConstFirst(a, f, s2) => {
                self.volume(*a, ctx, out, v);
                let mut sb = ctx.take(v.size());
                self.volume(*s2, ctx, &mut sb, v);
                for (o, sv) in out.iter_mut().zip(&sb) {
                    *o = lerp_choice(*o, *f, *sv);
                }
                ctx.give(sb);
            }
            S::LerpConstSecond(a, f, s2) => {
                self.volume(*a, ctx, out, v);
                let mut fb = ctx.take(v.size());
                self.volume(*f, ctx, &mut fb, v);
                for (o, fv) in out.iter_mut().zip(&fb) {
                    *o = lerp_choice(*o, *fv, *s2);
                }
                ctx.give(fb);
            }
            S::Lerp(a, f, s2) => {
                self.volume(*a, ctx, out, v);
                let mut fb = ctx.take(v.size());
                self.volume(*f, ctx, &mut fb, v);
                let mut sb = ctx.take(v.size());
                self.volume(*s2, ctx, &mut sb, v);
                for i in 0..out.len() {
                    out[i] = lerp_choice(out[i], fb[i], sb[i]);
                }
                ctx.give(fb);
                ctx.give(sb);
            }
            S::Clamp(a, mn, mx) => {
                self.volume(*a, ctx, out, v);
                out.iter_mut().for_each(|o| *o = clamp(*o, *mn, *mx));
            }
            S::RangeConst { input, min, max, inside, outside } => {
                self.volume(*input, ctx, out, v);
                out.iter_mut().for_each(|o| *o = if *o >= *min && *o < *max { *inside } else { *outside });
            }
            S::Range { input, min, max, inside, outside } => {
                self.volume(*inside, ctx, out, v);
                let mut ib = ctx.take(v.size());
                self.volume(*input, ctx, &mut ib, v);
                let mut ob = ctx.take(v.size());
                self.volume(*outside, ctx, &mut ob, v);
                for i in 0..out.len() {
                    let x = ib[i];
                    if !(x >= *min && x < *max) {
                        out[i] = ob[i];
                    }
                }
                ctx.give(ib);
                ctx.give(ob);
            }
            S::IntervalSingle { input, threshold, below, above } => {
                self.volume(*input, ctx, out, v);
                let mut bb = ctx.take(v.size());
                self.volume(*below, ctx, &mut bb, v);
                let mut ab = ctx.take(v.size());
                self.volume(*above, ctx, &mut ab, v);
                for i in 0..out.len() {
                    out[i] = if out[i] < *threshold { bb[i] } else { ab[i] };
                }
                ctx.give(bb);
                ctx.give(ab);
            }
            S::IntervalSelect { input, thresholds, functions } => {
                self.volume(*input, ctx, out, v);
                let mut bufs = Vec::with_capacity(functions.len());
                for f in functions {
                    let mut b = ctx.take(v.size());
                    self.volume(*f, ctx, &mut b, v);
                    bufs.push(b);
                }
                for i in 0..out.len() {
                    let x = out[i];
                    let k = thresholds.iter().position(|t| x < *t).unwrap_or(functions.len() - 1);
                    out[i] = bufs[k][i];
                }
                for b in bufs {
                    ctx.give(b);
                }
            }
            S::Cache { id, input } => self.volume_cached(*id, *input, ctx, out, v),
            S::Interpolated { input, cxz, cy, ixz, iy } => {
                let (cxz, cy) = (*cxz, *cy);
                if (v.stx == cxz || v.sx == 1)
                    && (v.sty == cy || v.sy == 1)
                    && (v.stz == cxz || v.sz == 1)
                    && floor_mod(v.x0, cxz) == 0
                    && floor_mod(v.y0, cy) == 0
                    && floor_mod(v.z0, cxz) == 0
                {
                    self.volume(*input, ctx, out, v);
                } else if v.stx == 1 && v.sty == 1 && v.stz == 1 {
                    self.interpolate_blocks(*input, cxz, cy, *ixz, *iy, ctx, out, v);
                } else {
                    let bv = Volume::new(v.sx * v.stx, v.sy * v.sty, v.sz * v.stz, v.x0, v.y0, v.z0);
                    let mut bb = ctx.take(bv.size());
                    self.interpolate_blocks(*input, cxz, cy, *ixz, *iy, ctx, &mut bb, &bv);
                    for z in 0..v.sz {
                        for x in 0..v.sx {
                            for y in 0..v.sy {
                                out[v.index(x, y, z)] = bb[bv.index(x * v.stx, y * v.sty, z * v.stz)];
                            }
                        }
                    }
                    ctx.give(bb);
                }
            }
            S::SliceX(a, c) => {
                if v.sx == 1 && v.x0 == *c {
                    self.volume(*a, ctx, out, v);
                } else {
                    let iv = Volume::stepped(1, v.sy, v.sz, *c, v.y0, v.z0, v.stx, v.sty, v.stz);
                    let mut ib = ctx.take(iv.size());
                    self.volume(*a, ctx, &mut ib, &iv);
                    let mut i = 0;
                    for z in 0..v.sz {
                        for _ in 0..v.sx {
                            for y in 0..v.sy {
                                out[i] = ib[iv.index(0, y, z)];
                                i += 1;
                            }
                        }
                    }
                    ctx.give(ib);
                }
            }
            S::SliceY(a, c) => {
                if v.sy == 1 && v.y0 == *c {
                    self.volume(*a, ctx, out, v);
                } else {
                    let iv = Volume::stepped(v.sx, 1, v.sz, v.x0, *c, v.z0, v.stx, v.sty, v.stz);
                    let mut ib = ctx.take(iv.size());
                    self.volume(*a, ctx, &mut ib, &iv);
                    for z in 0..v.sz {
                        for x in 0..v.sx {
                            let val = ib[iv.index(x, 0, z)];
                            let at = v.index(x, 0, z);
                            out[at..at + v.sy as usize].fill(val);
                        }
                    }
                    ctx.give(ib);
                }
            }
            S::SliceZ(a, c) => {
                if v.sz == 1 && v.z0 == *c {
                    self.volume(*a, ctx, out, v);
                } else {
                    let iv = Volume::stepped(v.sx, v.sy, 1, v.x0, v.y0, *c, v.stx, v.sty, v.stz);
                    let mut ib = ctx.take(iv.size());
                    self.volume(*a, ctx, &mut ib, &iv);
                    let mut i = 0;
                    for _ in 0..v.sz {
                        for x in 0..v.sx {
                            for y in 0..v.sy {
                                out[i] = ib[iv.index(x, y, 0)];
                                i += 1;
                            }
                        }
                    }
                    ctx.give(ib);
                }
            }
            S::SliceXz(a, cx, cz) => {
                if v.sx == 1 && v.sz == 1 && v.x0 == *cx && v.z0 == *cz {
                    self.volume(*a, ctx, out, v);
                } else {
                    let iv = Volume::stepped(1, v.sy, 1, *cx, v.y0, *cz, v.stx, v.sty, v.stz);
                    let mut ib = ctx.take(iv.size());
                    self.volume(*a, ctx, &mut ib, &iv);
                    for y in 0..v.sy {
                        let val = ib[iv.index(0, y, 0)];
                        let mut at = v.index(0, y, 0);
                        for _ in 0..v.sz {
                            for _ in 0..v.sx {
                                out[at] = val;
                                at += v.sy as usize;
                            }
                        }
                    }
                    ctx.give(ib);
                }
            }
            S::FindTop { density, upper, lower, cell } => {
                assert!(v.sy == 1, "find_top_surface in a volume of height {}", v.sy);
                self.volume(*upper, ctx, out, v);
                let mut i = 0;
                for z in 0..v.sz {
                    let bz = v.bz(z);
                    for x in 0..v.sx {
                        let bx = v.bx(x);
                        out[i] = self.find_top(*density, *lower, *cell, ctx, bx, bz, out[i]);
                        i += 1;
                    }
                }
            }
        }
    }

    /// Spline over a volume. Each coordinate's volume is computed the first time an element reads
    /// it, never all of them up front: a coordinate read only on some branches may stay uncomputed.
    fn spline_volume(&self, spline: &SSpline, coords: &[Sid], ctx: &mut Ctx, out: &mut [f32], v: &Volume) {
        // The 26.3 splines read at most 4 coordinates: their buffers fit on the stack, without an
        // allocated list per call.
        let mut on_stack: [Option<Vec<f32>>; 8] = Default::default();
        let mut on_heap: Vec<Option<Vec<f32>>>;
        let bufs: &mut [Option<Vec<f32>>] = if ctx.opts.spline_inputs_on_stack && coords.len() <= on_stack.len() {
            &mut on_stack[..coords.len()]
        } else {
            on_heap = vec![None; coords.len()];
            &mut on_heap
        };
        let direct = ctx.opts.spline_stack;
        for (i, o) in out.iter_mut().enumerate() {
            let mut input = |c: usize| {
                if bufs[c].is_none() {
                    let mut b = ctx.take(v.size());
                    self.volume(coords[c], ctx, &mut b, v);
                    bufs[c] = Some(b);
                }
                bufs[c].as_ref().map_or(0.0, |b| b[i])
            };
            *o = if direct {
                spline_sample(spline, &mut input)
            } else {
                spline_sample::<dyn FnMut(usize) -> f32>(spline, &mut input)
            };
        }
        for b in bufs.iter_mut().filter_map(Option::take) {
            ctx.give(b);
        }
    }

    fn naive(&self, s: Sid, ctx: &mut Ctx, out: &mut [f32], v: &Volume) {
        let mut i = 0;
        for z in 0..v.sz {
            let bz = v.bz(z);
            for x in 0..v.sx {
                let bx = v.bx(x);
                for y in 0..v.sy {
                    out[i] = self.value(s, ctx, bx, v.by(y), bz);
                    i += 1;
                }
            }
        }
    }

    fn gradient_volume(&self, axis: Axis, out: &mut [f32], v: &Volume, f: impl Fn(i32) -> f32) {
        match axis {
            Axis::X => {
                for x in 0..v.sx {
                    let val = f(v.bx(x));
                    for z in 0..v.sz {
                        let at = v.index(x, 0, z);
                        out[at..at + v.sy as usize].fill(val);
                    }
                }
            }
            Axis::Y => {
                for y in 0..v.sy {
                    let val = f(v.by(y));
                    for z in 0..v.sz {
                        for x in 0..v.sx {
                            out[v.index(x, y, z)] = val;
                        }
                    }
                }
            }
            Axis::Z => {
                for z in 0..v.sz {
                    let val = f(v.bz(z));
                    let at = v.index(0, 0, z);
                    out[at..at + (v.sx * v.sy) as usize].fill(val);
                }
            }
        }
    }

    fn binary_volume(&self, a: Sid, b: Sid, ctx: &mut Ctx, out: &mut [f32], v: &Volume, op: impl Fn(&mut f32, f32)) {
        self.volume(a, ctx, out, v);
        let rb = self.borrow(b, ctx, v);
        for (o, r) in out.iter_mut().zip(&rb.0) {
            op(o, *r);
        }
        self.unborrow(ctx, rb);
    }

    /// Cell-wise interpolation of a volume of step 1 (§ 3.8).
    #[allow(clippy::too_many_arguments)]
    #[allow(clippy::explicit_counter_loop)]
    fn interpolate_blocks(
        &self,
        input: Sid,
        cxz: i32,
        cy: i32,
        ixz: f32,
        iy: f32,
        ctx: &mut Ctx,
        out: &mut [f32],
        v: &Volume,
    ) {
        let (mncx, mncy, mncz) = (floor_div(v.x0, cxz), floor_div(v.y0, cy), floor_div(v.z0, cxz));
        let (mxcx, mxcy, mxcz) = (floor_div(v.max_x(), cxz), floor_div(v.max_y(), cy), floor_div(v.max_z(), cxz));
        let (ncx, ncy, ncz) = (mxcx - mncx + 1, mxcy - mncy + 1, mxcz - mncz + 1);
        let cv = Volume::stepped(
            if floor_mod(v.max_x(), cxz) == 0 { ncx } else { ncx + 1 },
            if floor_mod(v.max_y(), cy) == 0 { ncy } else { ncy + 1 },
            if floor_mod(v.max_z(), cxz) == 0 { ncz } else { ncz + 1 },
            mncx * cxz,
            mncy * cy,
            mncz * cxz,
            cxz,
            cy,
            cxz,
        );
        let mut cb = ctx.take(cv.size());
        self.volume(input, ctx, &mut cb, &cv);
        for cz in 0..ncz {
            let nz = (cz + 1).min(cv.sz - 1);
            for cx in 0..ncx {
                let nx = (cx + 1).min(cv.sx - 1);
                let mut v000 = cb[cv.index(cx, 0, cz)];
                let mut v100 = cb[cv.index(nx, 0, cz)];
                let mut v001 = cb[cv.index(cx, 0, nz)];
                let mut v101 = cb[cv.index(nx, 0, nz)];
                for cyy in 0..ncy {
                    let ny = (cyy + 1).min(cv.sy - 1);
                    let v010 = cb[cv.index(cx, ny, cz)];
                    let v110 = cb[cv.index(nx, ny, cz)];
                    let v011 = cb[cv.index(cx, ny, nz)];
                    let v111 = cb[cv.index(nx, ny, nz)];
                    let ox = cv.bx(cx) - v.x0;
                    let oy = cv.by(cyy) - v.y0;
                    let oz = cv.bz(cz) - v.z0;
                    let (x0, y0, z0) = (0.max(-ox), 0.max(-oy), 0.max(-oz));
                    let x1 = cxz.min(v.sx - ox) - 1;
                    let y1 = cy.min(v.sy - oy) - 1;
                    let z1 = cxz.min(v.sz - oz) - 1;
                    for z in z0..=z1 {
                        let az = z as f32 * ixz;
                        let v00 = lerp(az, v000, v001);
                        let v01 = lerp(az, v010, v011);
                        let v10 = lerp(az, v100, v101);
                        let v11 = lerp(az, v110, v111);
                        for x in x0..=x1 {
                            let ax = x as f32 * ixz;
                            let lo = lerp(ax, v00, v10);
                            let hi = lerp(ax, v01, v11);
                            let step = (hi - lo) * iy;
                            let mut val = lo + step * y0 as f32;
                            let mut at = v.index(ox + x, oy + y0, oz + z);
                            for _ in y0..=y1 {
                                out[at] = val;
                                at += 1;
                                val += step;
                            }
                        }
                    }
                    v000 = v010;
                    v100 = v110;
                    v001 = v011;
                    v101 = v111;
                }
            }
        }
        ctx.give(cb);
    }
}

#[cfg(test)]
mod tests {
    use super::size_class;

    #[test]
    fn size_classes_cover_the_request_and_keep_a_chunk_volume_exact() {
        assert_eq!(size_class(16 * 384 * 16), 16 * 384 * 16);
        assert_eq!(size_class(8), 8);
        assert_eq!(size_class(65), 96);
        assert_eq!(size_class(97), 128);
        for n in 0..5000 {
            let c = size_class(n);
            assert!(c >= n && c <= (n * 3 / 2).max(4), "{n} -> {c}");
        }
    }
}
