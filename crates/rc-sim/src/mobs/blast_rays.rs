//! The rays of an explosion or a wind burst (their first phase): 1,352 rays from the centre
//! towards the cells on the surface of a 16³ cube, each with a power drawn on the world random,
//! stepping 0.3 block at a time and losing power in every cell they cross. A cell reached with
//! power left is hit; the hit cells come out in the order they were first reached, which is the
//! order the shuffle of the destruction starts from.
//!
//! The world does not change while the rays are walked, so what a cell does to a ray is the same
//! for every ray: it is worked out once per explosion and kept in an array over the cube the rays
//! can reach, next to a bit that says whether the cell is already among the hit ones.

use std::sync::OnceLock;

use hashbrown::HashMap;

use crate::server::Server;
use crate::tag_bits::{TagBits, TagId};

/// Rays of an explosion: one per cell on the surface of a 16³ cube.
pub(crate) const RAY_COUNT: usize = 16 * 16 * 16 - 14 * 14 * 14;

/// Power every step loses, whatever the cell.
const STEP_COST: f32 = 0.225_000_01;

/// What a cell does to a ray.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CellEffect {
    /// Power taken, `(resistance + 0.3) × 0.3`; 0 for a cell that does not resist (air, an
    /// ignored portal): subtracting 0 leaves the power exactly as it was.
    pub drop: f32,
    /// The cell joins the hit cells when power is left (false: ignored portal, spared rail).
    pub hittable: bool,
}

impl CellEffect {
    /// A cell that takes no power and can be hit (air).
    pub(crate) const OPEN: CellEffect = CellEffect { drop: 0.0, hittable: true };
    /// A cell the explosion ignores entirely (portal crossed by its TNT).
    pub(crate) const IGNORED: CellEffect = CellEffect { drop: 0.0, hittable: false };

    /// A cell of resistance `resistance`.
    pub(crate) fn resisting(resistance: f32, hittable: bool) -> CellEffect {
        CellEffect { drop: (resistance + 0.3) * 0.3, hittable }
    }
}

/// Paths of the explosion engine, and of the mobs' block clip its visible parts go through, one
/// switch each (`=0`: the former path), read once.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct BlastPaths {
    /// `RUSTCRAFT_OPT_BLAST_LAST_CELL`: a step in the same cell as the step before takes the
    /// same power and skips the height test, the cell lookup and the hit test (about two steps
    /// in three).
    pub last_cell: bool,
    /// `RUSTCRAFT_OPT_BLAST_CELL_CACHE`: each cell worked out once per explosion.
    pub cell_cache: bool,
    /// `RUSTCRAFT_OPT_BLAST_HIT_BITS`: a bit per cell says whether it is already hit (formerly a
    /// search through the hit list at every step).
    pub hit_bits: bool,
    /// `RUSTCRAFT_OPT_BLAST_ONE_STATE_READ`: resistance, fluid, air test and the explosion's
    /// rules of a cell from one state read (formerly four to six reads of the same cell).
    pub one_state_read: bool,
    /// `RUSTCRAFT_OPT_BLAST_PARALLEL_RAYS` (default off): from `parallel_min_radius`, the rays
    /// are shared between threads.
    pub parallel_rays: bool,
    /// `RUSTCRAFT_OPT_SHARED_RAY_WALKER`: the wind burst walks its rays with this walker
    /// (formerly its own loop with a hash set and a tag test per step).
    pub shared_walker: bool,
    /// `RUSTCRAFT_OPT_SIGHTLINE_BORROWED_SHAPES`: every block clip of the mobs' segments
    /// (`clip_blocks` / `clip_t`: mob sight and targeting, breeze, golems, llama spit, spears,
    /// surface navigation, slimes, advancement predicates, and the visible part of an
    /// explosion's targets) borrows each cell's collision shape (formerly copied into a new list
    /// per cell). Kept here so that the explosion proofs switch it with the rest; it is not an
    /// explosion path only.
    pub sightline_borrowed_shapes: bool,
    /// `RUSTCRAFT_OPT_BLAST_LOOKUPS_ONCE`: rules, block ids, particle and sound ids looked up
    /// once per explosion (formerly per destroyed block, and by name).
    pub lookups_once: bool,
    /// `RUSTCRAFT_OPT_BLAST_MOBS_IN_BOX`: only the mobs in the game's box around the explosion
    /// are gathered (formerly every mob of the list, then the distance test).
    pub mobs_in_box: bool,
    /// `RUSTCRAFT_OPT_BLAST_OPEN_STACKS`: a drop is compared only with the loot stacks that still
    /// have room (formerly with every stack of the explosion).
    pub open_stacks: bool,
    /// Threads of the parallel rays (0: `RUSTCRAFT_THREADS`, otherwise every logical core).
    pub threads: usize,
    /// Smallest radius walked in parallel.
    pub parallel_min_radius: f32,
    /// Cells of the largest cube kept as arrays; a bigger explosion keeps its cells in hash maps.
    pub max_cube_cells: usize,
}

/// Largest cube kept as arrays (5 bytes a cell: 10 MB, radius 36 or so).
const MAX_CUBE_CELLS: usize = 1 << 21;

#[cfg(test)]
thread_local! {
    /// Tests: paths of the explosions of this thread (`None`: the switches).
    pub(crate) static FORCED_BLAST_PATHS: std::cell::Cell<Option<BlastPaths>> = const { std::cell::Cell::new(None) };
}

impl BlastPaths {
    /// Every new path (the parallel rays off: they are measured separately).
    pub(crate) const NEW: BlastPaths = BlastPaths {
        last_cell: true,
        cell_cache: true,
        hit_bits: true,
        one_state_read: true,
        parallel_rays: false,
        shared_walker: true,
        sightline_borrowed_shapes: true,
        lookups_once: true,
        mobs_in_box: true,
        open_stacks: true,
        threads: 0,
        parallel_min_radius: 7.0,
        max_cube_cells: MAX_CUBE_CELLS,
    };
    /// Every former path.
    #[cfg(test)]
    pub(crate) const OLD: BlastPaths = BlastPaths {
        last_cell: false,
        cell_cache: false,
        hit_bits: false,
        one_state_read: false,
        shared_walker: false,
        sightline_borrowed_shapes: false,
        lookups_once: false,
        mobs_in_box: false,
        open_stacks: false,
        ..BlastPaths::NEW
    };

    pub(crate) fn current() -> BlastPaths {
        #[cfg(test)]
        if let Some(forced) = FORCED_BLAST_PATHS.get() {
            return forced;
        }
        static PATHS: OnceLock<BlastPaths> = OnceLock::new();
        *PATHS.get_or_init(|| BlastPaths {
            last_cell: rc_data::opt_switch!("RUSTCRAFT_OPT_BLAST_LAST_CELL"),
            cell_cache: rc_data::opt_switch!("RUSTCRAFT_OPT_BLAST_CELL_CACHE"),
            hit_bits: rc_data::opt_switch!("RUSTCRAFT_OPT_BLAST_HIT_BITS"),
            one_state_read: rc_data::opt_switch!("RUSTCRAFT_OPT_BLAST_ONE_STATE_READ"),
            parallel_rays: rc_data::opt_switch!("RUSTCRAFT_OPT_BLAST_PARALLEL_RAYS", default_off),
            shared_walker: rc_data::opt_switch!("RUSTCRAFT_OPT_SHARED_RAY_WALKER"),
            sightline_borrowed_shapes: rc_data::opt_switch!("RUSTCRAFT_OPT_SIGHTLINE_BORROWED_SHAPES"),
            lookups_once: rc_data::opt_switch!("RUSTCRAFT_OPT_BLAST_LOOKUPS_ONCE"),
            mobs_in_box: rc_data::opt_switch!("RUSTCRAFT_OPT_BLAST_MOBS_IN_BOX"),
            open_stacks: rc_data::opt_switch!("RUSTCRAFT_OPT_BLAST_OPEN_STACKS"),
            threads: rc_data::env_str!("RUSTCRAFT_THREADS").and_then(|v| v.trim().parse().ok()).unwrap_or(0),
            ..BlastPaths::NEW
        })
    }

    /// Threads the rays of an explosion of `radius` are shared between (1: walked in place).
    /// The shared walk always takes the new cell paths (shared view, last cell, cell cache, hit
    /// bits): with any of them switched off, the rays stay on one thread so that the former path
    /// really runs.
    fn ray_threads(&self, radius: f32) -> usize {
        let new_cell_paths = self.one_state_read && self.last_cell && self.cell_cache && self.hit_bits;
        if !self.parallel_rays || !new_cell_paths || radius < self.parallel_min_radius {
            return 1;
        }
        let threads = match self.threads {
            0 => std::thread::available_parallelism().map_or(1, std::num::NonZero::get),
            n => n,
        };
        threads.clamp(1, RAY_COUNT / 32)
    }
}

/// Step of each ray, 0.3 block along its direction, in the game's ray order (cells on the
/// surface of the 16³ cube, x then y then z): the same products as the game's walk.
fn ray_steps() -> &'static [[f64; 3]; RAY_COUNT] {
    static STEPS: OnceLock<[[f64; 3]; RAY_COUNT]> = OnceLock::new();
    STEPS.get_or_init(|| {
        let mut steps = [[0.0; 3]; RAY_COUNT];
        let mut n = 0;
        for x in 0..16 {
            for y in 0..16 {
                for z in 0..16 {
                    if !(x == 0 || x == 15 || y == 0 || y == 15 || z == 0 || z == 15) {
                        continue;
                    }
                    let dir = [x, y, z].map(|c| f64::from(c as f32 / 15.0 * 2.0 - 1.0));
                    let length = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
                    steps[n] = dir.map(|c| c / length * f64::from(0.3f32));
                    n += 1;
                }
            }
        }
        steps
    })
}

/// Farthest a ray of an explosion of `radius` can get from the centre, with a margin: the power
/// starts below 1.3 × radius and every step of 0.3 block costs at least 0.225.
fn ray_reach(radius: f32) -> f64 {
    f64::from(radius) * 1.74 + 1.5
}

const KNOWN: u8 = 1;
const HITTABLE: u8 = 2;
const HIT: u8 = 4;

/// Cells within reach of an explosion: each cell's effect once worked out, and whether it is
/// already among the hit cells. Arrays over the cube the rays can reach; a hash map for a cube
/// too big to keep as arrays, and for any cell outside the cube (none is expected).
struct CellCube {
    origin: [i32; 3],
    size: [usize; 3],
    drops: Vec<f32>,
    flags: Vec<u8>,
    outside: HashMap<[i32; 3], (f32, u8)>,
}

impl CellCube {
    /// The cells every ray of an explosion of `radius` at `center` can reach.
    fn new(center: [f64; 3], radius: f32, max_cells: usize) -> CellCube {
        let reach = ray_reach(radius);
        CellCube::spanning(center.map(|c| c - reach), center.map(|c| c + reach), reach.is_finite(), max_cells)
    }

    /// The cells the rays `steps` of an explosion of `radius` at `center` can reach: the box of
    /// the centre and of each ray's farthest point (a segment's cells lie in its ends' box).
    fn around_rays(center: [f64; 3], steps: &[[f64; 3]], radius: f32, max_cells: usize) -> CellCube {
        let reach = ray_reach(radius);
        let steps_to_reach = reach / f64::from(0.3f32);
        let (mut lo, mut hi) = (center, center);
        for step in steps {
            for axis in 0..3 {
                let end = center[axis] + step[axis] * steps_to_reach;
                lo[axis] = lo[axis].min(end);
                hi[axis] = hi[axis].max(end);
            }
        }
        CellCube::spanning(lo, hi, reach.is_finite(), max_cells)
    }

    /// Arrays over the cells from `lo` to `hi` when they are finite and at most `max_cells`,
    /// otherwise none (every cell then goes to the hash map).
    fn spanning(lo: [f64; 3], hi: [f64; 3], finite: bool, max_cells: usize) -> CellCube {
        let lo = lo.map(f64::floor);
        let hi = hi.map(f64::floor);
        let size: [usize; 3] = std::array::from_fn(|i| (hi[i] - lo[i] + 1.0).max(0.0) as usize);
        let cells = size.iter().try_fold(1usize, |n, &s| n.checked_mul(s)).filter(|&n| finite && n <= max_cells);
        let (size, cells) = match cells {
            Some(n) => (size, n),
            None => ([0; 3], 0),
        };
        CellCube {
            origin: lo.map(|c| c as i32),
            size,
            drops: vec![0.0; cells],
            flags: vec![0; cells],
            outside: HashMap::new(),
        }
    }

    fn slot(&self, p: [i32; 3]) -> Option<usize> {
        let mut index = 0;
        for axis in [1, 2, 0] {
            let offset = usize::try_from(p[axis].wrapping_sub(self.origin[axis])).ok()?;
            if offset >= self.size[axis] {
                return None;
            }
            index = index * self.size[axis] + offset;
        }
        Some(index)
    }

    /// Power drop and flags kept for cell `p`.
    fn cell_mut(&mut self, p: [i32; 3]) -> (&mut f32, &mut u8) {
        match self.slot(p) {
            Some(i) => (&mut self.drops[i], &mut self.flags[i]),
            None => {
                let (drop, flags) = self.outside.entry(p).or_insert((0.0, 0));
                (drop, flags)
            }
        }
    }

    /// Effect of cell `p`, worked out by `effect_of` the first time only.
    fn effect(&mut self, p: [i32; 3], effect_of: &impl Fn([i32; 3]) -> CellEffect) -> CellEffect {
        let (drop, flags) = self.cell_mut(p);
        if *flags & KNOWN == 0 {
            let e = effect_of(p);
            *drop = e.drop;
            *flags |= KNOWN | if e.hittable { HITTABLE } else { 0 };
        }
        CellEffect { drop: *drop, hittable: *flags & HITTABLE != 0 }
    }

    /// Marks cell `p` hit; true if it was not yet.
    fn mark_hit(&mut self, p: [i32; 3]) -> bool {
        let (_, flags) = self.cell_mut(p);
        let new = *flags & HIT == 0;
        *flags |= HIT;
        new
    }
}

/// What a ray does with the cells it crosses.
trait RayCells {
    /// Effect of cell `p` on the ray.
    fn effect(&mut self, p: [i32; 3]) -> CellEffect;
    /// Cell `p` reached with power left, its effect hittable.
    fn reached(&mut self, p: [i32; 3]);
}

/// The hit cells of a walk on one thread, in the order first reached.
struct HitCells<F> {
    paths: BlastPaths,
    cube: CellCube,
    hit: Vec<[i32; 3]>,
    effect_of: F,
}

impl<F: Fn([i32; 3]) -> CellEffect> RayCells for HitCells<F> {
    fn effect(&mut self, p: [i32; 3]) -> CellEffect {
        if self.paths.cell_cache { self.cube.effect(p, &self.effect_of) } else { (self.effect_of)(p) }
    }

    fn reached(&mut self, p: [i32; 3]) {
        let new = if self.paths.hit_bits { self.cube.mark_hit(p) } else { !self.hit.contains(&p) };
        if new {
            self.hit.push(p);
        }
    }
}

/// The cells reached by a share of the rays on a worker thread, in (ray, step) order, repeats
/// left in: they are merged afterwards, in ray order, into the hit cells.
struct ReachedCells<'f, F> {
    cache: CellCube,
    reached: Vec<[i32; 3]>,
    effect_of: &'f F,
}

impl<F: Fn([i32; 3]) -> CellEffect> RayCells for ReachedCells<'_, F> {
    fn effect(&mut self, p: [i32; 3]) -> CellEffect {
        self.cache.effect(p, self.effect_of)
    }

    fn reached(&mut self, p: [i32; 3]) {
        self.reached.push(p);
    }
}

/// The world's height range, `[bottom, top)`.
#[derive(Clone, Copy)]
pub(crate) struct Heights {
    pub bottom: i32,
    pub top: i32,
}

/// One ray from `center` with `power`, stepping by `step`.
fn walk_ray(
    center: [f64; 3],
    step: [f64; 3],
    mut power: f32,
    heights: Heights,
    last_cell: bool,
    cells: &mut impl RayCells,
) {
    let mut pos = center;
    let mut last: Option<([i32; 3], f32)> = None;
    while power > 0.0 {
        let p = pos.map(|v| v.floor() as i32);
        match last {
            // Same cell as the step before: same power taken, and the cell is already hit (power
            // only falls along a ray, so the step before had power left in it) or never hittable.
            Some((cell, drop)) if last_cell && cell == p => power -= drop,
            _ => {
                if p[1] < heights.bottom || p[1] >= heights.top {
                    break;
                }
                let effect = cells.effect(p);
                power -= effect.drop;
                if power > 0.0 && effect.hittable {
                    cells.reached(p);
                }
                last = Some((p, effect.drop));
            }
        }
        pos = [pos[0] + step[0], pos[1] + step[1], pos[2] + step[2]];
        power -= STEP_COST;
    }
}

/// Starting power of each ray: `draws` are the world random's floats, one per ray in order.
fn ray_power(radius: f32, draw: f32) -> f32 {
    radius * (0.7 + draw * 0.6)
}

/// The hit cells of the rays of an explosion of `radius` at `center`, in the order first reached;
/// `draws`: one float of the world random per ray, in ray order; `effect_of`: what a cell does
/// to a ray (it must give the same answer for the whole walk).
pub(crate) fn hit_cells(
    center: [f64; 3],
    radius: f32,
    draws: &[f32; RAY_COUNT],
    heights: Heights,
    paths: BlastPaths,
    effect_of: impl Fn([i32; 3]) -> CellEffect,
) -> Vec<[i32; 3]> {
    let cube_cells = if paths.cell_cache || paths.hit_bits { paths.max_cube_cells } else { 0 };
    let mut cells = HitCells { paths, cube: CellCube::new(center, radius, cube_cells), hit: Vec::new(), effect_of };
    for (step, &draw) in ray_steps().iter().zip(draws) {
        walk_ray(center, *step, ray_power(radius, draw), heights, paths.last_cell, &mut cells);
    }
    cells.hit
}

/// `hit_cells` with the rays shared between `threads` threads (contiguous shares, each with
/// its own cell cache), merged in ray order: the same cells in the same order. It always takes
/// the last-cell step, the cell cache and the hit bits (`ray_threads` keeps the former paths on
/// one thread).
pub(crate) fn hit_cells_parallel(
    center: [f64; 3],
    radius: f32,
    draws: &[f32; RAY_COUNT],
    heights: Heights,
    paths: BlastPaths,
    threads: usize,
    effect_of: impl Fn([i32; 3]) -> CellEffect + Sync,
) -> Vec<[i32; 3]> {
    let per = RAY_COUNT.div_ceil(threads.max(1));
    let effect_of = &effect_of;
    let shares: Vec<Vec<[i32; 3]>> = std::thread::scope(|scope| {
        let workers: Vec<_> = ray_steps()
            .chunks(per)
            .zip(draws.chunks(per))
            .map(|(steps, draws)| {
                scope.spawn(move || {
                    // Only the cells this share's rays can reach: a whole cube per thread would
                    // multiply the memory of the walk by the thread count.
                    let cache = CellCube::around_rays(center, steps, radius, paths.max_cube_cells);
                    let mut cells = ReachedCells { cache, reached: Vec::new(), effect_of };
                    for (step, &draw) in steps.iter().zip(draws) {
                        walk_ray(center, *step, ray_power(radius, draw), heights, true, &mut cells);
                    }
                    cells.reached
                })
            })
            .collect();
        workers.into_iter().map(|w| w.join().expect("ray thread")).collect()
    });
    let mut seen = CellCube::new(center, radius, paths.max_cube_cells);
    let mut hit = Vec::new();
    for p in shares.into_iter().flatten() {
        if seen.mark_hit(p) {
            hit.push(p);
        }
    }
    hit
}

/// The explosion's own rules for its cells.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ExplosionRules {
    /// Respawn anchor in water: this cell resists like water, even empty.
    pub water_cell: Option<[i32; 3]>,
    /// TNT that went through a portal: the Nether portal neither resists nor is hit.
    pub portal_immune: bool,
    pub nether_portal: u16,
    /// Primed TNT minecart: rails and the block under them resist nothing and are never hit.
    pub rail_safe: bool,
    /// Blue wither skull: destructible blocks resist 0.8 at most.
    pub dangerous_wither: bool,
}

/// Read-only view of what the rays need, shareable between threads: the chunks, the state
/// tables and the block tags.
pub(crate) struct BlastWorld<'a> {
    chunks: &'a HashMap<(i32, i32), Option<Box<crate::chunk::Chunk>>>,
    bottom: i32,
    data: &'a rc_data::VersionData,
    fluid_mob: &'a [u8],
    tag_bits: &'a TagBits,
    block_tags: &'a HashMap<String, Vec<u32>>,
    tag_mode: u8,
}

impl BlastWorld<'_> {
    fn state(&self, p: [i32; 3]) -> u32 {
        crate::play::block_at(self.chunks, self.bottom, p).unwrap_or(0)
    }

    pub(crate) fn has_tag(&self, state: u32, t: TagId) -> bool {
        let block = self.data.state(state).map_or(u32::MAX, |s| u32::from(s.block));
        crate::mobs::env::block_tag_member(self.tag_bits, self.block_tags, t, block, self.tag_mode)
    }

    fn is_air(&self, state: u32) -> bool {
        self.data.state(state).is_some_and(|i| i.has(rc_data::flags::AIR))
    }

    /// The registry resistance of a state, 100 for water and lava, `None` for air without fluid.
    pub(crate) fn resistance(&self, state: u32) -> Option<f32> {
        let fluid = self.data.state(state).filter(|i| i.fluid != 0).and_then(|i| self.fluid_mob.get(i.fluid as usize));
        let in_fluid = matches!(fluid, Some(1 | 2));
        if self.is_air(state) && !in_fluid {
            return None;
        }
        let block = self.data.blast.get(state as usize).copied().unwrap_or(0.0);
        Some(block.max(if in_fluid { 100.0 } else { 0.0 }))
    }

    /// What cell `p` does to the rays of an explosion with `rules`.
    pub(crate) fn explosion_effect(&self, p: [i32; 3], rules: &ExplosionRules) -> CellEffect {
        let state = self.state(p);
        let block = self.data.state(state).map_or(u16::MAX, |i| i.block);
        if rules.portal_immune && block == rules.nether_portal {
            return CellEffect::IGNORED;
        }
        let spared = rules.rail_safe
            && (self.has_tag(state, TagId::Rails) || self.has_tag(self.state([p[0], p[1] + 1, p[2]]), TagId::Rails));
        let resistance = match rules.water_cell {
            Some(w) if w == p => Some(100.0),
            _ => self.resistance(state),
        };
        match resistance {
            None => CellEffect { drop: 0.0, hittable: !spared },
            Some(_) if spared => CellEffect::resisting(0.0, false),
            Some(r) if rules.dangerous_wither && !self.is_air(state) && !self.has_tag(state, TagId::WitherImmune) => {
                CellEffect::resisting(r.min(0.8), true)
            }
            Some(r) => CellEffect::resisting(r, true),
        }
    }

    /// What cell `p` does to the rays of a wind burst: only the blocks of the
    /// `blocks_wind_charge_explosions` tag stop them.
    pub(crate) fn wind_effect(&self, p: [i32; 3]) -> CellEffect {
        if self.has_tag(self.state(p), TagId::BlocksWindChargeExplosions) {
            CellEffect::resisting(3_600_000.0, true)
        } else {
            CellEffect::OPEN
        }
    }
}

impl Server {
    /// One float of the world random per ray, in ray order: the walk draws nothing else, so
    /// drawing them up front leaves the random as the game's interleaved draws do.
    pub(crate) fn ray_draws(&mut self) -> [f32; RAY_COUNT] {
        std::array::from_fn(|_| self.mobs.rng.next_float())
    }

    pub(crate) fn world_heights(&self) -> Heights {
        let h = self.dim.height;
        Heights { bottom: h.min_y, top: h.min_y + h.sections as i32 * 16 }
    }

    pub(crate) fn blast_world(&self) -> BlastWorld<'_> {
        BlastWorld {
            chunks: &self.chunks,
            bottom: self.dim.height.min_y,
            data: &self.data,
            fluid_mob: &self.bs.t.fluid_mob,
            tag_bits: &self.tag_bits,
            block_tags: &self.block_tags,
            tag_mode: crate::tag_bits::mode(),
        }
    }

    /// The hit cells of an explosion of `radius` at `center` whose cells act through
    /// `effect_of`, on one thread or shared between threads by `paths`.
    pub(crate) fn walk_blast_rays(
        &mut self,
        center: [f64; 3],
        radius: f32,
        paths: BlastPaths,
        effect_of: impl Fn(&BlastWorld, [i32; 3]) -> CellEffect + Sync,
    ) -> Vec<[i32; 3]> {
        let draws = self.ray_draws();
        let heights = self.world_heights();
        let world = self.blast_world();
        let threads = paths.ray_threads(radius);
        if threads > 1 {
            return hit_cells_parallel(center, radius, &draws, heights, paths, threads, |p| effect_of(&world, p));
        }
        hit_cells(center, radius, &draws, heights, paths, |p| effect_of(&world, p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A made-up world: resistance (`None`: air) and hittability drawn from a cell's coordinates.
    fn made_up_cell(p: [i32; 3]) -> (Option<f32>, bool) {
        let h = (p[0].wrapping_mul(73_856_093) ^ p[1].wrapping_mul(19_349_663) ^ p[2].wrapping_mul(83_492_791)) as u32;
        match h % 9 {
            0..=3 => (None, true),
            4 => (Some(0.5), true),
            5 => (Some(3.0), true),
            6 => (Some(100.0), true),
            7 => (None, false),
            _ => (Some(0.0), false),
        }
    }

    fn made_up_effect(p: [i32; 3]) -> CellEffect {
        match made_up_cell(p) {
            (Some(r), hittable) => CellEffect::resisting(r, hittable),
            (None, hittable) => CellEffect { drop: 0.0, hittable },
        }
    }

    /// The walk as the game writes it: every step reads its cell, subtracts its resistance when
    /// it has one, then searches the hit list.
    fn reference_walk(center: [f64; 3], radius: f32, draws: &[f32; RAY_COUNT], heights: Heights) -> Vec<[i32; 3]> {
        let mut hit: Vec<[i32; 3]> = Vec::new();
        let mut ray = 0;
        for xx in 0..16 {
            for yy in 0..16 {
                for zz in 0..16 {
                    if !(xx == 0 || xx == 15 || yy == 0 || yy == 15 || zz == 0 || zz == 15) {
                        continue;
                    }
                    let mut d = [xx, yy, zz].map(|c| f64::from(c as f32 / 15.0 * 2.0 - 1.0));
                    let length = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    d = d.map(|c| c / length);
                    let mut power = radius * (0.7 + draws[ray] * 0.6);
                    ray += 1;
                    let mut pos = center;
                    while power > 0.0 {
                        let p = pos.map(|v| v.floor() as i32);
                        if p[1] < heights.bottom || p[1] >= heights.top {
                            break;
                        }
                        let (resistance, hittable) = made_up_cell(p);
                        if let Some(r) = resistance {
                            power -= (r + 0.3) * 0.3;
                        }
                        if power > 0.0 && hittable && !hit.contains(&p) {
                            hit.push(p);
                        }
                        for a in 0..3 {
                            pos[a] += d[a] * f64::from(0.3f32);
                        }
                        power -= 0.225_000_01;
                    }
                }
            }
        }
        hit
    }

    /// Floats in [0, 1) of a small fixed generator.
    fn draws(seed: u64) -> [f32; RAY_COUNT] {
        let mut x = seed;
        std::array::from_fn(|_| {
            x = x.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (x >> 40) as f32 / (1u64 << 24) as f32
        })
    }

    /// Every combination of the walk's paths, with and without the cube arrays.
    fn every_walk_path() -> impl Iterator<Item = BlastPaths> {
        (0..16u8).map(|bits| BlastPaths {
            last_cell: bits & 1 != 0,
            cell_cache: bits & 2 != 0,
            hit_bits: bits & 4 != 0,
            max_cube_cells: if bits & 8 != 0 { MAX_CUBE_CELLS } else { 0 },
            ..BlastPaths::NEW
        })
    }

    #[test]
    fn walk_gives_the_reference_cells_on_every_path_and_thread_count() {
        let heights = Heights { bottom: -64, top: 320 };
        let cases: [([f64; 3], f32); 9] = [
            ([0.5, 64.0, 0.5], 4.0),
            ([10.3, -63.7, -5.9], 4.0),
            ([-7.25, 319.6, 3.5], 4.0),
            ([1.0, 0.0, 1.0], 1.2),
            ([3.9, 70.1, -2.2], 7.0),
            ([-30.5, 100.5, 12.5], 20.0),
            ([0.5, 0.5, 0.5], 0.0),
            ([0.5, 0.5, 0.5], -3.0),
            ([0.5, 0.5, 0.5], f32::NAN),
        ];
        for (n, (center, radius)) in cases.into_iter().enumerate() {
            let draws = draws(n as u64 + 1);
            let want = reference_walk(center, radius, &draws, heights);
            assert!(radius.is_nan() || radius < 4.0 || want.len() > 20, "case {n}: too few cells to prove anything");
            for paths in every_walk_path() {
                let got = hit_cells(center, radius, &draws, heights, paths, made_up_effect);
                assert_eq!(got, want, "case {n}, {paths:?}");
            }
            for threads in [1, 2, 4, 8, 32] {
                for cube in [0, MAX_CUBE_CELLS] {
                    let paths = BlastPaths { max_cube_cells: cube, ..BlastPaths::NEW };
                    let got = hit_cells_parallel(center, radius, &draws, heights, paths, threads, made_up_effect);
                    assert_eq!(got, want, "case {n}, {threads} threads, cube {cube}");
                }
            }
        }
    }

    /// The cube holds every cell a ray can reach: none spills into the hash map, at any radius
    /// the arrays are kept for.
    #[test]
    fn cube_holds_every_reachable_cell() {
        let heights = Heights { bottom: i32::MIN, top: i32::MAX };
        for radius in [0.5, 1.2, 3.0, 4.0, 6.0, 7.0, 12.0, 20.0, 30.0] {
            let center = [0.75, 0.25, -0.5];
            let mut cells = HitCells {
                paths: BlastPaths::NEW,
                cube: CellCube::new(center, radius, MAX_CUBE_CELLS),
                hit: Vec::new(),
                effect_of: |_| CellEffect::OPEN,
            };
            assert!(!cells.cube.drops.is_empty(), "radius {radius}: no cube");
            // The highest draw (just below 1) gives the longest rays.
            for step in ray_steps() {
                walk_ray(center, *step, ray_power(radius, 0.999_999_94), heights, true, &mut cells);
            }
            assert!(
                cells.cube.outside.is_empty(),
                "radius {radius}: {} cells outside the cube",
                cells.cube.outside.len()
            );
        }
    }

    /// The cube of a thread's share of the rays holds every cell those rays can reach, and is
    /// smaller than the whole cube (cells printed: the memory of the walk on several threads).
    #[test]
    fn share_cube_holds_every_cell_its_rays_reach() {
        let heights = Heights { bottom: i32::MIN, top: i32::MAX };
        let center = [0.75, 0.25, -0.5];
        for radius in [4.0, 7.0, 20.0, 30.0] {
            let whole = CellCube::new(center, radius, MAX_CUBE_CELLS).drops.len();
            for threads in [2, 4, 8, 32, RAY_COUNT / 32] {
                let per = RAY_COUNT.div_ceil(threads);
                let mut largest = 0;
                for steps in ray_steps().chunks(per) {
                    let cache = CellCube::around_rays(center, steps, radius, MAX_CUBE_CELLS);
                    let mut cells = ReachedCells { cache, reached: Vec::new(), effect_of: &|_| CellEffect::OPEN };
                    for step in steps {
                        walk_ray(center, *step, ray_power(radius, 0.999_999_94), heights, true, &mut cells);
                    }
                    let outside = cells.cache.outside.len();
                    assert_eq!(outside, 0, "radius {radius}, {threads} threads: {outside} cells outside the share's cube");
                    largest = largest.max(cells.cache.drops.len());
                }
                assert!(largest < whole, "radius {radius}, {threads} threads: share cube {largest} of {whole} cells");
                eprintln!("radius {radius}, {threads} threads: largest share cube {largest} cells, whole cube {whole}");
            }
        }
    }

    /// The shared walk always takes the new cell paths: with any of them switched off, the rays
    /// stay on one thread so that the former path runs.
    #[test]
    fn parallel_rays_leave_every_former_cell_path_on_one_thread() {
        let parallel = BlastPaths { parallel_rays: true, threads: 4, ..BlastPaths::NEW };
        assert_eq!(parallel.ray_threads(20.0), 4);
        for former in [
            BlastPaths { last_cell: false, ..parallel },
            BlastPaths { cell_cache: false, ..parallel },
            BlastPaths { hit_bits: false, ..parallel },
            BlastPaths { one_state_read: false, ..parallel },
        ] {
            assert_eq!(former.ray_threads(20.0), 1, "{former:?}");
        }
    }
}
