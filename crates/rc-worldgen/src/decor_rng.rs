//! Decoration randomness (`generation-decoration.md` § 2.4): "generation" generator that wraps a
//! xoroshiro (or a linear congruential); `next(bits)` takes the high-order bits and all the draws
//! follow the historical formulas.

use crate::rng::{LegacyRandom, Xoroshiro128, upgrade_seed};

const FLOAT_UNIT: f32 = 1.0 / (1u32 << 24) as f32;
const DOUBLE_UNIT: f64 = 1.0 / (1u64 << 53) as f64;

#[derive(Debug, Clone)]
enum Inner {
    Xoro(Xoroshiro128),
    Legacy(LegacyRandom),
}

/// Generation generator.
#[derive(Debug, Clone)]
pub struct DecoRng {
    inner: Inner,
    gaussian: Option<f64>,
    /// Journal of the draws (bits, value) for debugging against the probe.
    pub trace: Option<Vec<(u32, i32)>>,
    /// Current label (debug) added to the journal bits (bits + 100·label).
    pub label: u32,
}

impl DecoRng {
    /// Wrapper around a xoroshiro (64-bit seed mixed).
    pub fn xoroshiro(seed: i64) -> Self {
        let (lo, hi) = upgrade_seed(seed);
        DecoRng { inner: Inner::Xoro(Xoroshiro128::new(lo, hi)), gaussian: None, trace: None, label: 0 }
    }

    /// Wrapper around a linear congruential.
    pub fn legacy(seed: i64) -> Self {
        DecoRng { inner: Inner::Legacy(LegacyRandom::new(seed)), gaussian: None, trace: None, label: 0 }
    }

    /// Reseeds the inside only (the Gaussian reserve is kept, like the game).
    pub fn set_seed(&mut self, seed: i64) {
        match &mut self.inner {
            Inner::Xoro(x) => {
                let (lo, hi) = upgrade_seed(seed);
                *x = Xoroshiro128::new(lo, hi);
            }
            Inner::Legacy(l) => l.set_seed(seed),
        }
    }

    #[inline]
    pub fn next(&mut self, bits: u32) -> i32 {
        let v = match &mut self.inner {
            Inner::Xoro(x) => ((x.next_long() as u64) >> (64 - bits)) as i32,
            Inner::Legacy(l) => l.next(bits),
        };
        if let Some(t) = &mut self.trace {
            t.push((bits + 100 * self.label, v));
        }
        v
    }

    #[inline]
    pub fn next_int(&mut self) -> i32 {
        self.next(32)
    }

    #[inline]
    pub fn next_int_bounded(&mut self, n: i32) -> i32 {
        assert!(n > 0, "positive bound required ({n})");
        if n & (n - 1) == 0 {
            return ((i64::from(n) * i64::from(self.next(31))) >> 31) as i32;
        }
        loop {
            let s = self.next(31);
            let m = s % n;
            if s.wrapping_sub(m).wrapping_add(n - 1) >= 0 {
                return m;
            }
        }
    }

    /// `min + next_int(max − min + 1)`.
    #[inline]
    pub fn between(&mut self, min: i32, max_inclusive: i32) -> i32 {
        self.next_int_bounded(max_inclusive - min + 1) + min
    }

    /// `origin + next_int(bound − origin)` (bound excluded; ≥ only if bound > origin).
    #[inline]
    pub fn range(&mut self, origin: i32, bound: i32) -> i32 {
        origin + self.next_int_bounded(bound - origin)
    }

    #[inline]
    pub fn next_long(&mut self) -> i64 {
        let hi = i64::from(self.next(32));
        let lo = i64::from(self.next(32));
        (hi << 32).wrapping_add(lo)
    }

    #[inline]
    pub fn next_bool(&mut self) -> bool {
        self.next(1) != 0
    }

    #[inline]
    pub fn next_float(&mut self) -> f32 {
        self.next(24) as f32 * FLOAT_UNIT
    }

    #[inline]
    pub fn next_double(&mut self) -> f64 {
        let hi = i64::from(self.next(26));
        let lo = i64::from(self.next(27));
        ((hi << 27) + lo) as f64 * DOUBLE_UNIT
    }

    pub fn next_gaussian(&mut self) -> f64 {
        if let Some(v) = self.gaussian.take() {
            return v;
        }
        loop {
            let x = 2.0 * self.next_double() - 1.0;
            let y = 2.0 * self.next_double() - 1.0;
            let r = x * x + y * y;
            if r < 1.0 && r != 0.0 {
                let k = (-2.0 * r.ln() / r).sqrt();
                self.gaussian = Some(y * k);
                return x * k;
            }
        }
    }

    /// Returns the decoration seed of the chunk (x, z in blocks).
    pub fn set_decoration_seed(&mut self, seed: i64, x: i32, z: i32) -> i64 {
        self.set_seed(seed);
        let a = self.next_long() | 1;
        let b = self.next_long() | 1;
        let r = (i64::from(x).wrapping_mul(a)).wrapping_add(i64::from(z).wrapping_mul(b)) ^ seed;
        self.set_seed(r);
        r
    }

    pub fn set_feature_seed(&mut self, seed: i64, index: i32, step: i32) {
        self.set_seed(seed.wrapping_add(i64::from(index)).wrapping_add(i64::from(10000i32.wrapping_mul(step))));
    }

    pub fn set_large_feature_seed(&mut self, seed: i64, cx: i32, cz: i32) {
        self.set_seed(seed);
        let a = self.next_long();
        let b = self.next_long();
        self.set_seed(i64::from(cx).wrapping_mul(a) ^ i64::from(cz).wrapping_mul(b) ^ seed);
    }

    /// Draw of a shuffled list "the Java way": for i = n..2, swap [i−1] and [next_int(i)].
    pub fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (2..=v.len()).rev() {
            let j = self.next_int_bounded(i as i32) as usize;
            v.swap(i - 1, j);
        }
    }
}

/// The game's sine table (65 536 single floats).
pub struct SinTable(Box<[f32; 65536]>);

impl SinTable {
    pub fn new() -> Self {
        let mut t = Box::new([0f32; 65536]);
        for (i, v) in t.iter_mut().enumerate() {
            *v = (i as f64 / 10430.378350470453).sin() as f32;
        }
        SinTable(t)
    }

    #[inline]
    pub fn sin(&self, x: f64) -> f32 {
        self.0[((x * 10430.378350470453) as i64 & 65535) as usize]
    }

    #[inline]
    pub fn cos(&self, x: f64) -> f32 {
        self.0[((x * 10430.378350470453 + 16384.0) as i64 & 65535) as usize]
    }
}

impl Default for SinTable {
    fn default() -> Self {
        Self::new()
    }
}
