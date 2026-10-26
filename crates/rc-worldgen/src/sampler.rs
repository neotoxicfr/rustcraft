//! Compiled samplers and their evaluation (spec § 3.4–3.8), point path and volume
//! path, as the game uses them (they are not always bit-for-bit equal).

use crate::density::{Axis, Metric, RoundKind, Unary, round_to_integer, squeeze};
use crate::math::{clamp, floor_div, floor_f, floor_mod, jmax, jmin, lerp, lerp3, signum};
use crate::noise::{NoiseStack, Simplex};
use crate::volume::Volume;

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
}

impl Opts {
    /// Code from before the mission.
    pub const OLD: Opts = Opts { range_yvol: false, vol_pool: false, spline_stack: false };
    /// All the tracks.
    pub const NEW: Opts = Opts { range_yvol: true, vol_pool: true, spline_stack: true };

    pub fn current() -> Opts {
        static P: std::sync::LazyLock<Opts> = std::sync::LazyLock::new(|| Opts {
            range_yvol: crate::opt::range_yvol(),
            vol_pool: crate::opt::vol_pool(),
            spline_stack: crate::opt::spline_stack(),
        });
        *P
    }
}

impl Default for Opts {
    fn default() -> Self {
        Opts::current()
    }
}

/// Evaluation context: cache cells (like a context with caches in the game) and buffers.
#[derive(Debug, Default)]
pub struct Ctx {
    cells: Option<Vec<CacheCell>>,
    pool: Vec<Vec<f32>>,
    opts: Opts,
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
        Ctx { cells: None, pool: Vec::new(), opts }
    }

    /// Context with caches, paths forced (equivalence tests, benches).
    pub fn cached_with(opts: Opts) -> Self {
        Ctx { cells: Some(Vec::new()), pool: Vec::new(), opts }
    }

    /// Hands a buffer (received from [`Generator::volume`](crate::Generator::volume)) back to the context, which
    /// will take it again for the next volume (`vol_pool`).
    pub fn recycle(&mut self, v: Vec<f32>) {
        self.give(v);
    }

    /// Output buffer of a volume: taken from the context (`vol_pool`) or new.
    pub(crate) fn output(&mut self, n: usize) -> Vec<f32> {
        if self.opts.vol_pool { self.take(n) } else { vec![0.0; n] }
    }

    fn take(&mut self, n: usize) -> Vec<f32> {
        let mut v = self.pool.pop().unwrap_or_default();
        v.clear();
        v.resize(n, 0.0);
        v
    }

    fn give(&mut self, v: Vec<f32>) {
        if self.pool.len() < 64 {
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
fn round_apply(k: RoundKind, v: f32, m: f32) -> f32 {
    if m == 0.0 { v } else { round_to_integer(v / m, k) * m }
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
            _ => EXACT,
        };
        if let S::Cache { input, .. } = s
            && self.flags_of(input) & EXACT == 0
        {
            flags |= PINNED;
        }
        if matches!(s, S::Cache { .. }) {
            self.n_caches += 1;
        }
        s.children(|c| {
            cell = cell_join(cell, self.cell_of(c));
            let f = self.flags_of(c);
            flags = (flags & f & EXACT) | ((flags | f) & PINNED);
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
                self.noises[*noise as usize].get(f64::from(x) * xz, f64::from(y) * ys, f64::from(z) * xz)
            }
            S::ShiftedXz { sx, sz, noise, xz, y: ys } => {
                let nx = f64::from(x) * xz + f64::from(self.value(*sx, ctx, x, y, z));
                let ny = f64::from(y) * ys;
                let nz = f64::from(z) * xz + f64::from(self.value(*sz, ctx, x, y, z));
                self.noises[*noise as usize].get(nx, ny, nz)
            }
            S::ShiftedXyz { sx, sy, sz, noise, xz, y: ys } => {
                let nx = f64::from(x) * xz + f64::from(self.value(*sx, ctx, x, y, z));
                let ny = f64::from(y) * ys + f64::from(self.value(*sy, ctx, x, y, z));
                let nz = f64::from(z) * xz + f64::from(self.value(*sz, ctx, x, y, z));
                self.noises[*noise as usize].get(nx, ny, nz)
            }
            S::ShiftB { noise } => {
                self.noises[*noise as usize].get(f64::from(z) * 0.25, f64::from(x) * 0.25, 0.0) * 4.0
            }
            S::EndIslands { simplex } => (island_height(&self.simplex[*simplex as usize], x / 8, z / 8) - 8.0) / 128.0,
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
            S::RoundInt(k, a) => round_to_integer(self.value(*a, ctx, x, y, z), *k),
            S::Round(k, a, m) => {
                let v = self.value(*a, ctx, x, y, z);
                let m = self.value(*m, ctx, x, y, z);
                round_apply(*k, v, m)
            }
            S::PowConstBase(b, e) => b.powf(f64::from(self.value(*e, ctx, x, y, z))) as f32,
            S::PowConstExp(b, e) => f64::from(self.value(*b, ctx, x, y, z)).powf(*e) as f32,
            S::Pow(b, e) => {
                let b = self.value(*b, ctx, x, y, z);
                let e = self.value(*e, ctx, x, y, z);
                f64::from(b).powf(f64::from(e)) as f32
            }
            S::Spline { spline, coords } if ctx.opts.spline_stack && coords.len() <= 8 => {
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
        let top = floor_f(upper / cell as f32).wrapping_mul(cell);
        if top <= lower {
            return lower as f32;
        }
        let mut p = top;
        while p >= lower {
            if self.value(density, ctx, x, p, z) > 0.0 {
                return p as f32;
            }
            p -= cell;
        }
        lower as f32
    }

    fn value_cached(&self, id: u32, input: Sid, ctx: &mut Ctx, x: i32, y: i32, z: i32) -> f32 {
        let key = pack_pos(x, y, z);
        match ctx.cell(id, self.n_caches) {
            None => return self.value(input, ctx, x, y, z),
            Some(c) => {
                if c.key == key && !c.value.is_nan() {
                    return c.value;
                }
                if let (Some(b), Some(v)) = (&c.buf, &c.volume)
                    && let Some(i) = v.index_of_block(x, y, z)
                {
                    return b[i];
                }
            }
        }
        let v = self.value(input, ctx, x, y, z);
        if let Some(c) = ctx.cell(id, self.n_caches) {
            c.key = key;
            c.value = v;
        }
        v
    }

    fn volume_cached(&self, id: u32, input: Sid, ctx: &mut Ctx, out: &mut [f32], v: &Volume) {
        let rows = ctx.opts.range_yvol;
        let cell = self.cell_of(input);
        let hit = match ctx.cell(id, self.n_caches) {
            None => {
                self.volume(input, ctx, out, v);
                return;
            }
            Some(c) => match (&c.buf, &c.volume) {
                (Some(_), Some(w)) if w == v => true,
                // Rows of a volume already in cache (sub-volume of a choice restricted in y).
                (Some(b), Some(w)) if rows => {
                    if let Some(k) = rows_of(w, v, cell) {
                        let (sy, len) = (w.sy as usize, v.sy as usize);
                        for (col, dst) in out.chunks_exact_mut(len).enumerate() {
                            let at = col * sy + k;
                            dst.copy_from_slice(&b[at..at + len]);
                        }
                        return;
                    }
                    false
                }
                _ => false,
            },
        };
        if !hit {
            let old = ctx.cell(id, self.n_caches).and_then(|c| {
                c.volume = Some(*v);
                c.buf.take()
            });
            if let Some(o) = old {
                ctx.give(o);
            }
            let mut b = ctx.take(v.size());
            self.volume(input, ctx, &mut b, v);
            if let Some(c) = ctx.cell(id, self.n_caches) {
                c.buf = Some(b);
            }
        }
        if let Some(b) = ctx.cell(id, self.n_caches).and_then(|c| c.buf.as_ref()) {
            out.copy_from_slice(b);
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
            S::ShiftedXz { sx, sz, noise, xz, y: ys } => {
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
                out.iter_mut().for_each(|o| *o = round_to_integer(*o, *k));
            }
            S::Round(k, a, m) => self.binary_volume(*a, *m, ctx, out, v, |l, r| *l = round_apply(*k, *l, r)),
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
            S::Spline { spline, coords } => {
                let mut bufs: Vec<Option<Vec<f32>>> = vec![None; coords.len()];
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
                for b in bufs.into_iter().flatten() {
                    ctx.give(b);
                }
            }
            // Choice: each branch only on the rows where an element reads it.
            S::LerpConstFirst(a, f, s2) if ctx.opts.range_yvol && self.restrictable(s) => {
                self.volume(*a, ctx, out, v);
                let mut sb = ctx.take(v.size());
                self.volume_rows(*s2, ctx, &mut sb, v, rows_hull(v, out, |a| a != 0.0));
                for (o, sv) in out.iter_mut().zip(&sb) {
                    *o = lerp_choice(*o, *f, *sv);
                }
                ctx.give(sb);
            }
            S::LerpConstSecond(a, f, s2) if ctx.opts.range_yvol && self.restrictable(s) => {
                self.volume(*a, ctx, out, v);
                let mut fb = ctx.take(v.size());
                self.volume_rows(*f, ctx, &mut fb, v, rows_hull(v, out, |a| a != 1.0));
                for (o, fv) in out.iter_mut().zip(&fb) {
                    *o = lerp_choice(*o, *fv, *s2);
                }
                ctx.give(fb);
            }
            S::Lerp(a, f, s2) if ctx.opts.range_yvol && self.restrictable(s) => {
                self.volume(*a, ctx, out, v);
                let mut fb = ctx.take(v.size());
                self.volume_rows(*f, ctx, &mut fb, v, rows_hull(v, out, |a| a != 1.0));
                let mut sb = ctx.take(v.size());
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
