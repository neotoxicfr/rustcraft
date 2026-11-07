//! Changed cells of a resolution and their writing (mission C2-M10, `docs/perf.md`): the same
//! cells, sheets and touched sections, written in the same order, as the stack-by-stack scan
//! (`scan_column`) and write (`apply_column`) of `fast.rs`.
//! - `LIGHTSCANPLANES`: the interior is walked plane by plane, row by row of consecutive cells,
//!   where the stack-by-stack walk jumps a whole plane between two cells. A row without change is
//!   settled by a branch-free test the compiler vectorises; the changed cells are then grouped by
//!   stack, in stack order and bottom to top within a stack, by a counting sort over the stacks met
//!   (a bitmap marks them, so the untouched stacks cost nothing).
//! - `LIGHTWRITECOLUMN`: the sheets of a column are fetched once for its consecutive changed stacks
//!   instead of once per stack (a write access stamps the client's column, the server's grid looks
//!   it up in a hash map).

use super::*;

/// Deliberate error of the tests: the grouping forgets the last stack met.
const SABOTAGE_SCAN_LOSES_LAST_STACK: u8 = 41;

/// Changed cells of a resolution, stack by stack: (x, z, end in `cells`), each stack's cells from
/// the bottom up as (height, new value).
#[derive(Default)]
pub(super) struct Changes {
    pub(super) stacks: Vec<(i32, i32, usize)>,
    pub(super) cells: Vec<(i32, u8)>,
}

impl Changes {
    pub(super) fn bytes(&self) -> usize {
        self.stacks.capacity() * size_of::<(i32, i32, usize)>() + self.cells.capacity() * size_of::<(i32, u8)>()
    }

    fn clear(&mut self) {
        self.stacks.clear();
        self.cells.clear();
    }
}

/// Scratch buffers of the plane scan, kept in the lane from one resolution to the next.
#[derive(Default)]
pub(super) struct PlaneScan {
    /// Changed cells in plane order: (stack index `dz · sx + dx`, height, new value).
    found: Vec<(u32, i32, u8)>,
    /// Per stack: its count of changed cells, then its next slot in `Changes::cells` (all zero
    /// between two scans).
    per_stack: Vec<u32>,
    /// One bit per stack with at least one changed cell (all zero between two scans).
    stacks_met: Vec<u64>,
}

impl PlaneScan {
    pub(super) fn bytes(&self) -> usize {
        self.found.capacity() * size_of::<(u32, i32, u8)>()
            + self.per_stack.capacity() * 4
            + self.stacks_met.capacity() * 8
    }
}

/// Changed cells of the interior of a resolution over the rows `r0..r0 + ny` (margins excluded),
/// into `out`: plane by plane (`LIGHTSCANPLANES`) or stack by stack.
#[allow(clippy::too_many_arguments)]
pub(super) fn find_changes(
    o: &Opts,
    lay: &[u8],
    level: &[u8],
    scan: &mut PlaneScan,
    g: Geo,
    r0: i32,
    ny: i32,
    out: &mut Changes,
) {
    out.clear();
    if o.scan_by_planes {
        scan_planes(lay, level, scan, g, r0, ny);
        group_by_stack(scan, g, out);
    } else {
        scan_stacks(lay, level, g, r0, ny, out);
    }
}

/// Stack by stack, with the walk of the old write.
fn scan_stacks(lay: &[u8], level: &[u8], g: Geo, r0: i32, ny: i32, out: &mut Changes) {
    for dz in 1..g.size[2] - 1 {
        for dx in 1..g.size[0] - 1 {
            let start = out.cells.len();
            scan_column(lay, level, g, r0, ny, dx, dz, &mut out.cells);
            if out.cells.len() > start {
                out.stacks.push((g.lo[0] + dx, g.lo[2] + dz, out.cells.len()));
            }
        }
    }
}

/// Interior rows of every interior plane, into `scan.found` (plane order: bottom to top).
fn scan_planes(lay: &[u8], level: &[u8], scan: &mut PlaneScan, g: Geo, r0: i32, ny: i32) {
    let (sx, sz) = (g.size[0] as usize, g.size[2] as usize);
    let PlaneScan { found, per_stack, stacks_met } = scan;
    found.clear();
    if per_stack.len() < sx * sz {
        per_stack.resize(sx * sz, 0);
    }
    if stacks_met.len() < (sx * sz).div_ceil(64) {
        stacks_met.resize((sx * sz).div_ceil(64), 0);
    }
    for r in r0 + 1..r0 + ny - 1 {
        let y = g.lo[1] + r;
        for dz in 1..sz - 1 {
            let start = g.at(1, r, dz as i32);
            let row = start..start + sx - 2;
            let first_stack = dz * sx + 1;
            for_each_changed(&lay[row.clone()], &level[row], |k, value| {
                let stack = first_stack + k;
                found.push((stack as u32, y, value));
                per_stack[stack] += 1;
                stacks_met[stack >> 6] |= 1 << (stack & 63);
            });
        }
    }
}

/// Calls `hit(offset, new value)` for each cell of the row whose stored sheet value the resolution
/// changed (the test of `scan_column`).
#[inline]
fn for_each_changed(lay: &[u8], level: &[u8], mut hit: impl FnMut(usize, u8)) {
    if !row_has_change(lay, level) {
        return;
    }
    for (k, (&b, &v)) in lay.iter().zip(level).enumerate() {
        if b & DATA != 0 && v != b & 15 {
            hit(k, v);
        }
    }
}

/// Some cell of the row holds a sheet value (bit `DATA`) other than its new level (at most 15). No
/// branch, so the compiler tests 16 or 32 cells per vector instruction: most rows have no change.
#[inline]
fn row_has_change(lay: &[u8], level: &[u8]) -> bool {
    let differs = |b: u8, v: u8| (b ^ v) & 15 & 0u8.wrapping_sub((b >> 4) & 1);
    lay.iter().zip(level).fold(0u8, |any, (&b, &v)| any | differs(b, v)) != 0
}

/// The cells of `scan.found` grouped by stack into `out` (stack order, bottom to top within a
/// stack since they were found plane by plane); leaves the counts and the bitmap at zero.
fn group_by_stack(scan: &mut PlaneScan, g: Geo, out: &mut Changes) {
    let sx = g.size[0] as usize;
    let PlaneScan { found, per_stack, stacks_met } = scan;
    // First slot of each stack met, in stack order.
    let mut end = 0u32;
    for_each_bit(stacks_met, |stack| {
        let count = per_stack[stack];
        per_stack[stack] = end;
        end += count;
        out.stacks.push((g.lo[0] + (stack % sx) as i32, g.lo[2] + (stack / sx) as i32, end as usize));
    });
    if sabotage(SABOTAGE_SCAN_LOSES_LAST_STACK) {
        out.stacks.pop();
    }
    out.cells.resize(end as usize, (0, 0));
    for &(stack, y, value) in found.iter() {
        let slot = &mut per_stack[stack as usize];
        out.cells[*slot as usize] = (y, value);
        *slot += 1;
    }
    // Back to zero for the next scan: only the stacks met were touched.
    for_each_bit(stacks_met, |stack| per_stack[stack] = 0);
    stacks_met.fill(0);
}

/// Calls `f` with the index of every set bit, in increasing order.
fn for_each_bit(words: &[u64], mut f: impl FnMut(usize)) {
    for (w, &word) in words.iter().enumerate() {
        let mut bits = word;
        while bits != 0 {
            f(w * 64 + bits.trailing_zeros() as usize);
            bits &= bits - 1;
        }
    }
}

/// Writes the changes stack by stack, in order, and marks the touched sections (like
/// `apply_column` per stack). `LIGHTWRITECOLUMN`: one sheet access per run of stacks of a column.
pub(super) fn apply_changes(grid: &mut impl LightGrid, o: &Opts, ch: &Changes, sky: bool, touched: &mut Touched) {
    let mut start = 0;
    if !o.keep_column_while_writing {
        for &(x, z, end) in &ch.stacks {
            apply_column(grid, x, z, &ch.cells[start..end], sky, touched);
            start = end;
        }
        return;
    }
    let column = |s: &(i32, i32, usize)| (s.0 >> 4, s.1 >> 4);
    for run in ch.stacks.chunk_by(|a, b| column(a) == column(b)) {
        let (cx, cz) = column(&run[0]);
        // Absent column: nothing written, nothing touched (as `apply_column`).
        let mut layers = grid.layers_mut(cx, cz, sky);
        for &(x, z, end) in run {
            if let Some(layers) = layers.as_deref_mut() {
                write_stack(layers, x, z, &ch.cells[start..end], touched);
            }
            start = end;
        }
    }
}
