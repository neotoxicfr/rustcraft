//! The game's arithmetic to reproduce as is (`server-mobs.md` § 5.5): approximate `atan2`,
//! angles wrapped, bounded rotations, tabulated sine.

use std::sync::OnceLock;

pub use rc_physics::{cos_t as cos, sin_t as sin};

pub const DEG: f32 = std::f32::consts::PI / 180.0;

/// The game's square root of 2: single-precision constant promoted to double (1.4142135381…), not the
/// double root. Serves as the distance factor of oriented random positions.
pub const SQRT_OF_TWO: f64 = std::f32::consts::SQRT_2 as f64;

/// Distance of an oriented draw: interpolation `min + t·(max − min)` then × [`SQRT_OF_TWO`].
pub fn rpos_dist(t: f64, min: f64, max: f64) -> f64 {
    (min + t * (max - min)) * SQRT_OF_TWO
}

fn tables() -> &'static ([f64; 257], [f64; 257]) {
    static T: OnceLock<([f64; 257], [f64; 257])> = OnceLock::new();
    T.get_or_init(|| {
        let mut asin = [0.0; 257];
        let mut cos = [0.0; 257];
        for i in 0..257 {
            let a = (f64::from(i as u32) / 256.0).asin();
            asin[i] = a;
            cos[i] = a.cos();
        }
        (asin, cos)
    })
}

fn fast_inv_sqrt(x: f64) -> f64 {
    let half = 0.5 * x;
    let i = 0x5FE6_EB50_C7B5_37AAi64 - ((x.to_bits() as i64) >> 1);
    let y = f64::from_bits(i as u64);
    y * (1.5 - half * y * y)
}

/// The game's `atan2` (table of 257 arcsines, cubic correction).
pub fn atan2(mut y: f64, mut x: f64) -> f64 {
    let d2 = x * x + y * y;
    if d2.is_nan() {
        return f64::NAN;
    }
    let neg_y = y < 0.0;
    if neg_y {
        y = -y;
    }
    let neg_x = x < 0.0;
    if neg_x {
        x = -x;
    }
    let steep = y > x;
    if steep {
        std::mem::swap(&mut x, &mut y);
    }
    let r = fast_inv_sqrt(d2);
    x *= r;
    y *= r;
    const BIAS: f64 = 17_592_186_044_416.0; // 2^44
    let yp = BIAS + y;
    let index = (yp.to_bits() as i64) as i32 as usize;
    let (asin, cos) = tables();
    let phi = asin[index];
    let c = cos[index];
    let s = yp - BIAS;
    let sd = y * c - x * s;
    let d = (6.0 + sd * sd) * sd * 0.166_666_666_666_666_66;
    let mut theta = phi + d;
    if steep {
        theta = std::f64::consts::FRAC_PI_2 - theta;
    }
    if neg_x {
        theta = std::f64::consts::PI - theta;
    }
    if neg_y {
        theta = -theta;
    }
    theta
}

/// Yaw aimed toward (dx, dz) in degrees: `atan2(dz, dx) × 180/π − 90` (the game's `f32` computation).
pub fn yaw_to(dx: f64, dz: f64) -> f32 {
    (atan2(dz, dx) * f64::from(180.0f32 / std::f32::consts::PI)) as f32 - 90.0
}

/// Pitch aimed (dy vertical, sd horizontal).
pub fn pitch_to(dy: f64, sd: f64) -> f32 {
    -((atan2(dy, sd) * f64::from(180.0f32 / std::f32::consts::PI)) as f32)
}

pub fn wrap(a: f32) -> f32 {
    let mut n = a % 360.0;
    if n >= 180.0 {
        n -= 360.0;
    }
    if n < -180.0 {
        n += 360.0;
    }
    n
}

pub fn degrees_difference(from: f32, to: f32) -> f32 {
    wrap(to - from)
}

/// `target − clamp(diff(base, target), ±max)`.
pub fn rotate_if_necessary(base: f32, target: f32, max: f32) -> f32 {
    let d = degrees_difference(base, target).clamp(-max, max);
    target - d
}

/// The rotation toward a target of the controls.
pub fn rotate_towards(from: f32, to: f32, max: f32) -> f32 {
    from + degrees_difference(from, to).clamp(-max, max)
}

/// `rotlerp` of a mob's orientation toward a point (without wrapping into [0, 360]).
pub fn rotlerp(a: f32, b: f32, max: f32) -> f32 {
    a + wrap(b - a).clamp(-max, max)
}

/// `rotlerp` of the movement control (result wrapped into [0, 360]).
pub fn rotlerp_360(a: f32, b: f32, max: f32) -> f32 {
    let mut r = rotlerp(a, b, max);
    if r < 0.0 {
        r += 360.0;
    } else if r > 360.0 {
        r -= 360.0;
    }
    r
}

/// Network byte angle.
pub fn pack_degrees(a: f32) -> i8 {
    (a * 256.0 / 360.0).floor() as i32 as i8
}

/// Ceiling of n/2.
pub fn reduced(n: i32) -> i32 {
    (n + 1).div_euclid(2)
}

/// Nearest quarter turn of the yaw: 0 south, 1 west, 2 north, 3 east. Computed in double like
/// the game (in single precision, a yaw one ulp below 45° would already fall to the west).
pub fn quarter(yaw: f32) -> usize {
    ((f64::from(yaw) / 90.0 + 0.5).floor() as i32 & 3) as usize
}

/// Horizontal direction of the yaw in steps (x, z). It is also the "movement" direction of
/// entities other than boats and minecarts: the game derives it from the orientation, never from the velocity.
pub fn facing(yaw: f32) -> (i32, i32) {
    [(0, 1), (-1, 0), (0, -1), (1, 0)][quarter(yaw)]
}

pub fn dist2(a: [f64; 3], b: [f64; 3]) -> f64 {
    (a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)
}

pub fn block_of(p: [f64; 3]) -> [i32; 3] {
    [p[0].floor() as i32, p[1].floor() as i32, p[2].floor() as i32]
}

#[cfg(test)]
mod tests {
    #[test]
    fn atan2_close_to_exact() {
        for &(y, x) in &[(1.0, 1.0), (-0.3, 2.0), (5.0, -0.1), (-1.0, -1.0), (0.0, 1.0), (2.0, 0.0)] {
            let a = super::atan2(y, x);
            assert!((a - f64::atan2(y, x)).abs() < 1e-6, "{y} {x} {a}");
        }
        assert_eq!(super::pack_degrees(-90.0), -64);
        assert_eq!(super::reduced(5), 3);
    }

    /// Yaw → direction: 0 south, 90 west, 180 north, −90 east; borders rounded up;
    /// one ulp below 45° stays south (the single-precision computation gave west).
    #[test]
    fn facing_from_yaw_in_double_precision() {
        use super::facing;
        assert_eq!(facing(0.0), (0, 1));
        assert_eq!(facing(90.0), (-1, 0));
        assert_eq!(facing(180.0), (0, -1));
        assert_eq!(facing(-90.0), (1, 0));
        assert_eq!(facing(270.0), (1, 0));
        assert_eq!(facing(45.0), (-1, 0));
        assert_eq!(facing(-135.0), (1, 0));
        let below = f32::from_bits(45.0f32.to_bits() - 1);
        assert_eq!(facing(below), (0, 1));
        assert_ne!(((below / 90.0 + 0.5).floor() as i32) & 3, 0, "single precision crosses the border");
    }

    /// The √2 factor of oriented draws is the promoted single-precision constant: near a
    /// cell border, truncation differs from the double root.
    #[test]
    fn rpos_dist_uses_single_precision_sqrt2() {
        assert_eq!(super::SQRT_OF_TWO, 1.4142135381698608);
        let t = 1.0 / std::f64::consts::SQRT_2;
        let d = super::rpos_dist(t, 0.0, 16.0);
        assert_eq!(d.floor(), 15.0, "{d}");
        assert!((t * 16.0 * std::f64::consts::SQRT_2) > 15.999_999_99);
        assert_eq!(super::rpos_dist(0.5, 2.0, 10.0), 6.0 * super::SQRT_OF_TWO);
    }
}
