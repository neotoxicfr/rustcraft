//! Full light of a new chunk (`light` generation status): resolution from
//! zero over the 3×3 chunk zone around it, same rule as [`crate::relight`] (emission
//! sources, sky columns above their source height, propagation by decreasing levels,
//! occluding faces, minimum attenuation 1). Only the central chunk is returned:
//! light coming from beyond the zone cannot reach it (16 cells or more).
//!
//! New path (mission C2-M08, `docs/perf.md`), same result byte for byte as
//! [`light_chunk_inner`] (kept as is, taken when the three switches below and the property
//! grid are at 0; `RUSTCRAFT_OPT_INTERN_UNIFORM_LIGHT` only shares the uniform sheets of every path):
//! - `RUSTCRAFT_OPT_FULLSKIP`: void skipped. A "neutral" cell (zero emission and attenuation,
//!   no face) neither creates nor stops light. Above the highest non-neutral cell of the
//!   9 columns (`ymax`, excluded), the sky is 15 everywhere and the block light dies out 14 cells
//!   higher (emitter at `ymax − 1` at most, loss of at least 1 per step). The resolution is thus done
//!   over `[0, top)`, `top` = `ymax + 15` rounded to the section: the sections above are
//!   full sky sheets and zero block sheets, which the result already omits. Absent centre: no
//!   skip (its sky sheets are zero, not full).
//! - `RUSTCRAFT_OPT_FULLNOEMIT`: with a column scan ([`ColumnScan`]), the sections without
//!   an emitter are skipped in the search for block sources (without a scan: no effect).
//! - `RUSTCRAFT_OPT_FULLLAYOUT`: buffers kept per thread (zero reset on `[0, top)` only),
//!   search for emitters by rows of 256 contiguous states (outer y), sheets written two
//!   cells at a time.
//!
//! The top and the source height of each stack are read in a single top-to-bottom pass.
//! The order of the seeds in the buckets changes nothing: propagation by decreasing levels
//! with relaxation by maximum reaches the same fixed point whatever the order.
//!
//! Mission C2-M15 (`docs/perf.md`), on top of the switches above:
//! - `RUSTCRAFT_OPT_FULL_PROPERTY_GRID` and its sub-switches: resolution on a grid of cell
//!   properties with a ring of closed cells (`grid`);
//! - `RUSTCRAFT_OPT_FULL_UNION_TILES`: the chunks of a batch resolved by tiles, one zone per tile
//!   instead of one per chunk ([`light_chunks`], `batch`).

use std::cell::RefCell;
use std::sync::Arc;

use rc_world::{LIGHT_BYTES, LightLayer};

use crate::{LightTables, STEP};

mod batch;
mod grid;

pub(crate) use batch::light_chunks;
pub(crate) use grid::CLOSED;

const W: i32 = 48;

/// Light sheets of a chunk: sky, then block (see [`light_chunk_inner`]).
pub type LightSheets = (Vec<LightLayer>, Vec<LightLayer>);

/// State columns of the zone: index `(dz + 1)·3 + (dx + 1)`, each `16·16·height` states
/// indexed `(y << 8) | (z << 4) | x` (y relative to the bottom of the world); `None`: absent column
/// (neither light nor passage).
pub type Columns<'a> = [Option<&'a [u32]>; 9];

struct Grid<'a> {
    cols: Columns<'a>,
    height: i32,
}

impl Grid<'_> {
    #[inline]
    fn state_at(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        let c = self.cols[((z >> 4) * 3 + (x >> 4)) as usize]?;
        Some(c[((y << 8) | ((z & 15) << 4) | (x & 15)) as usize])
    }

    #[inline]
    fn index(&self, x: i32, y: i32, z: i32) -> usize {
        ((y * W + z) * W + x) as usize
    }
}

/// Light sheets (sky, block) of the central chunk, `height / 16 + 2` positions (one below the
/// world, one above): zero block sheets absent; full sky sheets (15) at the
/// top absent (the game reads them full). `has_sky` false: no sky sheet. `intern_uniform`: see
/// [`FullOpts::intern_uniform`].
pub(crate) fn light_chunk_inner(
    t: &LightTables,
    cols: Columns,
    height: i32,
    has_sky: bool,
    intern_uniform: bool,
) -> (Vec<LightLayer>, Vec<LightLayer>) {
    let g = Grid { cols, height };
    let n = (W * W * height) as usize;
    crate::stats::FULL_CELLS.fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
    let sections = (height / 16) as usize;
    let mut level = vec![0u8; n];
    let mut buckets: Vec<Vec<u32>> = vec![Vec::new(); 16];
    // Block light: emission sources.
    for z in 0..W {
        for x in 0..W {
            if g.cols[((z >> 4) * 3 + (x >> 4)) as usize].is_none() {
                continue;
            }
            for y in 0..height {
                let s = g.state_at(x, y, z).unwrap_or(0);
                let e = t.emission.get(s as usize).copied().unwrap_or(0);
                if e > 0 {
                    let i = g.index(x, y, z);
                    level[i] = e;
                    buckets[e as usize].push(i as u32);
                }
            }
        }
    }
    propagate(&g, t, &mut level, &mut buckets);
    let block = pack_sheets(&level, sections, 0, false, intern_uniform);
    if !has_sky {
        return (vec![None; sections + 2], block);
    }
    // Sky: source height of each column, sources pushed only where they border
    // a lower cell (the rest propagates nothing new).
    level.fill(0);
    let mut src = vec![i32::MAX; (W * W) as usize];
    for z in 0..W {
        for x in 0..W {
            if g.cols[((z >> 4) * 3 + (x >> 4)) as usize].is_some() {
                src[(z * W + x) as usize] = source_height(&g, t, x, z).max(0);
            }
        }
    }
    for z in 0..W {
        for x in 0..W {
            let from = src[(z * W + x) as usize];
            if from == i32::MAX {
                continue;
            }
            for y in from..height {
                level[g.index(x, y, z)] = 15;
            }
            let mut top = from;
            for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                let (nx, nz) = (x + dx, z + dz);
                if (0..W).contains(&nx) && (0..W).contains(&nz) {
                    let o = src[(nz * W + nx) as usize];
                    if o != i32::MAX {
                        top = top.max(o);
                    }
                }
            }
            for y in from..=top.min(height - 1) {
                buckets[15].push(g.index(x, y, z) as u32);
            }
        }
    }
    propagate(&g, t, &mut level, &mut buckets);
    let sky = pack_sheets(&level, sections, 0xff, true, intern_uniform);
    (sky, block)
}

/// Source height of a column: the smallest y whose positions up to the top are all
/// sources (below the first occluded edge going down).
fn source_height(g: &Grid, t: &LightTables, x: i32, z: i32) -> i32 {
    let mut above = 0u32;
    for y in (0..g.height).rev() {
        let below = g.state_at(x, y, z).unwrap_or(0);
        let damp = t.dampening.get(below as usize).copied().unwrap_or(0);
        if damp != 0 || t.occluded(above, below, 0) {
            return y + 1;
        }
        above = below;
    }
    i32::MIN
}

fn propagate(g: &Grid, t: &LightTables, level: &mut [u8], buckets: &mut [Vec<u32>]) {
    for l in (2..16usize).rev() {
        while let Some(i) = buckets[l].pop() {
            let i = i as usize;
            if level[i] as usize != l {
                continue;
            }
            let ii = i as i32;
            let (x, z, y) = (ii % W, (ii / W) % W, ii / (W * W));
            let Some(si) = g.state_at(x, y, z) else { continue };
            for (dir, s) in STEP.iter().enumerate() {
                let (qx, qy, qz) = (x + s[0], y + s[1], z + s[2]);
                if !(0..W).contains(&qx) || !(0..W).contains(&qz) || !(0..g.height).contains(&qy) {
                    continue;
                }
                let Some(sj) = g.state_at(qx, qy, qz) else { continue };
                if t.occluded(si, sj, dir) {
                    continue;
                }
                let nv = l as i32 - i32::from(t.opacity(sj));
                let j = g.index(qx, qy, qz);
                if nv > i32::from(level[j]) {
                    level[j] = nv as u8;
                    buckets[nv as usize].push(j as u32);
                }
            }
        }
    }
}

/// Switches of the full light (see the header), read once per process; the benches pass their
/// own to compare the variants in one executable.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FullOpts {
    /// `RUSTCRAFT_OPT_FULLSKIP`.
    pub skip: bool,
    /// `RUSTCRAFT_OPT_FULLNOEMIT`.
    pub noemit: bool,
    /// `RUSTCRAFT_OPT_FULLLAYOUT` (buffers kept per thread on every new path).
    pub layout: bool,
    /// All-0 and all-15 sheets replaced by the shared ones ([`crate::uniform_light_interned`]).
    pub intern_uniform: bool,
    /// `RUSTCRAFT_OPT_FULL_PROPERTY_GRID` (`grid`; tables of more than 4,095 shapes: not taken).
    pub property_grid: bool,
    /// `RUSTCRAFT_OPT_FULL_OCCLUSION_TABLE` (property grid only).
    pub occlusion_table: bool,
    /// `RUSTCRAFT_OPT_FULL_QUEUE_DIRECTIONS` (property grid only).
    pub queue_directions: bool,
    /// `RUSTCRAFT_OPT_FULL_NO_EMITTER_EXIT` (property grid only).
    pub no_emitter_exit: bool,
    /// `RUSTCRAFT_OPT_FULL_SKY_SEED_FILTER` (property grid only).
    pub sky_seed_filter: bool,
    /// `RUSTCRAFT_OPT_FULL_EMITTERS_IN_FILL` (property grid only).
    pub emitters_in_fill: bool,
    /// `RUSTCRAFT_OPT_FULL_SKIP_DEPTHS` (property grid only, with the scans).
    pub skip_depths: bool,
    /// `RUSTCRAFT_OPT_FULL_UNION_TILES` (`batch`, with the property grid only).
    pub union_tiles: bool,
    /// Side of a union tile in chunks (`RUSTCRAFT_OPT_FULL_UNION_TILE_SIDE`, 2 to 8; 0: chosen per
    /// batch, see `batch`).
    pub union_tile_side: i32,
    /// Bytes of buffers a thread keeps between two lightings at most
    /// (`RUSTCRAFT_OPT_FULL_KEPT_BUFFER_MIB`, see [`with_bufs`]).
    pub max_kept_buffer_bytes: usize,
}

impl FullOpts {
    /// Every switch off: the old path.
    pub const OLD: FullOpts = FullOpts {
        skip: false,
        noemit: false,
        layout: false,
        intern_uniform: false,
        property_grid: false,
        occlusion_table: false,
        queue_directions: false,
        no_emitter_exit: false,
        sky_seed_filter: false,
        emitters_in_fill: false,
        skip_depths: false,
        union_tiles: false,
        union_tile_side: 0,
        max_kept_buffer_bytes: DEFAULT_KEPT_BUFFER_BYTES,
    };

    /// Every switch on.
    pub const ALL: FullOpts = FullOpts {
        skip: true,
        noemit: true,
        layout: true,
        intern_uniform: true,
        property_grid: true,
        occlusion_table: true,
        queue_directions: true,
        no_emitter_exit: true,
        sky_seed_filter: true,
        emitters_in_fill: true,
        skip_depths: true,
        union_tiles: true,
        union_tile_side: 0,
        max_kept_buffer_bytes: DEFAULT_KEPT_BUFFER_BYTES,
    };

    /// The process's switches (`=0`: old path).
    pub fn current() -> FullOpts {
        FullOpts {
            skip: rc_data::opt_switch!("RUSTCRAFT_OPT_FULLSKIP"),
            noemit: rc_data::opt_switch!("RUSTCRAFT_OPT_FULLNOEMIT"),
            layout: rc_data::opt_switch!("RUSTCRAFT_OPT_FULLLAYOUT"),
            intern_uniform: crate::uniform_light_interned(),
            property_grid: rc_data::opt_switch!("RUSTCRAFT_OPT_FULL_PROPERTY_GRID"),
            occlusion_table: rc_data::opt_switch!("RUSTCRAFT_OPT_FULL_OCCLUSION_TABLE"),
            queue_directions: rc_data::opt_switch!("RUSTCRAFT_OPT_FULL_QUEUE_DIRECTIONS"),
            no_emitter_exit: rc_data::opt_switch!("RUSTCRAFT_OPT_FULL_NO_EMITTER_EXIT"),
            sky_seed_filter: rc_data::opt_switch!("RUSTCRAFT_OPT_FULL_SKY_SEED_FILTER"),
            emitters_in_fill: rc_data::opt_switch!("RUSTCRAFT_OPT_FULL_EMITTERS_IN_FILL"),
            skip_depths: rc_data::opt_switch!("RUSTCRAFT_OPT_FULL_SKIP_DEPTHS"),
            union_tiles: rc_data::opt_switch!("RUSTCRAFT_OPT_FULL_UNION_TILES"),
            union_tile_side: union_tile_side(),
            max_kept_buffer_bytes: max_kept_buffer_bytes(),
        }
    }

    /// None of the resolution switches: [`light_chunk_inner`] resolves.
    pub(crate) fn old_resolution(self) -> bool {
        !self.skip && !self.noemit && !self.layout && !self.property_grid
    }

    /// A batch is resolved by tiles (`batch`), which needs the property grid.
    pub(crate) fn lights_by_tiles(self, t: &LightTables) -> bool {
        self.union_tiles && self.property_grid && t.has_property_grid()
    }
}

/// `RUSTCRAFT_OPT_FULL_UNION_TILE_SIDE`: side of a union tile in chunks, 2 to 8 (otherwise 0:
/// chosen per batch), read once.
fn union_tile_side() -> i32 {
    static SIDE: std::sync::OnceLock<i32> = std::sync::OnceLock::new();
    *SIDE.get_or_init(|| {
        let side = rc_data::env_str!("RUSTCRAFT_OPT_FULL_UNION_TILE_SIDE").and_then(|v| v.parse().ok());
        side.filter(|s| (2..=8).contains(s)).unwrap_or(0)
    })
}

/// Buffers a thread keeps by default: the 3 × 3 zone of a world 384 high with its ring fits
/// (0.97 MB of levels and 1.93 MB of properties, plus the queues), a batch tile does not (5 to 11 MB
/// at full height).
const DEFAULT_KEPT_BUFFER_BYTES: usize = 4 << 20;

/// `RUSTCRAFT_OPT_FULL_KEPT_BUFFER_MIB`: MiB of buffers a thread keeps at most, 1 to 256 (otherwise
/// 4), read once. Above 11, the tiles of a batch of a world 384 high reuse their thread's buffers
/// instead of allocating theirs.
fn max_kept_buffer_bytes() -> usize {
    static BYTES: std::sync::OnceLock<usize> = std::sync::OnceLock::new();
    *BYTES.get_or_init(|| {
        let mib = rc_data::env_str!("RUSTCRAFT_OPT_FULL_KEPT_BUFFER_MIB").and_then(|v| v.parse::<usize>().ok());
        mib.filter(|m| (1..=256).contains(m)).map_or(DEFAULT_KEPT_BUFFER_BYTES, |m| m << 20)
    })
}

thread_local! {
    /// Deliberate error of the tests (constants `SABOTAGE_*` of this module, `grid` and `batch`).
    pub(crate) static SABOTAGE: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
}

/// The deliberate error `k` is on (tests only; always false otherwise).
#[inline(always)]
fn sabotaged(k: u8) -> bool {
    #[cfg(test)]
    {
        SABOTAGE.with(|s| s.get() == k)
    }
    #[cfg(not(test))]
    {
        let _ = k;
        false
    }
}

/// Scan of a state column, valid as long as its states do not change: reused by the
/// 9 chunk lightings that read it (generation batch cache).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ColumnScan {
    /// Highest non-neutral cell + 1 (0: all-neutral column).
    top: i32,
    /// Sky source height of each stack (`z·16 + x`), `i32::MIN`: none.
    src: Box<[i32; 256]>,
    /// Sections carrying at least one emitter (bit `s`, sections 0..256).
    emit: [u64; 4],
}

/// Scans of the 9 columns, same order as [`Columns`].
pub type Scans<'a> = [Option<&'a ColumnScan>; 9];

/// Scan of column `col` (`16·16·height` states, see [`Columns`]).
pub fn scan_column(t: &LightTables, col: &[u32], height: i32) -> ColumnScan {
    let mut src = Box::new([i32::MIN; 256]);
    let mut top = 0;
    for z in 0..16 {
        for x in 0..16 {
            let (h, s) = stack(t, col, height, x, z, true);
            top = top.max(h);
            src[(z * 16 + x) as usize] = s;
        }
    }
    let mut emit = [0u64; 4];
    for y in 0..top {
        let sy = (y >> 4) as usize;
        if sy >= 256 || emit[sy >> 6] >> (sy & 63) & 1 != 0 {
            continue;
        }
        let row = &col[(y << 8) as usize..((y + 1) << 8) as usize];
        if row.iter().any(|&s| t.emission.get(s as usize).is_some_and(|&e| e > 0)) {
            emit[sy >> 6] |= 1 << (sy & 63);
        }
    }
    ColumnScan { top, src, emit }
}

impl ColumnScan {
    /// Section `section` may hold an emitter (sections from 256 on are not tracked: always).
    fn section_has_emitter(&self, section: i32) -> bool {
        let s = section as usize;
        s >= 256 || self.emit[s >> 6] >> (s & 63) & 1 != 0
    }

    /// Lowest section holding an emitter (`None`: no emitter in the 256 tracked sections).
    fn lowest_emitter_section(&self) -> Option<i32> {
        let (word, bits) = self.emit.iter().enumerate().find(|(_, w)| **w != 0)?;
        Some(word as i32 * 64 + bits.trailing_zeros() as i32)
    }
}

/// Stack `(x, z)` of a column, top to bottom: (highest non-neutral cell + 1, sky
/// source height or `i32::MIN`, the latter only if `want_src`). Same rule as
/// [`source_height`]: neutral cells do not attenuate and have no face.
fn stack(t: &LightTables, col: &[u32], height: i32, x: i32, z: i32, want_src: bool) -> (i32, i32) {
    let at = |y: i32| col[((y << 8) | (z << 4) | x) as usize];
    let mut y = height - 1;
    while y >= 0 && t.neutral(at(y)) {
        y -= 1;
    }
    if y < 0 {
        return (0, i32::MIN);
    }
    let top = y + 1;
    if !want_src {
        return (top, i32::MIN);
    }
    let mut above = if top < height { at(top) } else { 0 };
    for y in (0..top).rev() {
        let below = at(y);
        if t.damps(below) || t.occluded(above, below, 0) {
            return (top, y + 1);
        }
        above = below;
    }
    (top, i32::MIN)
}

/// Buffers of the new paths, kept per thread with `FULLLAYOUT`.
#[derive(Default)]
struct Bufs {
    level: Vec<u8>,
    src: Vec<i32>,
    buckets: [Vec<u32>; 16],
    /// Cell properties of the property grid (`grid`).
    props: Vec<u16>,
}

impl Bufs {
    fn bytes(&self) -> usize {
        self.level.capacity()
            + self.src.capacity() * 4
            + self.props.capacity() * 2
            + self.buckets.iter().map(|b| b.capacity() * 4).sum::<usize>()
    }
}

thread_local! {
    static BUFS: RefCell<Bufs> = RefCell::default();
}

/// Deliberate errors of the tests: buffers grown by the default doubling, and queues kept even
/// when they take the kept buffers over their maximum (both throw the kept buffers away).
pub(crate) const SABOTAGE_BUFFERS_GROW_BY_DOUBLING: u8 = 22;
pub(crate) const SABOTAGE_QUEUES_KEPT_OVER_THE_MAX: u8 = 23;

/// `v` cleared and refilled with `len` copies of `value` (see [`reserve_exact_len`]).
fn reset_exact<T: Clone>(v: &mut Vec<T>, len: usize, value: T) {
    v.clear();
    reserve_exact_len(v, len);
    v.resize(len, value);
}

/// Capacity of `v` raised to exactly `len` when below. A kept buffer grown by the default doubling
/// could reach twice the zone it serves: the 3 × 3 buffers of a world 384 high went from 2.9 MB to
/// 5.6 MB after a zone cut short by the void skip, over the kept maximum, and were thrown away.
fn reserve_exact_len<T>(v: &mut Vec<T>, len: usize) {
    if !sabotaged(SABOTAGE_BUFFERS_GROW_BY_DOUBLING) {
        v.reserve_exact(len.saturating_sub(v.len()));
    }
}

/// `f` with this thread's kept buffers (`FULLLAYOUT`, given back after, see [`give_back`]) or new
/// ones. A zone expected to need more than [`FullOpts::max_kept_buffer_bytes`] (`expected_bytes`,
/// see [`grid::zone_buffer_bytes`]) gets new buffers and leaves the kept ones in place, so that a
/// large batch tile does not throw away the buffers the next 3 × 3 lighting on this thread reuses.
fn with_bufs<R>(opts: FullOpts, expected_bytes: usize, f: impl FnOnce(&mut Bufs) -> R) -> R {
    let keep = opts.layout && expected_bytes <= opts.max_kept_buffer_bytes;
    let mut b = if keep { BUFS.with(|c| std::mem::take(&mut *c.borrow_mut())) } else { Bufs::default() };
    let out = f(&mut b);
    if keep {
        give_back(b, opts.max_kept_buffer_bytes);
    }
    out
}

/// Keeps `b` for this thread's next zone within `max` bytes. The zone-sized buffers fit, being
/// grown exactly ([`reserve_exact_len`]) for zones expected under `max`; the queues, whose size
/// follows the light rather than the zone, are released first when the whole goes over, so the
/// rest is not thrown away with them.
fn give_back(mut b: Bufs, max: usize) {
    use std::sync::atomic::Ordering::Relaxed;
    if b.bytes() > max && !sabotaged(SABOTAGE_QUEUES_KEPT_OVER_THE_MAX) {
        b.buckets = Default::default();
        crate::stats::KEPT_QUEUES_RELEASED.fetch_add(1, Relaxed);
    }
    if b.bytes() > max {
        crate::stats::KEPT_BUFFERS_DROPPED.fetch_add(1, Relaxed);
        return;
    }
    BUFS.with(|c| *c.borrow_mut() = b);
}

/// Bytes of the buffers this thread keeps (tests).
#[cfg(test)]
fn kept_buffer_bytes() -> usize {
    BUFS.with(|c| c.borrow().bytes())
}

/// New paths (see the header): the property grid when on and possible, otherwise
/// [`light_chunk_fast`].
pub(crate) fn light_chunk_opt(
    t: &LightTables,
    cols: Columns,
    scans: Scans,
    height: i32,
    has_sky: bool,
    opts: FullOpts,
) -> LightSheets {
    with_bufs(opts, grid::zone_buffer_bytes(3, 3, height), |b| {
        if opts.property_grid && t.has_property_grid() {
            let zc = grid::ZoneColumns { chunks_x: 3, chunks_z: 3, columns: &cols, scans: &scans };
            grid::light_zone(t, &zc, &[[1, 1]], height, has_sky, opts, b).pop().expect("one target")
        } else {
            light_chunk_fast(t, cols, scans, height, has_sky, opts, b)
        }
    })
}

/// Path of the switches `FULLSKIP`, `FULLNOEMIT` and `FULLLAYOUT` (see the header).
fn light_chunk_fast(
    t: &LightTables,
    cols: Columns,
    scans: Scans,
    height: i32,
    has_sky: bool,
    opts: FullOpts,
    b: &mut Bufs,
) -> LightSheets {
    let sections = (height / 16) as usize;
    // Stacks: non-neutral top (void skip) and source height (sky), one pass each.
    reset_exact(&mut b.src, (W * W) as usize, i32::MAX);
    let mut ymax = 0;
    if has_sky || opts.skip {
        for (ci, col) in cols.iter().enumerate() {
            let Some(col) = col else { continue };
            let (ox, oz) = ((ci % 3) as i32 * 16, (ci / 3) as i32 * 16);
            for z in 0..16 {
                for x in 0..16 {
                    let (h, s) = match scans[ci] {
                        Some(sc) => (sc.top, sc.src[(z * 16 + x) as usize]),
                        None => stack(t, col, height, x, z, has_sky),
                    };
                    ymax = ymax.max(h);
                    b.src[((oz + z) * W + ox + x) as usize] = s.max(0);
                }
            }
        }
    }
    let top = if opts.skip && cols[4].is_some() { height.min((ymax + 30) & !15) } else { height };
    let g = Grid { cols, height: top };
    let n = (W * W * top) as usize;
    crate::stats::FULL_CELLS.fetch_add(n as u64, std::sync::atomic::Ordering::Relaxed);
    reset_exact(&mut b.level, n, 0);
    for k in &mut b.buckets {
        k.clear();
    }
    // Block light: emission sources under `top`.
    let level = &mut b.level;
    let buckets = &mut b.buckets;
    for (ci, col) in cols.iter().enumerate() {
        let Some(col) = col else { continue };
        let (ox, oz) = ((ci % 3) as i32 * 16, (ci / 3) as i32 * 16);
        let scan = if opts.noemit { scans[ci] } else { None };
        let mut push = |x: i32, y: i32, z: i32, s: u32| {
            let e = t.emission.get(s as usize).copied().unwrap_or(0);
            if e > 0 {
                let i = ((y * W + oz + z) * W + ox + x) as usize;
                level[i] = e;
                buckets[e as usize].push(i as u32);
            }
        };
        for sy in 0..top / 16 {
            if scan.is_some_and(|sc| !sc.section_has_emitter(sy)) {
                continue;
            }
            if opts.layout {
                for y in sy * 16..sy * 16 + 16 {
                    let row = &col[(y << 8) as usize..((y + 1) << 8) as usize];
                    for (k, &s) in row.iter().enumerate() {
                        push((k & 15) as i32, y, (k >> 4) as i32, s);
                    }
                }
            } else {
                for z in 0..16 {
                    for x in 0..16 {
                        for y in sy * 16..sy * 16 + 16 {
                            push(x, y, z, col[((y << 8) | (z << 4) | x) as usize]);
                        }
                    }
                }
            }
        }
    }
    propagate(&g, t, level, buckets);
    // Central chunk: row `(y, z)` starts at cell (16, y, 16 + z) of the 48 × 48 layout.
    let row_start = |y: i32, z: usize| ((y * W + z as i32 + 16) * W + 16) as usize;
    let block = pack_sheets_below_top(level, sections, 0..top, row_start, 0, false, opts.intern_uniform);
    if !has_sky {
        return (vec![None; sections + 2], block);
    }
    level.fill(0);
    let src = &b.src;
    for z in 0..W {
        for x in 0..W {
            let from = src[(z * W + x) as usize];
            if from == i32::MAX {
                continue;
            }
            for y in from..top {
                level[g.index(x, y, z)] = 15;
            }
            let mut highest = from;
            for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                let (nx, nz) = (x + dx, z + dz);
                if (0..W).contains(&nx) && (0..W).contains(&nz) {
                    let s = src[(nz * W + nx) as usize];
                    if s != i32::MAX {
                        highest = highest.max(s);
                    }
                }
            }
            for y in from..=highest.min(top - 1) {
                buckets[15].push(g.index(x, y, z) as u32);
            }
        }
    }
    propagate(&g, t, level, buckets);
    let sky = pack_sheets_below_top(level, sections, 0..top, row_start, 0xff, true, opts.intern_uniform);
    (sky, block)
}

/// Like [`pack_sheets`] on only the sections under `rows.end` (the others are omitted: full sky or
/// zero block, see the header), two cells per byte; the sections under `rows.start` (a multiple of
/// 16) are all 0 and not read. `row_start(y, z)`: index in `level` of the 16 cells of row `z`
/// (0 to 15) at height `y` of the chunk.
fn pack_sheets_below_top(
    level: &[u8],
    sections: usize,
    rows: std::ops::Range<i32>,
    row_start: impl Fn(i32, usize) -> usize,
    empty_byte: u8,
    drop_only_top_empty: bool,
    intern_uniform: bool,
) -> Vec<LightLayer> {
    let mut out: Vec<LightLayer> = vec![None; sections + 2];
    let mut keep = !drop_only_top_empty;
    let first_read = (rows.start / 16) as usize;
    for s in (0..(rows.end / 16) as usize).rev() {
        let (a, or, and) = if s < first_read { ([0u8; LIGHT_BYTES], 0, 0) } else { pack_section(level, s, &row_start) };
        if sheet_kept(or, and, empty_byte, drop_only_top_empty, &mut keep) {
            out[s + 1] = Some(stored_sheet(a, or, and, intern_uniform));
        }
    }
    out
}

/// Sheet of section `s` packed two cells per byte, with the OR and AND of its bytes, gathered
/// while packing: they tell an empty sheet and a uniform one without a second walk over its 2 KB.
fn pack_section(level: &[u8], s: usize, row_start: &impl Fn(i32, usize) -> usize) -> ([u8; LIGHT_BYTES], u8, u8) {
    let mut a = [0u8; LIGHT_BYTES];
    let (mut or, mut and) = (0u8, 0xffu8);
    for ly in 0..16usize {
        let y = (s * 16 + ly) as i32;
        for z in 0..16usize {
            let base = row_start(y, z);
            let row = &level[base..base + 16];
            for xx in 0..8 {
                let v = row[2 * xx] | row[2 * xx + 1] << 4;
                a[(ly << 7) | (z << 3) | xx] = v;
                or |= v;
                and &= v;
            }
        }
    }
    (a, or, and)
}

/// Sheets of the central chunk. `drop_only_top_empty`: "empty" sheets (all `empty_byte`)
/// removed only at the top (sky); otherwise removed everywhere (block).
fn pack_sheets(
    level: &[u8],
    sections: usize,
    empty_byte: u8,
    drop_only_top_empty: bool,
    intern_uniform: bool,
) -> Vec<LightLayer> {
    let mut out: Vec<LightLayer> = vec![None; sections + 2];
    let mut keep = !drop_only_top_empty;
    for s in (0..sections).rev() {
        let mut a = [0u8; LIGHT_BYTES];
        for ly in 0..16usize {
            let y = (s * 16 + ly) as i32;
            for z in 0..16usize {
                for x in 0..16usize {
                    let v = level[((y * W + z as i32 + 16) * W + x as i32 + 16) as usize];
                    let i = (ly << 8) | (z << 4) | x;
                    a[i >> 1] |= v << ((i & 1) * 4);
                }
            }
        }
        let (or, and) = a.iter().fold((0u8, 0xffu8), |(or, and), &v| (or | v, and & v));
        if sheet_kept(or, and, empty_byte, drop_only_top_empty, &mut keep) {
            out[s + 1] = Some(stored_sheet(a, or, and, intern_uniform));
        }
    }
    out
}

/// Whether a sheet (OR and AND of its bytes), visited from the top down, is kept: an empty one
/// (all `empty_byte`) is dropped everywhere, or only above the highest non-empty one
/// (`drop_only_top_empty`, which `keep` tracks).
fn sheet_kept(or: u8, and: u8, empty_byte: u8, drop_only_top_empty: bool, keep: &mut bool) -> bool {
    let is_empty = or == empty_byte && and == empty_byte;
    if drop_only_top_empty {
        *keep |= !is_empty;
        *keep
    } else {
        !is_empty
    }
}

/// Heap sheet for the packed bytes `a` (OR and AND of them given). A uniform sheet (zero sky
/// under the terrain, full sky under an overhang) becomes the shared one: thousands of chunks
/// then hold one copy, and a writer copies it first (`Arc::make_mut`).
fn stored_sheet(a: [u8; LIGHT_BYTES], or: u8, and: u8, intern_uniform: bool) -> Arc<[u8; LIGHT_BYTES]> {
    let shared = if intern_uniform { rc_world::shared_layer_from_or_and(or, and) } else { None };
    shared.unwrap_or_else(|| Arc::new(a))
}

#[cfg(test)]
mod tests;
