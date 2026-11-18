//! 48-bit linear congruential generator, same outputs as the classic Java generator (spec § 4.4).

const MULT: i64 = 0x5_DEEC_E66D;
const MASK: i64 = (1 << 48) - 1;

#[derive(Debug, Clone)]
pub struct JavaRandom {
    seed: i64,
}

impl JavaRandom {
    pub fn new(seed: i64) -> Self {
        JavaRandom { seed: (seed ^ MULT) & MASK }
    }

    /// Non-deterministic seed (music, interface, ambience).
    pub fn from_entropy() -> Self {
        use std::sync::atomic::{AtomicI64, Ordering};
        static UNIQ: AtomicI64 = AtomicI64::new(8_682_522_807_148_012);
        let u = UNIQ.fetch_add(181_783_497_276_652_981, Ordering::Relaxed);
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos() as i64);
        JavaRandom::new(u ^ t)
    }

    pub fn next(&mut self, bits: u32) -> i32 {
        self.seed = (self.seed.wrapping_mul(MULT).wrapping_add(0xB)) & MASK;
        (self.seed >> (48 - bits)) as i32
    }

    pub fn next_int(&mut self) -> i32 {
        self.next(32)
    }

    /// Uniform integer in [0, n), n > 0.
    pub fn next_int_bounded(&mut self, n: i32) -> i32 {
        debug_assert!(n > 0);
        if n & (n - 1) == 0 {
            return ((i64::from(n) * i64::from(self.next(31))) >> 31) as i32;
        }
        loop {
            let x = self.next(31);
            let m = x % n;
            if x.wrapping_sub(m).wrapping_add(n - 1) >= 0 {
                return m;
            }
        }
    }

    /// Double in [0, 1): 26 then 27 bits, over 2^53.
    pub fn next_double(&mut self) -> f64 {
        let hi = i64::from(self.next(26) as u32);
        let lo = i64::from(self.next(27) as u32);
        ((hi << 27) + lo) as f64 / (1u64 << 53) as f64
    }

    pub fn next_float(&mut self) -> f32 {
        self.next(24) as f32 / (1 << 24) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_double_matches_java() {
        // first double drawn from a Java linear congruential generator, seed 42
        assert_eq!(JavaRandom::new(42).next_double(), 0.727_563_680_032_868_1);
    }

    // Reference values of the Java generator.
    #[test]
    fn reference_values() {
        assert_eq!(JavaRandom::new(0).next_int(), -1_155_484_576);
        assert_eq!(JavaRandom::new(42).next_int(), -1_170_105_035);
        assert_eq!(JavaRandom::new(42).next_int_bounded(10), 0);
        assert_eq!(JavaRandom::new(0).next_int_bounded(100), 60);
        assert!((JavaRandom::new(0).next_float() - 0.730_967_8).abs() < 1e-6);
        let mut r = JavaRandom::new(0);
        let seq: Vec<i32> = (0..5).map(|_| r.next_int()).collect();
        assert_eq!(seq, [-1_155_484_576, -723_955_400, 1_033_096_058, -1_690_734_402, -1_557_280_266]);
    }

    #[test]
    fn bounded_power_of_two() {
        // Power-of-2 branch: (n × next(31)) >> 31 with next(31) = -1155484576 >>> 1.
        let mut r = JavaRandom::new(0);
        let x = (-1_155_484_576i32 as u32 >> 1) as i64;
        assert_eq!(r.next_int_bounded(16), ((16 * x) >> 31) as i32);
    }
}
