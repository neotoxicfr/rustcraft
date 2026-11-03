//! Arithmetic in the manner of the game's platform (spec § 0) and range intervals (§ 3.6).
//! The conversions to integers shared by every crate live in `rc_data::java_math`.

/// Old path of the noise kernel's floor (`RUSTCRAFT_OPT_FLOOR_JAVA=0`): library rounding then
/// saturated conversion (NaN → 0, < −2³¹ → MIN). The other generator floors call the shared one
/// directly: reading the switch at each call cost 27% of the floor.
#[inline]
pub fn floor_i_libm(v: f64) -> i32 {
    v.floor() as i32
}

#[inline]
pub fn lerp(a: f32, p: f32, q: f32) -> f32 {
    p + a * (q - p)
}

#[inline]
pub fn lerp2(a1: f32, a2: f32, x00: f32, x10: f32, x01: f32, x11: f32) -> f32 {
    lerp(a2, lerp(a1, x00, x10), lerp(a1, x01, x11))
}

#[allow(clippy::too_many_arguments)]
#[inline]
pub fn lerp3(
    a1: f32,
    a2: f32,
    a3: f32,
    x000: f32,
    x100: f32,
    x010: f32,
    x110: f32,
    x001: f32,
    x101: f32,
    x011: f32,
    x111: f32,
) -> f32 {
    lerp(a3, lerp2(a1, a2, x000, x100, x010, x110), lerp2(a1, a2, x001, x101, x011, x111))
}

#[inline]
pub fn smoothstep(x: f32) -> f32 {
    x * x * x * (x * (x * 6.0 - 15.0) + 10.0)
}

/// Java-style minimum: contagious NaN, −0 < +0.
#[inline]
pub fn jmin(a: f32, b: f32) -> f32 {
    if a.is_nan() {
        a
    } else if a == 0.0 && b == 0.0 && b.is_sign_negative() {
        b
    } else if a <= b {
        a
    } else {
        b
    }
}

/// Java-style maximum: contagious NaN, +0 > −0.
#[inline]
pub fn jmax(a: f32, b: f32) -> f32 {
    if a.is_nan() {
        a
    } else if a == 0.0 && b == 0.0 && a.is_sign_negative() {
        b
    } else if a >= b {
        a
    } else {
        b
    }
}

#[inline]
pub fn clamp(v: f32, mn: f32, mx: f32) -> f32 {
    if v < mn { mn } else { jmin(v, mx) }
}

#[inline]
pub fn signum(v: f32) -> f32 {
    if v.is_nan() || v == 0.0 { v } else { 1.0f32.copysign(v) }
}

#[inline]
pub fn floor_div(a: i32, b: i32) -> i32 {
    a.div_euclid(b) - i32::from(b < 0 && a.rem_euclid(b) != 0)
}

#[inline]
pub fn floor_mod(a: i32, b: i32) -> i32 {
    a.wrapping_sub(floor_div(a, b).wrapping_mul(b))
}

/// Declared range of a function: f32 interval, or "undefined".
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Interval {
    pub min: f32,
    pub max: f32,
    pub nai: bool,
}

#[inline]
fn mul_bound(a: f32, b: f32) -> f32 {
    if a != 0.0 && b != 0.0 { a * b } else { 0.0 }
}

// Operations named as in the spec (associated functions with two operands, not traits).
#[allow(clippy::should_implement_trait)]
impl Interval {
    pub const NAI: Interval = Interval { min: f32::NAN, max: f32::NAN, nai: true };
    pub const INFINITE: Interval = Interval { min: f32::NEG_INFINITY, max: f32::INFINITY, nai: false };

    pub fn of(min: f32, max: f32) -> Self {
        Interval { min, max, nai: false }
    }

    pub fn exact(v: f32) -> Self {
        Self::of(v, v)
    }

    pub fn symmetric(r: f32) -> Self {
        Self::of(-r, r)
    }

    pub fn contains(&self, v: f32) -> bool {
        v >= self.min && v <= self.max
    }

    pub fn encapsulating(list: impl IntoIterator<Item = Interval>) -> Self {
        let (mut mn, mut mx) = (f32::INFINITY, f32::NEG_INFINITY);
        for i in list {
            if !i.nai {
                mn = jmin(i.min, mn);
                mx = jmax(i.max, mx);
            }
        }
        if mx < mn { Self::NAI } else { Self::of(mn, mx) }
    }

    pub fn encapsulating2(a: f32, b: f32) -> Self {
        match (a.is_nan(), b.is_nan()) {
            (true, true) => Self::NAI,
            (true, false) => Self::exact(b),
            (false, true) => Self::exact(a),
            _ => Self::of(jmin(a, b), jmax(a, b)),
        }
    }

    fn encapsulating_with(first: Interval, v: f32) -> Self {
        if v.is_nan() {
            first
        } else if first.nai {
            Self::exact(v)
        } else {
            Self::of(jmin(first.min, v), jmax(first.max, v))
        }
    }

    pub fn add(a: Interval, b: Interval) -> Self {
        let (mn, mx) = (a.min + b.min, a.max + b.max);
        if mn.is_nan() || mx.is_nan() { Self::NAI } else { Self::of(mn, mx) }
    }

    pub fn sub(a: Interval, b: Interval) -> Self {
        let (mn, mx) = (a.min - b.max, a.max - b.min);
        if mn.is_nan() || mx.is_nan() { Self::NAI } else { Self::of(mn, mx) }
    }

    pub fn mul(a: Interval, b: Interval) -> Self {
        if a.nai || b.nai {
            return Self::NAI;
        }
        let p = [mul_bound(a.min, b.min), mul_bound(a.min, b.max), mul_bound(a.max, b.min), mul_bound(a.max, b.max)];
        Self::of(jmin(jmin(p[0], p[1]), jmin(p[2], p[3])), jmax(jmax(p[0], p[1]), jmax(p[2], p[3])))
    }

    pub fn reciprocal(i: Interval) -> Self {
        if i.nai || (i.min == 0.0 && i.max == 0.0) {
            Self::NAI
        } else if !i.contains(0.0) {
            Self::of(1.0 / i.max, 1.0 / i.min)
        } else if i.max == 0.0 {
            Self::of(f32::NEG_INFINITY, 1.0 / i.min)
        } else if i.min == 0.0 {
            Self::of(1.0 / i.max, f32::INFINITY)
        } else {
            Self::INFINITE
        }
    }

    pub fn div(a: Interval, b: Interval) -> Self {
        Self::mul(a, Self::reciprocal(b))
    }

    pub fn min(a: Interval, b: Interval) -> Self {
        if a.nai || b.nai { Self::NAI } else { Self::of(jmin(a.min, b.min), jmin(a.max, b.max)) }
    }

    pub fn max(a: Interval, b: Interval) -> Self {
        if a.nai || b.nai { Self::NAI } else { Self::of(jmax(a.min, b.min), jmax(a.max, b.max)) }
    }

    pub fn clamp(i: Interval, mn: f32, mx: f32) -> Self {
        if i.nai {
            Self::NAI
        } else if i.min >= mx {
            Self::of(mx, mx)
        } else if i.max <= mn {
            Self::of(mn, mn)
        } else {
            Self::of(jmax(i.min, mn), jmin(i.max, mx))
        }
    }

    pub fn abs(i: Interval) -> Self {
        if i.nai {
            return Self::NAI;
        }
        let mx = jmax(i.min.abs(), i.max.abs());
        if i.contains(0.0) { Self::of(0.0, mx) } else { Self::of(jmin(i.min.abs(), i.max.abs()), mx) }
    }

    pub fn square(i: Interval) -> Self {
        if i.nai {
            return Self::NAI;
        }
        let (a, b) = (i.min * i.min, i.max * i.max);
        let mx = jmax(a, b);
        if i.contains(0.0) { Self::of(0.0, mx) } else { Self::of(jmin(a, b), mx) }
    }

    pub fn map_monotonic(i: Interval, f: impl Fn(f32) -> f32) -> Self {
        if i.nai {
            return Self::NAI;
        }
        let (a, b) = (f(i.min), f(i.max));
        Self::of(jmin(a, b), jmax(a, b))
    }

    pub fn log(i: Interval) -> Self {
        if i.max < 0.0 {
            return Self::NAI;
        }
        let c = Self::max(i, Self::exact(0.0));
        Self::map_monotonic(c, |x| f64::from(x).ln() as f32)
    }

    pub fn sign(i: Interval) -> Self {
        if i.nai {
            Self::NAI
        } else if i.min == i.max {
            Self::exact(signum(i.min))
        } else if i.contains(0.0) {
            if i.min == 0.0 {
                Self::of(0.0, 1.0)
            } else if i.max == 0.0 {
                Self::of(-1.0, 0.0)
            } else {
                Self::of(-1.0, 1.0)
            }
        } else {
            Self::exact(if i.min > 0.0 { 1.0 } else { -1.0 })
        }
    }

    pub fn lerp(alpha: Interval, first: Interval, second: Interval) -> Self {
        if alpha.nai || first.nai || second.nai {
            return Self::NAI;
        }
        Self::encapsulating([
            Self::lerp_bounds(alpha, first.min, second.min),
            Self::lerp_bounds(alpha, first.max, second.min),
            Self::lerp_bounds(alpha, first.min, second.max),
            Self::lerp_bounds(alpha, first.max, second.max),
        ])
    }

    fn lerp_bounds(alpha: Interval, first: f32, second: f32) -> Self {
        if alpha.nai || first.is_nan() || second.is_nan() {
            Self::NAI
        } else if first.is_finite() && second.is_finite() {
            let f = |a: f32| first + mul_bound(a, second - first);
            Self::encapsulating2(f(alpha.min), f(alpha.max))
        } else {
            if first == second {
                return Self::exact(first);
            }
            let f = |a: f32| {
                let p1 = mul_bound(1.0 - a, first);
                let p2 = mul_bound(a, second);
                if !p1.is_infinite() || !p2.is_infinite() {
                    p1 + p2
                } else if a <= 0.0 {
                    if second > first { f32::NEG_INFINITY } else { f32::INFINITY }
                } else if a >= 1.0 {
                    if second > first { f32::INFINITY } else { f32::NEG_INFINITY }
                } else {
                    f32::NAN
                }
            };
            let (a, b) = (f(alpha.min), f(alpha.max));
            if a.is_nan() || b.is_nan() { Self::NAI } else { Self::encapsulating2(a, b) }
        }
    }

    pub fn pow(base: Interval, exp: Interval) -> Self {
        if base.nai || exp.nai {
            return Self::NAI;
        }
        if base.min == base.max {
            return Self::pow_scalar(base.min, exp);
        }
        let mut r = Self::encapsulating([Self::pow_scalar(base.min, exp), Self::pow_scalar(base.max, exp)]);
        if base.contains(0.0) {
            if base.max > 0.0 {
                r = Self::encapsulating([r, Self::pow_scalar(0.0, exp)]);
            }
            if base.min < 0.0 {
                r = Self::encapsulating([r, Self::pow_scalar(-0.0, exp)]);
            }
        }
        r
    }

    fn pow_scalar(base: f32, exp: Interval) -> Self {
        let p = |b: f32, e: f32| f64::from(b).powf(f64::from(e)) as f32;
        if base.is_nan() || exp.nai {
            Self::NAI
        } else if exp.min == exp.max {
            let v = p(base, exp.min);
            if v.is_nan() { Self::NAI } else { Self::exact(v) }
        } else if base == 0.0 {
            let z = if exp.contains(0.0) {
                if exp.max == 0.0 {
                    Self::of(1.0, f32::INFINITY)
                } else if exp.min == 0.0 {
                    Self::of(0.0, 1.0)
                } else {
                    Self::of(0.0, f32::INFINITY)
                }
            } else if exp.max < 0.0 {
                Self::exact(f32::INFINITY)
            } else {
                Self::exact(0.0)
            };
            Self::mul(z, Self::exact(1.0f32.copysign(base)))
        } else if base == 1.0 {
            Self::exact(1.0)
        } else if base > 0.0 {
            if exp.min.is_finite() && exp.max.is_finite() {
                Self::encapsulating2(p(base, exp.min), p(base, exp.max))
            } else if exp.min.is_infinite() && exp.max.is_infinite() {
                Self::of(0.0, f32::INFINITY)
            } else if exp.min.is_infinite() {
                if base < 1.0 { Self::of(p(base, exp.max), f32::INFINITY) } else { Self::of(0.0, p(base, exp.max)) }
            } else if base < 1.0 {
                Self::of(0.0, p(base, exp.min))
            } else {
                Self::of(p(base, exp.min), f32::INFINITY)
            }
        } else {
            let emin = f64::from(exp.min).ceil() as f32;
            let emax = f64::from(exp.max).floor() as f32;
            if emax < emin {
                return Self::NAI;
            }
            let bmin = p(base, emin);
            let bmax = p(base, emax);
            let mut r = Self::encapsulating2(bmin, bmax);
            if emin.is_infinite() {
                r = Self::encapsulating_with(r, -bmin);
            } else if emin + 1.0 < emax {
                r = Self::encapsulating_with(r, p(base, emin + 1.0));
            }
            if emax.is_infinite() {
                r = Self::encapsulating_with(r, -bmax);
            } else if emax - 1.0 > emin {
                r = Self::encapsulating_with(r, p(base, emax - 1.0));
            }
            r
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn java_min_max_and_divisions() {
        assert!(jmin(f32::NAN, 1.0).is_nan());
        assert!(jmin(1.0, f32::NAN).is_nan());
        assert!(jmin(0.0, -0.0).is_sign_negative());
        assert!(jmax(-0.0, 0.0).is_sign_positive());
        assert_eq!(floor_div(-7, 4), -2);
        assert_eq!(floor_mod(-7, 4), 1);
        assert_eq!(floor_div(7, -4), -2);
        assert_eq!(floor_mod(7, -4), -1);
    }
}
