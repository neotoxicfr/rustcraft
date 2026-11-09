//! Per-axis collisions and step-up (spec `movement.md` § 6.3).

use crate::{Aabb, World};

pub const EPS: f64 = 1.0e-7;

/// Collision boxes of the blocks that count in zone `b` (edge cell rules).
pub fn collect(w: &dyn World, b: &Aabb, out: &mut Vec<Aabb>) {
    collect_in(w, b, out, WalkOrder::YOuter, WalkPath::current());
}

/// Like `collect`, in the game's walk order (x then y then z): useful when the
/// first box found matters (auto-jump).
pub fn collect_ordered(w: &dyn World, b: &Aabb, out: &mut Vec<Aabb>) {
    collect_in(w, b, out, WalkOrder::ZOuter, WalkPath::current());
}

/// `collect` walking every cell of the margin one by one, the path before
/// `RUSTCRAFT_OPT_COLLISION_SCAN_TRIM` and `RUSTCRAFT_OPT_COLLISION_BATCH_READ`: the reference of
/// the equivalence proofs, here and in the crates that wrap a world.
pub fn collect_untrimmed(w: &dyn World, b: &Aabb, out: &mut Vec<Aabb>) {
    collect_in(w, b, out, WalkOrder::YOuter, WalkPath::WHOLE);
}

/// Order of the collision walk; x always varies fastest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum WalkOrder {
    /// y outermost, then z.
    YOuter,
    /// The game's order (x, then y, then z): z outermost, then y.
    ZOuter,
}

/// Path of `is_free` and of the cell buffer, `RUSTCRAFT_OPT_ISFREE` (read once): `0`
/// [`IsFreePath::CollectAll`], `1` [`IsFreePath::FirstOverlap`], anything else the default
/// [`IsFreePath::FirstOverlapSharedBuffer`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IsFreePath {
    /// Old path: every box collected, then the entities always queried; a new cell buffer per walk.
    CollectAll,
    /// Exits at the first overlap; the entities are only queried if blocks and border let through.
    FirstOverlap,
    /// `FirstOverlap`, plus the thread's cell buffer reused by every walk (collections too).
    FirstOverlapSharedBuffer,
}

impl IsFreePath {
    pub fn current() -> IsFreePath {
        static PATH: std::sync::OnceLock<IsFreePath> = std::sync::OnceLock::new();
        *PATH.get_or_init(|| match rc_data::env_str!("RUSTCRAFT_OPT_ISFREE").map(str::trim) {
            Some("0") => IsFreePath::CollectAll,
            Some("1") => IsFreePath::FirstOverlap,
            _ => IsFreePath::FirstOverlapSharedBuffer,
        })
    }
}

thread_local! {
    /// Buffer of the boxes of a cell, reused from one call to the next (`FirstOverlapSharedBuffer`).
    static CELL: std::cell::RefCell<Vec<Aabb>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// `f` with a cell buffer: the thread's on the shared-buffer path (a new one if it is already
/// borrowed by a nested call), otherwise a new one.
pub(crate) fn with_cell<R>(path: IsFreePath, f: impl FnOnce(&mut Vec<Aabb>) -> R) -> R {
    if path == IsFreePath::FirstOverlapSharedBuffer {
        CELL.with(|c| match c.try_borrow_mut() {
            Ok(mut v) => f(&mut v),
            Err(_) => f(&mut Vec::new()),
        })
    } else {
        f(&mut Vec::new())
    }
}

pub(crate) fn collect_in(w: &dyn World, b: &Aabb, out: &mut Vec<Aabb>, order: WalkOrder, path: WalkPath) {
    out.clear();
    with_cell(IsFreePath::current(), |cell| {
        walk_checked(w, b, order, path, cell, |_, bx| {
            out.push(bx);
            false
        })
    });
}

/// Cells walked for `b`: those it touches, widened by EPS, plus a one-cell margin.
#[inline]
fn scan_bounds(b: &Aabb) -> ([i32; 3], [i32; 3]) {
    // Every collision walk starts here: six floors, a library call each on x86-64 without SSE4.1
    // (`RUSTCRAFT_OPT_FLOOR_JAVA=0` restores it). Same results, NaN and saturation included.
    let shared = rc_data::opt_switch!("RUSTCRAFT_OPT_FLOOR_JAVA");
    let floor = |v: f64| if shared { rc_data::java_math::floor_to_i32(v) } else { v.floor() as i32 };
    (b.min.map(|v| floor(v - EPS) - 1), b.max.map(|v| floor(v + EPS) + 1))
}

/// Paths of the collision walk: `trim` skips the cells out of reach of the shapes
/// (`RUSTCRAFT_OPT_COLLISION_SCAN_TRIM`), `batch` reads the states of the walked cells in one
/// call to the world (`RUSTCRAFT_OPT_COLLISION_BATCH_READ`). Both on by default, `=0` turns one off.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct WalkPath {
    pub(crate) trim: bool,
    pub(crate) batch: bool,
}

impl WalkPath {
    /// The path before both: every cell of the margin, read one by one.
    pub(crate) const WHOLE: WalkPath = WalkPath { trim: false, batch: false };

    pub(crate) fn current() -> WalkPath {
        WalkPath {
            trim: rc_data::opt_switch!("RUSTCRAFT_OPT_COLLISION_SCAN_TRIM"),
            batch: rc_data::opt_switch!("RUSTCRAFT_OPT_COLLISION_BATCH_READ"),
        }
    }
}

#[cfg(test)]
thread_local! {
    /// Set by the micro-benches of this crate's tests on their own threads only: a flag shared by
    /// the whole process would turn the check off for the tests running beside them.
    static WALK_CHECK_SKIPPED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Skips the check of every walk on this thread until dropped (a micro-bench would time it).
#[cfg(test)]
pub(crate) struct WalkCheckSkipped {
    before: bool,
}

#[cfg(test)]
pub(crate) fn skip_walk_check_on_this_thread() -> WalkCheckSkipped {
    WalkCheckSkipped { before: WALK_CHECK_SKIPPED.replace(true) }
}

#[cfg(test)]
impl Drop for WalkCheckSkipped {
    fn drop(&mut self) {
        WALK_CHECK_SKIPPED.set(self.before);
    }
}

/// `RUSTCRAFT_COLLISION_WALK_CHECK=1` (and always in this crate's tests, benches aside): every
/// trimmed or batched walk is compared with the whole one and panics on the first difference.
fn walk_check() -> bool {
    #[cfg(test)]
    if !WALK_CHECK_SKIPPED.get() {
        return true;
    }
    rc_data::env_str!("RUSTCRAFT_COLLISION_WALK_CHECK") == Some("1")
}

/// Narrows the cells `lo..=hi` to those whose boxes can reach `b`: on an axis, a box of cell `c`
/// spans at most `c + reach.min .. c + reach.max`, and only overlaps `b` if that span does.
/// The test makes the same addition as placing a box in its cell, and the rounding is monotonic,
/// so a skipped cell could never have produced a box (NaN and infinities included). `None`:
/// no cell can reach `b`.
fn reachable_span(reach: &Aabb, b: &Aabb, lo: [i32; 3], hi: [i32; 3]) -> Option<([i32; 3], [i32; 3])> {
    let (mut first, mut last) = (lo, hi);
    for a in 0..3 {
        first[a] = (lo[a]..=hi[a]).find(|&c| f64::from(c) + reach.max[a] > b.min[a])?;
        last[a] = (first[a]..=hi[a]).rev().find(|&c| f64::from(c) + reach.min[a] < b.max[a])?;
    }
    Some((first, last))
}

/// Walks the collision boxes (placed in the world) that touch `b`, according to the edge
/// cell rules; `f` receives the cell and the box, and returns true to stop (the walk
/// then returns true).
pub(crate) fn walk_collision_cells(
    w: &dyn World,
    b: &Aabb,
    order: WalkOrder,
    cell: &mut Vec<Aabb>,
    f: impl FnMut([i32; 3], Aabb) -> bool,
) -> bool {
    walk_checked(w, b, order, WalkPath::current(), cell, f)
}

/// `walk_cells`, compared with the whole walk first in check mode.
pub(crate) fn walk_checked(
    w: &dyn World,
    b: &Aabb,
    order: WalkOrder,
    path: WalkPath,
    cell: &mut Vec<Aabb>,
    f: impl FnMut([i32; 3], Aabb) -> bool,
) -> bool {
    let mut reach = Aabb::EVERYWHERE;
    walk_checked_noting_reach(w, b, order, path, cell, &mut reach, f)
}

/// `walk_checked`, writing into `reach` the shape reach the walk trimmed with (unbounded when it
/// did not trim).
fn walk_checked_noting_reach(
    w: &dyn World,
    b: &Aabb,
    order: WalkOrder,
    path: WalkPath,
    cell: &mut Vec<Aabb>,
    reach: &mut Aabb,
    f: impl FnMut([i32; 3], Aabb) -> bool,
) -> bool {
    if path != WalkPath::WHOLE && walk_check() {
        assert_same_as_whole_walk(w, b, order, path);
    }
    walk_cells_noting_reach(w, b, order, path, cell, reach, f)
}

/// Check mode: the walk on `path` gives the same cells and boxes, in the same order, as the whole one.
fn assert_same_as_whole_walk(w: &dyn World, b: &Aabb, order: WalkOrder, path: WalkPath) {
    let walk = |path: WalkPath| {
        let mut found = Vec::new();
        walk_cells(w, b, order, path, &mut Vec::new(), |c, bx| {
            found.push((c, bx));
            false
        });
        found
    };
    assert_eq!(walk(path), walk(WalkPath::WHOLE), "trimmed collision walk differs for {b:?} ({path:?})");
}

/// Cells whose states a batched walk reads at once (a mob's movement needs 4 to 30); a larger
/// span is read cell by cell.
const BATCH_CELLS: usize = 64;

/// Number of cells of `first..=last`, `None` past `usize` (or an empty span).
fn span_cells(first: [i32; 3], last: [i32; 3]) -> Option<usize> {
    (0..3).try_fold(1usize, |n, a| {
        let len = usize::try_from(i64::from(last[a]) - i64::from(first[a]) + 1).ok()?;
        n.checked_mul(len)
    })
}

/// The walk on `path`: `trim` skips the cells that cannot reach `b` (`World::shape_reach`),
/// `batch` reads the states of the remaining cells in one `World::states_in` call. The edge
/// rules still count from the whole margin `lo..=hi`.
pub(crate) fn walk_cells(
    w: &dyn World,
    b: &Aabb,
    order: WalkOrder,
    path: WalkPath,
    cell: &mut Vec<Aabb>,
    f: impl FnMut([i32; 3], Aabb) -> bool,
) -> bool {
    let mut reach = Aabb::EVERYWHERE;
    walk_cells_noting_reach(w, b, order, path, cell, &mut reach, f)
}

/// `walk_cells`, writing into `reach` the shape reach it trimmed with (unbounded when it did not
/// trim).
fn walk_cells_noting_reach(
    w: &dyn World,
    b: &Aabb,
    order: WalkOrder,
    path: WalkPath,
    cell: &mut Vec<Aabb>,
    reach: &mut Aabb,
    mut f: impl FnMut([i32; 3], Aabb) -> bool,
) -> bool {
    let (lo, hi) = scan_bounds(b);
    *reach = if path.trim { w.shape_reach(lo, hi) } else { Aabb::EVERYWHERE };
    let walked = if path.trim { reachable_span(reach, b, lo, hi) } else { Some((lo, hi)) };
    let Some((first, last)) = walked else { return false };
    // One column lookup for all the cells instead of one per cell (a hash lookup on the server).
    let mut states = [crate::UNLOADED; BATCH_CELLS];
    let batched = path.batch
        && span_cells(first, last).is_some_and(|n| n <= BATCH_CELLS && w.states_in(first, last, &mut states[..n]));
    let (nx, nz) = if batched { ((last[0] - first[0] + 1) as usize, (last[2] - first[2] + 1) as usize) } else { (0, 0) };
    let read_state = |x: i32, y: i32, z: i32| {
        let s = states[((y - first[1]) as usize * nz + (z - first[2]) as usize) * nx + (x - first[0]) as usize];
        (s != crate::UNLOADED).then_some(s)
    };
    let (outer, middle) = match order {
        WalkOrder::YOuter => (1, 2),
        WalkOrder::ZOuter => (2, 1),
    };
    for a in first[outer]..=last[outer] {
        for c in first[middle]..=last[middle] {
            let (y, z) = match order {
                WalkOrder::YOuter => (a, c),
                WalkOrder::ZOuter => (c, a),
            };
            for x in first[0]..=last[0] {
                let edges = u8::from(x == lo[0] || x == hi[0])
                    + u8::from(y == lo[1] || y == hi[1])
                    + u8::from(z == lo[2] || z == hi[2]);
                cell.clear();
                // Corners ignored; two edges: only a block in piston movement.
                if edges >= 3 || (edges == 2 && !w.moving_collision(x, y, z, cell)) {
                    continue;
                }
                if edges < 2 {
                    let oversized =
                        if batched { w.collision_of_state(read_state(x, y, z), x, y, z, cell) } else { w.collision(x, y, z, cell) };
                    if edges == 1 && !oversized {
                        continue;
                    }
                }
                for local in cell.iter() {
                    let bx = local.offset(f64::from(x), f64::from(y), f64::from(z));
                    if bx.intersects(b) && f([x, y, z], bx) {
                        return true;
                    }
                }
            }
        }
    }
    false
}

/// Distance allowed on axis `axis` for box `b` that wants to move by `d`.
pub fn clip_axis(boxes: &[Aabb], b: &Aabb, axis: usize, d: f64) -> f64 {
    clip_axis_parts(&[boxes], b, axis, d)
}

/// `clip_axis` over boxes given in several slices, walked in order as if they were one: the
/// order matters (the walk stops as soon as the distance falls under `EPS`).
fn clip_axis_parts(parts: &[&[Aabb]], b: &Aabb, axis: usize, mut d: f64) -> f64 {
    let (o1, o2) = match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    for part in parts {
        for s in *part {
            if d.abs() < EPS {
                return 0.0;
            }
            let overlap = |a: usize| s.max[a] - EPS > b.min[a] && s.min[a] + EPS < b.max[a];
            if !overlap(o1) || !overlap(o2) {
                continue;
            }
            if d > 0.0 && s.min[axis] >= b.max[axis] - EPS {
                d = d.min(s.min[axis] - b.max[axis]);
            } else if d < 0.0 && s.max[axis] <= b.min[axis] + EPS {
                d = d.max(s.max[axis] - b.min[axis]);
            }
        }
    }
    if d.abs() < EPS { 0.0 } else { d }
}

/// Sweep Y, then X and Z (the larger first); returns the allowed movement.
pub fn sweep(boxes: &[Aabb], start: &Aabb, d: [f64; 3]) -> [f64; 3] {
    sweep_parts(&[boxes], start, d)
}

/// `sweep` over boxes given in several slices, in order (see `clip_axis_parts`).
fn sweep_parts(parts: &[&[Aabb]], start: &Aabb, d: [f64; 3]) -> [f64; 3] {
    let mut m = [0.0f64; 3];
    let mut b = *start;
    let order = if d[0].abs() >= d[2].abs() { [1, 0, 2] } else { [1, 2, 0] };
    for axis in order {
        if d[axis] != 0.0 {
            let v = clip_axis_parts(parts, &b, axis, d[axis]);
            m[axis] = v;
            let mut o = [0.0; 3];
            o[axis] = v;
            b = b.offset(o[0], o[1], o[2]);
        }
    }
    m
}

/// Boxes and heights of a `resolve`, kept per thread so that a movement allocates nothing once
/// the buffers have grown.
#[derive(Default)]
struct ResolveBuffers {
    blocks: CollectedBlocks,
    border: Vec<Aabb>,
    entities: Vec<Aabb>,
    heights: Vec<f32>,
}

/// Block boxes of one collision collection. A movement made by [`resolve_keeping`] also keeps the
/// cell of each box, the zone and the shape reach of its last collection: the support search
/// that follows it ([`crate::support_of_kept`]) filters them instead of walking the same cells
/// again (`RUSTCRAFT_OPT_SUPPORTSHARE`).
#[derive(Debug)]
pub struct CollectedBlocks {
    boxes: Vec<Aabb>,
    /// Cell of each box (filled only when kept).
    cells: Vec<[i32; 3]>,
    /// Zone of the kept collection; `None` when nothing is kept.
    zone: Option<Aabb>,
    /// Shape reach the kept collection walked with (unbounded when it did not trim).
    reach: Aabb,
}

impl Default for CollectedBlocks {
    fn default() -> Self {
        CollectedBlocks { boxes: Vec::new(), cells: Vec::new(), zone: None, reach: Aabb::EVERYWHERE }
    }
}

impl CollectedBlocks {
    /// Collects the block boxes of `zone`; with `keep`, also their cells, the zone and the reach.
    fn collect(&mut self, w: &dyn World, zone: &Aabb, walk: WalkPath, keep: bool) {
        let CollectedBlocks { boxes, cells, zone: kept_zone, reach } = self;
        boxes.clear();
        cells.clear();
        *kept_zone = keep.then_some(*zone);
        with_cell(IsFreePath::current(), |cell| {
            walk_checked_noting_reach(w, zone, WalkOrder::YOuter, walk, cell, reach, |at, bx| {
                boxes.push(bx);
                if keep {
                    cells.push(at);
                }
                false
            })
        });
    }

    /// Nothing kept any more: the next support search walks.
    pub fn forget(&mut self) {
        self.zone = None;
    }

    /// The cells of the boxes the support walk of `zone` would find (one per box, in the walk's
    /// order), taken from the kept collection, or `None` when it cannot tell them all.
    ///
    /// Exact when `zone` lies inside the kept zone and the kept reach spills out of no cell
    /// sideways. The cells the support walk reads are then all read by the kept walk, with no more
    /// edge restrictions (an edge of the kept walk is an edge of the support walk). The support
    /// walk only skips or restricts its own edge cells: a box of an edge cell that does not leave
    /// its cell never reaches `zone`; on two or three edges one of them is sideways, which no box
    /// crosses without a sideways spill. Kept boxes are taken from the support walk's cells only.
    pub(crate) fn support_cells(&self, zone: Aabb) -> Option<impl Iterator<Item = [i32; 3]> + use<'_>> {
        let kept = self.zone?;
        let inside = (0..3).all(|a| kept.min[a] <= zone.min[a] && zone.max[a] <= kept.max[a]);
        let r = &self.reach;
        let spills_sideways = !(r.min[0] >= 0.0 && r.min[2] >= 0.0 && r.max[0] <= 1.0 && r.max[2] <= 1.0);
        if !inside || spills_sideways {
            return None;
        }
        let (lo, hi) = scan_bounds(&zone);
        let walked = move |c: &[i32; 3]| (0..3).all(|a| lo[a] <= c[a] && c[a] <= hi[a]);
        Some(self.cells.iter().zip(&self.boxes).filter(move |(c, bx)| walked(c) && bx.intersects(&zone)).map(|(c, _)| *c))
    }
}

thread_local! {
    static RESOLVE: std::cell::RefCell<ResolveBuffers> = std::cell::RefCell::new(ResolveBuffers::default());
}

/// Paths of `resolve`, compared by the proofs and the benches: `trim` for
/// `RUSTCRAFT_OPT_COLLISION_SCAN_TRIM`, `no_copy` for `RUSTCRAFT_OPT_RESOLVE_NO_COPY`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ResolvePath {
    pub(crate) walk: WalkPath,
    pub(crate) no_copy: bool,
}

impl ResolvePath {
    #[cfg(test)]
    pub(crate) const OLD: ResolvePath = ResolvePath { walk: WalkPath::WHOLE, no_copy: false };

    fn current() -> ResolvePath {
        ResolvePath { walk: WalkPath::current(), no_copy: rc_data::opt_switch!("RUSTCRAFT_OPT_RESOLVE_NO_COPY") }
    }
}

/// Full movement with step-up (§ 6.3). `on_ground`: state before the movement.
pub fn resolve(w: &dyn World, start: &Aabb, d: [f64; 3], on_ground: bool, step: f64) -> [f64; 3] {
    resolve_with(w, start, d, on_ground, step, ResolvePath::current())
}

/// `resolve` on a given path.
pub(crate) fn resolve_with(w: &dyn World, start: &Aabb, d: [f64; 3], on_ground: bool, step: f64, path: ResolvePath) -> [f64; 3] {
    if !path.no_copy {
        return resolve_copying(w, start, d, on_ground, step, path.walk);
    }
    resolve_in_thread_buffers(w, start, d, on_ground, step, path.walk, None)
}

/// `resolve`, keeping in `kept` the block boxes of its last collection with their cells, for the
/// support search that follows the movement ([`crate::support_of_kept`]). Nothing is kept on the
/// old copying path (`RUSTCRAFT_OPT_RESOLVE_NO_COPY=0`).
pub fn resolve_keeping(w: &dyn World, start: &Aabb, d: [f64; 3], on_ground: bool, step: f64, kept: &mut CollectedBlocks) -> [f64; 3] {
    let path = ResolvePath::current();
    kept.forget();
    if !path.no_copy {
        return resolve_copying(w, start, d, on_ground, step, path.walk);
    }
    resolve_in_thread_buffers(w, start, d, on_ground, step, path.walk, Some(kept))
}

/// `resolve_in` with the thread's buffers (own ones when a collision search resolves a movement
/// from inside another); block boxes collected into `kept` when given.
fn resolve_in_thread_buffers(
    w: &dyn World,
    start: &Aabb,
    d: [f64; 3],
    on_ground: bool,
    step: f64,
    walk: WalkPath,
    kept: Option<&mut CollectedBlocks>,
) -> [f64; 3] {
    RESOLVE.with(|cell| match cell.try_borrow_mut() {
        Ok(mut buffers) => resolve_in(w, start, d, on_ground, step, walk, &mut buffers, kept),
        Err(_) => resolve_in(w, start, d, on_ground, step, walk, &mut ResolveBuffers::default(), kept),
    })
}

/// `resolve` with the boxes left in their own buffers: blocks, border and entities are walked
/// as three slices in that order instead of being copied one after the other. The block boxes go
/// into `kept` (with their cells) when given, otherwise into the buffers.
#[allow(clippy::too_many_arguments)]
fn resolve_in(
    w: &dyn World,
    start: &Aabb,
    d: [f64; 3],
    on_ground: bool,
    step: f64,
    walk: WalkPath,
    buffers: &mut ResolveBuffers,
    kept: Option<&mut CollectedBlocks>,
) -> [f64; 3] {
    let ResolveBuffers { blocks: own_blocks, border, entities, heights } = buffers;
    let keep = kept.is_some();
    let collected = kept.unwrap_or(own_blocks);
    // Shape of the world border, if the entity is close to it (`weather-border.md` § 4.5).
    border.clear();
    if let Some(b) = w.border() {
        b.collision(start, border);
    }
    // Solid entities: searched once in the zone of the movement and the step.
    entities.clear();
    w.entity_colliders(&start.expand_towards(d).expand_towards([0.0, step.max(0.0), 0.0]), entities);
    collected.collect(w, &start.expand_towards(d), walk, keep);
    let allowed = sweep_parts(&[&collected.boxes, border, entities], start, d);
    let Some((step_base, zone)) = step_up_zone(start, d, allowed, on_ground, step) else { return allowed };
    collected.collect(w, &zone, walk, keep);
    let blocks = &collected.boxes;
    heights.clear();
    push_step_heights(blocks.iter().chain(border.iter()).chain(entities.iter()), &step_base, allowed, step, heights);
    let horizontal_len_sq = |v: [f64; 3]| v[0] * v[0] + v[2] * v[2];
    for &c in heights.iter() {
        let stepped = sweep_parts(&[blocks, border, entities], &step_base, [d[0], f64::from(c), d[2]]);
        if horizontal_len_sq(stepped) > horizontal_len_sq(allowed) {
            let dy = start.min[1] - step_base.min[1];
            return [stepped[0], stepped[1] - dy, stepped[2]];
        }
    }
    allowed
}

/// Box the step-up starts from and zone it searches, or `None` when no step-up is tried: no step
/// height, neither blocked downwards nor on the ground, or nothing blocked horizontally.
fn step_up_zone(start: &Aabb, d: [f64; 3], allowed: [f64; 3], on_ground: bool, step: f64) -> Option<(Aabb, Aabb)> {
    let blocked_down = d[1] != allowed[1] && d[1] < 0.0;
    let horizontal = d[0] != allowed[0] || d[2] != allowed[2];
    if step <= 0.0 || !(blocked_down || on_ground) || !horizontal {
        return None;
    }
    let step_base = if blocked_down { start.offset(0.0, allowed[1], 0.0) } else { *start };
    let mut zone = step_base.expand_towards([d[0], step, d[2]]);
    if !blocked_down {
        zone = zone.expand_towards([0.0, f64::from(-1.0e-5f32), 0.0]);
    }
    Some((step_base, zone))
}

/// Candidate step heights: Y edges of the boxes relative to the bottom of `step_base`, in f32,
/// sorted increasing without duplicates.
fn push_step_heights<'a>(
    boxes: impl Iterator<Item = &'a Aabb>,
    step_base: &Aabb,
    allowed: [f64; 3],
    step: f64,
    heights: &mut Vec<f32>,
) {
    let allowed_y = allowed[1] as f32;
    for s in boxes {
        for y in [s.min[1], s.max[1]] {
            let c = (y - step_base.min[1]) as f32;
            if c >= 0.0 && c != allowed_y && f64::from(c) <= step {
                heights.push(c);
            }
        }
    }
    heights.sort_by(f32::total_cmp);
    heights.dedup();
}

/// Old `resolve` (`RUSTCRAFT_OPT_RESOLVE_NO_COPY=0`): new buffers on every call, border and
/// entities copied after the blocks.
fn resolve_copying(w: &dyn World, start: &Aabb, d: [f64; 3], on_ground: bool, step: f64, walk: WalkPath) -> [f64; 3] {
    let mut border = Vec::new();
    if let Some(b) = w.border() {
        b.collision(start, &mut border);
    }
    let mut ents = Vec::new();
    w.entity_colliders(&start.expand_towards(d).expand_towards([0.0, step.max(0.0), 0.0]), &mut ents);
    let mut boxes = Vec::new();
    collect_in(w, &start.expand_towards(d), &mut boxes, WalkOrder::YOuter, walk);
    boxes.extend_from_slice(&border);
    boxes.extend_from_slice(&ents);
    let allowed = sweep(&boxes, start, d);
    let Some((step_base, zone)) = step_up_zone(start, d, allowed, on_ground, step) else { return allowed };
    collect_in(w, &zone, &mut boxes, WalkOrder::YOuter, walk);
    boxes.extend_from_slice(&border);
    boxes.extend_from_slice(&ents);
    let mut heights: Vec<f32> = Vec::new();
    push_step_heights(boxes.iter(), &step_base, allowed, step, &mut heights);
    let horizontal_len_sq = |v: [f64; 3]| v[0] * v[0] + v[2] * v[2];
    for c in heights {
        let stepped = sweep(&boxes, &step_base, [d[0], f64::from(c), d[2]]);
        if horizontal_len_sq(stepped) > horizontal_len_sq(allowed) {
            let dy = start.min[1] - step_base.min[1];
            return [stepped[0], stepped[1] - dy, stepped[2]];
        }
    }
    allowed
}

/// True if no collision box overlaps `b`.
pub fn is_free(w: &dyn World, b: &Aabb) -> bool {
    is_free_with(w, b, IsFreePath::current())
}

/// `is_free` on a given path of `RUSTCRAFT_OPT_ISFREE` (the proofs compare the paths).
pub fn is_free_with(w: &dyn World, b: &Aabb, path: IsFreePath) -> bool {
    if path == IsFreePath::CollectAll {
        return is_free_old(w, b);
    }
    with_cell(path, |cell| {
        // A box retained by the walk already overlaps `b` (same test as the old filter).
        if walk_collision_cells(w, b, WalkOrder::YOuter, cell, |_, _| true) {
            return false;
        }
        cell.clear();
        if let Some(border) = w.border() {
            border.collision(b, cell);
            if cell.iter().any(|s| s.intersects(b)) {
                return false;
            }
        }
        // Solid entities: the game searches in the box inflated by 1e-7, a simple contact blocks.
        cell.clear();
        w.entity_colliders(b, cell);
        cell.is_empty()
    })
}

/// Old `is_free` (`RUSTCRAFT_OPT_ISFREE=0`): all the boxes collected, entities always
/// queried.
fn is_free_old(w: &dyn World, b: &Aabb) -> bool {
    let mut boxes = Vec::new();
    collect(w, b, &mut boxes);
    if let Some(border) = w.border() {
        border.collision(b, &mut boxes);
    }
    let blocks = !boxes.iter().any(|s| (0..3).all(|a| s.max[a] > b.min[a] && s.min[a] < b.max[a]));
    // Solid entities: the game searches in the box inflated by 1e-7, a simple contact blocks.
    let mut ents = Vec::new();
    w.entity_colliders(b, &mut ents);
    blocks && ents.is_empty()
}
