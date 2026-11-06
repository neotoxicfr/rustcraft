//! Seeding of a zone resolution without the queue entries that cannot raise anything (mission
//! C2-M02, `docs/perf.md`). The levels and opened cells are those of the old seeding, so the fixed
//! point is the same; only the queue is shorter. An entry whose every neighbour already holds,
//! once the seeding is done, at least what the entry would bring it does nothing when spread
//! (levels only rise): leaving it out is exact.
//!
//! - `LIGHTSKYFRONTIER`: in a stack, the sky sources (level 15) run from its source height up. The
//!   cells above and below a source are sources too, or closed (margin row, no sheet), except the
//!   cell under the lowest source; a lateral interior neighbour is not a source under its own
//!   source height; a margin stack is never opened. So a source is only queued below the highest of
//!   its own source height + 1 and the source heights of its four interior neighbours. The redstone
//!   trace seeded 74,636 sources at 15 per relight (C2-M01 counters).
//! - `LIGHTMARGINFRONTIER`: a margin cell keeps its stored value and is never opened; the only
//!   neighbour it can raise is the interior cell next to it (none for the corner stacks). Seeded
//!   after the interior, it is only queued if its value minus that cell's opacity is above the
//!   cell's seeded level. Sky and block.
//! - `LIGHTSEEDROWS`: the interior is walked row by row (consecutive cells) instead of stack by
//!   stack (one plane apart from one cell to the next); the source heights are computed first.

use super::*;

/// Sky seeding of a stack of the zone.
#[derive(Clone, Copy)]
pub(super) struct StackSeed {
    /// Height from which the stack receives full sky (`i32::MAX`: never; margin stacks, block light).
    source_from: i32,
    /// Sources at this height or above only have sources or closed cells around: not queued.
    queue_below: i32,
}

const NO_SKY: StackSeed = StackSeed { source_from: i32::MAX, queue_below: i32::MAX };

/// Deliberate errors of the tests (the equivalence proofs must see them).
pub(super) const SABOTAGE_FORGETS_LOWEST_SOURCE: u8 = 30;
pub(super) const SABOTAGE_FORGETS_HIGHER_NEIGHBOUR: u8 = 31;
pub(super) const SABOTAGE_MARGIN_15_NOT_QUEUED: u8 = 32;

/// The old seeding (same levels, opened cells and return value) with the switches above.
#[allow(clippy::too_many_arguments)]
pub(super) fn seed(
    grid: &impl LightGrid,
    c: &Ctx,
    z: &Cells,
    lay: &mut [u8],
    lane: &mut Lane,
    aux: Option<&mut Aux>,
    pre: Option<&[i32]>,
    g: Geo,
    r0: i32,
    ny: i32,
    sky: bool,
) -> bool {
    debug_assert!(ny >= 3, "a resolution has a margin row at each end");
    prepare_lane(lane, g.cells(), c.o.zone_buf);
    stack_seeds(grid, c, z, &mut lane.stacks, aux, pre, g, r0, ny, sky);
    let interior = if c.o.seed_rows {
        seed_interior_by_rows(z, lay, lane, g, r0, ny, sky)
    } else {
        seed_interior_by_stacks(z, lay, lane, g, r0, ny, sky)
    };
    let any = interior | seed_margins(c, z, lay, lane, g, r0, ny);
    // Neither value nor emitter: everything stays at 0, nothing to write.
    sky || !c.o.block_early || any != 0
}

/// Source height and queuing limit of every stack (block light: no source, every seed queued).
/// `pre`: source heights already computed (`par::sources`).
#[allow(clippy::too_many_arguments)]
fn stack_seeds(
    grid: &impl LightGrid,
    c: &Ctx,
    z: &Cells,
    stacks: &mut Vec<StackSeed>,
    mut aux: Option<&mut Aux>,
    pre: Option<&[i32]>,
    g: Geo,
    r0: i32,
    ny: i32,
    sky: bool,
) {
    let (sx, sz) = (g.size[0], g.size[2]);
    stacks.clear();
    stacks.resize((sx * sz) as usize, NO_SKY);
    if !sky {
        return;
    }
    for dz in 1..sz - 1 {
        for dx in 1..sx - 1 {
            let k = (dz * sx + dx) as usize;
            stacks[k].source_from = match pre {
                Some(p) => p[k],
                None => column_source(grid, c, aux.as_deref_mut().expect("sky caches"), z, g, r0, ny, dx, dz),
            };
        }
    }
    if c.o.sky_frontier {
        for dz in 1..sz - 1 {
            for dx in 1..sx - 1 {
                stacks[(dz * sx + dx) as usize].queue_below = queue_below(stacks, sx, sz, dx, dz);
            }
        }
    }
}

/// Height below which the sources of the interior stack `(dx, dz)` have a neighbour to raise.
fn queue_below(stacks: &[StackSeed], sx: i32, sz: i32, dx: i32, dz: i32) -> i32 {
    let own = stacks[(dz * sx + dx) as usize].source_from;
    // The lowest source has a cell below it that is not a source.
    let mut below = if sabotage(SABOTAGE_FORGETS_LOWEST_SOURCE) { i32::MIN } else { own.saturating_add(1) };
    for (side, (ddx, ddz)) in [(-1, 0), (1, 0), (0, -1), (0, 1)].into_iter().enumerate() {
        let (nx, nz) = (dx + ddx, dz + ddz);
        // A margin stack is never opened: nothing to raise there.
        let interior = (1..sx - 1).contains(&nx) && (1..sz - 1).contains(&nz);
        if interior && !(side == 1 && sabotage(SABOTAGE_FORGETS_HIGHER_NEIGHBOUR)) {
            below = below.max(stacks[(nz * sx + nx) as usize].source_from);
        }
    }
    below
}

/// Seeds an interior cell with the old rule (opened when its sheet is present; value: emission,
/// or full sky from the stack's source height) and queues it, unless it is a sky source at or
/// above the stack's limit. Returns its value and stored value (early stop of the block).
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn seed_interior_cell(
    z: &Cells,
    lay: &mut [u8],
    level: &mut [u8],
    buckets: &mut [Vec<u64>; 16],
    stack: StackSeed,
    i: usize,
    y: i32,
    sky: bool,
) -> u8 {
    let b = lay[i];
    let mut v = 0;
    if b & DATA != 0 {
        lay[i] = b | OPEN;
        v = if !sky {
            z.emit[i]
        } else if y >= stack.source_from {
            15
        } else {
            0
        };
    }
    if v > 1 && (v < 15 || y < stack.queue_below) {
        buckets[v as usize].push(i as u64 | ALL_DIRS);
    }
    level[i] = v;
    v | (b & 15)
}

/// Interior cells stack by stack (the order of the old seeding).
fn seed_interior_by_stacks(z: &Cells, lay: &mut [u8], lane: &mut Lane, g: Geo, r0: i32, ny: i32, sky: bool) -> u8 {
    let (sx, sz) = (g.size[0], g.size[2]);
    let Lane { level, buckets, stacks, .. } = lane;
    let mut any = 0;
    for dz in 1..sz - 1 {
        for dx in 1..sx - 1 {
            let stack = stacks[(dz * sx + dx) as usize];
            for r in r0 + 1..r0 + ny - 1 {
                any |= seed_interior_cell(z, lay, level, buckets, stack, g.at(dx, r, dz), g.lo[1] + r, sky);
            }
        }
    }
    any
}

/// Interior cells row by row along x: consecutive indices (`LIGHTSEEDROWS`).
fn seed_interior_by_rows(z: &Cells, lay: &mut [u8], lane: &mut Lane, g: Geo, r0: i32, ny: i32, sky: bool) -> u8 {
    let (sx, sz) = (g.size[0], g.size[2]);
    let Lane { level, buckets, stacks, .. } = lane;
    let mut any = 0;
    for r in r0 + 1..r0 + ny - 1 {
        let y = g.lo[1] + r;
        for dz in 1..sz - 1 {
            let (row, first) = (g.at(0, r, dz), (dz * sx) as usize);
            // Interior stacks of the row: x from 1 to sx - 2.
            for (dx, &stack) in stacks[first..first + sx as usize - 1].iter().enumerate().skip(1) {
                any |= seed_interior_cell(z, lay, level, buckets, stack, row + dx, y, sky);
            }
        }
    }
    any
}

/// Margin cells (stored value kept, never opened): the bottom and top rows, then the four sides of
/// the interior rows, each with the interior cell next to it (none for the corner stacks).
fn seed_margins(c: &Ctx, z: &Cells, lay: &[u8], lane: &mut Lane, g: Geo, r0: i32, ny: i32) -> u8 {
    let (sx, sz) = (g.size[0], g.size[2]);
    let (bottom, top) = (r0, r0 + ny - 1);
    let mut any = 0;
    let mut seed = |i: usize, inner: Option<usize>| any |= seed_margin_cell(c, z, lay, lane, i, inner);
    for dz in 0..sz {
        for dx in 0..sx {
            let inside = (1..sx - 1).contains(&dx) && (1..sz - 1).contains(&dz);
            seed(g.at(dx, bottom, dz), inside.then(|| g.at(dx, bottom + 1, dz)));
            seed(g.at(dx, top, dz), inside.then(|| g.at(dx, top - 1, dz)));
        }
    }
    for r in bottom + 1..top {
        for dx in 0..sx {
            let inside = (1..sx - 1).contains(&dx);
            seed(g.at(dx, r, 0), inside.then(|| g.at(dx, r, 1)));
            seed(g.at(dx, r, sz - 1), inside.then(|| g.at(dx, r, sz - 2)));
        }
        for dz in 1..sz - 1 {
            seed(g.at(0, r, dz), Some(g.at(1, r, dz)));
            seed(g.at(sx - 1, r, dz), Some(g.at(sx - 2, r, dz)));
        }
    }
    any
}

/// Seeds a margin cell: its stored value, queued when lit (value above 1 with a sheet, or full sky
/// outside the world) and, with `LIGHTMARGINFRONTIER`, only if it raises its interior neighbour.
#[inline]
fn seed_margin_cell(c: &Ctx, z: &Cells, lay: &[u8], lane: &mut Lane, i: usize, inner: Option<usize>) -> u8 {
    let b = lay[i];
    let v = b & 15;
    let lit = v > 1 && (b & DATA != 0 || v == 15);
    let useful = !c.o.margin_frontier || inner.is_some_and(|j| raises(z, lay, &lane.level, v, j));
    let dropped = v == 15 && sabotage(SABOTAGE_MARGIN_15_NOT_QUEUED);
    if lit && useful && !dropped {
        lane.buckets[v as usize].push(i as u64 | ALL_DIRS);
    }
    lane.level[i] = v;
    v
}

/// A seed of value `v` raises the interior cell `j` above its seeded level (levels only rise
/// afterwards: if it does not now, it never will).
fn raises(z: &Cells, lay: &[u8], level: &[u8], v: u8, j: usize) -> bool {
    lay[j] & OPEN != 0 && i32::from(v) - i32::from(z.prop[j] & 15) > i32::from(level[j])
}
