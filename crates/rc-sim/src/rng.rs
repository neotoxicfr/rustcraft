//! Server random (`server-gameplay.md` § 5.5): the game's Xoroshiro128++ generator, named
//! random sequences of the world (`random_sequences.dat`, resumed and saved so that
//! loot follows the official game's on the same world).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use rc_proto::nbt::Nbt;

use crate::nbtio::{self, int, set};

const GOLDEN: u64 = 0x9E37_79B9_7F4A_7C15;
const SILVER: u64 = 0x6A09_E667_F3BC_C909;

fn mix(mut z: u64) -> u64 {
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

/// Xoroshiro128++ generator and its draws (same formulas as the game).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rng {
    lo: u64,
    hi: u64,
}

impl Rng {
    pub fn from_parts(lo: u64, hi: u64) -> Rng {
        if lo | hi == 0 { Rng { lo: GOLDEN, hi: SILVER } } else { Rng { lo, hi } }
    }

    /// 64-bit seed expanded then mixed.
    pub fn new(seed: i64) -> Rng {
        let lo = (seed as u64) ^ SILVER;
        let hi = lo.wrapping_add(GOLDEN);
        Rng::from_parts(mix(lo), mix(hi))
    }

    /// Seed drawn from the clock (level random, not reproducible as in the game). In tests,
    /// followed seed specific to the test thread (`test_seed`).
    pub fn from_time() -> Rng {
        #[cfg(test)]
        let n = test_seed();
        #[cfg(not(test))]
        let n = std::time::SystemTime::UNIX_EPOCH.elapsed().map_or(1, |d| d.as_nanos() as i64);
        Rng::new(n ^ 0x0123_4567_89AB_CDEFi64)
    }

    pub fn parts(&self) -> (u64, u64) {
        (self.lo, self.hi)
    }

    pub fn next_long(&mut self) -> i64 {
        let (s0, mut s1) = (self.lo, self.hi);
        let r = s0.wrapping_add(s1).rotate_left(17).wrapping_add(s0);
        s1 ^= s0;
        self.lo = s0.rotate_left(49) ^ s1 ^ (s1 << 21);
        self.hi = s1.rotate_left(28);
        r as i64
    }

    pub fn next_int(&mut self) -> i32 {
        self.next_long() as i32
    }

    /// Uniform integer in `[0, bound)` (the game's Lemire method).
    pub fn next_int_bound(&mut self, bound: i32) -> i32 {
        if bound <= 0 {
            return 0;
        }
        let b = bound as u64;
        let mut m = u64::from(self.next_int() as u32) * b;
        let mut frac = m & 0xFFFF_FFFF;
        if frac < b {
            let start = (bound.wrapping_neg() as u32 % bound as u32) as u64;
            while frac < start {
                m = u64::from(self.next_int() as u32) * b;
                frac = m & 0xFFFF_FFFF;
            }
        }
        (m >> 32) as i32
    }

    /// `[min, max]` inclusive.
    pub fn int_between(&mut self, min: i32, max: i32) -> i32 {
        if min >= max { min } else { min + self.next_int_bound(max - min + 1) }
    }

    fn bits(&mut self, n: u32) -> u64 {
        (self.next_long() as u64) >> (64 - n)
    }

    pub fn next_float(&mut self) -> f32 {
        self.bits(24) as f32 * 5.960_464_5e-8
    }

    pub fn next_double(&mut self) -> f64 {
        self.bits(53) as f64 * f64::from(1.110_223e-16f32)
    }

    pub fn next_bool(&mut self) -> bool {
        self.next_long() & 1 != 0
    }

    /// Gaussian draw (Marsaglia polar method, no reserve kept between calls).
    pub fn next_gaussian(&mut self) -> f64 {
        loop {
            let a = 2.0 * self.next_double() - 1.0;
            let b = 2.0 * self.next_double() - 1.0;
            let s = a * a + b * b;
            if s < 1.0 && s != 0.0 {
                return a * (-2.0 * s.ln() / s).sqrt();
            }
        }
    }

    /// The game's `triangle(centre, spread)`: centre + spread × (u − v).
    pub fn triangle(&mut self, mean: f64, spread: f64) -> f64 {
        mean + spread * (self.next_double() - self.next_double())
    }
}

/// MD5 (RFC 1321): seed of a sequence drawn from its name.
pub fn md5(input: &[u8]) -> [u8; 16] {
    const S: [u32; 64] = [
        7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14,
        20, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6,
        10, 15, 21,
    ];
    let k: Vec<u32> = (0..64).map(|i| ((i as f64 + 1.0).sin().abs() * 4_294_967_296.0) as u32).collect();
    let mut msg = input.to_vec();
    let bit_len = (input.len() as u64).wrapping_mul(8);
    msg.push(0x80);
    while msg.len() % 64 != 56 {
        msg.push(0);
    }
    msg.extend_from_slice(&bit_len.to_le_bytes());
    let (mut a0, mut b0, mut c0, mut d0) = (0x6745_2301u32, 0xefcd_ab89u32, 0x98ba_dcfeu32, 0x1032_5476u32);
    for chunk in msg.chunks(64) {
        let m: Vec<u32> = chunk.chunks(4).map(|w| u32::from_le_bytes([w[0], w[1], w[2], w[3]])).collect();
        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);
        for i in 0..64 {
            let (f, g) = match i / 16 {
                0 => ((b & c) | (!b & d), i),
                1 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                2 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };
            let f = f.wrapping_add(a).wrapping_add(k[i]).wrapping_add(m[g]);
            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }
        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }
    let mut out = [0u8; 16];
    for (i, v) in [a0, b0, c0, d0].iter().enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(&v.to_le_bytes());
    }
    out
}

/// Named random sequences of a world (`<world>/data/minecraft/random_sequences.dat`):
/// a single set for the whole server, shared by all dimensions.
#[derive(Debug, Clone)]
pub struct Sequences {
    path: PathBuf,
    world_seed: i64,
    salt: i32,
    include_world_seed: bool,
    include_id: bool,
    seqs: BTreeMap<String, Rng>,
    pub dirty: bool,
}

impl Sequences {
    pub fn load(world_dir: &Path, world_seed: i64) -> Sequences {
        let path = world_dir.join("data").join("minecraft").join("random_sequences.dat");
        let root = nbtio::read_dat(&path).ok();
        let d = root.as_ref().and_then(|r| r.get("data"));
        let mut seqs = BTreeMap::new();
        if let Some(Nbt::Compound(f)) = d.and_then(|d| d.get("sequences")) {
            for (k, v) in f {
                if let Some(Nbt::LongArray(a)) = v.get("source")
                    && a.len() == 2
                {
                    seqs.insert(k.clone(), Rng::from_parts(a[0] as u64, a[1] as u64));
                }
            }
        }
        let flag = |k: &str| d.and_then(|d| int(d, k)).is_none_or(|v| v != 0);
        Sequences {
            path,
            world_seed,
            salt: d.and_then(|d| int(d, "salt")).unwrap_or(0) as i32,
            include_world_seed: flag("include_world_seed"),
            include_id: flag("include_sequence_id"),
            seqs,
            dirty: false,
        }
    }

    /// Generator of a sequence (created on first use).
    pub fn get(&mut self, key: &str) -> &mut Rng {
        self.dirty = true;
        let (ws, salt, wsf, idf) = (self.world_seed, self.salt, self.include_world_seed, self.include_id);
        self.seqs.entry(key.to_owned()).or_insert_with(|| Self::fresh(key, ws, salt, wsf, idf))
    }

    /// New generator of a sequence: world seed (or 0) mixed with the salt, then with the md5 of the name.
    fn fresh(key: &str, world_seed: i64, salt: i32, with_world: bool, with_id: bool) -> Rng {
        let seed = (if with_world { world_seed } else { 0 }) ^ i64::from(salt);
        let mut lo = (seed as u64) ^ SILVER;
        let mut hi = lo.wrapping_add(GOLDEN);
        if with_id {
            let h = md5(key.as_bytes());
            lo ^= u64::from_be_bytes(h[..8].try_into().unwrap_or_default());
            hi ^= u64::from_be_bytes(h[8..].try_into().unwrap_or_default());
        }
        Rng::from_parts(mix(lo), mix(hi))
    }

    /// `/random reset <id>`: the sequence restarts from its seed, with the default settings or
    /// the given ones (`Some((salt, world seed, name))`). Like the game, does not mark the file
    /// for rewrite: it will be at the next draw.
    pub fn reset(&mut self, key: &str, custom: Option<(i32, bool, bool)>) {
        let (salt, wsf, idf) = custom.unwrap_or((self.salt, self.include_world_seed, self.include_id));
        self.seqs.insert(key.to_owned(), Self::fresh(key, self.world_seed, salt, wsf, idf));
    }

    /// `/random reset *`: all sequences forgotten (count returned); with settings, they
    /// become the defaults of the sequences created afterwards.
    pub fn clear(&mut self, defaults: Option<(i32, bool, bool)>) -> usize {
        if let Some((salt, wsf, idf)) = defaults {
            self.salt = salt;
            self.include_world_seed = wsf;
            self.include_id = idf;
        }
        let n = self.seqs.len();
        self.seqs.clear();
        n
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// File to write (`None` if nothing changed).
    pub fn to_nbt(&mut self) -> Option<Nbt> {
        if !self.dirty {
            return None;
        }
        self.dirty = false;
        let mut root = nbtio::read_dat(&self.path).unwrap_or(Nbt::Compound(Vec::new()));
        let seqs = self
            .seqs
            .iter()
            .map(|(k, r)| {
                let (lo, hi) = r.parts();
                (k.clone(), Nbt::Compound(vec![("source".into(), Nbt::LongArray(vec![lo as i64, hi as i64]))]))
            })
            .collect();
        let data = Nbt::Compound(vec![
            ("salt".into(), Nbt::Int(self.salt)),
            ("include_world_seed".into(), Nbt::Byte(i8::from(self.include_world_seed))),
            ("include_sequence_id".into(), Nbt::Byte(i8::from(self.include_id))),
            ("sequences".into(), Nbt::Compound(seqs)),
        ]);
        set(&mut root, "data", data);
        set(&mut root, "DataVersion", Nbt::Int(crate::chunk::DATA_VERSION));
        Some(root)
    }
}

/// Tests: "clock" seeds replaced by a fixed sequence specific to the thread (each server,
/// generator or probe has its own thread): mobs and the level draw the same numbers from one
/// run to the next. Without this, a step-by-step test passed or failed depending on the clock
/// (snow golem missing all its snowballs, vex not hitting in the allotted time).
#[cfg(test)]
pub(crate) fn test_seed() -> i64 {
    use std::cell::Cell;
    thread_local! {
        static NEXT: Cell<i64> = const { Cell::new(0x5EED_0000) };
    }
    NEXT.with(|c| {
        let v = c.get();
        c.set(v.wrapping_add(0x1E37_79B9_7F4A_7C15));
        v
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn md5_reference() {
        let h = md5(b"abc");
        assert_eq!(h[..4], [0x90, 0x01, 0x50, 0x98]);
        assert_eq!(md5(b"")[..4], [0xd4, 0x1d, 0x8c, 0xd9]);
    }

    #[test]
    fn bounded_ints_in_range() {
        let mut r = Rng::new(42);
        for b in [1, 2, 3, 7, 100, 1 << 30] {
            for _ in 0..1000 {
                let v = r.next_int_bound(b);
                assert!((0..b).contains(&v));
            }
        }
        let f = r.next_float();
        assert!((0.0..1.0).contains(&f));
    }

    /// A single sequences file, at the world root; `reset` and `clear` make them restart.
    #[test]
    fn sequences_reset_and_clear() {
        let tmp = std::env::temp_dir().join("rc-seq-reset");
        let mut s = Sequences::load(&tmp, 6_470_889_051_986_534_830);
        assert_eq!(s.path(), tmp.join("data").join("minecraft").join("random_sequences.dat"));
        let draw =
            |s: &mut Sequences| -> Vec<i32> { (0..3).map(|_| s.get("minecraft:s8test").next_int_bound(95)).collect() };
        assert_eq!(draw(&mut s), vec![7, 59, 47]);
        s.reset("minecraft:s8test", None);
        assert_eq!(draw(&mut s), vec![7, 59, 47]);
        let _ = s.get("minecraft:other");
        assert_eq!(s.clear(None), 2);
        assert_eq!(draw(&mut s), vec![7, 59, 47]);
        s.reset("minecraft:s8test", Some((5, false, true)));
        let salted = draw(&mut s);
        assert_ne!(salted, vec![7, 59, 47]);
        assert_eq!(s.clear(Some((5, false, true))), 1);
        assert_eq!(draw(&mut s), salted);
    }
}
