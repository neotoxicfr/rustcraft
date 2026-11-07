//! Full light of the chunks of a batch (`RUSTCRAFT_OPT_FULL_UNION_TILES`, mission C2-M15,
//! `docs/perf.md`). The chunks are grouped by aligned square tiles (side: see [`tile_side`]);
//! each tile is resolved once over the bounding box of its chunks plus a margin of one chunk,
//! instead of once per chunk over its 3 × 3 columns: a full 4 × 4 tile reads 36 columns for 16
//! chunks (2.25 per chunk instead of 9), a row of 4 reads 18 (4.5 per chunk), a row of 2 reads 12
//! (6 per chunk). Fewer, larger tasks: less work in all, but less spread over the threads when the
//! batch is small. Same sheets, byte for byte: a level drops by at least 1 per step from at
//! most 15, so the light of a cell comes from at most 14 steps away and every path that reaches a
//! chunk stays inside its 3 × 3 columns, which the tile's zone contains, with the same states and
//! the same sources (emitters, and sky above each stack's own source height). Tiles are resolved
//! in parallel; a sparse tile, whose zone would cost more than its chunks lit alone, or a chunk
//! whose own column is absent, is lit chunk by chunk. Needs the property grid (`grid`).

use std::sync::atomic::Ordering::Relaxed;

use hashbrown::HashMap;
use rayon::prelude::*;

use super::grid::{self, ZoneColumns};
use super::{ColumnScan, Columns, FullOpts, LightSheets, Scans, sabotaged, with_bufs};
use crate::{LightTables, stats};

/// Sides of a tile tried by [`tile_side`], largest first, in chunks.
const TILE_SIDES: [i32; 3] = [4, 3, 2];

/// Deliberate error of the tests: the tile's zone without its margin of one chunk.
pub(crate) const SABOTAGE_TILE_WITHOUT_MARGIN: u8 = 20;
/// Deliberate error of the tests: a tile above the kept maximum takes its thread's kept buffers.
pub(crate) const SABOTAGE_TILE_TAKES_KEPT_BUFFERS: u8 = 21;

/// Light sheets of each chunk of `chunks`, in that order; `column` and `scan` give the states and
/// the scan (if done) of a column. Counted in `stats` like [`crate::light_chunk`].
pub(crate) fn light_chunks<'a, C, S>(
    t: &LightTables,
    chunks: &[(i32, i32)],
    column: C,
    scan: S,
    height: i32,
    has_sky: bool,
    opts: FullOpts,
) -> Vec<LightSheets>
where
    C: Fn(i32, i32) -> Option<&'a [u32]> + Sync,
    S: Fn(i32, i32) -> Option<&'a ColumnScan> + Sync,
{
    let alone = |&(x, z): &(i32, i32)| {
        let cols: Columns = std::array::from_fn(|i| column(x + i as i32 % 3 - 1, z + i as i32 / 3 - 1));
        let scans: Scans = std::array::from_fn(|i| scan(x + i as i32 % 3 - 1, z + i as i32 / 3 - 1));
        counted(1, || crate::light_chunk_dispatch(t, cols, scans, height, has_sky, opts))
    };
    if !opts.lights_by_tiles(t) {
        return chunks.par_iter().map(alone).collect();
    }
    let tiles = tiles_of(chunks, tile_side(chunks, opts.union_tile_side));
    let lit: Vec<Vec<(usize, LightSheets)>> = tiles
        .par_iter()
        .map(|members| {
            let tile = Tile { t, chunks, members, height, has_sky, opts };
            tile.light(&column, &scan).unwrap_or_else(|| members.par_iter().map(|&m| (m, alone(&chunks[m]))).collect())
        })
        .collect();
    let mut out: Vec<Option<LightSheets>> = (0..chunks.len()).map(|_| None).collect();
    for (m, sheets) in lit.into_iter().flatten() {
        out[m] = Some(sheets);
    }
    out.into_iter().map(|s| s.expect("every chunk lit")).collect()
}

/// Side of the tiles of a batch: `wanted` if given (above 0), otherwise the largest of
/// [`TILE_SIDES`] that still gives every thread of the pool a tile, else the smallest. A batch of
/// a row of 24 chunks on 32 threads takes tiles of 2 (12 tiles, each twice as long as a chunk lit
/// alone, a third less work); a large batch, or one thread, tiles of 4.
fn tile_side(chunks: &[(i32, i32)], wanted: i32) -> i32 {
    if wanted > 0 {
        return wanted;
    }
    let threads = rayon::current_num_threads();
    let tiles = |side: i32| {
        let keys: hashbrown::HashSet<(i32, i32)> =
            chunks.iter().map(|&(x, z)| (x.div_euclid(side), z.div_euclid(side))).collect();
        keys.len()
    };
    let smallest = TILE_SIDES[TILE_SIDES.len() - 1];
    TILE_SIDES.into_iter().find(|&side| tiles(side) >= threads).unwrap_or(smallest)
}

/// Indices into `chunks` grouped by tile of side `side`, tiles in coordinate order.
fn tiles_of(chunks: &[(i32, i32)], side: i32) -> Vec<Vec<usize>> {
    let mut by_tile: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (m, &(x, z)) in chunks.iter().enumerate() {
        by_tile.entry((x.div_euclid(side), z.div_euclid(side))).or_default().push(m);
    }
    let mut tiles: Vec<((i32, i32), Vec<usize>)> = by_tile.into_iter().collect();
    tiles.sort_unstable_by_key(|(k, _)| *k);
    tiles.into_iter().map(|(_, members)| members).collect()
}

/// `f`'s lighting of `n` chunks counted in `stats`.
fn counted<R>(n: u64, f: impl FnOnce() -> R) -> R {
    let t0 = std::time::Instant::now();
    let out = f();
    stats::CHUNK_NS.fetch_add(t0.elapsed().as_nanos() as u64, Relaxed);
    stats::CHUNKS.fetch_add(n, Relaxed);
    out
}

/// The chunks of one tile (indices `members` into `chunks`).
struct Tile<'a> {
    t: &'a LightTables,
    chunks: &'a [(i32, i32)],
    members: &'a [usize],
    height: i32,
    has_sky: bool,
    opts: FullOpts,
}

impl Tile<'_> {
    /// Sheets of the members resolved together, or `None` when they are better lit alone (sparse
    /// tile: the zone holds at least 9 columns per chunk) or cannot be (a member's own column absent).
    fn light<'c, C, S>(&self, column: &C, scan: &S) -> Option<Vec<(usize, LightSheets)>>
    where
        C: Fn(i32, i32) -> Option<&'c [u32]>,
        S: Fn(i32, i32) -> Option<&'c ColumnScan>,
    {
        let at = |m: usize| self.chunks[m];
        let x0 = self.members.iter().map(|&m| at(m).0).min()?;
        let x1 = self.members.iter().map(|&m| at(m).0).max()?;
        let z0 = self.members.iter().map(|&m| at(m).1).min()?;
        let z1 = self.members.iter().map(|&m| at(m).1).max()?;
        let margin = if sabotaged(SABOTAGE_TILE_WITHOUT_MARGIN) { 0 } else { 1 };
        let (w, d) = (x1 - x0 + 1 + 2 * margin, z1 - z0 + 1 + 2 * margin);
        if (w * d) as usize >= 9 * self.members.len() || !grid::zone_fits(w, d, self.height) {
            return None;
        }
        let corner = (x0 - margin, z0 - margin);
        let positions: Vec<(i32, i32)> = (0..w * d).map(|k| (corner.0 + k % w, corner.1 + k / w)).collect();
        let columns: Vec<Option<&[u32]>> = positions.iter().map(|&(x, z)| column(x, z)).collect();
        let targets: Vec<[i32; 2]> = self.members.iter().map(|&m| [at(m).0 - corner.0, at(m).1 - corner.1]).collect();
        if targets.iter().any(|&[x, z]| columns[(z * w + x) as usize].is_none()) {
            return None;
        }
        let scans: Vec<Option<&ColumnScan>> = positions.iter().map(|&(x, z)| scan(x, z)).collect();
        let zc = ZoneColumns { chunks_x: w, chunks_z: d, columns: &columns, scans: &scans };
        let (t, height, has_sky, opts) = (self.t, self.height, self.has_sky, self.opts);
        let expected_bytes = if sabotaged(SABOTAGE_TILE_TAKES_KEPT_BUFFERS) { 0 } else { grid::zone_buffer_bytes(w, d, height) };
        let lit = counted(self.members.len() as u64, || {
            with_bufs(opts, expected_bytes, |b| grid::light_zone(t, &zc, &targets, height, has_sky, opts, b))
        });
        Some(self.members.iter().copied().zip(lit).collect())
    }
}
