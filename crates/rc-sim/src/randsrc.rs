//! Common random source (S7): loot, enchanting and menus draw either from the
//! game's Xoroshiro generator (named sequences, level random), or from the 48-bit
//! linear congruential generator (a chest's loot seed, the player's enchanting seed). The two
//! keep their own formulas (`next_int(n)` differs between them).

use crate::mobs::jrand::JRand;
use crate::rng::Rng;

pub trait Rand {
    fn next_int(&mut self) -> i32;
    /// `[0, bound)`.
    fn next_int_bound(&mut self, bound: i32) -> i32;
    fn next_long(&mut self) -> i64;
    fn next_float(&mut self) -> f32;
    fn next_double(&mut self) -> f64;
    fn next_bool(&mut self) -> bool;
    fn next_gaussian(&mut self) -> f64;
    /// `[min, max]` inclusive, `min` if the interval is empty (the game's utility form).
    fn between(&mut self, min: i32, max: i32) -> i32 {
        if min >= max { min } else { min + self.next_int_bound(max - min + 1) }
    }
    /// The game's shuffle: from the end to the start, swapping with an index drawn in `[0, i]`.
    fn shuffle<T>(&mut self, v: &mut [T])
    where
        Self: Sized,
    {
        for i in (1..v.len()).rev() {
            let j = self.next_int_bound(i as i32 + 1) as usize;
            v.swap(i, j);
        }
    }
}

/// Shuffle on a dynamic source.
pub fn shuffle_dyn<T>(r: &mut dyn Rand, v: &mut [T]) {
    for i in (1..v.len()).rev() {
        let j = r.next_int_bound(i as i32 + 1) as usize;
        v.swap(i, j);
    }
}

impl Rand for Rng {
    fn next_int(&mut self) -> i32 {
        Rng::next_int(self)
    }
    fn next_int_bound(&mut self, bound: i32) -> i32 {
        Rng::next_int_bound(self, bound)
    }
    fn next_long(&mut self) -> i64 {
        Rng::next_long(self)
    }
    fn next_float(&mut self) -> f32 {
        Rng::next_float(self)
    }
    fn next_double(&mut self) -> f64 {
        Rng::next_double(self)
    }
    fn next_bool(&mut self) -> bool {
        Rng::next_bool(self)
    }
    fn next_gaussian(&mut self) -> f64 {
        Rng::next_gaussian(self)
    }
}

impl Rand for JRand {
    fn next_int(&mut self) -> i32 {
        JRand::next_int(self)
    }
    fn next_int_bound(&mut self, bound: i32) -> i32 {
        JRand::next_int_bound(self, bound)
    }
    fn next_long(&mut self) -> i64 {
        JRand::next_long(self)
    }
    fn next_float(&mut self) -> f32 {
        JRand::next_float(self)
    }
    fn next_double(&mut self) -> f64 {
        JRand::next_double(self)
    }
    fn next_bool(&mut self) -> bool {
        JRand::next_bool(self)
    }
    fn next_gaussian(&mut self) -> f64 {
        JRand::next_gaussian(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reference values of a Java linear congruential generator with seed 42.
    #[test]
    fn legacy_matches_java() {
        let mut r = JRand::new(42);
        assert_eq!(Rand::next_int_bound(&mut r, 10), 0);
        assert_eq!(Rand::next_int_bound(&mut r, 10), 3);
        let mut r = JRand::new(42);
        assert_eq!(Rand::next_int(&mut r), -1_170_105_035);
    }
}
