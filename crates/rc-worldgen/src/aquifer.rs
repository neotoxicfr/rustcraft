//! Aquifers (spec generation-terrain § 3.2–3.3): global fluid picker, noise aquifers on
//! 16 × 12 × 16 cells (level, water/lava type, barriers between neighbouring aquifers).

use hashbrown::HashMap;

use crate::Generator;
use crate::math::floor_div;
use crate::palette::{AIR, BlockId};
use crate::rng::Positional;
use crate::sampler::{Ctx, Sid};
use crate::volume::Volume;

/// "Very low" level: no fluid.
pub const WAY_BELOW_MIN_Y: i32 = -32512;

/// Fluid status: `kind` under `level`, air above.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FluidStatus {
    pub level: i32,
    pub kind: BlockId,
}

impl FluidStatus {
    #[inline]
    pub fn at(self, y: i32) -> BlockId {
        if y < self.level { self.kind } else { AIR }
    }
}

/// Global picker: lava under `min(−54, sea)`, default fluid under the sea.
#[derive(Debug, Clone, Copy)]
pub struct FluidPicker {
    pub lava: FluidStatus,
    pub sea: FluidStatus,
}

impl FluidPicker {
    pub fn new(sea_level: i32, default_fluid: BlockId, lava: BlockId) -> Self {
        FluidPicker {
            lava: FluidStatus { level: -54, kind: lava },
            sea: FluidStatus { level: sea_level, kind: default_fluid },
        }
    }

    #[inline]
    pub fn at(&self, y: i32) -> FluidStatus {
        if y < (-54).min(self.sea.level) { self.lava } else { self.sea }
    }
}

/// Functions and randomness of the aquifers (shared by all the chunks).
#[derive(Debug, Clone, Copy)]
pub struct AquiferConfig {
    pub barrier: Sid,
    pub floodedness: Sid,
    pub spread: Sid,
    pub lava: Sid,
    pub exclusion: Sid,
    pub surface_level: Sid,
    pub random: Positional,
}

#[inline]
fn grid_x(b: i32) -> i32 {
    b >> 4
}
#[inline]
fn grid_y(b: i32) -> i32 {
    floor_div(b, 12)
}
#[inline]
fn similarity(d1: i32, d2: i32) -> f64 {
    1.0 - f64::from(d2.wrapping_sub(d1)) / 25.0
}

const SURFACE_OFFSETS: [(i32, i32); 13] =
    [(0, 0), (-2, -1), (-1, -1), (0, -1), (1, -1), (-3, 0), (-2, 0), (-1, 0), (1, 0), (-2, 1), (-1, 1), (0, 1), (1, 1)];

/// Cache of the preliminary surface heights per quart: dense window around the chunk
/// (everything the aquifer reads) and table fallback for the rest.
#[derive(Debug)]
struct SurfaceCache {
    qx0: i32,
    qz0: i32,
    dense: Vec<i32>,
    far: HashMap<(i32, i32), i32>,
}

const SC_W: i32 = 48;
const EMPTY: i32 = i32::MIN;

impl SurfaceCache {
    fn new(min_x: i32, min_z: i32) -> Self {
        SurfaceCache {
            qx0: (min_x >> 2) - 24,
            qz0: (min_z >> 2) - 20,
            dense: vec![EMPTY; (SC_W * SC_W) as usize],
            far: HashMap::new(),
        }
    }

    #[inline]
    fn slot(&self, qx: i32, qz: i32) -> Option<usize> {
        let (a, b) = (qx - self.qx0, qz - self.qz0);
        (a >= 0 && b >= 0 && a < SC_W && b < SC_W).then(|| (a + b * SC_W) as usize)
    }

    fn get(&self, qx: i32, qz: i32) -> Option<i32> {
        match self.slot(qx, qz) {
            Some(i) => (self.dense[i] != EMPTY).then_some(self.dense[i]),
            None => self.far.get(&(qx, qz)).copied(),
        }
    }

    fn put(&mut self, qx: i32, qz: i32, v: i32) {
        match self.slot(qx, qz) {
            Some(i) => self.dense[i] = v,
            None => {
                self.far.insert((qx, qz), v);
            }
        }
    }
}

/// Side of a shared surface tile: 16 quarts, the quarts of 4 × 4 chunks (1 KiB of levels).
const TILE_QUARTS_SHIFT: i32 = 4;
const TILE_QUARTS: i32 = 1 << TILE_QUARTS_SHIFT;

/// Rectangle of quarts whose values are laid out row by row (z major, x minor): a tile, the
/// window of an aquifer. A volume one row high over the same quarts has the same layout.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct QuartRect {
    qx0: i32,
    qz0: i32,
    width: i32,
    depth: i32,
}

/// Quarts along x that two rectangles share on one row: where they sit in each one's layout.
struct SharedRun {
    in_source: std::ops::Range<usize>,
    in_target: std::ops::Range<usize>,
}

impl QuartRect {
    fn of_tile((tx, tz): (i32, i32)) -> Self {
        QuartRect { qx0: tx << TILE_QUARTS_SHIFT, qz0: tz << TILE_QUARTS_SHIFT, width: TILE_QUARTS, depth: TILE_QUARTS }
    }

    /// Quarts of the blocks `x0..=x1`, `z0..=z1`.
    fn of_blocks((x0, z0, x1, z1): (i32, i32, i32, i32)) -> Self {
        let (qx0, qz0) = (x0 >> 2, z0 >> 2);
        QuartRect { qx0, qz0, width: (x1 >> 2) - qx0 + 1, depth: (z1 >> 2) - qz0 + 1 }
    }

    /// Volume that samples each quart once, at y = 0.
    fn volume(self) -> Volume {
        Volume::stepped(self.width, 1, self.depth, self.qx0 << 2, 0, self.qz0 << 2, 4, 1, 4)
    }

    #[inline]
    fn index(self, qx: i32, qz: i32) -> usize {
        ((qx - self.qx0) + (qz - self.qz0) * self.width) as usize
    }

    /// Positions of the tiles that hold a quart of the rectangle.
    fn tiles(self) -> impl Iterator<Item = (i32, i32)> {
        let along = |from: i32, len: i32| (from >> TILE_QUARTS_SHIFT)..=((from + len - 1) >> TILE_QUARTS_SHIFT);
        let (xs, zs) = (along(self.qx0, self.width), along(self.qz0, self.depth));
        zs.flat_map(move |tz| xs.clone().map(move |tx| (tx, tz)))
    }

    /// The quarts `self` shares with `target`, one run per shared row.
    fn runs_shared_with(self, target: QuartRect) -> impl Iterator<Item = SharedRun> {
        let (from, to) = (self.qx0.max(target.qx0), (self.qx0 + self.width).min(target.qx0 + target.width));
        let rows = self.qz0.max(target.qz0)..(self.qz0 + self.depth).min(target.qz0 + target.depth);
        let len = (to - from).max(0) as usize;
        rows.filter(move |_| len > 0).map(move |qz| {
            let (in_source, in_target) = (self.index(from, qz), target.index(from, qz));
            SharedRun { in_source: in_source..in_source + len, in_target: in_target..in_target + len }
        })
    }
}

/// Preliminary surface levels of one tile of quarts, floored as the aquifers keep them, and
/// optionally the 2D climate they were computed from.
#[derive(Debug, Clone)]
pub struct SurfaceLevelTile {
    /// Row by row (z major, x minor).
    levels: Box<[i32]>,
    /// Values over the tile of the caches that computing the levels filled (cache id, values laid
    /// out like the levels): offset, factor and the climate noises under them. Empty when the
    /// climate is not kept.
    climate: Vec<(u32, Box<[f32]>)>,
}

impl SurfaceLevelTile {
    /// Levels of the tile `at`: one volume over its quarts, in a context of its own. Only valid
    /// when the level of a quart depends on nothing but its position (see [`SurfaceLevelTiles`]).
    /// `climate_caches`: caches whose values over the tile are kept with it (exact input only).
    pub fn compute(g: &Generator, cfg: &AquiferConfig, at: (i32, i32), climate_caches: &[u32]) -> Self {
        let v = QuartRect::of_tile(at).volume();
        let mut ctx = Ctx::cached();
        let buf = g.volume(cfg.surface_level, &mut ctx, &v);
        let levels = buf.iter().map(|&level| ctx.floor_f32_to_i32(level)).collect();
        // A cache the search did not read over the whole tile (a spline branch no quart took)
        // holds another volume, or none: it is left out.
        let kept = |&id: &u32| Some((id, ctx.cached_volume(id, &v)?.into()));
        SurfaceLevelTile { levels, climate: climate_caches.iter().filter_map(kept).collect() }
    }

    /// Values over the tile kept for the cache `cache_id`.
    fn climate_of(&self, cache_id: u32) -> Option<&[f32]> {
        self.climate.iter().find(|(id, _)| *id == cache_id).map(|(_, values)| &**values)
    }
}

/// Test sabotages: every value of a tile moved one quart along x, inside its row.
fn move_one_quart_along_rows<T>(tile_values: &mut [T]) {
    for row in tile_values.as_chunks_mut::<{ TILE_QUARTS as usize }>().0 {
        row.rotate_right(1);
    }
}

/// Tiles that hold the quarts of the chunks `from..=to`, along one axis.
#[inline]
fn tiles_of_chunks(from: i32, to: i32) -> std::ops::RangeInclusive<i32> {
    // 4 quarts per chunk.
    (from >> (TILE_QUARTS_SHIFT - 2))..=(to >> (TILE_QUARTS_SHIFT - 2))
}

/// Chunks whose surface the aquifer of a chunk may read, counted from that chunk: its cells are
/// centred in the chunk and in the chunks next to it, and a cell reads the surface from 3 chunks
/// west to 1 chunk east of its centre, and 1 chunk north and south ([`SURFACE_OFFSETS`]).
const SURFACE_REACH_WEST: i32 = 4;
const SURFACE_REACH_EAST: i32 = 2;
const SURFACE_REACH_NORTH_SOUTH: i32 = 2;

/// Preliminary surface levels shared by the aquifers of the chunks of a batch, and of the batches
/// after it.
///
/// An aquifer starts by reading the preliminary surface over the 11 × 11 quarts around its chunk,
/// and each neighbour reads most of the same quarts again (a quart is read by 7.6 chunks on
/// average). When the level of a quart depends on nothing but its position
/// ([`crate::sampler::Program::value_depends_only_on_position`], true for the 26.3 data), it is
/// computed once per tile before the chunks of the batch are generated, and each aquifer copies
/// the levels it reads. A quart no tile holds is computed by the aquifer as before.
///
/// If the blending with old terrain is reproduced one day, the level of a quart will depend on
/// the chunk that reads it (the blend weights belong to the chunk): the chunks that blend must
/// then skip the tiles.
#[derive(Debug, Default)]
pub struct SurfaceLevelTiles {
    tiles: HashMap<(i32, i32), SurfaceLevelTile>,
}

impl SurfaceLevelTiles {
    /// Tiles that the aquifers of `chunks` may read and that are not held yet, each once: those of
    /// the window an aquifer reads at its creation and of the quarts its cells read further out
    /// (otherwise computed point by point, at 5 point noises and 2 splines each).
    pub fn missing_for(&self, chunks: &[(i32, i32)]) -> Vec<(i32, i32)> {
        let mut missing = Vec::new();
        let mut seen = hashbrown::HashSet::new();
        // Chunks listed side by side read the same tiles: the last answer is not looked up again.
        let mut last_read = None;
        for &(cx, cz) in chunks {
            let read = (
                tiles_of_chunks(cx - SURFACE_REACH_WEST, cx + SURFACE_REACH_EAST),
                tiles_of_chunks(cz - SURFACE_REACH_NORTH_SOUTH, cz + SURFACE_REACH_NORTH_SOUTH),
            );
            if last_read.as_ref() == Some(&read) {
                continue;
            }
            for tz in read.1.clone() {
                for tx in read.0.clone() {
                    if !self.tiles.contains_key(&(tx, tz)) && seen.insert((tx, tz)) {
                        missing.push((tx, tz));
                    }
                }
            }
            last_read = Some(read);
        }
        missing
    }

    /// Adds computed tiles.
    pub fn extend(&mut self, tiles: impl IntoIterator<Item = ((i32, i32), SurfaceLevelTile)>) {
        self.tiles.extend(tiles);
    }

    /// Number of tiles held.
    pub fn len(&self) -> usize {
        self.tiles.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tiles.is_empty()
    }

    /// Bytes held by the tiles (levels and climate).
    pub fn bytes(&self) -> usize {
        let of_tile = |tile: &SurfaceLevelTile| {
            size_of_val(&*tile.levels) + tile.climate.iter().map(|(_, values)| size_of_val(&**values)).sum::<usize>()
        };
        self.tiles.values().map(of_tile).sum()
    }

    /// Level of the quart (qx, qz), if a tile holds it.
    pub fn level(&self, qx: i32, qz: i32) -> Option<i32> {
        let at = (qx >> TILE_QUARTS_SHIFT, qz >> TILE_QUARTS_SHIFT);
        Some(self.tiles.get(&at)?.levels[QuartRect::of_tile(at).index(qx, qz)])
    }

    /// Copies into the dense window of `cache` every level held for one of its quarts.
    fn copy_levels_into(&self, cache: &mut SurfaceCache) {
        let window = QuartRect { qx0: cache.qx0, qz0: cache.qz0, width: SC_W, depth: SC_W };
        for at in window.tiles() {
            let Some(tile) = self.tiles.get(&at) else { continue };
            for run in QuartRect::of_tile(at).runs_shared_with(window) {
                cache.dense[run.in_target].copy_from_slice(&tile.levels[run.in_source]);
            }
        }
    }

    /// Makes the caches of `ctx` hold the climate kept for the quarts of the chunk (cx, cz), when
    /// its tile is held: the biomes of the chunk, computed in `ctx`, read that part of their
    /// climate (continents, erosion, ridges, the offset under the depth) instead of computing it.
    pub fn preload_chunk_climate(&self, ctx: &mut Ctx, cx: i32, cz: i32) {
        self.preload_climate(ctx, QuartRect { qx0: cx * 4, qz0: cz * 4, width: 4, depth: 4 });
    }

    /// Makes the caches of `ctx` hold, over `window`, the 2D climate kept with the tiles: what
    /// the window's own volume would have left in them. The final density of the chunk then finds
    /// its corners there, as it does after a window computed by the chunk, instead of computing
    /// the climate again. A cache that a tile of the window did not keep is left alone.
    fn preload_climate(&self, ctx: &mut Ctx, window: QuartRect) {
        let held: Option<Vec<_>> =
            window.tiles().map(|at| Some((QuartRect::of_tile(at), self.tiles.get(&at)?))).collect();
        let Some(tiles) = held else { return };
        let Some((_, first)) = tiles.first() else { return };
        let volume = window.volume();
        for &(cache_id, _) in &first.climate {
            if !tiles.iter().all(|(_, tile)| tile.climate_of(cache_id).is_some()) {
                continue;
            }
            ctx.hold_in_cache(cache_id, &volume, |values| {
                for (rect, tile) in &tiles {
                    // Checked just above: every tile of the window kept this cache.
                    let Some(kept) = tile.climate_of(cache_id) else { continue };
                    for run in rect.runs_shared_with(window) {
                        values[run.in_target].copy_from_slice(&kept[run.in_source]);
                    }
                }
            });
        }
    }

    /// Test sabotage: the same tiles with every level moved one quart along x inside its row (what
    /// a key off by one quart would read).
    #[doc(hidden)]
    pub fn shifted_by_one_quart(&self) -> SurfaceLevelTiles {
        let shift = |tile: &SurfaceLevelTile| {
            let mut shifted = tile.clone();
            move_one_quart_along_rows(&mut shifted.levels);
            shifted
        };
        SurfaceLevelTiles { tiles: self.tiles.iter().map(|(&at, tile)| (at, shift(tile))).collect() }
    }

    /// Test sabotage: the same tiles with the kept climate moved one quart along x inside its row.
    #[doc(hidden)]
    pub fn with_climate_shifted_by_one_quart(&self) -> SurfaceLevelTiles {
        let shift = |tile: &SurfaceLevelTile| {
            let mut shifted = tile.clone();
            for (_, values) in &mut shifted.climate {
                move_one_quart_along_rows(values);
            }
            shifted
        };
        SurfaceLevelTiles { tiles: self.tiles.iter().map(|(&at, tile)| (at, shift(tile))).collect() }
    }

    /// Number of caches whose climate each tile keeps, smallest and largest over the tiles.
    #[doc(hidden)]
    pub fn climate_caches_kept(&self) -> (usize, usize) {
        let kept = || self.tiles.values().map(|tile| tile.climate.len());
        (kept().min().unwrap_or(0), kept().max().unwrap_or(0))
    }
}

/// Noise aquifer of a chunk (cell caches private to the chunk).
#[derive(Debug)]
pub struct NoiseAquifer {
    cfg: AquiferConfig,
    picker: FluidPicker,
    lava_id: BlockId,
    water_id: BlockId,
    min_gx: i32,
    min_gy: i32,
    min_gz: i32,
    size_x: i32,
    size_z: i32,
    /// Cell centres (x, y, z), `valid` false until drawn.
    loc: Vec<[i32; 3]>,
    loc_valid: Vec<bool>,
    status: Vec<Option<FluidStatus>>,
    skip_above: i32,
    surface: SurfaceCache,
    /// Column window (`compute_fill`): key (x, z, y cell), indices of the 12
    /// neighbouring cells in the generic walk order, dx² + dz² of their centre and y of the centre.
    win_key: (i32, i32, i32),
    win_idx: [usize; 12],
    win_dxz: [i32; 12],
    win_ly: [i32; 12],
}

/// 4 nearest out of 12 in the generic walk order (insertion with `>=`: at equal
/// distance, the cell seen last goes first). Key = (distance, 15 − rank): the 4
/// smallest keys, sorted, give the same result. Branchless comparison network: three
/// sorts of 4, then two bitonic merges keeping the 4 smallest.
#[inline]
fn nearest4_net(nd: &[i32; 12]) -> [i64; 4] {
    #[inline(always)]
    fn cs(a: &mut [i64; 4], i: usize, j: usize) {
        let (x, y) = (a[i], a[j]);
        a[i] = x.min(y);
        a[j] = x.max(y);
    }
    #[inline(always)]
    fn sort4(a: &mut [i64; 4]) {
        cs(a, 0, 1);
        cs(a, 2, 3);
        cs(a, 0, 2);
        cs(a, 1, 3);
        cs(a, 1, 2);
    }
    #[inline(always)]
    fn merge(a: &[i64; 4], b: &[i64; 4]) -> [i64; 4] {
        let mut c = [a[0].min(b[3]), a[1].min(b[2]), a[2].min(b[1]), a[3].min(b[0])];
        cs(&mut c, 0, 2);
        cs(&mut c, 1, 3);
        cs(&mut c, 0, 1);
        cs(&mut c, 2, 3);
        c
    }
    let key = |k: usize| (i64::from(nd[k]) << 4) | (15 - k as i64);
    let mut g: [[i64; 4]; 3] = std::array::from_fn(|q| std::array::from_fn(|i| key(q * 4 + i)));
    for a in &mut g {
        sort4(a);
    }
    merge(&merge(&g[0], &g[1]), &g[2])
}

/// Aquifer of a chunk: noise-based, or disabled (global fluid under the zero density).
#[derive(Debug)]
pub enum Aquifer {
    Disabled(FluidPicker),
    Noise(Box<NoiseAquifer>),
}

impl Aquifer {
    /// Creates the aquifer of volume `v` (samples the surface maximum first, like the game).
    /// `shared_surface`: tiles the preliminary surface levels are copied from instead of being
    /// computed, where they hold them (the caller checked that the levels can be shared).
    pub fn new(
        g: &Generator,
        ctx: &mut Ctx,
        cfg: Option<AquiferConfig>,
        picker: FluidPicker,
        fluids: (BlockId, BlockId),
        v: &Volume,
        shared_surface: Option<&SurfaceLevelTiles>,
    ) -> Self {
        let Some(cfg) = cfg else { return Aquifer::Disabled(picker) };
        let (lava_id, water_id) = fluids;
        let min_gx = grid_x(v.x0 - 5);
        let max_gx = grid_x(v.max_x() - 5) + 1;
        let min_gy = grid_y(v.y0 + 1) - 1;
        let max_gy = grid_y(v.max_y() + 1) + 1;
        let min_gz = grid_x(v.z0 - 5);
        let max_gz = grid_x(v.max_z() - 5) + 1;
        let (size_x, size_y, size_z) = (max_gx - min_gx + 1, max_gy - min_gy + 1, max_gz - min_gz + 1);
        let total = (size_x * size_y * size_z) as usize;
        let mut a = NoiseAquifer {
            cfg,
            picker,
            lava_id,
            water_id,
            min_gx,
            min_gy,
            min_gz,
            size_x,
            size_z,
            loc: vec![[0; 3]; total],
            loc_valid: vec![false; total],
            status: vec![None; total],
            skip_above: 0,
            surface: SurfaceCache::new(v.x0, v.z0),
            win_key: (i32::MIN, i32::MIN, i32::MIN),
            win_idx: [0; 12],
            win_dxz: [0; 12],
            win_ly: [0; 12],
        };
        let window = QuartRect::of_blocks((min_gx << 4, min_gz << 4, (max_gx << 4) + 9, (max_gz << 4) + 9));
        let max_surface = match shared_surface.and_then(|tiles| a.max_surface_level_from_tiles(tiles, ctx, window)) {
            Some(max) => max,
            None => a.max_surface_level(g, ctx, window),
        };
        let adjusted = max_surface.wrapping_add(8);
        let skip_gy = grid_y(adjusted + 12) + 1;
        a.skip_above = skip_gy * 12 + 11 - 1;
        Aquifer::Noise(Box::new(a))
    }

    /// Substance at the block for a density (§ 3.3): `None` = solid (default block or barrier),
    /// and the "fluid update requested" flag.
    #[inline]
    pub fn compute(
        &mut self,
        g: &Generator,
        ctx: &mut Ctx,
        x: i32,
        y: i32,
        z: i32,
        density: f64,
    ) -> (Option<BlockId>, bool) {
        match self {
            Aquifer::Disabled(p) => {
                if density > 0.0 {
                    (None, false)
                } else {
                    (Some(p.at(y).at(y)), false)
                }
            }
            Aquifer::Noise(a) => a.compute(g, ctx, x, y, z, density),
        }
    }

    /// Like [`Self::compute`], for a column walk (x, z fixed, y variable): the 12
    /// neighbouring cells and their dx² + dz² are kept as long as x, z and the y cell do not
    /// change; `net` picks the 4 nearest through the branchless network. Identical result.
    #[allow(clippy::too_many_arguments)]
    #[inline]
    pub fn compute_column(
        &mut self,
        g: &Generator,
        ctx: &mut Ctx,
        x: i32,
        y: i32,
        z: i32,
        density: f64,
        net: bool,
    ) -> (Option<BlockId>, bool) {
        match self {
            Aquifer::Disabled(_) => self.compute(g, ctx, x, y, z, density),
            Aquifer::Noise(a) => a.compute_window(g, ctx, x, y, z, density, net),
        }
    }

    /// Preliminary surface level kept for the block column (x, z), computed like a cell's read
    /// when the aquifer does not hold it yet; `None` without noise aquifers (equivalence tests).
    #[doc(hidden)]
    pub fn preliminary_surface_level(&mut self, g: &Generator, ctx: &mut Ctx, x: i32, z: i32) -> Option<i32> {
        match self {
            Aquifer::Disabled(_) => None,
            Aquifer::Noise(a) => Some(a.surface_level(g, ctx, x, z)),
        }
    }
}

impl NoiseAquifer {
    /// Highest preliminary surface over the quarts of `window`, read through one volume like the
    /// game does; every level is kept for the cells' later reads.
    fn max_surface_level(&mut self, g: &Generator, ctx: &mut Ctx, window: QuartRect) -> i32 {
        let v = window.volume();
        let buf = g.volume(self.cfg.surface_level, ctx, &v);
        let mut max = i32::MIN;
        for z in 0..v.sz {
            for x in 0..v.sx {
                let s = ctx.floor_f32_to_i32(buf[v.index(x, 0, z)]);
                self.surface.put(v.bx(x) >> 2, v.bz(z) >> 2, s);
                max = max.max(s);
            }
        }
        max
    }

    /// [`Self::max_surface_level`] with the levels copied from the shared tiles: no volume is
    /// computed. Every level the tiles hold around the chunk is kept, the window's and those the
    /// cells read further out. `None` when a quart of the window is missing: the window is then
    /// computed as before (the levels already copied are the same ones). The climate kept with
    /// the tiles goes into the caches of `ctx`, where the window's volume would have left it.
    fn max_surface_level_from_tiles(
        &mut self,
        tiles: &SurfaceLevelTiles,
        ctx: &mut Ctx,
        window: QuartRect,
    ) -> Option<i32> {
        tiles.copy_levels_into(&mut self.surface);
        let mut max = i32::MIN;
        for qz in window.qz0..window.qz0 + window.depth {
            for qx in window.qx0..window.qx0 + window.width {
                max = max.max(self.surface.get(qx, qz)?);
            }
        }
        tiles.preload_climate(ctx, window);
        Some(max)
    }

    fn surface_level(&mut self, g: &Generator, ctx: &mut Ctx, x: i32, z: i32) -> i32 {
        let (qx, qz) = (x >> 2, z >> 2);
        if let Some(v) = self.surface.get(qx, qz) {
            return v;
        }
        let level = g.value(self.cfg.surface_level, ctx, qx << 2, 0, qz << 2);
        let v = ctx.floor_f32_to_i32(level);
        self.surface.put(qx, qz, v);
        v
    }

    #[inline]
    fn index(&self, gx: i32, gy: i32, gz: i32) -> usize {
        (((gy - self.min_gy) * self.size_z + (gz - self.min_gz)) * self.size_x + (gx - self.min_gx)) as usize
    }

    fn compute(
        &mut self,
        g: &Generator,
        ctx: &mut Ctx,
        x: i32,
        y: i32,
        z: i32,
        density: f64,
    ) -> (Option<BlockId>, bool) {
        if let Some(r) = self.early(y, density) {
            return r;
        }
        let (ax, ay, az) = (grid_x(x - 5), grid_y(y + 1), grid_x(z - 5));
        let mut d = [i32::MAX; 4];
        let mut c = [0usize; 4];
        for ox in 0..=1 {
            for oy in -1..=1 {
                for oz in 0..=1 {
                    let (gx, gy, gz) = (ax + ox, ay + oy, az + oz);
                    let i = self.center(gx, gy, gz);
                    let [lx, ly, lz] = self.loc[i];
                    let (dx, dy, dz) = (lx - x, ly - y, lz - z);
                    let nd = dx * dx + dy * dy + dz * dz;
                    insert4(&mut d, &mut c, nd, i);
                }
            }
        }
        self.resolve(g, ctx, x, y, z, density, d, c)
    }

    /// Common early exits: positive density, above the aquifers, global lava.
    #[inline]
    fn early(&self, y: i32, density: f64) -> Option<(Option<BlockId>, bool)> {
        if density > 0.0 {
            return Some((None, false));
        }
        let global = self.picker.at(y);
        if y > self.skip_above {
            return Some((Some(global.at(y)), false));
        }
        if global.at(y) == self.lava_id && self.lava_id != AIR {
            return Some((Some(self.lava_id), false));
        }
        None
    }

    /// Index of the cell, centre drawn as needed (pure: depends only on the cell).
    #[inline]
    fn center(&mut self, gx: i32, gy: i32, gz: i32) -> usize {
        let i = self.index(gx, gy, gz);
        if !self.loc_valid[i] {
            let mut r = self.cfg.random.at(gx, gy, gz);
            let lx = (gx << 4) + r.next_int_bounded(10);
            let ly = gy * 12 + r.next_int_bounded(9);
            let lz = (gz << 4) + r.next_int_bounded(10);
            self.loc[i] = [lx, ly, lz];
            self.loc_valid[i] = true;
        }
        i
    }

    #[allow(clippy::too_many_arguments)]
    fn compute_window(
        &mut self,
        g: &Generator,
        ctx: &mut Ctx,
        x: i32,
        y: i32,
        z: i32,
        density: f64,
        net: bool,
    ) -> (Option<BlockId>, bool) {
        if let Some(r) = self.early(y, density) {
            return r;
        }
        let ay = grid_y(y + 1);
        if self.win_key != (x, z, ay) {
            let (ax, az) = (grid_x(x - 5), grid_x(z - 5));
            let mut k = 0;
            for ox in 0..=1 {
                for oy in -1..=1 {
                    for oz in 0..=1 {
                        let i = self.center(ax + ox, ay + oy, az + oz);
                        let [lx, ly, lz] = self.loc[i];
                        let (dx, dz) = (lx - x, lz - z);
                        self.win_idx[k] = i;
                        self.win_dxz[k] = dx * dx + dz * dz;
                        self.win_ly[k] = ly;
                        k += 1;
                    }
                }
            }
            self.win_key = (x, z, ay);
        }
        let nd: [i32; 12] = std::array::from_fn(|k| {
            let dy = self.win_ly[k] - y;
            self.win_dxz[k] + dy * dy
        });
        let mut d = [i32::MAX; 4];
        let mut c = [0usize; 4];
        if net {
            let best = nearest4_net(&nd);
            for j in 0..4 {
                d[j] = (best[j] >> 4) as i32;
                c[j] = self.win_idx[(15 - (best[j] & 15)) as usize];
            }
        } else {
            for (k, &v) in nd.iter().enumerate() {
                insert4(&mut d, &mut c, v, self.win_idx[k]);
            }
        }
        self.resolve(g, ctx, x, y, z, density, d, c)
    }

    /// Rest of the computation once the 4 nearest cells are known (increasing distances).
    #[allow(clippy::too_many_arguments)]
    fn resolve(
        &mut self,
        g: &Generator,
        ctx: &mut Ctx,
        x: i32,
        y: i32,
        z: i32,
        density: f64,
        d: [i32; 4],
        c: [usize; 4],
    ) -> (Option<BlockId>, bool) {
        let flowing = similarity(100, 144);
        let s1 = self.status_of(g, ctx, c[0]);
        let sim12 = similarity(d[0], d[1]);
        let fluid = s1.at(y);
        if sim12 <= 0.0 {
            let update = if sim12 >= flowing { s1 != self.status_of(g, ctx, c[1]) } else { false };
            return (Some(fluid), update);
        }
        if fluid == self.water_id
            && fluid != AIR
            && self.picker.at(y - 1).at(y - 1) == self.lava_id
            && self.lava_id != AIR
        {
            return (Some(fluid), true);
        }
        let mut barrier = f64::NAN;
        let s2 = self.status_of(g, ctx, c[1]);
        let b12 = sim12 * self.pressure(g, ctx, x, y, z, &mut barrier, s1, s2);
        if density + b12 > 0.0 {
            return (None, false);
        }
        let s3 = self.status_of(g, ctx, c[2]);
        let sim13 = similarity(d[0], d[2]);
        if sim13 > 0.0 {
            let b13 = sim12 * sim13 * self.pressure(g, ctx, x, y, z, &mut barrier, s1, s3);
            if density + b13 > 0.0 {
                return (None, false);
            }
        }
        let sim23 = similarity(d[1], d[2]);
        if sim23 > 0.0 {
            let b23 = sim12 * sim23 * self.pressure(g, ctx, x, y, z, &mut barrier, s2, s3);
            if density + b23 > 0.0 {
                return (None, false);
            }
        }
        let flow12 = s1 != s2;
        let flow23 = sim23 >= flowing && s2 != s3;
        let flow13 = sim13 >= flowing && s1 != s3;
        let update = if !flow12 && !flow23 && !flow13 {
            sim13 >= flowing && similarity(d[0], d[3]) >= flowing && s1 != self.status_of(g, ctx, c[3])
        } else {
            true
        };
        (Some(fluid), update)
    }

    #[allow(clippy::too_many_arguments)]
    fn pressure(
        &self,
        g: &Generator,
        ctx: &mut Ctx,
        x: i32,
        y: i32,
        z: i32,
        barrier: &mut f64,
        a: FluidStatus,
        b: FluidStatus,
    ) -> f64 {
        let (ta, tb) = (a.at(y), b.at(y));
        let lava_water = |p: BlockId, q: BlockId| p == self.lava_id && q == self.water_id && p != AIR && q != AIR;
        if lava_water(ta, tb) || lava_water(tb, ta) {
            return 2.0;
        }
        let diff = (a.level - b.level).abs();
        if diff == 0 {
            return 0.0;
        }
        let avg = 0.5 * f64::from(a.level + b.level);
        let above = f64::from(y) + 0.5 - avg;
        let base = f64::from(diff) / 2.0;
        let toward = base - above.abs();
        let gradient = if above > 0.0 {
            let c = toward;
            if c > 0.0 { c / 1.5 } else { c / 2.5 }
        } else {
            let c = 3.0 + toward;
            if c > 0.0 { c / 3.0 } else { c / 10.0 }
        };
        let noise = if (-2.0..=2.0).contains(&gradient) {
            if barrier.is_nan() {
                *barrier = f64::from(g.value(self.cfg.barrier, ctx, x, y, z));
            }
            *barrier
        } else {
            0.0
        };
        2.0 * (noise + gradient)
    }

    fn status_of(&mut self, g: &Generator, ctx: &mut Ctx, i: usize) -> FluidStatus {
        if let Some(s) = self.status[i] {
            return s;
        }
        let [x, y, z] = self.loc[i];
        let s = self.compute_fluid(g, ctx, x, y, z);
        self.status[i] = Some(s);
        s
    }

    fn compute_fluid(&mut self, g: &Generator, ctx: &mut Ctx, x: i32, y: i32, z: i32) -> FluidStatus {
        let global = self.picker.at(y);
        let mut lowest = i32::MAX;
        let top = y + 12;
        let bottom = y - 12;
        let mut center_under = false;
        for (ox, oz) in SURFACE_OFFSETS {
            let (sx, sz) = (x + (ox << 4), z + (oz << 4));
            let s = self.surface_level(g, ctx, sx, sz);
            let adj = s + 8;
            let start = ox == 0 && oz == 0;
            if start && bottom > adj {
                return global;
            }
            let pokes = top > adj;
            if pokes || start {
                let at_surface = self.picker.at(adj);
                if at_surface.at(adj) != AIR {
                    if start {
                        center_under = true;
                    }
                    if pokes {
                        return at_surface;
                    }
                }
            }
            lowest = lowest.min(s);
        }
        let level = self.surface_fluid_level(g, ctx, x, y, z, global, lowest, center_under);
        FluidStatus { level, kind: self.fluid_type(g, ctx, x, y, z, global, level) }
    }

    #[allow(clippy::too_many_arguments)]
    fn surface_fluid_level(
        &self,
        g: &Generator,
        ctx: &mut Ctx,
        x: i32,
        y: i32,
        z: i32,
        global: FluidStatus,
        lowest: i32,
        center_under: bool,
    ) -> i32 {
        let (partial, full) = if g.value(self.cfg.exclusion, ctx, x, y, z) > 0.0 {
            (-1.0, -1.0)
        } else {
            let below = (lowest + 8) - y;
            let factor = if center_under { clamped_map(f64::from(below), 0.0, 64.0, 1.0, 0.0) } else { 0.0 };
            let noise = f64::from(g.value(self.cfg.floodedness, ctx, x, y, z)).clamp(-1.0, 1.0);
            let full_t = map(factor, 1.0, 0.0, -0.3, 0.8);
            let part_t = map(factor, 1.0, 0.0, -0.8, 0.4);
            (noise - part_t, noise - full_t)
        };
        if full > 0.0 {
            global.level
        } else if partial > 0.0 {
            let (cx, cy, cz) = (floor_div(x, 16), floor_div(y, 40), floor_div(z, 16));
            let middle = cy * 40 + 20;
            let spread = f64::from(g.value(self.cfg.spread, ctx, cx, cy, cz) * 10.0);
            let q = (spread / 3.0).floor() as i32 * 3;
            lowest.min(middle + q)
        } else {
            WAY_BELOW_MIN_Y
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn fluid_type(
        &self,
        g: &Generator,
        ctx: &mut Ctx,
        x: i32,
        y: i32,
        z: i32,
        global: FluidStatus,
        level: i32,
    ) -> BlockId {
        if level <= -10 && level != WAY_BELOW_MIN_Y && global.kind != self.lava_id {
            let (cx, cy, cz) = (floor_div(x, 64), floor_div(y, 40), floor_div(z, 64));
            if f64::from(g.value(self.cfg.lava, ctx, cx, cy, cz)).abs() > 0.3 {
                return self.lava_id;
            }
        }
        global.kind
    }
}

/// Insertion into the 4 nearest (game order: `>=`, the new cell goes first at
/// equal distance).
#[inline(always)]
fn insert4(d: &mut [i32; 4], c: &mut [usize; 4], nd: i32, i: usize) {
    if d[0] >= nd {
        *c = [i, c[0], c[1], c[2]];
        *d = [nd, d[0], d[1], d[2]];
    } else if d[1] >= nd {
        *c = [c[0], i, c[1], c[2]];
        *d = [d[0], nd, d[1], d[2]];
    } else if d[2] >= nd {
        c[3] = c[2];
        c[2] = i;
        d[3] = d[2];
        d[2] = nd;
    } else if d[3] >= nd {
        c[3] = i;
        d[3] = nd;
    }
}

#[inline]
fn inverse_lerp(v: f64, a: f64, b: f64) -> f64 {
    (v - a) / (b - a)
}

#[inline]
fn lerp(t: f64, a: f64, b: f64) -> f64 {
    a + t * (b - a)
}

#[inline]
pub(crate) fn map(v: f64, a: f64, b: f64, c: f64, d: f64) -> f64 {
    lerp(inverse_lerp(v, a, b), c, d)
}

#[inline]
pub(crate) fn clamped_map(v: f64, a: f64, b: f64, c: f64, d: f64) -> f64 {
    let t = inverse_lerp(v, a, b);
    if t < 0.0 {
        c
    } else if t > 1.0 {
        d
    } else {
        lerp(t, c, d)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A level, then a climate value, that name their quart.
    fn level_of(qx: i32, qz: i32) -> i32 {
        qx * 1000 + qz
    }

    fn climate_of(qx: i32, qz: i32) -> f32 {
        qx as f32 + qz as f32 / 1024.0
    }

    /// Tiles at `positions` whose levels and one kept cache (`cache_id`) name their quart.
    fn tiles_naming_their_quarts(positions: &[(i32, i32)], cache_id: u32) -> SurfaceLevelTiles {
        let mut tiles = SurfaceLevelTiles::default();
        tiles.extend(positions.iter().map(|&at| {
            let rect = QuartRect::of_tile(at);
            let quarts =
                || (0..rect.depth).flat_map(move |z| (0..rect.width).map(move |x| (rect.qx0 + x, rect.qz0 + z)));
            let levels = quarts().map(|(qx, qz)| level_of(qx, qz)).collect();
            let climate = quarts().map(|(qx, qz)| climate_of(qx, qz)).collect();
            (at, SurfaceLevelTile { levels, climate: vec![(cache_id, climate)] })
        }));
        tiles
    }

    /// The reach of the surface tiles is what the cells read: cells centred in the chunks −1 to
    /// +1, each reading at every sampling offset.
    #[test]
    fn surface_reach_covers_every_sampling_offset() {
        let west_to_east = SURFACE_OFFSETS.iter().map(|o| o.0);
        let north_to_south = SURFACE_OFFSETS.iter().map(|o| o.1);
        assert_eq!(-SURFACE_REACH_WEST, west_to_east.clone().min().unwrap() - 1);
        assert_eq!(SURFACE_REACH_EAST, west_to_east.max().unwrap() + 1);
        assert_eq!(-SURFACE_REACH_NORTH_SOUTH, north_to_south.clone().min().unwrap() - 1);
        assert_eq!(SURFACE_REACH_NORTH_SOUTH, north_to_south.max().unwrap() + 1);
    }

    /// Each quart of an aquifer's dense window gets the level of its own tile, on both sides of
    /// the origin, and stays empty where no tile is held.
    #[test]
    fn levels_are_copied_to_their_own_quarts() {
        let held = [(-1, -1), (0, -1), (-1, 0), (1, 0), (2, 1), (-3, 2)];
        let tiles = tiles_naming_their_quarts(&held, 7);
        let mut checked = 0;
        for (min_x, min_z) in [(0, 0), (-16, -16), (48, -32), (-112, 80), (16 * 7, 16 * 5)] {
            let mut cache = SurfaceCache::new(min_x, min_z);
            tiles.copy_levels_into(&mut cache);
            for qz in cache.qz0..cache.qz0 + SC_W {
                for qx in cache.qx0..cache.qx0 + SC_W {
                    let in_held_tile = held.contains(&(qx >> TILE_QUARTS_SHIFT, qz >> TILE_QUARTS_SHIFT));
                    let expected = in_held_tile.then_some(level_of(qx, qz));
                    assert_eq!(cache.get(qx, qz), expected, "quart {qx},{qz} copied");
                    assert_eq!(tiles.level(qx, qz), expected, "quart {qx},{qz} looked up");
                    checked += usize::from(in_held_tile);
                }
            }
        }
        assert!(checked > 3000, "only {checked} levels copied");
    }

    /// The climate kept with the tiles lands on its own quarts of the window, whichever tiles
    /// the window spans; a window with a tile missing is left alone.
    #[test]
    fn climate_is_preloaded_on_its_own_quarts() {
        let tiles = tiles_naming_their_quarts(&[(-1, -1), (0, -1), (-1, 0), (0, 0)], 7);
        for (qx0, qz0) in [(-4, -4), (2, 3), (-16, -11), (-9, 5)] {
            let window = QuartRect { qx0, qz0, width: 11, depth: 11 };
            let mut ctx = Ctx::cached();
            tiles.preload_climate(&mut ctx, window);
            let held = ctx.cached_volume(7, &window.volume()).expect("climate held over the window");
            for qz in qz0..qz0 + 11 {
                for qx in qx0..qx0 + 11 {
                    assert_eq!(held[window.index(qx, qz)], climate_of(qx, qz), "quart {qx},{qz}");
                }
            }
        }
        let half_outside = QuartRect { qx0: 10, qz0: 0, width: 11, depth: 11 };
        let mut ctx = Ctx::cached();
        tiles.preload_climate(&mut ctx, half_outside);
        assert!(ctx.cached_volume(7, &half_outside.volume()).is_none());
    }

    /// Network against insertion: distances drawn in small ranges (many ties)
    /// and large ones; indices = rank. Fails if the tie-breaking order changes.
    #[test]
    fn net_matches_insertion() {
        let mut s = 0x1234_5678_9ABC_DEF0u64;
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        let mut ties = 0;
        for n in 0..200_000 {
            let span = [3u64, 6, 40, 3000][n % 4];
            let nd: [i32; 12] = std::array::from_fn(|_| (next() % span) as i32);
            let (mut d, mut c) = ([i32::MAX; 4], [0usize; 4]);
            for (k, &v) in nd.iter().enumerate() {
                insert4(&mut d, &mut c, v, k);
            }
            let best = nearest4_net(&nd);
            let got_d: [i32; 4] = std::array::from_fn(|j| (best[j] >> 4) as i32);
            let got_c: [usize; 4] = std::array::from_fn(|j| (15 - (best[j] & 15)) as usize);
            assert_eq!((got_d, got_c), (d, c), "{nd:?}");
            ties += usize::from(d[0] == d[1] || d[1] == d[2] || d[2] == d[3]);
        }
        assert!(ties > 50_000, "too few ties ({ties})");
    }
}
