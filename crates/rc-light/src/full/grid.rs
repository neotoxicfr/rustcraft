//! Full light resolved on a grid of cell properties (`RUSTCRAFT_OPT_FULL_PROPERTY_GRID`, mission
//! C2-M15, `docs/perf.md`): same sheets, byte for byte, as the other paths of `full`.
//!
//! The zone (the 3 × 3 columns around one chunk, or a whole tile of a batch, see `batch`) is copied
//! once into 16 bits per cell: index of the face shape (12 bits) and opacity (4 bits). A ring of
//! closed cells surrounds it and the absent columns are closed too, so the propagation steps
//! through two flat arrays (levels and properties) by fixed offsets, without a column lookup, a
//! bound test or a state table; the block and sky passes share the grid. A neighbour's level is
//! tested before its property is read, and level 1 is never queued (it lights nothing).
//! Sub-switches, measured one by one:
//! - `RUSTCRAFT_OPT_FULL_OCCLUSION_TABLE`: occluded faces read from the table of shape pairs
//!   (`=0`: compared on the face masks);
//! - `RUSTCRAFT_OPT_FULL_QUEUE_DIRECTIONS`: a queue entry keeps the directions still to try (no
//!   step back towards the cell that lit it);
//! - `RUSTCRAFT_OPT_FULL_NO_EMITTER_EXIT`: a zone without an emitter skips the block propagation,
//!   its packing and the reset of the levels before the sky;
//! - `RUSTCRAFT_OPT_FULL_SKY_SEED_FILTER`: a sky source is queued only if one of its neighbours
//!   can still rise when it spreads (see [`sky_seed_useful`]);
//! - `RUSTCRAFT_OPT_FULL_EMITTERS_IN_FILL`: the emitters are seeded by the pass that fills the
//!   properties (one read of the states instead of two);
//! - `RUSTCRAFT_OPT_FULL_SKIP_DEPTHS`: the rows that no light reaches, deep under every source,
//!   are neither filled nor resolved (see [`unlit_depth`]).

use rc_world::LightLayer;

use super::{Bufs, ColumnScan, FullOpts, LightSheets, pack_sheets_below_top, sabotaged, stack};
use crate::{LightTables, OPPOSITE};

/// Property of a closed cell: opacity 15 and no face, so no light ever enters it (a level minus
/// 15 is never above 0). The ring around the zone and the absent columns hold it.
pub(crate) const CLOSED: u16 = 15;

/// Queue entry: zone index in the low bits, directions still to try above them (see
/// [`zone_fits`]).
const DIRECTION_SHIFT: u32 = 26;
const INDEX_MASK: u32 = (1 << DIRECTION_SHIFT) - 1;
const ALL_DIRECTIONS: u32 = 63 << DIRECTION_SHIFT;

/// A zone of `chunks_x × chunks_z` columns `height` high has its cell indices in a queue entry
/// (3 × 3 columns always do, up to the highest world of 4,064 rows).
pub(crate) fn zone_fits(chunks_x: i32, chunks_z: i32, height: i32) -> bool {
    Zone { chunks_x, chunks_z, bottom: 0, top: height }.cells() <= INDEX_MASK as usize + 1
}

/// Deliberate errors of the tests (each must break the equivalence proof).
pub(crate) const SABOTAGE_LEVEL_TEST_ASSUMES_OPACITY_2: u8 = 1;
pub(crate) const SABOTAGE_ABSENT_COLUMN_OPEN: u8 = 2;
pub(crate) const SABOTAGE_SKY_SEED_FILTER_FORGETS_BELOW: u8 = 3;
pub(crate) const SABOTAGE_QUEUE_DIRECTIONS_DROP_FORWARD: u8 = 4;
pub(crate) const SABOTAGE_NO_EMITTER_EXIT_MISSES_LEVEL_1: u8 = 5;
pub(crate) const SABOTAGE_EMITTERS_IN_FILL_MISS_A_ROW: u8 = 6;
pub(crate) const SABOTAGE_SKIP_DEPTHS_WITHOUT_MARGIN: u8 = 7;

/// Zone of `chunks_x × chunks_z` columns resolved over the rows `[bottom, top)`, inside a ring of
/// closed cells: index `((y − bottom + 1) · depth + z + 1) · width + x + 1`, `x` and `z` in cells
/// from the zone's corner.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Zone {
    pub chunks_x: i32,
    pub chunks_z: i32,
    /// Lowest resolved row, a multiple of 16 (see [`unlit_depth`]).
    pub bottom: i32,
    pub top: i32,
}

impl Zone {
    /// Cells along x, ring included.
    fn width(self) -> usize {
        (16 * self.chunks_x + 2) as usize
    }

    /// Cells along z, ring included.
    fn depth(self) -> usize {
        (16 * self.chunks_z + 2) as usize
    }

    fn plane(self) -> usize {
        self.width() * self.depth()
    }

    fn cells(self) -> usize {
        self.plane() * (self.top - self.bottom + 2) as usize
    }

    #[inline]
    fn index(self, x: i32, y: i32, z: i32) -> usize {
        ((y - self.bottom + 1) as usize * self.depth() + (z + 1) as usize) * self.width() + (x + 1) as usize
    }

    /// Index offsets of the six neighbours, in the order of [`crate::STEP`] (down, up, north,
    /// south, west, east).
    fn steps(self) -> [isize; 6] {
        let (w, p) = (self.width() as isize, self.plane() as isize);
        [-p, p, -w, w, -1, 1]
    }
}

/// Columns of a zone (`None`: absent, closed) and their scans (`None`: done on the fly), row by
/// row: index `z · chunks_x + x`, in chunks.
pub(crate) struct ZoneColumns<'a> {
    pub chunks_x: i32,
    pub chunks_z: i32,
    pub columns: &'a [Option<&'a [u32]>],
    pub scans: &'a [Option<&'a ColumnScan>],
}

impl ZoneColumns<'_> {
    /// Corner of column `k`, in cells.
    fn origin(&self, k: usize) -> (i32, i32) {
        let k = k as i32;
        (k % self.chunks_x * 16, k / self.chunks_x * 16)
    }

    fn column_at(&self, [x, z]: [i32; 2]) -> Option<&[u32]> {
        self.columns[(z * self.chunks_x + x) as usize]
    }
}

/// Light sheets (sky, block) of each chunk of `targets` (chunk coordinates in the zone, each with
/// its 8 neighbours inside it), in that order: the sheets [`super::light_chunk_inner`] gives on the
/// target's 3 × 3 columns (see the header of `batch` for a zone larger than 3 × 3).
pub(crate) fn light_zone(
    t: &LightTables,
    zc: &ZoneColumns,
    targets: &[[i32; 2]],
    height: i32,
    has_sky: bool,
    opts: FullOpts,
    b: &mut Bufs,
) -> Vec<LightSheets> {
    let sections = (height / 16) as usize;
    let ymax = zone_stacks(t, zc, height, has_sky || opts.skip, has_sky, &mut b.src);
    // Absent centre: no skip (its sky sheets are zero, not full).
    let centres_present = targets.iter().all(|&c| zc.column_at(c).is_some());
    let top = if opts.skip && centres_present { height.min((ymax + 30) & !15) } else { height };
    let bottom = if opts.skip_depths && centres_present { unlit_depth(zc, &b.src, has_sky) } else { 0 };
    let zone = Zone { chunks_x: zc.chunks_x, chunks_z: zc.chunks_z, bottom, top };
    debug_assert!(zone.cells() <= INDEX_MASK as usize + 1, "zone of {} cells", zone.cells());
    crate::stats::FULL_CELLS.fetch_add(zone.cells() as u64, std::sync::atomic::Ordering::Relaxed);
    let Bufs { level, buckets, src, props } = b;
    level.clear();
    level.resize(zone.cells(), 0);
    for k in buckets.iter_mut() {
        k.clear();
    }
    let mut seeds = BlockSeeds { level, buckets, found: false };
    if opts.emitters_in_fill {
        fill_properties(t, zc, zone, props, Some((&mut seeds, opts.noemit)));
    } else {
        fill_properties(t, zc, zone, props, None);
        seed_block(t, zc, zone, opts.noemit, &mut seeds);
    }
    let emitters = seeds.found;
    let props = &props[..zone.cells()];
    let skip_block = opts.no_emitter_exit && !emitters;
    let blocks: Vec<Vec<LightLayer>> = if skip_block {
        vec![vec![None; sections + 2]; targets.len()]
    } else {
        propagate(t, props, level, buckets, zone, opts);
        targets.iter().map(|&c| pack_target(level, zone, c, sections, 0, false, opts.intern_uniform)).collect()
    };
    if !has_sky {
        return blocks.into_iter().map(|block| (vec![None; sections + 2], block)).collect();
    }
    // Without an emitter the levels are still all zero.
    if !skip_block {
        level.fill(0);
    }
    seed_sky(t, zone, src, props, level, buckets, opts);
    propagate(t, props, level, buckets, zone, opts);
    targets
        .iter()
        .zip(blocks)
        .map(|(&c, block)| (pack_target(level, zone, c, sections, 0xff, true, opts.intern_uniform), block))
        .collect()
}

/// Rows under which no light reaches (`RUSTCRAFT_OPT_FULL_SKIP_DEPTHS`), a multiple of 16: a
/// cell is only raised by a neighbour at 2 or more, at most 13 steps from a source, so every cell
/// more than 14 rows under the lowest source (sky source height, or lowest section a scan marks
/// with an emitter) stays at 0 and never lights anything; those rows are left out (closed) and
/// packed as zero sheets. 0 when a present column has no scan (its emitters are unknown) or the
/// zone has no source at all.
fn unlit_depth(zc: &ZoneColumns, src: &[i32], has_sky: bool) -> i32 {
    let mut lowest = i32::MAX;
    for (col, scan) in zc.columns.iter().zip(zc.scans) {
        if col.is_none() {
            continue;
        }
        let Some(scan) = scan else { return 0 };
        if let Some(section) = scan.lowest_emitter_section() {
            lowest = lowest.min(16 * section);
        }
    }
    if has_sky && let Some(s) = src.iter().copied().filter(|&s| s != i32::MAX).min() {
        lowest = lowest.min(s);
    }
    if lowest == i32::MAX {
        return 0;
    }
    let margin = if sabotaged(SABOTAGE_SKIP_DEPTHS_WITHOUT_MARGIN) { 0 } else { 15 };
    (lowest - margin).max(0) & !15
}

/// Source height of every stack of the zone into `src` (absent column: `i32::MAX`; no source: 0)
/// and the highest non-neutral cell + 1 over the zone; nothing read unless `needed` (sky or void
/// skip), source heights only if `want_src`.
fn zone_stacks(t: &LightTables, zc: &ZoneColumns, height: i32, needed: bool, want_src: bool, src: &mut Vec<i32>) -> i32 {
    let w = 16 * zc.chunks_x;
    src.clear();
    src.resize((w * 16 * zc.chunks_z) as usize, i32::MAX);
    if !needed {
        return 0;
    }
    let mut ymax = 0;
    for (k, col) in zc.columns.iter().enumerate() {
        let Some(col) = col else { continue };
        let (ox, oz) = zc.origin(k);
        for z in 0..16 {
            for x in 0..16 {
                let (h, s) = match zc.scans[k] {
                    Some(sc) => (sc.top, sc.src[(z * 16 + x) as usize]),
                    None => stack(t, col, height, x, z, want_src),
                };
                ymax = ymax.max(h);
                src[((oz + z) * w + ox + x) as usize] = s.max(0);
            }
        }
    }
    ymax
}

/// Block light sources: emitters at their level, queued (level 1 lights nothing: not queued).
struct BlockSeeds<'a> {
    level: &'a mut [u8],
    buckets: &'a mut [Vec<u32>; 16],
    /// The zone holds an emitter.
    found: bool,
}

impl BlockSeeds<'_> {
    #[inline]
    fn seed(&mut self, i: usize, emission: u8) {
        let least_counted = if sabotaged(SABOTAGE_NO_EMITTER_EXIT_MISSES_LEVEL_1) { 2 } else { 1 };
        self.found |= emission >= least_counted;
        self.level[i] = emission;
        if emission > 1 {
            self.buckets[emission as usize].push(i as u32 | ALL_DIRECTIONS);
        }
    }
}

/// The emitters of section `section` of column `k` are read: always, unless `skip_quiet_sections`
/// and its scan marks it without any.
fn reads_emitters(zc: &ZoneColumns, k: usize, section: i32, skip_quiet_sections: bool) -> bool {
    !skip_quiet_sections || zc.scans[k].is_none_or(|sc| sc.section_has_emitter(section))
}

/// Properties of every cell of the zone, written once: rows `[bottom, top)` of the present columns
/// from their states, the ring and the absent columns closed. With `seeds`
/// (`RUSTCRAFT_OPT_FULL_EMITTERS_IN_FILL`), the emitters are seeded in the same pass over the
/// states (sections chosen like [`seed_block`]), instead of reading them all a second time.
fn fill_properties(
    t: &LightTables,
    zc: &ZoneColumns,
    zone: Zone,
    props: &mut Vec<u16>,
    seeds: Option<(&mut BlockSeeds, bool)>,
) {
    let n = zone.cells();
    // Stale values are fine: every cell below is written.
    if props.len() < n {
        props.resize(n, CLOSED);
    }
    let (mut seeds, skip_quiet_sections) = match seeds {
        Some((s, skip)) => (Some(s), skip),
        None => (None, false),
    };
    // Columns whose emitters are read in the current section (decided once per section, not per row).
    let mut seeded_columns = vec![false; zc.columns.len()];
    let (w, plane) = (zone.width(), zone.plane());
    let absent = if sabotaged(SABOTAGE_ABSENT_COLUMN_OPEN) { t.cell_property(0) } else { CLOSED };
    let mut layers = props[..n].chunks_exact_mut(plane);
    layers.next().expect("ring below").fill(CLOSED);
    for (i, layer) in layers.by_ref().take((zone.top - zone.bottom) as usize).enumerate() {
        let y = zone.bottom as usize + i;
        if seeds.is_some() && y & 15 == 0 {
            for (k, on) in seeded_columns.iter_mut().enumerate() {
                *on = reads_emitters(zc, k, (y >> 4) as i32, skip_quiet_sections);
            }
        }
        let mut rows = layer.chunks_exact_mut(w);
        rows.next().expect("ring north").fill(CLOSED);
        for (z, row) in rows.by_ref().take(16 * zc.chunks_z as usize).enumerate() {
            let (cz, lz) = (z >> 4, z & 15);
            row[0] = CLOSED;
            row[w - 1] = CLOSED;
            for (cx, cells) in row[1..w - 1].as_chunks_mut::<16>().0.iter_mut().enumerate() {
                let k = cz * zc.chunks_x as usize + cx;
                let Some(col) = zc.columns[k] else {
                    cells.fill(absent);
                    continue;
                };
                let states = &col[(y << 8) | (lz << 4)..][..16];
                for (p, &s) in cells.iter_mut().zip(states) {
                    *p = t.cell_property(s);
                }
                if let Some(seeds) = seeds.as_deref_mut()
                    && seeded_columns[k]
                    && !(lz == 0 && sabotaged(SABOTAGE_EMITTERS_IN_FILL_MISS_A_ROW))
                {
                    seed_row(t, states, zone.index(16 * cx as i32, y as i32, z as i32), seeds);
                }
            }
        }
        rows.next().expect("ring south").fill(CLOSED);
    }
    layers.next().expect("ring above").fill(CLOSED);
}

/// Emitters of a row of `states`, its first cell at zone index `first`.
#[inline]
fn seed_row(t: &LightTables, states: &[u32], first: usize, seeds: &mut BlockSeeds) {
    for (x, &s) in states.iter().enumerate() {
        let e = t.emission_of(s);
        if e != 0 {
            seeds.seed(first + x, e);
        }
    }
}

/// Emitters of the zone's rows, read from the states in a pass of their own.
/// `skip_quiet_sections`: the sections a scan marks without an emitter are not read.
fn seed_block(t: &LightTables, zc: &ZoneColumns, zone: Zone, skip_quiet_sections: bool, seeds: &mut BlockSeeds) {
    for (k, col) in zc.columns.iter().enumerate() {
        let Some(col) = col else { continue };
        let (ox, oz) = zc.origin(k);
        for section in zone.bottom / 16..zone.top / 16 {
            if !reads_emitters(zc, k, section, skip_quiet_sections) {
                continue;
            }
            for y in section * 16..section * 16 + 16 {
                for z in 0..16 {
                    let states = &col[((y << 8) | (z << 4)) as usize..][..16];
                    seed_row(t, states, zone.index(ox, y, oz + z), seeds);
                }
            }
        }
    }
}

/// Sky sources: every cell at or above its stack's source height at 15, then the queued ones,
/// between the source height and the highest source height beside it (above, every neighbour is a
/// source too); with the filter, only those [`sky_seed_useful`] keeps.
fn seed_sky(
    t: &LightTables,
    zone: Zone,
    src: &[i32],
    props: &[u16],
    level: &mut [u8],
    buckets: &mut [Vec<u32>; 16],
    opts: FullOpts,
) {
    let (w, d) = (16 * zone.chunks_x, 16 * zone.chunks_z);
    light_sky_sources(zone, src, level);
    for z in 0..d {
        for x in 0..w {
            let from = src[(z * w + x) as usize];
            if from == i32::MAX {
                continue;
            }
            let mut highest = from;
            for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                let (nx, nz) = (x + dx, z + dz);
                if (0..w).contains(&nx) && (0..d).contains(&nz) {
                    let s = src[(nz * w + nx) as usize];
                    if s != i32::MAX {
                        highest = highest.max(s);
                    }
                }
            }
            for y in from..=highest.min(zone.top - 1) {
                if opts.sky_seed_filter && !sky_seed_useful(t, zone, src, props, [x, y, z], from, opts) {
                    continue;
                }
                buckets[15].push(zone.index(x, y, z) as u32 | ALL_DIRECTIONS);
            }
        }
    }
}

/// Every cell at or above its stack's source height at 15, plane by plane: contiguous stores, where
/// a stack at a time touches one cache line per cell.
fn light_sky_sources(zone: Zone, src: &[i32], level: &mut [u8]) {
    let (w, d) = (16 * zone.chunks_x as usize, 16 * zone.chunks_z);
    let Some(lowest) = src.iter().copied().filter(|&s| s != i32::MAX).min() else { return };
    for y in lowest..zone.top {
        for z in 0..d {
            let row = &mut level[zone.index(0, y, z)..][..w];
            for (l, &s) in row.iter_mut().zip(&src[z as usize * w..][..w]) {
                if y >= s {
                    *l = 15;
                }
            }
        }
    }
}

/// Whether the sky source `[x, y, z]` of a stack whose source height is `from` can raise a
/// neighbour when it spreads. Every other source is already at 15 and every other cell at 0, so
/// only an open neighbour (opacity under 15, faces not closing the passage) that is not a source
/// can rise: the cell under the source height, or a side cell below its own stack's source height.
/// The cell above is always a source or the ring.
fn sky_seed_useful(t: &LightTables, zone: Zone, src: &[i32], props: &[u16], [x, y, z]: [i32; 3], from: i32, opts: FullOpts) -> bool {
    let i = zone.index(x, y, z);
    let face_i = u32::from(props[i] >> 4);
    let steps = zone.steps();
    let enters = |dir: usize| {
        let p = props[i.wrapping_add_signed(steps[dir])];
        let face_j = u32::from(p >> 4);
        p & 15 < 15 && ((face_i | face_j) == 0 || !t.occluded_faces(face_i, face_j, dir, opts.occlusion_table))
    };
    if y == from && !sabotaged(SABOTAGE_SKY_SEED_FILTER_FORGETS_BELOW) && enters(0) {
        return true;
    }
    let (w, d) = (16 * zone.chunks_x, 16 * zone.chunks_z);
    [(2, 0, -1), (3, 0, 1), (4, -1, 0), (5, 1, 0)].into_iter().any(|(dir, dx, dz)| {
        let (nx, nz) = (x + dx, z + dz);
        (0..w).contains(&nx) && (0..d).contains(&nz) && src[(nz * w + nx) as usize] > y && enters(dir)
    })
}

/// Spreads the queues by decreasing level over the grid: same fixed point as `super::propagate`
/// (relaxation by maximum, whatever the order).
fn propagate(t: &LightTables, props: &[u16], level: &mut [u8], buckets: &mut [Vec<u32>; 16], zone: Zone, opts: FullOpts) {
    let steps = zone.steps();
    for l in (2..16usize).rev() {
        while let Some(entry) = buckets[l].pop() {
            let i = (entry & INDEX_MASK) as usize;
            if usize::from(level[i]) != l {
                continue;
            }
            let directions = if opts.queue_directions { entry >> DIRECTION_SHIFT } else { 63 };
            let face_i = u32::from(props[i] >> 4);
            for (dir, &step) in steps.iter().enumerate() {
                if directions >> dir & 1 == 0 {
                    continue;
                }
                // Never out of the arrays: queued cells are inside the ring.
                let j = i.wrapping_add_signed(step);
                let Some(new_level) = rise(t, props, j, level[j], face_i, dir, l, opts) else { continue };
                level[j] = new_level;
                if new_level > 1 {
                    let back = if sabotaged(SABOTAGE_QUEUE_DIRECTIONS_DROP_FORWARD) { dir } else { OPPOSITE[dir] };
                    let rest = 63 & !(1 << back);
                    buckets[new_level as usize].push(j as u32 | rest << DIRECTION_SHIFT);
                }
            }
        }
    }
}

/// New level of the neighbour `j` (level `level_j`) reached from a cell of level `l` and face
/// `face_i` through its face `dir`, if it rises. The level goes first: every opacity is at least 1,
/// so a neighbour already at `l − 1` or more cannot rise, which settles most neighbours of a dense
/// front on one byte, without reading its property.
#[inline(always)]
#[allow(clippy::too_many_arguments)]
fn rise(t: &LightTables, props: &[u16], j: usize, level_j: u8, face_i: u32, dir: usize, l: usize, opts: FullOpts) -> Option<u8> {
    let least_opacity = if sabotaged(SABOTAGE_LEVEL_TEST_ASSUMES_OPACITY_2) { 2 } else { 1 };
    if usize::from(level_j) + least_opacity >= l {
        return None;
    }
    let p = props[j];
    let new_level = l as i32 - i32::from(p & 15);
    if new_level <= i32::from(level_j) {
        return None;
    }
    let face_j = u32::from(p >> 4);
    if (face_i | face_j) != 0 && t.occluded_faces(face_i, face_j, dir, opts.occlusion_table) {
        return None;
    }
    Some(new_level as u8)
}

/// Sheets of the target chunk `[cx, cz]` of the zone (see [`pack_sheets_below_top`]).
fn pack_target(
    level: &[u8],
    zone: Zone,
    [cx, cz]: [i32; 2],
    sections: usize,
    empty_byte: u8,
    drop_only_top_empty: bool,
    intern_uniform: bool,
) -> Vec<LightLayer> {
    let row_start = |y: i32, z: usize| zone.index(16 * cx, y, 16 * cz + z as i32);
    let rows = zone.bottom..zone.top;
    pack_sheets_below_top(level, sections, rows, row_start, empty_byte, drop_only_top_empty, intern_uniform)
}
