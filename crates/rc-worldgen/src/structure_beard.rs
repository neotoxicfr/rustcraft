//! Terrain adaptation to the structures ("beard", `generation-decoration.md` § 9.3): additive
//! contribution to the final density of a chunk, from the pieces of the starts it references.
//!
//! Every element (rigid piece or junction) only reaches a small box of blocks around itself
//! ([`Rigid::reach`], [`junction_reach`]); outside it, its term is exactly +0.0. No term is ever
//! -0.0 and the sum of a point starts at +0.0, so that sum is never -0.0 either, and skipping a
//! term worth +0.0 leaves it bit for bit unchanged. The column paths rely on this: each column only
//! visits the elements that reach it, in the order of the point-by-point sum.

use std::ops::Range;
use std::sync::{LazyLock, OnceLock};

use rayon::prelude::*;

use crate::jigsaw::{Junction, Projection};
use crate::structure::{Adapt, Piece, Start, StructureDef};
use crate::template::BBox;
use crate::terrain::StructureDensity;
use crate::volume::Volume;

struct Rigid {
    b: BBox,
    a: Adapt,
    delta: i32,
}

/// Contribution of a chunk.
pub struct Beard {
    rigids: Vec<Rigid>,
    junctions: Vec<Junction>,
    zone: Option<BBox>,
}

/// Optimisation switches of the beard, read once at first use. `COLUMN_BUFFER` and
/// `KERNEL_TABLES`: absent or any value other than `0` = new path, `0` = old path.
/// `COLUMN_LISTS` and `PARALLEL` lost on the loaded-machine micro-bench (figures in
/// `docs/perf.md`): off unless `=1`, kept for the quiet-machine scene bench. Both column paths
/// off = the point-by-point path. The equivalence tests and the bench pass their own values.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BeardPaths {
    /// Each column only visits the elements that reach it, listed once per chunk
    /// (`RUSTCRAFT_OPT_BEARD_COLUMN_LISTS`); without it, each column tests every element.
    pub column_lists: bool,
    /// Within a column, element after element, each over its own y range of a column buffer that
    /// is then added to the volume (`RUSTCRAFT_OPT_BEARD_COLUMN_BUFFER`).
    pub column_buffer: bool,
    /// With `column_buffer`: terms of every element kind but the box beard read from tables
    /// computed once (`RUSTCRAFT_OPT_BEARD_KERNEL_TABLES`).
    pub kernel_tables: bool,
    /// With a column path: the z rows of a chunk with enough work spread over the threads
    /// (`RUSTCRAFT_OPT_BEARD_PARALLEL`).
    pub parallel: bool,
}

impl BeardPaths {
    /// Code from before the column paths.
    pub const OLD: BeardPaths =
        BeardPaths { column_lists: false, column_buffer: false, kernel_tables: false, parallel: false };

    pub fn current() -> BeardPaths {
        static P: LazyLock<BeardPaths> = LazyLock::new(|| BeardPaths {
            column_lists: rc_data::env_str!("RUSTCRAFT_OPT_BEARD_COLUMN_LISTS").is_some_and(|v| v.trim() == "1"),
            column_buffer: rc_data::env_str!("RUSTCRAFT_OPT_BEARD_COLUMN_BUFFER").is_none_or(|v| v.trim() != "0"),
            kernel_tables: rc_data::env_str!("RUSTCRAFT_OPT_BEARD_KERNEL_TABLES").is_none_or(|v| v.trim() != "0"),
            parallel: rc_data::env_str!("RUSTCRAFT_OPT_BEARD_PARALLEL").is_some_and(|v| v.trim() == "1"),
        });
        *P
    }
}

/// Column slots (one element over one y) below which a chunk stays on one thread with
/// [`BeardPaths::parallel`].
const PARALLEL_MIN_WORK: usize = 1 << 16;

fn kernel() -> &'static [f32] {
    static K: OnceLock<Vec<f32>> = OnceLock::new();
    K.get_or_init(|| {
        let mut k = vec![0f32; 24 * 24 * 24];
        for zi in 0..24 {
            for xi in 0..24 {
                for yi in 0..24 {
                    let (dx, dy, dz) = (f64::from(xi - 12), f64::from(yi - 12) + 0.5, f64::from(zi - 12));
                    let d = dx * dx + dy * dy + dz * dz;
                    k[(zi * 576 + xi * 24 + yi) as usize] = std::f64::consts::E.powf(-d / 16.0) as f32;
                }
            }
        }
        k
    })
}

/// Index in the kernel (and in [`KernelTables`]) of the offsets (dx, dy, dz), each within
/// [-12, 11]; y varies fastest.
#[inline]
fn kernel_index(dx: i32, dy: i32, dz: i32) -> usize {
    ((dz + 12) * 576 + (dx + 12) * 24 + (dy + 12)) as usize
}

fn fast_inv_sqrt(x: f64) -> f64 {
    let half = 0.5 * x;
    let i = 0x5FE6_EB50_C7B5_37AAi64 - ((x.to_bits() as i64) >> 1);
    let y = f64::from_bits(i as u64);
    y * (1.5 - half * y * y)
}

/// [`fast_inv_sqrt`] for `x > 0`: its bits then fit in 63, so the logical shift equals the
/// arithmetic one, and unlike the 64-bit arithmetic shift it has an SSE2 instruction (`psrlq`),
/// which lets the y loops vectorise.
#[inline]
fn fast_inv_sqrt_positive(x: f64) -> f64 {
    let half = 0.5 * x;
    let i = 0x5FE6_EB50_C7B5_37AAi64 - (x.to_bits() >> 1) as i64;
    let y = f64::from_bits(i as u64);
    y * (1.5 - half * y * y)
}

fn bury(dx: f32, dy: f32, dz: f32) -> f32 {
    let q = dx * dx + dy * dy + dz * dz;
    if q >= 36.0 { 0.0 } else { 1.0 - q.sqrt() / 6.0 }
}

fn beard(dx: i32, dy: i32, dz: i32, to_ground: i32) -> f32 {
    let (xi, yi, zi) = (dx + 12, dy + 12, dz + 12);
    if !(0..24).contains(&xi) || !(0..24).contains(&yi) || !(0..24).contains(&zi) {
        return 0.0;
    }
    let h = to_ground as f32 + 0.5;
    let (fx, fz) = (dx as f32, dz as f32);
    let q = fx * fx + h * h + fz * fz;
    let v = -h * (fast_inv_sqrt(f64::from(q / 2.0)) as f32) / 2.0;
    v * kernel()[(zi * 576 + xi * 24 + yi) as usize]
}

/// [`beard`] without its kernel value, for offsets known to lie inside the kernel (the caller
/// multiplies by the kernel value, as [`beard`] does). `h` is never 0, so `q > 0`.
#[inline]
fn beard_ground_factor(dx: i32, dz: i32, to_ground: i32) -> f32 {
    let h = to_ground as f32 + 0.5;
    let (fx, fz) = (dx as f32, dz as f32);
    let q = fx * fx + h * h + fz * fz;
    -h * (fast_inv_sqrt_positive(f64::from(q / 2.0)) as f32) / 2.0
}

/// Kernel values of the 24 y offsets at (dx, dz).
fn kernel_row(kernel: &[f32], dx: i32, dz: i32) -> &[f32; 24] {
    let start = kernel_index(dx, -12, dz);
    kernel[start..start + 24].try_into().expect("24 kernel values")
}

/// Term of one element at every offset of a box of offsets (bounds excluded at the end), y
/// fastest, so the y range of a column is a contiguous run.
struct TermTable {
    x: Range<i32>,
    y: Range<i32>,
    z: Range<i32>,
    values: Vec<f32>,
}

impl TermTable {
    fn new(x: Range<i32>, y: Range<i32>, z: Range<i32>, term: impl Fn(i32, i32, i32) -> f32) -> TermTable {
        let mut values = Vec::with_capacity(x.len() * y.len() * z.len());
        for oz in z.clone() {
            for ox in x.clone() {
                values.extend(y.clone().map(|oy| term(ox, oy, oz)));
            }
        }
        TermTable { x, y, z, values }
    }

    /// Values from the offset (x, y, z) on, y increasing.
    fn run(&self, x: i32, y: i32, z: i32) -> &[f32] {
        let row = (z - self.z.start) as usize * self.x.len() + (x - self.x.start) as usize;
        &self.values[row * self.y.len() + (y - self.y.start) as usize..]
    }

    /// Values of every y offset at (x, z).
    fn row(&self, x: i32, z: i32) -> &[f32] {
        &self.run(x, self.y.start, z)[..self.y.len()]
    }
}

/// Terms of the element kinds that depend on three bounded offsets only, computed once by calling
/// the point-by-point terms on a one-block element at the origin (so with the very same
/// operations): offset = distance to the box for a piece, difference of coordinates for a
/// junction; y offset = to the ground, or to the box for `Encapsulate`. The bounds are those of
/// the reaches.
struct KernelTables {
    junction: TermTable,
    thin: TermTable,
    bury: TermTable,
    encapsulate: TermTable,
}

fn kernel_tables() -> &'static KernelTables {
    static T: OnceLock<KernelTables> = OnceLock::new();
    T.get_or_init(|| {
        let piece = |a| Rigid { b: BBox { min: [0; 3], max: [0; 3] }, a, delta: 0 };
        let (thin, bury, encapsulate) = (piece(Adapt::BeardThin), piece(Adapt::Bury), piece(Adapt::Encapsulate));
        let junction = Junction { x: 0, ground_y: 0, z: 0, dy: 0, projection: Projection::TerrainMatching };
        KernelTables {
            junction: TermTable::new(-12..12, -12..12, -12..12, |x, y, z| junction_term(&junction, x, y, z)),
            thin: TermTable::new(0..12, -12..12, 0..12, |x, y, z| thin.term(x, y, z)),
            bury: TermTable::new(0..6, -11..12, 0..6, |x, y, z| bury.term(x, y, z)),
            encapsulate: TermTable::new(0..12, 0..12, 0..12, |x, y, z| encapsulate.term(x, y, z)),
        }
    })
}

/// Distance from `p` to the segment [lo, hi] (0 inside).
#[inline]
fn distance_to(p: i32, lo: i32, hi: i32) -> i32 {
    0.max((lo - p).max(p - hi))
}

impl Rigid {
    fn ground(&self) -> i32 {
        self.b.min[1] + self.delta
    }

    /// Term of this piece at (x, y, z).
    fn term(&self, x: i32, y: i32, z: i32) -> f32 {
        let b = &self.b;
        let dx = distance_to(x, b.min[0], b.max[0]);
        let dz = distance_to(z, b.min[2], b.max[2]);
        let ground = self.ground();
        let ds = y - ground;
        let dy = match self.a {
            Adapt::None => 0,
            Adapt::Bury | Adapt::BeardThin => ds,
            Adapt::BeardBox => distance_to(y, ground, b.max[1]),
            Adapt::Encapsulate => distance_to(y, b.min[1], b.max[1]),
        };
        match self.a {
            Adapt::None => 0.0,
            Adapt::Bury => bury(dx as f32, dy as f32 / 2.0, dz as f32),
            Adapt::BeardThin | Adapt::BeardBox => beard(dx, dy, dz, ds) * 0.8,
            Adapt::Encapsulate => bury(dx as f32 / 2.0, dy as f32 / 2.0, dz as f32 / 2.0) * 0.8,
        }
    }

    /// Blocks outside which [`Self::term`] is exactly +0.0 (bounds included); `None` when it is
    /// +0.0 everywhere. The beard kernel spans the offsets −12 to +11 (here the y offset of a box
    /// is a distance, so 0 to +11); the burial sphere has radius 6 (squared distance < 36), with y
    /// halved for `Bury` and every axis halved for `Encapsulate`.
    fn reach(&self) -> Option<BBox> {
        let (b, ground) = (&self.b, self.ground());
        let (xz, y_lo, y_hi) = match self.a {
            Adapt::None => return None,
            Adapt::BeardThin => (11, ground - 12, ground + 11),
            Adapt::BeardBox => (11, ground - 11, b.max[1] + 11),
            Adapt::Bury => (5, ground - 11, ground + 11),
            Adapt::Encapsulate => (11, b.min[1] - 11, b.max[1] + 11),
        };
        (y_lo <= y_hi)
            .then_some(BBox { min: [b.min[0] - xz, y_lo, b.min[2] - xz], max: [b.max[0] + xz, y_hi, b.max[2] + xz] })
    }
}

/// Term of a junction at (x, y, z).
fn junction_term(j: &Junction, x: i32, y: i32, z: i32) -> f32 {
    let dy = y - j.ground_y;
    beard(x - j.x, dy, z - j.z, dy) * 0.4
}

/// Blocks outside which [`junction_term`] is exactly +0.0: the kernel offsets −12 to +11.
fn junction_reach(j: &Junction) -> BBox {
    BBox { min: [j.x - 12, j.ground_y - 12, j.z - 12], max: [j.x + 11, j.ground_y + 11, j.z + 11] }
}

impl Beard {
    /// Nearby pieces (within 12 blocks) of the adapted starts referenced by the chunk. The starts
    /// are read in place: borrowed from the reference lists, or owned copies (old path, tests).
    pub fn new<S: std::borrow::Borrow<Start>>(
        cx: i32,
        cz: i32,
        starts: &[(usize, Vec<S>)],
        defs: &[StructureDef],
    ) -> Option<Beard> {
        let (x0, z0) = (cx * 16, cz * 16);
        let mut rigids = Vec::new();
        let mut junctions = Vec::new();
        let mut any: Option<BBox> = None;
        let include = |b: BBox, any: &mut Option<BBox>| match any {
            Some(a) => a.encapsulate(&b),
            None => *any = Some(b),
        };
        for (s, list) in starts {
            let a = defs[*s].adapt;
            if a == Adapt::None {
                continue;
            }
            for st in list {
                for p in &st.borrow().pieces {
                    let bb = p.bbox();
                    if !bb.intersects_xz(x0 - 12, z0 - 12, x0 + 15 + 12, z0 + 15 + 12) {
                        continue;
                    }
                    match p {
                        Piece::Jigsaw(jp) => {
                            if jp.element.projection() == Projection::Rigid {
                                rigids.push(Rigid { b: bb, a, delta: jp.ground_delta });
                                include(bb, &mut any);
                            }
                            for j in &jp.junctions {
                                if j.x > x0 - 12 && j.z > z0 - 12 && j.x < x0 + 15 + 12 && j.z < z0 + 15 + 12 {
                                    junctions.push(*j);
                                    let p = [j.x, j.ground_y, j.z];
                                    include(BBox::new(p, p), &mut any);
                                }
                            }
                        }
                        _ => {
                            rigids.push(Rigid { b: bb, a, delta: 0 });
                            include(bb, &mut any);
                        }
                    }
                }
            }
        }
        let zone = any?.inflated(24);
        Some(Beard { rigids, junctions, zone: Some(zone) })
    }

    fn value(&self, x: i32, y: i32, z: i32) -> f32 {
        let mut v = 0f32;
        for r in &self.rigids {
            v += r.term(x, y, z);
        }
        for j in &self.junctions {
            v += junction_term(j, x, y, z);
        }
        v
    }

    /// [`StructureDensity::add_to_volume`] through the given paths.
    #[doc(hidden)]
    pub fn add_to_volume_with(&self, out: &mut [f32], v: &Volume, paths: BeardPaths) {
        self.add_with_parallel_threshold(out, v, paths, PARALLEL_MIN_WORK);
    }

    fn add_with_parallel_threshold(&self, out: &mut [f32], v: &Volume, paths: BeardPaths, parallel_min_work: usize) {
        let Some(zone) = self.zone else { return };
        // The column paths walk y as contiguous runs: they need the unit steps of the filling volume.
        let unit_steps = v.stx == 1 && v.sty == 1 && v.stz == 1;
        if !(paths.column_lists || paths.column_buffer) || !unit_steps {
            self.add_point_by_point(out, v, &zone);
            return;
        }
        if let Some(pass) = ColumnPass::new(self, &zone, v, paths) {
            let parallel = paths.parallel && pass.work() >= parallel_min_work;
            pass.run(out, parallel);
        }
    }

    fn add_point_by_point(&self, out: &mut [f32], v: &Volume, zone: &BBox) {
        for iz in 0..v.sz {
            let z = v.bz(iz);
            if z < zone.min[2] || z > zone.max[2] {
                continue;
            }
            for ix in 0..v.sx {
                let x = v.bx(ix);
                if x < zone.min[0] || x > zone.max[0] {
                    continue;
                }
                for iy in 0..v.sy {
                    let y = v.by(iy);
                    if y < zone.min[1] || y > zone.max[1] {
                        continue;
                    }
                    out[v.index(ix, iy, iz)] += self.value(x, y, z);
                }
            }
        }
    }

    /// Elements with a non-empty reach, in the order of the point-by-point sum (rigid pieces, then
    /// junctions).
    fn reached_elements(&self) -> Vec<Reached<'_>> {
        let rigids = self.rigids.iter().filter_map(|r| Some(Reached { reach: r.reach()?, element: Element::Rigid(r) }));
        let junctions =
            self.junctions.iter().map(|j| Reached { reach: junction_reach(j), element: Element::Junction(j) });
        rigids.chain(junctions).collect()
    }
}

impl StructureDensity for Beard {
    fn add_to_volume(&self, out: &mut [f32], v: &Volume) {
        self.add_to_volume_with(out, v, BeardPaths::current());
    }
}

#[derive(Clone, Copy)]
enum Element<'a> {
    Rigid(&'a Rigid),
    Junction(&'a Junction),
}

impl Element<'_> {
    fn term(self, x: i32, y: i32, z: i32) -> f32 {
        match self {
            Element::Rigid(r) => r.term(x, y, z),
            Element::Junction(j) => junction_term(j, x, y, z),
        }
    }
}

/// An element and the blocks it reaches.
struct Reached<'a> {
    reach: BBox,
    element: Element<'a>,
}

/// Index range of a unit-step axis starting at block `origin`, restricted to `within`, whose
/// blocks lie in [lo, hi] (possibly empty).
fn clip(within: &Range<i32>, lo: i32, hi: i32, origin: i32) -> Range<i32> {
    (lo - origin).max(within.start)..(hi - origin + 1).min(within.end)
}

/// Index ranges of the volume inside the zone (unit steps).
struct ZoneGrid {
    ix: Range<i32>,
    iy: Range<i32>,
    iz: Range<i32>,
}

impl ZoneGrid {
    fn new(zone: &BBox, v: &Volume) -> Option<ZoneGrid> {
        let ix = clip(&(0..v.sx), zone.min[0], zone.max[0], v.x0);
        let iy = clip(&(0..v.sy), zone.min[1], zone.max[1], v.y0);
        let iz = clip(&(0..v.sz), zone.min[2], zone.max[2], v.z0);
        (!ix.is_empty() && !iy.is_empty() && !iz.is_empty()).then_some(ZoneGrid { ix, iy, iz })
    }

    fn column(&self, ix: i32, iz: i32) -> usize {
        ((iz - self.iz.start) * self.ix.len() as i32 + (ix - self.ix.start)) as usize
    }

    /// Index ranges (x, y, z) of the grid that `reach` covers.
    fn clip(&self, v: &Volume, reach: &BBox) -> [Range<i32>; 3] {
        [
            clip(&self.ix, reach.min[0], reach.max[0], v.x0),
            clip(&self.iy, reach.min[1], reach.max[1], v.y0),
            clip(&self.iz, reach.min[2], reach.max[2], v.z0),
        ]
    }

    /// Columns of the grid where `reach` covers at least one y of the grid.
    fn columns_reached(&self, v: &Volume, reach: &BBox) -> impl Iterator<Item = usize> + '_ {
        let [x, y, z] = self.clip(v, reach);
        let x = if y.is_empty() { 0..0 } else { x };
        z.flat_map(move |iz| x.clone().map(move |ix| self.column(ix, iz)))
    }
}

/// For each column of the grid, the indices (into the reached elements) of those that reach it,
/// in summation order, as compressed rows: column `c` owns `elements[first[c]..first[c + 1]]`.
struct ColumnLists {
    first: Vec<u32>,
    elements: Vec<u32>,
}

impl ColumnLists {
    fn build(grid: &ZoneGrid, v: &Volume, reached: &[Reached]) -> ColumnLists {
        let columns = grid.ix.len() * grid.iz.len();
        let mut first = vec![0u32; columns + 1];
        for e in reached {
            for c in grid.columns_reached(v, &e.reach) {
                first[c] += 1;
            }
        }
        // Counts to start offsets; the extra last slot ends up holding the total.
        let mut total = 0u32;
        for slot in &mut first {
            let count = *slot;
            *slot = total;
            total += count;
        }
        let mut next = first.clone();
        let mut elements = vec![0u32; first[columns] as usize];
        for (i, e) in reached.iter().enumerate() {
            for c in grid.columns_reached(v, &e.reach) {
                elements[next[c] as usize] = i as u32;
                next[c] += 1;
            }
        }
        ColumnLists { first, elements }
    }

    fn column(&self, c: usize) -> &[u32] {
        &self.elements[self.first[c] as usize..self.first[c + 1] as usize]
    }
}

/// What every column of a chunk shares on the column paths.
struct ColumnPass<'a> {
    v: &'a Volume,
    grid: ZoneGrid,
    reached: Vec<Reached<'a>>,
    lists: Option<ColumnLists>,
    buffer: bool,
    tables: Option<&'static KernelTables>,
    kernel: &'static [f32],
}

impl<'a> ColumnPass<'a> {
    fn new(beard: &'a Beard, zone: &BBox, v: &'a Volume, paths: BeardPaths) -> Option<ColumnPass<'a>> {
        let grid = ZoneGrid::new(zone, v)?;
        let reached = beard.reached_elements();
        let lists = paths.column_lists.then(|| ColumnLists::build(&grid, v, &reached));
        let tables = (paths.column_buffer && paths.kernel_tables).then(kernel_tables);
        Some(ColumnPass { v, grid, reached, lists, buffer: paths.column_buffer, tables, kernel: kernel() })
    }

    /// Column slots to fill (one element over one y of the grid), to decide on threads.
    fn work(&self) -> usize {
        self.reached.iter().map(|e| self.grid.clip(self.v, &e.reach).iter().map(|r| r.len()).product::<usize>()).sum()
    }

    fn run(&self, out: &mut [f32], parallel: bool) {
        let row_len = (self.v.sx * self.v.sy) as usize;
        let first = self.grid.iz.start as usize;
        let rows = &mut out[first * row_len..self.grid.iz.end as usize * row_len];
        let column_len = self.grid.iy.len();
        let row_index = |i: usize| (first + i) as i32;
        if parallel {
            rows.par_chunks_mut(row_len)
                .enumerate()
                .for_each_init(|| vec![0f32; column_len], |column, (i, row)| self.add_row(row, row_index(i), column));
        } else {
            let mut column = vec![0f32; column_len];
            for (i, row) in rows.chunks_mut(row_len).enumerate() {
                self.add_row(row, row_index(i), &mut column);
            }
        }
    }

    /// Adds the beard to the z row `iz` of the volume (`row`: its `sx × sy` values).
    fn add_row(&self, row: &mut [f32], iz: i32, column: &mut [f32]) {
        let (sy, iy) = (self.v.sy as usize, &self.grid.iy);
        for ix in self.grid.ix.clone() {
            let base = ix as usize * sy;
            let slots = &mut row[base + iy.start as usize..base + iy.end as usize];
            if self.buffer {
                self.add_column_buffered(slots, ix, iz, column);
            } else {
                self.add_column_by_point(slots, ix, iz);
            }
        }
    }

    /// Calls `f` on the elements that reach column (ix, iz), in summation order: its list, or every
    /// element tested on the spot.
    fn for_each_element(&self, ix: i32, iz: i32, mut f: impl FnMut(&Reached<'a>)) {
        match &self.lists {
            Some(lists) => lists.column(self.grid.column(ix, iz)).iter().for_each(|&i| f(&self.reached[i as usize])),
            None => {
                let (x, z) = (self.v.bx(ix), self.v.bz(iz));
                self.reached.iter().filter(|e| e.reach.intersects_xz(x, z, x, z)).for_each(f);
            }
        }
    }

    /// Point first: the sum of a point over the column's elements, as the point-by-point path does.
    fn add_column_by_point(&self, slots: &mut [f32], ix: i32, iz: i32) {
        let (x, z) = (self.v.bx(ix), self.v.bz(iz));
        for (slot, iy) in slots.iter_mut().zip(self.grid.iy.clone()) {
            let y = self.v.by(iy);
            let mut sum = 0f32;
            self.for_each_element(ix, iz, |e| sum += e.element.term(x, y, z));
            *slot += sum;
        }
    }

    /// Element first: each element adds its terms over its own y range of `column`, in summation
    /// order, so each slot gets the same sum as the point-by-point path.
    fn add_column_buffered(&self, slots: &mut [f32], ix: i32, iz: i32, column: &mut [f32]) {
        let (x, z) = (self.v.bx(ix), self.v.bz(iz));
        let y_first = self.v.by(self.grid.iy.start);
        let column = &mut column[..slots.len()];
        column.fill(0.0);
        self.for_each_element(ix, iz, |e| {
            let (lo, hi) = (e.reach.min[1].max(y_first), e.reach.max[1].min(y_first + column.len() as i32 - 1));
            if lo <= hi {
                let span = &mut column[(lo - y_first) as usize..=(hi - y_first) as usize];
                match e.element {
                    Element::Rigid(r) => self.add_rigid(span, lo, x, z, r),
                    Element::Junction(j) => self.add_junction(span, lo, x, z, j),
                }
            }
        });
        // The point-by-point path adds a sum to every point of the zone, +0.0 included (which turns
        // a -0.0 density into +0.0): the whole column is added.
        add_run(slots, column);
    }

    /// Terms of a junction over `span` (blocks `y_first..`, all within its reach).
    fn add_junction(&self, span: &mut [f32], y_first: i32, x: i32, z: i32, j: &Junction) {
        let (dx, dz, dy_first) = (x - j.x, z - j.z, y_first - j.ground_y);
        if let Some(t) = self.tables {
            return add_run(span, t.junction.run(dx, dy_first, dz));
        }
        let row = kernel_row(self.kernel, dx, dz);
        for (dy, slot) in (dy_first..).zip(span) {
            *slot += beard_ground_factor(dx, dz, dy) * row[(dy + 12) as usize] * 0.4;
        }
    }

    /// Terms of a rigid piece over `span` (blocks `y_first..`, all within its reach), equal to
    /// those of [`Rigid::term`].
    fn add_rigid(&self, span: &mut [f32], y_first: i32, x: i32, z: i32, r: &Rigid) {
        let b = &r.b;
        let dx = distance_to(x, b.min[0], b.max[0]);
        let dz = distance_to(z, b.min[2], b.max[2]);
        let ground = r.ground();
        match (r.a, self.tables) {
            (Adapt::None, _) => {}
            (Adapt::BeardThin, Some(t)) => add_run(span, t.thin.run(dx, y_first - ground, dz)),
            (Adapt::Bury, Some(t)) => add_run(span, t.bury.run(dx, y_first - ground, dz)),
            (Adapt::Encapsulate, Some(t)) => {
                add_by_box_distance(span, y_first, b.min[1], b.max[1], t.encapsulate.row(dx, dz))
            }
            (Adapt::BeardBox, _) => self.add_box_beard(span, y_first, dx, dz, r),
            (Adapt::BeardThin | Adapt::Bury | Adapt::Encapsulate, None) => {
                self.add_rigid_computed(span, y_first, dx, dz, r);
            }
        }
    }

    /// Box beard: its ground offset is bounded by the box height only, so its terms are computed;
    /// the kernel row of the column is read once.
    fn add_box_beard(&self, span: &mut [f32], y_first: i32, dx: i32, dz: i32, r: &Rigid) {
        let (ground, top) = (r.ground(), r.b.max[1]);
        let row = kernel_row(self.kernel, dx, dz);
        for (y, slot) in (y_first..).zip(span) {
            // The reach keeps dy <= 11: `min(11)` changes nothing but proves the index in bounds,
            // which takes the bounds check out of the loop.
            let dy = distance_to(y, ground, top).min(11);
            *slot += beard_ground_factor(dx, dz, y - ground) * row[(dy + 12) as usize] * 0.8;
        }
    }

    /// Thin beard, burial and encapsulation without the tables.
    fn add_rigid_computed(&self, span: &mut [f32], y_first: i32, dx: i32, dz: i32, r: &Rigid) {
        let (b, ground) = (&r.b, r.ground());
        let ys = (y_first..).zip(span);
        match r.a {
            Adapt::BeardThin => {
                let row = kernel_row(self.kernel, dx, dz);
                for (y, slot) in ys {
                    let ds = y - ground;
                    *slot += beard_ground_factor(dx, dz, ds) * row[(ds + 12) as usize] * 0.8;
                }
            }
            Adapt::Bury => {
                for (y, slot) in ys {
                    *slot += bury(dx as f32, (y - ground) as f32 / 2.0, dz as f32);
                }
            }
            Adapt::Encapsulate => {
                for (y, slot) in ys {
                    let dy = distance_to(y, b.min[1], b.max[1]);
                    *slot += bury(dx as f32 / 2.0, dy as f32 / 2.0, dz as f32 / 2.0) * 0.8;
                }
            }
            Adapt::BeardBox | Adapt::None => {}
        }
    }
}

/// Adds `run` term by term to `slots` (as many terms as `slots` has).
#[inline]
fn add_run(slots: &mut [f32], run: &[f32]) {
    for (slot, t) in slots.iter_mut().zip(run) {
        *slot += *t;
    }
}

/// Adds `by_distance[d]` to each slot of `span` (blocks `y_first..`), `d` being the distance from
/// the block to [lo, hi] (`lo <= hi`). Split into the three parts where `d` is monotonic
/// (decreasing below the box, 0 inside, increasing above) so that each part is a plain run instead
/// of a lookup per block.
fn add_by_box_distance(span: &mut [f32], y_first: i32, lo: i32, hi: i32, by_distance: &[f32]) {
    let len = span.len() as i32;
    let below = (lo - y_first).clamp(0, len);
    let inside = (hi + 1 - y_first).clamp(below, len) - below;
    let (under, rest) = span.split_at_mut(below as usize);
    let (within, over) = rest.split_at_mut(inside as usize);
    if !under.is_empty() {
        let first = (lo - y_first) as usize;
        let run = &by_distance[first + 1 - under.len()..=first];
        under.iter_mut().zip(run.iter().rev()).for_each(|(slot, t)| *slot += *t);
    }
    within.iter_mut().for_each(|slot| *slot += by_distance[0]);
    if !over.is_empty() {
        add_run(over, &by_distance[(y_first + below + inside - hi) as usize..]);
    }
}

#[cfg(test)]
#[path = "structure_beard_tests.rs"]
mod tests;
