//! Incremental relight checked by the uniqueness of the fixed point (`LIGHTINCVERIFY`, mission
//! C2-M12, `docs/perf.md`): the zone's result, bit for bit, on any sheet, for the cost of the
//! incremental plus one pass over the zone read.
//!
//! Why a check is enough. A resolution ends with every open cell (interior, sheet present) at
//! `max(source, brightest neighbour − its opacity)`, a neighbour counting only where the faces of the
//! two shapes leave the passage open, the margin frozen at its stored value (a seed when it has a
//! sheet or is the full sky above the world), every other cell dark. Each step costs at least 1, so
//! these equations have a single solution: in any solution, a lit cell takes its value from its source
//! or from a strictly brighter neighbour, and following the brighter neighbours ends at a source or a
//! margin seed, so the value is at most the best path the zone's propagation finds, which is itself
//! a solution and the smallest one. Any levels that satisfy every equation of the box are therefore
//! the zone's own, whatever the sheets held before.
//!
//! A resolution, once the zone is read (a read the zone pays anyway):
//! - the incremental of `fast/inc.rs` runs in place on the read's sheet values (`lay`), the first
//!   change of each cell saved in an undo log: no table of its own, no lazy read of the grid;
//! - the check walks the open cells plane by plane, each plane's interior as one stretch of the array
//!   ([`plane_holds`]): brightest of the six neighbours minus the opacity, against the source, compared
//!   with the cell's value, branch-free so the compiler vectorises it; the faces are only looked up
//!   where a shape is around and a neighbour brings more than the cell's own light (`SHAPED`); it stops
//!   at the first chunk that does not hold;
//! - it holds: the logged cells whose value changed are written, the same cells, sheets and touched
//!   sections as the zone; it does not: the log puts the values back and the zone resolves on the same
//!   read, with the sky source heights the check already computed.
//!
//! Large zones (`LIGHTPARSOLVE`, at least `par_min` cells): the check runs by planes on the thread
//! pool, the incremental stays on the caller's thread. GPU: not applicable (tens of µs per
//! resolution, less than one upload of its zone).
//!
//! `LIGHTINCOVERLAP` ([`run_in_order`], off by default): every group checked in the zone's order. A
//! group's zone can hold the changes of another group (its incremental starts from every change of
//! the call inside its zone) and cells an earlier group rewrote, whose neighbours outside that group's
//! box still carry the light of before: those cells are put out again (from the brighter of their old
//! and new values), otherwise the check would nearly always hand such a group back to the zone.

use std::cell::RefCell;
use std::ops::Range;
use std::time::Instant;

use rayon::prelude::*;

use super::*;

/// Sheet byte bits the zone read leaves free (value: bits 0-3, `DATA`, `OPEN`): the cell's byte is
/// in the undo log, and the cell is a starting point of the decrease.
const LOGGED: u8 = 64;
const REMOVED: u8 = 128;
const _: () = assert!((LOGGED | REMOVED) & (15 | DATA | OPEN) == 0);

/// Deliberate errors of the tests (the equivalence proofs must see them).
pub(super) const SABOTAGE_CHECK_WITHOUT_SKY_SOURCE: u8 = 60;
pub(super) const SABOTAGE_CHECK_IGNORES_MARGIN_SEEDS: u8 = 61;
pub(super) const SABOTAGE_CHECK_FACES_THE_WRONG_WAY: u8 = 62;
pub(super) const SABOTAGE_CHECK_SKIPPED: u8 = 63;
pub(super) const SABOTAGE_UNDO_FORGOTTEN: u8 = 64;
pub(super) const SABOTAGE_REWRITTEN_CELLS_KEPT: u8 = 65;
pub(super) const SABOTAGE_OWN_CHANGES_ONLY: u8 = 66;

/// Buffers of the checked resolutions, kept per thread with the zone buffers.
#[derive(Default)]
pub(super) struct Scratch {
    /// Cells the incremental changed, with their byte before the first change.
    undo: Vec<(u32, u8)>,
    /// Starting points of the decrease: cell and level to put out.
    removed: Vec<(u32, u8)>,
    /// Decrease: cell (32 bits), level to put out (4 bits), directions still to try (6 bits).
    decrease: Vec<u64>,
    /// Emptied cells that light by themselves: cell and own light (emission or full sky).
    reseed: Vec<(u32, u8)>,
    /// Increase per level: cell (32 bits), directions still to try (6 bits).
    increase: [Vec<u64>; 16],
    /// Sky source height of every stack, index `dz · sx + dx` (`i32::MAX` for the margin stacks).
    sources: Vec<i32>,
    /// The same heights as rows of the zone, in plane layout (see [`plane_source_rows`]).
    source_rows: Vec<u16>,
    /// Light and shape byte of every cell of the rows, read by the check (see [`light_and_shape_bytes`]).
    given: Vec<u8>,
    /// Interior stacks of the changes `(x, z, height of a change)`, sorted.
    stacks: Vec<(i32, i32, i32)>,
    /// Changed cells (index in the plane, height, new value), then grouped by stack.
    changed: Vec<(u32, i32, u8)>,
    changes: change_scan::Changes,
}

impl Scratch {
    pub(super) fn bytes(&self) -> usize {
        self.undo.capacity() * 8
            + self.removed.capacity() * 8
            + self.decrease.capacity() * 8
            + self.reseed.capacity() * 8
            + self.increase.iter().map(|b| b.capacity() * 8).sum::<usize>()
            + self.sources.capacity() * 4
            + self.source_rows.capacity() * 2
            + self.given.capacity()
            + self.stacks.capacity() * 12
            + self.changed.capacity() * 12
            + self.changes.bytes()
    }

    fn clear(&mut self) {
        self.undo.clear();
        self.removed.clear();
        self.decrease.clear();
        self.reseed.clear();
        for b in &mut self.increase {
            b.clear();
        }
    }
}

/// What the other groups of the call bring to a checked resolution (`LIGHTINCOVERLAP`), per sheet
/// (index 0 block, 1 sky): every change of the call (the zone of a group sees the changes of the
/// groups whose box it crosses: they are starting points of its incremental too), and the cells the
/// earlier groups rewrote, with their value before. Inactive (isolated groups): nothing.
#[derive(Default)]
pub(super) struct Overlap {
    active: bool,
    changes: [Vec<[i32; 3]>; 2],
    rewritten: [RefCell<Vec<Rewrite>>; 2],
}

/// A cell an earlier group rewrote: position and value before.
type Rewrite = ([i32; 3], u8);

/// A resolution of a call: `(changes, block, sky)`.
type Unit = (Vec<[i32; 3]>, bool, bool);

impl Overlap {
    /// The resolutions of a call, every group seeing the others.
    fn of_call(units: &[Unit]) -> Self {
        let changes_of = |sheet: fn(&Unit) -> bool| units.iter().filter(|u| sheet(u)).flat_map(|u| u.0.iter().copied()).collect::<Vec<_>>();
        Overlap { active: units.len() > 1, changes: [changes_of(|u| u.1), changes_of(|u| u.2)], ..Overlap::default() }
    }

    /// Cells of a sheet just written, with their value before.
    fn record(&self, sky: bool, cells: impl Iterator<Item = Rewrite>) {
        if self.active {
            self.rewritten[usize::from(sky)].borrow_mut().extend(cells);
        }
    }
}

#[cfg(test)]
thread_local! {
    /// Checked resolutions of this thread: (kept, handed back to the zone).
    pub(super) static CHECKS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
    /// Time this thread spent checking the fixed point, and in checked resolutions (micro-bench: the
    /// share a cache of consistent sections could save at most).
    pub(super) static CHECK_NS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    pub(super) static CHECKED_NS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Cells the check walked on this thread, and those it looked the faces up for.
    pub(super) static CHECKED_CELLS: std::cell::Cell<(u64, u64)> = const { std::cell::Cell::new((0, 0)) };
}

/// The rows `r0..r0 + ny` of a zone read that one resolution solves (margin rows included).
#[derive(Clone, Copy)]
struct Rows {
    g: Geo,
    r0: i32,
    ny: i32,
}

impl Rows {
    /// Array index of the interior cell at `p` (world coordinates), if it is one.
    fn interior_index(&self, p: [i32; 3]) -> Option<usize> {
        let g = self.g;
        let d = [p[0] - g.lo[0], p[1] - g.lo[1], p[2] - g.lo[2]];
        let inside = (1..g.size[0] - 1).contains(&d[0])
            && (self.r0 + 1..self.r0 + self.ny - 1).contains(&d[1])
            && (1..g.size[2] - 1).contains(&d[2]);
        inside.then(|| g.at(d[0], d[1], d[2]))
    }

    /// Interior rows, as rows of the zone read.
    fn interior(&self) -> Range<i32> {
        self.r0 + 1..self.r0 + self.ny - 1
    }

    /// Whole planes of the rows (sentinel ring included), as an index range.
    fn planes(&self) -> Range<usize> {
        let g = self.g;
        (self.r0 + g.ring) as usize * g.plane..(self.r0 + self.ny + g.ring) as usize * g.plane
    }

    /// `(dx, world y, dz)` of an array index.
    fn coords(&self, i: usize) -> (i32, i32, i32) {
        let g = self.g;
        let (ry, rest) = (i / g.plane, i % g.plane);
        ((rest % g.ax) as i32 - g.ring, g.lo[1] + ry as i32 - g.ring, (rest / g.ax) as i32 - g.ring)
    }

    /// Index offsets of the six neighbours, in the order of `STEP`.
    fn offsets(&self) -> [isize; 6] {
        let (plane, ax) = (self.g.plane as isize, self.g.ax as isize);
        [-plane, plane, -ax, ax, -1, 1]
    }
}

/// `LIGHTINCOVERLAP`: every resolution `(changes, block, sky)` in order, by the checked incremental
/// (groups of more than `LIGHTINCMAX` changes: by the zone), the cells each one rewrites put out again
/// by the later ones.
pub(super) fn run_in_order(grid: &mut impl LightGrid, c: &Ctx, units: &[Unit], out: &mut [Touched]) {
    let overlap = Overlap::of_call(units);
    with_work(c.o.zone_buf, |w| {
        w.aux.memo.clear();
        for (cs, block, sky) in units {
            solve_group_by(grid, c, w, cs, *block, *sky, Resolution::Checked(cs, &overlap), out);
        }
    });
}

/// One resolution of the zone read (rows `r0..r0 + ny` of `g`): the incremental from the changes
/// `cs` (and from what `overlap` brings), kept if the zone holds the fixed point, otherwise the zone's
/// seeding and propagation. A group of more than `LIGHTINCMAX` changes goes to the zone at once.
#[allow(clippy::too_many_arguments)]
pub(super) fn solve(
    grid: &mut impl LightGrid,
    c: &Ctx,
    w: &mut Work,
    g: Geo,
    r0: i32,
    ny: i32,
    sky: bool,
    cs: &[[i32; 3]],
    overlap: &Overlap,
    out: &mut [Touched],
) {
    let t0 = Instant::now();
    let rows = Rows { g, r0, ny };
    let Work { face, prop, emit, emitters, sky: sky_values, blk, aux, lane, checked: s, .. } = w;
    let z = Cells { face, prop, emit, emitters };
    let lay: &mut [u8] = if sky { sky_values } else { blk };
    s.clear();
    let tried = cs.len() <= c.o.inc_max;
    if sky && tried {
        source_heights(&*grid, c, aux, &z, rows, &mut s.sources);
        plane_source_rows(&s.sources, g, &mut s.source_rows);
    }
    if !c.o.seeds_at_read() {
        open_interior(lay, rows);
    }
    // A dark zone (nothing stored, nothing lighting by itself) is its own fixed point: nothing to do.
    let held = tried && (zone_is_dark(c, &z, lay, &s.sources, rows, sky) || incremental_holds(c, &z, lay, s, rows, sky, cs, overlap));
    let touched = &mut out[if sky { out.len() - 1 } else { 0 }];
    if held {
        write_logged(grid, c, lay, s, rows, sky, overlap, touched);
    } else {
        if !sabotage(SABOTAGE_UNDO_FORGOTTEN) {
            undo(lay, &s.undo);
        }
        let known_sources = (sky && tried).then_some(&s.sources[..]);
        if compute(&*grid, c, &z, lay, lane, sky.then_some(aux), known_sources, g, r0, ny, sky) {
            write(grid, c, lay, lane, g, r0, ny, sky, out);
            record_zone_writes(c, lay, &lane.changes, rows, sky, overlap);
        }
    }
    if tried {
        count_check(held);
    }
    #[cfg(test)]
    CHECKED_NS.with(|n| n.set(n.get() + t0.elapsed().as_nanos() as u64));
    count_solve(sky, t0.elapsed());
}

/// Runs the incremental in place, from the changes `cs` or, groups overlapping, from every change of
/// the call; true if the zone then holds the fixed point.
#[allow(clippy::too_many_arguments)]
fn incremental_holds(c: &Ctx, z: &Cells, lay: &mut [u8], s: &mut Scratch, rows: Rows, sky: bool, cs: &[[i32; 3]], overlap: &Overlap) -> bool {
    let every_change = overlap.active && !sabotage(SABOTAGE_OWN_CHANGES_ONLY);
    let starts = if every_change { &overlap.changes[usize::from(sky)][..] } else { cs };
    if !incremental(c, z, lay, s, rows, sky, starts, overlap) {
        return false;
    }
    #[cfg(test)]
    let check_start = Instant::now();
    let held = sabotage(SABOTAGE_CHECK_SKIPPED) || fixed_point_holds(c, z, lay, &s.source_rows, &mut s.given, rows, sky);
    #[cfg(test)]
    CHECK_NS.with(|n| n.set(n.get() + check_start.elapsed().as_nanos() as u64));
    held
}

/// A checked resolution counted in `stats` (and per thread in the tests).
fn count_check(held: bool) {
    use std::sync::atomic::Ordering::Relaxed;
    let counter = if held { &crate::stats::CHECKED_KEPT } else { &crate::stats::CHECKED_FALLBACKS };
    counter.fetch_add(1, Relaxed);
    #[cfg(test)]
    CHECKS.with(|n| {
        let (kept, fallbacks) = n.get();
        n.set(if held { (kept + 1, fallbacks) } else { (kept, fallbacks + 1) });
    });
}

/// Sky source height of every interior stack (`column_source`, the zone's rule), on the thread pool
/// for a large zone.
fn source_heights(grid: &impl LightGrid, c: &Ctx, aux: &mut Aux, z: &Cells, rows: Rows, into: &mut Vec<i32>) {
    let g = rows.g;
    let (sx, sz) = (g.size[0], g.size[2]);
    let columns = SkyColumns::when_cached(grid, c, aux, g);
    if c.o.par_solve && g.cells() >= c.o.par_min {
        *into = par::install(|| par::sources(grid, c, z, aux.sec_geo, columns.as_ref(), g, rows.r0, rows.ny));
        return;
    }
    into.clear();
    into.resize((sx * sz) as usize, i32::MAX);
    for dz in 1..sz - 1 {
        for dx in 1..sx - 1 {
            into[(dz * sx + dx) as usize] = column_source(grid, c, aux, columns.as_ref(), z, g, rows.r0, rows.ny, dx, dz);
        }
    }
}

/// The stacks' source heights as rows of the zone (counted from its bottom margin row, clamped to
/// `u16`: below the zone, every row; out of reach, none), in plane layout `(dz + ring) · ax + dx +
/// ring`, so that the check reads them beside the cells, eight per vector lane group.
fn plane_source_rows(sources: &[i32], g: Geo, into: &mut Vec<u16>) {
    into.clear();
    into.resize(g.plane, u16::MAX);
    for dz in 0..g.size[2] {
        for dx in 0..g.size[0] {
            let row = i64::from(sources[(dz * g.size[0] + dx) as usize]) - i64::from(g.lo[1]);
            into[g.at(dx, -g.ring, dz)] = row.clamp(0, i64::from(u16::MAX)) as u16;
        }
    }
}

/// Opens the interior cells that have a sheet, when the read did not (`LIGHTSEEDATREAD=0`); the
/// zone's seeding opens the same ones if it takes over.
fn open_interior(lay: &mut [u8], rows: Rows) {
    let g = rows.g;
    let n = (g.size[0] - 2) as usize;
    for r in rows.interior() {
        for dz in 1..g.size[2] - 1 {
            let start = g.at(1, r, dz);
            for b in &mut lay[start..start + n] {
                *b |= (*b & DATA) << 1;
            }
        }
    }
}

/// No light stored in the rows (margins included) and no own light inside (no open emitter for the
/// block, no sky source for the sky: `sources` all above the top interior row): every level stays 0.
fn zone_is_dark(c: &Ctx, z: &Cells, lay: &[u8], sources: &[i32], rows: Rows, sky: bool) -> bool {
    if lay[rows.planes()].iter().fold(0, |acc, &b| acc | b) & 15 != 0 {
        return false;
    }
    if sky {
        let top = rows.g.lo[1] + rows.r0 + rows.ny - 2;
        return sources.iter().all(|&from| from > top);
    }
    if c.o.seeds_at_read() {
        // The read listed the emitters of the block's interior rows.
        return z.emitters.iter().all(|&i| lay[i as usize] & OPEN == 0);
    }
    let lit_open = |acc: u8, (&b, &e): (&u8, &u8)| acc | e & 0u8.wrapping_sub((b & OPEN) >> 5);
    lay[rows.planes()].iter().zip(&z.emit[rows.planes()]).fold(0, lit_open) == 0
}

/// Saves the byte of cell `i` before its first change.
#[inline]
fn log(lay: &mut [u8], undo: &mut Vec<(u32, u8)>, i: usize) {
    if lay[i] & LOGGED == 0 {
        undo.push((i as u32, lay[i]));
        lay[i] |= LOGGED;
    }
}

/// Sets the value of cell `i` (byte logged first).
#[inline]
fn set_level(lay: &mut [u8], undo: &mut Vec<(u32, u8)>, i: usize, v: u8) {
    log(lay, undo, i);
    lay[i] = lay[i] & !15 | v;
}

/// Marks the open cell `i` as a starting point of the decrease, once, with the level to put out.
fn remove(lay: &mut [u8], s: &mut Scratch, i: usize, level: u8) {
    if lay[i] & (OPEN | REMOVED) == OPEN {
        log(lay, &mut s.undo, i);
        lay[i] |= REMOVED;
        s.removed.push((i as u32, level));
    }
}

/// The light a neighbour can give: its value when it is lit by the zone's rule (open, or a margin
/// seed: sheet present or full sky above the world; every other cell reads 0). `margin_seeds`: 1, or
/// 0 under the sabotage that leaves them out (read once per row, not per cell).
#[inline(always)]
fn light_given(b: u8, margin_seeds: u8) -> u8 {
    let v = b & 15;
    let seed = ((b & DATA) >> 4 | u8::from(v == 15)) & margin_seeds;
    let open = (b & OPEN) >> 5;
    v & 0u8.wrapping_sub(seed | open)
}

/// The `margin_seeds` of [`light_given`] for the check.
fn margin_seeds_checked() -> u8 {
    u8::from(!sabotage(SABOTAGE_CHECK_IGNORES_MARGIN_SEEDS))
}

/// The own light of the interior cell `i`: its emission, or full sky from its stack's source height.
fn own_light(z: &Cells, sources: &[i32], rows: Rows, i: usize, sky: bool) -> u8 {
    if !sky {
        return z.emit[i];
    }
    let (dx, y, dz) = rows.coords(i);
    15 * u8::from(y >= sources[(dz * rows.g.size[0] + dx) as usize])
}

/// The decrease then increase of `fast/inc.rs`, in place in `lay`: the cells an earlier group rewrote,
/// the changed cells and the sky cells whose source changed are put out with what depended on them,
/// then lit again from what stayed, the sources and the margin. False when it gave up (see
/// [`decrease`]): the zone takes over.
#[allow(clippy::too_many_arguments)]
fn incremental(c: &Ctx, z: &Cells, lay: &mut [u8], s: &mut Scratch, rows: Rows, sky: bool, cs: &[[i32; 3]], overlap: &Overlap) -> bool {
    // The rewritten cells first: a cell is removed once, and theirs is the highest level to put out.
    if overlap.active && !sabotage(SABOTAGE_REWRITTEN_CELLS_KEPT) {
        for &(p, before) in overlap.rewritten[usize::from(sky)].borrow().iter() {
            if let Some(i) = rows.interior_index(p) {
                // From the brighter of the values before and after the rewrite: a darkened cell must
                // still empty the neighbours it lit before.
                let rewritten_value = lay[i] & 15;
                remove(lay, s, i, before.max(rewritten_value));
            }
        }
    }
    for &p in cs {
        if let Some(i) = rows.interior_index(p) {
            let stored = lay[i] & 15;
            remove(lay, s, i, stored);
        }
    }
    if sky && !sabotage(20) {
        remove_changed_sky_sources(lay, s, rows, cs);
    }
    if !decrease(z, lay, s, rows, sky) {
        return false;
    }
    for k in 0..s.reseed.len() {
        let (i, own) = s.reseed[k];
        let i = i as usize;
        if own > lay[i] & 15 {
            set_level(lay, &mut s.undo, i, own);
            if own > 1 {
                s.increase[own as usize].push(i as u64 | ALL_DIRS);
            }
        }
    }
    increase(c, z, lay, s, rows);
    true
}

/// Under each change, the cells whose sky source changed (15 stored against the source height of the
/// states), from the stack's highest change down to the first cell dark on both counts.
fn remove_changed_sky_sources(lay: &mut [u8], s: &mut Scratch, rows: Rows, cs: &[[i32; 3]]) {
    let g = rows.g;
    let (bottom, top) = (g.lo[1] + rows.r0 + 1, g.lo[1] + rows.r0 + rows.ny - 2);
    // The interior stacks of the changes, each with its highest change (sorted: no quadratic search
    // when a call has thousands of changes).
    let mut stacks = std::mem::take(&mut s.stacks);
    stacks.clear();
    stacks.extend(cs.iter().filter(|p| rows.interior_index([p[0], bottom, p[2]]).is_some()).map(|p| (p[0], p[2], p[1])));
    stacks.sort_unstable();
    for stack in stacks.chunk_by(|a, b| (a.0, a.1) == (b.0, b.1)) {
        let (x, z) = (stack[0].0, stack[0].1);
        let highest = if sabotage(23) { stack[0].2 } else { stack[stack.len() - 1].2 };
        let source_from = s.sources[((z - g.lo[2]) * g.size[0] + x - g.lo[0]) as usize];
        for y in (bottom..=highest.min(top)).rev() {
            let i = g.at(x - g.lo[0], y - g.lo[1], z - g.lo[2]);
            if lay[i] & OPEN == 0 {
                continue;
            }
            let stored = lay[i] & 15;
            let (source_now, source_stored) = (y >= source_from, stored == 15);
            if source_now != source_stored {
                remove(lay, s, i, stored);
            } else if !source_now {
                break;
            }
        }
    }
    s.stacks = stacks;
}

/// One emptied cell in this many of the zone's interior and the incremental gives up: emptied then lit
/// again, plus the check, such a share costs about the zone's own propagation, so this bounds what an
/// incremental the zone has to redo can waste. No micro-bench trace reaches it (the 16 × 16 slab
/// placed in the air stays below); kept as a bound, to measure on the scene bench.
const GIVE_UP_SHARE: usize = 8;

/// Empties the removed cells, then each lit neighbour darker than what was put out (game rule,
/// level − 1, no state read: may empty too much, the increase restores it); a neighbour at least
/// as bright, or a lit margin cell, lights the emptied cell back in the increase. False, half done,
/// once more than one interior cell in [`GIVE_UP_SHARE`] is emptied (`LIGHTINCGIVEUP=0`: never).
fn decrease(z: &Cells, lay: &mut [u8], s: &mut Scratch, rows: Rows, sky: bool) -> bool {
    let g = rows.g;
    let interior = ((g.size[0] - 2) * (g.size[2] - 2) * (rows.ny - 2)) as usize;
    let budget = if rc_data::opt_switch!("RUSTCRAFT_OPT_LIGHTINCGIVEUP") { interior / GIVE_UP_SHARE } else { usize::MAX };
    for k in 0..s.removed.len() {
        let (i, level) = s.removed[k];
        let i = i as usize;
        set_level(lay, &mut s.undo, i, 0);
        s.decrease.push(i as u64 | u64::from(level) << 32 | 63 << 36);
        let own = own_light(z, &s.sources, rows, i, sky);
        if own > 0 {
            s.reseed.push((i as u32, own));
        }
    }
    let offsets = rows.offsets();
    let mut k = 0;
    while k < s.decrease.len() {
        if s.undo.len() > budget {
            return false;
        }
        let e = s.decrease[k];
        k += 1;
        let i = (e as u32) as usize;
        let put_out = (e >> 32 & 15) as u8;
        let dirs = (e >> 36) as u8;
        for (dir, off) in offsets.iter().enumerate() {
            if dirs >> dir & 1 == 0 {
                continue;
            }
            let j = i.wrapping_add_signed(*off);
            let (b, lj) = (lay[j], lay[j] & 15);
            if lj == 0 {
                continue;
            }
            let back = 1u64 << OPPOSITE[dir];
            if b & OPEN == 0 {
                // Frozen margin: seed of the increase towards the emptied cell.
                if light_given(b, 1) > 1 {
                    s.increase[lj as usize].push(j as u64 | back << 32);
                }
                continue;
            }
            if lj < put_out {
                set_level(lay, &mut s.undo, j, 0);
                let own = if sky || sabotage(21) { 0 } else { z.emit[j] };
                // A value explained by its own emission: what it lights stays right.
                if own < lj {
                    s.decrease.push(j as u64 | u64::from(lj) << 32 | (63 & !back) << 36);
                }
                if own > 0 {
                    s.reseed.push((j as u32, own));
                }
            } else if lj > 1 {
                s.increase[lj as usize].push(j as u64 | back << 32);
            }
        }
    }
    true
}

/// Lights the open cells by buckets of decreasing level (each cell gets its final value once), the
/// remaining directions kept in the entry.
fn increase(c: &Ctx, z: &Cells, lay: &mut [u8], s: &mut Scratch, rows: Rows) {
    let (t, tab) = (c.t, c.o.occ_tab);
    let offsets = rows.offsets();
    for l in (2..16usize).rev() {
        while let Some(e) = s.increase[l].pop() {
            let i = (e as u32) as usize;
            if (lay[i] & 15) as usize != l {
                continue;
            }
            let dirs = (e >> 32) as u8;
            let face_i = z.face[i];
            for (dir, off) in offsets.iter().enumerate() {
                if dirs >> dir & 1 == 0 {
                    continue;
                }
                let j = i.wrapping_add_signed(*off);
                let (b, lj) = (lay[j], lay[j] & 15);
                if b & OPEN == 0 || l as u8 - 1 <= lj {
                    continue;
                }
                let new_level = l as i32 - i32::from(z.prop[j] & 15);
                if new_level <= i32::from(lj) || t.occluded_faces(face_i, z.face[j], dir, tab) {
                    continue;
                }
                set_level(lay, &mut s.undo, j, new_level as u8);
                if new_level > 1 {
                    s.increase[new_level as usize].push(j as u64 | (63 & !(1u64 << OPPOSITE[dir])) << 32);
                }
            }
        }
    }
}

/// Every open cell of the interior rows holds the zone's equation (see the module header).
/// `source_rows`: for the sky, each stack's source height as a row of the zone, in plane layout;
/// `given`: scratch for [`light_and_shape_bytes`].
#[allow(clippy::too_many_arguments)]
fn fixed_point_holds(c: &Ctx, z: &Cells, lay: &[u8], source_rows: &[u16], given: &mut Vec<u8>, rows: Rows, sky: bool) -> bool {
    light_and_shape_bytes(z, lay, rows, margin_seeds_checked(), given);
    let given = &given[..];
    let plane_holds = |r: i32| plane_holds(c, z, lay, given, source_rows, rows, r, sky);
    if c.o.par_solve && rows.g.cells() >= c.o.par_min {
        // Planes on the thread pool; `all` stops the others at the first plane that fails.
        return par::install(|| rows.interior().into_par_iter().all(plane_holds));
    }
    rows.interior().all(plane_holds)
}

/// One byte per cell of the rows' planes, read by the check for each neighbour: the light the cell can
/// give (bits 4 to 7, see [`light_given`]) and whether it has an occluding shape (bit 0). Computed once
/// (the incremental has finished), where each cell would otherwise be decoded six times, once per
/// neighbour; the brightest neighbour is then the largest of six bytes shifted down, and a shape
/// around the OR of seven.
fn light_and_shape_bytes(z: &Cells, lay: &[u8], rows: Rows, margin_seeds: u8, into: &mut Vec<u8>) {
    let planes = rows.planes();
    if into.len() < planes.end {
        into.resize(planes.end, 0);
    }
    let (out, cells, props) = (&mut into[planes.clone()], &lay[planes.clone()], &z.prop[planes]);
    for ((o, &b), &p) in out.iter_mut().zip(cells).zip(props) {
        *o = light_given(b, margin_seeds) << 4 | (p & SHAPED) >> 5;
    }
}

/// Cells of a plane checked per pass of the vectorised loop (stack buffers of that many bytes).
const CHECK_CHUNK: usize = 256;

/// The open cells of the interior plane `r` hold their equation ([`plane_holds_body`]). With
/// `RUSTCRAFT_OPT_LIGHTCHECKAVX2=1`, compiled for AVX2 when the processor has it (32 cells per vector
/// instruction instead of 16; the release targets plain x86-64). Off by default: slower in the
/// micro-bench (0.74 against 0.53 ns per cell walked, plains, two passes each, loaded machine), kept
/// for the quiet-machine bench.
#[allow(unsafe_code, clippy::too_many_arguments)]
fn plane_holds(c: &Ctx, z: &Cells, lay: &[u8], given: &[u8], source_rows: &[u16], rows: Rows, r: i32, sky: bool) -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        #[target_feature(enable = "avx2")]
        #[allow(clippy::too_many_arguments)]
        fn avx2(c: &Ctx, z: &Cells, lay: &[u8], given: &[u8], source_rows: &[u16], rows: Rows, r: i32, sky: bool) -> bool {
            plane_holds_body(c, z, lay, given, source_rows, rows, r, sky)
        }
        if rc_data::opt_switch!("RUSTCRAFT_OPT_LIGHTCHECKAVX2", default_off) && rc_data::cpu::has_avx2() {
            // SAFETY: the shared detection found AVX2 on this processor.
            return unsafe { avx2(c, z, lay, given, source_rows, rows, r, sky) };
        }
    }
    plane_holds_body(c, z, lay, given, source_rows, rows, r, sky)
}

/// Body of [`plane_holds`]. The interior of a plane is one stretch of the array, from its first
/// interior cell to its last: it is walked in one loop (the side margins and ring cells it also holds
/// are not open, so not checked), long enough for the vectorised loop to pay, where a zone row is only
/// `sx - 2` cells. Each chunk of the stretch is checked without the faces first ([`chunk_without_faces`]);
/// the cells where the faces may matter are then checked one by one with them ([`flagged_cells_hold`]).
/// Plain loops, no closure around the vectorised one, so that all of it is compiled with the body's
/// instruction set.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn plane_holds_body(c: &Ctx, z: &Cells, lay: &[u8], given: &[u8], source_rows: &[u16], rows: Rows, r: i32, sky: bool) -> bool {
    let g = rows.g;
    let (mut start, end) = (g.at(1, r, 1), g.at(g.size[0] - 2, r, g.size[2] - 2) + 1);
    let plane_start = (r + g.ring) as usize * g.plane;
    // Rows of the zone count from its bottom margin, as the source rows do.
    let row = r as u16;
    let sky_sources = u8::from(!sabotage(SABOTAGE_CHECK_WITHOUT_SKY_SOURCE));
    while start < end {
        let n = CHECK_CHUNK.min(end - start);
        let (mut own, mut flags) = ([0u8; CHECK_CHUNK], [0u8; CHECK_CHUNK]);
        let (own, faces_matter) = (&mut own[..n], &mut flags[..n]);
        if sky {
            for (o, &from) in own.iter_mut().zip(&source_rows[start - plane_start..start - plane_start + n]) {
                *o = 15 * (u8::from(row >= from) & sky_sources);
            }
        } else {
            own.copy_from_slice(&z.emit[start..start + n]);
        }
        let (held, any_faces) = chunk_without_faces(lay, z.prop, given, g, start, own, faces_matter);
        #[cfg(test)]
        count_checked_cells(faces_matter, any_faces);
        if !held || (any_faces && !flagged_cells_hold(c, z, lay, rows, start, own, faces_matter)) {
            return false;
        }
        start += n;
    }
    true
}

/// The cells from `start` (as many as `own` and `faces_matter` hold), faces ignored: `(every open cell
/// where the faces cannot matter holds its equation, the faces may matter somewhere)`, `faces_matter[k]`
/// set for the open cells where they may. A face can only take a neighbour's light away, so it matters
/// only where a shape is around (the cell or a neighbour has one) and some neighbour brings more than
/// the cell's own light; elsewhere the bound without faces is the exact value. `own[k]`: the own light
/// of the `k`-th cell; `given`: see [`light_and_shape_bytes`].
#[inline(always)]
fn chunk_without_faces(lay: &[u8], prop: &[u8], given: &[u8], g: Geo, start: usize, own: &[u8], faces_matter: &mut [u8]) -> (bool, bool) {
    let n = faces_matter.len();
    let (cells, props, own) = (&lay[start..start + n], &prop[start..start + n], &own[..n]);
    // Seven slices of equal length read side by side: no bounds test left, the compiler vectorises.
    let [itself, west, east, north, south, below, above] = cells_and_neighbours(given, g, start, n);
    let (mut wrong, mut any_faces) = (0u8, 0u8);
    for k in 0..n {
        let largest = west[k].max(east[k]).max(north[k]).max(south[k]).max(below[k]).max(above[k]);
        let shape_around = (itself[k] | west[k] | east[k] | north[k] | south[k] | below[k] | above[k]) & 1;
        let from_neighbours = (largest >> 4).saturating_sub(props[k] & 15);
        let open = (cells[k] & OPEN) >> 5;
        let may_matter = open & shape_around & u8::from(from_neighbours > own[k]);
        faces_matter[k] = may_matter;
        any_faces |= may_matter;
        wrong |= open & (may_matter ^ 1) & u8::from(from_neighbours.max(own[k]) != cells[k] & 15);
    }
    (wrong == 0, any_faces != 0)
}

/// The `n` cells from `start`, then the same cells shifted to each of their six neighbours (west,
/// east, north, south, below, above).
fn cells_and_neighbours(v: &[u8], g: Geo, start: usize, n: usize) -> [&[u8]; 7] {
    let (ax, plane) = (g.ax, g.plane);
    [start, start - 1, start + 1, start - ax, start + ax, start - plane, start + plane].map(|from| &v[from..from + n])
}

/// The cells of a chunk flagged in `faces_matter` hold their equation, faces included.
#[inline(never)]
fn flagged_cells_hold(c: &Ctx, z: &Cells, lay: &[u8], rows: Rows, start: usize, own: &[u8], faces_matter: &[u8]) -> bool {
    let margin_seeds = margin_seeds_checked();
    let way = if sabotage(SABOTAGE_CHECK_FACES_THE_WRONG_WAY) { OPPOSITE } else { [0, 1, 2, 3, 4, 5] };
    let flagged = faces_matter.iter().enumerate().filter(|(_, f)| **f != 0);
    flagged.into_iter().all(|(k, _)| cell_holds(c, z, lay, rows, start + k, own[k], margin_seeds, way))
}

/// Cells the check walked and those it looked the faces up for (tests and micro-benches).
#[cfg(test)]
fn count_checked_cells(faces_matter: &[u8], any_faces: bool) {
    let flagged = if any_faces { faces_matter.iter().filter(|f| **f != 0).count() as u64 } else { 0 };
    CHECKED_CELLS.with(|c| {
        let (cells, with_faces) = c.get();
        c.set((cells + faces_matter.len() as u64, with_faces + flagged));
    });
}

/// The open cell `i` (own light `own`) holds its equation, faces included. `way[dir]`: the direction
/// whose faces are looked up for a step in direction `dir` (itself, except under a sabotage).
#[allow(clippy::too_many_arguments)]
fn cell_holds(c: &Ctx, z: &Cells, lay: &[u8], rows: Rows, i: usize, own: u8, margin_seeds: u8, way: [usize; 6]) -> bool {
    let (t, tab) = (c.t, c.o.occ_tab);
    let opacity = i32::from(z.prop[i] & 15);
    let mut expected = i32::from(own);
    for (dir, off) in rows.offsets().iter().enumerate() {
        // The neighbour from which a step in direction `dir` reaches the cell; its faces are only
        // looked up when its light would raise the value.
        let from = i.wrapping_add_signed(-off);
        let brought = i32::from(light_given(lay[from], margin_seeds)) - opacity;
        if brought > expected && !t.occluded_faces(z.face[from], z.face[i], way[dir], tab) {
            expected = brought;
        }
    }
    expected.max(0) == i32::from(lay[i] & 15)
}

/// Writes the logged cells whose value changed (the zone's changed cells, once the check held) and
/// clears the bits of the log.
#[allow(clippy::too_many_arguments)]
fn write_logged(
    grid: &mut impl LightGrid,
    c: &Ctx,
    lay: &mut [u8],
    s: &mut Scratch,
    rows: Rows,
    sky: bool,
    overlap: &Overlap,
    touched: &mut Touched,
) {
    let g = rows.g;
    s.changed.clear();
    for &(i, before) in &s.undo {
        let i = i as usize;
        lay[i] &= !(LOGGED | REMOVED);
        let v = lay[i] & 15;
        if v != before & 15 {
            let (ry, rest) = (i / g.plane, i % g.plane);
            s.changed.push((rest as u32, g.lo[1] + ry as i32 - g.ring, v));
        }
    }
    overlap.record(
        sky,
        s.undo.iter().filter(|&&(i, before)| lay[i as usize] & 15 != before & 15).map(|&(i, before)| {
            let (dx, y, dz) = rows.coords(i as usize);
            ([g.lo[0] + dx, y, g.lo[2] + dz], before & 15)
        }),
    );
    inc::write_cells(grid, c, &mut s.changed, &mut s.changes, g, sky, touched);
}

/// Puts back the bytes of the logged cells.
fn undo(lay: &mut [u8], log: &[(u32, u8)]) {
    for &(i, before) in log {
        lay[i as usize] = before;
    }
}

/// The cells the zone just wrote (`changes`, found by `change_scan`), kept with their value before
/// for the later groups (`LIGHTINCOVERLAP`; not kept by the old stack-by-stack write).
fn record_zone_writes(c: &Ctx, lay: &[u8], changes: &change_scan::Changes, rows: Rows, sky: bool, overlap: &Overlap) {
    if !overlap.active || !(c.o.scan_by_planes || c.o.keep_column_while_writing) {
        return;
    }
    let g = rows.g;
    let mut start = 0;
    let mut cells = Vec::new();
    for &(x, z, end) in &changes.stacks {
        for &(y, _) in &changes.cells[start..end] {
            let before = lay[g.at(x - g.lo[0], y - g.lo[1], z - g.lo[2])] & 15;
            cells.push(([x, y, z], before));
        }
        start = end;
    }
    overlap.record(sky, cells.into_iter());
}
