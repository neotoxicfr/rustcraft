//! Incremental relight (mission C2-M14, `docs/perf.md`): instead of recomputing the whole zone
//! of a group from its sources, the light is first removed around the changed cells
//! (decrease), then restored from the cells that stayed lit and the sources (increase), in the way of the
//! game's engine. Cost proportional to the cells whose light depended on the changes, not
//! to the entire zone (33 × 31 × 33 cells at least for the block).
//!
//! Same result, bit for bit, as the zone (`solve` in `fast.rs`) when the sheets read are
//! consistent (fixed point held before the call):
//! - bounds: the group's zone, margin included; the margin stays frozen and serves as a seed as
//!   in the zone;
//! - a group whose zone crosses that of another group of the same call (same sheet) goes through the
//!   zone: the zone of a group can leave a stale light coming from its margin when another
//!   group changes that margin, and only the zone reproduces it;
//! - decrease: a lit neighbour darker than the emptied cell is emptied in turn (game rule:
//!   level − 1, with no state or occlusion read, which sometimes empties too much: the increase
//!   then restores the right value); a neighbour at least as bright sends its light back to the cell;
//! - sources: emission of the emptied cells; sky, columns of the changes walked from top to bottom
//!   (source according to the states versus 15 stored);
//! - increase: by buckets of decreasing level (each cell receives its final value once),
//!   remaining directions kept in the entry;
//! - writing: only the cells whose final value differs from the value read.
//!
//! Inconsistent sheets (official game light, whose rules differ slightly from ours;
//! absent sheets filled by the server before the relight; emitters stored under their
//! emission): the zone repairs its whole box, the incremental only touches what depends on the
//! changes, like the game's engine. The result is then no longer bit for bit on this data
//! (real traces: 20 relights out of 400 of the synthetic suite, 2 out of 100 of the Nether, at equal
//! input): switch OFF by default until the user has decided.
//!
//! Switches: `RUSTCRAFT_OPT_LIGHTINC=1` (absent or `0`: zone everywhere),
//! `RUSTCRAFT_OPT_LIGHTINCMAX` (groups of more than N changes resolved by the zone; absent:
//! no limit).

use super::*;
use rc_world::LightLayer;

/// Interior cell with a sheet: emptied, raised and written.
const OPEN: u8 = 1;
/// Frozen margin cell that lights (sheet present, or full sky above the world).
const SEED: u8 = 2;
/// Cell removed at entry (change or changed sky source).
const DIRTY: u8 = 4;

/// Buffers kept per thread (in `Work`): table of the cells read, queues, differences.
#[derive(Default)]
pub(super) struct Buf {
    /// Cell read in the current resolution when `stamp == epoch`.
    stamp: Vec<u32>,
    epoch: u32,
    lvl: Vec<u8>,
    flag: Vec<u8>,
    state: Vec<u32>,
    /// Decreases: cell (32 bits), level (4 bits), directions (6 bits).
    dec: Vec<u64>,
    /// Increases per level: cell (32 bits), directions (6 bits).
    buckets: [Vec<u64>; 16],
    /// Interior cells read and their stored value.
    seen: Vec<(u32, u8)>,
    reseed: Vec<(u32, u8)>,
    dirty: Vec<u32>,
    /// Height from which the sky descends unobstructed, per stack (states fixed during the call).
    floors: HashMap<(i32, i32), i32>,
    changed: Vec<(u32, i32, u8)>,
    column: Vec<(i32, u8)>,
}

impl Buf {
    pub(super) fn bytes(&self) -> usize {
        self.stamp.capacity() * 4
            + self.lvl.capacity()
            + self.flag.capacity()
            + self.state.capacity() * 4
            + self.dec.capacity() * 8
            + self.buckets.iter().map(|b| b.capacity() * 8).sum::<usize>()
            + self.seen.capacity() * 8
            + self.reseed.capacity() * 8
            + self.dirty.capacity() * 4
            + self.floors.capacity() * 16
            + self.changed.capacity() * 12
            + self.column.capacity() * 8
    }

    fn prepare(&mut self, n: usize) {
        if self.stamp.len() < n {
            self.stamp.resize(n, 0);
            self.lvl.resize(n, 0);
            self.flag.resize(n, 0);
            self.state.resize(n, 0);
        }
        self.epoch = self.epoch.wrapping_add(1);
        if self.epoch == 0 {
            self.stamp.fill(0);
            self.epoch = 1;
        }
        self.dec.clear();
        for b in &mut self.buckets {
            b.clear();
        }
        self.seen.clear();
        self.reseed.clear();
        self.dirty.clear();
    }
}

/// Resolution `(changes, block, sky)`.
type Unit = (Vec<[i32; 3]>, bool, bool);

/// Inclusive bounds of a zone (margin included).
fn bounds(g: Geo) -> ([i32; 3], [i32; 3]) {
    (g.lo, [g.lo[0] + g.size[0] - 1, g.lo[1] + g.size[1] - 1, g.lo[2] + g.size[2] - 1])
}

fn cross(a: Geo, b: Geo) -> bool {
    let ((alo, ahi), (blo, bhi)) = (bounds(a), bounds(b));
    (0..3).all(|k| alo[k] <= bhi[k] && blo[k] <= ahi[k])
}

/// Zones disjoint from all the others: incremental resolution allowed.
fn alone(zones: &[Option<Geo>]) -> Vec<bool> {
    if sabotage(22) {
        return zones.iter().map(Option::is_some).collect();
    }
    zones
        .iter()
        .enumerate()
        .map(|(k, z)| {
            z.is_some_and(|a| zones.iter().enumerate().all(|(j, o)| j == k || !o.is_some_and(|b| cross(a, b))))
        })
        .collect()
}

/// Resolves incrementally the sheets of the isolated groups and removes them from `units`: only
/// what goes through the zone remains there, which the caller resolves by its own path (parallel groups
/// included). An isolated sheet crosses, margin included, no zone of the same sheet: resolving
/// it before the others changes nothing.
pub(super) fn run(grid: &mut impl LightGrid, c: &Ctx, units: &mut Vec<Unit>, out: &mut [Touched]) {
    let n = units.len();
    let small = |cs: &[[i32; 3]]| cs.len() <= c.o.inc_max;
    // Block zones of all the groups (a min and a max each): those of groups that are too large
    // count too, they may cross a small group.
    let bz: Vec<Geo> = units.iter().map(|(cs, _, _)| block_zone(c, cs)).collect();
    let of = |want: fn(&Unit) -> bool| -> Vec<Option<Geo>> { units.iter().zip(&bz).map(|(u, z)| want(u).then_some(*z)).collect() };
    let ib: Vec<bool> = alone(&of(|u| u.1)).iter().zip(&*units).map(|(a, (cs, b, _))| *a && *b && small(cs)).collect();
    // The sky zone contains the block one (same x, z and top, lower bottom): a group whose
    // block zone crosses that of another sky group is not isolated, with no further computation
    // (sabotage 26: this sort removed).
    let cand: Vec<bool> =
        alone(&of(|u| u.2)).iter().zip(&*units).map(|(a, (cs, _, s))| (*a || sabotage(26)) && *s && small(cs)).collect();
    with_work(c.o.zone_buf, |w| {
        w.aux.memo.clear();
        w.inc.floors.clear();
        // Sky zone computed once, for the candidates only, then for the other sky groups
        // whose wide box (down to the bottom of the world) crosses that of a candidate.
        let mut sz: Vec<Option<(Geo, i32)>> = vec![None; n];
        for k in (0..n).filter(|&k| cand[k]) {
            sz[k] = Some(sky_zone(&*grid, c, &mut w.aux, &units[k].0, bz[k]));
        }
        let wide = |z: Geo| Geo::new([z.lo[0], -1, z.lo[2]], [z.size[0], z.lo[1] + z.size[1] + 1, z.size[2]]);
        let mut sky: Vec<Option<Geo>> = vec![None; n];
        for k in (0..n).filter(|&k| units[k].2) {
            sky[k] = Some(match sz[k] {
                Some((g, _)) => g,
                None if (0..n).any(|j| sz[j].is_some_and(|(g, _)| cross(wide(bz[k]), g))) => {
                    sky_zone(&*grid, c, &mut w.aux, &units[k].0, bz[k]).0
                }
                None => wide(bz[k]),
            });
        }
        let is: Vec<bool> = alone(&sky).iter().zip(&cand).map(|(a, k)| *a && *k).collect();
        for k in 0..n {
            let cs = &units[k].0;
            if ib[k] {
                solve(grid, c, w, bz[k], cs, false, out);
            }
            if is[k] {
                solve(grid, c, w, sz[k].expect("sky zone").0, cs, true, out);
            }
        }
        let mut k = 0;
        units.retain_mut(|u| {
            u.1 &= !ib[k];
            u.2 &= !is[k];
            k += 1;
            u.1 || u.2
        });
        // Sabotage 24: old scheduling, the rest resolved here in series as soon as a group is
        // isolated (parallel groups lost). Same result; only the count of paths sees it.
        if sabotage(24) && ib.iter().chain(&is).any(|&x| x) {
            for (cs, b, s) in units.drain(..) {
                solve_group(grid, c, w, &cs, b, s, out);
            }
        }
    });
}

/// Columns of the zone, read once: sections and sheets of the sort being resolved.
struct Win<'a> {
    g: Geo,
    h: i32,
    sky: bool,
    cx0: i32,
    cz0: i32,
    ncx: i32,
    cols: Vec<Option<(&'a [Section], &'a [LightLayer])>>,
}

impl<'a> Win<'a> {
    fn new(grid: &'a impl LightGrid, g: Geo, h: i32, sky: bool) -> Self {
        let (cx0, cz0) = (g.lo[0] >> 4, g.lo[2] >> 4);
        let (cx1, cz1) = ((g.lo[0] + g.size[0] - 1) >> 4, (g.lo[2] + g.size[2] - 1) >> 4);
        let mut cols = Vec::with_capacity(((cx1 - cx0 + 1) * (cz1 - cz0 + 1)) as usize);
        for cz in cz0..=cz1 {
            for cx in cx0..=cx1 {
                cols.push(grid.column(cx, cz).map(|(s, k, b)| (s, if sky { k } else { b })));
            }
        }
        Win { g, h, sky, cx0, cz0, ncx: cx1 - cx0 + 1, cols }
    }

    /// Coordinates in the zone (−1 and `size`: outer ring) of an index.
    #[inline]
    fn coords(&self, i: usize) -> [i32; 3] {
        let g = self.g;
        let (ry, rem) = (i / g.plane, i % g.plane);
        [(rem % g.ax) as i32 - g.ring, ry as i32 - g.ring, (rem / g.ax) as i32 - g.ring]
    }
}

/// Reads cell `i` on its first pass: state, stored value, role.
#[inline]
fn load(b: &mut Buf, w: &Win, i: usize) {
    if b.stamp[i] == b.epoch {
        return;
    }
    b.stamp[i] = b.epoch;
    let (mut s, mut v, mut f) = (0u32, 0u8, 0u8);
    let d = w.coords(i);
    let g = w.g;
    if (0..3).all(|a| (0..g.size[a]).contains(&d[a])) {
        let (x, y, z) = (g.lo[0] + d[0], g.lo[1] + d[1], g.lo[2] + d[2]);
        let col = w.cols[(((z >> 4) - w.cz0) * w.ncx + ((x >> 4) - w.cx0)) as usize];
        if let Some((sections, layers)) = col {
            if (0..w.h).contains(&y) {
                let (lx, ly, lz) = ((x & 15) as usize, (y & 15) as usize, (z & 15) as usize);
                s = sections.get((y >> 4) as usize).map_or(0, |sec| sec.blocks.get(rc_world::block_index(lx, ly, lz)));
                if let Some(Some(l)) = layers.get(((y >> 4) + 1) as usize) {
                    v = nibble(l, lx, ly, lz);
                    let inner = (0..3).all(|a| (1..g.size[a] - 1).contains(&d[a]));
                    if inner {
                        f = OPEN;
                        b.seen.push((i as u32, v));
                    } else {
                        f = SEED;
                    }
                }
            } else if y >= w.h && w.sky {
                // Above the world: full sky, frozen seed.
                v = 15;
                f = SEED;
            }
        }
    }
    b.state[i] = s;
    b.lvl[i] = v;
    b.flag[i] = f;
}

/// Smallest height from which the sky descends unobstructed from the top of the world in stack
/// `(x, z)` (same rule as `clear_above` and `source_height`); `i32::MIN`: all the way down.
fn floor_of(grid: &impl LightGrid, c: &Ctx, aux: &mut Aux, floors: &mut HashMap<(i32, i32), i32>, x: i32, z: i32) -> i32 {
    if let Some(&f) = floors.get(&(x, z)) {
        return f;
    }
    let (t, h, tab) = (c.t, c.height, c.o.occ_tab);
    let (cx, cz) = (x >> 4, z >> 4);
    let f = match grid.column(cx, cz) {
        None => i32::MIN,
        Some((sections, _, _)) => {
            let (lx, lz) = ((x & 15) as usize, (z & 15) as usize);
            let (mut y, mut hf) = (h - 1, t.face(0));
            loop {
                if y < 0 {
                    break i32::MIN;
                }
                if c.o.sky_skip && sec_clear(aux, t, cx, cz, Some(sections), y >> 4) {
                    if t.occluded_faces(hf, 0, 0, tab) {
                        break y + 1;
                    }
                    hf = 0;
                    y = (y & !15) - 1;
                    continue;
                }
                let s = state_in(Some(sections), lx, lz, y, h);
                if t.damps(s) || t.occluded_faces(hf, t.face(s), 0, tab) {
                    break y + 1;
                }
                hf = t.face(s);
                y -= 1;
            }
        }
    };
    floors.insert((x, z), f);
    f
}

/// An incremental resolution (one sheet) on zone `zone` of group `cs`.
fn solve(grid: &mut impl LightGrid, c: &Ctx, w: &mut Work, zone: Geo, cs: &[[i32; 3]], sky: bool, out: &mut [Touched]) {
    let t0 = std::time::Instant::now();
    // Outer ring always present: neighbours of a margin cell without bounds tests.
    let (ax, az) = ((zone.size[0] + 2) as usize, (zone.size[2] + 2) as usize);
    let g = Geo { lo: zone.lo, size: zone.size, ring: 1, ax, az, plane: ax * az };
    let Work { aux, inc: b, .. } = w;
    b.prepare(g.cells());
    {
        let win = Win::new(&*grid, g, c.height, sky);
        compute(&*grid, c, aux, b, &win, cs);
    }
    write(grid, b, g, sky, out);
    count_solve(sky, t0.elapsed());
    #[cfg(test)]
    SOLVES.with(|s| s.set(s.get() + 1));
}

#[cfg(test)]
thread_local! {
    /// Incremental resolutions done on this thread (the proofs verify that they take place).
    pub(super) static SOLVES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

fn compute(grid: &impl LightGrid, c: &Ctx, aux: &mut Aux, b: &mut Buf, win: &Win, cs: &[[i32; 3]]) {
    let (t, g) = (c.t, win.g);
    let inner = |p: [i32; 3]| (0..3).all(|a| (1..g.size[a] - 1).contains(&(p[a] - g.lo[a])));
    let idx = |p: [i32; 3]| g.at(p[0] - g.lo[0], p[1] - g.lo[1], p[2] - g.lo[2]);
    let mark = |b: &mut Buf, i: usize| {
        if b.flag[i] & (OPEN | DIRTY) == OPEN {
            b.flag[i] |= DIRTY;
            b.dirty.push(i as u32);
        }
    };
    for &p in cs {
        if inner(p) {
            let i = idx(p);
            load(b, win, i);
            mark(b, i);
        }
    }
    // Sky: under each change, cells whose source quality changed (15 stored versus
    // sky clear according to the states), up to the first dark cell on both sides.
    if win.sky && !sabotage(20) {
        let (ybot, ytop) = (g.lo[1] + 1, g.lo[1] + g.size[1] - 2);
        for (k, &p) in cs.iter().enumerate() {
            if !inner([p[0], ybot, p[2]]) || cs[..k].iter().any(|q| q[0] == p[0] && q[2] == p[2]) {
                continue;
            }
            // From the highest change of the stack.
            let top = if sabotage(23) {
                p[1]
            } else {
                cs[k..].iter().filter(|q| q[0] == p[0] && q[2] == p[2]).map(|q| q[1]).max().unwrap_or(p[1])
            };
            let floor = floor_of(grid, c, aux, &mut b.floors, p[0], p[2]);
            for y in (ybot..=top.min(ytop)).rev() {
                let i = idx([p[0], y, p[2]]);
                load(b, win, i);
                if b.flag[i] & OPEN == 0 {
                    continue;
                }
                let (new, old) = (y >= floor, b.lvl[i] == 15);
                if new != old {
                    mark(b, i);
                } else if !new {
                    break;
                }
            }
        }
    }
    // Removed cells: emptied, decrease in the six directions, source restored afterwards.
    for k in 0..b.dirty.len() {
        let i = b.dirty[k] as usize;
        let v = b.lvl[i];
        b.lvl[i] = 0;
        b.dec.push(i as u64 | u64::from(v) << 32 | 63 << 36);
        let src = if win.sky {
            let [dx, dy, dz] = win.coords(i);
            let (x, y, z) = (g.lo[0] + dx, g.lo[1] + dy, g.lo[2] + dz);
            if y >= floor_of(grid, c, aux, &mut b.floors, x, z) { 15 } else { 0 }
        } else {
            t.emission.get(b.state[i] as usize).copied().unwrap_or(0)
        };
        if src > 0 {
            b.reseed.push((i as u32, src));
        }
    }
    let offs: [isize; 6] = [-(g.plane as isize), g.plane as isize, -(g.ax as isize), g.ax as isize, -1, 1];
    let mut k = 0;
    while k < b.dec.len() {
        let e = b.dec[k];
        k += 1;
        let i = (e as u32) as usize;
        let lc = (e >> 32 & 15) as u8;
        let dirs = (e >> 36) as u8;
        for (dir, off) in offs.iter().enumerate() {
            if dirs >> dir & 1 == 0 {
                continue;
            }
            let j = i.wrapping_add_signed(*off);
            load(b, win, j);
            let lj = b.lvl[j];
            if lj == 0 {
                continue;
            }
            let back = 1u64 << OPPOSITE[dir];
            if b.flag[j] & OPEN == 0 {
                // Frozen margin: seed of the increase towards the emptied cell.
                if b.flag[j] & SEED != 0 && lj > 1 {
                    b.buckets[lj as usize].push(j as u64 | back << 32);
                }
                continue;
            }
            if lj < lc {
                b.lvl[j] = 0;
                let e = if win.sky || sabotage(21) { 0 } else { t.emission.get(b.state[j] as usize).copied().unwrap_or(0) };
                // Value explained by its own emission: what it lights stays right.
                if e < lj {
                    b.dec.push(j as u64 | u64::from(lj) << 32 | (63 & !back) << 36);
                }
                if e > 0 {
                    b.reseed.push((j as u32, e));
                }
            } else if lj > 1 {
                b.buckets[lj as usize].push(j as u64 | back << 32);
            }
        }
    }
    for k in 0..b.reseed.len() {
        let (i, s) = b.reseed[k];
        let i = i as usize;
        if s > b.lvl[i] {
            b.lvl[i] = s;
            if s > 1 {
                b.buckets[s as usize].push(i as u64 | 63 << 32);
            }
        }
    }
    let tab = c.o.occ_tab;
    for l in (2..16usize).rev() {
        while let Some(e) = b.buckets[l].pop() {
            let i = (e as u32) as usize;
            if b.lvl[i] as usize != l {
                continue;
            }
            let dirs = (e >> 32) as u8;
            let fi = t.face(b.state[i]);
            for (dir, off) in offs.iter().enumerate() {
                if dirs >> dir & 1 == 0 {
                    continue;
                }
                let j = i.wrapping_add_signed(*off);
                load(b, win, j);
                let lj = b.lvl[j];
                if b.flag[j] & OPEN == 0 || l as u8 - 1 <= lj {
                    continue;
                }
                let sj = b.state[j];
                let nv = l as i32 - i32::from(t.opacity(sj));
                if nv <= i32::from(lj) || t.occluded_faces(fi, t.face(sj), dir, tab) {
                    continue;
                }
                b.lvl[j] = nv as u8;
                if nv > 1 {
                    b.buckets[nv as usize].push(j as u64 | (63 & !(1u64 << OPPOSITE[dir])) << 32);
                }
            }
        }
    }
}

/// Writes only the changed cells, stack by stack from bottom to top, and marks their sections.
fn write(grid: &mut impl LightGrid, b: &mut Buf, g: Geo, sky: bool, out: &mut [Touched]) {
    let k = if sky { out.len() - 1 } else { 0 };
    let touched = &mut out[k];
    b.changed.clear();
    for &(i, orig) in &b.seen {
        let i = i as usize;
        let v = b.lvl[i];
        if v != orig {
            let (ry, rem) = (i / g.plane, i % g.plane);
            b.changed.push((rem as u32, g.lo[1] + ry as i32 - g.ring, v));
        }
    }
    if b.changed.is_empty() {
        return;
    }
    b.changed.sort_unstable_by_key(|&(col, y, _)| (col, y));
    let mut s = 0;
    while s < b.changed.len() {
        let col = b.changed[s].0;
        let mut e = s;
        b.column.clear();
        while e < b.changed.len() && b.changed[e].0 == col {
            b.column.push((b.changed[e].1, b.changed[e].2));
            e += 1;
        }
        let (dx, dz) = ((col as usize % g.ax) as i32 - g.ring, (col as usize / g.ax) as i32 - g.ring);
        apply_column(grid, g.lo[0] + dx, g.lo[2] + dz, &b.column, sky, touched);
        s = e;
    }
}
