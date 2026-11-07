//! Terrain draw lists: columns visible in the frame, runs of sections in the field of view,
//! batches and indirect commands. No GPU object here: the construction is compared frame by frame
//! with the old one (every switch at 0) in the tests.
//!
//! Switches (read once): `RUSTCRAFT_OPT_VISORDER` (cached column order, array ranks,
//! merged translucent sections, cached radius), `RUSTCRAFT_OPT_REACHGRID` (reached
//! sections as a bit grid built by the walk task), `RUSTCRAFT_OPT_ARGSKIP`
//! (indirect commands uploaded only over the changed range), and the paths of [`ListPaths`]
//! (column command cache, hierarchical frustum, plane coherence, kept buffers, parallel builds).
//! `=0`: old path.

use std::ops::Range;
use std::sync::OnceLock;

use glam::{Vec3, Vec4};
use hashbrown::HashMap;
use rayon::prelude::*;

use rc_mesh::blocks::BUCKETS;
use rc_mesh::wire::Sections;

use super::column_upload::ColumnGpu;
use super::frustum::{aabb_inside, aabb_visible, box_inside_plane, box_reaches_plane, column_box, facing, section_box};
use super::{Batch, DrawArgs, TRANS_KEY};

fn switch(cell: &'static OnceLock<bool>, name: &str) -> bool {
    *cell.get_or_init(|| {
        let on = rc_data::env_flags::read_os(name).is_none_or(|v| v != "0");
        eprintln!("[opt] {name} : {on}");
        on
    })
}

pub(super) fn vis_order() -> bool {
    static S: OnceLock<bool> = OnceLock::new();
    switch(&S, "RUSTCRAFT_OPT_VISORDER")
}

pub(super) fn reach_grid() -> bool {
    static S: OnceLock<bool> = OnceLock::new();
    switch(&S, "RUSTCRAFT_OPT_REACHGRID")
}

pub(super) fn args_skip() -> bool {
    static S: OnceLock<bool> = OnceLock::new();
    switch(&S, "RUSTCRAFT_OPT_ARGSKIP")
}

/// How the lists are built; every field off is the old construction. Read once from the
/// switches by [`ListPaths::from_switches`]; the tests set the fields directly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct ListPaths {
    /// `RUSTCRAFT_OPT_VISORDER`: column order kept while its key repeats, region ranks by a
    /// counting sort, translucent sections merged from both ends.
    pub cached_order: bool,
    /// `RUSTCRAFT_OPT_COLUMN_COMMAND_CACHE`: the commands of each column are kept while what they
    /// are made from stays the same; the lists become copies of them, merged at the seams.
    pub column_commands: bool,
    /// `RUSTCRAFT_OPT_HIERARCHICAL_FRUSTUM`: box of the region of 16 × 16 columns first, then box
    /// of the column's reached sections; sections tested one by one only across a plane.
    pub hierarchical_frustum: bool,
    /// `RUSTCRAFT_OPT_PLANE_COHERENCE`: the plane that rejected a column at the last build is
    /// tested first.
    pub plane_coherence: bool,
    /// `RUSTCRAFT_OPT_SECTION_RANGE_SEARCH`: in a column across some planes, the sections in the
    /// field found by a binary search per crossed plane instead of a test per section.
    pub section_search: bool,
    /// `RUSTCRAFT_OPT_LIST_BUFFER_REUSE`: working buffers kept from one build to the next.
    pub reuse_buffers: bool,
    /// `RUSTCRAFT_OPT_PARALLEL_LISTS`: slices of this many columns as tasks on the current rayon
    /// pool, for the column selection and then, without the cache, the building of the lists, with
    /// it, the rebuilding of the columns whose commands changed; `None`: on the calling thread.
    pub column_slices: Option<usize>,
    /// `RUSTCRAFT_OPT_PARALLEL_LIST_KINDS`: the six lists as six tasks on the current rayon pool,
    /// built (without the cache, each one whole unless `column_slices` cuts it) or copied from the
    /// cache.
    pub parallel_kinds: bool,
    #[cfg(test)]
    pub sabotage: Option<Sabotage>,
}

/// Columns per task of `RUSTCRAFT_OPT_PARALLEL_LISTS`.
const SLICE_COLUMNS: usize = 128;

impl ListPaths {
    /// Old construction: every path off (the reference of the tests).
    #[cfg(test)]
    pub const OLD: ListPaths = ListPaths {
        cached_order: false,
        column_commands: false,
        hierarchical_frustum: false,
        plane_coherence: false,
        section_search: false,
        reuse_buffers: false,
        column_slices: None,
        parallel_kinds: false,
        #[cfg(test)]
        sabotage: None,
    };

    /// Paths chosen by the switches (read once).
    pub fn from_switches() -> Self {
        static PATHS: OnceLock<ListPaths> = OnceLock::new();
        *PATHS.get_or_init(|| ListPaths {
            cached_order: vis_order(),
            column_commands: rc_data::opt_switch!("RUSTCRAFT_OPT_COLUMN_COMMAND_CACHE"),
            hierarchical_frustum: rc_data::opt_switch!("RUSTCRAFT_OPT_HIERARCHICAL_FRUSTUM"),
            plane_coherence: rc_data::opt_switch!("RUSTCRAFT_OPT_PLANE_COHERENCE"),
            section_search: rc_data::opt_switch!("RUSTCRAFT_OPT_SECTION_RANGE_SEARCH"),
            reuse_buffers: rc_data::opt_switch!("RUSTCRAFT_OPT_LIST_BUFFER_REUSE"),
            column_slices: rc_data::opt_switch!("RUSTCRAFT_OPT_PARALLEL_LISTS", default_off).then_some(SLICE_COLUMNS),
            parallel_kinds: rc_data::opt_switch!("RUSTCRAFT_OPT_PARALLEL_LIST_KINDS", default_off),
            #[cfg(test)]
            sabotage: None,
        })
    }

    /// Some part runs as tasks: the caller installs [`list_pool`] around the build.
    pub fn parallel(&self) -> bool {
        self.column_slices.is_some() || self.parallel_kinds
    }

    #[cfg(test)]
    fn sabotaged(&self, s: Sabotage) -> bool {
        self.sabotage == Some(s)
    }

    #[cfg(not(test))]
    fn sabotaged(&self, _: Sabotage) -> bool {
        false
    }
}

/// Deliberate errors the equivalence tests must catch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Sabotage {
    /// The column command cache ignores the visible face groups: stale ones reused after a step.
    StaleFacings,
    /// The column command cache ignores the mesh version: a remesh written in place goes unseen.
    IgnoredMeshVersion,
    /// The column command cache ignores the eye height: translucent sections keep a stale order.
    IgnoredEyeHeight,
    /// The column command cache is kept when the face vertices or the split draws change.
    CacheKeptAcrossSettings,
    /// The cache's runs are joined across sections that draw nothing (the command order changes).
    RunsJoinedAcrossEmptySections,
    /// A slice's batches keep their place in the slice instead of their place in the list.
    ForgottenBatchOffset,
    /// A column across a plane is accepted whole instead of section by section.
    StraddlingColumnAcceptedWhole,
    /// The section search keeps the lower bound of the planes facing up and forgets the others.
    SectionSearchFromBelowOnly,
}

/// Pool of the parallel builds: its own threads, because the common pool is busy with meshing
/// (8 to 9 ms of wait measured). `RUSTCRAFT_LIST_THREADS` sets its size, otherwise half the
/// logical cores (at least one).
pub(super) fn list_pool() -> &'static rayon::ThreadPool {
    static POOL: OnceLock<rayon::ThreadPool> = OnceLock::new();
    POOL.get_or_init(|| {
        let threads = rc_data::env_str!("RUSTCRAFT_LIST_THREADS")
            .and_then(|v| v.parse().ok())
            .unwrap_or_else(|| (std::thread::available_parallelism().map_or(1, std::num::NonZero::get) / 2).max(1));
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .thread_name(|i| format!("draw lists {i}"))
            .build()
            .expect("draw list pool")
    })
}

/// Inputs of the construction of a frame.
pub(super) struct ListIn<'a> {
    pub columns: &'a HashMap<(i32, i32), ColumnGpu>,
    /// Reached sections, stored by column (x, z) then height.
    pub reached: &'a [(i32, i32, i32)],
    pub planes: &'a [Vec4; 6],
    /// Integer origin (x, z) of the render frame.
    pub origin: [i32; 2],
    pub eye: [f64; 3],
    pub min_y: i32,
    pub sections: i32,
    pub face_vertices: u32,
    pub split_draws: bool,
    /// Keys of the order cache: walk generation, column version.
    pub reached_gen: u64,
    pub columns_rev: u64,
}

/// Outputs (buffers kept from one frame to the next) and caches of the construction.
#[derive(Default)]
pub(super) struct Lists {
    pub runs: Vec<Range<usize>>,
    pub batches: Vec<Batch>,
    pub args: Vec<DrawArgs>,
    /// Candidate columns from farthest to nearest; valid for (walk generation, origin, column
    /// version).
    order_key: Option<OrderKey>,
    order: Vec<OrderedColumn>,
    /// Plane that last rejected each column of `order` (`RUSTCRAFT_OPT_PLANE_COHERENCE`).
    plane_hints: Vec<u8>,
    /// Key of the previous frame (the order is kept only on its second frame).
    seen_key: Option<OrderKey>,
    rank: HashMap<(i32, i32), u32>,
    /// Commands of each column (`RUSTCRAFT_OPT_COLUMN_COMMAND_CACHE`).
    commands: ColumnCommandCache,
    /// Working buffers kept between builds (`RUSTCRAFT_OPT_LIST_BUFFER_REUSE`).
    scratch: ListScratch,
}

impl Lists {
    /// Columns whose commands the last build reused from the cache, and those it rebuilt.
    pub fn column_command_reuse(&self) -> (u64, u64) {
        (self.commands.hits, self.commands.misses)
    }

    /// Forgets the cached commands of every column. Called when the terrain is cleared: its buffers
    /// restart from their first place, so a column of the next world could meet the key of an entry
    /// left from this one.
    pub fn forget_columns(&mut self) {
        self.commands = ColumnCommandCache::default();
    }

    /// Columns held by the command cache and the bytes they take (micro-bench).
    #[cfg(test)]
    fn command_cache_size(&self) -> (usize, usize) {
        let c = &self.commands;
        let entries: usize = c
            .entries
            .iter()
            .map(|e| {
                std::mem::size_of::<ColumnCommands>()
                    + e.commands.capacity() * std::mem::size_of::<DrawArgs>()
                    + e.runs.capacity() * std::mem::size_of::<Range<usize>>()
            })
            .sum();
        (c.slots.len(), entries + c.slots.capacity() * 16 + c.free.capacity() * 4)
    }
}

/// Key of the column order: walk generation, origin, column version.
type OrderKey = (u64, [i32; 2], u64);

/// Candidate column of the cached order: distance² to the origin, coordinates, its sections
/// `reached[first..end]`.
#[derive(Clone, Copy)]
struct OrderedColumn {
    dist2: i64,
    key: (i32, i32),
    first: u32,
    end: u32,
}

/// Visible column of the frame: its data, coordinates and range of its runs in `Lists::runs`.
struct VisibleColumn<'a> {
    column: &'a ColumnGpu,
    key: (i32, i32),
    runs: Range<usize>,
}

/// Candidate column of the uncached path: distance² to the origin, then the visible column.
struct Candidate<'a> {
    dist2: i64,
    visible: VisibleColumn<'a>,
}

/// Working buffers of one build (kept with `RUSTCRAFT_OPT_LIST_BUFFER_REUSE`).
#[derive(Default)]
struct ListScratch {
    /// Region rank of each visible column, then the start of each rank in `near`.
    ranks: Vec<u32>,
    rank_starts: Vec<usize>,
    /// Visible columns grouped by region from the nearest (opaque and cutout lists), and
    /// from the farthest to the nearest (translucent lists).
    near: Vec<usize>,
    far: Vec<usize>,
    /// Groups of faces seen from the front, per visible column.
    facings: Vec<[(usize, usize); BUCKETS]>,
    pieces: PieceScratch,
    /// Cache slot of each visible column, and the visible columns whose commands are rebuilt.
    slots: Vec<u32>,
    missed: Vec<usize>,
}

/// Sections of a column in draw order, and the sections of its runs one by one.
#[derive(Default)]
struct PieceScratch {
    pieces: Vec<Range<usize>>,
    sections: Vec<usize>,
}

/// Builds the runs, batches and commands of the frame; returns (columns, faces) drawn. `ordered`:
/// called once the column order is fixed (end of the "visibility" measurement). With a parallel
/// path, the tasks run on the current rayon pool.
pub(super) fn build(input: &ListIn<'_>, out: &mut Lists, paths: ListPaths, ordered: impl FnOnce()) -> (usize, u64) {
    let mut scratch = if paths.reuse_buffers { std::mem::take(&mut out.scratch) } else { ListScratch::default() };
    let visible = select_visible(input, out, paths);
    ordered();
    let ListScratch { ranks, rank_starts, near, far, facings, pieces, slots, missed } = &mut scratch;
    if paths.cached_order {
        group_by_region(&visible, &mut out.rank, ranks, rank_starts, near);
    } else {
        *near = group_by_region_with_a_sort(&visible);
    }
    far.clear();
    far.extend(0..visible.len());
    facings.clear();
    facings.extend(visible.iter().map(|v| facing(input.eye, v.key, input.min_y)));
    out.batches.clear();
    out.args.clear();
    let emission = Emission {
        visible: &visible,
        runs: &out.runs,
        facings,
        near,
        far,
        eye_height: input.eye[1] - f64::from(input.min_y),
        face_vertices: input.face_vertices,
        split_draws: input.split_draws,
        merged_translucent: paths.cached_order,
    };
    let faces = if paths.column_commands {
        let cache_scratch = CacheScratch { slots, missed, pieces };
        emit_from_cache(&emission, &mut out.commands, cache_scratch, &mut out.batches, &mut out.args, paths)
    } else if paths.parallel() {
        emit_in_tasks(&emission, paths.column_slices, &mut out.batches, &mut out.args, paths)
    } else {
        let mut faces = 0;
        for kind in LIST_KINDS {
            faces += emission.emit_span(kind, 0..visible.len(), pieces, &mut out.batches, &mut out.args);
        }
        faces
    };
    if paths.reuse_buffers {
        out.scratch = scratch;
    }
    (visible.len(), faces)
}

/// Visible columns from farthest to nearest, their runs written into `out.runs`.
fn select_visible<'a>(input: &ListIn<'a>, out: &mut Lists, paths: ListPaths) -> Vec<VisibleColumn<'a>> {
    out.runs.clear();
    let culling = ColumnCulling::new(input, paths);
    let key = (input.reached_gen, input.origin, input.columns_rev);
    // Order of all the candidates kept only when the key repeats (rotation in place,
    // slow walk): an origin that changes every frame (fast flight) would redo it for nothing.
    let cached = paths.cached_order && (out.order_key == Some(key) || out.seen_key.replace(key) == Some(key));
    if !cached {
        return sorted_visible(input, &culling, &mut out.runs);
    }
    refresh_order(input, out, key);
    match paths.column_slices {
        Some(slice) => select_in_slices(input, &culling, &out.order, &mut out.plane_hints, slice, &mut out.runs),
        None => select_in_order(input, &culling, &out.order, &mut out.plane_hints, &mut out.runs),
    }
}

/// Candidates of `order` whose runs are not empty, in that order.
fn select_in_order<'a>(
    input: &ListIn<'a>,
    culling: &ColumnCulling<'_>,
    order: &[OrderedColumn],
    hints: &mut [u8],
    runs: &mut Vec<Range<usize>>,
) -> Vec<VisibleColumn<'a>> {
    let mut regions = RegionVerdicts::new(input.origin);
    let mut visible = Vec::new();
    for (c, hint) in order.iter().zip(hints) {
        let first = runs.len();
        culling.column_runs(&mut regions, c.key, &input.reached[c.first as usize..c.end as usize], hint, runs);
        if runs.len() > first {
            // Present: the order keeps only the loaded columns.
            visible.push(VisibleColumn { column: &input.columns[&c.key], key: c.key, runs: first..runs.len() });
        }
    }
    visible
}

/// [`select_in_order`] on slices of `slice` candidates as parallel tasks, then put end to end
/// (the runs of each slice shifted to their place in `runs`).
fn select_in_slices<'a>(
    input: &ListIn<'a>,
    culling: &ColumnCulling<'_>,
    order: &[OrderedColumn],
    hints: &mut [u8],
    slice: usize,
    runs: &mut Vec<Range<usize>>,
) -> Vec<VisibleColumn<'a>> {
    let slice = slice.max(1);
    let parts: Vec<(Vec<Range<usize>>, Vec<VisibleColumn<'a>>)> = order
        .par_chunks(slice)
        .zip(hints.par_chunks_mut(slice))
        .map(|(order, hints)| {
            let mut part_runs = Vec::new();
            let visible = select_in_order(input, culling, order, hints, &mut part_runs);
            (part_runs, visible)
        })
        .collect();
    let mut visible = Vec::with_capacity(parts.iter().map(|p| p.1.len()).sum());
    for (part_runs, part) in parts {
        let shift = runs.len();
        runs.extend(part_runs);
        visible.extend(part.into_iter().map(|v| VisibleColumn { runs: v.runs.start + shift..v.runs.end + shift, ..v }));
    }
    visible
}

/// Horizontal distance² from the centre of column `k` to the frame origin.
fn dist2(origin: [i32; 2], k: (i32, i32)) -> i64 {
    let (dx, dz) = (i64::from(k.0 * 16 + 8 - origin[0]), i64::from(k.1 * 16 + 8 - origin[1]));
    dx * dx + dz * dz
}

/// Vertical distance from the middle of section `sy` to the eye (`ey` from the bottom of the world).
fn section_dist(sy: usize, ey: f64) -> f64 {
    (sy as f64 * 16.0 + 8.0 - ey).abs()
}

/// Visible columns (runs added to `runs`) from farthest to nearest; at equal distance,
/// fixed order of the coordinates (stable image from one run to the next).
fn sorted_visible<'a>(
    input: &ListIn<'a>,
    culling: &ColumnCulling<'_>,
    runs: &mut Vec<Range<usize>>,
) -> Vec<VisibleColumn<'a>> {
    let mut regions = RegionVerdicts::new(input.origin);
    let mut cand: Vec<Candidate<'_>> = Vec::new();
    for secs in input.reached.chunk_by(|a, b| (a.0, a.2) == (b.0, b.2)) {
        let key = (secs[0].0, secs[0].2);
        let first = runs.len();
        // No plane remembered on this path: every column starts with the first plane.
        culling.column_runs(&mut regions, key, secs, &mut 0, runs);
        match input.columns.get(&key) {
            Some(column) if runs.len() > first => cand.push(Candidate {
                dist2: dist2(input.origin, key),
                visible: VisibleColumn { column, key, runs: first..runs.len() },
            }),
            _ => runs.truncate(first),
        }
    }
    cand.sort_unstable_by_key(|c| (std::cmp::Reverse(c.dist2), c.visible.key));
    cand.into_iter().map(|c| c.visible).collect()
}

/// Order of the candidate columns redone if the walk, the origin or the columns changed.
fn refresh_order(input: &ListIn<'_>, out: &mut Lists, key: OrderKey) {
    if out.order_key == Some(key) {
        return;
    }
    out.order.clear();
    let mut first = 0;
    for secs in input.reached.chunk_by(|a, b| (a.0, a.2) == (b.0, b.2)) {
        let k = (secs[0].0, secs[0].2);
        let end = first + secs.len();
        if input.columns.contains_key(&k) {
            out.order.push(OrderedColumn {
                dist2: dist2(input.origin, k),
                key: k,
                first: first as u32,
                end: end as u32,
            });
        }
        first = end;
    }
    out.order.sort_unstable_by_key(|c| (std::cmp::Reverse(c.dist2), c.key));
    out.plane_hints.clear();
    out.plane_hints.resize(out.order.len(), 0);
    out.order_key = Some(key);
}

/// Runs of sections of column `k` (reached sections `secs`, increasing height) whose
/// box (one block of margin: a model can overflow) is in the field of view, added to `runs`
/// (old path, the reference of the tests).
#[cfg(test)]
fn column_runs(input: &ListIn<'_>, k: (i32, i32), secs: &[(i32, i32, i32)], runs: &mut Vec<Range<usize>>) {
    let culling = ColumnCulling::new(input, ListPaths::OLD);
    culling.column_runs(&mut RegionVerdicts::new(input.origin), k, secs, &mut 0, runs);
}

/// Verdict of a box against the frustum.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FrustumSide {
    Outside,
    Inside,
    Across,
}

/// Columns per side of a region of the hierarchical frustum, as a shift (16 columns).
const REGION_SHIFT: i32 = 4;
/// Regions kept on each side of the camera's region by [`RegionVerdicts`] (128 columns, more
/// than any view distance); a region beyond is tested at each use.
const REGION_REACH: i32 = 8;
const REGION_GRID: usize = 2 * REGION_REACH as usize + 1;

/// Frustum test of the columns of one build.
struct ColumnCulling<'a> {
    planes: &'a [Vec4; 6],
    origin: [i32; 2],
    min_y: i32,
    sections: i32,
    paths: ListPaths,
}

impl<'a> ColumnCulling<'a> {
    fn new(input: &ListIn<'a>, paths: ListPaths) -> Self {
        ColumnCulling {
            planes: input.planes,
            origin: input.origin,
            min_y: input.min_y,
            sections: input.sections,
            paths,
        }
    }

    /// Runs of the reached sections `secs` of column `k` (increasing height) whose box is in the
    /// field, added to `runs`. Every path gives the sections whose own box is visible: the
    /// enclosing boxes (region, column, reached sections) only skip tests whose answer they know
    /// (a box out of the field holds no visible box; a box wholly inside holds only visible ones).
    fn column_runs(
        &self,
        regions: &mut RegionVerdicts,
        k: (i32, i32),
        secs: &[(i32, i32, i32)],
        hint: &mut u8,
        runs: &mut Vec<Range<usize>>,
    ) {
        if !self.paths.hierarchical_frustum {
            let (min, max) = column_box(self.origin, self.min_y, self.sections, k);
            return self.runs_in_box(min, max, k, secs, hint, runs);
        }
        match regions.verdict(self, k) {
            FrustumSide::Outside => {}
            FrustumSide::Inside => push_section_runs(secs, |_| true, runs),
            FrustumSide::Across => {
                // Box of the reached sections only: tighter than the whole column, so more often
                // wholly in or out of the field.
                let (min, _) = section_box(self.origin, self.min_y, k, secs[0].1);
                let (_, max) = section_box(self.origin, self.min_y, k, secs[secs.len() - 1].1);
                self.runs_in_box(min, max, k, secs, hint, runs);
            }
        }
    }

    /// Runs of a column whose sections all lie in the box `min..max`: none if the box is out of the
    /// field, all if it is wholly inside, otherwise each section tested.
    fn runs_in_box(
        &self,
        min: Vec3,
        max: Vec3,
        k: (i32, i32),
        secs: &[(i32, i32, i32)],
        hint: &mut u8,
        runs: &mut impl Extend<Range<usize>>,
    ) {
        let visible = if self.paths.plane_coherence {
            visible_with_hint(self.planes, min, max, hint)
        } else {
            aabb_visible(self.planes, min, max)
        };
        if !visible {
            return;
        }
        if aabb_inside(self.planes, min, max) || self.paths.sabotaged(Sabotage::StraddlingColumnAcceptedWhole) {
            return push_section_runs(secs, |_| true, runs);
        }
        if self.paths.section_search {
            let seen = self.sections_across_planes(min, max, k, secs);
            return push_section_runs(&secs[seen], |_| true, runs);
        }
        push_section_runs(
            secs,
            |sy| {
                let (min, max) = section_box(self.origin, self.min_y, k, sy);
                aabb_visible(self.planes, min, max)
            },
            runs,
        );
    }

    /// Positions in `secs` (reached sections of column `k`, increasing height, all inside the box
    /// `min..max`) of the sections whose box reaches every plane. A plane the whole box is inside
    /// passes them all. For a plane it crosses, the test is monotonic in height (a section's box
    /// moves up along a fixed normal, and the rounded sums keep that order): the sections that pass
    /// are an upper part of `secs` (normal pointing up) or a lower part, found by a binary search
    /// with the very test of the per-section path, so the answer is the same.
    fn sections_across_planes(&self, min: Vec3, max: Vec3, k: (i32, i32), secs: &[(i32, i32, i32)]) -> Range<usize> {
        let (mut lo, mut hi) = (0, secs.len());
        for p in self.planes {
            if box_inside_plane(p, min, max) {
                continue;
            }
            let reaches = |s: &(i32, i32, i32)| {
                let (min, max) = section_box(self.origin, self.min_y, k, s.1);
                box_reaches_plane(p, min, max)
            };
            if p.y >= 0.0 {
                lo = lo.max(secs.partition_point(|s| !reaches(s)));
            } else if !self.paths.sabotaged(Sabotage::SectionSearchFromBelowOnly) {
                hi = hi.min(secs.partition_point(reaches));
            }
        }
        lo..hi.max(lo)
    }

    /// Box of region `(rx, rz)` (16 × 16 columns, whole height, same frame and margin as
    /// `column_box`: it holds the box of each of its columns).
    fn region_box(&self, (rx, rz): (i32, i32)) -> (Vec3, Vec3) {
        let span = 16 << REGION_SHIFT;
        let min =
            Vec3::new((rx * span - self.origin[0]) as f32, self.min_y as f32, (rz * span - self.origin[1]) as f32)
                - 1.0;
        (min, min + Vec3::new(span as f32 + 2.0, (self.sections * 16) as f32 + 2.0, span as f32 + 2.0))
    }

    fn region_verdict(&self, region: (i32, i32)) -> FrustumSide {
        let (min, max) = self.region_box(region);
        if !aabb_visible(self.planes, min, max) {
            FrustumSide::Outside
        } else if aabb_inside(self.planes, min, max) {
            FrustumSide::Inside
        } else {
            FrustumSide::Across
        }
    }
}

/// `aabb_visible`, starting with the plane that rejected this box last time (`hint`, updated):
/// a box out of the field is most often out by the same plane at the next frame, so one plane
/// test instead of several. Same answer, the planes are all tested when the box is visible.
fn visible_with_hint(planes: &[Vec4; 6], min: Vec3, max: Vec3, hint: &mut u8) -> bool {
    let first = usize::from(*hint);
    if !box_reaches_plane(&planes[first], min, max) {
        return false;
    }
    for (i, p) in planes.iter().enumerate() {
        if i != first && !box_reaches_plane(p, min, max) {
            *hint = i as u8;
            return false;
        }
    }
    true
}

/// Verdicts of the regions around the camera, each computed at its first use in a build.
struct RegionVerdicts {
    /// Region at index 0 of the grid: the camera's region minus `REGION_REACH` on each axis.
    corner: (i32, i32),
    cells: [Option<FrustumSide>; REGION_GRID * REGION_GRID],
}

impl RegionVerdicts {
    fn new(origin: [i32; 2]) -> Self {
        let region = |v: i32| (v >> 4 >> REGION_SHIFT) - REGION_REACH;
        RegionVerdicts { corner: (region(origin[0]), region(origin[1])), cells: [None; REGION_GRID * REGION_GRID] }
    }

    /// Verdict of the region holding column `k`.
    fn verdict(&mut self, culling: &ColumnCulling<'_>, k: (i32, i32)) -> FrustumSide {
        let region = (k.0 >> REGION_SHIFT, k.1 >> REGION_SHIFT);
        let (dx, dz) = (region.0 - self.corner.0, region.1 - self.corner.1);
        let grid = 0..REGION_GRID as i32;
        if !grid.contains(&dx) || !grid.contains(&dz) {
            return culling.region_verdict(region);
        }
        *self.cells[dx as usize * REGION_GRID + dz as usize].get_or_insert_with(|| culling.region_verdict(region))
    }
}

/// Adds to `runs` the runs of consecutive sections among `secs` (increasing height) that `keep`
/// accepts.
fn push_section_runs(
    secs: &[(i32, i32, i32)],
    mut keep: impl FnMut(i32) -> bool,
    runs: &mut impl Extend<Range<usize>>,
) {
    let mut open: Option<Range<usize>> = None;
    for &(_, sy, _) in secs {
        if !keep(sy) {
            continue;
        }
        let sy = sy as usize;
        open = match open {
            Some(r) if r.end >= sy => Some(r.start..r.end.max(sy + 1)),
            Some(r) => {
                runs.extend([r]);
                Some(sy..sy + 1)
            }
            None => Some(sy..sy + 1),
        };
    }
    runs.extend(open);
}

/// Sections among `reached` (the reached sections of column `key`, increasing height) whose box is
/// in the field of `planes`: the column test of the draw lists, for the meshing view. The region
/// hierarchy and the remembered plane pay off only over a whole build: left out here.
pub(super) fn sections_in_field(
    planes: &[Vec4; 6],
    origin: [i32; 2],
    (min_y, sections): (i32, i32),
    key: (i32, i32),
    reached: &[(i32, i32, i32)],
    paths: ListPaths,
) -> Sections {
    let paths = ListPaths { hierarchical_frustum: false, plane_coherence: false, ..paths };
    let culling = ColumnCulling { planes, origin, min_y, sections, paths };
    let (min, max) = column_box(origin, min_y, sections, key);
    let mut seen = SectionSet(Sections::default());
    culling.runs_in_box(min, max, key, reached, &mut 0, &mut seen);
    seen.0
}

/// Runs of sections written as a set (no allocation for a single column).
struct SectionSet(Sections);

impl Extend<Range<usize>> for SectionSet {
    fn extend<I: IntoIterator<Item = Range<usize>>>(&mut self, runs: I) {
        runs.into_iter().flatten().for_each(|sy| self.0.insert(sy as i32));
    }
}

/// Opaque and cutout order: rank of each region = order of first appearance starting from the
/// nearest; columns grouped by rank, from nearest to farthest within each group (same result as
/// the old path's stable sort by rank), by a counting sort.
fn group_by_region(
    visible: &[VisibleColumn<'_>],
    rank: &mut HashMap<(i32, i32), u32>,
    ranks: &mut Vec<u32>,
    rank_starts: &mut Vec<usize>,
    near: &mut Vec<usize>,
) {
    rank.clear();
    ranks.clear();
    ranks.resize(visible.len(), 0);
    for i in (0..visible.len()).rev() {
        let n = rank.len() as u32;
        ranks[i] = *rank.entry(visible[i].column.region).or_insert(n);
    }
    rank_starts.clear();
    rank_starts.resize(rank.len() + 1, 0);
    ranks.iter().for_each(|&r| rank_starts[r as usize + 1] += 1);
    for r in 1..rank_starts.len() {
        rank_starts[r] += rank_starts[r - 1];
    }
    near.clear();
    near.resize(visible.len(), 0);
    for i in (0..visible.len()).rev() {
        let r = ranks[i] as usize;
        near[rank_starts[r]] = i;
        rank_starts[r] += 1;
    }
}

/// [`group_by_region`] of the old path: a stable sort by the rank read from a map.
fn group_by_region_with_a_sort(visible: &[VisibleColumn<'_>]) -> Vec<usize> {
    let mut n: Vec<usize> = (0..visible.len()).rev().collect();
    let mut rank: HashMap<(i32, i32), usize> = HashMap::new();
    for &i in &n {
        let r = rank.len();
        rank.entry(visible[i].column.region).or_insert(r);
    }
    n.sort_by_key(|&i| rank[&visible[i].column.region]);
    n
}

/// One of the six terrain lists: layer (opaque, cutout, translucent), fluid quads or block faces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ListKind {
    layer: usize,
    fluid: bool,
}

/// The lists in draw order.
const LIST_KINDS: [ListKind; 6] = [
    ListKind { layer: 0, fluid: false },
    ListKind { layer: 0, fluid: true },
    ListKind { layer: 1, fluid: false },
    ListKind { layer: 1, fluid: true },
    ListKind { layer: 2, fluid: false },
    ListKind { layer: 2, fluid: true },
];

/// Key of a batch: layer, fluid, region (all the translucent fluids share one batch).
type BatchKey = (usize, bool, (i32, i32));

impl ListKind {
    /// Start (absolute) of the instances of each section of column `c` in this list, group after
    /// group, each with one more entry for the end; `None` when the list holds nothing of it.
    /// Translucent fluids: indices of the sorted draw order.
    fn starts(self, c: &ColumnGpu) -> Option<&[u32]> {
        let range = if self.fluid { &c.fluid_ranges } else { &c.ranges };
        if range[self.layer].is_empty() {
            return None;
        }
        Some(match (self.fluid, self.layer) {
            (true, 2) => &c.order_starts,
            (true, _) => &c.fluid_starts[self.layer],
            _ => &c.starts[self.layer],
        })
    }

    /// Groups of faces stored one after the other in [`ListKind::starts`].
    fn groups(self) -> usize {
        if self.fluid { 1 } else { BUCKETS }
    }

    fn batch_key(self, column: &ColumnGpu) -> BatchKey {
        let home = if self.fluid && self.layer == 2 { TRANS_KEY } else { column.region };
        (self.layer, self.fluid, home)
    }

    /// Command drawing the instances `r`: fluids as two triangles, block faces as a strip.
    fn command(self, r: Range<u32>, face_vertices: u32) -> DrawArgs {
        DrawArgs {
            vertex_count: if self.fluid { 6 } else { face_vertices },
            instance_count: r.end - r.start,
            first_vertex: 0,
            first_instance: r.start,
        }
    }
}

/// `next` draws the instances that follow those of `last`: one command for both (same
/// primitive order), unless each draw must stay apart.
fn continues(last: &DrawArgs, next: &DrawArgs, split_draws: bool) -> bool {
    !split_draws && last.first_instance + last.instance_count == next.first_instance
}

/// Appends the commands `cmds` of batch `key` (consecutive draws, already merged among
/// themselves): they continue the last batch if it has the same key, and the first one is merged
/// with the last command when its instances follow.
fn append_commands(
    batches: &mut Vec<Batch>,
    args: &mut Vec<DrawArgs>,
    key: BatchKey,
    cmds: &[DrawArgs],
    split_draws: bool,
) {
    let Some(first) = cmds.first() else { return };
    match batches.last_mut() {
        Some(b) if (b.0, b.1, b.2) == key => {
            let last = args.last_mut().expect("a batch holds a command");
            if continues(last, first, split_draws) {
                last.instance_count += first.instance_count;
                b.4 += cmds.len() as u32 - 1;
                args.extend_from_slice(&cmds[1..]);
                return;
            }
            b.4 += cmds.len() as u32;
        }
        _ => batches.push((key.0, key.1, key.2, args.len() as u32, cmds.len() as u32)),
    }
    args.extend_from_slice(cmds);
}

/// What the emission of the commands reads, shared by its paths.
struct Emission<'a, 'b> {
    visible: &'b [VisibleColumn<'a>],
    runs: &'b [Range<usize>],
    facings: &'b [[(usize, usize); BUCKETS]],
    near: &'b [usize],
    far: &'b [usize],
    /// Eye height above the bottom of the world.
    eye_height: f64,
    face_vertices: u32,
    split_draws: bool,
    /// Translucent sections ordered by the two-ended merge (otherwise the old stable sort).
    merged_translucent: bool,
}

impl Emission<'_, '_> {
    /// Visible columns in the order of list `kind`: from the farthest to the nearest for the
    /// translucent layer, grouped by region from the nearest for the others.
    fn order(&self, kind: ListKind) -> &[usize] {
        if kind.layer == 2 { self.far } else { self.near }
    }

    /// Instance ranges of visible column `i` drawn with its runs `runs` in list `kind`, in draw
    /// order, given to `emit`.
    fn column_ranges(
        &self,
        i: usize,
        runs: &[Range<usize>],
        kind: ListKind,
        scratch: &mut PieceScratch,
        mut emit: impl FnMut(Range<u32>),
    ) {
        let c = self.visible[i].column;
        let Some(starts) = kind.starts(c) else { return };
        let PieceScratch { pieces, sections } = scratch;
        pieces.clear();
        if kind.layer == 2 {
            // Translucent: section by section, from farthest to nearest.
            if self.merged_translucent {
                far_to_near(runs, self.eye_height, sections, pieces);
            } else {
                pieces.extend(runs.iter().flat_map(|r| r.clone()).map(|sy| sy..sy + 1));
                let ey = self.eye_height;
                pieces.sort_by(|a, b| section_dist(b.start, ey).total_cmp(&section_dist(a.start, ey)));
            }
        } else {
            pieces.extend(runs.iter().filter(|r| !r.is_empty()).cloned());
        }
        // Block faces stored by visible direction: a group entirely facing away
        // is not submitted (the GPU would have rejected it after the vertices).
        let (groups, stride) = (kind.groups(), starts.len() / kind.groups());
        let allowed = if kind.fluid { [(0, usize::MAX); BUCKETS] } else { self.facings[i] };
        for run in pieces.iter() {
            for (g, &(lo, hi)) in allowed.iter().enumerate().take(groups) {
                let (a, e) = (run.start.max(lo), run.end.min(hi));
                if a >= e {
                    continue;
                }
                let r = starts[g * stride + a]..starts[g * stride + e];
                if !r.is_empty() {
                    emit(r);
                }
            }
        }
    }

    /// Commands of the columns at positions `span` of list `kind`'s order, appended to `batches`
    /// and `args`; returns the faces drawn.
    fn emit_span(
        &self,
        kind: ListKind,
        span: Range<usize>,
        scratch: &mut PieceScratch,
        batches: &mut Vec<Batch>,
        args: &mut Vec<DrawArgs>,
    ) -> u64 {
        let mut faces = 0;
        for &i in &self.order(kind)[span] {
            let v = &self.visible[i];
            let key = kind.batch_key(v.column);
            self.column_ranges(i, &self.runs[v.runs.clone()], kind, scratch, |r| {
                faces += u64::from(r.end - r.start);
                append_commands(batches, args, key, &[kind.command(r, self.face_vertices)], self.split_draws);
            });
        }
        faces
    }
}

/// Batches and commands made by one task, to put end to end with the other tasks' ones.
#[derive(Default)]
struct Segment {
    batches: Vec<Batch>,
    args: Vec<DrawArgs>,
    faces: u64,
}

/// The six lists cut into tasks on the current rayon pool (one per list, or slices of `slice`
/// columns of each list), then put end to end: a task's first batch continues the previous task's
/// last one when they share a key, and its first command merges at the seam.
fn emit_in_tasks(
    emission: &Emission<'_, '_>,
    slice: Option<usize>,
    batches: &mut Vec<Batch>,
    args: &mut Vec<DrawArgs>,
    paths: ListPaths,
) -> u64 {
    let n = emission.visible.len();
    let step = slice.unwrap_or(n).max(1);
    let tasks: Vec<(ListKind, Range<usize>)> =
        LIST_KINDS.iter().flat_map(|&kind| (0..n).step_by(step).map(move |s| (kind, s..(s + step).min(n)))).collect();
    let segments: Vec<Segment> = tasks
        .par_iter()
        .map_init(PieceScratch::default, |scratch, (kind, span)| {
            let mut s = Segment::default();
            s.faces = emission.emit_span(*kind, span.clone(), scratch, &mut s.batches, &mut s.args);
            s
        })
        .collect();
    join_segments(&segments, batches, args, emission.split_draws, paths);
    segments.iter().map(|s| s.faces).sum()
}

/// Puts the tasks' segments end to end in `batches` and `args`: a segment's first batch continues
/// the previous one when they share a key, and its first command merges at the seam.
fn join_segments(
    segments: &[Segment],
    batches: &mut Vec<Batch>,
    args: &mut Vec<DrawArgs>,
    split_draws: bool,
    paths: ListPaths,
) {
    for s in segments {
        for &(layer, fluid, home, first, count) in &s.batches {
            let pushed = batches.len();
            append_commands(
                batches,
                args,
                (layer, fluid, home),
                &s.args[first as usize..(first + count) as usize],
                split_draws,
            );
            if paths.sabotaged(Sabotage::ForgottenBatchOffset) && batches.len() > pushed {
                batches.last_mut().expect("just pushed").3 = first;
            }
        }
    }
}

/// Mesh a column's commands are made from: its version and the starts of its faces, fluids,
/// translucents and translucent order in their buffers. A new upload changes one of them, and with
/// them every instance range. The key holds only because a version is never given twice in the
/// process (the meshing's counter never restarts, not even for a new world) and because
/// [`Lists::forget_columns`] empties the cache when the buffers restart from their first place.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MeshKey {
    version: u64,
    places: [u32; 4],
}

impl MeshKey {
    fn of(c: &ColumnGpu) -> Self {
        MeshKey { version: c.version, places: [c.face_alloc.0, c.fluid_alloc.0, c.trans_alloc.0, c.order_alloc.0] }
    }
}

/// What a column's commands depend on besides its mesh and its runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ViewKey {
    /// Groups of faces seen from the front.
    facing: [(usize, usize); BUCKETS],
    /// Bits of the eye height for a column with translucents (their sections are drawn from the
    /// farthest to the nearest in height), 0 otherwise.
    eye_height: u64,
}

impl ViewKey {
    fn of(c: &ColumnGpu, facing: [(usize, usize); BUCKETS], eye_height: f64) -> Self {
        let translucent = !c.ranges[2].is_empty() || !c.fluid_ranges[2].is_empty();
        ViewKey { facing, eye_height: if translucent { eye_height.to_bits() } else { 0 } }
    }
}

/// Sections of column `c` holding at least one instance of some list.
fn drawable_sections(c: &ColumnGpu) -> Sections {
    let mut drawable = Sections::default();
    for kind in LIST_KINDS {
        let Some(starts) = kind.starts(c) else { continue };
        let stride = starts.len() / kind.groups();
        if stride == 0 {
            continue;
        }
        for group in starts.chunks_exact(stride) {
            for (sy, w) in group.windows(2).enumerate() {
                if w[0] < w[1] {
                    drawable.insert(sy as i32);
                }
            }
        }
    }
    drawable
}

/// Runs without the sections that draw nothing at either end, the empty ones dropped. The commands
/// stay the same: an empty section adds no instance, so in every group a run's instance range
/// starts and ends at the same numbers. A run that grows or shrinks over air (the top plane
/// sweeping the sky while the camera turns) thus keeps its cached commands. The empty sections
/// inside a run stay: joining or splitting runs would change the order of the commands.
fn trimmed<'r>(runs: &'r [Range<usize>], drawable: &'r Sections) -> impl Iterator<Item = Range<usize>> + 'r {
    runs.iter().filter_map(move |r| {
        let start = r.clone().find(|&s| drawable.contains(s))?;
        let end = r.clone().rev().find(|&s| drawable.contains(s))? + 1;
        Some(start..end)
    })
}

/// Commands of one column for the six lists, already merged among themselves.
#[derive(Default)]
struct ColumnCommands {
    /// Mesh the entry describes, and its sections that draw something.
    mesh: Option<MeshKey>,
    drawable: Sections,
    view: Option<ViewKey>,
    /// Runs of sections in the field the commands were made for, without their empty ends.
    runs: Vec<Range<usize>>,
    /// Commands of list `LIST_KINDS[l]`: `commands[ends[l - 1]..ends[l]]` (from 0 for the first).
    commands: Vec<DrawArgs>,
    ends: [u32; LIST_KINDS.len()],
    faces: u64,
    /// Build that used it last: unused entries are dropped after `STALE_BUILDS` builds.
    used_at: u32,
}

impl ColumnCommands {
    /// Brings the entry to column `c` (its mesh `mesh`, seen with `view` and `runs`); true when
    /// the kept commands still hold, otherwise the entry's runs are those to rebuild them with.
    fn holds(&mut self, c: &ColumnGpu, mesh: MeshKey, view: ViewKey, runs: &[Range<usize>], paths: ListPaths) -> bool {
        if self.mesh != Some(mesh) {
            self.mesh = Some(mesh);
            self.drawable = drawable_sections(c);
            self.view = None;
        }
        if self.view == Some(view) && trimmed(runs, &self.drawable).eq(self.runs.iter().cloned()) {
            return true;
        }
        self.view = Some(view);
        self.runs.clear();
        self.runs.extend(trimmed(runs, &self.drawable));
        if paths.sabotaged(Sabotage::RunsJoinedAcrossEmptySections) {
            let drawable = self.drawable;
            self.runs.dedup_by(|next, prev| {
                let gap_is_empty = (prev.end..next.start).all(|s| !drawable.contains(s));
                if gap_is_empty {
                    prev.end = next.end;
                }
                gap_is_empty
            });
        }
        false
    }

    fn list(&self, l: usize) -> &[DrawArgs] {
        let start = if l == 0 { 0 } else { self.ends[l - 1] };
        &self.commands[start as usize..self.ends[l] as usize]
    }
}

/// Builds after which a column no build has drawn leaves the cache.
const STALE_BUILDS: u32 = 64;

/// Commands of the columns drawn by the recent builds (`RUSTCRAFT_OPT_COLUMN_COMMAND_CACHE`).
#[derive(Default)]
struct ColumnCommandCache {
    slots: HashMap<(i32, i32), u32>,
    entries: Vec<ColumnCommands>,
    free: Vec<u32>,
    /// Face vertices and split draws of the cached commands: a change empties the cache.
    settings: Option<(u32, bool)>,
    builds: u32,
    /// Columns served from the cache and rebuilt by the last build.
    hits: u64,
    misses: u64,
}

impl ColumnCommandCache {
    /// Start of a build: empties the cache if the command settings changed, and now and then drops
    /// the columns unused for `STALE_BUILDS` builds.
    fn begin(&mut self, face_vertices: u32, split_draws: bool, paths: ListPaths) {
        let changed = self.settings != Some((face_vertices, split_draws));
        if changed && !(self.settings.is_some() && paths.sabotaged(Sabotage::CacheKeptAcrossSettings)) {
            *self = ColumnCommandCache { settings: Some((face_vertices, split_draws)), ..Default::default() };
        }
        self.builds = self.builds.wrapping_add(1);
        (self.hits, self.misses) = (0, 0);
        if self.builds.is_multiple_of(STALE_BUILDS) {
            self.drop_stale();
        }
    }

    fn drop_stale(&mut self) {
        let (entries, free, now) = (&mut self.entries, &mut self.free, self.builds);
        self.slots.retain(|_, &mut slot| {
            let e = &mut entries[slot as usize];
            let keep = now.wrapping_sub(e.used_at) < STALE_BUILDS;
            if !keep {
                (e.mesh, e.view) = (None, None);
                free.push(slot);
            }
            keep
        });
    }

    /// Slot of column `key`'s entry (a free one at its first build), marked as used by this build.
    fn slot_of(&mut self, key: (i32, i32)) -> u32 {
        let slot = match self.slots.get(&key) {
            Some(&s) => s,
            None => {
                let s = self.free.pop().unwrap_or_else(|| {
                    self.entries.push(ColumnCommands::default());
                    self.entries.len() as u32 - 1
                });
                self.slots.insert(key, s);
                s
            }
        };
        self.entries[slot as usize].used_at = self.builds;
        slot
    }
}

/// Working buffers of [`emit_from_cache`].
struct CacheScratch<'s> {
    /// Cache slot of each visible column.
    slots: &'s mut Vec<u32>,
    /// Visible columns whose commands are rebuilt.
    missed: &'s mut Vec<usize>,
    pieces: &'s mut PieceScratch,
}

/// The six lists as copies of each column's cached commands (rebuilt only for the columns whose
/// sources changed), merged at the seams between columns as the old path merges them. With
/// `column_slices`, the changed columns are rebuilt as tasks of that many columns; with
/// `parallel_kinds`, the six lists are copied as six tasks, then put end to end.
fn emit_from_cache(
    emission: &Emission<'_, '_>,
    cache: &mut ColumnCommandCache,
    scratch: CacheScratch<'_>,
    batches: &mut Vec<Batch>,
    args: &mut Vec<DrawArgs>,
    paths: ListPaths,
) -> u64 {
    let CacheScratch { slots, missed, pieces } = scratch;
    cache.begin(emission.face_vertices, emission.split_draws, paths);
    find_changed_columns(emission, cache, slots, missed, paths);
    match paths.column_slices {
        Some(slice) => rebuild_in_tasks(emission, &mut cache.entries, slots, missed, slice),
        None => {
            for &i in missed.iter() {
                rebuild_column_commands(emission, i, pieces, &mut cache.entries[slots[i] as usize]);
            }
        }
    }
    let entries = &cache.entries;
    if paths.parallel_kinds {
        let segments: Vec<Segment> = (0..LIST_KINDS.len())
            .into_par_iter()
            .map(|l| {
                let mut s = Segment::default();
                copy_cached_list(emission, entries, slots, l, &mut s.batches, &mut s.args);
                s
            })
            .collect();
        join_segments(&segments, batches, args, emission.split_draws, paths);
    } else {
        for l in 0..LIST_KINDS.len() {
            copy_cached_list(emission, entries, slots, l, batches, args);
        }
    }
    slots.iter().map(|&slot| entries[slot as usize].faces).sum()
}

/// Cache slot of each visible column into `slots`, and into `missed` the visible columns whose
/// entry no longer holds (its runs then set to the new ones).
fn find_changed_columns(
    emission: &Emission<'_, '_>,
    cache: &mut ColumnCommandCache,
    slots: &mut Vec<u32>,
    missed: &mut Vec<usize>,
    paths: ListPaths,
) {
    slots.clear();
    missed.clear();
    for (i, v) in emission.visible.iter().enumerate() {
        let mut mesh = MeshKey::of(v.column);
        let mut view = ViewKey::of(v.column, emission.facings[i], emission.eye_height);
        if paths.sabotaged(Sabotage::StaleFacings) {
            view.facing = [(0, 0); BUCKETS];
        }
        if paths.sabotaged(Sabotage::IgnoredMeshVersion) {
            mesh.version = 0;
        }
        if paths.sabotaged(Sabotage::IgnoredEyeHeight) {
            view.eye_height = 0;
        }
        let slot = cache.slot_of(v.key);
        let entry = &mut cache.entries[slot as usize];
        if entry.holds(v.column, mesh, view, &emission.runs[v.runs.clone()], paths) {
            cache.hits += 1;
        } else {
            cache.misses += 1;
            missed.push(i);
        }
        slots.push(slot);
    }
}

/// Rebuilds the commands of the visible columns `missed` as tasks of `slice` columns: each entry
/// is taken out of the cache for its task, then put back in its slot.
fn rebuild_in_tasks(
    emission: &Emission<'_, '_>,
    entries: &mut [ColumnCommands],
    slots: &[u32],
    missed: &[usize],
    slice: usize,
) {
    let mut taken: Vec<(usize, ColumnCommands)> =
        missed.iter().map(|&i| (i, std::mem::take(&mut entries[slots[i] as usize]))).collect();
    taken.par_chunks_mut(slice.max(1)).for_each_init(PieceScratch::default, |pieces, chunk| {
        for (i, entry) in chunk {
            rebuild_column_commands(emission, *i, pieces, entry);
        }
    });
    for (i, entry) in taken {
        entries[slots[i] as usize] = entry;
    }
}

/// Copies list `LIST_KINDS[l]` of the cached commands of every visible column, in that list's
/// order, into `batches` and `args`.
fn copy_cached_list(
    emission: &Emission<'_, '_>,
    entries: &[ColumnCommands],
    slots: &[u32],
    l: usize,
    batches: &mut Vec<Batch>,
    args: &mut Vec<DrawArgs>,
) {
    let kind = LIST_KINDS[l];
    for &i in emission.order(kind) {
        let commands = entries[slots[i] as usize].list(l);
        append_commands(batches, args, kind.batch_key(emission.visible[i].column), commands, emission.split_draws);
    }
}

/// Commands of visible column `i` for the six lists, from the entry's runs, merged as within one
/// batch.
fn rebuild_column_commands(
    emission: &Emission<'_, '_>,
    i: usize,
    pieces: &mut PieceScratch,
    entry: &mut ColumnCommands,
) {
    let ColumnCommands { runs, commands, ends, faces, .. } = entry;
    commands.clear();
    *faces = 0;
    for (l, &kind) in LIST_KINDS.iter().enumerate() {
        let start = commands.len();
        emission.column_ranges(i, runs, kind, pieces, |r| {
            *faces += u64::from(r.end - r.start);
            let cmd = kind.command(r, emission.face_vertices);
            match commands[start..].last_mut() {
                Some(last) if continues(last, &cmd, emission.split_draws) => last.instance_count += cmd.instance_count,
                _ => commands.push(cmd),
            }
        });
        ends[l] = commands.len() as u32;
    }
}

/// Sections of the runs `runs` (increasing) one by one, from the farthest to the nearest
/// to the eye in height. The distance decreases then increases with height: the farthest of the
/// remaining ones is always at one end; on a tie, the lowest first, like a stable sort.
fn far_to_near(runs: &[Range<usize>], ey: f64, src: &mut Vec<usize>, out: &mut Vec<Range<usize>>) {
    src.clear();
    src.extend(runs.iter().flat_map(|r| r.clone()));
    // Remaining: src[i..j].
    let (mut i, mut j) = (0, src.len());
    while i < j {
        let sy = if section_dist(src[i], ey) >= section_dist(src[j - 1], ey) {
            i += 1;
            src[i - 1]
        } else {
            j -= 1;
            src[j]
        };
        out.push(sy..sy + 1);
    }
}

/// Range of the commands `new` to upload when the GPU buffer holds `old` (commands of the
/// previous frame): from the first different element to the last (or to the end if the length changes);
/// `None` if nothing changes. The GPU reads only the first `new.len()` commands.
pub(super) fn changed_range<T: bytemuck::Pod>(old: &[T], new: &[T]) -> Option<Range<usize>> {
    let (o, n): (&[u8], &[u8]) = (bytemuck::cast_slice(old), bytemuck::cast_slice(new));
    let size = std::mem::size_of::<T>();
    let common = o.len().min(n.len());
    let first = o[..common].iter().zip(&n[..common]).position(|(a, b)| a != b).map_or(common, |p| p / size * size);
    if first == n.len() {
        return None;
    }
    let last = if o.len() == n.len() {
        // Last differing byte, rounded to the end of its element.
        let p = o.iter().zip(n).rposition(|(a, b)| a != b).expect("one byte differs");
        (p / size + 1) * size
    } else {
        n.len()
    };
    Some(first / size..last / size)
}

/// Reached sections as a bit grid over their bounding box (≈ 13 KB at distance 32): same
/// answer as a set of the same sections, outside the box = not reached.
#[derive(Debug, Default)]
pub(super) struct ReachGrid {
    lo: [i64; 3],
    n: [i64; 3],
    bits: Vec<u64>,
}

impl ReachGrid {
    pub fn new(reached: &[(i32, i32, i32)]) -> Self {
        let Some(&first) = reached.first() else { return Self::default() };
        let (mut lo, mut hi) = ([first.0, first.1, first.2], [first.0, first.1, first.2]);
        for &(x, y, z) in reached {
            for (a, v) in [x, y, z].into_iter().enumerate() {
                lo[a] = lo[a].min(v);
                hi[a] = hi[a].max(v);
            }
        }
        let lo = lo.map(i64::from);
        let n = std::array::from_fn(|a| i64::from(hi[a]) - lo[a] + 1);
        let mut g = ReachGrid { lo, n, bits: vec![0; ((n[0] * n[1] * n[2]) as usize).div_ceil(64)] };
        for &(x, y, z) in reached {
            let i = g.index(x, y, z).expect("inside the box");
            g.bits[i >> 6] |= 1 << (i & 63);
        }
        g
    }

    fn index(&self, x: i32, y: i32, z: i32) -> Option<usize> {
        let d = [i64::from(x) - self.lo[0], i64::from(y) - self.lo[1], i64::from(z) - self.lo[2]];
        if (0..3).any(|a| d[a] < 0 || d[a] >= self.n[a]) {
            return None;
        }
        Some(((d[0] * self.n[2] + d[2]) * self.n[1] + d[1]) as usize)
    }

    pub fn contains(&self, x: i32, y: i32, z: i32) -> bool {
        self.index(x, y, z).is_some_and(|i| self.bits[i >> 6] >> (i & 63) & 1 != 0)
    }

    pub fn is_empty(&self) -> bool {
        self.bits.is_empty()
    }
}

/// Reached sections for the visibility test of entities, particles, block entities, leashes.
pub(super) enum Reach {
    Set(hashbrown::HashSet<(i32, i32, i32)>),
    Grid(ReachGrid),
}

impl Default for Reach {
    fn default() -> Self {
        Reach::Set(Default::default())
    }
}

impl Reach {
    pub fn contains(&self, x: i32, y: i32, z: i32) -> bool {
        match self {
            Reach::Set(s) => s.contains(&(x, y, z)),
            Reach::Grid(g) => g.contains(x, y, z),
        }
    }

    pub fn is_empty(&self) -> bool {
        match self {
            Reach::Set(s) => s.is_empty(),
            Reach::Grid(g) => g.is_empty(),
        }
    }
}

#[cfg(test)]
#[path = "lists_test.rs"]
mod tests;
