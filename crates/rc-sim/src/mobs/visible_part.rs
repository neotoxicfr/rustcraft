//! Visible part of a box from an explosion's centre (`seen_percent`), shared by every pass that
//! weighs an explosion or a burst on its targets (mobs, players, objects, decorations, minecarts,
//! crystals, projectiles): a grid of sample points over the box, each joined to the centre by a
//! sightline the blocks' collision shapes may block; the part is the share of clear sightlines.
//!
//! The blocks do not change while one explosion weighs its objects, so the faster paths read the
//! cells the sightlines can cross once, into a cube (`SightCube`), with a running count of the
//! cells that have a collision shape: a layer of sample points, or a sightline, whose cells hold
//! none is clear without its walk (nine sightlines in ten in a TNT heap). An explosion's objects
//! (primed TNT, falling blocks, items, orbs) are weighed together on one cube, two equal boxes
//! once, and on several threads when there are many. Every path gives the same parts to the bit:
//! same sample points, same walk, same box test.

use std::sync::{Mutex, OnceLock, PoisonError};

use hashbrown::HashMap;
use rayon::prelude::*;
use rc_physics::Aabb;

use super::arrows::{CellWalk, clip_boxes};
use crate::chunk::Chunk;
use crate::server::Server;

/// Paths of the visible part, one switch each (`=0`: the former path), read once.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct VisiblePartPaths {
    /// `RUSTCRAFT_OPT_VISIBLE_PART_CELL_CUBE`: the sightlines of targets weighed together read
    /// each cell from one cube filled once, formerly a chunk lookup per cell crossed.
    pub cell_cube: bool,
    /// `RUSTCRAFT_OPT_VISIBLE_PART_AIR_SHORTCUT`: on a cube, a layer of sample points, or a
    /// sightline, whose cells hold no collision shape is clear without a walk.
    pub air_shortcut: bool,
    /// `RUSTCRAFT_OPT_VISIBLE_PART_CUBE_PER_CALL` (default off: no clear gain on the loaded
    /// machine): a box weighed alone (mobs, players, decorations, minecarts…) gets a cube of its
    /// own, with the cell cube and air shortcut switches; off, it takes the former path.
    pub cube_per_call: bool,
    /// `RUSTCRAFT_OPT_VISIBLE_PART_SAME_BOX_ONCE`: among an explosion's objects, a box equal to
    /// the bit to an earlier one takes its part (stacked TNT).
    pub same_box_once: bool,
    /// `RUSTCRAFT_OPT_VISIBLE_PART_THREADS`: an explosion's objects weighed on several threads
    /// from `parallel_min_targets` distinct boxes.
    pub parallel: bool,
    /// `RUSTCRAFT_OPT_BLAST_OBJECTS_ONE_PASS`: an explosion's objects gathered once, in reach and
    /// with their slot, and weighed together (formerly every item and orb gathered, then each
    /// one weighed alone and searched for by identifier, twice for an item).
    pub objects_one_pass: bool,
    pub parallel_min_targets: usize,
    /// Threads of the parallel path (0: `RUSTCRAFT_THREADS`, otherwise every logical core).
    pub threads: usize,
}

#[cfg(test)]
thread_local! {
    /// Tests: paths of the visible parts of this thread (`None`: the switches).
    pub(crate) static FORCED_VISIBLE_PART_PATHS: std::cell::Cell<Option<VisiblePartPaths>> =
        const { std::cell::Cell::new(None) };
}

impl VisiblePartPaths {
    /// The defaults (the cube per call off).
    pub(crate) const NEW: VisiblePartPaths = VisiblePartPaths {
        cell_cube: true,
        air_shortcut: true,
        cube_per_call: false,
        same_box_once: true,
        parallel: true,
        objects_one_pass: true,
        parallel_min_targets: 32,
        threads: 0,
    };
    /// Every new path, the cube per call included.
    #[cfg(test)]
    pub(crate) const ALL_NEW: VisiblePartPaths = VisiblePartPaths { cube_per_call: true, ..VisiblePartPaths::NEW };
    #[cfg(test)]
    pub(crate) const OLD: VisiblePartPaths = VisiblePartPaths {
        cell_cube: false,
        air_shortcut: false,
        cube_per_call: false,
        same_box_once: false,
        parallel: false,
        objects_one_pass: false,
        ..VisiblePartPaths::NEW
    };

    pub(crate) fn current() -> VisiblePartPaths {
        #[cfg(test)]
        if let Some(forced) = FORCED_VISIBLE_PART_PATHS.get() {
            return forced;
        }
        static PATHS: OnceLock<VisiblePartPaths> = OnceLock::new();
        *PATHS.get_or_init(|| VisiblePartPaths {
            cell_cube: rc_data::opt_switch!("RUSTCRAFT_OPT_VISIBLE_PART_CELL_CUBE"),
            air_shortcut: rc_data::opt_switch!("RUSTCRAFT_OPT_VISIBLE_PART_AIR_SHORTCUT"),
            cube_per_call: rc_data::opt_switch!("RUSTCRAFT_OPT_VISIBLE_PART_CUBE_PER_CALL", default_off),
            same_box_once: rc_data::opt_switch!("RUSTCRAFT_OPT_VISIBLE_PART_SAME_BOX_ONCE"),
            parallel: rc_data::opt_switch!("RUSTCRAFT_OPT_VISIBLE_PART_THREADS"),
            objects_one_pass: rc_data::opt_switch!("RUSTCRAFT_OPT_BLAST_OBJECTS_ONE_PASS"),
            threads: rc_data::env_str!("RUSTCRAFT_THREADS").and_then(|v| v.trim().parse().ok()).unwrap_or(0),
            ..VisiblePartPaths::NEW
        })
    }

    /// The sightlines need a cube (its cells, or its count of shaped cells).
    fn reads_cube(&self) -> bool {
        self.cell_cube || self.air_shortcut
    }

    /// Targets weighed together take another path than one former call each.
    fn weighs_together(&self) -> bool {
        self.reads_cube() || self.same_box_once || self.parallel
    }

    /// Threads to weigh `targets` distinct boxes on, `None` for the calling thread alone.
    fn parallel_threads(&self, targets: usize) -> Option<usize> {
        if !self.parallel || targets < self.parallel_min_targets.max(2) {
            return None;
        }
        let threads = match self.threads {
            0 => std::thread::available_parallelism().map_or(1, std::num::NonZero::get),
            n => n,
        };
        (threads > 1).then_some(threads)
    }
}

/// The game's sample points of box `b`, each with its layer (the rank of its height): a grid of
/// about two points per block on each axis, the x and z rows shifted by half the room the grid
/// leaves (which can put a point up to half a step past the box). Every point of a layer has the
/// same height.
pub(crate) fn for_each_sample(b: &Aabb, mut sample: impl FnMut([f64; 3], usize)) {
    let step_x = 1.0 / ((b.max[0] - b.min[0]) * 2.0 + 1.0);
    let step_y = 1.0 / ((b.max[1] - b.min[1]) * 2.0 + 1.0);
    let step_z = 1.0 / ((b.max[2] - b.min[2]) * 2.0 + 1.0);
    let offset_x = (1.0 - (1.0 / step_x).floor() * step_x) / 2.0;
    let offset_z = (1.0 - (1.0 / step_z).floor() * step_z) / 2.0;
    let mut xx = 0.0;
    while xx <= 1.0 {
        let mut yy = 0.0;
        let mut layer = 0;
        while yy <= 1.0 {
            let mut zz = 0.0;
            while zz <= 1.0 {
                let x = b.min[0] + xx * (b.max[0] - b.min[0]);
                let y = b.min[1] + yy * (b.max[1] - b.min[1]);
                let z = b.min[2] + zz * (b.max[2] - b.min[2]);
                sample([x + offset_x, y, z + offset_z], layer);
                zz += step_z;
            }
            yy += step_y;
            layer += 1;
        }
        xx += step_x;
    }
}

/// Every cell a sightline to `center` from a sample point of `b`'s layer at height `y` can
/// visit: the box of the centre and the layer's points (which lie up to half a step past the box
/// in x and z), widened by what the walk adds (each end pushed out by 1e-7 of the segment's
/// length) and a margin for rounding; `None` when not finite.
fn layer_sightline_cells(center: [f64; 3], b: &Aabb, y: f64) -> Option<([i32; 3], [i32; 3])> {
    let lowest = [b.min[0].min(b.max[0]), y, b.min[2].min(b.max[2])];
    let highest = [b.min[0].max(b.max[0]) + 0.5, y, b.min[2].max(b.max[2]) + 0.5];
    let low: [f64; 3] = std::array::from_fn(|a| lowest[a].min(center[a]));
    let high: [f64; 3] = std::array::from_fn(|a| highest[a].max(center[a]));
    let extent = (0..3).map(|a| high[a] - low[a]).fold(0.0, f64::max);
    let margin = 1e-6 + 3e-7 * extent;
    let low = low.map(|v| (v - margin).floor());
    let high = high.map(|v| (v + margin).floor());
    let fits = |v: f64| v.is_finite() && v.abs() < f64::from(i32::MAX);
    (low.iter().chain(&high).all(|&v| fits(v))).then(|| (low.map(|v| v as i32), high.map(|v| v as i32)))
}

/// Which layers of a box's sample points have every sightline clear, worked out at the first
/// point of each layer; layers past the 64th are not tried.
#[derive(Default)]
struct ClearLayers {
    known: u64,
    clear: u64,
}

impl ClearLayers {
    fn is_clear(&mut self, layer: usize, work_out: impl FnOnce() -> bool) -> bool {
        let Some(bit) = u32::try_from(layer).ok().and_then(|shift| 1u64.checked_shl(shift)) else { return false };
        if self.known & bit == 0 {
            self.known |= bit;
            if work_out() {
                self.clear |= bit;
            }
        }
        self.clear & bit != 0
    }
}

/// Share of `clear` among `count` sightlines, as the game divides it.
fn part(clear: i32, count: i32) -> f32 {
    if count == 0 { 0.0 } else { clear as f32 / count as f32 }
}

/// What the sightlines read, shareable between threads: the chunks and the collision shapes.
pub(crate) struct SightWorld<'a> {
    chunks: &'a HashMap<(i32, i32), Option<Box<Chunk>>>,
    min_y: i32,
    phys: &'a rc_physics::Tables,
}

impl SightWorld<'_> {
    /// State of cell `p`, as the server reads it (air outside the loaded chunks and the height).
    fn state(&self, p: [i32; 3]) -> u32 {
        crate::play::block_at(self.chunks, self.min_y, p).unwrap_or(0)
    }

    /// Does a collision shape block the segment from `from` to `to`? The walk and the box test of
    /// the mobs' block clip (`clip_blocks`), the cells read from `cube` when it holds them.
    fn blocked(&self, cube: Option<&SightCube>, paths: VisiblePartPaths, from: [f64; 3], to: [f64; 3]) -> bool {
        if from == to {
            return false;
        }
        let mut walk = CellWalk::new(from, to);
        if paths.air_shortcut
            && let Some(cube) = cube
            && walk.cell_range().is_some_and(|(low, high)| cube.holds_no_shape(low, high))
        {
            return false;
        }
        let cells = if paths.cell_cube { cube } else { None };
        loop {
            let cell = walk.cell();
            let state = cells.and_then(|c| c.state(cell)).unwrap_or_else(|| self.state(cell));
            if clip_boxes(self.phys.shape(state), cell, from, to).is_some() {
                return true;
            }
            if !walk.advance() {
                return false;
            }
        }
    }

    /// Visible part of `b` from `center`. With the air shortcut, a layer of sample points whose
    /// sightlines can only visit cells without a shape counts as clear without a walk (a box on
    /// the ground: every layer but the lowest, whose sightlines start in the ground's cell).
    fn visible_part(&self, cube: Option<&SightCube>, paths: VisiblePartPaths, center: [f64; 3], b: &Aabb) -> f32 {
        let shortcut = if paths.air_shortcut { cube } else { None };
        let mut layers = ClearLayers::default();
        let (mut clear, mut count) = (0, 0);
        for_each_sample(b, |sample, layer| {
            let layer_clear = shortcut.is_some_and(|cube| {
                layers.is_clear(layer, || {
                    layer_sightline_cells(center, b, sample[1]).is_some_and(|(low, high)| cube.holds_no_shape(low, high))
                })
            });
            if layer_clear || !self.blocked(cube, paths, sample, center) {
                clear += 1;
            }
            count += 1;
        });
        part(clear, count)
    }
}

/// Most cells a cube holds (4 bytes of state and 4 of count each: 2 MB); targets spread wider
/// are weighed one by one.
const MAX_CUBE_CELLS: usize = 1 << 18;

/// First cell and size of a cube.
#[derive(Clone, Copy)]
struct CubeBounds {
    origin: [i32; 3],
    size: [usize; 3],
}

/// Every cell the sightlines from the sample points of `boxes` to `center` can cross: their box
/// widened by one block, which holds the half step a sample point can lie past its box and the
/// segment's ends pushed 1e-7 out. `None` when that box is not finite or holds more than
/// `MAX_CUBE_CELLS`. (A cell outside is still read right, from the world: the bounds only
/// decide what is read up front.)
fn cube_bounds(center: [f64; 3], boxes: &[Aabb]) -> Option<CubeBounds> {
    let (mut low, mut high) = (center, center);
    for b in boxes {
        for axis in 0..3 {
            low[axis] = low[axis].min(b.min[axis]);
            high[axis] = high[axis].max(b.max[axis]);
        }
    }
    let low = low.map(|v| (v - 1.0).floor());
    let high = high.map(|v| (v + 1.0).floor());
    if !(0..3).all(|a| low[a].is_finite() && high[a].is_finite() && high[a] - low[a] < 4096.0) {
        return None;
    }
    let size: [usize; 3] = std::array::from_fn(|a| (high[a] - low[a] + 1.0) as usize);
    (size.iter().product::<usize>() <= MAX_CUBE_CELLS).then(|| CubeBounds { origin: low.map(|v| v as i32), size })
}

/// The states of the cells of a box of the world, read once, and the running count of the cells
/// with a collision shape, which tells in eight reads whether any box of cells holds one.
#[derive(Default)]
pub(crate) struct SightCube {
    origin: [i32; 3],
    size: [usize; 3],
    /// States, y fastest, then z, then x (the order a column of a chunk is read in).
    states: Vec<u32>,
    /// Over (size + 1)³ corners, the cells with a collision shape between the origin and the
    /// corner (excluded); empty when not counted.
    shaped_sums: Vec<u32>,
}

thread_local! {
    /// The cube of the visible parts weighed on this thread, its buffers kept from one
    /// explosion to the next.
    static SCRATCH_CUBE: std::cell::RefCell<SightCube> = std::cell::RefCell::new(SightCube::default());
}

impl SightCube {
    /// Runs `f` with this thread's cube (a call nested in `f` gets a new one).
    fn with_scratch<T>(f: impl FnOnce(&mut SightCube) -> T) -> T {
        SCRATCH_CUBE.with(|cell| match cell.try_borrow_mut() {
            Ok(mut cube) => f(&mut cube),
            Err(_) => f(&mut SightCube::default()),
        })
    }

    /// Fills the cube with the cells of `bounds`, and counts their shapes if `count_shapes`.
    fn refill(&mut self, bounds: CubeBounds, world: &SightWorld, count_shapes: bool) {
        self.origin = bounds.origin;
        self.size = bounds.size;
        self.states.clear();
        self.shaped_sums.clear();
        self.read_states(world);
        if count_shapes {
            self.count_shapes(world.phys);
        }
    }

    fn read_states(&mut self, world: &SightWorld) {
        let [sx, sy, sz] = self.size;
        let (bottom, top) = (self.origin[1], self.origin[1] + sy as i32);
        for x in 0..sx {
            for z in 0..sz {
                let (wx, wz) = (self.origin[0] + x as i32, self.origin[2] + z as i32);
                // One chunk lookup per column and one stack read per section, instead of a
                // lookup and a division per cell.
                let column = world.chunks.get(&(wx >> 4, wz >> 4)).and_then(Option::as_deref);
                let mut y = bottom;
                while y < top {
                    let height = y - world.min_y;
                    let from = height.rem_euclid(16) as usize;
                    let run = (16 - from).min((top - y) as usize);
                    let section = usize::try_from(height >> 4).ok().and_then(|s| column?.sections.get(s));
                    match section {
                        Some(s) => self.states.extend(s.blocks.column_iter((wx & 15) as usize, (wz & 15) as usize, from..from + run)),
                        // Below or above the sections, or no chunk: air, as the server reads it.
                        None => self.states.extend(std::iter::repeat_n(0, run)),
                    }
                    y += run as i32;
                }
            }
        }
    }

    /// Index of a corner of the running count.
    fn corner(&self, x: usize, z: usize, y: usize) -> usize {
        (x * (self.size[2] + 1) + z) * (self.size[1] + 1) + y
    }

    fn count_shapes(&mut self, phys: &rc_physics::Tables) {
        let [sx, sy, sz] = self.size;
        self.shaped_sums.resize((sx + 1) * (sz + 1) * (sy + 1), 0);
        let mut cell = 0;
        // The shape table is large and a column is mostly runs of one state (air, the ground):
        // the answer for the state before is kept.
        let mut last: Option<(u32, u32)> = None;
        for x in 0..sx {
            for z in 0..sz {
                let start = self.corner(x + 1, z + 1, 1);
                for y in 0..sy {
                    let state = self.states[cell];
                    let shaped = match last {
                        Some((known, shaped)) if known == state => shaped,
                        _ => u32::from(!phys.shape(state).is_empty()),
                    };
                    last = Some((state, shaped));
                    self.shaped_sums[start + y] = shaped;
                    cell += 1;
                }
            }
        }
        // Running sums along y, then z, then x (the corners of index 0 on an axis stay 0): each
        // corner then counts its whole box. Along z and x the runs are contiguous rows and
        // planes, added as whole slices.
        let row = sy + 1;
        let plane = (sz + 1) * row;
        for column in self.shaped_sums.chunks_mut(row) {
            for y in 1..row {
                column[y] += column[y - 1];
            }
        }
        for corners in self.shaped_sums.chunks_mut(plane) {
            for i in row..plane {
                corners[i] += corners[i - row];
            }
        }
        for i in plane..self.shaped_sums.len() {
            self.shaped_sums[i] += self.shaped_sums[i - plane];
        }
    }

    /// Offset of world cell `p` in the cube on each axis, `None` outside.
    fn local(&self, p: [i32; 3]) -> Option<[usize; 3]> {
        let mut out = [0; 3];
        for axis in 0..3 {
            let offset = usize::try_from(p[axis].checked_sub(self.origin[axis])?).ok()?;
            if offset >= self.size[axis] {
                return None;
            }
            out[axis] = offset;
        }
        Some(out)
    }

    fn state(&self, p: [i32; 3]) -> Option<u32> {
        let [x, y, z] = self.local(p)?;
        Some(self.states[(x * self.size[2] + z) * self.size[1] + y])
    }

    /// The cells from `low` to `high` (included) lie in the cube and none has a collision shape;
    /// false when the shapes were not counted.
    fn holds_no_shape(&self, low: [i32; 3], high: [i32; 3]) -> bool {
        if self.shaped_sums.is_empty() {
            return false;
        }
        let (Some([x0, y0, z0]), Some([x1, y1, z1])) = (self.local(low), self.local(high)) else { return false };
        let (x1, y1, z1) = (x1 + 1, y1 + 1, z1 + 1);
        let sum = |x, z, y| i64::from(self.shaped_sums[self.corner(x, z, y)]);
        let shaped = sum(x1, z1, y1) - sum(x0, z1, y1) - sum(x1, z0, y1) - sum(x1, z1, y0)
            + sum(x0, z0, y1)
            + sum(x0, z1, y0)
            + sum(x1, z0, y0)
            - sum(x0, z0, y0);
        shaped == 0
    }
}

/// The boxes of `boxes` that differ (to the bit) from every earlier one, and for each box the
/// index of its first equal among them. Equal bits go through the same arithmetic, so they get
/// the same part.
fn distinct_boxes(boxes: &[Aabb]) -> (Vec<Aabb>, Vec<usize>) {
    let mut first_of: HashMap<[u64; 6], usize> = HashMap::with_capacity(boxes.len());
    let mut distinct = Vec::new();
    let slots = boxes
        .iter()
        .map(|b| {
            let key = [b.min[0], b.min[1], b.min[2], b.max[0], b.max[1], b.max[2]].map(f64::to_bits);
            *first_of.entry(key).or_insert_with(|| {
                distinct.push(*b);
                distinct.len() - 1
            })
        })
        .collect();
    (distinct, slots)
}

/// Fewest boxes one task of the parallel path weighs.
const PARALLEL_MIN_BOXES_PER_TASK: usize = 8;

/// Thread pool of `threads` threads for the visible parts, built on first use and kept apart
/// from the pool chunk generation uses (an explosion would otherwise wait behind a generation
/// task).
fn pool_of(threads: usize) -> &'static rayon::ThreadPool {
    static POOLS: Mutex<Vec<(usize, &'static rayon::ThreadPool)>> = Mutex::new(Vec::new());
    let mut pools = POOLS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(&(_, pool)) = pools.iter().find(|(n, _)| *n == threads) {
        return pool;
    }
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|i| format!("visible-part-{i}"))
        .build()
        .expect("visible part threads");
    let pool: &'static rayon::ThreadPool = Box::leak(Box::new(pool));
    pools.push((threads, pool));
    pool
}

impl Server {
    pub(crate) fn sight_world(&self) -> SightWorld<'_> {
        SightWorld { chunks: &self.chunks, min_y: self.dim.height.min_y, phys: &self.phys }
    }

    /// Visible part of a box from the centre.
    pub(crate) fn seen_percent(&self, center: [f64; 3], b: &Aabb) -> f32 {
        let paths = VisiblePartPaths::current();
        if !(paths.cube_per_call && paths.reads_cube()) {
            return self.seen_percent_by_world_reads(center, b);
        }
        let world = self.sight_world();
        let Some(bounds) = cube_bounds(center, std::slice::from_ref(b)) else {
            return world.visible_part(None, paths, center, b);
        };
        SightCube::with_scratch(|cube| {
            cube.refill(bounds, &world, paths.air_shortcut);
            world.visible_part(Some(cube), paths, center, b)
        })
    }

    /// The former path: every sightline through the mobs' block clip, a world read per cell.
    fn seen_percent_by_world_reads(&self, center: [f64; 3], b: &Aabb) -> f32 {
        let (mut clear, mut count) = (0, 0);
        for_each_sample(b, |sample, _| {
            if !self.clip_blocks(sample, center) {
                clear += 1;
            }
            count += 1;
        });
        part(clear, count)
    }

    /// Visible parts of `boxes` from `center`, in order, for targets weighed one after the other
    /// while the blocks do not change (an explosion's objects): one cube for all of them, two
    /// equal boxes weighed once, on several threads when there are many.
    pub(crate) fn seen_percents(&self, center: [f64; 3], boxes: &[Aabb]) -> Vec<f32> {
        let paths = VisiblePartPaths::current();
        if !paths.weighs_together() || boxes.is_empty() {
            return boxes.iter().map(|b| self.seen_percent(center, b)).collect();
        }
        let bounds = cube_bounds(center, boxes);
        if paths.reads_cube() && bounds.is_none() {
            // Targets too spread out for one cube: one by one.
            return boxes.iter().map(|b| self.seen_percent(center, b)).collect();
        }
        let world = self.sight_world();
        SightCube::with_scratch(|scratch| {
            let cube = match bounds {
                Some(bounds) if paths.reads_cube() => {
                    scratch.refill(bounds, &world, paths.air_shortcut);
                    Some(&*scratch)
                }
                _ => None,
            };
            let (distinct, slot_of) =
                if paths.same_box_once { distinct_boxes(boxes) } else { (boxes.to_vec(), (0..boxes.len()).collect()) };
            let weigh = |b: &Aabb| world.visible_part(cube, paths, center, b);
            let parts: Vec<f32> = match paths.parallel_threads(distinct.len()) {
                // At least `PARALLEL_MIN_BOXES_PER_TASK` boxes a task: fewer threads woken for a
                // small batch.
                Some(threads) => pool_of(threads)
                    .install(|| distinct.par_iter().with_min_len(PARALLEL_MIN_BOXES_PER_TASK).map(weigh).collect()),
                None => distinct.iter().map(weigh).collect(),
            };
            slot_of.iter().map(|&i| parts[i]).collect()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Floats in [0, 1) of a small fixed generator.
    struct Draws(u64);

    impl Draws {
        fn next(&mut self) -> f64 {
            self.0 = self.0.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (self.0 >> 11) as f64 / (1u64 << 53) as f64
        }
    }

    /// Segments of every kind: random, along one axis, along two, on whole and half coordinates,
    /// very short, and from a whole point.
    fn segments() -> Vec<([f64; 3], [f64; 3])> {
        let mut r = Draws(5);
        let mut out = Vec::new();
        for i in 0..4000 {
            let from: [f64; 3] = std::array::from_fn(|_| r.next() * 40.0 - 20.0);
            let mut to: [f64; 3] = std::array::from_fn(|_| r.next() * 40.0 - 20.0);
            match i % 8 {
                0 => to = [from[0], from[1], to[2]],
                1 => to = [to[0], from[1], from[2]],
                2 => to = [to[0], to[1], from[2]],
                3 => to = to.map(f64::round),
                4 => to = std::array::from_fn(|a| from[a] + (r.next() - 0.5) * 1e-6),
                5 => to = to.map(|v| (v * 2.0).round() / 2.0),
                _ => {}
            }
            let from = if i % 16 == 3 { from.map(f64::round) } else { from };
            out.push((from, to));
        }
        out.push(([0.5, -60.0, 0.5], [3.5, -59.938_75, 0.5]));
        out.push(([1.0, 2.0, 3.0], [1.0, 2.0, 7.0]));
        out
    }

    /// The range a walk announces is exactly the lowest and highest cell it visits on each axis.
    #[test]
    fn cell_range_is_the_walks_own_range() {
        let mut checked = 0;
        for (from, to) in segments() {
            if from == to {
                continue;
            }
            let mut walk = CellWalk::new(from, to);
            let announced = walk.cell_range().expect("finite segment");
            let (mut low, mut high) = (walk.cell(), walk.cell());
            while walk.advance() {
                for axis in 0..3 {
                    low[axis] = low[axis].min(walk.cell()[axis]);
                    high[axis] = high[axis].max(walk.cell()[axis]);
                }
            }
            assert_eq!(announced, (low, high), "segment {from:?} to {to:?}");
            checked += 1;
        }
        assert!(checked > 3900, "too few segments: {checked}");
    }

    /// A segment whose crossing fractions are not finite, or too long, announces no range (the
    /// walk then runs).
    #[test]
    fn cell_range_refuses_what_it_cannot_count() {
        assert_eq!(CellWalk::new([0.5, 0.5, 0.5], [f64::NAN, 0.5, 0.5]).cell_range(), None);
        assert_eq!(CellWalk::new([f64::NAN, 0.5, 0.5], [3.5, 1.5, 0.5]).cell_range(), None);
        assert_eq!(CellWalk::new([0.5, 0.5, 0.5], [f64::INFINITY, 0.5, 0.5]).cell_range(), None);
        assert_eq!(CellWalk::new([0.5, 0.5, 0.5], [5000.5, 0.5, 0.5]).cell_range(), None);
        assert!(CellWalk::new([0.5, 0.5, 0.5], [900.5, 0.5, 0.5]).cell_range().is_some());
    }

    /// The running count of shaped cells answers like a cell by cell count, for boxes inside the
    /// cube, and no for a box that leaves it; the cube holds the world's states.
    #[test]
    fn shape_count_matches_a_cell_by_cell_count() {
        let (mut p, _dir) = crate::tests_g3f13::open("visible-part-count");
        let blocks = ["oak_fence", "glass_pane", "stone_slab", "oak_carpet", "torch", "stone", "air", "water", "chain"];
        let mut r = Draws(9);
        for _ in 0..300 {
            let (x, y, z) = ((r.next() * 20.0) as i32 + 30, (r.next() * 10.0) as i32 - 61, (r.next() * 20.0) as i32 + 5);
            let block = blocks[(r.next() * blocks.len() as f64) as usize];
            let src = p.s.server_source();
            let _ = p.s.perform(src, &format!("setblock {x} {y} {z} minecraft:{block}"));
        }
        let world = p.s.sight_world();
        let area = Aabb { min: [31.2, -60.0, 6.3], max: [49.0, -52.5, 24.1] };
        let bounds = cube_bounds([40.5, -56.0, 15.5], &[area]).expect("cube bounds");
        let mut cube = SightCube::default();
        cube.refill(bounds, &world, true);
        let shaped = |p: [i32; 3]| !world.phys.shape(world.state(p)).is_empty();
        let mut answers = [0; 2];
        for _ in 0..3000 {
            let low: [i32; 3] =
                std::array::from_fn(|a| bounds.origin[a] + (r.next() * (bounds.size[a] as f64 + 2.0)) as i32 - 1);
            let high: [i32; 3] = std::array::from_fn(|a| low[a] + (r.next() * 4.0) as i32);
            let inside =
                (0..3).all(|a| low[a] >= bounds.origin[a] && high[a] < bounds.origin[a] + bounds.size[a] as i32);
            let none = (low[0]..=high[0])
                .all(|x| (low[1]..=high[1]).all(|y| (low[2]..=high[2]).all(|z| !shaped([x, y, z]))));
            let expected = inside && none;
            assert_eq!(cube.holds_no_shape(low, high), expected, "cells {low:?} to {high:?}");
            answers[usize::from(expected)] += 1;
        }
        assert!(answers.iter().all(|&n| n > 300), "both answers must be tested: {answers:?}");
        // The states, here and across sections, below the world, at its top, past the loaded
        // chunks.
        let areas = [
            area,
            Aabb { min: [28.0, -75.0, 3.0], max: [37.0, -40.0, 9.0] },
            Aabb { min: [28.0, 300.0, 3.0], max: [33.0, 330.0, 7.0] },
            Aabb { min: [150.0, -62.0, 40.0], max: [170.0, -58.0, 52.0] },
        ];
        for area in areas {
            let bounds = cube_bounds(area.min, &[area]).expect("cube bounds");
            cube.refill(bounds, &world, false);
            for x in 0..bounds.size[0] as i32 {
                for z in 0..bounds.size[2] as i32 {
                    for y in 0..bounds.size[1] as i32 {
                        let at = [bounds.origin[0] + x, bounds.origin[1] + y, bounds.origin[2] + z];
                        assert_eq!(cube.state(at), Some(world.state(at)), "state at {at:?}");
                    }
                }
            }
        }
    }

    /// The cells announced for a layer's sightlines hold every cell their walks visit, and all
    /// the points of a layer have its height.
    #[test]
    fn layer_sightline_cells_hold_every_walk_of_the_layer() {
        let mut r = Draws(13);
        let mut walks = 0;
        for i in 0..3000 {
            let center: [f64; 3] = std::array::from_fn(|_| r.next() * 20.0 - 10.0);
            let size = [r.next() * 2.0, r.next() * 3.0, r.next() * 2.0].map(|v| if i % 7 == 0 { v * 0.2 - 0.1 } else { v });
            let corner: [f64; 3] = std::array::from_fn(|a| center[a] + r.next() * 16.0 - 8.0);
            let corner = if i % 3 == 0 { corner.map(f64::round) } else { corner };
            let b = Aabb { min: corner, max: std::array::from_fn(|a| corner[a] + size[a]) };
            let mut heights: Vec<f64> = Vec::new();
            for_each_sample(&b, |sample, layer| {
                if layer == heights.len() {
                    heights.push(sample[1]);
                }
                assert_eq!(sample[1].to_bits(), heights[layer].to_bits(), "box {b:?}: layer {layer} height");
                let (low, high) = layer_sightline_cells(center, &b, sample[1]).expect("finite box");
                if sample == center {
                    return;
                }
                walks += 1;
                let mut walk = CellWalk::new(sample, center);
                loop {
                    let c = walk.cell();
                    assert!((0..3).all(|a| low[a] <= c[a] && c[a] <= high[a]), "box {b:?} centre {center:?}: cell {c:?}");
                    if !walk.advance() {
                        break;
                    }
                }
            });
        }
        assert!(walks > 50_000, "too few walks: {walks}");
    }

    /// Equal boxes share their first index; boxes equal but for one bound (or a zero's sign) do
    /// not.
    #[test]
    fn distinct_boxes_keep_the_first_of_each() {
        let a = Aabb { min: [0.0, 1.0, 2.0], max: [1.0, 2.0, 3.0] };
        let b = Aabb { max: [1.0, 2.0, 3.5], ..a };
        let c = Aabb { min: [-0.0, 1.0, 2.0], ..a };
        let (distinct, slots) = distinct_boxes(&[a, b, a, c, b, a]);
        assert_eq!(distinct, vec![a, b, c]);
        assert_eq!(slots, vec![0, 1, 0, 2, 1, 0]);
    }
}
