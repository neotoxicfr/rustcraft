//! Terrain draw lists: columns visible in the frame, runs of sections in the field of view,
//! batches and indirect commands. No GPU object here: the construction is compared frame by frame
//! with the old one (`RUSTCRAFT_OPT_VISORDER=0`) in the tests.
//!
//! Switches (read once): `RUSTCRAFT_OPT_VISORDER` (cached column order, array ranks,
//! merged translucent sections, cached radius), `RUSTCRAFT_OPT_REACHGRID` (reached
//! sections as a bit grid built by the walk task), `RUSTCRAFT_OPT_ARGSKIP`
//! (indirect commands uploaded only over the changed range). `=0`: old path.

use std::ops::Range;
use std::sync::OnceLock;

use glam::{Vec3, Vec4};
use hashbrown::HashMap;

use super::{BUCKETS, Batch, ColumnGpu, DrawArgs, TRANS_KEY, aabb_inside, aabb_visible, facing};

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

/// Outputs (buffers kept from one frame to the next) and cache of the column order.
#[derive(Default)]
pub(super) struct Lists {
    pub runs: Vec<Range<usize>>,
    pub batches: Vec<Batch>,
    pub args: Vec<DrawArgs>,
    /// Candidate columns from farthest to nearest: distance², key, range in
    /// `reached`; valid for (walk generation, origin, column version).
    order_key: Option<OrderKey>,
    order: Vec<(i64, (i32, i32), u32, u32)>,
    /// Key of the previous frame (the order is kept only on its second frame).
    seen_key: Option<OrderKey>,
    rank: HashMap<(i32, i32), u32>,
}

/// Key of the column order: walk generation, origin, column version.
type OrderKey = (u64, [i32; 2], u64);

/// Visible column of the frame: data, coordinates, range of its runs.
type Vis<'a> = (&'a ColumnGpu, (i32, i32), Range<usize>);

/// Candidate column of the old path: data, distance², coordinates, range of its runs.
type Cand<'a> = (&'a ColumnGpu, i64, (i32, i32), Range<usize>);

/// Builds the runs, batches and commands of the frame; returns (columns, faces) drawn. `ordered`:
/// called once the column order is fixed (end of the "visibility" measurement).
pub(super) fn build(inp: &ListIn<'_>, out: &mut Lists, new: bool, ordered: impl FnOnce()) -> (usize, u64) {
    out.runs.clear();
    let key = (inp.reached_gen, inp.origin, inp.columns_rev);
    // Order of all the candidates kept only when the key repeats (rotation in place,
    // slow walk): an origin that changes every frame (fast flight) would redo it for nothing.
    let cached = new && (out.order_key == Some(key) || out.seen_key.replace(key) == Some(key));
    let visible = if cached {
        refresh_order(inp, out, key);
        let mut visible: Vec<Vis<'_>> = Vec::new();
        for &(_, k, lo, hi) in &out.order {
            let first = out.runs.len();
            column_runs(inp, k, &inp.reached[lo as usize..hi as usize], &mut out.runs);
            if out.runs.len() > first {
                // Present: the order keeps only the loaded columns.
                visible.push((&inp.columns[&k], k, first..out.runs.len()));
            }
        }
        visible
    } else {
        sorted_visible(inp, &mut out.runs)
    };
    ordered();
    let near = if new {
        // Rank of each region = order of first appearance starting from the nearest;
        // columns grouped by rank, from nearest to farthest within each group
        // (same result as the old path's stable sort by rank).
        out.rank.clear();
        let mut ranks = vec![0u32; visible.len()];
        for i in (0..visible.len()).rev() {
            let n = out.rank.len() as u32;
            ranks[i] = *out.rank.entry(visible[i].0.region).or_insert(n);
        }
        let mut at = vec![0usize; out.rank.len() + 1];
        ranks.iter().for_each(|&r| at[r as usize + 1] += 1);
        for r in 1..at.len() {
            at[r] += at[r - 1];
        }
        let mut sorted = vec![0usize; visible.len()];
        for i in (0..visible.len()).rev() {
            let r = ranks[i] as usize;
            sorted[at[r]] = i;
            at[r] += 1;
        }
        sorted
    } else {
        let mut n: Vec<usize> = (0..visible.len()).rev().collect();
        let mut rank: HashMap<(i32, i32), usize> = HashMap::new();
        for &i in &n {
            let r = rank.len();
            rank.entry(visible[i].0.region).or_insert(r);
        }
        n.sort_by_key(|&i| rank[&visible[i].0.region]);
        n
    };
    let facings: Vec<[(usize, usize); BUCKETS]> = visible.iter().map(|v| facing(inp.eye, v.1, inp.min_y)).collect();
    let far: Vec<usize> = (0..visible.len()).collect();
    out.batches.clear();
    out.args.clear();
    let mut faces = 0u64;
    let (mut pieces, mut src): (Vec<Range<usize>>, Vec<usize>) = (Vec::new(), Vec::new());
    let ey = inp.eye[1] - f64::from(inp.min_y);
    for layer in 0..3 {
        let order = if layer == 2 { &far } else { &near };
        for fluid in [false, true] {
            for &i in order {
                let (c, _, ref span) = visible[i];
                let runs = &out.runs[span.clone()];
                let range = if fluid { &c.fluid_ranges } else { &c.ranges };
                if range[layer].is_empty() {
                    continue;
                }
                // Translucent: instances = indices of the sorted draw order.
                let starts: &[u32] = match (fluid, layer) {
                    (true, 2) => &c.order_starts,
                    (true, _) => &c.fluid_starts[layer],
                    _ => &c.starts[layer],
                };
                // Translucent: section by section, from farthest to nearest.
                pieces.clear();
                if layer == 2 {
                    if new {
                        far_to_near(runs, ey, &mut src, &mut pieces);
                    } else {
                        pieces.extend(runs.iter().flat_map(|r| r.clone()).map(|sy| sy..sy + 1));
                        pieces.sort_by(|a, b| section_dist(b.start, ey).total_cmp(&section_dist(a.start, ey)));
                    }
                } else {
                    pieces.extend(runs.iter().filter(|r| !r.is_empty()).cloned());
                }
                // Block faces stored by visible direction: a group entirely facing away
                // is not submitted (the GPU would have rejected it after the vertices).
                let (groups, s1) = if fluid { (1, starts.len()) } else { (BUCKETS, starts.len() / BUCKETS) };
                let allowed = if fluid { [(0, usize::MAX); BUCKETS] } else { facings[i] };
                for run in &pieces {
                    for (g, &(lo, hi)) in allowed.iter().enumerate().take(groups) {
                        let (a, e) = (run.start.max(lo), run.end.min(hi));
                        if a >= e {
                            continue;
                        }
                        let r = starts[g * s1 + a]..starts[g * s1 + e];
                        if r.is_empty() {
                            continue;
                        }
                        faces += u64::from(r.end - r.start);
                        let home = if fluid && layer == 2 { TRANS_KEY } else { c.region };
                        match out.batches.last_mut() {
                            Some(b) if b.0 == layer && b.1 == fluid && b.2 == home => {
                                // Range contiguous with the previous command of the batch: a single
                                // command (same primitive order).
                                if let Some(last) = out.args.last_mut()
                                    && !inp.split_draws
                                    && last.first_instance + last.instance_count == r.start
                                {
                                    last.instance_count += r.end - r.start;
                                    continue;
                                }
                                b.4 += 1;
                            }
                            _ => out.batches.push((layer, fluid, home, out.args.len() as u32, 1)),
                        }
                        out.args.push(DrawArgs {
                            // Block faces as a strip: 4 vertices.
                            vertex_count: if fluid { 6 } else { inp.face_vertices },
                            instance_count: r.end - r.start,
                            first_vertex: 0,
                            first_instance: r.start,
                        });
                    }
                }
            }
        }
    }
    (visible.len(), faces)
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
fn sorted_visible<'a>(inp: &ListIn<'a>, runs: &mut Vec<Range<usize>>) -> Vec<Vis<'a>> {
    let mut cand: Vec<Cand<'_>> = Vec::new();
    let reached = inp.reached;
    let mut i = 0;
    while i < reached.len() {
        let (x, _, z) = reached[i];
        let lo = i;
        while i < reached.len() && reached[i].0 == x && reached[i].2 == z {
            i += 1;
        }
        let first = runs.len();
        column_runs(inp, (x, z), &reached[lo..i], runs);
        match inp.columns.get(&(x, z)) {
            Some(c) if runs.len() > first => cand.push((c, dist2(inp.origin, (x, z)), (x, z), first..runs.len())),
            _ => runs.truncate(first),
        }
    }
    cand.sort_unstable_by_key(|&(_, d, k, _)| (std::cmp::Reverse(d), k));
    cand.into_iter().map(|(c, _, k, s)| (c, k, s)).collect()
}

/// Order of the candidate columns redone if the walk, the origin or the columns changed.
fn refresh_order(inp: &ListIn<'_>, out: &mut Lists, key: OrderKey) {
    if out.order_key == Some(key) {
        return;
    }
    out.order.clear();
    let reached = inp.reached;
    let mut i = 0;
    while i < reached.len() {
        let (x, _, z) = reached[i];
        let lo = i;
        while i < reached.len() && reached[i].0 == x && reached[i].2 == z {
            i += 1;
        }
        if inp.columns.contains_key(&(x, z)) {
            out.order.push((dist2(inp.origin, (x, z)), (x, z), lo as u32, i as u32));
        }
    }
    out.order.sort_unstable_by_key(|&(d, k, _, _)| (std::cmp::Reverse(d), k));
    out.order_key = Some(key);
}

/// Runs of sections of column `k` (reached sections `secs`, increasing height) whose
/// box (one block of margin: a model can overflow) is in the field of view, added to `runs`.
fn column_runs(inp: &ListIn<'_>, k: (i32, i32), secs: &[(i32, i32, i32)], runs: &mut Vec<Range<usize>>) {
    let (x, z) = k;
    let planes = inp.planes;
    // Box of the whole column: out of the field, all its sections are; entirely inside the
    // field, all are in it, without a per-section test.
    let col_min = Vec3::new((x * 16 - inp.origin[0]) as f32, inp.min_y as f32, (z * 16 - inp.origin[1]) as f32) - 1.0;
    let col_max = col_min + Vec3::new(18.0, (inp.sections * 16) as f32 + 2.0, 18.0);
    let column_in = if aabb_visible(planes, col_min, col_max) { Some(aabb_inside(planes, col_min, col_max)) } else { None };
    let Some(all) = column_in else { return };
    let mut open: Option<(usize, usize)> = None;
    for &(_, sy, _) in secs {
        if !all {
            let min = Vec3::new(
                (x * 16 - inp.origin[0]) as f32,
                (inp.min_y + sy * 16) as f32,
                (z * 16 - inp.origin[1]) as f32,
            ) - 1.0;
            if !aabb_visible(planes, min, min + Vec3::splat(18.0)) {
                continue;
            }
        }
        let sy = sy as usize;
        open = match open {
            Some((a, end)) if end >= sy => Some((a, end.max(sy + 1))),
            Some((a, end)) => {
                runs.push(a..end);
                Some((sy, sy + 1))
            }
            None => Some((sy, sy + 1)),
        };
    }
    if let Some((a, end)) = open {
        runs.push(a..end);
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
