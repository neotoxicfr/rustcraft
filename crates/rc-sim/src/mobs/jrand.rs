//! The game's "legacy" generator (48-bit linear congruential): each mob's random and the
//! world's random for everything touching mobs (`server-mobs.md` § 5.1, § 5.3).

const MULT: i64 = 0x5_DEEC_E66D;
const MASK: i64 = (1 << 48) - 1;

#[derive(Debug, Clone, Copy)]
pub struct JRand {
    seed: i64,
    next_gaussian: Option<f64>,
}

impl JRand {
    pub fn new(seed: i64) -> JRand {
        JRand { seed: (seed ^ MULT) & MASK, next_gaussian: None }
    }

    /// "Unique" seed drawn from the clock, like the game (not reproducible). In tests, a
    /// followed seed specific to the test thread (`crate::rng::test_seed`): same draws from one run to
    /// the next.
    pub fn from_time(salt: i64) -> JRand {
        #[cfg(test)]
        let n = crate::rng::test_seed();
        #[cfg(not(test))]
        let n = std::time::SystemTime::UNIX_EPOCH.elapsed().map_or(1, |d| d.as_nanos() as i64);
        JRand::new(n.wrapping_mul(0x5DEE_CE66).wrapping_add(salt.wrapping_mul(0x9E37_79B9)))
    }

    fn next(&mut self, bits: u32) -> i32 {
        self.seed = (self.seed.wrapping_mul(MULT).wrapping_add(0xB)) & MASK;
        (self.seed >> (48 - bits)) as i32
    }

    pub fn next_int(&mut self) -> i32 {
        self.next(32)
    }

    /// Bounded integer draw `[0, bound)` of Java (power of two, otherwise rejection).
    pub fn next_int_bound(&mut self, bound: i32) -> i32 {
        if bound <= 0 {
            return 0;
        }
        if bound & (bound - 1) == 0 {
            return ((i64::from(bound) * i64::from(self.next(31))) >> 31) as i32;
        }
        loop {
            let bits = self.next(31);
            let val = bits % bound;
            if bits.wrapping_sub(val).wrapping_add(bound - 1) >= 0 {
                return val;
            }
        }
    }

    /// The integer draw between two inclusive bounds.
    pub fn int_between(&mut self, a: i32, b: i32) -> i32 {
        self.next_int_bound(b - a + 1) + a
    }

    pub fn next_long(&mut self) -> i64 {
        let hi = i64::from(self.next(32));
        let lo = i64::from(self.next(32));
        (hi << 32).wrapping_add(lo)
    }

    pub fn next_bool(&mut self) -> bool {
        self.next(1) != 0
    }

    pub fn next_float(&mut self) -> f32 {
        self.next(24) as f32 * 5.960_464_5e-8
    }

    pub fn next_double(&mut self) -> f64 {
        let hi = i64::from(self.next(26));
        let lo = i64::from(self.next(27));
        ((hi << 27) + lo) as f64 * f64::from(1.110_223e-16f32)
    }

    /// Marsaglia polar Gaussian with reserve (like the game).
    pub fn next_gaussian(&mut self) -> f64 {
        if let Some(g) = self.next_gaussian.take() {
            return g;
        }
        loop {
            let a = 2.0 * self.next_double() - 1.0;
            let b = 2.0 * self.next_double() - 1.0;
            let s = a * a + b * b;
            if s < 1.0 && s != 0.0 {
                let m = (-2.0 * s.ln() / s).sqrt();
                self.next_gaussian = Some(b * m);
                return a * m;
            }
        }
    }

    /// `triangle(centre, spread)`.
    pub fn triangle(&mut self, mean: f64, spread: f64) -> f64 {
        mean + spread * (self.next_double() - self.next_double())
    }

    /// The game's Fisher-Yates shuffle: from the end to the start.
    pub fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            let j = self.next_int_bound(i as i32 + 1) as usize;
            v.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::JRand;

    #[test]
    fn matches_java_util_random() {
        // Values of a Java linear congruential generator with seed 42.
        let mut r = JRand::new(42);
        assert_eq!(r.next_int(), -1_170_105_035);
        assert_eq!(r.next_int_bound(10), 3);
        let mut r = JRand::new(0);
        assert_eq!(r.next_int_bound(100), 60);
        assert_eq!(r.next_double(), 0.831_440_988_787_061_2);
    }
}
