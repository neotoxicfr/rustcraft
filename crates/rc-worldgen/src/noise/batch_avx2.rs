//! Batches of improved Perlin points (doubled table), 8 points at a time in AVX2 registers.
//!
//! Lane by lane, the operations are those of the scalar point path (`Perlin::get_with` with
//! `perm512`): the same additions, subtractions and multiplications, in the same order, on the
//! same values, without fused multiply-add, hence the same bits. Only a group whose 24 coordinates
//! all lie where the wrap leaves them as they are takes the registers; any other group (far
//! coordinates, NaN) goes through the scalar path, one point at a time.

use std::arch::x86_64::*;

use super::{GRAD_F, HALF_ROUND_OFF, PERM_LEN, Paths, Perlin, Points};

/// Points of a group (lanes of a 256-bit register of f32).
const LANES: usize = 8;

/// 1.5·2^52, the shift of the floor of `rc_data::java_math`: added to `|v| < 2^51`, the sum rounds
/// `v` to the nearest integer.
const ROUNDING_SHIFT: f64 = 6_755_399_441_055_744.0;

/// Adds `amp × noise(point × freq)` to `out` for the whole groups of 8 points and returns how many
/// points it did (0 without AVX2): the caller computes the rest.
#[allow(unsafe_code)]
pub(super) fn add_points(n: &Perlin, pts: Points<'_>, freq: f64, amp: f32, out: &mut [f32], p: Paths) -> usize {
    // `Paths` is public: `p.batch_avx2` proves nothing, the detection is the only guard.
    if !rc_data::cpu::has_avx2() {
        return 0;
    }
    // SAFETY: AVX2 has just been detected on this processor.
    unsafe { add_groups(n, pts, freq, amp, out, p) }
}

/// Cell (as an integer of the table, masked) and position in the cell of 8 points on one axis.
#[derive(Clone, Copy)]
struct Axis {
    cell: __m256i,
    offset_in_cell: __m256,
}

/// The gradient components of the 16 hash values (`GRAD_F`), 8 per register.
struct GradientTables {
    x: [__m256; 2],
    y: [__m256; 2],
    z: [__m256; 2],
}

#[target_feature(enable = "avx2")]
#[allow(unsafe_code)]
fn add_groups(n: &Perlin, pts: Points<'_>, freq: f64, amp: f32, out: &mut [f32], p: Paths) -> usize {
    let groups = out.len() / LANES;
    let tables = gradient_tables();
    let amp_v = _mm256_set1_ps(amp);
    for at in (0..groups).map(|g| g * LANES) {
        let o = &mut out[at..at + LANES];
        let cells = [(pts.x, n.ox), (pts.y, n.oy), (pts.z, n.oz)].map(|(c, offset)| axis(&c[at..at + LANES], freq, offset));
        let [Some(x), Some(y), Some(z)] = cells else {
            for (k, o) in o.iter_mut().enumerate() {
                let i = at + k;
                *o += amp * n.get_with(pts.x[i] * freq, pts.y[i] * freq, pts.z[i] * freq, p);
            }
            continue;
        };
        let noise = sample(&n.perm, x, y, z, &tables, p.batch_shared_cell);
        // SAFETY: `o` holds 8 f32 (unaligned read and write).
        unsafe {
            let sum = _mm256_add_ps(_mm256_loadu_ps(o.as_ptr()), _mm256_mul_ps(amp_v, noise));
            _mm256_storeu_ps(o.as_mut_ptr(), sum);
        }
    }
    groups * LANES
}

#[target_feature(enable = "avx2")]
#[allow(unsafe_code)]
fn gradient_tables() -> GradientTables {
    let component = |c: usize| -> [__m256; 2] {
        let half = |h: usize| {
            let v: [f32; LANES] = std::array::from_fn(|k| GRAD_F[h * LANES + k][c]);
            // SAFETY: 8 f32 of `v`, unaligned read.
            unsafe { _mm256_loadu_ps(v.as_ptr()) }
        };
        [half(0), half(1)]
    };
    GradientTables { x: component(0), y: component(1), z: component(2) }
}

/// The 8 coordinates `c × freq`, wrapped, plus the noise offset: cell and position in the cell,
/// or `None` if one of them is outside the range the wrap leaves as it is.
#[target_feature(enable = "avx2")]
#[inline]
fn axis(c: &[f64], freq: f64, offset: f64) -> Option<Axis> {
    let (lo_cell, lo_offset) = axis_of_four(&c[..4], freq, offset)?;
    let (hi_cell, hi_offset) = axis_of_four(&c[4..LANES], freq, offset)?;
    let cell = _mm256_and_si256(_mm256_set_m128i(hi_cell, lo_cell), _mm256_set1_epi32(255));
    Some(Axis { cell, offset_in_cell: _mm256_set_m128(hi_offset, lo_offset) })
}

/// [`axis`] for 4 coordinates (lanes of a register of f64).
#[target_feature(enable = "avx2")]
#[inline]
#[allow(unsafe_code)]
fn axis_of_four(c: &[f64], freq: f64, offset: f64) -> Option<(__m128i, __m128)> {
    assert!(c.len() >= 4);
    // SAFETY: 4 f64 of `c` (length checked above), unaligned read.
    let scaled = _mm256_mul_pd(unsafe { _mm256_loadu_pd(c.as_ptr()) }, _mm256_set1_pd(freq));
    // The wrap leaves [-HALF_ROUND_OFF, HALF_ROUND_OFF[ as it is; the ordered compares put NaN outside.
    let low_bound = _mm256_cmp_pd::<_CMP_GE_OQ>(scaled, _mm256_set1_pd(-HALF_ROUND_OFF));
    let high_bound = _mm256_cmp_pd::<_CMP_LT_OQ>(scaled, _mm256_set1_pd(HALF_ROUND_OFF));
    if _mm256_movemask_pd(_mm256_and_pd(low_bound, high_bound)) != 0b1111 {
        return None;
    }
    let v = _mm256_add_pd(scaled, _mm256_set1_pd(offset));
    // The floor of the scalar path (`floor_in_i32_range`), its four operations: the shifted sum
    // rounds to the nearest integer, minus one where that rounded up.
    let shift = _mm256_set1_pd(ROUNDING_SHIFT);
    let nearest = _mm256_sub_pd(_mm256_add_pd(v, shift), shift);
    let rounded_up = _mm256_and_pd(_mm256_cmp_pd::<_CMP_GT_OQ>(nearest, v), _mm256_set1_pd(1.0));
    let floor = _mm256_sub_pd(nearest, rounded_up);
    // |v| < 2^24 + 256: the floor is an exact integer of i32, the truncation keeps it as it is.
    Some((_mm256_cvttpd_epi32(floor), _mm256_cvtpd_ps(_mm256_sub_pd(v, floor))))
}

/// The 8 permutation entries at `indices` (all below 512): 4 bytes read at each, the low one kept.
#[target_feature(enable = "avx2")]
#[inline]
#[allow(unsafe_code)]
fn perm_at(perm: &[u8; PERM_LEN], indices: __m256i) -> __m256i {
    // SAFETY: every index is below 512 and the table has 4 bytes more, so each 4-byte read stays
    // inside it; a gather has no alignment requirement.
    let words = unsafe { _mm256_i32gather_epi32::<1>(perm.as_ptr().cast::<i32>(), indices) };
    _mm256_and_si256(words, _mm256_set1_epi32(0xFF))
}

/// `g · (x, y, z)` for the gradient `h & 15` of each lane, in the order of `gdot_f`.
#[target_feature(enable = "avx2")]
#[inline]
fn gradient_dot(t: &GradientTables, h: __m256i, x: __m256, y: __m256, z: __m256) -> __m256 {
    // The low 3 bits pick in each half of the table, bit 3 (moved to the sign) picks the half.
    let upper_half = _mm256_castsi256_ps(_mm256_slli_epi32::<28>(h));
    let pick = |half: &[__m256; 2]| {
        _mm256_blendv_ps(_mm256_permutevar8x32_ps(half[0], h), _mm256_permutevar8x32_ps(half[1], h), upper_half)
    };
    let gx = _mm256_mul_ps(pick(&t.x), x);
    let gy = _mm256_mul_ps(pick(&t.y), y);
    _mm256_add_ps(_mm256_add_ps(gx, gy), _mm256_mul_ps(pick(&t.z), z))
}

#[target_feature(enable = "avx2")]
#[inline]
fn lerp(a: __m256, p: __m256, q: __m256) -> __m256 {
    _mm256_add_ps(p, _mm256_mul_ps(a, _mm256_sub_ps(q, p)))
}

#[target_feature(enable = "avx2")]
#[inline]
fn smoothstep(x: __m256) -> __m256 {
    let cube = _mm256_mul_ps(_mm256_mul_ps(x, x), x);
    let inner = _mm256_sub_ps(_mm256_mul_ps(x, _mm256_set1_ps(6.0)), _mm256_set1_ps(15.0));
    _mm256_mul_ps(cube, _mm256_add_ps(_mm256_mul_ps(x, inner), _mm256_set1_ps(10.0)))
}

/// Hash values of the 8 corners of each lane's cell, in the order 000, 100, 010, 110, 001, 101,
/// 011, 111 (x, y, z: 0 the cell, 1 the next one).
type CornerHashes = [__m256i; 8];

/// Corner hashes of 8 cells by gathers, the lookups of `sample_and_lerp_512` lane by lane.
#[target_feature(enable = "avx2")]
#[inline]
fn gathered_hashes(perm: &[u8; PERM_LEN], x: __m256i, y: __m256i, z: __m256i) -> CornerHashes {
    let one = _mm256_set1_epi32(1);
    let plus_one = |v: __m256i| _mm256_add_epi32(v, one);
    let x0 = perm_at(perm, x);
    let x1 = perm_at(perm, plus_one(x));
    let xy00 = perm_at(perm, _mm256_add_epi32(x0, y));
    let xy01 = perm_at(perm, plus_one(_mm256_add_epi32(x0, y)));
    let xy10 = perm_at(perm, _mm256_add_epi32(x1, y));
    let xy11 = perm_at(perm, plus_one(_mm256_add_epi32(x1, y)));
    let near = |xy: __m256i| perm_at(perm, _mm256_add_epi32(xy, z));
    let far = |xy: __m256i| perm_at(perm, plus_one(_mm256_add_epi32(xy, z)));
    [near(xy00), near(xy10), near(xy01), near(xy11), far(xy00), far(xy10), far(xy01), far(xy11)]
}

/// Corner hashes when the 8 lanes share one cell (`x`, `y`, `z` masked): the 14 lookups of a
/// single point, each value then copied to every lane.
#[target_feature(enable = "avx2")]
#[inline]
fn shared_cell_hashes(perm: &[u8; PERM_LEN], x: usize, y: usize, z: usize) -> CornerHashes {
    let p = |i: usize| usize::from(perm[i]);
    let (x0, x1) = (p(x), p(x + 1));
    let (xy00, xy01, xy10, xy11) = (p(x0 + y), p(x0 + y + 1), p(x1 + y), p(x1 + y + 1));
    let lanes = |i: usize| _mm256_set1_epi32(p(i) as i32);
    [
        lanes(xy00 + z),
        lanes(xy10 + z),
        lanes(xy01 + z),
        lanes(xy11 + z),
        lanes(xy00 + z + 1),
        lanes(xy10 + z + 1),
        lanes(xy01 + z + 1),
        lanes(xy11 + z + 1),
    ]
}

/// The cell of lane 0 if every lane holds it.
#[target_feature(enable = "avx2")]
#[inline]
fn shared_lane(v: __m256i) -> Option<usize> {
    let first = _mm256_permutevar8x32_epi32(v, _mm256_setzero_si256());
    let all_equal = _mm256_movemask_epi8(_mm256_cmpeq_epi32(v, first)) == -1;
    all_equal.then(|| _mm256_cvtsi256_si32(v) as usize)
}

/// Noise of 8 points from their cells: `sample_and_lerp_512`, lane by lane. With `shared_cell`, a
/// group whose 8 points lie in one cell (the low octaves of a small batch: its points span less
/// than a cell) reads the permutations once instead of gathering them.
#[target_feature(enable = "avx2")]
#[inline]
fn sample(perm: &[u8; PERM_LEN], x: Axis, y: Axis, z: Axis, t: &GradientTables, shared_cell: bool) -> __m256 {
    let shared = if shared_cell { shared_lane(x.cell).zip(shared_lane(y.cell)).zip(shared_lane(z.cell)) } else { None };
    let h = match shared {
        Some(((cx, cy), cz)) => shared_cell_hashes(perm, cx, cy, cz),
        None => gathered_hashes(perm, x.cell, y.cell, z.cell),
    };
    let (rx, ry, rz) = (x.offset_in_cell, y.offset_in_cell, z.offset_in_cell);
    let unit = _mm256_set1_ps(1.0);
    let (rx1, ry1, rz1) = (_mm256_sub_ps(rx, unit), _mm256_sub_ps(ry, unit), _mm256_sub_ps(rz, unit));
    let d000 = gradient_dot(t, h[0], rx, ry, rz);
    let d100 = gradient_dot(t, h[1], rx1, ry, rz);
    let d010 = gradient_dot(t, h[2], rx, ry1, rz);
    let d110 = gradient_dot(t, h[3], rx1, ry1, rz);
    let d001 = gradient_dot(t, h[4], rx, ry, rz1);
    let d101 = gradient_dot(t, h[5], rx1, ry, rz1);
    let d011 = gradient_dot(t, h[6], rx, ry1, rz1);
    let d111 = gradient_dot(t, h[7], rx1, ry1, rz1);
    let (ax, ay, az) = (smoothstep(rx), smoothstep(ry), smoothstep(rz));
    let near_z = lerp(ay, lerp(ax, d000, d100), lerp(ax, d010, d110));
    let far_z = lerp(ay, lerp(ax, d001, d101), lerp(ax, d011, d111));
    lerp(az, near_z, far_z)
}
