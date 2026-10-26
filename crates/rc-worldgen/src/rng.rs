//! Random generators of terrain generation (spec generation-noise § 1): "legacy" 48-bit linear
//! congruential, Xoroshiro128++, seed drawn from a name (MD5) and positional factories.

use md5::{Digest, Md5};

const LCG_MULT: i64 = 0x5_DEEC_E66D;
const LCG_MASK: i64 = (1 << 48) - 1;
/// 2^-24 and 2^-53: units of the drawn floats.
const FLOAT_UNIT: f32 = 1.0 / (1u32 << 24) as f32;
const DOUBLE_UNIT: f64 = 1.0 / (1u64 << 53) as f64;

/// 48-bit linear congruential (§ 1.1).
#[derive(Debug, Clone)]
pub struct LegacyRandom {
    seed: i64,
    gaussian: Option<f64>,
}

impl LegacyRandom {
    pub fn new(seed: i64) -> Self {
        LegacyRandom { seed: (seed ^ LCG_MULT) & LCG_MASK, gaussian: None }
    }

    pub fn set_seed(&mut self, seed: i64) {
        self.seed = (seed ^ LCG_MULT) & LCG_MASK;
        self.gaussian = None;
    }

    pub fn next(&mut self, bits: u32) -> i32 {
        self.seed = self.seed.wrapping_mul(LCG_MULT).wrapping_add(0xB) & LCG_MASK;
        (self.seed >> (48 - bits)) as i32
    }

    pub fn next_int(&mut self) -> i32 {
        self.next(32)
    }

    pub fn next_int_bounded(&mut self, n: i32) -> i32 {
        assert!(n > 0, "positive bound required");
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

    pub fn next_long(&mut self) -> i64 {
        let hi = i64::from(self.next(32));
        let lo = i64::from(self.next(32));
        (hi << 32).wrapping_add(lo)
    }

    pub fn next_bool(&mut self) -> bool {
        self.next(1) != 0
    }

    pub fn next_float(&mut self) -> f32 {
        self.next(24) as f32 * FLOAT_UNIT
    }

    pub fn next_double(&mut self) -> f64 {
        let hi = i64::from(self.next(26));
        let lo = i64::from(self.next(27));
        ((hi << 27) + lo) as f64 * DOUBLE_UNIT
    }
}

/// Raw Xoroshiro128++ state (§ 1.2).
#[derive(Debug, Clone, Copy)]
pub struct Xoroshiro128 {
    lo: u64,
    hi: u64,
}

impl Xoroshiro128 {
    pub fn new(lo: i64, hi: i64) -> Self {
        let (mut lo, mut hi) = (lo as u64, hi as u64);
        if lo == 0 && hi == 0 {
            lo = 0x9E37_79B9_7F4A_7C15;
            hi = 0x6A09_E667_F3BC_C909;
        }
        Xoroshiro128 { lo, hi }
    }

    pub fn next_long(&mut self) -> i64 {
        let (l, h) = (self.lo, self.hi);
        let r = l.wrapping_add(h).rotate_left(17).wrapping_add(l);
        let h = h ^ l;
        self.lo = l.rotate_left(49) ^ h ^ (h << 21);
        self.hi = h.rotate_left(28);
        r as i64
    }
}

fn mix_stafford13(z: i64) -> i64 {
    let mut z = z as u64;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31)) as i64
}

/// 64-bit seed extended and mixed into a 128-bit state.
pub fn upgrade_seed(seed: i64) -> (i64, i64) {
    let lo = seed ^ 0x6A09_E667_F3BC_C909u64 as i64;
    let hi = lo.wrapping_add(0x9E37_79B9_7F4A_7C15u64 as i64);
    (mix_stafford13(lo), mix_stafford13(hi))
}

/// MD5 of the name: two halves read big-endian (§ 1.3).
pub fn seed_from_hash_of(name: &str) -> (i64, i64) {
    let h = Md5::digest(name.as_bytes());
    let lo = i64::from_be_bytes(h[0..8].try_into().unwrap_or_default());
    let hi = i64::from_be_bytes(h[8..16].try_into().unwrap_or_default());
    (lo, hi)
}

/// Legacy string hash (UTF-16 units, i32).
pub fn string_hash(s: &str) -> i32 {
    s.encode_utf16().fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(i32::from(c)))
}

/// Position hash (§ 1.4).
pub fn position_seed(x: i32, y: i32, z: i32) -> i64 {
    let mut s = i64::from(x.wrapping_mul(3_129_871)) ^ i64::from(z).wrapping_mul(116_129_781) ^ i64::from(y);
    s = s.wrapping_mul(s).wrapping_mul(42_317_861).wrapping_add(s.wrapping_mul(11));
    s >> 16
}

/// Random source: one of the two algorithms.
#[derive(Debug, Clone)]
pub enum Rng {
    Legacy(LegacyRandom),
    Xoro(Xoroshiro128, Option<f64>),
}

impl Rng {
    pub fn legacy(seed: i64) -> Self {
        Rng::Legacy(LegacyRandom::new(seed))
    }

    /// Xoroshiro from a 64-bit seed (mixed).
    pub fn xoroshiro(seed: i64) -> Self {
        let (lo, hi) = upgrade_seed(seed);
        Rng::Xoro(Xoroshiro128::new(lo, hi), None)
    }

    /// Xoroshiro from a raw state (not mixed).
    pub fn xoroshiro_raw(lo: i64, hi: i64) -> Self {
        Rng::Xoro(Xoroshiro128::new(lo, hi), None)
    }

    /// Algorithm chosen by `legacy_random_source`.
    pub fn for_world(seed: i64, legacy: bool) -> Self {
        if legacy { Rng::legacy(seed) } else { Rng::xoroshiro(seed) }
    }

    pub fn next_long(&mut self) -> i64 {
        match self {
            Rng::Legacy(r) => r.next_long(),
            Rng::Xoro(x, _) => x.next_long(),
        }
    }

    pub fn next_int(&mut self) -> i32 {
        match self {
            Rng::Legacy(r) => r.next_int(),
            Rng::Xoro(x, _) => x.next_long() as i32,
        }
    }

    pub fn next_int_bounded(&mut self, n: i32) -> i32 {
        match self {
            Rng::Legacy(r) => r.next_int_bounded(n),
            Rng::Xoro(x, _) => {
                assert!(n > 0, "positive bound required");
                let n64 = n as u64;
                let mut m = u64::from(x.next_long() as u32) * n64;
                let mut low = m & 0xFFFF_FFFF;
                if low < n64 {
                    let t = u64::from((n as u32).wrapping_neg() % (n as u32));
                    while low < t {
                        m = u64::from(x.next_long() as u32) * n64;
                        low = m & 0xFFFF_FFFF;
                    }
                }
                (m >> 32) as i32
            }
        }
    }

    pub fn next_bool(&mut self) -> bool {
        match self {
            Rng::Legacy(r) => r.next_bool(),
            Rng::Xoro(x, _) => x.next_long() & 1 != 0,
        }
    }

    pub fn next_float(&mut self) -> f32 {
        match self {
            Rng::Legacy(r) => r.next_float(),
            Rng::Xoro(x, _) => ((x.next_long() as u64) >> 40) as f32 * FLOAT_UNIT,
        }
    }

    pub fn next_double(&mut self) -> f64 {
        match self {
            Rng::Legacy(r) => r.next_double(),
            Rng::Xoro(x, _) => ((x.next_long() as u64) >> 11) as f64 * DOUBLE_UNIT,
        }
    }

    /// Gaussian (polar method, second value kept in reserve).
    pub fn next_gaussian(&mut self) -> f64 {
        let reserve = match self {
            Rng::Legacy(r) => &mut r.gaussian,
            Rng::Xoro(_, g) => g,
        };
        if let Some(v) = reserve.take() {
            return v;
        }
        loop {
            let x = 2.0 * self.next_double() - 1.0;
            let y = 2.0 * self.next_double() - 1.0;
            let r = x * x + y * y;
            if r < 1.0 && r != 0.0 {
                let k = (-2.0 * r.ln() / r).sqrt();
                match self {
                    Rng::Legacy(l) => l.gaussian = Some(y * k),
                    Rng::Xoro(_, g) => *g = Some(y * k),
                }
                return x * k;
            }
        }
    }

    /// Consumes `n` draws (legacy "integer", not Xoroshiro).
    pub fn consume(&mut self, n: u32) {
        for _ in 0..n {
            match self {
                Rng::Legacy(r) => {
                    r.next_int();
                }
                Rng::Xoro(x, _) => {
                    x.next_long();
                }
            }
        }
    }

    pub fn fork(&mut self) -> Rng {
        match self {
            Rng::Legacy(r) => Rng::legacy(r.next_long()),
            Rng::Xoro(x, _) => {
                let lo = x.next_long();
                let hi = x.next_long();
                Rng::xoroshiro_raw(lo, hi)
            }
        }
    }

    pub fn fork_positional(&mut self) -> Positional {
        match self {
            Rng::Legacy(r) => Positional::Legacy(r.next_long()),
            Rng::Xoro(x, _) => {
                let lo = x.next_long();
                let hi = x.next_long();
                Positional::Xoro(lo, hi)
            }
        }
    }
}

/// Positional factory (§ 1.4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Positional {
    Legacy(i64),
    Xoro(i64, i64),
}

impl Positional {
    pub fn at(&self, x: i32, y: i32, z: i32) -> Rng {
        let p = position_seed(x, y, z);
        match *self {
            Positional::Legacy(g) => Rng::legacy(p ^ g),
            Positional::Xoro(lo, hi) => Rng::xoroshiro_raw(p ^ lo, hi),
        }
    }

    pub fn from_hash_of(&self, name: &str) -> Rng {
        match *self {
            Positional::Legacy(g) => Rng::legacy(i64::from(string_hash(name)) ^ g),
            Positional::Xoro(lo, hi) => {
                let (a, b) = seed_from_hash_of(name);
                Rng::xoroshiro_raw(a ^ lo, b ^ hi)
            }
        }
    }

    pub fn from_seed(&self, seed: i64) -> Rng {
        match *self {
            Positional::Legacy(_) => Rng::legacy(seed),
            Positional::Xoro(lo, hi) => Rng::xoroshiro_raw(seed ^ lo, seed ^ hi),
        }
    }
}

/// Biome zoom seed: first 8 bytes (little-endian) of the SHA-256 of the seed
/// (8 little-endian bytes) — the hashed seed the client receives.
pub fn biome_zoom_seed(seed: i64) -> i64 {
    let d = ring::digest::digest(&ring::digest::SHA256, &seed.to_le_bytes());
    i64::from_le_bytes(d.as_ref()[0..8].try_into().unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_matches_classic_generator() {
        let mut r = LegacyRandom::new(0);
        assert_eq!(r.next_int(), -1_155_484_576);
        assert_eq!(LegacyRandom::new(42).next_double(), 0.727_563_680_032_868_1);
        // first long drawn from a Java linear congruential generator, seed 0
        assert_eq!(LegacyRandom::new(0).next_long(), -4_962_768_465_676_381_896);
    }

    #[test]
    fn string_hash_like_java() {
        assert_eq!(
            string_hash("minecraft:terrain"),
            "minecraft:terrain".encode_utf16().fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(c as i32))
        );
        assert_eq!(string_hash("a"), 97);
        assert_eq!(string_hash("ab"), 97 * 31 + 98);
    }
}
