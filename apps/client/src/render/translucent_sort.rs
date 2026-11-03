//! Draw order of the translucent quads (water, glass...): sorting a section from the farthest to the
//! nearest quad, the viewpoint rule that decides which sections of a column are sorted again as the
//! camera moves, and the per-frame re-sort of the columns (`Renderer::resort_translucent_columns`).
//!
//! Switches (read once): `RUSTCRAFT_OPT_TRANSKEY` (way of sorting a section), `RUSTCRAFT_OPT_TRANSPOV`
//! (sort section by section according to the viewpoint), `RUSTCRAFT_SORTALL=1` (comparison).

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
fn trans_pov() -> bool {
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
}

/// Viewpoint never noted (section never sorted with a camera): always different.
const POV_UNSEEN: u8 = u8::MAX;

/// Viewpoint of a section (section coordinates `sec`): on each axis, camera section
/// minus this one, bounded to −1..1; coded (x + 1)·9 + (y + 1)·3 + (z + 1).
fn section_pov(eye: [f64; 3], sec: [i32; 3]) -> u8 {
    let c = |e: f64, s: i32| (((e.floor() as i32) >> 4) - s).clamp(-1, 1) + 1;
    (c(eye[0], sec[0]) * 9 + c(eye[1], sec[1]) * 3 + c(eye[2], sec[2])) as u8
}

/// Camera within the slab of the section on at least one axis.
fn pov_aligned(p: u8) -> bool {
    p / 9 == 1 || p / 3 % 3 == 1 || p % 3 == 1
}

/// Viewpoints of the sections of a column (`starts`: first quad of each section, plus the end;
/// `sy0`: bottom section of the world) for camera `eye`, or never noted without a camera.
pub(super) fn column_povs(eye: Option<[f64; 3]>, starts: &[u32], column: (i32, i32), sy0: i32) -> Vec<u8> {
    (0..starts.len().saturating_sub(1))
        .map(|s| eye.map_or(POV_UNSEEN, |e| section_pov(e, [column.0, sy0 + s as i32, column.1])))
        .collect()
}

/// Translucent sections of an examined column to re-sort, into `out` (increasing), viewpoints
/// noted: viewpoint changed since the last sort, or camera that has just changed block
/// (`moved`) and section on the axis. Sections without translucents ignored.
fn pov_resorts(
    povs: &mut [u8],
    starts: &[u32],
    eye: [f64; 3],
    column: (i32, i32),
    sy0: i32,
    moved: bool,
    out: &mut Vec<usize>,
) {
    out.clear();
    for (s, w) in starts.windows(2).enumerate() {
        if w[1] == w[0] {
            continue;
        }
        let p = section_pov(eye, [column.0, sy0 + s as i32, column.1]);
        if p != povs[s] || moved && pov_aligned(p) {
            povs[s] = p;
            out.push(s);
        }
    }
}

/// Translucent columns examined per frame (`keys` non-empty, `start` < length): the 25 within
/// ±2 of the camera's column, then max(n / 8, 15) in rotation from `start` (without the
/// nearby ones, already seen); and the start of the next rotation.
fn pov_picks(keys: &[(i32, i32)], start: usize, cam: (i32, i32)) -> (impl Iterator<Item = (i32, i32)> + '_, usize) {
    let n = keys.len();
    let window = (n / 8).max(15).min(n);
    let near = move |k: (i32, i32)| (k.0 - cam.0).abs() <= 2 && (k.1 - cam.1).abs() <= 2;
    let picks = (-2..=2)
        .flat_map(move |dx| (-2..=2).map(move |dz| (cam.0 + dx, cam.1 + dz)))
        .chain((0..window).map(move |i| keys[(start + i) % n]).filter(move |&k| !near(k)));
    (picks, (start + window) % n)
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

/// `RUSTCRAFT_SORTALL=1`: all the translucent columns re-sorted (comparison).
pub(super) fn sort_all() -> bool {
    rc_data::env_flag!("RUSTCRAFT_SORTALL")
}

impl Renderer {
    /// Translucent sorting, like the game: when the camera changes block, the neighbouring columns are
    /// re-sorted right away; the others in rotation, at least 15 per frame. Nothing is sorted in improved
    /// transparency (`oit_on`).
    pub(super) fn resort_translucent_columns(&mut self, eye: [f64; 3], min_y: i32, cam_col: (i32, i32), oit_on: bool) {
        let cam_block = eye.map(|v| v.floor() as i32);
        self.sort_eye = Some(eye);
        self.sort_min_y = min_y;
        let moved = self.sort_block != Some(cam_block);
        self.sort_block = Some(cam_block);
        // List of translucent columns redone only when the columns change; nothing to
        // do as long as the camera stays in the same block and everything is sorted.
        if self.trans_keys_rev != Some(self.columns_rev) {
            self.trans_keys =
                self.columns.iter().filter(|(_, c)| !c.trans_centers.is_empty()).map(|(&k, _)| k).collect();
            self.trans_keys.sort_unstable();
            self.trans_keys_rev = Some(self.columns_rev);
            self.unsorted = self.trans_keys.len();
        }
        if trans_pov() {
            // Like the game: nothing in improved transparency; otherwise columns within ±2 of the camera
            // examined at every frame, plus max(n / 8, 15) others in rotation; in an
            // examined column, only the sections to re-sort (`pov_resorts`) are, one write per
            // run of contiguous sections.
            let n = self.trans_keys.len();
            if !oit_on && n > 0 {
                let start = self.sort_cursor % n;
                let (picks, next) = pov_picks(&self.trans_keys, start, cam_col);
                let (sy0, how) = (min_y.div_euclid(16), trans_key());
                let scratch = &mut self.sort_scratch;
                let mut secs = Vec::new();
                for k in picks {
                    let Some(c) = self.columns.get_mut(&k) else { continue };
                    if c.trans_centers.is_empty() {
                        continue;
                    }
                    pov_resorts(&mut c.povs, &c.trans_starts, eye, k, sy0, moved, &mut secs);
                    let (e, st, base) = (eye_in_column(eye, k, min_y), &c.trans_starts, c.fluid_ranges[2].start);
                    let mut i = 0;
                    while i < secs.len() {
                        // Run of sections to re-sort separated only by empty sections.
                        let mut j = i;
                        while j + 1 < secs.len() && st[secs[j + 1]] == st[secs[j] + 1] {
                            j += 1;
                        }
                        let (lo, hi) = (st[secs[i]], st[secs[j] + 1]);
                        let order = &mut scratch.order;
                        order.clear();
                        order.extend(lo..hi);
                        for &s in &secs[i..=j] {
                            let r = (st[s] - lo) as usize..(st[s + 1] - lo) as usize;
                            sort_section(&c.trans_centers, &mut order[r], e, how, &mut scratch.sort);
                        }
                        order.iter_mut().for_each(|v| *v += base);
                        self.up.write(
                            &self.queue,
                            &self.trans.order,
                            u64::from(c.order_alloc.0 + lo) * ORDER_BYTES,
                            bytemuck::cast_slice(order),
                        );
                        crate::perf_log::trans_resorts(j + 1 - i);
                        i = j + 1;
                    }
                }
                self.sort_cursor = next;
            }
        } else {
            if moved {
                self.unsorted = self.trans_keys.len();
            }
            let keys = if self.unsorted > 0 { std::mem::take(&mut self.trans_keys) } else { Vec::new() };
            let mut budget = (keys.len() / 8).max(15);
            let n = keys.len();
            for step in 0..n {
                let k = keys[(self.sort_cursor + step) % n];
                let near = (k.0 - cam_col.0).abs() <= 1 && (k.1 - cam_col.1).abs() <= 1;
                let Some(c) = self.columns.get_mut(&k) else { continue };
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
                self.up.write(
                    &self.queue,
                    &self.trans.order,
                    u64::from(c.order_alloc.0) * ORDER_BYTES,
                    bytemuck::cast_slice(&order),
                );
            }
            if n > 0 {
                self.sort_cursor = (self.sort_cursor + (n / 8).max(15)) % n;
                let columns = &self.columns;
                self.unsorted =
                    keys.iter().filter(|k| columns.get(*k).is_some_and(|c| c.sorted_at != Some(cam_block))).count();
                self.trans_keys = keys;
            }
        }
    }
}

#[cfg(test)]
#[path = "translucent_sort_test.rs"]
mod tests;
