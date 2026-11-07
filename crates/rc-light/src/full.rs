//! Full light of a new chunk (`light` generation status): resolution from
//! zero over the 3×3 chunk zone around it, same rule as [`crate::relight`] (emission
//! sources, sky columns above their source height, propagation by decreasing levels,
//! occluding faces, minimum attenuation 1). Only the central chunk is returned:
//! light coming from beyond the zone cannot reach it (16 cells or more).
//!
//! New path (mission C2-M08, `docs/perf.md`), same result byte for byte as
//! [`light_chunk_inner`] (kept as is, taken when the three switches are at 0; the fourth,
//! `RUSTCRAFT_OPT_INTERN_UNIFORM_LIGHT`, only shares the uniform sheets of either path):
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

use std::cell::RefCell;
use std::sync::{Arc, OnceLock};

use rc_world::{LIGHT_BYTES, LightLayer};

use crate::{LightTables, STEP};

const W: i32 = 48;

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
    fn state(&self, x: i32, y: i32, z: i32) -> Option<u32> {
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
                let s = g.state(x, y, z).unwrap_or(0);
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
    let mut hi = 0u32;
    for y in (0..g.height).rev() {
        let lo = g.state(x, y, z).unwrap_or(0);
        let damp = t.dampening.get(lo as usize).copied().unwrap_or(0);
        if damp != 0 || t.occluded(hi, lo, 0) {
            return y + 1;
        }
        hi = lo;
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
            let Some(si) = g.state(x, y, z) else { continue };
            for (dir, s) in STEP.iter().enumerate() {
                let (qx, qy, qz) = (x + s[0], y + s[1], z + s[2]);
                if !(0..W).contains(&qx) || !(0..W).contains(&qz) || !(0..g.height).contains(&qy) {
                    continue;
                }
                let Some(sj) = g.state(qx, qy, qz) else { continue };
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

/// Switches of the new path (see the header), read once per process.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FullOpts {
    pub skip: bool,
    pub noemit: bool,
    pub layout: bool,
    /// All-0 and all-15 sheets replaced by the shared ones ([`crate::uniform_light_interned`]).
    pub intern_uniform: bool,
}

impl FullOpts {
    /// Every switch off (tests: the old path).
    #[cfg(test)]
    pub(crate) const OLD: FullOpts = FullOpts { skip: false, noemit: false, layout: false, intern_uniform: false };

    pub(crate) fn current() -> FullOpts {
        static O: OnceLock<FullOpts> = OnceLock::new();
        let on = |v: Option<&str>| v.map(str::trim) != Some("0");
        *O.get_or_init(|| FullOpts {
            skip: on(rc_data::env_str!("RUSTCRAFT_OPT_FULLSKIP")),
            noemit: on(rc_data::env_str!("RUSTCRAFT_OPT_FULLNOEMIT")),
            layout: on(rc_data::env_str!("RUSTCRAFT_OPT_FULLLAYOUT")),
            intern_uniform: crate::uniform_light_interned(),
        })
    }

    /// None of the three resolution switches: [`light_chunk_inner`] resolves.
    pub(crate) fn old_resolution(self) -> bool {
        !self.skip && !self.noemit && !self.layout
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
    let mut hi = if top < height { at(top) } else { 0 };
    for y in (0..top).rev() {
        let lo = at(y);
        if t.damps(lo) || t.occluded(hi, lo, 0) {
            return (top, y + 1);
        }
        hi = lo;
    }
    (top, i32::MIN)
}

/// Buffers of the new path, kept per thread with `FULLLAYOUT`.
#[derive(Default)]
struct Bufs {
    level: Vec<u8>,
    src: Vec<i32>,
    buckets: [Vec<u32>; 16],
}

impl Bufs {
    fn bytes(&self) -> usize {
        self.level.capacity() + self.src.capacity() * 4 + self.buckets.iter().map(|b| b.capacity() * 4).sum::<usize>()
    }
}

/// Bytes kept at most per thread (world 384 high: ≈ 0.9 MiB of levels and a few
/// hundred KiB of buckets; beyond that, everything is returned after the call).
const KEEP_BYTES: usize = 4 << 20;

thread_local! {
    static BUFS: RefCell<Bufs> = RefCell::default();
}

/// New path (see the header).
pub(crate) fn light_chunk_opt(
    t: &LightTables,
    cols: Columns,
    scans: Scans,
    height: i32,
    has_sky: bool,
    o: FullOpts,
) -> (Vec<LightLayer>, Vec<LightLayer>) {
    let mut b = if o.layout { BUFS.with(|c| std::mem::take(&mut *c.borrow_mut())) } else { Bufs::default() };
    let out = run(t, cols, scans, height, has_sky, o, &mut b);
    if o.layout && b.bytes() <= KEEP_BYTES {
        BUFS.with(|c| *c.borrow_mut() = b);
    }
    out
}

fn run(
    t: &LightTables,
    cols: Columns,
    scans: Scans,
    height: i32,
    has_sky: bool,
    o: FullOpts,
    b: &mut Bufs,
) -> (Vec<LightLayer>, Vec<LightLayer>) {
    let sections = (height / 16) as usize;
    // Stacks: non-neutral top (void skip) and source height (sky), one pass each.
    b.src.clear();
    b.src.resize((W * W) as usize, i32::MAX);
    let mut ymax = 0;
    if has_sky || o.skip {
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
    let top = if o.skip && cols[4].is_some() { height.min((ymax + 30) & !15) } else { height };
    let g = Grid { cols, height: top };
    let n = (W * W * top) as usize;
    b.level.clear();
    b.level.resize(n, 0);
    for k in &mut b.buckets {
        k.clear();
    }
    // Block light: emission sources under `top`.
    let level = &mut b.level;
    let buckets = &mut b.buckets;
    for (ci, col) in cols.iter().enumerate() {
        let Some(col) = col else { continue };
        let (ox, oz) = ((ci % 3) as i32 * 16, (ci / 3) as i32 * 16);
        let scan = if o.noemit { scans[ci] } else { None };
        let mut push = |x: i32, y: i32, z: i32, s: u32| {
            let e = t.emission.get(s as usize).copied().unwrap_or(0);
            if e > 0 {
                let i = ((y * W + oz + z) * W + ox + x) as usize;
                level[i] = e;
                buckets[e as usize].push(i as u32);
            }
        };
        for sy in 0..top / 16 {
            let su = sy as usize;
            if let Some(sc) = scan
                && su < 256
                && sc.emit[su >> 6] >> (su & 63) & 1 == 0
            {
                continue;
            }
            if o.layout {
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
    let block = pack_sheets_below_top(level, sections, top, 0, false, o.intern_uniform);
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
            let mut hi = from;
            for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                let (nx, nz) = (x + dx, z + dz);
                if (0..W).contains(&nx) && (0..W).contains(&nz) {
                    let o = src[(nz * W + nx) as usize];
                    if o != i32::MAX {
                        hi = hi.max(o);
                    }
                }
            }
            for y in from..=hi.min(top - 1) {
                buckets[15].push(g.index(x, y, z) as u32);
            }
        }
    }
    propagate(&g, t, level, buckets);
    let sky = pack_sheets_below_top(level, sections, top, 0xff, true, o.intern_uniform);
    (sky, block)
}

/// Like [`pack_sheets`] on only the sections under `top` (the others are omitted: full sky or
/// zero block, see the header), two cells per byte.
fn pack_sheets_below_top(
    level: &[u8],
    sections: usize,
    top: i32,
    empty_byte: u8,
    drop_only_top_empty: bool,
    intern_uniform: bool,
) -> Vec<LightLayer> {
    let mut out: Vec<LightLayer> = vec![None; sections + 2];
    let mut keep = !drop_only_top_empty;
    for s in (0..(top / 16) as usize).rev() {
        let mut a = [0u8; LIGHT_BYTES];
        // OR and AND of the packed bytes, gathered while packing: they tell an empty sheet and
        // a uniform one without a second walk over its 2 KB.
        let (mut or, mut and) = (0u8, 0xffu8);
        for ly in 0..16usize {
            let y = (s * 16 + ly) as i32;
            for z in 0..16usize {
                let base = ((y * W + z as i32 + 16) * W + 16) as usize;
                let row = &level[base..base + 16];
                for xx in 0..8 {
                    let v = row[2 * xx] | row[2 * xx + 1] << 4;
                    a[(ly << 7) | (z << 3) | xx] = v;
                    or |= v;
                    and &= v;
                }
            }
        }
        if sheet_kept(or, and, empty_byte, drop_only_top_empty, &mut keep) {
            out[s + 1] = Some(stored_sheet(a, or, and, intern_uniform));
        }
    }
    out
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
