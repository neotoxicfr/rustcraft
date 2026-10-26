//! Lean zone resolution (mission C2-M03, `docs/perf.md`): same rule and same result,
//! bit for bit, as `solve` in `lib.rs`, kept as is and taken when all the switches
//! are at 0. Each slimming has its switch `RUSTCRAFT_OPT_<NAME>` (`0`: off, `1`:
//! on, absent: on except `SKYMEMO`, `LIGHTDEDUP` and `LIGHTJOIN`, see `DEFAULT_OFF`), read once per
//! process:
//! - `SKYSKIP`: above the sky zone and below the changes, a section whose states all
//!   let the sky through (zero attenuation, no occluding face: flag drawn from
//!   the single value or from the whole palette, computed once per section and per resolution)
//!   is skipped in one go; only the edge of its top is tested.
//! - `SKYMEMO`: per stack, height down to which the sky descends unobstructed (or first blocked edge),
//!   kept for the whole call (the states do not change during a relight).
//! - `SKYBIT`: `relight_masked`, sky resolved around only the changes that touch it.
//! - `BLOCKEARLY`: block resolution stopped if the zone has neither value nor emitter.
//! - `ZONEONCE`: a single zone read for the block and the sky (the block zone is a
//!   y-slice of the sky zone, same x and z, same top).
//! - `ZONEBUF`: zone buffers kept per thread (returned beyond [`RETAIN`] cells or
//!   [`RETAIN_BYTES`] bytes).
//! - `ZONESENT`: ring of closed cells around the zone: neighbours by fixed steps, no bounds.
//! - `ZONEUNPACK`: read per section (palette resolved once per entry, no division).
//! - `LIGHTQDIR`: the queue keeps the remaining directions (no step back towards the source cell).
//! - `LIGHTDEDUP`: changes sorted and deduplicated on entry. The only slimming that is NOT
//!   bit for bit: the sort changes the groups, hence the resolved zones, the repair of inconsistent
//!   data and the reported sections. Off by default.
//! - `RELIGHT_OCCTAB`: occluded edge read from the table of shape pairs.
//! - `LIGHTJOIN`, `LIGHTPARSOLVE`, `LIGHTGROUPS`: multi-core (mission C2-M11, `fast/par.rs`).
//! - `LIGHTINC`, `LIGHTINCMAX`: isolated groups resolved by decrease then increase (mission C2-M14,
//!   `fast/inc.rs`). Bit for bit on consistent sheets only: off by default.
//!
//! Properties read once per cell (shape, opacity, non-zero attenuation, emission) instead of
//! three tables per neighbour.

use std::cell::RefCell;
use std::sync::{Arc, OnceLock};

use hashbrown::HashMap;
use rc_world::{Container, Section};

use crate::{GROUP, LightGrid, LightTables, OPPOSITE, STEP, Touched, nibble, set_nibble};

/// Switches of the new path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Opts {
    pub sky_skip: bool,
    pub sky_memo: bool,
    pub sky_bit: bool,
    pub block_early: bool,
    pub zone_once: bool,
    pub zone_buf: bool,
    pub zone_sent: bool,
    pub zone_unpack: bool,
    pub queue_dir: bool,
    pub dedup: bool,
    pub occ_tab: bool,
    /// `LIGHTJOIN`: sky and block of a group resolved at the same time (`par.rs`).
    pub join: bool,
    /// `LIGHTPARSOLVE`: interior of a resolution spread over the thread pool.
    pub par_solve: bool,
    /// `LIGHTGROUPS`: independent groups resolved at the same time, by levels.
    pub groups: bool,
    /// Zone cells from which `join` and `par_solve` act (tests: 0).
    pub join_min: usize,
    pub par_min: usize,
    /// Entries of a level handed to a task of the parallel propagation (tests: small, so
    /// that small zones get spread; benches: the shipped value).
    pub chunk: usize,
    /// `LIGHTINC`: isolated groups resolved by decrease then increase (`fast/inc.rs`), outside the bits
    /// of [`Opts::from_bits`] (the zone proofs keep their combinations).
    pub inc: bool,
    /// `LIGHTINCMAX`: at most this many changes in a group resolved incrementally.
    pub inc_max: usize,
}

/// Names of the switches (`RUSTCRAFT_OPT_<name>`), in the bit order of [`Opts::from_bits`].
pub(crate) const NAMES: [&str; 14] = [
    "SKYSKIP",
    "SKYMEMO",
    "SKYBIT",
    "BLOCKEARLY",
    "ZONEONCE",
    "ZONEBUF",
    "ZONESENT",
    "ZONEUNPACK",
    "LIGHTQDIR",
    "LIGHTDEDUP",
    "RELIGHT_OCCTAB",
    "LIGHTJOIN",
    "LIGHTPARSOLVE",
    "LIGHTGROUPS",
];

/// Switches off by default (`=1` to turn them on): losers in the micro-bench, kept for
/// the scene bench. `SKYMEMO`: +0.5 to +3.5 % against "all" (plains, cave, building).
/// `LIGHTDEDUP`: changes the result (see the header) for ±5 % within the noise (rule C3).
/// `LIGHTJOIN`: no measured gain (±2 % in the micro-bench, synthetic trace of a single group slower
/// when on), set aside (rule C3) until the scene bench.
pub(crate) const DEFAULT_OFF: u16 = 2 | 512 | 2048;

impl Opts {
    pub(crate) const OLD: Opts = Opts::from_bits(0);
    #[cfg(test)]
    pub(crate) const ALL: Opts = Opts::from_bits(u16::MAX);

    pub(crate) const fn from_bits(b: u16) -> Opts {
        Opts {
            sky_skip: b & 1 != 0,
            sky_memo: b & 2 != 0,
            sky_bit: b & 4 != 0,
            block_early: b & 8 != 0,
            zone_once: b & 16 != 0,
            zone_buf: b & 32 != 0,
            zone_sent: b & 64 != 0,
            zone_unpack: b & 128 != 0,
            queue_dir: b & 256 != 0,
            dedup: b & 512 != 0,
            occ_tab: b & 1024 != 0,
            join: b & 2048 != 0,
            par_solve: b & 4096 != 0,
            groups: b & 8192 != 0,
            join_min: par::JOIN_MIN,
            par_min: par::PAR_MIN,
            chunk: par::CHUNK,
            inc: false,
            inc_max: usize::MAX,
        }
    }

    fn from_env() -> Opts {
        let mut b = 0u16;
        for (k, name) in NAMES.iter().enumerate() {
            let v = rc_data::env_flags::read_os(&format!("RUSTCRAFT_OPT_{name}")).and_then(|v| v.into_string().ok());
            let on = match v.as_deref().map(str::trim) {
                Some("0") => false,
                Some("1") => true,
                _ => DEFAULT_OFF & 1 << k == 0,
            };
            if on {
                b |= 1 << k;
            }
        }
        // Off by default (`=1` turns it on): not bit for bit on inconsistent sheets (see
        // `fast/inc.rs`), like `LIGHTDEDUP`.
        let (inc, inc_max) =
            Self::inc_env(rc_data::env_str!("RUSTCRAFT_OPT_LIGHTINC"), rc_data::env_str!("RUSTCRAFT_OPT_LIGHTINCMAX"));
        Opts { inc, inc_max, ..Opts::from_bits(b) }
    }

    /// `(LIGHTINC, LIGHTINCMAX)` read. Threshold ignored when the incremental is off: everything at 0 stays
    /// equal to [`Opts::OLD`] (old code taken) even if `LIGHTINCMAX` remained in the environment.
    fn inc_env(inc: Option<&str>, max: Option<&str>) -> (bool, usize) {
        let inc = inc.map(str::trim) == Some("1");
        let max = max.and_then(|v| v.trim().parse().ok()).filter(|_| inc || sabotage(25)).unwrap_or(usize::MAX);
        (inc, max)
    }

    /// Switches of the process, read on the first call.
    pub(crate) fn current() -> Opts {
        static OPTS: OnceLock<Opts> = OnceLock::new();
        *OPTS.get_or_init(Opts::from_env)
    }
}

/// Grouping of neighbouring changes, identical to that of `relight_zones` (old path):
/// a change joins the first group whose box, widened by [`GROUP`], contains it.
pub(crate) fn groups(changes: &[[i32; 3]]) -> Vec<Vec<[i32; 3]>> {
    type Group = ([i32; 3], [i32; 3], Vec<[i32; 3]>);
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
    groups.into_iter().map(|(_, _, m)| m).collect()
}

/// Buffers kept at most (zone cells): beyond that (sky zone of very large groups), returned
/// after the call. About 9 bytes per zone cell.
const RETAIN: usize = 1 << 19;
/// Bytes kept at most per thread after a call, all buffers included (buckets, memo, sections):
/// beyond that, everything is returned. A single calling thread thus keeps at most 8 MiB.
const RETAIN_BYTES: usize = 8 << 20;

/// Properties byte per cell: propagation opacity (bits 0-3, at least 1) and non-zero
/// attenuation (sky column rule, unknown state counted as zero).
const DAMP: u8 = 16;
/// Byte per cell and per sheet: stored value (bits 0-3), sheet present, cell open to
/// propagation in the current resolution (interior and sheet present).
const DATA: u8 = 16;
const OPEN: u8 = 32;

/// Stack seen above the sky zone, for the whole call.
#[derive(Clone, Copy)]
enum Memo {
    /// Edges verified clear from the top of the world down to cell `down` included, of shape `face`.
    Clear { down: i32, face: u32 },
    /// First blocked edge going down: between `y + 1` and `y`.
    Blocked(i32),
}

/// Buffers of a resolution (one sheet): levels, queues, changed cells of a stack.
#[derive(Default)]
struct Lane {
    level: Vec<u8>,
    /// Entry: cell index (low 32 bits), remaining directions (6 bits above).
    buckets: [Vec<u64>; 16],
    changed: Vec<(i32, u8)>,
    /// Shared levels of the parallel propagation (`LIGHTPARSOLVE`).
    shared: Vec<std::sync::atomic::AtomicU8>,
}

impl Lane {
    fn bytes(&self) -> usize {
        self.level.capacity()
            + self.buckets.iter().map(|b| b.capacity() * 8).sum::<usize>()
            + self.changed.capacity() * 8
            + self.shared.capacity()
    }
}

/// Caches of the sky column rule (states only, valid for the whole call).
#[derive(Default)]
struct Aux {
    /// "Clear sky" section per (chunk, section) of the zone: 0 unknown, 1 yes, 2 no.
    secs: Vec<u8>,
    sec_geo: SecGeo,
    memo: HashMap<(i32, i32), Memo>,
}

#[derive(Default)]
struct Work {
    face: Vec<u32>,
    prop: Vec<u8>,
    emit: Vec<u8>,
    sky: Vec<u8>,
    blk: Vec<u8>,
    raw: Vec<u32>,
    aux: Aux,
    /// Resolution in progress (sky under `LIGHTJOIN`).
    lane: Lane,
    /// Block resolution run at the same time as the sky one (`LIGHTJOIN`).
    lane2: Lane,
    /// Incremental resolutions (`LIGHTINC`).
    inc: inc::Buf,
}

impl Work {
    /// Bytes reserved by the buffers (capacities, memo counted at 24 bytes per slot).
    fn bytes(&self) -> usize {
        self.face.capacity() * 4
            + self.prop.capacity()
            + self.emit.capacity()
            + self.sky.capacity()
            + self.blk.capacity()
            + self.raw.capacity() * 4
            + self.aux.secs.capacity()
            + self.aux.memo.capacity() * 24
            + self.lane.bytes()
            + self.lane2.bytes()
            + self.inc.bytes()
    }
}

#[derive(Clone, Copy, Default)]
struct SecGeo {
    cx0: i32,
    cz0: i32,
    ncx: i32,
    ncz: i32,
    nsec: i32,
}

thread_local! {
    static WORK: RefCell<Work> = RefCell::new(Work::default());
}

fn with_work<R>(keep: bool, f: impl FnOnce(&mut Work) -> R) -> R {
    if !keep {
        return f(&mut Work::default());
    }
    WORK.with(|cell| match cell.try_borrow_mut() {
        Ok(mut w) => {
            let r = f(&mut w);
            let big = w.lane.level.capacity().max(w.lane2.level.capacity()) > RETAIN;
            if big || w.face.capacity() > RETAIN || w.bytes() > RETAIN_BYTES {
                *w = Work::default();
            }
            r
        }
        Err(_) => f(&mut Work::default()),
    })
}

/// Array of `n` zeros: reused (`keep`) or new.
fn reset<T: Copy + Default>(v: &mut Vec<T>, n: usize, keep: bool) {
    if keep {
        v.clear();
        v.resize(n, T::default());
    } else {
        *v = vec![T::default(); n];
    }
}

#[cfg(test)]
thread_local! {
    /// Deliberate error (tests: the equivalence proof must see it).
    pub(crate) static SABOTAGE: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
    /// Zone reads done on this thread (tests of `ZONEONCE`).
    pub(crate) static READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Passes through the parallel groups (`LIGHTGROUPS`) on this thread.
    pub(crate) static RUN_GROUPS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    /// Sky zones computed on this thread (`sky_zone`).
    pub(crate) static SKY_ZONES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[inline(always)]
fn sabotage(k: u8) -> bool {
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

/// Zone read: `size` cells (margin of one cell included) from `lo`, surrounded by a ring of
/// `ring` closed cells; index `(y · (sz + 2r) + z) · (sx + 2r) + x`.
#[derive(Clone, Copy)]
struct Geo {
    lo: [i32; 3],
    size: [i32; 3],
    ring: i32,
    ax: usize,
    az: usize,
    plane: usize,
}

impl Geo {
    fn new(lo: [i32; 3], size: [i32; 3]) -> Self {
        let ring = if sabotage(3) { 0 } else { 1 };
        let (ax, az) = ((size[0] + 2 * ring) as usize, (size[2] + 2 * ring) as usize);
        Geo { lo, size, ring, ax, az, plane: ax * az }
    }

    fn cells(&self) -> usize {
        self.plane * (self.size[1] + 2 * self.ring) as usize
    }

    #[inline]
    fn at(&self, dx: i32, dy: i32, dz: i32) -> usize {
        let r = self.ring;
        (dy + r) as usize * self.plane + (dz + r) as usize * self.ax + (dx + r) as usize
    }
}

struct Ctx<'a> {
    height: i32,
    t: &'a LightTables,
    o: Opts,
}

/// New path of `relight_zones`: block sections in `out[0]`, sky ones in the last
/// element (the same when `out` has only one). `sky_changes`: changes that touch the sky
/// (`None`: all; ignored without `SKYBIT`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn relight_zones(
    grid: &mut impl LightGrid,
    height: i32,
    t: &LightTables,
    has_sky: bool,
    changes: &[[i32; 3]],
    sky_changes: Option<&[[i32; 3]]>,
    o: Opts,
    out: &mut [Touched],
) {
    let sorted = |c: &[[i32; 3]]| {
        let mut v = c.to_vec();
        v.sort_unstable();
        v.dedup();
        v
    };
    let own;
    let changes = if o.dedup {
        own = sorted(changes);
        &own[..]
    } else {
        changes
    };
    let own_sky;
    let sky_changes = match sky_changes {
        Some(s) if o.sky_bit && has_sky => {
            if o.dedup {
                own_sky = sorted(s);
                Some(&own_sky[..])
            } else {
                Some(s)
            }
        }
        _ => None,
    }
    .filter(|s| *s != changes);
    let c = Ctx { height, t, o };
    // Resolutions in order: (changes, block, sky).
    let mut units: Vec<(Vec<[i32; 3]>, bool, bool)> = match sky_changes {
        None => groups(changes).into_iter().map(|g| (g, true, has_sky)).collect(),
        Some(_) => groups(changes).into_iter().map(|g| (g, true, false)).collect(),
    };
    if let Some(sc) = sky_changes {
        units.extend(groups(sc).into_iter().map(|g| (g, false, true)));
    }
    if o.inc {
        // Isolated groups resolved incrementally; the rest goes through the zone below.
        inc::run(grid, &c, &mut units, out);
    }
    if o.groups && units.len() > 1 {
        #[cfg(test)]
        RUN_GROUPS.with(|n| n.set(n.get() + 1));
        par::run_groups(grid, &c, &units, out);
        return;
    }
    with_work(o.zone_buf, |w| {
        w.aux.memo.clear();
        for (g, block, sky) in &units {
            solve_group(grid, &c, w, g, *block, *sky, out);
        }
    });
}

/// Resolutions of a group: block then sky, zones computed like the old `solve`.
fn solve_group(
    grid: &mut impl LightGrid,
    c: &Ctx,
    w: &mut Work,
    cs: &[[i32; 3]],
    block: bool,
    sky: bool,
    out: &mut [Touched],
) {
    let block_geo = block_zone(c, cs);
    if !sky {
        if block {
            read(&*grid, c, w, block_geo, false, true);
            solve(grid, c, w, block_geo, 0, block_geo.size[1], false, out);
        }
        return;
    }
    let (sky_geo, slo) = sky_zone(&*grid, c, &mut w.aux, cs, block_geo);
    let blo = block_geo.lo[1] + 1;
    // `LIGHTJOIN` relies on the shared read: `ZONEONCE=0` brings it back to the two reads.
    if block && c.o.join && c.o.zone_once && sky_geo.cells() >= c.o.join_min {
        par::join_group(grid, c, w, sky_geo, blo - slo, block_geo.size[1], out);
        return;
    }
    if block && c.o.zone_once {
        read(&*grid, c, w, sky_geo, true, true);
        let r0 = blo - slo + i32::from(sabotage(4));
        solve(grid, c, w, sky_geo, r0, block_geo.size[1], false, out);
    } else {
        if block {
            read(&*grid, c, w, block_geo, false, true);
            solve(grid, c, w, block_geo, 0, block_geo.size[1], false, out);
        }
        read(&*grid, c, w, sky_geo, true, false);
    }
    solve(grid, c, w, sky_geo, 0, sky_geo.size[1], true, out);
}

/// Block zone of a group: box of the changes at ±16 in x and z, ±15 in y (bounded to the world),
/// margin of one cell included.
fn block_zone(c: &Ctx, cs: &[[i32; 3]]) -> Geo {
    let (mut bmin, mut bmax) = (cs[0], cs[0]);
    for p in cs {
        for a in 0..3 {
            bmin[a] = bmin[a].min(p[a]);
            bmax[a] = bmax[a].max(p[a]);
        }
    }
    let (lo0, lo2) = (bmin[0] - 16, bmin[2] - 16);
    let (sx, sz) = (bmax[0] - bmin[0] + 33, bmax[2] - bmin[2] + 33);
    let (blo, yhi) = ((bmin[1] - 15).max(0), (bmax[1] + 15).min(c.height - 1));
    Geo::new([lo0, blo - 1, lo2], [sx, yhi - blo + 3, sz])
}

/// Sky zone of a group (same x, z and top as `bg`, lowered below the sky columns
/// that pass through the changes) and its first row of the world; prepares the caches of `aux`.
fn sky_zone(grid: &impl LightGrid, c: &Ctx, aux: &mut Aux, cs: &[[i32; 3]], bg: Geo) -> (Geo, i32) {
    #[cfg(test)]
    SKY_ZONES.with(|n| n.set(n.get() + 1));
    let (lo0, lo2, sx, sz) = (bg.lo[0], bg.lo[2], bg.size[0], bg.size[2]);
    let cx0 = lo0 >> 4;
    let cz0 = lo2 >> 4;
    aux.sec_geo = SecGeo {
        cx0,
        cz0,
        ncx: ((lo0 + sx - 1) >> 4) - cx0 + 1,
        ncz: ((lo2 + sz - 1) >> 4) - cz0 + 1,
        nsec: (c.height + 15) >> 4,
    };
    let g = aux.sec_geo;
    reset(&mut aux.secs, (g.ncx * g.ncz * g.nsec) as usize, c.o.zone_buf);
    let bmin1 = cs.iter().map(|p| p[1]).min().unwrap_or(0);
    let bottom = sky_bottom(grid, c, aux, cs, bmin1);
    let slo = (bottom - 15).max(0);
    let yhi = bg.lo[1] + bg.size[1] - 2;
    (Geo::new([lo0, slo - 1, lo2], [sx, yhi - slo + 3, sz]), slo)
}

/// The whole section lets the sky through (absent: state 0 everywhere, like the read).
fn section_clear(t: &LightTables, sections: Option<&[Section]>, sec: i32) -> bool {
    match sections.and_then(|s| s.get(sec as usize)) {
        None => t.sky_clear(0),
        Some(s) => match &s.blocks {
            Container::Single(v) => t.sky_clear(*v),
            // An index outside the palette reads as state 0.
            Container::Packed { bits, palette: Some(p), .. } => {
                p.iter().all(|&v| t.sky_clear(v)) && ((p.len() as u64) >= 1u64 << *bits || t.sky_clear(0))
            }
            Container::Packed { palette: None, .. } => sabotage(7),
        },
    }
}

/// [`section_clear`] retained for the resolution (flag per chunk and section of the zone).
fn sec_clear(w: &mut Aux, t: &LightTables, cx: i32, cz: i32, sections: Option<&[Section]>, sec: i32) -> bool {
    let g = w.sec_geo;
    let (ix, iz) = (cx - g.cx0, cz - g.cz0);
    if !(0..g.ncx).contains(&ix) || !(0..g.ncz).contains(&iz) || !(0..g.nsec).contains(&sec) {
        return section_clear(t, sections, sec);
    }
    let k = ((iz * g.ncx + ix) * g.nsec + sec) as usize;
    match w.secs[k] {
        1 => true,
        2 => false,
        _ => {
            let v = section_clear(t, sections, sec);
            w.secs[k] = if v { 1 } else { 2 };
            v
        }
    }
}

/// State of a cell of a column (`y` relative to the bottom of the world); 0 outside the world or without section.
#[inline]
fn state_in(sections: Option<&[Section]>, lx: usize, lz: usize, y: i32, height: i32) -> u32 {
    if !(0..height).contains(&y) {
        return 0;
    }
    sections
        .and_then(|s| s.get((y >> 4) as usize))
        .map_or(0, |s| s.blocks.get(rc_world::block_index(lx, (y & 15) as usize, lz)))
}

/// Bottom of the sky columns that pass through the changes (same rule as the old `solve`).
fn sky_bottom(grid: &impl LightGrid, c: &Ctx, w: &mut Aux, cs: &[[i32; 3]], bmin1: i32) -> i32 {
    let (t, h, tab) = (c.t, c.height, c.o.occ_tab);
    let mut bottom = bmin1;
    for p in cs {
        let (cx, cz) = (p[0] >> 4, p[2] >> 4);
        let col = grid.column(cx, cz).map(|(s, _, _)| s);
        let (lx, lz) = ((p[0] & 15) as usize, (p[2] & 15) as usize);
        let first = p[1] - 1;
        let mut b = first;
        let mut hf = 0u32;
        while b >= 0 {
            if c.o.sky_skip && b < h && sec_clear(w, t, cx, cz, col, b >> 4) {
                if b != first && t.occluded_faces(hf, 0, 0, tab) {
                    break;
                }
                hf = 0;
                b = (b & !15) - 1;
                continue;
            }
            let s = state_in(col, lx, lz, b, h);
            if t.damps(s) || (b != first && t.occluded_faces(hf, t.face(s), 0, tab)) {
                break;
            }
            hf = t.face(s);
            b -= 1;
        }
        bottom = bottom.min(b + 1);
    }
    bottom
}

/// The sky descends without an occluded edge from the top of the world to `from` included (same rule as
/// the old `clear_above`).
fn clear_above(grid: &impl LightGrid, c: &Ctx, w: &mut Aux, x: i32, z: i32, from: i32) -> bool {
    let (t, h, tab) = (c.t, c.height, c.o.occ_tab);
    let stop = from.max(-1);
    let (mut y, mut hf) = (h - 1, t.face(0));
    if c.o.sky_memo {
        match w.memo.get(&(x, z)) {
            Some(Memo::Blocked(yf)) => return stop > *yf - i32::from(sabotage(6)),
            Some(&Memo::Clear { down, face }) => {
                if stop >= down {
                    return true;
                }
                y = down - 1;
                hf = face;
            }
            None => {}
        }
    }
    let (cx, cz) = (x >> 4, z >> 4);
    let Some((sections, _, _)) = grid.column(cx, cz) else { return true };
    let (lx, lz) = ((x & 15) as usize, (z & 15) as usize);
    let blocked = loop {
        if y < stop {
            break None;
        }
        if c.o.sky_skip && (0..h).contains(&y) && sec_clear(w, t, cx, cz, Some(sections), y >> 4) {
            if t.occluded_faces(hf, 0, 0, tab) {
                break Some(y);
            }
            if !sabotage(2) {
                hf = 0;
            }
            y = (y & !15) - if sabotage(1) { 17 } else { 1 };
            continue;
        }
        let s = state_in(Some(sections), lx, lz, y, h);
        if t.damps(s) || t.occluded_faces(hf, t.face(s), 0, tab) {
            break Some(y);
        }
        hf = t.face(s);
        y -= 1;
    };
    if c.o.sky_memo {
        let m = match blocked {
            Some(yf) => Memo::Blocked(yf),
            None => Memo::Clear { down: y + 1, face: hf },
        };
        w.memo.insert((x, z), m);
    }
    blocked.is_none()
}

/// Shape, properties byte and emission of a state.
#[inline]
fn props(t: &LightTables, s: u32) -> (u32, u8, u8) {
    (t.face(s), t.opacity(s) | if t.damps(s) { DAMP } else { 0 }, t.emission.get(s as usize).copied().unwrap_or(0))
}

/// Reads the zone: properties of the states, and values of the wanted sheets (sky, block).
fn read(grid: &impl LightGrid, c: &Ctx, w: &mut Work, g: Geo, want_sky: bool, want_blk: bool) {
    #[cfg(test)]
    READS.with(|r| r.set(r.get() + 1));
    let keep = c.o.zone_buf;
    let n = g.cells();
    reset(&mut w.face, n, keep);
    reset(&mut w.prop, n, keep);
    if want_blk {
        reset(&mut w.emit, n, keep);
        reset(&mut w.blk, n, keep);
    }
    if want_sky {
        reset(&mut w.sky, n, keep);
    }
    if c.o.zone_unpack && c.o.par_solve && n >= c.o.par_min {
        par::read_par(grid, c, w, g, want_sky, want_blk);
    } else if c.o.zone_unpack {
        read_sections(grid, c, w, g, want_sky, want_blk);
    } else {
        read_cells(grid, c, w, g, want_sky, want_blk);
    }
}

#[inline]
#[allow(clippy::too_many_arguments)]
fn put(w: &mut Work, i: usize, p: (u32, u8, u8), vs: u8, vb: u8, want_sky: bool, want_blk: bool) {
    w.face[i] = p.0;
    w.prop[i] = p.1;
    if want_blk {
        w.emit[i] = p.2;
        w.blk[i] = vb;
    }
    if want_sky {
        w.sky[i] = vs;
    }
}

/// Cell-by-cell read (like the old `solve`), one column lookup per stack.
fn read_cells(grid: &impl LightGrid, c: &Ctx, w: &mut Work, g: Geo, want_sky: bool, want_blk: bool) {
    let (t, h) = (c.t, c.height);
    for dz in 0..g.size[2] {
        for dx in 0..g.size[0] {
            let (x, z) = (g.lo[0] + dx, g.lo[2] + dz);
            let col = grid.column(x >> 4, z >> 4);
            let (lx, lz) = ((x & 15) as usize, (z & 15) as usize);
            for dy in 0..g.size[1] {
                let y = g.lo[1] + dy;
                let i = g.at(dx, dy, dz);
                let (mut s, mut vs, mut vb) = (0u32, 0u8, 0u8);
                if let Some((sections, sky_layers, block_layers)) = col {
                    if (0..h).contains(&y) {
                        s = state_in(Some(sections), lx, lz, y, h);
                        let li = ((y >> 4) + 1) as usize;
                        let ly = (y & 15) as usize;
                        if want_sky && let Some(Some(l)) = sky_layers.get(li) {
                            vs = DATA | nibble(l, lx, ly, lz);
                        }
                        if want_blk && let Some(Some(l)) = block_layers.get(li) {
                            vb = DATA | nibble(l, lx, ly, lz);
                        }
                    } else if y >= h {
                        // Above the world: full sky (without a sheet).
                        vs = 15;
                    }
                }
                put(w, i, props(t, s), vs, vb, want_sky, want_blk);
            }
        }
    }
}

/// Palette indices of a whole section (same packing as `Container::unpack_into`).
fn unpack_raw(bits: u8, data: &[u64], out: &mut [u32]) {
    let b = bits as usize;
    let vpl = 64 / b;
    let mask = (1u64 << b) - 1;
    for (word, chunk) in data.iter().zip(out.chunks_mut(vpl)) {
        let mut v = *word;
        for slot in chunk {
            *slot = (v & mask) as u32;
            v >>= b;
        }
    }
}

/// Output arrays of a zone read, from cell `base` (a slice of rows;
/// unwanted sheets: empty arrays).
struct Bufs<'a> {
    base: usize,
    face: &'a mut [u32],
    prop: &'a mut [u8],
    emit: &'a mut [u8],
    sky: &'a mut [u8],
    blk: &'a mut [u8],
    want_sky: bool,
    want_blk: bool,
}

impl Bufs<'_> {
    #[inline]
    fn put(&mut self, i: usize, p: (u32, u8, u8), vs: u8, vb: u8) {
        let i = i - self.base;
        self.face[i] = p.0;
        self.prop[i] = p.1;
        if self.want_blk {
            self.emit[i] = p.2;
            self.blk[i] = vb;
        }
        if self.want_sky {
            self.sky[i] = vs;
        }
    }
}

/// Read per chunk and section: single value set at once, palette resolved once per
/// entry, sheets looked up once per section.
fn read_sections(grid: &impl LightGrid, c: &Ctx, w: &mut Work, g: Geo, want_sky: bool, want_blk: bool) {
    let n = g.cells();
    let mut raw = std::mem::take(&mut w.raw);
    let out = Bufs {
        base: 0,
        face: &mut w.face[..n],
        prop: &mut w.prop[..n],
        emit: if want_blk { &mut w.emit[..n] } else { &mut [] },
        sky: if want_sky { &mut w.sky[..n] } else { &mut [] },
        blk: if want_blk { &mut w.blk[..n] } else { &mut [] },
        want_sky,
        want_blk,
    };
    read_rows(grid, c, g, g.lo[1], g.lo[1] + g.size[1], out, &mut raw);
    w.raw = raw;
}

/// Rows `ya..yb` (world y, within the zone) of [`read_sections`], written into `out`.
fn read_rows(grid: &impl LightGrid, c: &Ctx, g: Geo, ya: i32, yb: i32, mut out: Bufs, raw: &mut Vec<u32>) {
    let (t, h) = (c.t, c.height);
    let (x0, x1) = (g.lo[0], g.lo[0] + g.size[0]);
    let (y0, y1) = (g.lo[1], yb);
    let (z0, z1) = (g.lo[2], g.lo[2] + g.size[2]);
    raw.resize(rc_world::SECTION_BLOCKS, 0);
    let air = props(t, 0);
    for cz in (z0 >> 4)..=((z1 - 1) >> 4) {
        for cx in (x0 >> 4)..=((x1 - 1) >> 4) {
            let (xa, xb) = (x0.max(cx * 16), x1.min(cx * 16 + 16));
            let (za, zb) = (z0.max(cz * 16), z1.min(cz * 16 + 16));
            let col = grid.column(cx, cz);
            let mut y = ya;
            while y < y1 {
                let Some((sections, sky_layers, block_layers)) = col.filter(|_| (0..h).contains(&y)) else {
                    // Row outside the world, or column absent (neither light nor passage).
                    let vs = if col.is_some() && y >= h { 15 } else { 0 };
                    for z in za..zb {
                        for x in xa..xb {
                            out.put(g.at(x - x0, y - y0, z - z0), air, vs, 0);
                        }
                    }
                    y += 1;
                    continue;
                };
                let sec = y >> 4;
                let yb = y1.min(sec * 16 + 16).min(h);
                let li = (sec + 1) as usize;
                let lsky = if out.want_sky { sky_layers.get(li).and_then(|l| l.as_deref()) } else { None };
                let lblk = if out.want_blk { block_layers.get(li).and_then(|l| l.as_deref()) } else { None };
                let cells = (xb - xa) * (zb - za) * (yb - y);
                // Properties of a cell by index in the section, depending on the content.
                let mut pal = [(0u32, 0u8, 0u8); 256];
                let blocks = sections.get(sec as usize).map(|s| &s.blocks);
                enum Src<'a> {
                    Fixed((u32, u8, u8)),
                    Palette,
                    Values,
                    Cells(&'a Container),
                }
                let src = match blocks {
                    None => Src::Fixed(props(t, 0)),
                    Some(Container::Single(v)) => Src::Fixed(props(t, *v)),
                    Some(b) if cells < 512 => Src::Cells(b),
                    Some(Container::Packed { bits, palette: Some(p), data }) if *bits <= 8 => {
                        for (k, e) in pal.iter_mut().enumerate().take(1 << bits) {
                            *e = props(t, p.get(k).copied().unwrap_or(0));
                        }
                        unpack_raw(*bits, data, raw);
                        Src::Palette
                    }
                    Some(b) => {
                        b.unpack_into(raw);
                        Src::Values
                    }
                };
                for yy in y..yb {
                    let ly = (yy & 15) as usize;
                    for z in za..zb {
                        let lz = (z & 15) as usize;
                        for x in xa..xb {
                            let lx = (x & 15) as usize;
                            let k = rc_world::block_index(lx, ly, lz);
                            let p = match &src {
                                Src::Fixed(p) => *p,
                                Src::Palette => pal[raw[k] as usize],
                                Src::Values => props(t, raw[k]),
                                Src::Cells(b) => props(t, b.get(k)),
                            };
                            let vs = lsky.map_or(0, |l| DATA | nibble(l, lx, ly, lz));
                            let vb = lblk.map_or(0, |l| DATA | nibble(l, lx, ly, lz));
                            out.put(g.at(x - x0, yy - y0, z - z0), p, vs, vb);
                        }
                    }
                }
                y = yb;
            }
        }
    }
}

/// Arrays of the zone read, shared read-only by the resolutions.
#[derive(Clone, Copy)]
struct Cells<'a> {
    face: &'a [u32],
    prop: &'a [u8],
    emit: &'a [u8],
}

/// Source height of the sky of a stack (§ 1.4), over the rows `r0..r0 + ny` of the zone read.
#[allow(clippy::too_many_arguments)]
fn source_height(c: &Ctx, z: &Cells, g: Geo, r0: i32, ny: i32, dx: i32, dz: i32) -> i32 {
    let base = g.lo[1] + r0;
    let mut top = ny - 2;
    while top >= 1 && base + top >= c.height {
        top -= 1;
    }
    for dy in (1..=top).rev() {
        let (hi, lo) = (g.at(dx, r0 + dy + 1, dz), g.at(dx, r0 + dy, dz));
        if z.prop[lo] & DAMP != 0 || c.t.occluded_faces(z.face[hi], z.face[lo], 0, c.o.occ_tab) {
            return base + dy + 1;
        }
    }
    i32::MIN
}

/// Height from which the interior stack `(dx, dz)` receives full sky (`i32::MAX`: never).
#[allow(clippy::too_many_arguments)]
fn column_source(
    grid: &impl LightGrid,
    c: &Ctx,
    aux: &mut Aux,
    z: &Cells,
    g: Geo,
    r0: i32,
    ny: i32,
    dx: i32,
    dz: i32,
) -> i32 {
    let top_margin = g.lo[1] + r0 + ny - 1;
    if clear_above(grid, c, aux, g.lo[0] + dx, g.lo[2] + dz, top_margin) {
        source_height(c, z, g, r0, ny, dx, dz)
    } else {
        i32::MAX
    }
}

/// Resolution of a sheet over the rows `r0..r0 + ny` of the zone read (margins included): same
/// sources, seeds, propagation and writing as the old `solve`. Counted in `stats` like
/// the old `solve` (one resolution per call; the zone read, done before, is not).
#[allow(clippy::too_many_arguments)]
fn solve(grid: &mut impl LightGrid, c: &Ctx, w: &mut Work, g: Geo, r0: i32, ny: i32, sky: bool, out: &mut [Touched]) {
    let t0 = std::time::Instant::now();
    solve_body(grid, c, w, g, r0, ny, sky, out);
    count_solve(sky, t0.elapsed());
}

/// A resolution counted in `stats`, of duration `d`.
fn count_solve(sky: bool, d: std::time::Duration) {
    use std::sync::atomic::Ordering::Relaxed;
    let ns = d.as_nanos() as u64;
    let (count, time) = if sky {
        (&crate::stats::SKY_SOLVES, &crate::stats::SKY_NS)
    } else {
        (&crate::stats::BLOCK_SOLVES, &crate::stats::BLOCK_NS)
    };
    count.fetch_add(1, Relaxed);
    time.fetch_add(ns, Relaxed);
}

#[allow(clippy::too_many_arguments)]
fn solve_body(grid: &mut impl LightGrid, c: &Ctx, w: &mut Work, g: Geo, r0: i32, ny: i32, sky: bool, out: &mut [Touched]) {
    let Work { face, prop, emit, sky: lsky, blk, aux, lane, .. } = w;
    let z = Cells { face, prop, emit };
    let lay = if sky { lsky } else { blk };
    if compute(&*grid, c, &z, lay, lane, sky.then_some(aux), g, r0, ny, sky) {
        write(grid, lay, lane, g, r0, ny, sky, out);
    }
}

/// Sources, seeds and propagation of a resolution (levels in `lane.level`); false when
/// there is nothing to write. `aux`: sky caches (required when `sky`).
#[allow(clippy::too_many_arguments)]
fn compute(
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
    if c.o.par_solve && g.cells() >= c.o.par_min {
        return par::par_compute(grid, c, z, lay, lane, aux, g, r0, ny, sky);
    }
    if !seed(grid, c, z, lay, lane, aux, None, g, r0, ny, sky) {
        return false;
    }
    propagate(c, z, lay, lane, g, r0, ny);
    true
}

/// Starting values and queues: zone sources, margin seeds. `pre`: source heights
/// of the stacks already computed (`column_source`, index `dz · sx + dx`). False when the
/// block resolution stops early (`BLOCKEARLY`).
#[allow(clippy::too_many_arguments)]
fn seed(
    grid: &impl LightGrid,
    c: &Ctx,
    z: &Cells,
    lay: &mut [u8],
    lane: &mut Lane,
    mut aux: Option<&mut Aux>,
    pre: Option<&[i32]>,
    g: Geo,
    r0: i32,
    ny: i32,
    sky: bool,
) -> bool {
    let o = c.o;
    let (sx, sz) = (g.size[0], g.size[2]);
    let n = g.cells();
    if o.zone_buf {
        if lane.level.len() < n {
            lane.level.resize(n, 0);
        }
        for b in &mut lane.buckets {
            b.clear();
        }
    } else {
        lane.level = vec![0; n];
        lane.buckets = Default::default();
    }
    let all = 63u64 << 32;
    let mut any = 0u8;
    for dz in 0..sz {
        for dx in 0..sx {
            let hin = (1..sx - 1).contains(&dx) && (1..sz - 1).contains(&dz);
            let source_from = if !(sky && hin) {
                i32::MAX
            } else if let Some(p) = pre {
                p[(dz * sx + dx) as usize]
            } else {
                let aux = aux.as_deref_mut().expect("sky caches");
                column_source(grid, c, aux, z, g, r0, ny, dx, dz)
            };
            for r in r0..r0 + ny {
                let i = g.at(dx, r, dz);
                let b = lay[i];
                let (val, data) = (b & 15, b & DATA != 0);
                let v;
                if hin && r > r0 && r < r0 + ny - 1 {
                    if data {
                        lay[i] = b | OPEN;
                        v = if !sky {
                            z.emit[i]
                        } else if g.lo[1] + r >= source_from {
                            15
                        } else {
                            0
                        };
                    } else {
                        v = 0;
                    }
                    if v > 1 {
                        lane.buckets[v as usize].push(i as u64 | all);
                    }
                } else {
                    // Margin: stored value, seed if sheet present or full sky outside the world.
                    v = val;
                    if v > 1 && (data || val == 15) {
                        lane.buckets[v as usize].push(i as u64 | all);
                    }
                }
                lane.level[i] = v;
                any |= v | val;
            }
        }
    }
    // Neither value nor emitter: everything stays at 0, nothing to write.
    sky || !o.block_early || any != 0
}

fn propagate(c: &Ctx, z: &Cells, lay: &[u8], lane: &mut Lane, g: Geo, r0: i32, ny: i32) {
    let (t, o) = (c.t, c.o);
    let (face, prop) = (z.face, z.prop);
    let Lane { level, buckets, .. } = lane;
    let (plane, ax) = (g.plane as isize, g.ax as isize);
    let offs: [isize; 6] = [-plane, plane, -ax, ax, -1, 1];
    let r = g.ring;
    // Bounds (without sentinel ring): cells of the resolution zone, in array indices.
    let (xr, zr, yr) = (r..g.size[0] + r, r..g.size[2] + r, r0 + r..r0 + ny + r);
    for l in (2..16usize).rev() {
        while let Some(e) = buckets[l].pop() {
            let i = (e as u32) as usize;
            if level[i] as usize != l {
                continue;
            }
            let dirs = if o.queue_dir { (e >> 32) as u8 } else { 63 };
            let fi = face[i];
            let (px, pz, py) = if o.zone_sent {
                (0, 0, 0)
            } else {
                let ii = i as i32;
                (ii % g.ax as i32, (ii / g.ax as i32) % g.az as i32, ii / g.plane as i32)
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
                if lay[j] & OPEN == 0 || t.occluded_faces(fi, face[j], dir, o.occ_tab) {
                    continue;
                }
                let nv = l as i32 - i32::from(prop[j] & 15);
                if nv > i32::from(level[j]) {
                    level[j] = nv as u8;
                    let back = if sabotage(5) { dir } else { OPPOSITE[dir] };
                    let rest = if o.queue_dir { 63 & !(1u64 << back) } else { 63 };
                    buckets[nv as usize].push(j as u64 | rest << 32);
                }
            }
        }
    }
}

/// Changed cells of the interior stack `(dx, dz)`: (height, new value), bottom to top.
#[allow(clippy::too_many_arguments)]
fn scan_column(lay: &[u8], level: &[u8], g: Geo, r0: i32, ny: i32, dx: i32, dz: i32, into: &mut Vec<(i32, u8)>) {
    for r in r0 + 1..r0 + ny - 1 {
        let i = g.at(dx, r, dz);
        let b = lay[i];
        if b & DATA != 0 && level[i] != b & 15 {
            into.push((g.lo[1] + r, level[i]));
        }
    }
}

/// Writes the changed cells of a stack and marks its touched sections (like the old `solve`).
fn apply_column(grid: &mut impl LightGrid, x: i32, wz: i32, changed: &[(i32, u8)], sky: bool, touched: &mut Touched) {
    let (lx, lz) = ((x & 15) as usize, (wz & 15) as usize);
    let Some(layers) = grid.layers_mut(x >> 4, wz >> 4, sky) else { return };
    let flat =
        rc_world::edge::ANY | rc_world::edge::bits(lx, rc_world::edge::X0) | rc_world::edge::bits(lz, rc_world::edge::Z0);
    let mut run: Option<(i32, u8)> = None;
    for &(y, v) in changed {
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

/// Writing of the changed cells and touched sections (like the old `solve`).
#[allow(clippy::too_many_arguments)]
fn write(grid: &mut impl LightGrid, lay: &[u8], lane: &mut Lane, g: Geo, r0: i32, ny: i32, sky: bool, out: &mut [Touched]) {
    let k = if sky { out.len() - 1 } else { 0 };
    let touched = &mut out[k];
    let Lane { level, changed, .. } = lane;
    for dz in 1..g.size[2] - 1 {
        for dx in 1..g.size[0] - 1 {
            changed.clear();
            scan_column(lay, level, g, r0, ny, dx, dz, changed);
            if !changed.is_empty() {
                apply_column(grid, g.lo[0] + dx, g.lo[2] + dz, changed, sky, touched);
            }
        }
    }
}

mod inc;
mod par;

#[cfg(test)]
mod tests;
