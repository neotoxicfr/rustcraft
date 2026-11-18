//! Draw order of the translucent quads (water, glass...): sorting a section from the farthest to the
//! nearest quad, the viewpoint rule that decides which sections of a column are sorted again as the
//! camera moves, and the per-frame re-sort of the columns (`TranslucentSweep`).
//!
//! Switches (read once): `RUSTCRAFT_OPT_TRANSKEY` (way of sorting a section), `RUSTCRAFT_OPT_TRANSPOV`
//! (sort section by section according to the viewpoint), the paths of [`SweepPaths`],
//! `RUSTCRAFT_SORTALL=1` (comparison).

use hashbrown::HashMap;

use super::column_upload::ColumnGpu;
use super::{ORDER_BYTES, Renderer};

/// Centre of a quad for the translucent order: middle of the diagonal vertex 0 – vertex 2,
/// like the game (differs from the mean of the 4 vertices for a non-planar quad or a trapezoid).
pub(super) fn quad_center<const N: usize>(p: &[[f32; N]; 4]) -> [f32; 3] {
    std::array::from_fn(|a| (p[0][a] + p[2][a]) * 0.5)
}

/// Draw order of the translucent quads of a column: within each section, from the farthest to the
/// nearest (distance of their centre to the camera, stable sort), like the game; absolute indices
/// into the region's fluid buffer (`base` = first translucent quad of the column).
pub(super) fn translucent_order(centers: &[[f32; 3]], starts: &[u32], eye: Option<[f32; 3]>, base: u32) -> Vec<u32> {
    let mut order: Vec<u32> = (0..centers.len() as u32).collect();
    if let Some(eye) = eye {
        let mut scratch = SortScratch::default();
        for w in starts.windows(2) {
            sort_section(centers, &mut order[w[0] as usize..w[1] as usize], eye, trans_key(), &mut scratch);
        }
    }
    order.iter_mut().for_each(|i| *i += base);
    order
}

/// Way of sorting a translucent section (`RUSTCRAFT_OPT_TRANSKEY`, same order bit for bit): `Old`
/// recomputes the two distances at each comparison of a stable sort; `Pairs` (`=pairs`) computes them
/// once then sorts (decreasing distance, index); `Bits` (default) sorts integer keys.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TransKey {
    Old,
    Pairs,
    Bits,
}

fn trans_key() -> TransKey {
    static K: std::sync::OnceLock<TransKey> = std::sync::OnceLock::new();
    *K.get_or_init(|| {
        let v = rc_data::env_flags::read_os("RUSTCRAFT_OPT_TRANSKEY");
        let k = match v.as_ref().and_then(|v| v.to_str()) {
            Some("0") => TransKey::Old,
            Some("pairs") => TransKey::Pairs,
            _ => TransKey::Bits,
        };
        eprintln!("[opt] translucent sort: {k:?}");
        k
    })
}

/// Buffers of the section sorts, kept from one sort to the next.
#[derive(Default)]
struct SortScratch {
    pairs: Vec<(f32, u32)>,
    keys: Vec<u64>,
}

/// Sorts `section` (indices into `centers`, increasing on input) from the farthest to the nearest of
/// `eye`, ties in input order; `scratch`: reused buffers.
fn sort_section(centers: &[[f32; 3]], section: &mut [u32], eye: [f32; 3], how: TransKey, scratch: &mut SortScratch) {
    if section.len() < 2 {
        return;
    }
    let dist = |i: u32| {
        let c = centers[i as usize];
        (0..3).map(|a| (c[a] - eye[a]) * (c[a] - eye[a])).sum::<f32>()
    };
    match how {
        TransKey::Old => section.sort_by(|&a, &b| dist(b).total_cmp(&dist(a))),
        TransKey::Pairs => {
            let pairs = &mut scratch.pairs;
            pairs.clear();
            pairs.extend(section.iter().map(|&i| (dist(i), i)));
            pairs.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
            section.iter_mut().zip(pairs.iter()).for_each(|(s, p)| *s = p.1);
        }
        TransKey::Bits => {
            // Float bits made increasing in the order of `total_cmp`, inverted for the
            // decreasing distance; the index in the low bits breaks ties.
            let keys = &mut scratch.keys;
            keys.clear();
            keys.extend(section.iter().map(|&i| {
                let b = dist(i).to_bits();
                let k = (b ^ (((b as i32) >> 31) as u32 >> 1)) ^ 0x8000_0000;
                u64::from(!k) << 32 | u64::from(i)
            }));
            keys.sort_unstable();
            section.iter_mut().zip(keys.iter()).for_each(|(s, &k)| *s = k as u32);
        }
    }
}

/// `RUSTCRAFT_OPT_TRANSPOV` (default): re-sort of the translucents section by section according to its
/// viewpoint, like the game (`docs/specs/visibility.md`, "Translucent sorting"); `=0`: whole column
/// re-sorted at each camera block, in rotation.
fn resort_by_view_cell() -> bool {
    static P: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *P.get_or_init(|| {
        let on = rc_data::env_flags::read_os("RUSTCRAFT_OPT_TRANSPOV").is_none_or(|v| v != "0");
        eprintln!("[opt] translucent re-sort by viewpoint: {on}");
        on
    })
}

/// Buffers of the per-section re-sort, kept from one frame to the next.
#[derive(Default)]
pub(super) struct TransScratch {
    order: Vec<u32>,
    sort: SortScratch,
    /// Sections of the examined column to re-sort.
    sections: Vec<usize>,
}

/// View cell never noted (section never sorted with a camera): always different.
const VIEW_CELL_UNSEEN: u8 = u8::MAX;

/// Section of the camera: section coordinates of its block.
pub(super) fn camera_section(eye: [f64; 3]) -> [i32; 3] {
    eye.map(|v| (v.floor() as i32) >> 4)
}

/// View cell of a section (section coordinates `sec`) from the camera's section `camera`: on each
/// axis, camera section minus this one, bounded to −1..1; coded (x + 1)·9 + (y + 1)·3 + (z + 1).
fn section_view_cell(camera: [i32; 3], sec: [i32; 3]) -> u8 {
    let c = |a: usize| (camera[a] - sec[a]).clamp(-1, 1) + 1;
    (c(0) * 9 + c(1) * 3 + c(2)) as u8
}

/// Camera within the slab of the section on at least one axis.
fn view_cell_aligned(p: u8) -> bool {
    p / 9 == 1 || p / 3 % 3 == 1 || p % 3 == 1
}

/// View cells of the sections of a column (`starts`: first quad of each section, plus the end;
/// `bottom`: bottom section of the world) from camera section `camera`, or never noted without a
/// camera.
pub(super) fn column_view_cells(camera: Option<[i32; 3]>, starts: &[u32], column: (i32, i32), bottom: i32) -> Vec<u8> {
    (0..starts.len().saturating_sub(1))
        .map(|s| camera.map_or(VIEW_CELL_UNSEEN, |c| section_view_cell(c, [column.0, bottom + s as i32, column.1])))
        .collect()
}

/// Translucent sections of an examined column to re-sort, into `out` (increasing), view cells
/// noted: view cell changed since the last sort, or camera that has just changed block
/// (`moved`) and section on the axis. Sections without translucents ignored.
fn view_cell_resorts(
    view_cells: &mut [u8],
    starts: &[u32],
    camera: [i32; 3],
    column: (i32, i32),
    bottom: i32,
    moved: bool,
    out: &mut Vec<usize>,
) {
    out.clear();
    for (s, w) in starts.windows(2).enumerate() {
        if w[1] == w[0] {
            continue;
        }
        let p = section_view_cell(camera, [column.0, bottom + s as i32, column.1]);
        if p != view_cells[s] || moved && view_cell_aligned(p) {
            view_cells[s] = p;
            out.push(s);
        }
    }
}

/// Translucent columns examined in rotation per frame: max(n / 8, 15), at most all.
fn rotation_window(n: usize) -> usize {
    (n / 8).max(15).min(n)
}

/// Column within ±2 of the camera's column.
fn is_near(k: (i32, i32), cam: (i32, i32)) -> bool {
    (k.0 - cam.0).abs() <= 2 && (k.1 - cam.1).abs() <= 2
}

/// The 25 columns within ±2 of the camera's column, in key order.
fn near_cells(cam: (i32, i32)) -> [(i32, i32); 25] {
    std::array::from_fn(|i| (cam.0 + i as i32 / 5 - 2, cam.1 + i as i32 % 5 - 2))
}

/// The columns of the sorted list `keys` within ±2 of `cam`, in key order (the order of
/// [`near_cells`]): five searches in the list instead of 25 map lookups.
fn near_in_list(keys: &[(i32, i32)], cam: (i32, i32), out: &mut Vec<(i32, i32)>) {
    out.clear();
    for x in cam.0 - 2..=cam.0 + 2 {
        let first = keys.partition_point(|&k| k < (x, cam.1 - 2));
        out.extend(keys[first..].iter().take_while(|&&k| k <= (x, cam.1 + 2)));
    }
}

/// Translucent columns examined per frame (`keys` non-empty, `start` < length): `near` (the
/// nearby ones), then max(n / 8, 15) in rotation from `start` without the nearby ones (already
/// seen); and the start of the next rotation.
fn sweep_picks<'a>(
    near: &'a [(i32, i32)],
    keys: &'a [(i32, i32)],
    start: usize,
    cam: (i32, i32),
) -> (impl Iterator<Item = (i32, i32)> + 'a, usize) {
    let n = keys.len();
    let window = rotation_window(n);
    let rotation = (0..window).map(move |i| keys[(start + i) % n]).filter(move |&k| !is_near(k, cam));
    (near.iter().copied().chain(rotation), (start + window) % n)
}

/// True if the order of the translucent quads of each section cannot change the image: in
/// each section (`starts` = first quad of each section), all in a single axis plane
/// (surface of a lake, single-plane pane), each contained in a unit cell, never two of the
/// same orientation in the same cell. Back faces removed: from any viewpoint, the quads
/// drawn of such a section do not overlap.
pub(super) fn order_free(quads: &[rc_mesh::fluid::FluidQuad], starts: &[u32]) -> bool {
    const EPS: f32 = 1e-4;
    let mut keys: Vec<(i32, i32, bool)> = Vec::new();
    for (i, &s) in starts.iter().enumerate() {
        let e = starts.get(i + 1).map_or(quads.len(), |&e| e as usize);
        let section = &quads[(s as usize).min(e)..e];
        if section.len() < 2 {
            continue;
        }
        let c = &section[0].corners;
        let Some(a) = (0..3).find(|&a| c.iter().all(|p| p[a] == c[0][a])) else { return false };
        let plane = c[0][a];
        let (u, v) = ((a + 1) % 3, (a + 2) % 3);
        keys.clear();
        for q in section {
            let p = &q.corners;
            if p.iter().any(|p| p[a] != plane) {
                return false;
            }
            let cell = |i: usize| {
                let lo = p.iter().map(|p| p[i]).fold(f32::MAX, f32::min);
                let hi = p.iter().map(|p| p[i]).fold(f32::MIN, f32::max);
                let k = (lo + EPS).floor();
                (hi <= k + 1.0 + EPS).then_some(k as i32)
            };
            let (Some(cu), Some(cv)) = (cell(u), cell(v)) else { return false };
            // Orientation: sign of the normal (p1 − p0) × (p2 − p0) on the plane's axis.
            let (e1, e2) = ([p[1][u] - p[0][u], p[1][v] - p[0][v]], [p[2][u] - p[0][u], p[2][v] - p[0][v]]);
            let front = e1[0] * e2[1] - e1[1] * e2[0] > 0.0;
            keys.push((cu, cv, front));
            // Double face: both orientations, like its two earlier quads.
            if q.texture & rc_mesh::fluid::DOUBLE_FACE != 0 {
                keys.push((cu, cv, !front));
            }
        }
        keys.sort_unstable();
        if keys.windows(2).any(|k| k[0] == k[1]) {
            return false;
        }
    }
    true
}

/// Camera relative to a column (vertex frame: x, z in the column, y from the bottom).
pub(super) fn eye_in_column(eye: [f64; 3], column: (i32, i32), min_y: i32) -> [f32; 3] {
    [
        (eye[0] - f64::from(column.0 * 16)) as f32,
        (eye[1] - f64::from(min_y)) as f32,
        (eye[2] - f64::from(column.1 * 16)) as f32,
    ]
}

/// Perf log: translucent columns examined by the re-sort per frame (by viewpoint: the nearby ones and
/// the rotation; old rule: every column of the list while one is unsorted).
const EXAMINED_COUNTER: &str = "translucent_columns_examined_per_frame";

/// `RUSTCRAFT_SORTALL=1`: all the translucent columns re-sorted (comparison).
pub(super) fn sort_all() -> bool {
    rc_data::env_flag!("RUSTCRAFT_SORTALL")
}

/// Paths of the translucent re-sort, read once from their switches (the tests choose them).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct SweepPaths {
    /// `RUSTCRAFT_OPT_TRANSPOV`: re-sort section by section according to the view cell.
    pub by_view_cell: bool,
    /// `RUSTCRAFT_OPT_TRANSLUCENT_SWEEP_SKIP`: a frame whose camera stays in its block examines no
    /// column once every translucent column has been examined since the camera entered its section.
    pub skip_settled: bool,
    /// `RUSTCRAFT_OPT_TRANSLUCENT_NEAR_LIST`: the nearby translucent columns are found in the sorted
    /// list and kept while the camera's column and the list stay the same, instead of 25 map lookups
    /// on every frame.
    pub near_list: bool,
    /// `RUSTCRAFT_OPT_INCREMENTAL_TRANSLUCENT_KEYS`: the list of translucent columns follows each
    /// upload and removal instead of being rebuilt from every column when any column changes. The
    /// old per-column rotation (`RUSTCRAFT_OPT_TRANSPOV=0`) always rebuilds: its "everything
    /// unsorted" reset hangs on that rebuild.
    pub incremental_list: bool,
    #[cfg(test)]
    pub sabotage: Option<SweepSabotage>,
}

/// Faults the tests put into the re-sort to see their proofs fail.
#[cfg(test)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum SweepSabotage {
    /// A skipped frame leaves the rotation where it was.
    SkipFreezesCursor,
    /// The incremental list forgets removed columns.
    RemovalIgnored,
}

impl SweepPaths {
    pub fn from_switches() -> Self {
        let by_view_cell = resort_by_view_cell();
        SweepPaths {
            by_view_cell,
            skip_settled: rc_data::opt_switch!("RUSTCRAFT_OPT_TRANSLUCENT_SWEEP_SKIP"),
            near_list: rc_data::opt_switch!("RUSTCRAFT_OPT_TRANSLUCENT_NEAR_LIST"),
            incremental_list: by_view_cell && rc_data::opt_switch!("RUSTCRAFT_OPT_INCREMENTAL_TRANSLUCENT_KEYS"),
            #[cfg(test)]
            sabotage: None,
        }
    }

    #[cfg(test)]
    fn sabotaged(&self, s: SweepSabotage) -> bool {
        self.sabotage == Some(s)
    }
}

/// A frame's camera, as the re-sort reads it.
#[derive(Clone, Copy, Debug)]
pub(super) struct SweepCamera {
    pub eye: [f64; 3],
    /// Bottom of the world.
    pub min_y: i32,
    /// Column of the camera.
    pub column: (i32, i32),
    /// Improved transparency: nothing is sorted.
    pub improved_transparency: bool,
}

/// What the re-sort of an examined column reads of the frame.
struct SweepView {
    eye: [f64; 3],
    min_y: i32,
    camera_section: [i32; 3],
    bottom_section: i32,
    how: TransKey,
}

/// What the translucent re-sort keeps from one frame to the next: the list of the columns whose
/// translucents are sorted, the rotation, and what proves that a frame has nothing to re-sort.
pub(super) struct TranslucentSweep {
    paths: SweepPaths,
    /// Columns whose translucent order is sorted (not free), by key.
    translucent_columns: Vec<(i32, i32)>,
    /// Version of the columns the list was rebuilt for (rebuild path).
    listed_for: Option<u64>,
    /// Bumped at each change of the list: the nearby columns kept are only valid for one.
    list_version: u64,
    /// Start of the next rotation.
    cursor: usize,
    /// Camera block of the last frame.
    camera_block: Option<[i32; 3]>,
    /// Camera section and bottom section of the world of the last frame, and their number: a view
    /// cell depends on nothing else, so a column examined since the number last changed holds the
    /// view cell of the current camera in every section ("settled").
    view_section: Option<([i32; 3], i32)>,
    view_epoch: u64,
    /// Columns of the list not settled for the current view epoch.
    unsettled: usize,
    /// Nearby translucent columns and the (camera column, list version) they were found for.
    near: Vec<(i32, i32)>,
    near_for: Option<((i32, i32), u64)>,
    /// Old per-column rotation: columns not yet sorted for the camera's block.
    legacy_unsorted: usize,
    scratch: TransScratch,
}

impl TranslucentSweep {
    pub(super) fn new(paths: SweepPaths) -> Self {
        TranslucentSweep {
            paths,
            translucent_columns: Vec::new(),
            listed_for: None,
            list_version: 0,
            cursor: 0,
            camera_block: None,
            view_section: None,
            view_epoch: 0,
            unsettled: 0,
            near: Vec::new(),
            near_for: None,
            legacy_unsorted: 0,
            scratch: TransScratch::default(),
        }
    }

    /// Number of the camera section of the last frame: a column uploaded now notes its view cells
    /// (made from that frame's camera) for it.
    pub(super) fn view_epoch(&self) -> u64 {
        self.view_epoch
    }

    /// Incremental list: column `key` was uploaded or removed; `before` / `after`: view epoch of its
    /// old / new mesh when that mesh has a sorted translucent order (`None` otherwise or absent).
    pub(super) fn column_replaced(&mut self, key: (i32, i32), before: Option<u64>, after: Option<u64>) {
        if !self.paths.incremental_list {
            return;
        }
        #[cfg(test)]
        let after = if self.paths.sabotaged(SweepSabotage::RemovalIgnored) { after.or(before) } else { after };
        match (self.translucent_columns.binary_search(&key), after.is_some()) {
            (Ok(i), false) => {
                self.translucent_columns.remove(i);
                self.list_version += 1;
            }
            (Err(i), true) => {
                self.translucent_columns.insert(i, key);
                self.list_version += 1;
            }
            _ => {}
        }
        let epoch = self.view_epoch;
        let unsettled = |e: Option<u64>| usize::from(e.is_some_and(|e| e != epoch));
        self.unsettled = (self.unsettled + unsettled(after)).saturating_sub(unsettled(before));
    }

    /// Every column is gone (renderer cleared).
    pub(super) fn forget_columns(&mut self) {
        self.translucent_columns.clear();
        self.list_version += 1;
        self.unsettled = 0;
    }

    /// One frame of the translucent sort, like the game: when the camera changes block, the
    /// neighbouring columns are re-sorted right away; the others in rotation, at least 15 per frame;
    /// nothing in improved transparency. `columns_rev`: version of the columns (rebuild path);
    /// `write(column, first place in the order buffer, order)` sends each re-sorted run. Returns
    /// the number of columns examined.
    pub(super) fn run(
        &mut self,
        columns: &mut HashMap<(i32, i32), ColumnGpu>,
        columns_rev: u64,
        camera: SweepCamera,
        write: impl FnMut((i32, i32), u32, &[u32]),
    ) -> usize {
        let moved = self.note_camera(camera);
        if !self.paths.incremental_list {
            self.rebuild_list(columns, columns_rev);
        }
        let examined = if self.paths.by_view_cell {
            self.sweep_by_view_cell(columns, camera, moved, write)
        } else {
            self.legacy_sweep(columns, camera, moved, write)
        };
        crate::perf_log::counter(EXAMINED_COUNTER, examined as u64);
        examined
    }

    /// Notes the frame's camera: whether it changed block, and a new view epoch when its section
    /// or the bottom of the world changed (no column is settled for it yet).
    fn note_camera(&mut self, camera: SweepCamera) -> bool {
        let block = camera.eye.map(|v| v.floor() as i32);
        let moved = self.camera_block != Some(block);
        self.camera_block = Some(block);
        let section = (block.map(|b| b >> 4), camera.min_y.div_euclid(16));
        if self.view_section != Some(section) {
            self.view_section = Some(section);
            self.view_epoch += 1;
            self.unsettled = self.translucent_columns.len();
        }
        moved
    }

    /// Rebuild path: the list made again from every column when any column changed, the columns
    /// not settled counted on the way.
    fn rebuild_list(&mut self, columns: &HashMap<(i32, i32), ColumnGpu>, columns_rev: u64) {
        if self.listed_for == Some(columns_rev) {
            return;
        }
        let epoch = self.view_epoch;
        self.translucent_columns.clear();
        self.unsettled = 0;
        for (&k, c) in columns.iter().filter(|(_, c)| !c.trans_centers.is_empty()) {
            self.translucent_columns.push(k);
            self.unsettled += usize::from(c.view_epoch != epoch);
        }
        self.translucent_columns.sort_unstable();
        self.listed_for = Some(columns_rev);
        self.list_version += 1;
        self.legacy_unsorted = self.translucent_columns.len();
    }

    /// Nearby translucent columns of the list, found again only when the camera's column or the
    /// list changed.
    fn refresh_near_list(&mut self, cam: (i32, i32)) {
        let key = (cam, self.list_version);
        if self.near_for != Some(key) {
            near_in_list(&self.translucent_columns, cam, &mut self.near);
            self.near_for = Some(key);
        }
    }

    /// Re-sort by view cell (`RUSTCRAFT_OPT_TRANSPOV`): columns within ±2 of the camera examined at
    /// every frame, plus max(n / 8, 15) others in rotation; in an examined column, only the sections
    /// to re-sort (`view_cell_resorts`) are, one write per run of contiguous sections.
    fn sweep_by_view_cell(
        &mut self,
        columns: &mut HashMap<(i32, i32), ColumnGpu>,
        camera: SweepCamera,
        moved: bool,
        mut write: impl FnMut((i32, i32), u32, &[u32]),
    ) -> usize {
        let n = self.translucent_columns.len();
        if camera.improved_transparency || n == 0 {
            return 0;
        }
        let start = self.cursor % n;
        self.cursor = (start + rotation_window(n)) % n;
        // A view cell only depends on the camera's section: once every column was examined since the
        // camera entered it, a frame that stays in its block would find nothing to re-sort.
        if self.paths.skip_settled && !moved && self.unsettled == 0 {
            #[cfg(test)]
            if self.paths.sabotaged(SweepSabotage::SkipFreezesCursor) {
                self.cursor = start;
            }
            return 0;
        }
        if self.paths.near_list {
            self.refresh_near_list(camera.column);
        }
        let view = SweepView {
            eye: camera.eye,
            min_y: camera.min_y,
            camera_section: camera_section(camera.eye),
            bottom_section: camera.min_y.div_euclid(16),
            how: trans_key(),
        };
        let epoch = self.view_epoch;
        let Self { paths, translucent_columns, near, scratch, unsettled, .. } = self;
        let cells;
        let near: &[(i32, i32)] = if paths.near_list {
            near.as_slice()
        } else {
            cells = near_cells(camera.column);
            &cells
        };
        let (picks, _) = sweep_picks(near, translucent_columns, start, camera.column);
        let mut examined = 0;
        for k in picks {
            let Some(c) = columns.get_mut(&k) else { continue };
            if c.trans_centers.is_empty() {
                continue;
            }
            examined += 1;
            resort_column(k, c, &view, moved, scratch, &mut write);
            if c.view_epoch != epoch {
                c.view_epoch = epoch;
                *unsettled = unsettled.saturating_sub(1);
            }
        }
        examined
    }

    /// Old per-column rotation (`RUSTCRAFT_OPT_TRANSPOV=0`): whole column re-sorted at each camera
    /// block, the neighbours first, then at least 15 others per frame.
    fn legacy_sweep(
        &mut self,
        columns: &mut HashMap<(i32, i32), ColumnGpu>,
        camera: SweepCamera,
        moved: bool,
        mut write: impl FnMut((i32, i32), u32, &[u32]),
    ) -> usize {
        let (eye, min_y, cam_col) = (camera.eye, camera.min_y, camera.column);
        let cam_block = eye.map(|v| v.floor() as i32);
        if moved {
            self.legacy_unsorted = self.translucent_columns.len();
        }
        let keys: &[(i32, i32)] = if self.legacy_unsorted > 0 { &self.translucent_columns } else { &[] };
        let mut budget = (keys.len() / 8).max(15);
        let n = keys.len();
        for step in 0..n {
            let k = keys[(self.cursor + step) % n];
            let near = (k.0 - cam_col.0).abs() <= 1 && (k.1 - cam_col.1).abs() <= 1;
            let Some(c) = columns.get_mut(&k) else { continue };
            if c.sorted_at == Some(cam_block) || !(near && moved || budget > 0) {
                continue;
            }
            if !near {
                budget -= 1;
            }
            let order = translucent_order(
                &c.trans_centers,
                &c.trans_starts,
                Some(eye_in_column(eye, k, min_y)),
                c.fluid_ranges[2].start,
            );
            c.sorted_at = Some(cam_block);
            crate::perf_log::trans_resorts(c.trans_starts.windows(2).filter(|w| w[1] > w[0]).count());
            write(k, c.order_alloc.0, &order);
        }
        if n > 0 {
            self.cursor = (self.cursor + (n / 8).max(15)) % n;
            self.legacy_unsorted =
                keys.iter().filter(|k| columns.get(*k).is_some_and(|c| c.sorted_at != Some(cam_block))).count();
        }
        n
    }

    /// `RUSTCRAFT_MESHCHECK=1`: the list kept by the uploads and the count of columns not settled
    /// against a rebuild from every column; a difference is printed.
    pub(super) fn check_against_rebuild(&self, columns: &HashMap<(i32, i32), ColumnGpu>) {
        if !self.paths.incremental_list {
            return;
        }
        let mut rebuilt: Vec<(i32, i32)> =
            columns.iter().filter(|(_, c)| !c.trans_centers.is_empty()).map(|(&k, _)| k).collect();
        rebuilt.sort_unstable();
        let unsettled = rebuilt.iter().filter(|k| columns[*k].view_epoch != self.view_epoch).count();
        if rebuilt != self.translucent_columns || unsettled != self.unsettled {
            eprintln!(
                "[meshcheck] translucent list kept by the uploads: {} columns, {} not settled; rebuilt: {} columns, {} not settled",
                self.translucent_columns.len(),
                self.unsettled,
                rebuilt.len(),
                unsettled
            );
        }
    }
}

/// Re-sorts the sections of column `key` whose view cell asks for it (`view_cell_resorts`), one
/// write per run of sections separated only by empty ones.
fn resort_column(
    key: (i32, i32),
    c: &mut ColumnGpu,
    view: &SweepView,
    moved: bool,
    scratch: &mut TransScratch,
    write: &mut impl FnMut((i32, i32), u32, &[u32]),
) {
    let TransScratch { order, sort, sections } = scratch;
    view_cell_resorts(&mut c.view_cells, &c.trans_starts, view.camera_section, key, view.bottom_section, moved, sections);
    let (eye, st, base) = (eye_in_column(view.eye, key, view.min_y), &c.trans_starts, c.fluid_ranges[2].start);
    let mut i = 0;
    while i < sections.len() {
        let mut j = i;
        while j + 1 < sections.len() && st[sections[j + 1]] == st[sections[j] + 1] {
            j += 1;
        }
        let (lo, hi) = (st[sections[i]], st[sections[j] + 1]);
        order.clear();
        order.extend(lo..hi);
        for &s in &sections[i..=j] {
            let r = (st[s] - lo) as usize..(st[s + 1] - lo) as usize;
            sort_section(&c.trans_centers, &mut order[r], eye, view.how, sort);
        }
        order.iter_mut().for_each(|v| *v += base);
        write(key, c.order_alloc.0 + lo, order);
        crate::perf_log::trans_resorts(j + 1 - i);
        i = j + 1;
    }
}

impl Renderer {
    /// Translucent sorting of the frame (`TranslucentSweep::run`): each re-sorted run is written
    /// into the order buffer.
    pub(super) fn resort_translucent_columns(&mut self, eye: [f64; 3], min_y: i32, cam_col: (i32, i32), oit_on: bool) {
        self.sort_eye = Some(eye);
        self.sort_min_y = min_y;
        let camera = SweepCamera { eye, min_y, column: cam_col, improved_transparency: oit_on };
        let (up, queue, order_buffer) = (&mut self.up, &self.queue, &self.trans.order);
        let write = |_: (i32, i32), at: u32, order: &[u32]| {
            up.write(queue, order_buffer, u64::from(at) * ORDER_BYTES, bytemuck::cast_slice(order));
        };
        self.translucent_sweep.run(&mut self.columns, self.columns_rev, camera, write);
        if crate::remesh::checking() {
            self.translucent_sweep.check_against_rebuild(&self.columns);
        }
    }
}

#[cfg(test)]
#[path = "translucent_sort_test.rs"]
mod tests;
