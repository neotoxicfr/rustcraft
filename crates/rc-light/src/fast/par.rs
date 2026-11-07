//! Multi-core relight (mission C2-M11, `docs/perf.md`): same result, bit for bit, as the
//! sequential path of `fast.rs`, each approach behind its switch.
//!
//! - `LIGHTJOIN`: the sky and block of a group only read the states (fixed during a
//!   relight) and their own sheet: resolved at the same time on a common zone read, then
//!   written in the usual order (the grid is only written through `&mut`). No effect with
//!   `ZONEONCE=0` (two reads, like the sequential path), likewise for the groups.
//! - `LIGHTPARSOLVE`: in a large zone, sky source heights computed by rows of
//!   stacks in parallel, then propagation by synchronous levels: the cells of a level are
//!   spread across the threads, each neighbour rises through an atomic maximum. The fixed point is
//!   unique (maximum over the paths, each step costs at least 1): same values in any order.
//! - `LIGHTGROUPS`: groups scheduled by levels. A group comes after any earlier group
//!   whose written zone crosses its read zone (or the reverse), per sheet sort; the groups of the
//!   same level thus do not touch each other and are computed at the same time, then written in order.
//!   Sky zone taken down to the bottom of the world (cautious: its exact height is only known after
//!   reading the columns).
//! - Cheaper parallel propagation (mission C2-M02): `LIGHTATOMICVIEW` (levels viewed in place as
//!   atomics instead of two copies of the zone), `LIGHTSPREADBUFS` (task queues kept),
//!   `LIGHTSPREADINLINE` (step inlined, no division with the sentinel ring, plain stores for a
//!   level spread by one task), `LIGHTSPREADAFTERSEED` (sequential propagation when the seeded
//!   queues are short).
//!
//! Thread pool: called from a rayon pool thread (tests, client task), the work
//! stays in that pool; otherwise (integrated server thread, main thread) a pool of its own for
//! the light, created at first use ([`light_threads`] threads), so as not to wait behind the
//! meshing of the common pool.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering::Relaxed};
use std::time::{Duration, Instant};

use rayon::prelude::*;

use super::change_scan::Changes;
use super::*;

/// Zone cells (sky) below which sky and block stay one after the other; also the estimated
/// work of the block (see [`pair`]) and the cells of a level of groups below which everything stays
/// on one thread.
pub(crate) const JOIN_MIN: usize = 16_384;
/// Zone cells below which a resolution stays on one thread (micro-bench with [`CHUNK`], outside the
/// pool: slab of 142,500 cells −13 to −16 %; zones of 43,000 to 53,000 cells: −20 % or
/// +18 % depending on the scene, hence left on one thread).
pub(crate) const PAR_MIN: usize = 65_536;
/// Entries of a level handed to a task of the parallel propagation.
pub(crate) const CHUNK: usize = 2048;
/// Seeded queue entries below which a large zone is still propagated on one thread
/// (`LIGHTSPREADAFTERSEED`): under [`CHUNK`] no level could be shared anyway.
pub(crate) const SPREAD_MIN: usize = 4 * CHUNK;

/// Threads of the light pool: `RUSTCRAFT_LIGHT_THREADS`, otherwise an eighth of the logical
/// cores, at least 2 (micro-bench: beyond 8 threads, a resolution of a few hundred
/// µs loses more in wake-ups and sharing than it gains).
fn light_threads() -> usize {
    rc_data::env_str!("RUSTCRAFT_LIGHT_THREADS").and_then(|v| v.trim().parse::<usize>().ok()).map_or_else(
        || (std::thread::available_parallelism().map_or(1, |n| n.get()) / 8).max(2),
        |n| n.max(1),
    )
}

fn pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        rayon::ThreadPoolBuilder::new()
            .num_threads(light_threads())
            .thread_name(|i| format!("light-{i}"))
            .build()
            .expect("light thread pool")
    })
}

/// `f` in a thread pool (the current one, otherwise the light one).
fn install<R: Send>(f: impl FnOnce() -> R + Send) -> R {
    if rayon::current_thread_index().is_some() { f() } else { pool().install(f) }
}

/// `a` and `b` at the same time; outside a pool, the caller does `a` while a thread of the light
/// pool does `b` (a single wake-up).
fn both<A: Send, B: Send>(a: impl FnOnce() -> A + Send, b: impl FnOnce() -> B + Send) -> (A, B) {
    if rayon::current_thread_index().is_some() {
        return rayon::join(a, b);
    }
    let mut rb = None;
    let ra = pool().in_place_scope(|s| {
        s.spawn(|_| rb = Some(b()));
        a()
    });
    (ra, rb.expect("second half done"))
}

/// `LIGHTJOIN` in a group: one read of the sky zone (sky and block sheets), the
/// block on its rows `r0b..r0b + nyb` and the sky on the whole zone (see [`pair`]); then
/// writing of the block and the sky.
#[allow(clippy::too_many_arguments)]
pub(super) fn join_group(
    grid: &mut impl LightGrid,
    c: &Ctx,
    w: &mut Work,
    geo: Geo,
    r0b: i32,
    nyb: i32,
    out: &mut [Touched],
) {
    read(&*grid, c, w, geo, true, true);
    let Work { face, prop, emit, sky, blk, aux, lane, lane2, .. } = w;
    let z = Cells { face, prop, emit };
    let ny = geo.size[1];
    let (go_b, db, ds) = pair(&*grid, c, &z, sky, lane, aux, blk, lane2, geo, r0b, nyb);
    let t0 = Instant::now();
    if go_b {
        write(grid, c, blk, lane2, geo, r0b, nyb, sabotage(9), out);
    }
    let t1 = Instant::now();
    write(grid, c, sky, lane, geo, 0, ny, true, out);
    count_solve(false, db + (t1 - t0));
    count_solve(true, ds + t1.elapsed());
}

/// Sky and block of a group on the same zone read (`LIGHTJOIN`). Block seeds first (one
/// walk of the zone): when it has no light to spread (common case: no emitter
/// near the changes), everything stays on this thread. Otherwise the sky stays on this thread (its buffers
/// are already in cache) and the block propagation goes to another, if its estimated work
/// (queue entries × level³, lit volume at most) reaches `join_min`. Returns (block to
/// write, block duration, sky duration).
#[allow(clippy::too_many_arguments)]
fn pair(
    gr: &impl LightGrid,
    c: &Ctx,
    z: &Cells,
    lsky: &mut [u8],
    lane: &mut Lane,
    aux: &mut Aux,
    blk: &mut [u8],
    lane2: &mut Lane,
    g: Geo,
    r0b: i32,
    nyb: i32,
) -> (bool, Duration, Duration) {
    let t0 = Instant::now();
    let go_b = seed(gr, c, z, blk, lane2, None, None, g, r0b, nyb, false);
    let work: usize = lane2.buckets.iter().enumerate().skip(2).map(|(l, b)| b.len() * l * l * l).sum();
    let block_seeded = queue_lengths(&lane2.buckets);
    let seeded = t0.elapsed();
    let (ds, db) = if go_b && work > 0 && work >= c.o.join_min {
        both(|| sky_part(gr, c, z, lsky, lane, aux, g), || block_part(c, z, blk, lane2, g, r0b, nyb))
    } else {
        let ds = sky_part(gr, c, z, lsky, lane, aux, g);
        (ds, if go_b { block_part(c, z, blk, lane2, g, r0b, nyb) } else { (Duration::ZERO, [0; 16]) })
    };
    let (db, block_queued) = db;
    if go_b {
        count_queues(c, false, &block_seeded, &block_queued, g, nyb);
    }
    (go_b, seeded + db, ds)
}

/// Sky resolution of [`pair`], timed.
fn sky_part(gr: &impl LightGrid, c: &Ctx, z: &Cells, lsky: &mut [u8], lane: &mut Lane, aux: &mut Aux, g: Geo) -> Duration {
    let t = Instant::now();
    compute(gr, c, z, lsky, lane, Some(aux), g, 0, g.size[1], true);
    t.elapsed()
}

/// Block propagation of [`pair`], timed, with the entries each level held.
fn block_part(c: &Ctx, z: &Cells, blk: &[u8], lane2: &mut Lane, g: Geo, r0b: i32, nyb: i32) -> (Duration, LevelCounts) {
    let t = Instant::now();
    let queued = if c.o.par_solve && g.cells() >= c.o.par_min && !short_queues(c, lane2) {
        install(|| spread(c, z, blk, lane2, g, r0b, nyb))
    } else {
        propagate(c, z, blk, lane2, g, r0b, nyb)
    };
    (t.elapsed(), queued)
}

/// `LIGHTSPREADAFTERSEED`: the seeded queues are too short to share; the sequential propagation
/// then saves the atomics, the level copies and the wake-ups of the parallel one.
fn short_queues(c: &Ctx, lane: &Lane) -> bool {
    c.o.spread_after_seed && lane.buckets.iter().map(Vec::len).sum::<usize>() < c.o.spread_min
}

/// `LIGHTPARSOLVE`: [`compute`] spread over the thread pool.
#[allow(clippy::too_many_arguments)]
pub(super) fn par_compute(
    grid: &impl LightGrid,
    c: &Ctx,
    z: &Cells,
    lay: &mut [u8],
    lane: &mut Lane,
    aux: Option<&mut Aux>,
    g: Geo,
    r0: i32,
    ny: i32,
    sky: bool,
) -> bool {
    install(|| {
        let pre = match (&aux, sky) {
            (Some(a), true) => Some(sources(grid, c, z, a.sec_geo, g, r0, ny)),
            _ => None,
        };
        if !seed(grid, c, z, lay, lane, aux, pre.as_deref(), g, r0, ny, sky) {
            return false;
        }
        let seeded = queue_lengths(&lane.buckets);
        let queued = if short_queues(c, lane) {
            propagate(c, z, lay, lane, g, r0, ny)
        } else {
            spread(c, z, lay, lane, g, r0, ny)
        };
        count_queues(c, sky, &seeded, &queued, g, ny);
        true
    })
}

/// Cuts off the first `k` cells of `v` (empty: stays empty).
fn cut<'a, T>(v: &mut &'a mut [T], k: usize) -> &'a mut [T] {
    if v.is_empty() {
        return &mut [];
    }
    let (a, b) = std::mem::take(v).split_at_mut(k);
    *v = b;
    a
}

/// `LIGHTPARSOLVE`: zone read ([`read_sections`]) in slices of 16 rows in parallel,
/// each slice writing its contiguous share of the arrays.
pub(super) fn read_par(grid: &impl LightGrid, c: &Ctx, w: &mut Work, g: Geo, want_sky: bool, want_blk: bool) {
    let n = g.cells();
    let band = g.plane * 16;
    let mut face = &mut w.face[..n];
    let mut prop = &mut w.prop[..n];
    let mut emit: &mut [u8] = if want_blk { &mut w.emit[..n] } else { &mut [] };
    let mut sky: &mut [u8] = if want_sky { &mut w.sky[..n] } else { &mut [] };
    let mut blk: &mut [u8] = if want_blk { &mut w.blk[..n] } else { &mut [] };
    let mut parts = Vec::new();
    let mut base = 0;
    while base < n {
        let k = band.min(n - base);
        let b = ZoneSlice {
            base,
            face: cut(&mut face, k),
            prop: cut(&mut prop, k),
            emit: cut(&mut emit, k),
            sky: cut(&mut sky, k),
            blk: cut(&mut blk, k),
            want_sky,
            want_blk,
        };
        parts.push((base, k, b));
        base += k;
    }
    if sabotage(11) {
        parts.pop();
    }
    let (y0, y1) = (g.lo[1], g.lo[1] + g.size[1]);
    install(|| {
        parts.into_par_iter().for_each_init(Vec::new, |raw, (base, k, b)| {
            // Array rows -> world y (the ring takes one row on each side).
            let r = (base / g.plane) as i32;
            let ya = (y0 + r - g.ring).max(y0);
            let yb = (y0 + r + (k / g.plane) as i32 - g.ring).min(y1);
            if ya < yb {
                read_rows(grid, c, g, ya, yb, b, raw);
            }
        })
    });
}

/// Source heights of the interior stacks (`column_source`), row by row in parallel;
/// each task has its own caches (they only depend on the states).
fn sources(grid: &impl LightGrid, c: &Ctx, z: &Cells, sg: SecGeo, g: Geo, r0: i32, ny: i32) -> Vec<i32> {
    let (sx, sz) = (g.size[0], g.size[2]);
    let mut v = vec![i32::MAX; (sx * sz) as usize];
    let nsec = (sg.ncx * sg.ncz * sg.nsec) as usize;
    v.par_chunks_mut(sx as usize).enumerate().with_min_len(2).for_each_init(
        || Aux { secs: vec![0; nsec], sec_geo: sg, memo: HashMap::new() },
        |aux, (dz, row)| {
            let dz = dz as i32;
            if !(1..sz - 1).contains(&dz) {
                return;
            }
            for dx in 1..sx - 1 {
                row[dx as usize] = column_source(grid, c, aux, z, g, r0, ny, dx, dz);
            }
        },
    );
    v
}

/// Propagation by synchronous levels (same fixed point as `propagate`): the entries of level
/// `l` are spread over tasks of `chunk` entries ([`CHUNK`] shipped); a neighbour rises through `fetch_max` and only enters the
/// queue of its new value (always below `l`) if it really rose. Returns the entries each level
/// held when its turn came, like `propagate`.
fn spread(c: &Ctx, z: &Cells, lay: &[u8], lane: &mut Lane, g: Geo, r0: i32, ny: i32) -> LevelCounts {
    let n = g.cells();
    let Lane { level, buckets, shared, task_queues, .. } = lane;
    let levels: &[AtomicU8] = if c.o.atomic_view {
        as_atomic(&mut level[..n])
    } else {
        shared.clear();
        shared.extend(level[..n].iter().map(|&v| AtomicU8::new(v)));
        shared
    };
    let mut queued = [0u32; 16];
    for l in (2..16usize).rev() {
        let mut front = std::mem::take(&mut buckets[l]);
        queued[l] = front.len() as u32;
        let chunk = c.o.chunk.max(1);
        let step = Step { c, z, lay, levels, g, r0, ny, l };
        if front.len() <= chunk {
            step.alone(&front, buckets);
        } else if c.o.spread_bufs {
            step.shared_kept_queues(&front, chunk, task_queues, buckets);
        } else {
            step.shared_new_queues(&front, chunk, buckets);
        }
        front.clear();
        buckets[l] = front;
    }
    queued[1] = buckets[1].len() as u32;
    if !c.o.atomic_view {
        for (d, a) in level[..n].iter_mut().zip(shared.iter()) {
            *d = a.load(Relaxed);
        }
    }
    queued
}

/// The levels viewed as atomics for the parallel propagation, without a copy (`LIGHTATOMICVIEW`).
#[allow(unsafe_code)]
fn as_atomic(levels: &mut [u8]) -> &mut [AtomicU8] {
    const { assert!(size_of::<AtomicU8>() == 1 && align_of::<AtomicU8>() == 1) };
    // SAFETY: `AtomicU8` has the size, alignment and bit validity of `u8` (documented guarantee,
    // checked above), and the view keeps `levels` borrowed exclusively for its whole life: no
    // plain access to these bytes can happen while the atomics are in use.
    unsafe { std::slice::from_raw_parts_mut(levels.as_mut_ptr().cast::<AtomicU8>(), levels.len()) }
}

/// One level of [`spread`]: the entries of level `l` and what they read.
#[derive(Clone, Copy)]
struct Step<'a> {
    c: &'a Ctx<'a>,
    z: &'a Cells<'a>,
    lay: &'a [u8],
    levels: &'a [AtomicU8],
    g: Geo,
    r0: i32,
    ny: i32,
    l: usize,
}

impl Step<'_> {
    /// A front of one task, on the calling thread.
    fn alone(&self, front: &[u64], buckets: &mut [Vec<u64>; 16]) {
        let mut push = |nv: usize, en: u64| buckets[nv].push(en);
        if self.c.o.spread_inline {
            // A single task: no other thread writes the levels, a plain store suffices.
            front.iter().for_each(|&e| self.relax_entry::<true, _>(e, &mut push));
        } else {
            let push: &mut dyn FnMut(usize, u64) = &mut push;
            front.iter().for_each(|&e| self.relax_entry::<false, _>(e, push));
        }
    }

    /// A front shared over tasks of `chunk` entries, each with new queues (old path).
    fn shared_new_queues(&self, front: &[u64], chunk: usize, buckets: &mut [Vec<u64>; 16]) {
        let parts: Vec<[Vec<u64>; 16]> = front
            .par_chunks(chunk)
            .map(|entries| {
                let mut queues: [Vec<u64>; 16] = Default::default();
                self.relax_task(entries, &mut queues);
                queues
            })
            .collect();
        let last = parts.len() - 1;
        for (k, mut p) in parts.into_iter().enumerate() {
            if sabotage(10) && k == last {
                continue;
            }
            for (m, b) in p.iter_mut().enumerate().take(self.l) {
                buckets[m].append(b);
            }
        }
    }

    /// Like [`Step::shared_new_queues`], each task filling queues kept in the lane from one
    /// level and one resolution to the next (`LIGHTSPREADBUFS`): no allocation once warm.
    fn shared_kept_queues(
        &self,
        front: &[u64],
        chunk: usize,
        kept: &mut Vec<[Vec<u64>; 16]>,
        buckets: &mut [Vec<u64>; 16],
    ) {
        let tasks = front.len().div_ceil(chunk);
        if kept.len() < tasks {
            kept.resize_with(tasks, Default::default);
        }
        let kept = &mut kept[..tasks];
        front.par_chunks(chunk).zip(kept.par_iter_mut()).for_each(|(entries, queues)| self.relax_task(entries, queues));
        for (k, p) in kept.iter_mut().enumerate() {
            if sabotage(10) && k == tasks - 1 {
                p.iter_mut().for_each(Vec::clear);
                continue;
            }
            // `append` leaves each task queue empty with its capacity.
            for (m, b) in p.iter_mut().enumerate().take(self.l) {
                buckets[m].append(b);
            }
        }
    }

    /// The entries of one task, other tasks running at the same time.
    fn relax_task(&self, entries: &[u64], queues: &mut [Vec<u64>; 16]) {
        let mut push = |nv: usize, en: u64| queues[nv].push(en);
        if self.c.o.spread_inline {
            entries.iter().for_each(|&e| self.relax_entry::<false, _>(e, &mut push));
        } else {
            // Old path: the push behind a dynamic call.
            let push: &mut dyn FnMut(usize, u64) = &mut push;
            entries.iter().for_each(|&e| self.relax_entry::<false, _>(e, push));
        }
    }

    /// Relaxes the neighbours of an entry of level `l` (same neighbours and rules as `propagate`).
    /// `ALONE`: no other task runs, so a rise is a plain store instead of an atomic maximum.
    fn relax_entry<const ALONE: bool, P: FnMut(usize, u64) + ?Sized>(&self, entry: u64, push: &mut P) {
        let Step { c, z, lay, levels, g, r0, ny, l } = *self;
        let o = c.o;
        let i = (entry as u32) as usize;
        if levels[i].load(Relaxed) as usize != l {
            return;
        }
        let (plane, ax) = (g.plane as isize, g.ax as isize);
        let offs: [isize; 6] = [-plane, plane, -ax, ax, -1, 1];
        let r = g.ring;
        let (xr, zr, yr) = (r..g.size[0] + r, r..g.size[2] + r, r0 + r..r0 + ny + r);
        let dirs = if o.queue_dir { (entry >> 32) as u8 } else { 63 };
        let face_i = z.face[i];
        // The coordinates only bound a zone without its sentinel ring: three divisions saved.
        let (px, pz, py) = if o.spread_inline && o.zone_sent {
            (0, 0, 0)
        } else {
            let index = i as i32;
            (index % g.ax as i32, (index / g.ax as i32) % g.az as i32, index / g.plane as i32)
        };
        for (dir, s) in STEP.iter().enumerate() {
            if dirs >> dir & 1 == 0 {
                continue;
            }
            let j = if o.zone_sent {
                i.wrapping_add_signed(offs[dir])
            } else {
                let (qx, qy, qz) = (px + s[0], py + s[1], pz + s[2]);
                if !xr.contains(&qx) || !zr.contains(&qz) || !yr.contains(&qy) {
                    continue;
                }
                qy as usize * g.plane + qz as usize * g.ax + qx as usize
            };
            let Some(new_level) = rise(c, z, lay, levels[j].load(Relaxed), face_i, j, dir, l) else { continue };
            let risen = if ALONE {
                levels[j].store(new_level, Relaxed);
                true
            } else {
                levels[j].fetch_max(new_level, Relaxed) < new_level
            };
            if risen {
                let rest = if o.queue_dir { 63 & !(1u64 << OPPOSITE[dir]) } else { 63 };
                push(new_level as usize, j as u64 | rest << 32);
            }
        }
    }
}

/// Changed cells of a resolution computed on `lane` (its levels), kept for a later write.
fn scan_all(c: &Ctx, lay: &[u8], lane: &mut Lane, g: Geo, r0: i32, ny: i32) -> Changes {
    let mut ch = Changes::default();
    change_scan::find_changes(&c.o, lay, &lane.level, &mut lane.plane_scan, g, r0, ny, &mut ch);
    ch
}

fn apply(grid: &mut impl LightGrid, c: &Ctx, ch: &Changes, sky: bool, out: &mut [Touched]) {
    let k = if sky { out.len() - 1 } else { 0 };
    change_scan::apply_changes(grid, &c.o, ch, sky, &mut out[k]);
}

/// Result of a group computed outside the grid: cells to write for the block and the sky.
#[derive(Default)]
struct Rec {
    block: Option<Changes>,
    sky: Option<Changes>,
}

/// A group computed on the read-only grid (one zone read shared by the block and the
/// sky with `ZONEONCE`, two otherwise).
fn record(grid: &impl LightGrid, c: &Ctx, w: &mut Work, cs: &[[i32; 3]], block: bool, sky: bool) -> Rec {
    w.aux.memo.clear();
    let mut rec = Rec::default();
    let bg = block_zone(c, cs);
    if !sky {
        if block {
            read(grid, c, w, bg, false, true);
            let t0 = Instant::now();
            let Work { face, prop, emit, blk, lane2, .. } = w;
            let z = Cells { face, prop, emit };
            if compute(grid, c, &z, blk, lane2, None, bg, 0, bg.size[1], false) {
                rec.block = Some(scan_all(c, blk, lane2, bg, 0, bg.size[1]));
            }
            count_solve(false, t0.elapsed());
        }
        return rec;
    }
    let mut block = block;
    if block && !c.o.zone_once {
        // `ZONEONCE=0`: the block on its own read (like `solve_group`), then the sky alone.
        rec.block = record(grid, c, w, cs, true, false).block;
        block = false;
    }
    let (sg, slo) = sky_zone(grid, c, &mut w.aux, cs, bg);
    let (r0b, nyb, ny) = (bg.lo[1] + 1 - slo, bg.size[1], sg.size[1]);
    read(grid, c, w, sg, true, block);
    let Work { face, prop, emit, sky: lsky, blk, aux, lane, lane2, .. } = w;
    let z = Cells { face, prop, emit };
    if block && c.o.join && sg.cells() >= c.o.join_min {
        let (go_b, db, ds) = pair(grid, c, &z, lsky, lane, aux, blk, lane2, sg, r0b, nyb);
        let t0 = Instant::now();
        rec.block = go_b.then(|| scan_all(c, blk, lane2, sg, r0b, nyb));
        let t1 = Instant::now();
        rec.sky = Some(scan_all(c, lsky, lane, sg, 0, ny));
        count_solve(false, db + (t1 - t0));
        count_solve(true, ds + t1.elapsed());
        return rec;
    }
    if block {
        let t0 = Instant::now();
        if compute(grid, c, &z, blk, lane2, None, sg, r0b, nyb, false) {
            rec.block = Some(scan_all(c, blk, lane2, sg, r0b, nyb));
        }
        count_solve(false, t0.elapsed());
    }
    let t0 = Instant::now();
    compute(grid, c, &z, lsky, lane, Some(aux), sg, 0, ny, true);
    rec.sky = Some(scan_all(c, lsky, lane, sg, 0, ny));
    count_solve(true, t0.elapsed());
    rec
}

/// Box read (bounds included); the written box is the same without its one-cell margin.
#[derive(Clone, Copy)]
struct Bx {
    lo: [i32; 3],
    hi: [i32; 3],
}

/// The written zone of `w` crosses the read zone of `r`.
fn hits(w: &Bx, r: &Bx) -> bool {
    (0..3).all(|a| w.lo[a] < r.hi[a] && r.lo[a] < w.hi[a])
}

/// Boxes read by a resolution: (block, sky).
type Boxes = (Option<Bx>, Option<Bx>);

fn conflict(u: &Boxes, v: &Boxes) -> bool {
    if sabotage(8) {
        return false;
    }
    let two = |a: &Option<Bx>, b: &Option<Bx>| match (a, b) {
        (Some(a), Some(b)) => hits(a, b) || hits(b, a),
        _ => false,
    };
    two(&u.0, &v.0) || two(&u.1, &v.1)
}

/// Level of each resolution: 1 + the highest level of the earlier conflicting resolutions.
fn schedule(boxes: &[Boxes]) -> Vec<usize> {
    let mut lv = vec![0usize; boxes.len()];
    for v in 0..boxes.len() {
        for u in 0..v {
            if lv[u] >= lv[v] && conflict(&boxes[u], &boxes[v]) {
                lv[v] = lv[u] + 1;
            }
        }
    }
    lv
}

/// Levels of the resolutions `(changes, block, sky)`.
pub(super) fn unit_levels(c: &Ctx, units: &[(Vec<[i32; 3]>, bool, bool)]) -> Vec<usize> {
    let boxes: Vec<Boxes> = units
        .iter()
        .map(|(cs, block, sky)| {
            let g = block_zone(c, cs);
            let lo = g.lo;
            let hi = [lo[0] + g.size[0] - 1, lo[1] + g.size[1] - 1, lo[2] + g.size[2] - 1];
            (block.then_some(Bx { lo, hi }), sky.then_some(Bx { lo: [lo[0], -1, lo[2]], hi }))
        })
        .collect();
    schedule(&boxes)
}

/// `LIGHTGROUPS`: resolutions `(changes, block, sky)` scheduled by levels.
pub(super) fn run_groups(grid: &mut impl LightGrid, c: &Ctx, units: &[(Vec<[i32; 3]>, bool, bool)], out: &mut [Touched]) {
    let lv = unit_levels(c, units);
    let top = lv.iter().copied().max().unwrap_or(0);
    for level in 0..=top {
        let idx: Vec<usize> = (0..units.len()).filter(|&k| lv[k] == level).collect();
        let cells: usize = idx.iter().map(|&k| block_zone(c, &units[k].0).cells()).sum();
        if idx.len() < 2 || cells < c.o.join_min {
            with_work(c.o.zone_buf, |w| {
                w.aux.memo.clear();
                for &k in &idx {
                    let (cs, block, sky) = &units[k];
                    solve_group(grid, c, w, cs, *block, *sky, out);
                }
            });
            continue;
        }
        let gr = &*grid;
        let recs: Vec<Rec> = install(|| {
            idx.par_iter()
                .map(|&k| {
                    let (cs, block, sky) = &units[k];
                    with_work(c.o.zone_buf, |w| record(gr, c, w, cs, *block, *sky))
                })
                .collect()
        });
        for r in &recs {
            if let Some(b) = &r.block {
                apply(grid, c, b, false, out);
            }
            if let Some(s) = &r.sky {
                apply(grid, c, s, true, out);
            }
        }
    }
}
