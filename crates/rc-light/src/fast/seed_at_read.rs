//! Seeding prepared by the zone read (`LIGHTSEEDATREAD`, mission C2-M11 of series 3, `docs/perf.md`):
//! the same levels, opened cells and early stop as the seeding of `fast/frontier.rs`, so the same
//! fixed point; only the work moves.
//!
//! The seeding walked every interior cell of a resolution to open it (sheet present) and give it its
//! starting value, although nearly all of them start at 0: no emitter for the block, no sky below the
//! source heights. The read already writes each of those cells, section by section, and knows from
//! the section's palette whether any of them can emit. So:
//! - the read opens the interior cells of the rows each sheet's resolution will solve (one OR per
//!   byte, plane by plane once the zone is read, `LIGHTOPENAFTERREAD`; or on each section's segments
//!   as they are written), and lists the block's emitting interior cells, only in the sections whose
//!   states can emit;
//! - the seeding zeroes the levels of its rows plane by plane, gives their value to the listed
//!   emitters (block) or to each stack's cells from its source height up (sky), then copies the
//!   margins run by run (a vectorised copy), the lit margin cells alone going through the queue test;
//! - the early stop of the block (`BLOCKEARLY`) still needs the old light stored inside (light to put
//!   out): an OR over the rows' bytes, only when nothing else is lit.

use std::ops::Range;

use super::frontier::{SABOTAGE_MARGIN_15_NOT_QUEUED, StackSeed, raises};
use super::*;

/// Deliberate errors of the tests (the equivalence proofs must see them).
pub(super) const SABOTAGE_EMITTER_LOST_AT_SECTION_TOP: u8 = 50;
pub(super) const SABOTAGE_STORED_LIGHT_IGNORED: u8 = 51;
pub(super) const SABOTAGE_BLOCK_MARGIN_ROW_OPENED: u8 = 52;

/// The cell is opened by setting `OPEN` from `DATA`, one shift with no branch.
const _: () = assert!(OPEN == DATA << 1);

/// What the read prepares, in world coordinates: the interior stacks of the zone and, for each sheet,
/// the interior rows its resolution solves (empty when the sheet is not read).
pub(super) struct OpenPlan {
    x: Range<i32>,
    z: Range<i32>,
    sky_rows: Range<i32>,
    block_rows: Range<i32>,
    /// The cells are opened segment by segment while each section is read (`LIGHTOPENAFTERREAD=0`),
    /// otherwise plane by plane once the zone is read.
    open_while_reading: bool,
}

impl OpenPlan {
    pub(super) fn new(o: &Opts, g: Geo, sky: Option<SolvedRows>, block: Option<SolvedRows>) -> Self {
        let interior = |rows: Option<SolvedRows>, margin_opened: bool| match rows {
            Some(r) => g.lo[1] + r.first + i32::from(!margin_opened)..g.lo[1] + r.first + r.count - 1,
            None => 0..0,
        };
        OpenPlan {
            x: g.lo[0] + 1..g.lo[0] + g.size[0] - 1,
            z: g.lo[2] + 1..g.lo[2] + g.size[2] - 1,
            sky_rows: interior(sky, false),
            block_rows: interior(block, sabotage(SABOTAGE_BLOCK_MARGIN_ROW_OPENED)),
            open_while_reading: !o.open_after_read,
        }
    }
}

/// Opens the solved interior cells that have a sheet once the whole zone is read
/// (`LIGHTOPENAFTERREAD`), plane by plane: the interior of a plane is one stretch of the array, from
/// its first interior cell to its last, so it is opened in one loop (the sections give segments of
/// 16 cells at most, the zone rows of `sx - 2`, each with its own loop to start). The stretch also
/// holds the two side margins of its rows, closed again after; its ring cells hold no sheet.
pub(super) fn open_after_read(plan: &OpenPlan, g: Geo, sky: &mut [u8], block: &mut [u8]) {
    if plan.open_while_reading || plan.x.is_empty() || plan.z.is_empty() {
        return;
    }
    let (sx, sz) = (g.size[0], g.size[2]);
    for (values, rows) in [(sky, &plan.sky_rows), (block, &plan.block_rows)] {
        for y in rows.clone() {
            let r = y - g.lo[1];
            open_cells(&mut values[g.at(1, r, 1)..=g.at(sx - 2, r, sz - 2)]);
            for dz in 1..sz - 1 {
                values[g.at(0, r, dz)] &= !OPEN;
                values[g.at(sx - 1, r, dz)] &= !OPEN;
            }
        }
    }
}

/// The cells read from `source` can emit light (`palette_emits`: an entry of the section's palette does).
pub(super) fn may_emit(source: &CellSource, palette_emits: bool) -> bool {
    match source {
        CellSource::Fixed(p) => p.2 > 0,
        CellSource::Palette => palette_emits,
        CellSource::Values | CellSource::Cells(_) => true,
    }
}

/// The intersection of two ranges (empty when they do not meet).
fn overlap(a: Range<i32>, b: &Range<i32>) -> Range<i32> {
    a.start.max(b.start)..a.end.min(b.end)
}

/// After the read of the cells `xs × ys × zs` (inside one section of one column): lists the block
/// emitters among those of the solved interior rows and, with `LIGHTOPENAFTERREAD=0`, opens those
/// that have a sheet.
pub(super) fn prepare_section(
    out: &mut ZoneSlice,
    g: Geo,
    xs: Range<i32>,
    ys: Range<i32>,
    zs: Range<i32>,
    may_emit: bool,
) {
    let Some(plan) = out.plan else { return };
    let (xs, zs) = (overlap(xs, &plan.x), overlap(zs, &plan.z));
    if xs.is_empty() || zs.is_empty() {
        return;
    }
    if out.want_sky && plan.open_while_reading {
        let rows = SliceRows::new(g, out.base, &xs, overlap(ys.clone(), &plan.sky_rows), &zs);
        rows.for_each(|_, start, len| open_cells(&mut out.sky[start..start + len]));
    }
    if out.want_blk {
        let rows = SliceRows::new(g, out.base, &xs, overlap(ys, &plan.block_rows), &zs);
        if plan.open_while_reading {
            rows.for_each(|_, start, len| open_cells(&mut out.blk[start..start + len]));
        }
        if may_emit {
            list_emitters(out.emit, out.base, &rows, out.emitters);
        }
    }
}

/// Row segments of a box of the zone inside an output slice: `x` consecutive cells per row `(y, z)`.
struct SliceRows {
    /// Slice index of the first cell of the first row.
    first: usize,
    len: usize,
    ys: Range<i32>,
    rows_z: usize,
    plane: usize,
    ax: usize,
}

impl SliceRows {
    fn new(g: Geo, base: usize, xs: &Range<i32>, ys: Range<i32>, zs: &Range<i32>) -> Self {
        let first = if ys.is_empty() { 0 } else { g.at(xs.start - g.lo[0], ys.start - g.lo[1], zs.start - g.lo[2]) - base };
        SliceRows { first, len: xs.len(), ys, rows_z: zs.len(), plane: g.plane, ax: g.ax }
    }

    /// Calls `f(y, start, len)` for each row segment (none when the rows are empty).
    fn for_each(&self, mut f: impl FnMut(i32, usize, usize)) {
        for (iy, y) in self.ys.clone().enumerate() {
            for iz in 0..self.rows_z {
                f(y, self.first + iy * self.plane + iz * self.ax, self.len);
            }
        }
    }
}

/// Opens the cells that have a sheet: `OPEN` set from `DATA` (a loop the compiler vectorises).
fn open_cells(values: &mut [u8]) {
    for b in values {
        *b |= (*b & DATA) << 1;
    }
}

/// Appends the zone index of every emitting cell of `rows` to `emitters`.
fn list_emitters(emit: &[u8], base: usize, rows: &SliceRows, emitters: &mut Vec<u32>) {
    rows.for_each(|y, start, len| {
        if sabotage(SABOTAGE_EMITTER_LOST_AT_SECTION_TOP) && y & 15 == 15 {
            return;
        }
        for (k, &e) in emit[start..start + len].iter().enumerate() {
            if e != 0 {
                emitters.push((base + start + k) as u32);
            }
        }
    });
}

/// Seeding of a resolution over the rows `r0..r0 + ny` of a zone read that prepared it (the stacks'
/// source heights already in `lane.stacks`). Same return as the old seeding: false when the block
/// has neither value nor emitter to spread or put out.
#[allow(clippy::too_many_arguments)]
pub(super) fn seed(c: &Ctx, z: &Cells, lay: &[u8], lane: &mut Lane, g: Geo, r0: i32, ny: i32, sky: bool) -> bool {
    zero_levels(&mut lane.level, g, r0, ny);
    let interior = if sky { seed_sky_sources(lay, lane, g, r0, ny) } else { seed_emitters(z, lay, lane) };
    let lit = interior | seed_margins_by_runs(c, z, lay, lane, g, r0, ny);
    // Neither value nor emitter: everything stays at 0, nothing to write. The old light stored
    // inside counts too (it has to be put out); it is only looked at when nothing else is lit.
    sky || !c.o.block_early || lit != 0 || stored_light(lay, g, r0, ny) != 0
}

/// Whole planes of the rows `r0..r0 + ny` (sentinel ring included) as an index range.
fn planes(g: Geo, r0: i32, ny: i32) -> Range<usize> {
    (r0 + g.ring) as usize * g.plane..(r0 + ny + g.ring) as usize * g.plane
}

/// Levels of the rows zeroed at once: the cells that start dark are then left alone.
fn zero_levels(level: &mut [u8], g: Geo, r0: i32, ny: i32) {
    level[planes(g, r0, ny)].fill(0);
}

/// The listed block emitters that the read opened: their emission, queued above 1. Returns the
/// emissions OR-ed.
fn seed_emitters(z: &Cells, lay: &[u8], lane: &mut Lane) -> u8 {
    let Lane { level, buckets, .. } = lane;
    let mut lit = 0;
    for &i in z.emitters {
        let i = i as usize;
        // No sheet: the cell was not opened and stays dark, like in the old seeding.
        if lay[i] & OPEN == 0 {
            continue;
        }
        let v = z.emit[i];
        level[i] = v;
        lit |= v;
        if v > 1 {
            buckets[v as usize].push(i as u64 | ALL_DIRS);
        }
    }
    lit
}

/// Full sky in each interior stack's opened cells from its source height up, then the queue: only
/// the sources below their stack's limit (rows below the lowest source height of the zone skipped).
/// Returns 15 when a cell got it.
fn seed_sky_sources(lay: &[u8], lane: &mut Lane, g: Geo, r0: i32, ny: i32) -> u8 {
    let (sx, sz) = (g.size[0], g.size[2]);
    let Lane { level, buckets, stacks, .. } = lane;
    let lowest = interior_stacks(stacks, sx, sz).map(|s| s.source_from).min().unwrap_or(i32::MAX);
    let (first_row, end_row) = ((r0 + 1).max(lowest.saturating_sub(g.lo[1])), r0 + ny - 1);
    let mut lit = 0;
    for r in first_row..end_row {
        let y = g.lo[1] + r;
        for dz in 1..sz - 1 {
            let (row, first) = (g.at(1, r, dz), (dz * sx + 1) as usize);
            let (cells, levels) = (&lay[row..row + sx as usize - 2], &mut level[row..row + sx as usize - 2]);
            // Branch-free, so the compiler can vectorise the row: 15 where the cell is open and
            // its stack's source height reached, 0 elsewhere (already 0).
            for ((l, &b), stack) in levels.iter_mut().zip(cells).zip(&stacks[first..]) {
                *l = 15 * u8::from(y >= stack.source_from && b & OPEN != 0);
                lit |= *l;
            }
        }
    }
    // The queue: each stack's sources from its source height up to its limit, a cell or two.
    for dz in 1..sz - 1 {
        for dx in 1..sx - 1 {
            let stack = stacks[(dz * sx + dx) as usize];
            let from = (stack.source_from.saturating_sub(g.lo[1])).max(first_row);
            let to = (stack.queue_below.saturating_sub(g.lo[1])).min(end_row);
            for r in from..to {
                let i = g.at(dx, r, dz);
                if level[i] == 15 {
                    buckets[15].push(i as u64 | ALL_DIRS);
                }
            }
        }
    }
    lit
}

/// The interior stacks' seeding records (x and z from 1 to size - 2).
fn interior_stacks(stacks: &[StackSeed], sx: i32, sz: i32) -> impl Iterator<Item = &StackSeed> {
    (1..sz - 1).flat_map(move |dz| &stacks[(dz * sx + 1) as usize..(dz * sx + sx - 1) as usize])
}

/// Which cells of a margin run have an interior neighbour, and at which index offset.
#[derive(Clone, Copy)]
enum InteriorNeighbour {
    /// None of them (the corner rows of the bottom and top planes).
    None,
    /// All but the first and the last cell of the run.
    Middle(isize),
    /// Every cell of the run.
    All(isize),
}

/// Margin cells (stored value kept, never opened), run by run: the bottom and top planes row by row,
/// then the four sides of each interior row, as in `seed_margins`. Returns their values OR-ed.
fn seed_margins_by_runs(c: &Ctx, z: &Cells, lay: &[u8], lane: &mut Lane, g: Geo, r0: i32, ny: i32) -> u8 {
    let (sx, sz) = (g.size[0], g.size[2]);
    let (plane, ax) = (g.plane as isize, g.ax as isize);
    let (bottom, top) = (r0, r0 + ny - 1);
    let mut lit = 0;
    let mut run = |first: usize, count: i32, stride: usize, inner: InteriorNeighbour| {
        lit |= seed_margin_run(c, z, lay, lane, first, count as usize, stride, inner);
    };
    for (r, toward_interior) in [(bottom, plane), (top, -plane)] {
        for dz in 0..sz {
            let inner = if (1..sz - 1).contains(&dz) { InteriorNeighbour::Middle(toward_interior) } else { InteriorNeighbour::None };
            run(g.at(0, r, dz), sx, 1, inner);
        }
    }
    for r in bottom + 1..top {
        run(g.at(0, r, 0), sx, 1, InteriorNeighbour::Middle(ax));
        run(g.at(0, r, sz - 1), sx, 1, InteriorNeighbour::Middle(-ax));
        run(g.at(0, r, 1), sz - 2, g.ax, InteriorNeighbour::All(1));
        run(g.at(sx - 1, r, 1), sz - 2, g.ax, InteriorNeighbour::All(-1));
    }
    lit
}

/// `count` margin cells from `first`, `stride` apart: their stored values copied into the levels
/// first (branch-free, vectorised when the cells are consecutive), then, if any is above 1, each lit
/// one queued like in `seed_margin_cell`. Returns the values OR-ed.
#[allow(clippy::too_many_arguments)]
fn seed_margin_run(
    c: &Ctx,
    z: &Cells,
    lay: &[u8],
    lane: &mut Lane,
    first: usize,
    count: usize,
    stride: usize,
    inner: InteriorNeighbour,
) -> u8 {
    let mut lit = 0;
    if stride == 1 {
        for (l, &b) in lane.level[first..first + count].iter_mut().zip(&lay[first..first + count]) {
            *l = b & 15;
            lit |= b & 15;
        }
    } else {
        for k in 0..count {
            let i = first + k * stride;
            lane.level[i] = lay[i] & 15;
            lit |= lay[i] & 15;
        }
    }
    // Every value at 0 or 1: nothing to queue.
    if lit <= 1 {
        return lit;
    }
    for k in 0..count {
        let i = first + k * stride;
        let (b, v) = (lay[i], lay[i] & 15);
        if v <= 1 || (b & DATA == 0 && v != 15) {
            continue;
        }
        let j = match inner {
            InteriorNeighbour::None => None,
            InteriorNeighbour::Middle(offset) => (1..count - 1).contains(&k).then(|| i.wrapping_add_signed(offset)),
            InteriorNeighbour::All(offset) => Some(i.wrapping_add_signed(offset)),
        };
        let useful = !c.o.margin_frontier || j.is_some_and(|j| raises(z, lay, &lane.level, v, j));
        let dropped = v == 15 && sabotage(SABOTAGE_MARGIN_15_NOT_QUEUED);
        if useful && !dropped {
            lane.buckets[v as usize].push(i as u64 | ALL_DIRS);
        }
    }
    lit
}

/// Stored values of the rows `r0..r0 + ny` OR-ed (old light to put out), over whole planes: the
/// sentinel ring cells hold 0, and the opened and sheet bits are masked off.
fn stored_light(lay: &[u8], g: Geo, r0: i32, ny: i32) -> u8 {
    if sabotage(SABOTAGE_STORED_LIGHT_IGNORED) {
        return 0;
    }
    lay[planes(g, r0, ny)].iter().fold(0, |acc, &b| acc | b) & 15
}
