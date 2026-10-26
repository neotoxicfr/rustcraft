//! Sky geometry built on the CPU (`docs/specs/sky.md`): stars (the game's generator,
//! seed 10842), sunrise/sunset glow fan, cloud map and mesh.

use glam::{Mat3, Vec3};

/// The game's 48-bit linear congruential generator (identical to Java's linear congruential generator).
pub struct JavaRandom(u64);

const MASK: u64 = (1 << 48) - 1;

impl JavaRandom {
    pub fn new(seed: i64) -> Self {
        JavaRandom((seed as u64 ^ 0x5_DEEC_E66D) & MASK)
    }

    pub fn next(&mut self, bits: u32) -> i32 {
        self.0 = (self.0.wrapping_mul(0x5_DEEC_E66D).wrapping_add(0xB)) & MASK;
        (self.0 >> (48 - bits)) as i32
    }

    /// Integer in [0, n) (n > 0), same algorithm as the game.
    pub fn next_int(&mut self, n: i32) -> i32 {
        if (n as u32).is_power_of_two() {
            return ((i64::from(n) * i64::from(self.next(31))) >> 31) as i32;
        }
        loop {
            let bits = self.next(31);
            let val = bits % n;
            if bits.wrapping_sub(val).wrapping_add(n - 1) >= 0 {
                return val;
            }
        }
    }

    pub fn float(&mut self) -> f32 {
        self.next(24) as f32 * (1.0 / (1 << 24) as f32)
    }

    pub fn double(&mut self) -> f64 {
        let hi = i64::from(self.next(26)) << 27;
        (hi + i64::from(self.next(27))) as f64 * (1.0 / (1u64 << 53) as f64)
    }
}

/// The game's tabulated sine (65536 entries).
pub fn table_sin(x: f32) -> f32 {
    let i = ((f64::from(x) * 10_430.378_350_470_453) as i64 & 65_535) as f64;
    (i / 10_430.378_350_470_453).sin() as f32
}

pub fn table_cos(x: f32) -> f32 {
    let i = ((f64::from(x) * 10_430.378_350_470_453 + 16_384.0) as i64 & 65_535) as f64;
    (i / 10_430.378_350_470_453).sin() as f32
}

/// Stars: 6 vertices (two triangles) per accepted star, at distance 100 (§ 4.2).
pub fn stars() -> Vec<[f32; 3]> {
    let mut r = JavaRandom::new(10_842);
    let mut out = Vec::new();
    for _ in 0..1500 {
        let x = r.float() * 2.0 - 1.0;
        let y = r.float() * 2.0 - 1.0;
        let z = r.float() * 2.0 - 1.0;
        let t = 0.15 + r.float() * 0.1;
        let l = x * x + y * y + z * z;
        if l <= 0.010_000_001 || l >= 1.0 {
            continue;
        }
        let k = (1.0 / l.sqrt()) * 100.0;
        let c = Vec3::new(x * k, y * k, z * k);
        let zeta = (r.double() * f64::from(std::f32::consts::PI) * 2.0) as f32;
        let d = -c.normalize();
        let g = Vec3::Y.cross(d).normalize();
        let h = d.cross(g);
        let rot = Mat3::from_cols(g, h, d) * Mat3::from_rotation_z(-zeta);
        let v = [Vec3::new(t, -t, 0.0), Vec3::new(t, t, 0.0), Vec3::new(-t, t, 0.0), Vec3::new(-t, -t, 0.0)]
            .map(|p| (rot * p + c).to_array());
        out.extend([v[0], v[1], v[2], v[2], v[3], v[0]]);
    }
    out
}

/// Glow fan (§ 2.1) as triangles: (position, alpha), 16 triangles.
pub fn sunrise_fan() -> Vec<[f32; 4]> {
    let ring: Vec<[f32; 4]> = (0..=16)
        .map(|i| {
            let th = i as f32 * std::f32::consts::TAU / 16.0;
            [120.0 * table_sin(th), 120.0 * table_cos(th), -40.0 * table_cos(th), 0.0]
        })
        .collect();
    let center = [0.0, 100.0, 0.0, 1.0];
    ring.windows(2).flat_map(|w| [center, w[0], w[1]]).collect()
}

/// Cloud map (§ 5.1): per cell, full (bit 7) and empty neighbors north, east, south,
/// west (bits 0 to 3).
pub struct CloudMap {
    pub size: usize,
    pub cells: Vec<u8>,
}

impl CloudMap {
    pub fn from_rgba(width: usize, height: usize, rgba: &[u8]) -> CloudMap {
        let full = |x: usize, y: usize| rgba[(y * width + x) * 4 + 3] >= 10;
        let mut cells = vec![0u8; width * height];
        for y in 0..height {
            for x in 0..width {
                if !full(x, y) {
                    continue;
                }
                // Wrap-around: the game wraps x with the image height (no effect if square).
                let wrap = |v: isize, n: usize| v.rem_euclid(n as isize) as usize;
                let (xi, yi) = (x as isize, y as isize);
                let mut c = 0x80;
                c |= u8::from(!full(x, wrap(yi - 1, height)));
                c |= u8::from(!full(wrap(xi + 1, height), y)) << 1;
                c |= u8::from(!full(x, wrap(yi + 1, height))) << 2;
                c |= u8::from(!full(wrap(xi - 1, height), y)) << 3;
                cells[y * width + x] = c;
            }
        }
        CloudMap { size: width, cells }
    }

    fn at(&self, x: i32, z: i32) -> u8 {
        let n = self.size as i32;
        self.cells[(z.rem_euclid(n) * n + x.rem_euclid(n)) as usize]
    }
}

/// Camera position relative to the cloud layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudSide {
    Above,
    Below,
    Inside,
}

/// Faces of the unit cube (§ 5.7), order down, up, north, south, west, east.
const CLOUD_FACES: [[[f32; 3]; 4]; 6] = [
    [[1.0, 0.0, 0.0], [1.0, 0.0, 1.0], [0.0, 0.0, 1.0], [0.0, 0.0, 0.0]],
    [[0.0, 1.0, 0.0], [0.0, 1.0, 1.0], [1.0, 1.0, 1.0], [1.0, 1.0, 0.0]],
    [[0.0, 0.0, 0.0], [0.0, 1.0, 0.0], [1.0, 1.0, 0.0], [1.0, 0.0, 0.0]],
    [[1.0, 0.0, 1.0], [1.0, 1.0, 1.0], [0.0, 1.0, 1.0], [0.0, 0.0, 1.0]],
    [[0.0, 0.0, 1.0], [0.0, 1.0, 1.0], [0.0, 1.0, 0.0], [0.0, 0.0, 0.0]],
    [[1.0, 0.0, 0.0], [1.0, 1.0, 0.0], [1.0, 1.0, 1.0], [1.0, 0.0, 1.0]],
];
const CLOUD_SHADE: [f32; 6] = [0.7, 1.0, 0.8, 0.8, 0.9, 0.9];

/// Cloud mesh (§ 5.5–5.7), from the closest to the farthest: vertices (position
/// relative to the mesh origin, shading factor), two triangles per face. "Fancy": cells
/// in volume; `flat` ("fast"): only the bottom face of each cell, at the top
/// tint (1.0), drawn without face culling.
pub fn cloud_mesh(map: &CloudMap, cx: i32, cz: i32, radius: i32, side: CloudSide, flat: bool) -> Vec<[f32; 4]> {
    let mut out = Vec::new();
    let mut face = |i: i32, k: i32, f: usize, inside: bool| {
        let shade = if flat { CLOUD_SHADE[1] } else { CLOUD_SHADE[f] };
        let q = CLOUD_FACES[f];
        let order = if inside { [3, 2, 1, 0] } else { [0, 1, 2, 3] };
        let v = order.map(|o| {
            let p = q[o];
            [p[0] * 12.0 + 12.0 * i as f32, p[1] * 4.0, p[2] * 12.0 + 12.0 * k as f32, shade]
        });
        out.extend([v[0], v[1], v[2], v[2], v[3], v[0]]);
    };
    let mut cell = |i: i32, k: i32| {
        let c = map.at(cx + i, cz + k);
        if c & 0x80 == 0 {
            return;
        }
        if flat {
            face(i, k, 0, false);
            return;
        }
        if side != CloudSide::Below {
            face(i, k, 1, false);
        }
        if side != CloudSide::Above {
            face(i, k, 0, false);
        }
        if c & 1 != 0 && k > 0 {
            face(i, k, 2, false);
        }
        if c & 4 != 0 && k < 0 {
            face(i, k, 3, false);
        }
        if c & 8 != 0 && i > 0 {
            face(i, k, 4, false);
        }
        if c & 2 != 0 && i < 0 {
            face(i, k, 5, false);
        }
        if i.abs() <= 1 && k.abs() <= 1 {
            for f in 0..6 {
                face(i, k, f, true);
            }
        }
    };
    for r in 0..=2 * radius {
        for i in -r..=r {
            let k = r - i.abs();
            if k > radius || i * i + k * k > radius * radius {
                continue;
            }
            if k != 0 {
                cell(i, -k);
            }
            cell(i, k);
        }
    }
    out
}

/// Cells of the disk of radius `radius` in the draw order of [`cloud_mesh`] (rings of
/// increasing Manhattan distance; within a ring, i increasing, −k before +k).
pub fn cloud_spiral(radius: i32) -> Vec<[i16; 2]> {
    let mut out = Vec::new();
    disc_cells(radius, |i, k| out.push([i as i16, k as i16]));
    out
}

/// Number of cells of the disk of radius `radius`.
pub fn disc_len(radius: i32) -> usize {
    let mut n = 0;
    disc_cells(radius, |_, _| n += 1);
    n
}

/// Original sweep of the disk (same loop as [`cloud_mesh`]).
fn disc_cells(radius: i32, mut f: impl FnMut(i32, i32)) {
    for r in 0..=2 * radius {
        for i in -r..=r {
            let k = r - i.abs();
            if k > radius || i * i + k * k > radius * radius {
                continue;
            }
            if k != 0 {
                f(i, -k);
            }
            f(i, k);
        }
    }
}

/// Walk over the cells: sweep of the disk of this radius (`RUSTCRAFT_OPT_CLOUDSPIRAL=0`) or order
/// precomputed by [`cloud_spiral`].
#[derive(Clone, Copy)]
pub enum CloudCells<'a> {
    Scan(i32),
    Spiral(&'a [[i16; 2]]),
}

/// Face code: direction 0 to 5 (order of `CLOUD_FACES`), inner face (corners reversed),
/// top tint ("fast" mode).
pub const FACE_INSIDE: u32 = 1 << 3;
pub const FACE_TOP: u32 = 1 << 4;
/// Largest packable |i| or |k| (10 signed bits): the radius of the compact formats is
/// bounded there (unreachable: `cloudRange` ≤ 128 chunks gives a radius of 171, see the test
/// `cloud_range_fits_the_packed_radius`).
pub const FACE_LIMIT: i32 = 511;

/// Radius of the cloud disk (in cells of 12 blocks) for a range in chunks.
pub fn cloud_radius(range_chunks: i32) -> i32 {
    ((range_chunks * 16) as f32 / 12.0).ceil() as i32
}

/// Faces of each full cell, in the order of [`cloud_mesh`]: `emit(i, k, code)`.
fn cloud_codes(
    map: &CloudMap,
    cx: i32,
    cz: i32,
    cells: CloudCells<'_>,
    side: CloudSide,
    flat: bool,
    mut emit: impl FnMut(i32, i32, u32),
) {
    let mut cell = |i: i32, k: i32| {
        let c = map.at(cx + i, cz + k);
        if c & 0x80 == 0 {
            return;
        }
        if flat {
            emit(i, k, FACE_TOP);
            return;
        }
        if side != CloudSide::Below {
            emit(i, k, 1);
        }
        if side != CloudSide::Above {
            emit(i, k, 0);
        }
        if c & 1 != 0 && k > 0 {
            emit(i, k, 2);
        }
        if c & 4 != 0 && k < 0 {
            emit(i, k, 3);
        }
        if c & 8 != 0 && i > 0 {
            emit(i, k, 4);
        }
        if c & 2 != 0 && i < 0 {
            emit(i, k, 5);
        }
        if i.abs() <= 1 && k.abs() <= 1 {
            for f in 0..6 {
                emit(i, k, f | FACE_INSIDE);
            }
        }
    };
    match cells {
        CloudCells::Scan(r) => disc_cells(r, cell),
        CloudCells::Spiral(s) => {
            for &[i, k] in s {
                cell(i32::from(i), i32::from(k));
            }
        }
    }
}

/// One face per word (`RUSTCRAFT_OPT_CLOUDINST=face`): i on 10 signed bits (bits 0-9), k (10-19),
/// face code (20-24). The shader derives the 6 vertices of its two triangles from it.
pub fn pack_face(i: i32, k: i32, code: u32) -> u32 {
    (i as u32 & 0x3ff) | (k as u32 & 0x3ff) << 10 | code << 20
}

/// Cloud faces as [`pack_face`] words, into `out` (cleared), same order as [`cloud_mesh`].
pub fn cloud_faces(
    map: &CloudMap,
    cx: i32,
    cz: i32,
    cells: CloudCells<'_>,
    side: CloudSide,
    flat: bool,
    out: &mut Vec<u32>,
) {
    out.clear();
    cloud_codes(map, cx, cz, cells, side, flat, |i, k, code| out.push(pack_face(i, k, code)));
}

/// Code of an empty slot of a cell instance.
#[cfg(test)]
pub const CELL_EMPTY: u32 = 0xff;
/// Faces at most per cell instance (24 vertices; the "fast" mode has one, 6 vertices).
pub const CELL_FACES: usize = 4;

/// Cell instances (`RUSTCRAFT_OPT_CLOUDINST=cell`): (i, k) as two signed integers of
/// 16 bits, then up to 4 one-byte face codes (remaining slots empty); a cell with more
/// than 4 faces (inner faces of the center) takes several instances, faces in order.
pub fn cloud_cell_instances(
    map: &CloudMap,
    cx: i32,
    cz: i32,
    cells: CloudCells<'_>,
    side: CloudSide,
    flat: bool,
    out: &mut Vec<[u32; 2]>,
) {
    out.clear();
    let mut last = None;
    let mut slot = CELL_FACES;
    cloud_codes(map, cx, cz, cells, side, flat, |i, k, code| {
        if last != Some((i, k)) || slot == CELL_FACES {
            out.push([u32::from(i as u16) | u32::from(k as u16) << 16, u32::MAX]);
            last = Some((i, k));
            slot = 0;
        }
        let w = &mut out.last_mut().expect("open instance")[1];
        *w = (*w & !(0xff << (8 * slot))) | code << (8 * slot);
        slot += 1;
    });
}

/// Face or instance builder ([`cloud_faces`], [`cloud_cell_instances`]).
pub type CloudBuild<T> = fn(&CloudMap, i32, i32, CloudCells<'_>, CloudSide, bool, &mut Vec<T>);

/// Cells at least per slice of the parallel build.
const PAR_MIN: usize = 2048;

/// Parallel build (`RUSTCRAFT_OPT_CLOUDPAR`): the spiral cut into slices
/// built on the common pool then laid end to end, hence the same order and the same content as
/// `build` over the whole spiral (a cell is never split between two slices). `parts`
/// keeps a reserve per slice from one call to the next: no new buffer once the
/// radius is reached.
pub fn cloud_par<T: Copy + Send>(
    build: CloudBuild<T>,
    map: &CloudMap,
    (cx, cz): (i32, i32),
    spiral: &[[i16; 2]],
    side: CloudSide,
    flat: bool,
    (out, parts): (&mut Vec<T>, &mut Vec<Vec<T>>),
) {
    use rayon::prelude::*;
    let len = spiral.len().div_ceil(rayon::current_num_threads() * 4).max(PAR_MIN);
    if len >= spiral.len() {
        build(map, cx, cz, CloudCells::Spiral(spiral), side, flat, out);
        return;
    }
    let n = spiral.len().div_ceil(len);
    if parts.len() < n {
        parts.resize_with(n, Vec::new);
    }
    parts[..n]
        .par_iter_mut()
        .zip(spiral.par_chunks(len))
        .for_each(|(v, part)| build(map, cx, cz, CloudCells::Spiral(part), side, flat, v));
    out.clear();
    out.reserve(parts[..n].iter().map(Vec::len).sum());
    for p in &parts[..n] {
        out.extend_from_slice(p);
    }
}

/// Vertex `v` (0 to 5, two triangles 0-1-2 and 2-3-0) of face `code` of cell (i, k):
/// same computation as the shader (`cloud_corner` of sky.wgsl) and as [`cloud_mesh`].
#[cfg(test)]
pub fn face_vertex(i: i32, k: i32, code: u32, v: usize) -> [f32; 4] {
    let dir = (code & 7) as usize;
    let mut q = [0, 1, 2, 2, 3, 0][v];
    if code & FACE_INSIDE != 0 {
        q = 3 - q;
    }
    let p = CLOUD_FACES[dir][q];
    let shade = if code & FACE_TOP != 0 { CLOUD_SHADE[1] } else { CLOUD_SHADE[dir] };
    [p[0] * 12.0 + 12.0 * i as f32, p[1] * 4.0, p[2] * 12.0 + 12.0 * k as f32, shade]
}

/// Vertices described by [`pack_face`] words (shader emulation, proofs and bench).
#[cfg(test)]
pub fn expand_faces(faces: &[u32]) -> Vec<[f32; 4]> {
    let sext = |x: u32| ((x << 22) as i32) >> 22;
    faces.iter().flat_map(|&w| (0..6).map(move |v| face_vertex(sext(w), sext(w >> 10), (w >> 20) & 31, v))).collect()
}

/// Vertices described by cell instances (empty slots = degenerates removed).
#[cfg(test)]
pub fn expand_cells(cells: &[[u32; 2]]) -> Vec<[f32; 4]> {
    let mut out = Vec::new();
    for &[ik, codes] in cells {
        let (i, k) = (i32::from(ik as u16 as i16), i32::from((ik >> 16) as u16 as i16));
        for slot in 0..CELL_FACES {
            let code = (codes >> (8 * slot)) & 0xff;
            if code != CELL_EMPTY {
                out.extend((0..6).map(|v| face_vertex(i, k, code, v)));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stars_match_spec_vectors() {
        let s = stars();
        assert_eq!(s.len(), 780 * 6);
        let first = s[0];
        let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 2e-3);
        assert!(close(first, [-53.2247, 70.0560, 47.5324]), "{first:?}");
        // Center of the last star: middle of vertices 0 and 2.
        let last = &s[s.len() - 6..];
        let c: [f32; 3] = std::array::from_fn(|a| (last[0][a] + last[2][a]) / 2.0);
        assert!(close(c, [-19.1850, 92.1324, -33.8165]), "{c:?}");
    }

    #[test]
    fn flat_clouds_one_bottom_face_per_cell_with_top_shade() {
        // 4 × 4 map: two neighboring full cells.
        let mut rgba = vec![0u8; 4 * 4 * 4];
        for x in [1usize, 2] {
            rgba[(4 + x) * 4 + 3] = 255;
        }
        let map = CloudMap::from_rgba(4, 4, &rgba);
        // Radius 1: the central cell and its 4 neighbors; around (0, 0), none is full;
        // around (1, 1), (1, 1) and (2, 1) are.
        let flat = cloud_mesh(&map, 0, 0, 1, CloudSide::Below, true);
        let fancy = cloud_mesh(&map, 1, 1, 1, CloudSide::Below, false);
        let flat_hit = cloud_mesh(&map, 1, 1, 1, CloudSide::Below, true);
        assert!(flat.is_empty());
        // Two full cells reached: one face (6 vertices) each, all at y = 0, tint 1.
        assert_eq!(flat_hit.len(), 2 * 6);
        assert!(flat_hit.iter().all(|v| v[1] == 0.0 && v[3] == 1.0));
        // The detailed mesh keeps its volumes (inner faces near the center, tint 0.7 underneath).
        assert!(fancy.len() > flat_hit.len());
        assert!(fancy.iter().any(|v| v[3] == 0.7));
    }

    /// Test map 64 × 64 (fixed seed), about half full, varied neighbors.
    fn test_map() -> CloudMap {
        let mut s = 0x2545_f491_4f6c_dd1du64;
        let mut rgba = vec![0u8; 64 * 64 * 4];
        for px in rgba.chunks_mut(4) {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            px[3] = if s % 100 < 55 { 255 } else { 0 };
        }
        CloudMap::from_rgba(64, 64, &rgba)
    }

    fn fnv(v: &[[f32; 4]]) -> u64 {
        v.iter().flatten().fold(0xcbf2_9ce4_8422_2325u64, |h, x| {
            x.to_bits().to_le_bytes().iter().fold(h, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3))
        })
    }

    const SIDES: [CloudSide; 3] = [CloudSide::Above, CloudSide::Inside, CloudSide::Below];

    /// Frozen old mesh: stored fingerprint (failure if the order, the faces or the tints change).
    #[test]
    fn cloud_mesh_reference_fingerprint() {
        let map = test_map();
        let got: Vec<u64> =
            SIDES.iter().flat_map(|&s| [false, true].map(|flat| fnv(&cloud_mesh(&map, 7, -5, 20, s, flat)))).collect();
        assert_eq!(got, REFERENCE, "{got:#x?}");
    }
    const REFERENCE: [u64; 6] = [
        0x7a45_683c_63bb_0508,
        0xf723_b6a2_e292_5937,
        0xdaf4_094f_a071_b0fe,
        0xf723_b6a2_e292_5937,
        0xc74d_c4a9_91de_0f90,
        0xf723_b6a2_e292_5937,
    ];

    /// Faces and cell instances, unrolled like the shader: exactly the vertices of the
    /// old mesh, in the same order (degenerates removed), above, inside, below and
    /// in "fast" mode, by sweep and by precomputed spiral.
    #[test]
    fn packed_clouds_unroll_to_the_mesh() {
        let map = test_map();
        let (mut faces, mut cells) = (Vec::new(), Vec::new());
        let (mut face_parts, mut cell_parts) = (Vec::new(), Vec::new());
        for radius in [0, 1, 2, 5, 20, 40] {
            let spiral = cloud_spiral(radius);
            for (cx, cz) in [(0, 0), (7, -5), (-63, 64), (31, 2)] {
                for side in SIDES {
                    for flat in [false, true] {
                        let mesh = cloud_mesh(&map, cx, cz, radius, side, flat);
                        for order in [CloudCells::Scan(radius), CloudCells::Spiral(&spiral)] {
                            cloud_faces(&map, cx, cz, order, side, flat, &mut faces);
                            assert_eq!(faces.len() * 6, mesh.len());
                            assert!(expand_faces(&faces) == mesh, "faces r{radius} ({cx},{cz}) {side:?} {flat}");
                            cloud_cell_instances(&map, cx, cz, order, side, flat, &mut cells);
                            assert!(expand_cells(&cells) == mesh, "cells r{radius} ({cx},{cz}) {side:?} {flat}");
                            if flat {
                                assert!(cells.iter().all(|c| c[1] >> 8 == 0xff_ffff), "one face per flat cell");
                            }
                        }
                        // Parallel build (slices of the spiral from radius 26).
                        cloud_par(cloud_faces, &map, (cx, cz), &spiral, side, flat, (&mut faces, &mut face_parts));
                        assert!(expand_faces(&faces) == mesh, "parallel faces r{radius} ({cx},{cz}) {side:?} {flat}");
                        let cell_out = (&mut cells, &mut cell_parts);
                        cloud_par(cloud_cell_instances, &map, (cx, cz), &spiral, side, flat, cell_out);
                        assert!(
                            expand_cells(&cells) == mesh,
                            "parallel cells r{radius} ({cx},{cz}) {side:?} {flat}"
                        );
                    }
                }
            }
        }
        // Sabotage: spiral reversed -> vertices in another order, detected.
        let mut rev = cloud_spiral(20);
        rev.reverse();
        let mesh = cloud_mesh(&map, 7, -5, 20, CloudSide::Inside, false);
        cloud_faces(&map, 7, -5, CloudCells::Spiral(&rev), CloudSide::Inside, false, &mut faces);
        assert!(expand_faces(&faces) != mesh);
    }

    /// Micro-bench (loaded machine possible):
    /// `cargo test --release -p rc-app cloud_bench -- --ignored --nocapture`.
    /// 256 × 256 map in 8 × 8 patches (≈ 45% full, irregular edges), range 128 and
    /// 64 chunks, detailed (below, inside) and fast; build only (the buffer to send).
    #[test]
    #[ignore]
    fn cloud_bench() {
        use std::time::Instant;
        let mut s = 0x1234_5678_9abc_def1u64;
        let mut rnd = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        let tiles: Vec<bool> = (0..32 * 32).map(|_| rnd() % 100 < 45).collect();
        let mut rgba = vec![0u8; 256 * 256 * 4];
        for z in 0..256 {
            for x in 0..256 {
                let on = tiles[(z / 8) * 32 + x / 8] ^ (rnd() % 100 < 8);
                rgba[(z * 256 + x) * 4 + 3] = if on { 255 } else { 0 };
            }
        }
        let map = CloudMap::from_rgba(256, 256, &rgba);
        let time = |f: &mut dyn FnMut()| {
            f();
            let mut best = f64::MAX;
            let mut sum = 0.0;
            for _ in 0..20 {
                let t = Instant::now();
                f();
                let e = t.elapsed().as_secs_f64() * 1e3;
                best = best.min(e);
                sum += e;
            }
            (best, sum / 20.0)
        };
        let threads = std::thread::available_parallelism().map_or(1, |n| n.get());
        for range in [128, 64] {
            let radius = ((range * 16) as f32 / 12.0).ceil() as i32;
            let spiral = cloud_spiral(radius);
            for (side, flat) in [(CloudSide::Below, false), (CloudSide::Inside, false), (CloudSide::Below, true)] {
                let (cx, cz) = (100, 37);
                let mut n_mesh = 0;
                let old =
                    time(&mut || n_mesh = std::hint::black_box(cloud_mesh(&map, cx, cz, radius, side, flat)).len());
                let mut faces = Vec::new();
                let mut cells = Vec::new();
                let fs = time(&mut || cloud_faces(&map, cx, cz, CloudCells::Scan(radius), side, flat, &mut faces));
                let fp = time(&mut || cloud_faces(&map, cx, cz, CloudCells::Spiral(&spiral), side, flat, &mut faces));
                let cs =
                    time(&mut || cloud_cell_instances(&map, cx, cz, CloudCells::Scan(radius), side, flat, &mut cells));
                let cp = time(&mut || {
                    cloud_cell_instances(&map, cx, cz, CloudCells::Spiral(&spiral), side, flat, &mut cells)
                });
                let per = if flat { 6 } else { 24 };
                println!(
                    "range {range} r{radius} {side:?} flat={flat} | ms best/mean: mesh {:.3}/{:.3}, faces sweep {:.3}/{:.3}, faces spiral {:.3}/{:.3}, cells sweep {:.3}/{:.3}, cells spiral {:.3}/{:.3}",
                    old.0, old.1, fs.0, fs.1, fp.0, fp.1, cs.0, cs.1, cp.0, cp.1
                );
                println!(
                    "    bytes: mesh {} | faces {} | cells {}; vertices processed: {} | {} | {}",
                    n_mesh * 16,
                    faces.len() * 4,
                    cells.len() * 8,
                    n_mesh,
                    faces.len() * 6,
                    cells.len() * per
                );
                // Multi-core: spiral cut into slices built in parallel then laid
                // end to end (same order). 1 thread then powers of 2.
                let mut t = 1;
                let mut line = String::from("    faces in parallel (threads: best ms):");
                while t <= threads {
                    let pool = rayon::ThreadPoolBuilder::new().num_threads(t).build().expect("pool");
                    let (mut par, mut parts) = (Vec::new(), Vec::new());
                    let r = time(&mut || {
                        pool.install(|| {
                            cloud_par(cloud_faces, &map, (cx, cz), &spiral, side, flat, (&mut par, &mut parts))
                        })
                    });
                    assert_eq!(par, faces_of(&map, cx, cz, &spiral, side, flat));
                    line += &format!(" {t}: {:.3}", r.0);
                    t *= 2;
                }
                println!("{line}");
                // Busy pool as in flight: 8 tasks of 1 ms per thread already queued (chunk meshes)
                // when the main thread, outside the pool, starts the parallel build.
                if range == 128 && side == CloudSide::Below && !flat {
                    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("pool");
                    let (mut par, mut parts) = (Vec::new(), Vec::new());
                    let mut waits = Vec::new();
                    for _ in 0..10 {
                        for _ in 0..threads * 8 {
                            pool.spawn(|| {
                                let t = Instant::now();
                                while t.elapsed().as_micros() < 1000 {
                                    std::hint::spin_loop();
                                }
                            });
                        }
                        let t = Instant::now();
                        pool.install(|| {
                            cloud_par(cloud_faces, &map, (cx, cz), &spiral, side, flat, (&mut par, &mut parts))
                        });
                        waits.push(t.elapsed().as_secs_f64() * 1e3);
                        // Drain before the next pass.
                        std::thread::sleep(std::time::Duration::from_millis(20));
                    }
                    waits.sort_by(f64::total_cmp);
                    println!(
                        "    faces in parallel, busy pool ({threads} threads, {} tasks of 1 ms queued): min {:.3} / median {:.3} / max {:.3} ms",
                        threads * 8,
                        waits[0],
                        waits[5],
                        waits[9]
                    );
                }
            }
        }
    }

    /// Parallel build: the second pass at the same radius reuses the slice buffers
    /// and the final buffer (same addresses, no new reserve).
    #[test]
    fn cloud_par_reuses_slice_buffers() {
        let map = test_map();
        let spiral = cloud_spiral(60);
        let pool = rayon::ThreadPoolBuilder::new().num_threads(4).build().expect("pool");
        let (mut out, mut parts) = (Vec::new(), Vec::new());
        let run = |out: &mut Vec<u32>, parts: &mut Vec<Vec<u32>>| {
            pool.install(|| cloud_par(cloud_faces, &map, (3, 4), &spiral, CloudSide::Inside, false, (out, parts)));
        };
        run(&mut out, &mut parts);
        assert!(parts.len() > 1, "spiral cut into slices");
        let want = faces_of(&map, 3, 4, &spiral, CloudSide::Inside, false);
        assert_eq!(out, want);
        let addrs = |out: &Vec<u32>, parts: &Vec<Vec<u32>>| {
            (out.as_ptr(), parts.as_ptr(), parts.iter().map(|p| p.as_ptr()).collect::<Vec<_>>())
        };
        let before = addrs(&out, &parts);
        run(&mut out, &mut parts);
        assert_eq!(out, want);
        assert!(addrs(&out, &parts) == before, "buffers redone on the second pass");
    }

    fn faces_of(map: &CloudMap, cx: i32, cz: i32, spiral: &[[i16; 2]], side: CloudSide, flat: bool) -> Vec<u32> {
        let mut v = Vec::new();
        cloud_faces(map, cx, cz, CloudCells::Spiral(spiral), side, flat, &mut v);
        v
    }

    #[test]
    fn java_random_first_values() {
        // Known values of a Java linear congruential generator with seed 42.
        let mut r = JavaRandom::new(42);
        assert_eq!(r.next(32), -1_170_105_035);
        assert_eq!(r.next(32), 234_785_527);
    }
}
