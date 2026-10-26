//! Server-side block behaviours of the integrated server (`docs/specs/block-behaviours.md`): placement
//! with flags and notification scheduler (§ 1.8), scheduled, random and
//! block events (§ 1.3 to 1.7), redstone (§ 3), pistons (§ 3.9), fluids (§ 2.1), gravity,
//! growth and fire (§ 2), placement states (`interactions.md` § 6).
//!
//! Everything goes through `Server` (`impl Server` in each submodule): the world, queues and
//! randomness are the server's, in the game's sequential order (§ 4.3).

pub(crate) mod attach;
pub(crate) mod dispense_behaviors;
pub(crate) mod attack;
pub(crate) mod campfire;
pub(crate) mod conduit;
pub(crate) mod entities;
pub(crate) mod fire_table;
pub(crate) mod fluid;
pub(crate) mod growth;
pub(crate) mod harvest;
pub(crate) mod jukebox;
pub(crate) mod lectern;
pub(crate) mod shelf;
pub(crate) mod use_block;
pub(crate) mod hopper_items;
pub(crate) mod level;
pub(crate) mod moss_rules;
pub(crate) mod nature;
pub(crate) mod piston;
pub(crate) mod piston_plan;
pub(crate) mod placement;
pub(crate) mod plant_rules;
pub(crate) mod rails;
pub(crate) mod sculk_sensor;
pub(crate) mod redstone;
pub(crate) mod shape;
pub(crate) mod table;
pub(crate) mod ticks;

#[cfg(test)]
pub(crate) mod probe;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod g6f01_tests;
#[cfg(test)]
mod g5f09_tests;
#[cfg(test)]
mod g4f10_tests;
#[cfg(test)]
mod g6f10_tests;
#[cfg(test)]
mod g6f06_tests;
#[cfg(test)]
mod g6f12_tests;
#[cfg(test)]
mod g6f23_tests;

pub(crate) use table::{Kind, Table};

/// Block position.
pub(crate) type P = [i32; 3];

/// Direction, values in the game's order (down, up, north, south, west, east).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub(crate) enum Dir {
    Down = 0,
    Up = 1,
    North = 2,
    South = 3,
    West = 4,
    East = 5,
}

use Dir::*;

impl Dir {
    /// The game's value order.
    pub const ALL: [Dir; 6] = [Down, Up, North, South, West, East];
    /// Horizontal directions in plane order.
    pub const HORIZONTAL: [Dir; 4] = [North, East, South, West];
    /// Order of the "neighbour changed" updates (§ 1.8).
    pub const UPDATE_ORDER: [Dir; 6] = [West, East, Down, Up, North, South];
    /// Order of the shape updates (§ 1.8).
    pub const SHAPE_ORDER: [Dir; 6] = [West, East, North, South, Down, Up];

    pub fn from_index(i: usize) -> Dir {
        Dir::ALL[i % 6]
    }

    pub fn step(self) -> P {
        match self {
            Down => [0, -1, 0],
            Up => [0, 1, 0],
            North => [0, 0, -1],
            South => [0, 0, 1],
            West => [-1, 0, 0],
            East => [1, 0, 0],
        }
    }

    pub fn opposite(self) -> Dir {
        match self {
            Down => Up,
            Up => Down,
            North => South,
            South => North,
            West => East,
            East => West,
        }
    }

    /// Clockwise rotation seen from above (north → east → south → west); vertical axis unchanged.
    pub fn clockwise(self) -> Dir {
        match self {
            North => East,
            East => South,
            South => West,
            West => North,
            d => d,
        }
    }

    pub fn counter_clockwise(self) -> Dir {
        match self {
            North => West,
            West => South,
            South => East,
            East => North,
            d => d,
        }
    }

    /// 0 : x, 1 : y, 2 : z.
    pub fn axis(self) -> u8 {
        match self {
            West | East => 0,
            Down | Up => 1,
            North | South => 2,
        }
    }

    pub fn horizontal(self) -> bool {
        self.axis() != 1
    }

    pub fn name(self) -> &'static str {
        match self {
            Down => "down",
            Up => "up",
            North => "north",
            South => "south",
            West => "west",
            East => "east",
        }
    }

    pub fn parse(s: &str) -> Option<Dir> {
        Some(match s {
            "down" => Down,
            "up" => Up,
            "north" => North,
            "south" => South,
            "west" => West,
            "east" => East,
            _ => return None,
        })
    }

    pub fn step_x(self) -> i32 {
        self.step()[0]
    }

    pub fn step_z(self) -> i32 {
        self.step()[2]
    }
}

#[inline]
pub(crate) fn rel(p: P, d: Dir) -> P {
    let s = d.step();
    [p[0] + s[0], p[1] + s[1], p[2] + s[2]]
}

#[inline]
pub(crate) fn rel_n(p: P, d: Dir, n: i32) -> P {
    let s = d.step();
    [p[0] + s[0] * n, p[1] + s[1] * n, p[2] + s[2] * n]
}

#[inline]
pub(crate) fn add(p: P, dx: i32, dy: i32, dz: i32) -> P {
    [p[0] + dx, p[1] + dy, p[2] + dz]
}

#[inline]
pub(crate) fn above(p: P) -> P {
    [p[0], p[1] + 1, p[2]]
}

#[inline]
pub(crate) fn below(p: P) -> P {
    [p[0], p[1] - 1, p[2]]
}

/// Hash of a game position (32-bit signed integers, overflow): used to reproduce
/// the iteration order of the game's hashed tables (§ 3.3, § 3.9).
pub(crate) fn java_hash(p: P) -> i32 {
    p[1].wrapping_add(p[2].wrapping_mul(31)).wrapping_mul(31).wrapping_add(p[0])
}

/// The game's hashed table keyed by positions: insertion, growth and iteration order
/// identical (buckets of linked lists in insertion order; a list of 9 entries
/// grows the table while it has fewer than 64 buckets, as the game does, instead of making a tree).
pub(crate) struct JavaMap<V> {
    buckets: Vec<Vec<(P, V)>>,
    len: usize,
}

impl<V> JavaMap<V> {
    pub fn new() -> Self {
        JavaMap { buckets: (0..16).map(|_| Vec::new()).collect(), len: 0 }
    }

    fn spread(p: P) -> u32 {
        let h = java_hash(p) as u32;
        h ^ (h >> 16)
    }

    pub fn insert(&mut self, p: P, v: V) {
        let n = self.buckets.len();
        let i = (Self::spread(p) as usize) & (n - 1);
        if let Some(e) = self.buckets[i].iter_mut().find(|e| e.0 == p) {
            e.1 = v;
            return;
        }
        self.buckets[i].push((p, v));
        self.len += 1;
        let long_bin = self.buckets[i].len() >= 9 && n < 64;
        if self.len > n * 3 / 4 || long_bin {
            self.resize();
        }
    }

    fn resize(&mut self) {
        let n = self.buckets.len() * 2;
        let old = std::mem::take(&mut self.buckets);
        self.buckets = (0..n).map(|_| Vec::new()).collect();
        // The game splits each bucket into two lists, keeping the relative order.
        for b in old {
            for (p, v) in b {
                let i = (Self::spread(p) as usize) & (n - 1);
                self.buckets[i].push((p, v));
            }
        }
    }

    pub fn remove(&mut self, p: P) -> Option<V> {
        let n = self.buckets.len();
        let i = (Self::spread(p) as usize) & (n - 1);
        let k = self.buckets[i].iter().position(|e| e.0 == p)?;
        self.len -= 1;
        Some(self.buckets[i].remove(k).1)
    }

    pub fn iter(&self) -> impl Iterator<Item = &(P, V)> {
        self.buckets.iter().flatten()
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.len
    }
}

/// The game's random generator (48-bit linear congruential): the world's random `R` (§ 1.9).
#[derive(Debug, Clone)]
pub(crate) struct JavaRandom {
    seed: u64,
}

impl JavaRandom {
    const MUL: u64 = 0x5DEE_CE66D;

    pub fn new(seed: i64) -> Self {
        JavaRandom { seed: (seed as u64 ^ Self::MUL) & ((1 << 48) - 1) }
    }

    fn next(&mut self, bits: u32) -> i32 {
        self.seed = (self.seed.wrapping_mul(Self::MUL).wrapping_add(0xB)) & ((1 << 48) - 1);
        (self.seed >> (48 - bits)) as i32
    }

    pub fn next_int(&mut self) -> i32 {
        self.next(32)
    }

    pub fn next_int_bound(&mut self, bound: i32) -> i32 {
        if bound <= 0 {
            return 0;
        }
        if bound.isolate_lowest_one() == bound {
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

    pub fn next_float(&mut self) -> f32 {
        self.next(24) as f32 / (1 << 24) as f32
    }

    /// One bit drawn.
    pub fn next_bool(&mut self) -> bool {
        self.next(1) != 0
    }

    pub fn next_double(&mut self) -> f64 {
        let hi = i64::from(self.next(26)) << 27;
        (hi + i64::from(self.next(27))) as f64 * (1.0 / (1u64 << 53) as f64)
    }
}

/// Placement flags (§ 1.8).
pub(crate) mod flag {
    pub const NEIGHBORS: u32 = 1;
    pub const CLIENTS: u32 = 2;
    pub const INVISIBLE: u32 = 4;
    pub const KNOWN_SHAPE: u32 = 16;
    pub const NO_DROPS: u32 = 32;
    pub const MOVED: u32 = 64;
    pub const SKIP_WIRE_SHAPE: u32 = 128;
    pub const SKIP_BE_SIDE_EFFECTS: u32 = 256;
    pub const SKIP_ON_PLACE: u32 = 512;
    pub const ALL: u32 = NEIGHBORS | CLIENTS;
}
