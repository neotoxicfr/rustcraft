//! Noises (spec § 2): improved Perlin, "fudged" Perlin, 2D simplex, layer stacks, normal
//! data noise (`worldgen/noise/*.json`), legacy initialisation of the Nether climate and
//! legacy terrain noise stacks. Each noise has a point path and a volume path; the
//! two are reproduced as is (they are not always bit-for-bit equal, § 2.4).

use crate::Error;
use crate::json::Json;
use crate::math::{Interval, floor_i, lerp3, smoothstep};
use crate::rng::Rng;
use crate::volume::Volume;

/// Gradients (index `h & 15`).
const GRAD: [[i8; 3]; 16] = [
    [1, 1, 0],
    [-1, 1, 0],
    [1, -1, 0],
    [-1, -1, 0],
    [1, 0, 1],
    [-1, 0, 1],
    [1, 0, -1],
    [-1, 0, -1],
    [0, 1, 1],
    [0, -1, 1],
    [0, 1, -1],
    [0, -1, -1],
    [1, 1, 0],
    [0, -1, 1],
    [-1, 1, 0],
    [0, -1, -1],
];

/// Largest f64 strictly below 2^24.
const HALF_ROUND_OFF: f64 = 16_777_215.999_999_998;
const ROUND_OFF: f64 = 33_554_432.0;
/// Factor of the second stack of a normal noise.
pub const INPUT_FACTOR: f64 = 1.018_126_888_217_522_7;

/// Same gradients, already converted (exact values: −1, 0, 1).
const GRAD_F: [[f32; 3]; 16] = {
    let mut t = [[0.0f32; 3]; 16];
    let mut i = 0;
    while i < 16 {
        t[i] = [GRAD[i][0] as f32, GRAD[i][1] as f32, GRAD[i][2] as f32];
        i += 1;
    }
    t
};

/// Paths chosen by the switches (`crate::opt`), frozen at first use. The equivalence tests
/// pass their own values to compare old and new path.
#[doc(hidden)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Paths {
    pub floor_java: bool,
    pub perm512: bool,
    pub ypre: bool,
    pub colconst: bool,
    pub simd: bool,
    pub avx2: bool,
}

impl Paths {
    /// Code from before the mission.
    pub const OLD: Paths =
        Paths { floor_java: false, perm512: false, ypre: false, colconst: false, simd: false, avx2: false };

    pub fn current() -> Paths {
        static P: std::sync::LazyLock<Paths> = std::sync::LazyLock::new(|| Paths {
            floor_java: crate::opt::floor_java(),
            perm512: crate::opt::perm512(),
            ypre: crate::opt::noise_ypre(),
            colconst: crate::opt::noise_colconst(),
            simd: crate::opt::noise_simd(),
            avx2: crate::opt::noise_avx2(),
        });
        *P
    }
}

#[inline]
fn fl(v: f64, java: bool) -> i32 {
    if java { crate::math::floor_i_java(v) } else { crate::math::floor_i_libm(v) }
}

/// Cell and its value in f64 (always `f64::from(cell)`, like the old computation).
#[inline]
fn fl2(v: f64, java: bool) -> (i32, f64) {
    if java && v.abs() < 2_147_483_648.0 {
        crate::math::floor_small(v)
    } else {
        let i = fl(v, java);
        (i, f64::from(i))
    }
}

#[inline]
fn wrap(x: f64, java: bool) -> f64 {
    if (-HALF_ROUND_OFF..HALF_ROUND_OFF).contains(&x) {
        x
    } else {
        let q = x / ROUND_OFF + 0.5;
        // Identical for |q| < 2^63 (exact integer in both cases).
        let k = if java { crate::math::floor_l_java(q) as f64 } else { q.floor() };
        x - k * ROUND_OFF
    }
}

#[inline]
fn gdot(h: i32, x: f32, y: f32, z: f32) -> f32 {
    let g = GRAD[(h & 15) as usize];
    f32::from(g[0]) * x + f32::from(g[1]) * y + f32::from(g[2]) * z
}

#[inline]
fn gdot_f(h: u8, x: f32, y: f32, z: f32) -> f32 {
    let g = &GRAD_F[usize::from(h & 15)];
    g[0] * x + g[1] * y + g[2] * z
}

/// Gradient: partial x/z product and y component.
#[inline]
fn gxz(h: i32, x: f32, z: f32) -> (f32, f32) {
    let g = GRAD[(h & 15) as usize];
    (f32::from(g[0]) * x + f32::from(g[2]) * z, f32::from(g[1]))
}

/// f32 gradient: partial x/z product and y component.
#[inline]
fn gxz_f(h: u8, x: f32, z: f32) -> (f32, f32) {
    let g = &GRAD_F[usize::from(h & 15)];
    (g[0] * x + g[2] * z, g[1])
}

/// Precomputation in y of a volume (one octave): cell, smoothed weight, (fudged) position in the cell.
#[derive(Default)]
struct YPre {
    fy: Vec<i32>,
    ay: Vec<f32>,
    ty: Vec<f32>,
    ty1: Vec<f32>,
    /// Starts of the runs of the same cell (plus the end).
    runs: Vec<u32>,
}

thread_local! {
    static YPRE: std::cell::RefCell<YPre> = std::cell::RefCell::default();
}

/// Improved Perlin (§ 2.1).
#[derive(Clone)]
pub struct Perlin {
    /// Permutation, copied a second time (indices 256..512 = 0..256) to read without mask.
    perm: [u8; 512],
    pub ox: f64,
    pub oy: f64,
    pub oz: f64,
}

impl std::fmt::Debug for Perlin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Perlin({}, {}, {}, p0={}, p255={})", self.ox, self.oy, self.oz, self.perm[0], self.perm[255])
    }
}

impl Perlin {
    pub fn new(r: &mut Rng) -> Self {
        Self::with_offset_scale(r, 256.0)
    }

    fn with_offset_scale(r: &mut Rng, scale: f64) -> Self {
        let ox = r.next_double() * scale;
        let oy = r.next_double() * scale;
        let oz = r.next_double() * scale;
        let mut perm = [0u8; 512];
        for (i, p) in perm[..256].iter_mut().enumerate() {
            *p = i as u8;
        }
        for i in 0..256 {
            let j = i + r.next_int_bounded(256 - i as i32) as usize;
            perm.swap(i, j);
        }
        perm.copy_within(..256, 256);
        Perlin { perm, ox, oy, oz }
    }

    #[inline]
    fn p(&self, v: i32) -> i32 {
        i32::from(self.perm[(v & 255) as usize])
    }

    pub fn perm_first_last(&self) -> (u8, u8) {
        (self.perm[0], self.perm[255])
    }

    pub fn get(&self, x: f64, y: f64, z: f64) -> f32 {
        self.get_with(x, y, z, Paths::current())
    }

    #[doc(hidden)]
    pub fn get_with(&self, x: f64, y: f64, z: f64, p: Paths) -> f32 {
        let j = p.floor_java;
        let x = wrap(x, j) + self.ox;
        let y = wrap(y, j) + self.oy;
        let z = wrap(z, j) + self.oz;
        let ((fx, ffx), (fy, ffy), (fz, ffz)) = (fl2(x, j), fl2(y, j), fl2(z, j));
        let rx = (x - ffx) as f32;
        let ry = (y - ffy) as f32;
        let rz = (z - ffz) as f32;
        self.sample(fx, fy, fz, rx, ry, rz, ry, p.perm512)
    }

    #[allow(clippy::too_many_arguments)]
    #[inline]
    fn sample(&self, x: i32, y: i32, z: i32, rx: f32, ry: f32, rz: f32, ry_orig: f32, perm512: bool) -> f32 {
        if perm512 {
            self.sample_and_lerp_512(x, y, z, rx, ry, rz, ry_orig)
        } else {
            self.sample_and_lerp(x, y, z, rx, ry, rz, ry_orig)
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn sample_and_lerp(&self, x: i32, y: i32, z: i32, rx: f32, ry: f32, rz: f32, ry_orig: f32) -> f32 {
        let x0 = self.p(x);
        let x1 = self.p(x.wrapping_add(1));
        let xy00 = self.p(x0 + y);
        let xy01 = self.p(x0 + y.wrapping_add(1));
        let xy10 = self.p(x1 + y);
        let xy11 = self.p(x1 + y.wrapping_add(1));
        let d000 = gdot(self.p(xy00 + z), rx, ry, rz);
        let d100 = gdot(self.p(xy10 + z), rx - 1.0, ry, rz);
        let d010 = gdot(self.p(xy01 + z), rx, ry - 1.0, rz);
        let d110 = gdot(self.p(xy11 + z), rx - 1.0, ry - 1.0, rz);
        let d001 = gdot(self.p(xy00 + z.wrapping_add(1)), rx, ry, rz - 1.0);
        let d101 = gdot(self.p(xy10 + z.wrapping_add(1)), rx - 1.0, ry, rz - 1.0);
        let d011 = gdot(self.p(xy01 + z.wrapping_add(1)), rx, ry - 1.0, rz - 1.0);
        let d111 = gdot(self.p(xy11 + z.wrapping_add(1)), rx - 1.0, ry - 1.0, rz - 1.0);
        lerp3(smoothstep(rx), smoothstep(ry_orig), smoothstep(rz), d000, d100, d010, d110, d001, d101, d011, d111)
    }

    /// Same computation on the doubled table: `perm[a + b]` with `a, b < 256` equals `perm[(a + b) & 255]`,
    /// and `(a + b) & 255 = (a + (b & 255)) & 255`; no mask any more after the first.
    #[allow(clippy::too_many_arguments)]
    fn sample_and_lerp_512(&self, x: i32, y: i32, z: i32, rx: f32, ry: f32, rz: f32, ry_orig: f32) -> f32 {
        let p = &self.perm;
        let (x, y, z) = ((x & 255) as usize, (y & 255) as usize, (z & 255) as usize);
        let x0 = usize::from(p[x]);
        let x1 = usize::from(p[x + 1]);
        let xy00 = usize::from(p[x0 + y]);
        let xy01 = usize::from(p[x0 + y + 1]);
        let xy10 = usize::from(p[x1 + y]);
        let xy11 = usize::from(p[x1 + y + 1]);
        let d000 = gdot_f(p[xy00 + z], rx, ry, rz);
        let d100 = gdot_f(p[xy10 + z], rx - 1.0, ry, rz);
        let d010 = gdot_f(p[xy01 + z], rx, ry - 1.0, rz);
        let d110 = gdot_f(p[xy11 + z], rx - 1.0, ry - 1.0, rz);
        let d001 = gdot_f(p[xy00 + z + 1], rx, ry, rz - 1.0);
        let d101 = gdot_f(p[xy10 + z + 1], rx - 1.0, ry, rz - 1.0);
        let d011 = gdot_f(p[xy01 + z + 1], rx, ry - 1.0, rz - 1.0);
        let d111 = gdot_f(p[xy11 + z + 1], rx - 1.0, ry - 1.0, rz - 1.0);
        lerp3(smoothstep(rx), smoothstep(ry_orig), smoothstep(rz), d000, d100, d010, d110, d001, d101, d011, d111)
    }

    /// Volume path: `buf[i] += amp × noise(block × scale)`, walk z, x, y.
    pub fn add_to_volume(&self, buf: &mut [f32], v: &Volume, xz: f64, ys: f64, amp: f32) {
        self.add_to_volume_with(buf, v, xz, ys, amp, None, Paths::current());
    }

    /// Common body of the Perlin and the fudged Perlin (`fudge` = fudge scale).
    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)]
    pub fn add_to_volume_with(
        &self,
        buf: &mut [f32],
        v: &Volume,
        xz: f64,
        ys: f64,
        amp: f32,
        fudge: Option<f64>,
        p: Paths,
    ) {
        // `simd`, `avx2` and `colconst` live in the precomputed path: without `ypre`, old
        // whole path, so that each switch at 0 isolates its track on the bench.
        if p.ypre {
            YPRE.with_borrow_mut(|pre| {
                self.fill_ypre(pre, v, ys, fudge, p.floor_java);
                if p.avx2 && p.simd {
                    self.add_ypre_avx2(buf, v, xz, amp, p, pre);
                } else {
                    self.add_ypre(buf, v, xz, amp, p, pre);
                }
            });
        } else {
            self.add_to_volume_impl(buf, v, xz, ys, amp, fudge, p.floor_java);
        }
    }

    /// Old path: all the work in y redone for each column.
    #[allow(clippy::too_many_arguments)]
    fn add_to_volume_impl(
        &self,
        buf: &mut [f32],
        v: &Volume,
        xz: f64,
        ys: f64,
        amp: f32,
        fudge: Option<f64>,
        java: bool,
    ) {
        let mut idx = 0;
        let mut d = [0.0f32; 8];
        let mut gy = [0.0f32; 8];
        for iz in 0..v.sz {
            let z = wrap(f64::from(v.bz(iz)) * xz, java) + self.oz;
            let (fz, ffz) = fl2(z, java);
            let rz = (z - ffz) as f32;
            let az = smoothstep(rz);
            for ix in 0..v.sx {
                let x = wrap(f64::from(v.bx(ix)) * xz, java) + self.ox;
                let (fx, ffx) = fl2(x, java);
                let rx = (x - ffx) as f32;
                let x0 = self.p(fx);
                let x1 = self.p(fx.wrapping_add(1));
                let ax = smoothstep(rx);
                let mut last_fy = i32::MIN;
                for iy in 0..v.sy {
                    let orig_y = f64::from(v.by(iy)) * ys;
                    let y = wrap(orig_y, java) + self.oy;
                    let (fy, ffy) = fl2(y, java);
                    let ry64 = y - ffy;
                    let ry = ry64 as f32;
                    let ay = smoothstep(ry);
                    if last_fy != fy {
                        let xy00 = self.p(x0 + fy);
                        let xy01 = self.p(x0 + fy.wrapping_add(1));
                        let xy10 = self.p(x1 + fy);
                        let xy11 = self.p(x1 + fy.wrapping_add(1));
                        let z1 = fz.wrapping_add(1);
                        (d[0], gy[0]) = gxz(self.p(xy00 + fz), rx, rz);
                        (d[1], gy[1]) = gxz(self.p(xy10 + fz), rx - 1.0, rz);
                        (d[2], gy[2]) = gxz(self.p(xy01 + fz), rx, rz);
                        (d[3], gy[3]) = gxz(self.p(xy11 + fz), rx - 1.0, rz);
                        (d[4], gy[4]) = gxz(self.p(xy00 + z1), rx, rz - 1.0);
                        (d[5], gy[5]) = gxz(self.p(xy10 + z1), rx - 1.0, rz - 1.0);
                        (d[6], gy[6]) = gxz(self.p(xy01 + z1), rx, rz - 1.0);
                        (d[7], gy[7]) = gxz(self.p(xy11 + z1), rx - 1.0, rz - 1.0);
                        last_fy = fy;
                    }
                    let ty = match fudge {
                        None => ry,
                        Some(s) => (ry64 - fudge_y(orig_y, ry64, s, java)) as f32,
                    };
                    let ty1 = ty - 1.0;
                    let n = lerp3(
                        ax,
                        ay,
                        az,
                        d[0] + gy[0] * ty,
                        d[1] + gy[1] * ty,
                        d[2] + gy[2] * ty1,
                        d[3] + gy[3] * ty1,
                        d[4] + gy[4] * ty,
                        d[5] + gy[5] * ty,
                        d[6] + gy[6] * ty1,
                        d[7] + gy[7] * ty1,
                    );
                    buf[idx] += amp * n;
                    idx += 1;
                }
            }
        }
    }

    /// Work in y of an octave, same operations as the old path, done once.
    fn fill_ypre(&self, pre: &mut YPre, v: &Volume, ys: f64, fudge: Option<f64>, java: bool) {
        let n = v.sy as usize;
        for b in [&mut pre.ay, &mut pre.ty, &mut pre.ty1] {
            b.clear();
            b.reserve(n);
        }
        pre.fy.clear();
        pre.runs.clear();
        for iy in 0..v.sy {
            let orig_y = f64::from(v.by(iy)) * ys;
            let y = wrap(orig_y, java) + self.oy;
            let (fy, ffy) = fl2(y, java);
            let ry64 = y - ffy;
            let ry = ry64 as f32;
            let ty = match fudge {
                None => ry,
                Some(s) => (ry64 - fudge_y(orig_y, ry64, s, java)) as f32,
            };
            if pre.fy.last() != Some(&fy) {
                pre.runs.push(iy as u32);
            }
            pre.fy.push(fy);
            pre.ay.push(smoothstep(ry));
            pre.ty.push(ty);
            pre.ty1.push(ty - 1.0);
        }
        pre.runs.push(n as u32);
    }

    /// The 8 corners of a cell (x/z already taken, y component apart).
    #[allow(clippy::too_many_arguments)]
    #[inline(always)]
    fn corners(&self, x0: i32, x1: i32, fy: i32, fz: i32, rx: f32, rz: f32, perm512: bool) -> ([f32; 8], [f32; 8]) {
        let mut d = [0.0f32; 8];
        let mut gy = [0.0f32; 8];
        if perm512 {
            let p = &self.perm;
            let (x0, x1) = (x0 as usize, x1 as usize);
            let (y, z) = ((fy & 255) as usize, (fz & 255) as usize);
            let xy00 = usize::from(p[x0 + y]);
            let xy01 = usize::from(p[x0 + y + 1]);
            let xy10 = usize::from(p[x1 + y]);
            let xy11 = usize::from(p[x1 + y + 1]);
            (d[0], gy[0]) = gxz_f(p[xy00 + z], rx, rz);
            (d[1], gy[1]) = gxz_f(p[xy10 + z], rx - 1.0, rz);
            (d[2], gy[2]) = gxz_f(p[xy01 + z], rx, rz);
            (d[3], gy[3]) = gxz_f(p[xy11 + z], rx - 1.0, rz);
            (d[4], gy[4]) = gxz_f(p[xy00 + z + 1], rx, rz - 1.0);
            (d[5], gy[5]) = gxz_f(p[xy10 + z + 1], rx - 1.0, rz - 1.0);
            (d[6], gy[6]) = gxz_f(p[xy01 + z + 1], rx, rz - 1.0);
            (d[7], gy[7]) = gxz_f(p[xy11 + z + 1], rx - 1.0, rz - 1.0);
        } else {
            let xy00 = self.p(x0 + fy);
            let xy01 = self.p(x0 + fy.wrapping_add(1));
            let xy10 = self.p(x1 + fy);
            let xy11 = self.p(x1 + fy.wrapping_add(1));
            let z1 = fz.wrapping_add(1);
            (d[0], gy[0]) = gxz(self.p(xy00 + fz), rx, rz);
            (d[1], gy[1]) = gxz(self.p(xy10 + fz), rx - 1.0, rz);
            (d[2], gy[2]) = gxz(self.p(xy01 + fz), rx, rz);
            (d[3], gy[3]) = gxz(self.p(xy11 + fz), rx - 1.0, rz);
            (d[4], gy[4]) = gxz(self.p(xy00 + z1), rx, rz - 1.0);
            (d[5], gy[5]) = gxz(self.p(xy10 + z1), rx - 1.0, rz - 1.0);
            (d[6], gy[6]) = gxz(self.p(xy01 + z1), rx, rz - 1.0);
            (d[7], gy[7]) = gxz(self.p(xy11 + z1), rx - 1.0, rz - 1.0);
        }
        (d, gy)
    }

    fn add_ypre(&self, buf: &mut [f32], v: &Volume, xz: f64, amp: f32, p: Paths, pre: &YPre) {
        self.add_ypre_body(buf, v, xz, amp, p, pre);
    }

    /// Same body compiled for AVX2 (8 lanes); called only if the processor has it.
    #[cfg(target_arch = "x86_64")]
    #[allow(unsafe_code)]
    fn add_ypre_avx2(&self, buf: &mut [f32], v: &Volume, xz: f64, amp: f32, p: Paths, pre: &YPre) {
        #[target_feature(enable = "avx2")]
        fn body(n: &Perlin, buf: &mut [f32], v: &Volume, xz: f64, amp: f32, p: Paths, pre: &YPre) {
            n.add_ypre_body(buf, v, xz, amp, p, pre);
        }
        // `Paths` is public: `p.avx2` proves nothing, the detection (cached by the
        // standard library, one atomic read) is the only guard.
        if !crate::opt::avx2_available() {
            return self.add_ypre_body(buf, v, xz, amp, p, pre);
        }
        // SAFETY: AVX2 has just been detected on this processor.
        unsafe { body(self, buf, v, xz, amp, p, pre) }
    }

    #[cfg(not(target_arch = "x86_64"))]
    fn add_ypre_avx2(&self, buf: &mut [f32], v: &Volume, xz: f64, amp: f32, p: Paths, pre: &YPre) {
        self.add_ypre_body(buf, v, xz, amp, p, pre);
    }

    /// Precomputed path: per column, only the corners at the cell change and `lerp3`.
    #[inline(always)]
    fn add_ypre_body(&self, buf: &mut [f32], v: &Volume, xz: f64, amp: f32, p: Paths, pre: &YPre) {
        let java = p.floor_java;
        let sy = v.sy as usize;
        let (fys, ays, tys, ty1s) = (&pre.fy[..sy], &pre.ay[..sy], &pre.ty[..sy], &pre.ty1[..sy]);
        // All the y inputs equal (zero y scale, a single row…): one value per column.
        let constant = p.colconst && pre.runs.len() == 2 && {
            let (a, t) = (ays[0].to_bits(), tys[0].to_bits());
            ays.iter().all(|v| v.to_bits() == a) && tys.iter().all(|v| v.to_bits() == t)
        };
        let mut idx = 0;
        for iz in 0..v.sz {
            let z = wrap(f64::from(v.bz(iz)) * xz, java) + self.oz;
            let (fz, ffz) = fl2(z, java);
            let rz = (z - ffz) as f32;
            let az = smoothstep(rz);
            for ix in 0..v.sx {
                let x = wrap(f64::from(v.bx(ix)) * xz, java) + self.ox;
                let (fx, ffx) = fl2(x, java);
                let rx = (x - ffx) as f32;
                let (x0, x1) = if p.perm512 {
                    let i = (fx & 255) as usize;
                    (i32::from(self.perm[i]), i32::from(self.perm[i + 1]))
                } else {
                    (self.p(fx), self.p(fx.wrapping_add(1)))
                };
                let ax = smoothstep(rx);
                let col = &mut buf[idx..idx + sy];
                idx += sy;
                if constant {
                    let (d, g) = self.corners(x0, x1, fys[0], fz, rx, rz, p.perm512);
                    let n = cell_value(ax, ays[0], az, &d, &g, tys[0], ty1s[0]);
                    for o in col.iter_mut() {
                        *o += amp * n;
                    }
                } else if p.simd {
                    for r in pre.runs.windows(2) {
                        let (s, e) = (r[0] as usize, r[1] as usize);
                        let (d, g) = self.corners(x0, x1, fys[s], fz, rx, rz, p.perm512);
                        run_kernel(&mut col[s..e], &ays[s..e], &tys[s..e], &ty1s[s..e], ax, az, &d, &g, amp);
                    }
                } else {
                    let mut last_fy = i32::MIN;
                    let (mut d, mut g) = ([0.0f32; 8], [0.0f32; 8]);
                    for (k, o) in col.iter_mut().enumerate() {
                        let fy = fys[k];
                        if last_fy != fy {
                            (d, g) = self.corners(x0, x1, fy, fz, rx, rz, p.perm512);
                            last_fy = fy;
                        }
                        *o += amp * cell_value(ax, ays[k], az, &d, &g, tys[k], ty1s[k]);
                    }
                }
            }
        }
    }
}

/// Value in a cell, corners `d + gy × t` in the order of the old path.
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn cell_value(ax: f32, ay: f32, az: f32, d: &[f32; 8], g: &[f32; 8], ty: f32, ty1: f32) -> f32 {
    lerp3(
        ax,
        ay,
        az,
        d[0] + g[0] * ty,
        d[1] + g[1] * ty,
        d[2] + g[2] * ty1,
        d[3] + g[3] * ty1,
        d[4] + g[4] * ty,
        d[5] + g[5] * ty,
        d[6] + g[6] * ty1,
        d[7] + g[7] * ty1,
    )
}

/// Run of the same cell: branchless loop (vectorised by the compiler, multiplications
/// and additions separate, without FMA contraction).
#[allow(clippy::too_many_arguments)]
#[inline(always)]
fn run_kernel(
    out: &mut [f32],
    ay: &[f32],
    ty: &[f32],
    ty1: &[f32],
    ax: f32,
    az: f32,
    d: &[f32; 8],
    g: &[f32; 8],
    amp: f32,
) {
    let n = out.len();
    let (ay, ty, ty1) = (&ay[..n], &ty[..n], &ty1[..n]);
    for i in 0..n {
        out[i] += amp * cell_value(ax, ay[i], az, d, g, ty[i], ty1[i]);
    }
}

/// Vertical offset of the fudged Perlin (§ 2.2); `1e-7` is the f32 seen as f64.
#[inline]
fn fudge_y(orig_y: f64, ry: f64, scale: f64, java: bool) -> f64 {
    let lim = if orig_y >= 0.0 && orig_y < ry { orig_y } else { ry };
    f64::from(fl(lim / scale + f64::from(1.0e-7f32), java)) * scale
}

/// "Fudged" Perlin of the legacy terrain noise (§ 2.2).
#[derive(Debug, Clone)]
pub struct Smeared {
    base: Perlin,
    pub fudge: f64,
}

impl Smeared {
    pub fn new(r: &mut Rng, fudge: f64) -> Self {
        Smeared { base: Perlin::new(r), fudge }
    }

    pub fn get(&self, x0: f64, y0: f64, z0: f64) -> f32 {
        self.get_with(x0, y0, z0, Paths::current())
    }

    #[doc(hidden)]
    pub fn get_with(&self, x0: f64, y0: f64, z0: f64, p: Paths) -> f32 {
        let (b, j) = (&self.base, p.floor_java);
        let x = wrap(x0, j) + b.ox;
        let y = wrap(y0, j) + b.oy;
        let z = wrap(z0, j) + b.oz;
        let ((fx, ffx), (fy, ffy), (fz, ffz)) = (fl2(x, j), fl2(y, j), fl2(z, j));
        let rx = (x - ffx) as f32;
        let ry = y - ffy;
        let rz = (z - ffz) as f32;
        let fudged = (ry - fudge_y(y0, ry, self.fudge, j)) as f32;
        b.sample(fx, fy, fz, rx, fudged, rz, ry as f32, p.perm512)
    }

    pub fn range(fudge: f64) -> Interval {
        Interval::symmetric((fudge.abs() + 2.0) as f32)
    }
}

/// 2D simplex (§ 2.3), offsets drawn then cancelled.
#[derive(Debug, Clone)]
pub struct Simplex {
    base: Perlin,
}

const SQRT3: f64 = 1.732_050_807_568_877_2;

impl Simplex {
    pub fn new_without_offset(r: &mut Rng) -> Self {
        Simplex { base: Perlin::with_offset_scale(r, 0.0) }
    }

    fn corner(g: i32, x: f64, y: f64) -> f64 {
        let mut t = 0.5 - x * x - y * y - 0.0 * 0.0;
        if t < 0.0 {
            0.0
        } else {
            t *= t;
            let gr = GRAD[g as usize];
            t * t * (f64::from(gr[0]) * x + f64::from(gr[1]) * y + f64::from(gr[2]) * 0.0)
        }
    }

    pub fn get2(&self, xin: f64, yin: f64) -> f32 {
        let f2 = 0.5 * (SQRT3 - 1.0);
        let g2 = (3.0 - SQRT3) / 6.0;
        let b = &self.base;
        let xin = xin + b.ox;
        let yin = yin + b.oy;
        let s = (xin + yin) * f2;
        let i = floor_i(xin + s);
        let j = floor_i(yin + s);
        let t = f64::from(i.wrapping_add(j)) * g2;
        let x0 = xin - (f64::from(i) - t);
        let y0 = yin - (f64::from(j) - t);
        let (i1, j1) = if x0 > y0 { (1, 0) } else { (0, 1) };
        let x1 = x0 - f64::from(i1) + g2;
        let y1 = y0 - f64::from(j1) + g2;
        let x2 = x0 - 1.0 + 2.0 * g2;
        let y2 = y0 - 1.0 + 2.0 * g2;
        let ii = i & 255;
        let jj = j & 255;
        let g0 = b.p(ii + b.p(jj)) % 12;
        let g1 = b.p(ii + i1 + b.p(jj + j1)) % 12;
        let gg2 = b.p(ii + 1 + b.p(jj + 1)) % 12;
        let n0 = Self::corner(g0, x0, y0);
        let n1 = Self::corner(g1, x1, y1);
        let n2 = Self::corner(gg2, x2, y2);
        (70.0 * (n0 + n1 + n2)) as f32
    }
}

/// Elementary noise of a layer.
#[derive(Debug, Clone)]
pub enum Octave {
    Perlin(Perlin),
    Smeared(Smeared),
}

impl Octave {
    #[inline]
    fn get(&self, x: f64, y: f64, z: f64, p: Paths) -> f32 {
        match self {
            Octave::Perlin(n) => n.get_with(x, y, z, p),
            Octave::Smeared(s) => s.get_with(x, y, z, p),
        }
    }

    fn add_to_volume(&self, buf: &mut [f32], v: &Volume, xz: f64, ys: f64, amp: f32, p: Paths) {
        match self {
            Octave::Perlin(n) => n.add_to_volume_with(buf, v, xz, ys, amp, None, p),
            Octave::Smeared(s) => s.base.add_to_volume_with(buf, v, xz, ys, amp, Some(s.fudge), p),
        }
    }

    pub fn perlin(&self) -> &Perlin {
        match self {
            Octave::Perlin(p) => p,
            Octave::Smeared(s) => &s.base,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Layer {
    pub noise: Octave,
    pub freq: f64,
    pub amp: f32,
}

/// Layer stack (§ 2.4), summed in order.
#[derive(Debug, Clone, Default)]
pub struct NoiseStack {
    pub layers: Vec<Layer>,
}

impl NoiseStack {
    /// Point path: `Σ amp × noise(x·f, y·f, z·f)`.
    pub fn get(&self, x: f64, y: f64, z: f64) -> f32 {
        self.get_with(x, y, z, Paths::current())
    }

    #[doc(hidden)]
    pub fn get_with(&self, x: f64, y: f64, z: f64, p: Paths) -> f32 {
        let mut v = 0.0f32;
        for l in &self.layers {
            v += l.amp * l.noise.get(x * l.freq, y * l.freq, z * l.freq, p);
        }
        v
    }

    /// Volume path: each layer adds `(amp × layer_amp) × noise(block × (scale × f))`.
    pub fn add_to_volume(&self, buf: &mut [f32], v: &Volume, xz: f64, ys: f64, amp: f32) {
        self.add_to_volume_with(buf, v, xz, ys, amp, Paths::current());
    }

    #[doc(hidden)]
    pub fn add_to_volume_with(&self, buf: &mut [f32], v: &Volume, xz: f64, ys: f64, amp: f32, p: Paths) {
        for l in &self.layers {
            l.noise.add_to_volume(buf, v, xz * l.freq, ys * l.freq, amp * l.amp, p);
        }
    }

    /// Range of the stack: sum of the ranges of the layers (Perlin ±2, fudged ±(|s| + 2)).
    pub fn range(&self) -> Interval {
        let mut r = Interval::exact(0.0);
        for l in &self.layers {
            let nr = match &l.noise {
                Octave::Perlin(_) => Interval::symmetric(2.0),
                Octave::Smeared(s) => Smeared::range(s.fudge),
            };
            r = Interval::add(r, Interval::mul(nr, Interval::exact(l.amp)));
        }
        r
    }

    fn push_stack(&mut self, other: NoiseStack, freq: f64, amp: f32) {
        for l in other.layers {
            self.layers.push(Layer { noise: l.noise, freq: l.freq * freq, amp: l.amp * amp });
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Normalize {
    Disabled,
    Enabled,
    Legacy,
}

/// Parameters of a normal noise (§ 2.5) and derived quantities.
#[derive(Debug, Clone)]
pub struct NormalNoise {
    pub base_amplitude: f64,
    pub base_octave: i32,
    pub octave_count: i32,
    pub normalize: Normalize,
    pub modifiers: Vec<f64>,
    /// Octaves kept: (index, frequency, amplitude).
    pub octaves: Vec<(i32, f64, f64)>,
    pub factor: f64,
    pub range: Interval,
}

fn kahan_sum(values: impl Iterator<Item = f64>) -> f64 {
    let (mut s, mut c, mut simple) = (0.0f64, 0.0f64, 0.0f64);
    for v in values {
        let t = v - c;
        let w = s + t;
        c = (w - s) - t;
        s = w;
        simple += v;
    }
    let r = s - c;
    if r.is_nan() && simple.is_infinite() { simple } else { r }
}

impl NormalNoise {
    pub fn from_json(j: &Json) -> Result<Self, Error> {
        let bad = |w: &str| Error::Format(format!("noise: {w}"));
        let base_octave = j.get("base_octave").and_then(Json::as_i32).ok_or_else(|| bad("base_octave"))?;
        let octave_count = j.get("octave_count").map_or(Some(1), Json::as_i32).ok_or_else(|| bad("octave_count"))?;
        let base_amplitude =
            j.get("base_amplitude").map_or(Some(1.0), Json::as_f64).ok_or_else(|| bad("base_amplitude"))?;
        let normalize = match j.get("normalize") {
            None | Some(Json::Bool(true)) => Normalize::Enabled,
            Some(Json::Bool(false)) => Normalize::Disabled,
            Some(Json::Str(s)) if s == "legacy" => Normalize::Legacy,
            Some(_) => return Err(bad("normalize")),
        };
        let modifiers: Vec<f64> = match j.get("amplitude_modifiers") {
            None => Vec::new(),
            Some(a) => a
                .as_array()
                .ok_or_else(|| bad("amplitude_modifiers"))?
                .iter()
                .map(|v| v.as_f64().ok_or_else(|| bad("amplitude_modifiers")))
                .collect::<Result<_, _>>()?,
        };
        if !modifiers.is_empty() && modifiers.len() != octave_count as usize {
            return Err(bad("size of amplitude_modifiers"));
        }
        Ok(Self::new(base_amplitude, base_octave, octave_count, normalize, modifiers))
    }

    pub fn new(
        base_amplitude: f64,
        base_octave: i32,
        octave_count: i32,
        normalize: Normalize,
        modifiers: Vec<f64>,
    ) -> Self {
        let modifier = |i: i32| if modifiers.is_empty() { 1.0 } else { modifiers[i as usize] };
        let mut freq = 2f64.powi(base_octave);
        let mut amp = base_amplitude;
        if normalize != Normalize::Disabled {
            amp *= 0.5f64.powi(-(octave_count - 1)) / (0.5f64.powi(-octave_count) - 1.0);
        }
        let mut octaves = Vec::new();
        for i in 0..octave_count {
            let m = modifier(i);
            if m != 0.0 {
                octaves.push((base_octave + i, freq, amp * m));
            }
            freq *= 2.0;
            amp *= 0.5;
        }
        let mut target = kahan_sum(octaves.iter().map(|o| o.2.abs()));
        let mut factor = normalization_factor(target, &octaves);
        if normalize == Normalize::Legacy && factor != 0.0 {
            let (mut lo, mut hi) = (i32::MAX, i32::MIN);
            for i in 0..octave_count {
                if modifier(i) != 0.0 {
                    lo = lo.min(i);
                    hi = hi.max(i);
                }
            }
            let parity = base_amplitude * 0.5 * 0.333_333_333_333_333_3 / (0.1 * (1.0 + 1.0 / f64::from(hi - lo + 1)));
            target *= parity / factor;
            factor = parity;
        }
        let range = Interval::symmetric((target * 0.333_333_333_333_333_3 * 6.0) as f32);
        NormalNoise { base_amplitude, base_octave, octave_count, normalize, modifiers, octaves, factor, range }
    }

    /// Instantiation from the noise generator (§ 2.5).
    pub fn create(&self, r: &mut Rng) -> NoiseStack {
        let first = r.fork_positional();
        let second = r.fork_positional();
        let mut s = NoiseStack::default();
        for &(index, freq, amp) in &self.octaves {
            let seed = format!("octave_{index}");
            let a = Perlin::new(&mut first.from_hash_of(&seed));
            let b = Perlin::new(&mut second.from_hash_of(&seed));
            let k = (self.factor * amp) as f32;
            s.layers.push(Layer { noise: Octave::Perlin(a), freq, amp: k });
            s.layers.push(Layer { noise: Octave::Perlin(b), freq: freq * INPUT_FACTOR, amp: k });
        }
        s
    }

    /// Legacy initialisation of the Nether climate (§ 2.6).
    pub fn create_legacy_nether(&self, r: &mut Rng) -> NoiseStack {
        let amps: Vec<f64> =
            if self.modifiers.is_empty() { vec![1.0; self.octave_count as usize] } else { self.modifiers.clone() };
        let first = legacy_fbm(r, self.base_octave, &amps);
        let second = legacy_fbm(r, self.base_octave, &amps);
        let k = (self.factor * self.base_amplitude) as f32;
        let mut s = NoiseStack::default();
        s.push_stack(first, 1.0, k);
        s.push_stack(second, INPUT_FACTOR, k);
        s
    }
}

fn normalization_factor(target: f64, octaves: &[(i32, f64, f64)]) -> f64 {
    let mut variance = 0.0f64;
    for o in octaves {
        let d = 0.270_224_783_124_521_1 * o.2.abs();
        variance += d * d;
    }
    let dev = variance.sqrt();
    if dev == 0.0 { 0.0 } else { target * 0.333_333_333_333_333_3 / (dev * 2f64.sqrt()) }
}

fn legacy_fbm(r: &mut Rng, first_octave: i32, amps: &[f64]) -> NoiseStack {
    let n = amps.len() as i32;
    let zero = -first_octave;
    let mut levels: Vec<Option<Perlin>> = vec![None; amps.len()];
    let z = Perlin::new(r);
    if zero >= 0 && zero < n && amps[zero as usize] != 0.0 {
        levels[zero as usize] = Some(z);
    }
    for i in (0..zero).rev() {
        if i < n && amps[i as usize] != 0.0 {
            levels[i as usize] = Some(Perlin::new(r));
        } else {
            r.consume(262);
        }
    }
    let mut f = 2f64.powi(-zero);
    let mut v = 2f64.powi(n - 1) / (2f64.powi(n) - 1.0);
    let mut s = NoiseStack::default();
    for (i, lvl) in levels.into_iter().enumerate() {
        if let Some(p) = lvl {
            s.layers.push(Layer { noise: Octave::Perlin(p), freq: f, amp: (v * amps[i]) as f32 });
        }
        f *= 2.0;
        v /= 2.0;
    }
    s
}

/// Legacy terrain noise stack (`old_blended_noise`, § 2.7).
pub fn blended_fbm(r: &mut Rng, first_octave: i32, smear_y: f64, value_factor: f64) -> NoiseStack {
    let octaves = -first_octave + 1;
    let mut factor = 1.0f64;
    let mut vf = value_factor / (2f64.powi(octaves) - 1.0);
    let mut s = NoiseStack::default();
    for _ in 0..octaves {
        let noise = Smeared::new(r, smear_y * factor);
        s.layers.push(Layer { noise: Octave::Smeared(noise), freq: factor, amp: vf as f32 });
        factor /= 2.0;
        vf *= 2.0;
    }
    s
}

/// Declared range of this stack.
pub fn blended_fbm_range(first_octave: i32, smear_y: f64, value_factor: f64) -> Interval {
    let octaves = -first_octave + 1;
    let mut factor = 1.0f64;
    let mut vf = value_factor / (2f64.powi(octaves) - 1.0);
    let mut range = Interval::exact(0.0);
    for _ in 0..octaves {
        let layer = Interval::mul(Smeared::range(smear_y * factor), Interval::exact(vf as f32));
        range = Interval::add(range, layer);
        factor /= 2.0;
        vf *= 2.0;
    }
    range
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrap_bound_is_next_down_of_2_pow_24() {
        assert_eq!(HALF_ROUND_OFF, f64::from_bits(16_777_216f64.to_bits() - 1));
    }
}
