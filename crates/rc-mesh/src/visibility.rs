//! Section visibility (`docs/specs/visibility.md`): communication between the faces of a
//! section, computed at meshing, then walk from the camera through the open passages.
//! Directions in the order down, up, north, south, west, east.

use hashbrown::HashMap;
use std::collections::VecDeque;

/// 6 × 6 matrix: bit `a * 6 + b` = faces a and b communicate.
pub type Vis = u64;
/// All faces communicate (empty or nearly empty section).
pub const VIS_ALL: Vis = (1 << 36) - 1;
/// None (full section, or not yet meshed).
pub const VIS_NONE: Vis = 0;

const STEP: [(i32, i32, i32); 6] = [(0, -1, 0), (0, 1, 0), (0, 0, -1), (0, 0, 1), (-1, 0, 0), (1, 0, 0)];
const OPPOSITE: [usize; 6] = [1, 0, 3, 2, 5, 4];

fn sees(v: Vis, a: usize, b: usize) -> bool {
    v >> (a * 6 + b) & 1 == 1
}

/// Communication of the faces of a section of states `values` (index y << 8 | z << 4 | x), `solid`
/// giving the solid render states.
pub fn section_vis(values: &[u32], solid: &[bool]) -> Vis {
    section_vis_bits(&solid_bits(values, solid))
}

/// [`section_vis`] from the mask of the full cells (dilation of 64-bit masks,
/// 2.1 times faster than the cell-by-cell fill, identical result).
pub fn section_vis_bits(bits: &[u64; 64]) -> Vis {
    section_vis_dilate(bits)
}

/// Mask of the full cells of a section (bit i = cell i).
pub fn solid_bits(values: &[u32], solid: &[bool]) -> [u64; 64] {
    let mut bits = [0u64; 64];
    for (i, &s) in values.iter().enumerate() {
        if solid.get(s as usize).copied().unwrap_or(false) {
            bits[i >> 6] |= 1 << (i & 63);
        }
    }
    bits
}

/// Breadth-first fill cell by cell, like the game: reference for [`section_vis_bits`].
pub fn section_vis_fill(bits: &[u64; 64]) -> Vis {
    let mut filled = *bits;
    let opaque: u32 = bits.iter().map(|w| w.count_ones()).sum();
    if opaque < 256 {
        return VIS_ALL;
    }
    if opaque == 4096 {
        return VIS_NONE;
    }
    let mut vis = 0;
    let mut queue = Vec::with_capacity(4096);
    let edge = |c: usize| c == 0 || c == 15;
    for x in 0..16usize {
        for y in 0..16usize {
            for z in 0..16usize {
                if !(edge(x) || edge(y) || edge(z)) {
                    continue;
                }
                let start = (y << 8) | (z << 4) | x;
                if filled[start >> 6] >> (start & 63) & 1 == 1 {
                    continue;
                }
                // Breadth-first fill; section faces touched as bits.
                filled[start >> 6] |= 1 << (start & 63);
                queue.clear();
                queue.push(start);
                let mut faces = 0u8;
                let mut head = 0;
                while head < queue.len() {
                    let i = queue[head];
                    head += 1;
                    let (cx, cy, cz) = (i & 15, i >> 8, (i >> 4) & 15);
                    faces |= match cx {
                        0 => 1 << 4,
                        15 => 1 << 5,
                        _ => 0,
                    } | match cy {
                        0 => 1,
                        15 => 1 << 1,
                        _ => 0,
                    } | match cz {
                        0 => 1 << 2,
                        15 => 1 << 3,
                        _ => 0,
                    };
                    let mut push = |n: usize| {
                        if filled[n >> 6] >> (n & 63) & 1 == 0 {
                            filled[n >> 6] |= 1 << (n & 63);
                            queue.push(n);
                        }
                    };
                    if cy > 0 {
                        push(i - 256);
                    }
                    if cy < 15 {
                        push(i + 256);
                    }
                    if cz > 0 {
                        push(i - 16);
                    }
                    if cz < 15 {
                        push(i + 16);
                    }
                    if cx > 0 {
                        push(i - 1);
                    }
                    if cx < 15 {
                        push(i + 1);
                    }
                }
                for a in 0..6 {
                    for b in 0..6 {
                        if faces >> a & 1 == 1 && faces >> b & 1 == 1 {
                            vis |= 1 << (a * 6 + b);
                        }
                    }
                }
            }
        }
    }
    vis
}

/// Edge cells of each section face (direction order), as 64-bit masks.
const EDGE: [[u64; 64]; 6] = {
    let mut edge = [[0u64; 64]; 6];
    let mut w = 0;
    while w < 64 {
        let y = w >> 2;
        let mut r = 0;
        while r < 4 {
            let z = (w & 3) * 4 + r; // 4 z rows per word
            let row = 0xffffu64 << (r * 16);
            if y == 0 {
                edge[0][w] |= row;
            }
            if y == 15 {
                edge[1][w] |= row;
            }
            if z == 0 {
                edge[2][w] |= row;
            }
            if z == 15 {
                edge[3][w] |= row;
            }
            edge[4][w] |= 1 << (r * 16);
            edge[5][w] |= 0x8000 << (r * 16);
            r += 1;
        }
        w += 1;
    }
    edge
};

/// Like [`section_vis_fill`], by dilation of 64-bit masks (4 x rows per word:
/// x ± 1 = shift of 1 outside row edges, z ± 1 = shift of 16 with carry between words,
/// y ± 1 = word ± 4).
pub fn section_vis_dilate(bits: &[u64; 64]) -> Vis {
    let opaque: u32 = bits.iter().map(|w| w.count_ones()).sum();
    if opaque < 256 {
        return VIS_ALL;
    }
    if opaque == 4096 {
        return VIS_NONE;
    }
    const NOT_X0: u64 = !0x0001_0001_0001_0001; // x = 0 of each row
    const NOT_X15: u64 = !0x8000_8000_8000_8000; // x = 15
    let edge = &EDGE;
    let mut free = [0u64; 64];
    for w in 0..64 {
        free[w] = !bits[w];
    }
    let mut vis = 0;
    loop {
        // Seed: first remaining free cell on an edge.
        let any_edge = (0..64).find_map(|w| {
            let e = (edge[0][w] | edge[1][w] | edge[2][w] | edge[3][w] | edge[4][w] | edge[5][w]) & free[w];
            (e != 0).then(|| (w, e.trailing_zeros()))
        });
        let Some((w0, b0)) = any_edge else { break };
        let mut zone = [0u64; 64];
        zone[w0] = 1 << b0;
        loop {
            let mut next = [0u64; 64];
            let mut grew = false;
            for w in 0..64 {
                let z = zone[w];
                let mut g = z | ((z << 1) & NOT_X0) | ((z >> 1) & NOT_X15) | (z << 16) | (z >> 16);
                if w & 3 != 0 {
                    g |= zone[w - 1] >> 48;
                }
                if w & 3 != 3 {
                    g |= zone[w + 1] << 48;
                }
                if w >= 4 {
                    g |= zone[w - 4];
                }
                if w < 60 {
                    g |= zone[w + 4];
                }
                g &= free[w];
                grew |= g != z;
                next[w] = g;
            }
            zone = next;
            if !grew {
                break;
            }
        }
        let mut faces = 0u8;
        for (d, e) in edge.iter().enumerate() {
            if (0..64).any(|w| zone[w] & e[w] != 0) {
                faces |= 1 << d;
            }
        }
        for w in 0..64 {
            free[w] &= !zone[w];
        }
        for a in 0..6 {
            for b in 0..6 {
                if faces >> a & 1 == 1 && faces >> b & 1 == 1 {
                    vis |= 1 << (a * 6 + b);
                }
            }
        }
    }
    vis
}

/// Section reached by the walk: arrival directions and directions followed from the camera.
struct Node {
    pos: (i32, i32, i32),
    sources: u8,
    dirs: u8,
    /// Steps since the start (0: camera section or edge layer), for F3.
    step: u16,
}

/// Marking of the reached sections: node number per position.
trait Marks {
    fn get(&self, p: (i32, i32, i32)) -> Option<usize>;
    fn set(&mut self, p: (i32, i32, i32), node: usize);
}

impl Marks for HashMap<(i32, i32, i32), usize> {
    fn get(&self, p: (i32, i32, i32)) -> Option<usize> {
        HashMap::get(self, &p).copied()
    }
    fn set(&mut self, p: (i32, i32, i32), node: usize) {
        self.insert(p, node);
    }
}

/// Dense grid around the camera (side 2r + 1 columns, `sections` high).
struct Dense<'b> {
    cells: &'b mut [u32],
    base: (i32, i32),
    side: i32,
    sections: i32,
}

impl Dense<'_> {
    fn slot(&self, p: (i32, i32, i32)) -> Option<usize> {
        let (x, z) = (p.0 - self.base.0, p.2 - self.base.1);
        ((0..self.side).contains(&x) && (0..self.side).contains(&z) && (0..self.sections).contains(&p.1))
            .then(|| ((z * self.side + x) * self.sections + p.1) as usize)
    }
}

impl Marks for Dense<'_> {
    fn get(&self, p: (i32, i32, i32)) -> Option<usize> {
        self.slot(p).and_then(|i| (self.cells[i] != u32::MAX).then_some(self.cells[i] as usize))
    }
    fn set(&mut self, p: (i32, i32, i32), node: usize) {
        if let Some(i) = self.slot(p) {
            self.cells[i] = node as u32;
        }
    }
}

/// Walk with buffers reused from one frame to the next (dense grid, no hashing).
#[derive(Default)]
pub struct Graph {
    cells: Vec<u32>,
    nodes: Vec<Node>,
    queue: VecDeque<usize>,
    out: Vec<(i32, i32, i32)>,
}

impl Graph {
    /// Like [`visible_sections`]; `radius` bounds the grid (columns beyond are ignored).
    pub fn run<'a>(
        &mut self,
        column: impl Fn(i32, i32) -> Option<&'a [Vis]>,
        eye: [f64; 3],
        min_y: i32,
        sections: i32,
        radius: i32,
        smart: bool,
    ) -> &[(i32, i32, i32)] {
        let cam = [eye[0].floor() as i32 >> 4, eye[2].floor() as i32 >> 4];
        let side = 2 * radius + 1;
        let base = (cam[0] - radius, cam[1] - radius);
        // Columns ready in the grid: their communications, copied once.
        let mut lists: Vec<Option<&'a [Vis]>> = Vec::with_capacity((side * side) as usize);
        for z in 0..side {
            for x in 0..side {
                lists.push(column(base.0 + x, base.1 + z));
            }
        }
        self.cells.clear();
        self.cells.resize((side * side * sections.max(0)) as usize, u32::MAX);
        let lookup = |x: i32, z: i32| {
            let (lx, lz) = (x - base.0, z - base.1);
            if (0..side).contains(&lx) && (0..side).contains(&lz) { lists[(lz * side + lx) as usize] } else { None }
        };
        let mut marks = Dense { cells: &mut self.cells, base, side, sections };
        self.nodes.clear();
        self.queue.clear();
        search(lookup, eye, min_y, sections, radius, smart, &mut marks, &mut self.nodes, &mut self.queue);
        self.out.clear();
        self.out.extend(self.nodes.iter().map(|n| n.pos));
        &self.out
    }

    /// Nodes of the last walk: (section, arrival directions, steps since the start),
    /// for the path debug traces.
    pub fn nodes(&self) -> impl Iterator<Item = ((i32, i32, i32), u8, u16)> + '_ {
        self.nodes.iter().map(|n| (n.pos, n.sources, n.step))
    }
}

/// Faces `a` and `b` (order down, up, north, south, west, east) communicate.
pub fn faces_see(v: Vis, a: usize, b: usize) -> bool {
    sees(v, a, b)
}

/// Sections visible from the camera. `column(cx, cz)` gives the communication of each section
/// of a ready column (section index from the bottom of the world), `None` if absent.
/// `eye`: camera position in blocks; `min_y`: bottom of the world. Reference version
/// (hash table), same walk as [`Graph::run`].
pub fn visible_sections<'a>(
    column: impl Fn(i32, i32) -> Option<&'a [Vis]>,
    eye: [f64; 3],
    min_y: i32,
    sections: i32,
    view_distance: i32,
    smart: bool,
) -> Vec<(i32, i32, i32)> {
    let mut index: HashMap<(i32, i32, i32), usize> = HashMap::new();
    let (mut nodes, mut queue) = (Vec::new(), VecDeque::new());
    search(column, eye, min_y, sections, view_distance, smart, &mut index, &mut nodes, &mut queue);
    nodes.into_iter().map(|n| n.pos).collect()
}

#[allow(clippy::too_many_arguments)]
fn search<'a, M: Marks>(
    column: impl Fn(i32, i32) -> Option<&'a [Vis]>,
    eye: [f64; 3],
    min_y: i32,
    sections: i32,
    view_distance: i32,
    smart: bool,
    index: &mut M,
    nodes: &mut Vec<Node>,
    queue: &mut VecDeque<usize>,
) {
    let cam = [eye[0].floor() as i32 >> 4, (eye[1].floor() as i32 - min_y) >> 4, eye[2].floor() as i32 >> 4];
    let exists = |x: i32, y: i32, z: i32| {
        (0..sections).contains(&y)
            && (x - cam[0]).abs() <= view_distance
            && (z - cam[2]).abs() <= view_distance
            && column(x, z).is_some()
    };
    let vis_of = |x: i32, y: i32, z: i32| column(x, z).and_then(|c| c.get(y as usize).copied()).unwrap_or(VIS_NONE);
    if exists(cam[0], cam[1], cam[2]) {
        index.set((cam[0], cam[1], cam[2]), 0);
        nodes.push(Node { pos: (cam[0], cam[1], cam[2]), sources: 0, dirs: 0, step: 0 });
        queue.push_back(0);
    } else if !(0..sections).contains(&cam[1]) {
        // Camera above (or below) the world: start from the whole edge layer.
        let below = cam[1] < 0;
        let y = if below { 0 } else { sections - 1 };
        let source = if below { 1 } else { 0 };
        let mut start = Vec::new();
        for dx in -view_distance..=view_distance {
            for dz in -view_distance..=view_distance {
                let (x, z) = (cam[0] + dx, cam[2] + dz);
                if !exists(x, y, z) {
                    continue;
                }
                let mut dirs = 1u8 << source;
                dirs |= match dx.signum() {
                    1 => 1 << 5,
                    -1 => 1 << 4,
                    _ => 0,
                };
                dirs |= match dz.signum() {
                    1 => 1 << 3,
                    -1 => 1 << 2,
                    _ => 0,
                };
                let c = [x * 16 + 8, min_y + y * 16 + 8, z * 16 + 8].map(f64::from);
                let d = (0..3).map(|i| (c[i] - eye[i].floor()).powi(2)).sum::<f64>();
                start.push((d, Node { pos: (x, y, z), sources: 1 << source, dirs, step: 0 }));
            }
        }
        start.sort_by(|a, b| a.0.total_cmp(&b.0));
        for (_, n) in start {
            index.set(n.pos, nodes.len());
            queue.push_back(nodes.len());
            nodes.push(n);
        }
    }
    let center = [cam[0] * 16 + 8, min_y + cam[1] * 16 + 8, cam[2] * 16 + 8];
    let diagonal = (3f64.sqrt() * 16.0).ceil();
    let world_y = (f64::from(min_y), f64::from(min_y + sections * 16));
    while let Some(ni) = queue.pop_front() {
        let (pos, sources, dirs, depth) = (nodes[ni].pos, nodes[ni].sources, nodes[ni].dirs, nodes[ni].step);
        let far = (pos.0 - cam[0]).abs() > 3 || (pos.1 - cam[1]).abs() > 3 || (pos.2 - cam[2]).abs() > 3;
        let vis = vis_of(pos.0, pos.1, pos.2);
        for (d, &(sx, sy, sz)) in STEP.iter().enumerate() {
            let next = (pos.0 + sx, pos.1 + sy, pos.2 + sz);
            if !exists(next.0, next.1, next.2) {
                continue;
            }
            if smart && dirs >> OPPOSITE[d] & 1 == 1 {
                continue;
            }
            if smart && sources != 0 && !(0..6).any(|s| sources >> s & 1 == 1 && sees(vis, OPPOSITE[s], d)) {
                continue;
            }
            if smart && far {
                // Line from the section corner to the camera: each point must fall in an
                // already reached section.
                let origin = [pos.0 * 16, min_y + pos.1 * 16, pos.2 * 16];
                let axis = d / 2; // 0: y, 1: z, 2: x
                let corner = |a: usize| {
                    let on_axis = (axis == 0 && a == 1) || (axis == 1 && a == 2) || (axis == 2 && a == 0);
                    let beyond = if on_axis { center[a] > origin[a] } else { center[a] < origin[a] };
                    f64::from(origin[a] + if beyond { 16 } else { 0 })
                };
                let mut p = [corner(0), corner(1), corner(2)];
                let delta = [eye[0] - p[0], eye[1] - p[1], eye[2] - p[2]];
                let len = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
                let step = delta.map(|v| v / len * diagonal);
                let mut ok = true;
                while (0..3).map(|i| (p[i] - eye[i]).powi(2)).sum::<f64>() > 3600.0 {
                    for i in 0..3 {
                        p[i] += step[i];
                    }
                    if p[1] > world_y.1 || p[1] < world_y.0 {
                        break;
                    }
                    let s = (p[0].floor() as i32 >> 4, (p[1].floor() as i32 - min_y) >> 4, p[2].floor() as i32 >> 4);
                    if index.get(s).is_none() {
                        ok = false;
                        break;
                    }
                }
                if !ok {
                    continue;
                }
            }
            if let Some(e) = index.get(next) {
                nodes[e].sources |= 1 << d;
            } else {
                index.set(next, nodes.len());
                queue.push_back(nodes.len());
                nodes.push(Node { pos: next, sources: 1 << d, dirs: dirs | 1 << d, step: depth.saturating_add(1) });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_vis_rules() {
        let solid = [false, true];
        assert_eq!(section_vis(&[0; 4096], &solid), VIS_ALL);
        assert_eq!(section_vis(&[1; 4096], &solid), VIS_NONE);
        // Full floor at y = 8: top and bottom no longer communicate, the sides do.
        let mut v = [0u32; 4096];
        v[8 << 8..9 << 8].fill(1);
        let s = section_vis(&v, &solid);
        assert!(!sees(s, 0, 1) && !sees(s, 1, 0));
        assert!(sees(s, 0, 2) && sees(s, 1, 2) && sees(s, 4, 5));
    }

    #[test]
    fn floor_hides_what_is_below_when_looking_from_above() {
        // Columns of 4 sections; section 1 full everywhere: from section 3, nothing below it.
        let col = [VIS_ALL, VIS_NONE, VIS_ALL, VIS_ALL];
        let seen = visible_sections(|_, _| Some(&col[..]), [8.0, 56.0, 8.0], 0, 4, 2, true);
        assert!(seen.iter().all(|s| s.1 >= 1), "{seen:?}");
        assert!(seen.contains(&(0, 1, 0)), "the full section itself is reached");
        let all = visible_sections(|_, _| Some(&col[..]), [8.0, 56.0, 8.0], 0, 4, 2, false);
        assert_eq!(all.len(), 5 * 5 * 4);
    }

    /// Nodes for F3: step = walk distance from the camera (0 at the start, no arrival),
    /// arrivals accumulated for a node reached by several neighbours.
    #[test]
    fn graph_nodes_carry_step_and_all_sources() {
        let col = [VIS_ALL; 4];
        let mut g = Graph::default();
        g.run(|_, _| Some(&col[..]), [8.0, 24.0, 8.0], 0, 4, 2, false);
        let node = |p| g.nodes().find(|n| n.0 == p).unwrap();
        assert_eq!(node((0, 1, 0)).2, 0);
        assert_eq!(node((1, 1, 0)).2, 1);
        assert_eq!(node((1, 2, 0)).2, 2);
        assert_eq!(node((2, 3, 2)).2, 6);
        // (1, 2, 0): reached from below (up, bit 1) and from the west (east, bit 5).
        assert_eq!(node((1, 2, 0)).1 & (1 << 1 | 1 << 5), 1 << 1 | 1 << 5);
        assert!(faces_see(VIS_ALL, 2, 5) && !faces_see(VIS_NONE, 2, 5));
    }

    #[test]
    fn dense_graph_matches_reference_on_random_worlds() {
        // Simple reproducible pseudo-random generator.
        let mut seed = 0x2545_f491_4f6c_dd1du64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let mut g = Graph::default();
        for round in 0..40 {
            let r = 6;
            let cols: Vec<Vec<Vis>> = (0..(2 * r + 1) * (2 * r + 1))
                .map(|_| (0..8).map(|_| if next() % 3 == 0 { next() & VIS_ALL } else { VIS_ALL }).collect())
                .collect();
            let present: Vec<bool> = (0..cols.len()).map(|_| next() % 7 != 0).collect();
            let col = |x: i32, z: i32| {
                let (lx, lz) = (x + r, z + r);
                if !(0..2 * r + 1).contains(&lx) || !(0..2 * r + 1).contains(&lz) {
                    return None;
                }
                let i = (lz * (2 * r + 1) + lx) as usize;
                present[i].then(|| cols[i].as_slice())
            };
            let eye =
                [(next() % 96) as f64 - 48.0 + 0.3, (next() % 160) as f64 - 16.0 + 0.7, (next() % 96) as f64 - 48.0];
            let smart = round % 5 != 0;
            let a = visible_sections(col, eye, 0, 8, r, smart);
            let b = g.run(col, eye, 0, 8, r, smart).to_vec();
            assert_eq!(a, b, "round {round}");
        }
    }
}
