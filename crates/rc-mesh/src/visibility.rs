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

/// [`section_vis`] from the mask of the full cells: zones grown over 64-bit masks, the same result
/// as the cell-by-cell fill (about 12 times faster on the capture with the default paths).
pub fn section_vis_bits(bits: &[u64; 64]) -> Vis {
    section_vis_with(bits, DilationPaths::from_switches())
}

/// Ways of growing the zones of free cells in [`section_vis_with`]; all give the same result.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DilationPaths {
    /// Each step fills the whole free runs of the x rows at once by a carry
    /// (`RUSTCRAFT_OPT_SECTION_VIS_ROW_CARRY`), instead of one cell along x.
    pub row_carry: bool,
    /// Only the words next to a word that just grew are stepped again, from a queue
    /// (`RUSTCRAFT_OPT_SECTION_VIS_WORD_QUEUE`), instead of sweeping the 64 words until none grows.
    pub word_queue: bool,
    /// A zone touching the six faces ends the computation: every face then sees every other one
    /// (`RUSTCRAFT_OPT_SECTION_VIS_STOP_AT_SIX_FACES`).
    pub stop_at_six_faces: bool,
}

impl DilationPaths {
    /// The dilation as it was before (one cell per step, every zone grown to the end).
    pub const OLD: Self = Self { row_carry: false, word_queue: false, stop_at_six_faces: false };
    pub const ALL: Self = Self { row_carry: true, word_queue: true, stop_at_six_faces: true };

    pub fn from_switches() -> Self {
        Self {
            // Off: on the capture it saves 7 % alone but costs 21 % on top of the word queue.
            row_carry: rc_data::opt_switch!("RUSTCRAFT_OPT_SECTION_VIS_ROW_CARRY", default_off),
            word_queue: rc_data::opt_switch!("RUSTCRAFT_OPT_SECTION_VIS_WORD_QUEUE"),
            stop_at_six_faces: rc_data::opt_switch!("RUSTCRAFT_OPT_SECTION_VIS_STOP_AT_SIX_FACES"),
        }
    }

    /// Every combination of the three paths, the old one first.
    pub fn every_combination() -> [Self; 8] {
        std::array::from_fn(|i| Self { row_carry: i & 1 != 0, word_queue: i & 2 != 0, stop_at_six_faces: i & 4 != 0 })
    }
}

/// [`section_vis_bits`] through the given dilation paths.
pub fn section_vis_with(bits: &[u64; 64], paths: DilationPaths) -> Vis {
    if paths.row_carry || paths.word_queue {
        section_vis_by_zones(bits, paths)
    } else {
        section_vis_dilate(bits, paths.stop_at_six_faces)
    }
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
fn section_vis_dilate(bits: &[u64; 64], stop_at_six_faces: bool) -> Vis {
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
            if stop_at_six_faces && faces_touched(&zone) == ALL_FACES {
                return VIS_ALL;
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
        vis |= FACE_PAIRS[usize::from(faces)];
    }
    vis
}

/// The six faces, one bit per direction.
const ALL_FACES: u8 = 0b11_1111;
/// x = 0 (first bit) and x = 15 (last bit) of the four 16-bit rows of a word.
const ROW_FIRST: u64 = 0x0001_0001_0001_0001;
const ROW_LAST: u64 = 0x8000_8000_8000_8000;

/// Communications given by a zone touching the faces `i` (one bit per direction): each of them
/// with each of them.
const FACE_PAIRS: [Vis; 64] = {
    let mut table = [0; 64];
    let mut faces = 0;
    while faces < 64 {
        let mut a = 0;
        while a < 6 {
            let mut b = 0;
            while b < 6 {
                if faces >> a & 1 == 1 && faces >> b & 1 == 1 {
                    table[faces] |= 1 << (a * 6 + b);
                }
                b += 1;
            }
            a += 1;
        }
        faces += 1;
    }
    table
};

/// Section faces (direction order) reached by `zone`: y = word layer, z = row in the word
/// (4 per word), x = bit in the row.
fn faces_touched(zone: &[u64; 64]) -> u8 {
    fn union<'a>(words: impl Iterator<Item = &'a u64>) -> u64 {
        words.fold(0, |acc, w| acc | w)
    }
    let any = union(zone.iter());
    let bottom = union(zone[..4].iter());
    let top = union(zone[60..].iter());
    let north = union(zone.iter().step_by(4)) & 0xffff;
    let south = union(zone.iter().skip(3).step_by(4)) >> 48;
    [bottom, top, north, south, any & ROW_FIRST, any & ROW_LAST]
        .iter()
        .enumerate()
        .fold(0, |faces, (d, &cells)| faces | u8::from(cells != 0) << d)
}

/// Sum of two words as four independent 16-bit rows: a carry leaving x = 15 of a row is dropped
/// instead of entering x = 0 of the next row.
fn add_per_row(a: u64, b: u64) -> u64 {
    ((a & !ROW_LAST) + (b & !ROW_LAST)) ^ ((a ^ b) & ROW_LAST)
}

/// Cells of `free` reached from `seed` (a subset of `free`) going towards x + inside each row.
/// Adding a seed bit to the run of free bits holding it carries through the rest of the run,
/// which flips exactly those bits (plus the blocked cell just past the run, masked off); a second
/// seed in the same run absorbs the carry, hence the seeds added back.
fn spread_towards_x_plus(seed: u64, free: u64) -> u64 {
    ((add_per_row(free, seed) ^ free) & free) | seed
}

/// Cells of `free` reached from `seed` (a subset of `free`) going towards x − inside each row:
/// doubling fill (1, 2, 4 then 8 cells), `open` holding the cells whose whole stretch towards
/// x + is free. x = 15 never takes from the next row's x = 0.
fn spread_towards_x_minus(seed: u64, free: u64) -> u64 {
    let (mut reached, mut open) = (seed, free & !ROW_LAST);
    for shift in [1, 2, 4] {
        reached |= open & (reached >> shift);
        open &= open >> shift;
    }
    reached | (open & (reached >> 8))
}

/// Whole free runs of each row holding a seed.
fn fill_rows(seed: u64, free: u64) -> u64 {
    spread_towards_x_plus(seed, free) | spread_towards_x_minus(seed, free)
}

/// Like [`section_vis_dilate`], through [`grow_zone`]: same zones, hence the same result, in
/// fewer or cheaper steps.
fn section_vis_by_zones(bits: &[u64; 64], paths: DilationPaths) -> Vis {
    let opaque: u32 = bits.iter().map(|w| w.count_ones()).sum();
    if opaque < 256 {
        return VIS_ALL;
    }
    if opaque == 4096 {
        return VIS_NONE;
    }
    let free: [u64; 64] = std::array::from_fn(|w| !bits[w]);
    let mut unvisited = free;
    let mut zone = [0u64; 64];
    let mut vis = 0;
    // Earlier words keep no unvisited edge cell: the next seed is searched from the last one on.
    let mut first_word = 0;
    while let Some((w0, b0)) = first_free_edge_cell(&unvisited, first_word) {
        first_word = w0;
        zone[w0] = 1 << b0;
        let Some((faces, mut touched)) = grow_zone(&mut zone, &free, w0, paths) else {
            return VIS_ALL;
        };
        // Only the words the zone reached are marked visited and cleared for the next zone.
        while touched != 0 {
            let w = touched.trailing_zeros() as usize;
            touched &= touched - 1;
            unvisited[w] &= !zone[w];
            zone[w] = 0;
        }
        vis |= FACE_PAIRS[usize::from(faces)];
    }
    vis
}

/// Cells on any section face, per word.
const ANY_EDGE: [u64; 64] = {
    let mut any = [0u64; 64];
    let mut w = 0;
    while w < 64 {
        any[w] = EDGE[0][w] | EDGE[1][w] | EDGE[2][w] | EDGE[3][w] | EDGE[4][w] | EDGE[5][w];
        w += 1;
    }
    any
};

/// First free cell on a section face from word `first_word` (lowest word, then lowest bit), as
/// (word, bit).
fn first_free_edge_cell(free: &[u64; 64], first_word: usize) -> Option<(usize, u32)> {
    (first_word..64).find_map(|w| {
        let e = ANY_EDGE[w] & free[w];
        (e != 0).then(|| (w, e.trailing_zeros()))
    })
}

/// Faces (direction order) touched by the cells `cells` of word `w`.
fn word_faces(w: usize, cells: u64) -> u8 {
    let y = w >> 2;
    u8::from(y == 0 && cells != 0)
        | u8::from(y == 15 && cells != 0) << 1
        | u8::from(w & 3 == 0 && cells & 0xffff != 0) << 2
        | u8::from(w & 3 == 3 && cells >> 48 != 0) << 3
        | u8::from(cells & ROW_FIRST != 0) << 4
        | u8::from(cells & ROW_LAST != 0) << 5
}

/// Words whose growth step reads word `w`: its neighbours in z (same layer) and in y.
const NEIGHBOUR_WORDS: [u64; 64] = {
    let mut near = [0u64; 64];
    let mut w = 0;
    while w < 64 {
        if w & 3 != 0 {
            near[w] |= 1 << (w - 1);
        }
        if w & 3 != 3 {
            near[w] |= 1 << (w + 1);
        }
        if w >= 4 {
            near[w] |= 1 << (w - 4);
        }
        if w < 60 {
            near[w] |= 1 << (w + 4);
        }
        w += 1;
    }
    near
};

/// One growth step of word `w`: its cells plus its neighbours' one cell further in z and y, then
/// spread along x (whole free runs with `row_carry`, else one cell), kept to the free cells.
fn grow_word(zone: &[u64; 64], free: &[u64; 64], w: usize, row_carry: bool) -> u64 {
    let z = zone[w];
    let mut reach = z | (z << 16) | (z >> 16);
    if w & 3 != 0 {
        reach |= zone[w - 1] >> 48;
    }
    if w & 3 != 3 {
        reach |= zone[w + 1] << 48;
    }
    if w >= 4 {
        reach |= zone[w - 4];
    }
    if w < 60 {
        reach |= zone[w + 4];
    }
    if row_carry {
        fill_rows(reach & free[w], free[w])
    } else {
        (reach | ((z << 1) & !ROW_FIRST) | ((z >> 1) & !ROW_LAST)) & free[w]
    }
}

/// Grows `zone` (a seed in word `seed_word`) through `free` until it stops growing, and returns
/// the faces it touches, gathered word by word as it grows, and the mask of the words it holds;
/// `None` when `stop_at_six_faces` cut it short on the six faces.
fn grow_zone(zone: &mut [u64; 64], free: &[u64; 64], seed_word: usize, paths: DilationPaths) -> Option<(u8, u64)> {
    let mut faces = word_faces(seed_word, zone[seed_word]);
    let mut touched = 1u64 << seed_word;
    if paths.word_queue {
        // Words to step again: only those next to a word that grew can grow in turn (the seed
        // word has just grown from nothing).
        let mut pending = 1u64 << seed_word | NEIGHBOUR_WORDS[seed_word];
        while pending != 0 {
            let w = pending.trailing_zeros() as usize;
            pending &= pending - 1;
            let before = zone[w];
            // The word's own rows feed each other (z inside the word, x): settle it first.
            loop {
                let grown = grow_word(zone, free, w, paths.row_carry);
                if grown == zone[w] {
                    break;
                }
                zone[w] = grown;
            }
            if zone[w] != before {
                faces |= word_faces(w, zone[w]);
                if paths.stop_at_six_faces && faces == ALL_FACES {
                    return None;
                }
                pending |= NEIGHBOUR_WORDS[w];
                touched |= 1 << w;
            }
        }
        return Some((faces, touched));
    }
    // Sweeps over the 64 words, alternately up and down, updated in place: a word grown in this
    // sweep passes its cells on at once.
    let mut upward = true;
    loop {
        let mut grew = false;
        for i in 0..64 {
            let w = if upward { i } else { 63 - i };
            let grown = grow_word(zone, free, w, paths.row_carry);
            if grown != zone[w] {
                zone[w] = grown;
                grew = true;
                faces |= word_faces(w, grown);
                touched |= 1 << w;
            }
        }
        if !grew {
            return Some((faces, touched));
        }
        if paths.stop_at_six_faces && faces == ALL_FACES {
            return None;
        }
        upward = !upward;
    }
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

/// Section corners whose sight ray was found clear during this walk, one bit per corner of the
/// dense grid (`WalkPaths::clear_ray_per_corner`).
struct ClearCorners<'b> {
    bits: &'b mut [u64],
    base: (i32, i32),
    side: i32,
    sections: i32,
}

impl ClearCorners<'_> {
    /// Number of corners of a grid of `side` × `side` columns of `sections` sections.
    fn count(side: i32, sections: i32) -> usize {
        ((side + 1) * (side + 1) * (sections + 1)) as usize
    }

    /// Bit of corner `c` (in section units: the low corner of section `c`).
    fn bit(&self, c: (i32, i32, i32)) -> Option<usize> {
        let (x, z) = (c.0 - self.base.0, c.2 - self.base.1);
        let corners = 0..=self.side;
        (corners.contains(&x) && corners.contains(&z) && (0..=self.sections).contains(&c.1))
            .then(|| ((z * (self.side + 1) + x) * (self.sections + 1) + c.1) as usize)
    }

    fn contains(&self, c: (i32, i32, i32)) -> bool {
        self.bit(c).is_some_and(|i| self.bits[i >> 6] >> (i & 63) & 1 == 1)
    }

    fn insert(&mut self, c: (i32, i32, i32)) {
        if let Some(i) = self.bit(c) {
            self.bits[i >> 6] |= 1 << (i & 63);
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
    clear_corners: Vec<u64>,
    paths: WalkPaths,
}

impl Graph {
    /// A walk through the given paths instead of the switches' (benches, equivalence tests).
    pub fn with_paths(paths: WalkPaths) -> Self {
        Self { paths, ..Self::default() }
    }

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
        let paths = self.paths;
        let clear_corners = paths.clear_ray_per_corner.then(|| {
            self.clear_corners.clear();
            self.clear_corners.resize(ClearCorners::count(side, sections.max(0)).div_ceil(64), 0);
            ClearCorners { bits: &mut self.clear_corners, base, side, sections }
        });
        let buffers = WalkBuffers { index: &mut marks, nodes: &mut self.nodes, queue: &mut self.queue, clear_corners };
        search(lookup, eye, min_y, sections, radius, smart, paths.ray_per_axis, buffers);
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
    let buffers = WalkBuffers { index: &mut index, nodes: &mut nodes, queue: &mut queue, clear_corners: None };
    search(column, eye, min_y, sections, view_distance, smart, WalkPaths::OLD.ray_per_axis, buffers);
    nodes.into_iter().map(|n| n.pos).collect()
}

/// Optional paths of the walk; all reach the same sections with the same arrivals. Both rest on
/// one fact: the index of the reached sections only grows during a walk, so a sight ray found
/// clear stays clear.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WalkPaths {
    /// One sight ray per node and axis (`RUSTCRAFT_OPT_VISIBILITY_RAY_PER_AXIS`). Both directions
    /// of an axis start their ray from the same corner, and between the two the index only gains
    /// the first direction's neighbour, and only if its ray was clear: the first verdict, clear or
    /// not, holds for the second.
    pub ray_per_axis: bool,
    /// A clear ray is remembered by its corner for the rest of the walk
    /// (`RUSTCRAFT_OPT_VISIBILITY_CLEAR_RAY_PER_CORNER`): neighbouring sections share their
    /// corners, and the ray from a corner is the same whatever section asks (2.5 rays per corner
    /// on the capture). Refused rays (4 %) are cast again, the index may have grown since.
    pub clear_ray_per_corner: bool,
}

impl WalkPaths {
    /// The walk as it was before: one ray per direction.
    pub const OLD: Self = Self { ray_per_axis: false, clear_ray_per_corner: false };

    pub fn from_switches() -> Self {
        Self {
            ray_per_axis: rc_data::opt_switch!("RUSTCRAFT_OPT_VISIBILITY_RAY_PER_AXIS"),
            clear_ray_per_corner: rc_data::opt_switch!("RUSTCRAFT_OPT_VISIBILITY_CLEAR_RAY_PER_CORNER"),
        }
    }

    /// Every combination of the two paths, the old one first.
    pub fn every_combination() -> [Self; 4] {
        std::array::from_fn(|i| Self { ray_per_axis: i & 1 != 0, clear_ray_per_corner: i & 2 != 0 })
    }
}

impl Default for WalkPaths {
    fn default() -> Self {
        Self::from_switches()
    }
}

/// Where a walk writes: the index of the reached sections, the nodes, the queue of nodes to
/// expand, and the clear corners when they are kept.
struct WalkBuffers<'b, M> {
    index: &'b mut M,
    nodes: &'b mut Vec<Node>,
    queue: &'b mut VecDeque<usize>,
    clear_corners: Option<ClearCorners<'b>>,
}

/// What the sight rays of the walk need besides the index of the reached sections.
struct SightRays {
    eye: [f64; 3],
    /// Centre of the camera section, in blocks.
    center: [i32; 3],
    min_y: i32,
    /// Bottom and top of the world, in blocks.
    world_y: (f64, f64),
    /// Distance between two points of a ray: the diagonal of a section, rounded up.
    diagonal: f64,
}

impl SightRays {
    /// Corner of section `pos` the ray for a step along `axis` (0 y, 1 z, 2 x) starts from, in
    /// section units (the low corner of the returned section position).
    fn corner(&self, pos: (i32, i32, i32), axis: usize) -> (i32, i32, i32) {
        let center = self.center;
        let origin = [pos.0 * 16, self.min_y + pos.1 * 16, pos.2 * 16];
        let beyond = |a: usize| {
            let on_axis = (axis == 0 && a == 1) || (axis == 1 && a == 2) || (axis == 2 && a == 0);
            i32::from(if on_axis { center[a] > origin[a] } else { center[a] < origin[a] })
        };
        (pos.0 + beyond(0), pos.1 + beyond(1), pos.2 + beyond(2))
    }

    /// Line from `corner` to the camera: each point, until 60 blocks from the eye or out of the
    /// world's height, must fall in an already reached section.
    fn reach_camera<M: Marks>(&self, index: &M, corner: (i32, i32, i32)) -> bool {
        let eye = self.eye;
        let mut p = [corner.0 * 16, self.min_y + corner.1 * 16, corner.2 * 16].map(f64::from);
        let delta = [eye[0] - p[0], eye[1] - p[1], eye[2] - p[2]];
        let len = (delta[0] * delta[0] + delta[1] * delta[1] + delta[2] * delta[2]).sqrt();
        let step = delta.map(|v| v / len * self.diagonal);
        while (0..3).map(|i| (p[i] - eye[i]).powi(2)).sum::<f64>() > 3600.0 {
            for i in 0..3 {
                p[i] += step[i];
            }
            if p[1] > self.world_y.1 || p[1] < self.world_y.0 {
                break;
            }
            let s = (p[0].floor() as i32 >> 4, (p[1].floor() as i32 - self.min_y) >> 4, p[2].floor() as i32 >> 4);
            if index.get(s).is_none() {
                return false;
            }
        }
        true
    }

    /// [`Self::reach_camera`] from `corner`, unless it is known clear already.
    fn reach_camera_once<M: Marks>(
        &self,
        index: &M,
        clear_corners: &mut Option<ClearCorners<'_>>,
        corner: (i32, i32, i32),
    ) -> bool {
        let Some(known) = clear_corners else { return self.reach_camera(index, corner) };
        if known.contains(corner) {
            return true;
        }
        let clear = self.reach_camera(index, corner);
        if clear {
            known.insert(corner);
        }
        clear
    }
}

#[allow(clippy::too_many_arguments)]
fn search<'a, M: Marks>(
    column: impl Fn(i32, i32) -> Option<&'a [Vis]>,
    eye: [f64; 3],
    min_y: i32,
    sections: i32,
    view_distance: i32,
    smart: bool,
    ray_per_axis: bool,
    buffers: WalkBuffers<'_, M>,
) {
    let WalkBuffers { index, nodes, queue, mut clear_corners } = buffers;
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
    let rays = SightRays {
        eye,
        center: [cam[0] * 16 + 8, min_y + cam[1] * 16 + 8, cam[2] * 16 + 8],
        min_y,
        world_y: (f64::from(min_y), f64::from(min_y + sections * 16)),
        diagonal: (3f64.sqrt() * 16.0).ceil(),
    };
    while let Some(ni) = queue.pop_front() {
        let (pos, sources, dirs, depth) = (nodes[ni].pos, nodes[ni].sources, nodes[ni].dirs, nodes[ni].step);
        let far = (pos.0 - cam[0]).abs() > 3 || (pos.1 - cam[1]).abs() > 3 || (pos.2 - cam[2]).abs() > 3;
        let vis = vis_of(pos.0, pos.1, pos.2);
        // Ray verdict per axis (y, z, x) for this node: see `WalkPaths::ray_per_axis`.
        let mut ray_verdicts: [Option<bool>; 3] = [None; 3];
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
                let axis = d / 2;
                let clear = match ray_verdicts[axis] {
                    Some(clear) if ray_per_axis => clear,
                    _ => rays.reach_camera_once(&*index, &mut clear_corners, rays.corner(pos, axis)),
                };
                ray_verdicts[axis] = Some(clear);
                if !clear {
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
        let mut graphs = WalkPaths::every_combination().map(Graph::with_paths);
        for round in 0..200 {
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
            for g in &mut graphs {
                let b = g.run(col, eye, 0, 8, r, smart).to_vec();
                assert_eq!(a, b, "round {round}, {:?}", g.paths);
            }
            // Same arrivals and steps too (the F3 path traces read them).
            for g in &graphs[1..] {
                assert!(graphs[0].nodes().eq(g.nodes()), "round {round}, {:?}: arrivals or steps differ", g.paths);
            }
        }
    }

    /// Random section: each word an AND (sparse) or an OR (dense) of `terms` random words.
    fn random_section(next: &mut impl FnMut() -> u64, dense: bool, terms: u32) -> [u64; 64] {
        std::array::from_fn(|_| (1..terms).fold(next(), |acc, _| if dense { acc | next() } else { acc & next() }))
    }

    #[test]
    fn dilation_paths_match_the_fill_on_random_sections() {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let paths = DilationPaths::every_combination();
        let mut partial = 0;
        for round in 0..1200 {
            // Solid share from 50 % to 97 %: one open zone, then more and more small pockets.
            let mut bits = random_section(&mut next, true, 1 + round % 5);
            if round % 7 == 3 {
                // Solid slab at a random height, holed now and then (zones on five faces or fewer).
                let y = (next() % 16) as usize;
                bits = random_section(&mut next, false, 2 + round % 3);
                bits[y * 4..y * 4 + 4].fill(!0);
                for _ in 0..next() % 3 {
                    let i = (y << 8) | (next() % 256) as usize;
                    bits[i >> 6] &= !(1 << (i & 63));
                }
            }
            if round % 7 == 0 {
                // Straight tunnels along x, z and y through solid stone (long runs of free cells).
                bits = [!0; 64];
                for _ in 0..4 {
                    let (a, b) = ((next() % 16) as usize, (next() % 16) as usize);
                    for c in 0..16 {
                        for i in [(a << 8) | (b << 4) | c, (a << 8) | (c << 4) | b, (c << 8) | (a << 4) | b] {
                            bits[i >> 6] &= !(1 << (i & 63));
                        }
                    }
                }
            }
            let fill = section_vis_fill(&bits);
            partial += usize::from(fill != VIS_ALL && fill != VIS_NONE);
            for p in paths {
                assert_eq!(section_vis_with(&bits, p), fill, "round {round}, {p:?}");
            }
        }
        assert!(partial > 300, "too few sections with partial communications: {partial}");
    }
}
