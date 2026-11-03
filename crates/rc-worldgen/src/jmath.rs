//! Sine and cosine rounded to nearest, to reproduce those of the game.
//!
//! The game computes some shapes (icebergs, veins, End spikes) with the double sine and
//! cosine of its virtual machine. Measured over 20 000 angles: its results are the
//! exact value rounded to the nearest double in 99.9% of cases, when the system C library
//! differs from it by one unit of the last bit in about 3% of cases. We therefore compute in
//! double-double (about 104 bits) then round once: same value as the game except in
//! the rare cases where it does not itself round to nearest.
//!
//! The ways of computing chosen by [`SinCosPaths`] all do the same operations on each value,
//! hence the same bits: they only change how the exact product is obtained and which series
//! are run, and in what order.

use std::sync::LazyLock;

/// Unevaluated double-double number `hi + lo`, `|lo| <= ulp(hi) / 2`.
#[derive(Clone, Copy)]
struct Dd {
    hi: f64,
    lo: f64,
}

/// How the exact product `a * b = hi + lo` is obtained. All three give the same `lo`, bit for
/// bit, as long as nothing overflows or falls into the subnormals (our arguments stay below
/// 10^5 and the series stops well above 10^-300; only the cosine of an angle below about
/// 10^-150 reaches the subnormals, where both errors are far below the final rounding of 1.0).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProductPath {
    /// `f64::mul_add` in code compiled for any x86-64: a call to the C library's `fma`
    /// (old path, `RUSTCRAFT_OPT_TWO_PROD_FMA=0 RUSTCRAFT_OPT_TWO_PROD_SPLIT=0`).
    LibraryFma,
    /// Veltkamp split then Dekker product: only plain multiplications and additions.
    Split,
    /// `f64::mul_add` in bodies compiled for the FMA instruction, used only when the processor
    /// has it (otherwise this path falls back to [`ProductPath::Split`]).
    HardwareFma,
}

/// Choice of computation for [`sin_with`], [`cos_with`] and [`sincos_shared_with`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SinCosPaths {
    pub product: ProductPath,
    /// `sin` or `cos` alone runs only the series it returns, not both.
    pub only_needed_series: bool,
    /// When both series are needed, they advance in the same loop (see `both_series_interleaved`).
    pub interleaved_series: bool,
}

impl SinCosPaths {
    /// The code before these switches.
    pub const OLD: Self = Self { product: ProductPath::LibraryFma, only_needed_series: false, interleaved_series: false };

    /// Paths chosen once from the switches: FMA instruction if present and allowed, else the
    /// split, else the library call.
    pub fn selected() -> Self {
        static PATHS: LazyLock<SinCosPaths> = LazyLock::new(|| {
            let product = if crate::opt::two_prod_fma() && fma_available() {
                ProductPath::HardwareFma
            } else if crate::opt::two_prod_split() {
                ProductPath::Split
            } else {
                ProductPath::LibraryFma
            };
            SinCosPaths {
                product,
                only_needed_series: crate::opt::sincos_only_needed_series(),
                interleaved_series: crate::opt::sincos_interleaved_series(),
            }
        });
        *PATHS
    }
}

/// Exact product of two doubles, `hi = round(a * b)` and `lo = a * b - hi`.
trait ExactProduct {
    fn two_prod(a: f64, b: f64) -> Dd;
}

/// Through `f64::mul_add`: a library call or one instruction, depending on how the caller is compiled.
struct FusedProduct;

impl ExactProduct for FusedProduct {
    #[inline(always)]
    fn two_prod(a: f64, b: f64) -> Dd {
        let p = a * b;
        Dd { hi: p, lo: a.mul_add(b, -p) }
    }
}

/// Dekker's product on halves from Veltkamp's split: each half product is exact in a double.
struct SplitProduct;

/// 2^27 + 1: multiplying by it then subtracting back cuts a double into two halves of at
/// most 26 significant bits, whose products fit exactly in 53 bits.
const SPLITTER: f64 = 134_217_729.0;

/// `a = hi + lo` with `hi` and `lo` on at most 26 bits each.
#[inline(always)]
fn split(a: f64) -> (f64, f64) {
    let t = SPLITTER * a;
    let hi = t - (t - a);
    (hi, a - hi)
}

impl ExactProduct for SplitProduct {
    #[inline(always)]
    fn two_prod(a: f64, b: f64) -> Dd {
        let p = a * b;
        let (ah, al) = split(a);
        let (bh, bl) = split(b);
        let lo = al * bl - (((p - ah * bh) - al * bh) - ah * bl);
        Dd { hi: p, lo }
    }
}

#[inline]
fn two_sum(a: f64, b: f64) -> Dd {
    let s = a + b;
    let bb = s - a;
    let e = (a - (s - bb)) + (b - bb);
    Dd { hi: s, lo: e }
}

#[inline]
fn quick(a: f64, b: f64) -> Dd {
    let s = a + b;
    Dd { hi: s, lo: b - (s - a) }
}

#[inline]
fn add(a: Dd, b: Dd) -> Dd {
    let s = two_sum(a.hi, b.hi);
    let t = two_sum(a.lo, b.lo);
    let s = quick(s.hi, s.lo + t.hi);
    quick(s.hi, s.lo + t.lo)
}

// The functions that reach a product are `inline(always)`: inside the bodies compiled for FMA,
// they must be inlined to use the instruction instead of the library call.

#[inline(always)]
fn mul<P: ExactProduct>(a: Dd, b: Dd) -> Dd {
    let p = P::two_prod(a.hi, b.hi);
    quick(p.hi, p.lo + (a.hi * b.lo + a.lo * b.hi))
}

/// Division by a small exact integer.
#[inline(always)]
fn div_int<P: ExactProduct>(a: Dd, b: f64) -> Dd {
    let q1 = a.hi / b;
    let p = P::two_prod(q1, b);
    let s = ((a.hi - p.hi) - p.lo) + a.lo;
    quick(q1, s / b)
}

#[inline]
fn neg(a: Dd) -> Dd {
    Dd { hi: -a.hi, lo: -a.lo }
}

/// pi/2 in three pieces (33, 33 and 53 bits): `k * P1` and `k * P2` exact for `|k| < 2^20`.
const P1: f64 = 1.570_796_326_734_125_6;
const P2: f64 = 6.077_100_506_303_966e-11;
const P3: f64 = 2.022_266_248_795_950_6e-21;
const TWO_OVER_PI: f64 = std::f64::consts::FRAC_2_PI;

/// Reduction: `x = k * pi/2 + r`, `|r| <= ~pi/4`, `r` in double-double.
#[inline(always)]
fn reduce<P: ExactProduct>(x: f64) -> (i64, Dd) {
    let k = (x * TWO_OVER_PI).round();
    let t = x - k * P1; // exact (Sterbenz and exact product)
    let a = two_sum(t, -(k * P2));
    let c = P::two_prod(k, P3);
    (k as i64, add(a, neg(c)))
}

/// Taylor series of sin or cos on `|r| <= ~0.8`, in double-double, advanced one term at a time.
#[derive(Clone, Copy)]
struct Series {
    sum: Dd,
    term: Dd,
    /// Power of `r` in the last term.
    n: f64,
}

impl Series {
    fn sine(r: Dd) -> Self {
        Self { sum: r, term: r, n: 1.0 }
    }

    fn cosine() -> Self {
        let one = Dd { hi: 1.0, lo: 0.0 };
        Self { sum: one, term: one, n: 0.0 }
    }

    /// Adds the next term, `-term * r^2 / ((n + 1) (n + 2))`.
    #[inline(always)]
    fn step<P: ExactProduct>(&mut self, r2: Dd) {
        self.term = neg(div_int::<P>(mul::<P>(self.term, r2), (self.n + 1.0) * (self.n + 2.0)));
        self.n += 2.0;
        self.sum = add(self.sum, self.term);
    }

    /// The sine stops on a term negligible against its sum.
    fn sine_done(&self) -> bool {
        self.term.hi.abs() < 1e-36 * self.sum.hi.abs().max(1e-300) || self.n > 60.0
    }

    /// The cosine stops on a term negligible against 1.
    fn cosine_done(&self) -> bool {
        self.term.hi.abs() < 1e-36 || self.n > 60.0
    }
}

#[inline(always)]
fn sine_series<P: ExactProduct>(r: Dd, r2: Dd) -> Dd {
    let mut s = Series::sine(r);
    loop {
        s.step::<P>(r2);
        if s.sine_done() {
            return s.sum;
        }
    }
}

#[inline(always)]
fn cosine_series<P: ExactProduct>(r2: Dd) -> Dd {
    let mut c = Series::cosine();
    loop {
        c.step::<P>(r2);
        if c.cosine_done() {
            return c.sum;
        }
    }
}

/// Both series in one loop. Each term waits for the previous one through two divisions, so a
/// single series leaves the processor idle; the two chains are independent and overlap here.
/// Each series still does the same operations in the same order and stops on the same term.
#[inline(always)]
fn both_series_interleaved<P: ExactProduct>(r: Dd, r2: Dd) -> (Dd, Dd) {
    let (mut s, mut c) = (Series::sine(r), Series::cosine());
    let (mut sine_done, mut cosine_done) = (false, false);
    while !(sine_done && cosine_done) {
        if !sine_done {
            s.step::<P>(r2);
            sine_done = s.sine_done();
        }
        if !cosine_done {
            c.step::<P>(r2);
            cosine_done = c.cosine_done();
        }
    }
    (s.sum, c.sum)
}

#[inline(always)]
fn both_series<P: ExactProduct>(r: Dd, r2: Dd, paths: SinCosPaths) -> (Dd, Dd) {
    if paths.interleaved_series {
        both_series_interleaved::<P>(r, r2)
    } else {
        (sine_series::<P>(r, r2), cosine_series::<P>(r2))
    }
}

/// `sin(x)` (`want_sine`) or `cos(x)` from the reduced angle, in double-double.
#[inline(always)]
fn one_function<P: ExactProduct>(x: f64, want_sine: bool, paths: SinCosPaths) -> Dd {
    let (k, r) = reduce::<P>(x);
    let r2 = mul::<P>(r, r);
    // sin(x) is ±sin(r) on even quarters and ±cos(r) on odd ones; cos(x) the other way round.
    let quarter = k.rem_euclid(4);
    let sine_of_r = want_sine == (quarter % 2 == 0);
    let value = if paths.only_needed_series {
        if sine_of_r { sine_series::<P>(r, r2) } else { cosine_series::<P>(r2) }
    } else {
        let (s, c) = both_series::<P>(r, r2, paths);
        if sine_of_r { s } else { c }
    };
    let negative = if want_sine { quarter >= 2 } else { quarter == 1 || quarter == 2 };
    if negative { neg(value) } else { value }
}

fn round(d: Dd) -> f64 {
    d.hi + d.lo
}

#[inline(always)]
fn sin_body<P: ExactProduct>(x: f64, paths: SinCosPaths) -> f64 {
    if !x.is_finite() || x.abs() > 1e5 {
        return x.sin();
    }
    if x.abs() < 1e-8 {
        return x.sin();
    }
    round(one_function::<P>(x, true, paths))
}

#[inline(always)]
fn cos_body<P: ExactProduct>(x: f64, paths: SinCosPaths) -> f64 {
    if !x.is_finite() || x.abs() > 1e5 {
        return x.cos();
    }
    round(one_function::<P>(x, false, paths))
}

#[inline(always)]
fn sincos_body<P: ExactProduct>(x: f64, paths: SinCosPaths) -> (f64, f64) {
    if !x.is_finite() || x.abs() > 1e5 {
        return (x.sin(), x.cos());
    }
    let (k, r) = reduce::<P>(x);
    let r2 = mul::<P>(r, r);
    let (s, c) = both_series::<P>(r, r2, paths);
    let (sv, cv) = match k.rem_euclid(4) {
        0 => (s, c),
        1 => (c, neg(s)),
        2 => (neg(s), neg(c)),
        _ => (neg(c), s),
    };
    let sv = if x.abs() < 1e-8 { x.sin() } else { round(sv) };
    (sv, round(cv))
}

/// The same bodies compiled with the FMA instruction enabled, so that `mul_add` is one
/// instruction instead of a library call. Only called after detecting FMA on the processor.
#[cfg(target_arch = "x86_64")]
mod fma_compiled {
    use super::{FusedProduct, SinCosPaths, cos_body, sin_body, sincos_body};

    #[target_feature(enable = "fma")]
    pub(super) fn sin(x: f64, paths: SinCosPaths) -> f64 {
        sin_body::<FusedProduct>(x, paths)
    }

    #[target_feature(enable = "fma")]
    pub(super) fn cos(x: f64, paths: SinCosPaths) -> f64 {
        cos_body::<FusedProduct>(x, paths)
    }

    #[target_feature(enable = "fma")]
    pub(super) fn sincos(x: f64, paths: SinCosPaths) -> (f64, f64) {
        sincos_body::<FusedProduct>(x, paths)
    }
}

/// The processor has the FMA instruction (detection done once by the standard library).
pub fn fma_available() -> bool {
    #[cfg(target_arch = "x86_64")]
    {
        std::arch::is_x86_feature_detected!("fma")
    }
    #[cfg(not(target_arch = "x86_64"))]
    {
        false
    }
}

/// [`sin`] through given paths (equivalence tests and benches).
#[allow(unsafe_code)]
pub fn sin_with(x: f64, paths: SinCosPaths) -> f64 {
    match paths.product {
        ProductPath::LibraryFma => sin_body::<FusedProduct>(x, paths),
        #[cfg(target_arch = "x86_64")]
        // SAFETY: the FMA instruction was detected on this processor.
        ProductPath::HardwareFma if fma_available() => unsafe { fma_compiled::sin(x, paths) },
        ProductPath::Split | ProductPath::HardwareFma => sin_body::<SplitProduct>(x, paths),
    }
}

/// [`cos`] through given paths (equivalence tests and benches).
#[allow(unsafe_code)]
pub fn cos_with(x: f64, paths: SinCosPaths) -> f64 {
    match paths.product {
        ProductPath::LibraryFma => cos_body::<FusedProduct>(x, paths),
        #[cfg(target_arch = "x86_64")]
        // SAFETY: the FMA instruction was detected on this processor.
        ProductPath::HardwareFma if fma_available() => unsafe { fma_compiled::cos(x, paths) },
        ProductPath::Split | ProductPath::HardwareFma => cos_body::<SplitProduct>(x, paths),
    }
}

/// [`sincos_shared`] through given paths (equivalence tests and benches).
#[allow(unsafe_code)]
pub fn sincos_shared_with(x: f64, paths: SinCosPaths) -> (f64, f64) {
    match paths.product {
        ProductPath::LibraryFma => sincos_body::<FusedProduct>(x, paths),
        #[cfg(target_arch = "x86_64")]
        // SAFETY: the FMA instruction was detected on this processor.
        ProductPath::HardwareFma if fma_available() => unsafe { fma_compiled::sincos(x, paths) },
        ProductPath::Split | ProductPath::HardwareFma => sincos_body::<SplitProduct>(x, paths),
    }
}

/// Double sine, rounded to nearest (|x| < 10^5; beyond, the system's).
pub fn sin(x: f64) -> f64 {
    sin_with(x, SinCosPaths::selected())
}

/// Double cosine, rounded to nearest (|x| < 10^5; beyond, the system's).
pub fn cos(x: f64) -> f64 {
    cos_with(x, SinCosPaths::selected())
}

/// `(sin(x), cos(x))` with the same bits, with a single reduction and a single pair of series
/// (`RUSTCRAFT_OPT_SINCOS=0`: the two separate calls).
pub fn sincos(x: f64) -> (f64, f64) {
    if crate::opt::sincos() { sincos_shared(x) } else { (sin(x), cos(x)) }
}

#[doc(hidden)]
pub fn sincos_shared(x: f64) -> (f64, f64) {
    sincos_shared_with(x, SinCosPaths::selected())
}

#[cfg(test)]
mod tests {
    use rayon::prelude::*;

    use super::*;

    /// Known reference values (rounded to nearest) and consistency with the system.
    #[test]
    fn sin_cos_round_to_nearest() {
        assert_eq!(super::sin(0.0), 0.0);
        assert_eq!(super::cos(0.0), 1.0);
        assert_eq!(super::sin(std::f64::consts::FRAC_PI_2), 1.0);
        assert_eq!(super::cos(std::f64::consts::PI), -1.0);
        // sin(1) = 0.8414709848078965066525023216302989996225630607983710656727517099…
        assert_eq!(super::sin(1.0), 0.841_470_984_807_896_5);
        // cos(1) = 0.5403023058681397174009366074429766037323104206179222276700972553…
        assert_eq!(super::cos(1.0), 0.540_302_305_868_139_8);
        let mut x = -7.0;
        while x < 7.0 {
            assert!((super::sin(x) - x.sin()).abs() <= 2.0 * f64::EPSILON);
            assert!((super::cos(x) - x.cos()).abs() <= 2.0 * f64::EPSILON);
            x += 0.013;
        }
    }

    /// `sin`, `cos` and `sincos` of one angle, as bits, through one product implementation.
    fn bits_with<P: ExactProduct>(x: f64, paths: SinCosPaths) -> [u64; 4] {
        let (s, c) = sincos_body::<P>(x, paths);
        [sin_body::<P>(x, paths).to_bits(), cos_body::<P>(x, paths).to_bits(), s.to_bits(), c.to_bits()]
    }

    fn bits_of(x: f64, paths: SinCosPaths) -> [u64; 4] {
        let (s, c) = sincos_shared_with(x, paths);
        [sin_with(x, paths).to_bits(), cos_with(x, paths).to_bits(), s.to_bits(), c.to_bits()]
    }

    /// Edge angles: zeros, every power of two down to the subnormals (both signs), the
    /// limits 10^-8 and 10^5, quarter turns, non-finite values.
    fn edge_angles() -> Vec<f64> {
        let mut v = vec![0.0, -0.0, 1e-8, -1e-8, 1e5, -1e5, 1e5 + 1.0, f64::NAN, f64::INFINITY, f64::NEG_INFINITY];
        for e in -1074..17 {
            let p = 2f64.powi(e);
            v.extend([p, -p, p.next_up(), p.next_down()]);
        }
        for k in -63_662..63_662 {
            let q = f64::from(k) * std::f64::consts::FRAC_PI_2;
            v.extend([q, q.next_up(), q.next_down()]);
        }
        v
    }

    /// Random angles in ±10^5 (splitmix64, fixed seed), then the edge angles.
    fn test_angles(count: u64) -> impl ParallelIterator<Item = f64> {
        let random = (0..count).into_par_iter().map(|i| {
            let mut z = i.wrapping_add(1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
            z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
            z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
            z ^= z >> 31;
            let unit = (z >> 11) as f64 / (1u64 << 53) as f64;
            (unit * 2.0 - 1.0) * 1e5
        });
        random.chain(edge_angles().into_par_iter())
    }

    /// First angle where `candidate` differs from the old code.
    fn first_difference(count: u64, candidate: impl Fn(f64) -> [u64; 4] + Sync) -> Option<f64> {
        test_angles(count).find_any(|&x| candidate(x) != bits_with::<FusedProduct>(x, SinCosPaths::OLD))
    }

    /// Every combination of the three switches.
    fn all_paths() -> Vec<SinCosPaths> {
        let mut v = Vec::new();
        for product in [ProductPath::LibraryFma, ProductPath::Split, ProductPath::HardwareFma] {
            for only_needed_series in [false, true] {
                for interleaved_series in [false, true] {
                    v.push(SinCosPaths { product, only_needed_series, interleaved_series });
                }
            }
        }
        v
    }

    #[test]
    fn sin_cos_paths_match_old_bitwise() {
        if !fma_available() {
            eprintln!("no FMA on this processor: the hardware path is the split one");
        }
        for paths in all_paths().into_iter().filter(|&p| p != SinCosPaths::OLD) {
            assert_eq!(first_difference(10_000_000, |x| bits_of(x, paths)), None, "{paths:?}");
        }
    }

    /// A product that forgets its rounding error: the checker must see it.
    struct ProductWithoutError;

    impl ExactProduct for ProductWithoutError {
        fn two_prod(a: f64, b: f64) -> Dd {
            Dd { hi: a * b, lo: 0.0 }
        }
    }

    #[test]
    fn sin_cos_checker_detects_sabotage() {
        let new = SinCosPaths::selected();
        assert!(first_difference(100_000, |x| bits_with::<ProductWithoutError>(x, new)).is_some(), "product sabotage");
        // Wrong series for the odd quarters: sin(x) = cos(r) there, not sin(r).
        let wrong_series = |x: f64| {
            let mut bits = bits_of(x, new);
            let (k, r) = reduce::<FusedProduct>(x);
            if x.is_finite() && x.abs() >= 1e-8 && x.abs() <= 1e5 && k.rem_euclid(2) == 1 {
                bits[0] = round(sine_series::<FusedProduct>(r, mul::<FusedProduct>(r, r))).to_bits();
            }
            bits
        };
        assert!(first_difference(100_000, wrong_series).is_some(), "series sabotage");
    }
}
