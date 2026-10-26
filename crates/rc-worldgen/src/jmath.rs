//! Sine and cosine rounded to nearest, to reproduce those of the game.
//!
//! The game computes some shapes (icebergs, veins, End spikes) with the double sine and
//! cosine of its virtual machine. Measured over 20 000 angles: its results are the
//! exact value rounded to the nearest double in 99.9% of cases, when the system C library
//! differs from it by one unit of the last bit in about 3% of cases. We therefore compute in
//! double-double (about 104 bits) then round once: same value as the game except in
//! the rare cases where it does not itself round to nearest.

/// Unevaluated double-double number `hi + lo`, `|lo| <= ulp(hi) / 2`.
#[derive(Clone, Copy)]
struct Dd {
    hi: f64,
    lo: f64,
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
fn two_prod(a: f64, b: f64) -> Dd {
    let p = a * b;
    Dd { hi: p, lo: a.mul_add(b, -p) }
}

#[inline]
fn add(a: Dd, b: Dd) -> Dd {
    let s = two_sum(a.hi, b.hi);
    let t = two_sum(a.lo, b.lo);
    let s = quick(s.hi, s.lo + t.hi);
    quick(s.hi, s.lo + t.lo)
}

#[inline]
fn mul(a: Dd, b: Dd) -> Dd {
    let p = two_prod(a.hi, b.hi);
    quick(p.hi, p.lo + (a.hi * b.lo + a.lo * b.hi))
}

/// Division by a small exact integer.
#[inline]
fn div_int(a: Dd, b: f64) -> Dd {
    let q1 = a.hi / b;
    let p = two_prod(q1, b);
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
fn reduce(x: f64) -> (i64, Dd) {
    let k = (x * TWO_OVER_PI).round();
    let t = x - k * P1; // exact (Sterbenz and exact product)
    let a = two_sum(t, -(k * P2));
    let c = two_prod(k, P3);
    (k as i64, add(a, neg(c)))
}

/// Taylor series of sin and cos on `|r| <= ~0.8`, in double-double.
fn sin_cos_dd(r: Dd) -> (Dd, Dd) {
    let r2 = mul(r, r);
    let mut s = r;
    let mut term = r;
    let mut n = 1.0;
    loop {
        term = div_int(mul(term, r2), (n + 1.0) * (n + 2.0));
        term = neg(term);
        n += 2.0;
        s = add(s, term);
        if term.hi.abs() < 1e-36 * s.hi.abs().max(1e-300) || n > 60.0 {
            break;
        }
    }
    let mut c = Dd { hi: 1.0, lo: 0.0 };
    let mut term = c;
    let mut n = 0.0;
    loop {
        term = div_int(mul(term, r2), (n + 1.0) * (n + 2.0));
        term = neg(term);
        n += 2.0;
        c = add(c, term);
        if term.hi.abs() < 1e-36 || n > 60.0 {
            break;
        }
    }
    (s, c)
}

fn round(d: Dd) -> f64 {
    d.hi + d.lo
}

/// Double sine, rounded to nearest (|x| < 10^5; beyond, the system's).
pub fn sin(x: f64) -> f64 {
    if !x.is_finite() || x.abs() > 1e5 {
        return x.sin();
    }
    if x.abs() < 1e-8 {
        return x.sin();
    }
    let (k, r) = reduce(x);
    let (s, c) = sin_cos_dd(r);
    round(match k.rem_euclid(4) {
        0 => s,
        1 => c,
        2 => neg(s),
        _ => neg(c),
    })
}

/// Double cosine, rounded to nearest (|x| < 10^5; beyond, the system's).
pub fn cos(x: f64) -> f64 {
    if !x.is_finite() || x.abs() > 1e5 {
        return x.cos();
    }
    let (k, r) = reduce(x);
    let (s, c) = sin_cos_dd(r);
    round(match k.rem_euclid(4) {
        0 => c,
        1 => neg(s),
        2 => neg(c),
        _ => s,
    })
}

/// `(sin(x), cos(x))` with the same bits, with a single reduction and a single pair of series
/// (`RUSTCRAFT_OPT_SINCOS=0`: the two separate calls).
pub fn sincos(x: f64) -> (f64, f64) {
    if crate::opt::sincos() { sincos_shared(x) } else { (sin(x), cos(x)) }
}

#[doc(hidden)]
pub fn sincos_shared(x: f64) -> (f64, f64) {
    if !x.is_finite() || x.abs() > 1e5 {
        return (x.sin(), x.cos());
    }
    let (k, r) = reduce(x);
    let (s, c) = sin_cos_dd(r);
    let (sv, cv) = match k.rem_euclid(4) {
        0 => (s, c),
        1 => (c, neg(s)),
        2 => (neg(s), neg(c)),
        _ => (neg(c), s),
    };
    let sv = if x.abs() < 1e-8 { x.sin() } else { round(sv) };
    (sv, round(cv))
}

#[cfg(test)]
mod tests {
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
}
