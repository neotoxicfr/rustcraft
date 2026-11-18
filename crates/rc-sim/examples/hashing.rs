// Direct read allowed (example: setting read at launch); the game goes through rc_data::env_*!.
#![allow(clippy::disallowed_methods)]
//! Bench of the integrated server's chunk tables (key = chunk position `(x, z)`).
//!
//! Compares, for the same table of `(2R+1)²` chunks loaded around the player:
//! - `fold`: hashbrown + foldhash (current);
//! - `fx`: hashbrown + FxHash-style multiplicative hash (two words);
//! - `fxp`: FxHash of a single packed 64-bit key;
//! - `ident`: packed 64-bit key used as is (identity hash);
//! - `window`: toroidal 2^k × 2^k array indexed directly (checked key), fx fallback if
//!   the slot is taken by a distant chunk.
//!
//! Loads: `lookup` (positions drawn at random in the area, like `block_at`),
//! `neighbours` (9 lookups per loaded chunk, like the "ticking" filter), `walk`
//! (the player advances: one row inserted, one removed). Reads measured on 1 thread, then
//! 2, 4, … up to the number of logical cores (`available_parallelism`, `RUSTCRAFT_THREADS`
//! to cap). Output: ns per operation (median of 5 runs).
//!
//! `cargo run --release -p rc-sim --example hashing`

use std::hash::{BuildHasher, Hasher};
use std::hint::black_box;
use std::time::Instant;

type Key = (i32, i32);

#[derive(Clone, Copy, Default)]
struct FxBuild;
#[derive(Default)]
struct Fx(u64);
const SEED: u64 = 0x51_7c_c1_b7_27_22_0a_95;
impl Hasher for Fx {
    fn write(&mut self, b: &[u8]) {
        for c in b.chunks(8) {
            let mut w = [0u8; 8];
            w[..c.len()].copy_from_slice(c);
            self.write_u64(u64::from_le_bytes(w));
        }
    }
    fn write_u32(&mut self, v: u32) {
        self.write_u64(u64::from(v));
    }
    fn write_i32(&mut self, v: i32) {
        self.write_u64(u64::from(v as u32));
    }
    fn write_u64(&mut self, v: u64) {
        self.0 = (self.0.rotate_left(5) ^ v).wrapping_mul(SEED);
    }
    fn finish(&self) -> u64 {
        self.0
    }
}
impl BuildHasher for FxBuild {
    type Hasher = Fx;
    fn build_hasher(&self) -> Fx {
        Fx(0)
    }
}

#[derive(Clone, Copy, Default)]
struct IdBuild;
#[derive(Default)]
struct Id(u64);
impl Hasher for Id {
    fn write(&mut self, _: &[u8]) {
        unreachable!()
    }
    fn write_u64(&mut self, v: u64) {
        self.0 = v;
    }
    fn finish(&self) -> u64 {
        self.0
    }
}
impl BuildHasher for IdBuild {
    type Hasher = Id;
    fn build_hasher(&self) -> Id {
        Id(0)
    }
}

fn pack(k: Key) -> u64 {
    (u64::from(k.0 as u32) << 32) | u64::from(k.1 as u32)
}

/// Abstract chunk table (value = one word, like a chunk box).
trait Table: Sync {
    fn get(&self, k: Key) -> Option<u64>;
    fn insert(&mut self, k: Key, v: u64);
    fn remove(&mut self, k: Key);
}

struct Map<S>(hashbrown::HashMap<Key, u64, S>);
impl<S: BuildHasher + Sync> Table for Map<S> {
    #[inline]
    fn get(&self, k: Key) -> Option<u64> {
        self.0.get(&k).copied()
    }
    fn insert(&mut self, k: Key, v: u64) {
        self.0.insert(k, v);
    }
    fn remove(&mut self, k: Key) {
        self.0.remove(&k);
    }
}

struct Packed<S>(hashbrown::HashMap<u64, u64, S>);
impl<S: BuildHasher + Sync> Table for Packed<S> {
    #[inline]
    fn get(&self, k: Key) -> Option<u64> {
        self.0.get(&pack(k)).copied()
    }
    fn insert(&mut self, k: Key, v: u64) {
        self.0.insert(pack(k), v);
    }
    fn remove(&mut self, k: Key) {
        self.0.remove(&pack(k));
    }
}

/// Toroidal window: slot `(x & m, z & m)`, checked key; overflow goes into an fx table.
struct Window {
    bits: u32,
    cells: Vec<Option<(Key, u64)>>,
    over: hashbrown::HashMap<Key, u64, FxBuild>,
}
impl Window {
    fn new(r: i32) -> Window {
        let bits = (2 * r as u32 + 3).next_power_of_two().trailing_zeros();
        Window { bits, cells: vec![None; 1 << (2 * bits)], over: Default::default() }
    }
    #[inline]
    fn idx(&self, k: Key) -> usize {
        let m = (1 << self.bits) - 1;
        (((k.0 & m) as usize) << self.bits) | (k.1 & m) as usize
    }
}
impl Table for Window {
    #[inline]
    fn get(&self, k: Key) -> Option<u64> {
        match self.cells[self.idx(k)] {
            Some((c, v)) if c == k => Some(v),
            _ if self.over.is_empty() => None,
            _ => self.over.get(&k).copied(),
        }
    }
    fn insert(&mut self, k: Key, v: u64) {
        let i = self.idx(k);
        match &mut self.cells[i] {
            Some((c, x)) if *c == k => *x = v,
            Some(_) => {
                self.over.insert(k, v);
            }
            e @ None => *e = Some((k, v)),
        }
    }
    fn remove(&mut self, k: Key) {
        let i = self.idx(k);
        if matches!(self.cells[i], Some((c, _)) if c == k) {
            self.cells[i] = None;
            // An overflow into the same slot takes over the place.
            let mine: Vec<Key> = self.over.keys().copied().filter(|&o| self.idx(o) == i).collect();
            if let Some(&o) = mine.first() {
                let v = self.over.remove(&o).unwrap_or(0);
                self.cells[i] = Some((o, v));
            }
        } else {
            self.over.remove(&k);
        }
    }
}

/// The client's circular grid (`rc_world::ChunkGrid`): slot `(x mod N, z mod N)`,
/// N = 2R+3 (not a power of two: the eviction slot follows the game's cache). `fm`:
/// same slot computed with Lemire's fast modulo (multiplication) instead of division.
struct Grid {
    n: i32,
    m: u64,
    fm: bool,
    cells: Vec<Option<(Key, u64)>>,
}
impl Grid {
    fn new(r: i32, fm: bool) -> Grid {
        let n = 2 * r + 3;
        Grid { n, m: u64::MAX / n as u64 + 1, fm, cells: vec![None; (n * n) as usize] }
    }
    #[inline]
    fn modn(&self, x: i32) -> usize {
        if self.fm { fastmod(x, self.n, self.m) } else { x.rem_euclid(self.n) as usize }
    }
    #[inline]
    fn idx(&self, k: Key) -> usize {
        self.modn(k.0) * self.n as usize + self.modn(k.1)
    }
}
/// Exact `x.rem_euclid(n)`: positive offset that is a multiple of `n`, then Lemire's modulo on 32
/// bits; outside ±2^22 (absurd coordinates), plain division.
#[inline]
fn fastmod(x: i32, n: i32, m: u64) -> usize {
    const LIM: i32 = 1 << 22;
    if !(-LIM..LIM).contains(&x) {
        return x.rem_euclid(n) as usize;
    }
    let off = (LIM / n + 1) * n;
    let a = (x + off) as u32;
    let low = m.wrapping_mul(u64::from(a));
    ((u128::from(low) * n as u128) >> 64) as usize
}
impl Table for Grid {
    #[inline]
    fn get(&self, k: Key) -> Option<u64> {
        match self.cells[self.idx(k)] {
            Some((c, v)) if c == k => Some(v),
            _ => None,
        }
    }
    fn insert(&mut self, k: Key, v: u64) {
        let i = self.idx(k);
        self.cells[i] = Some((k, v));
    }
    fn remove(&mut self, k: Key) {
        let i = self.idx(k);
        if matches!(self.cells[i], Some((c, _)) if c == k) {
            self.cells[i] = None;
        }
    }
}

fn fill(t: &mut dyn Table, cx: i32, cz: i32, r: i32) {
    for x in cx - r..=cx + r {
        for z in cz - r..=cz + r {
            t.insert((x, z), pack((x, z)) ^ 0x55);
        }
    }
}

struct Rng(u64);
impl Rng {
    #[inline]
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

const CX: i32 = 100;
const CZ: i32 = -28;

/// ns per random lookup (loaded area + edge of an unloaded chunk), `threads` threads.
fn lookups(t: &dyn Table, r: i32, threads: usize, n: usize) -> f64 {
    let side = (2 * r + 3) as u64;
    let t0 = Instant::now();
    std::thread::scope(|s| {
        for th in 0..threads {
            s.spawn(move || {
                let mut g = Rng(0x9E37_79B9_7F4A_7C15 ^ ((th as u64 + 1) * 0x1_2345_6789));
                let mut acc = 0u64;
                for _ in 0..n {
                    let v = g.next();
                    let x = CX - r - 1 + (v % side) as i32;
                    let z = CZ - r - 1 + ((v >> 32) % side) as i32;
                    acc = acc.wrapping_add(t.get((x, z)).unwrap_or(1));
                }
                black_box(acc);
            });
        }
    });
    // Wall time per operation and per thread: constant if scaling is perfect.
    t0.elapsed().as_nanos() as f64 / n as f64
}

/// ns per lookup, 9 neighbours of each loaded chunk (row order), `threads` threads.
fn neighbours(t: &dyn Table, r: i32, threads: usize, rounds: usize) -> f64 {
    let t0 = Instant::now();
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(move || {
                let mut acc = 0u64;
                for _ in 0..rounds {
                    for x in CX - r..=CX + r {
                        for z in CZ - r..=CZ + r {
                            for dx in -1..=1 {
                                for dz in -1..=1 {
                                    acc += u64::from(t.get((x + dx, z + dz)).is_some());
                                }
                            }
                        }
                    }
                }
                black_box(acc);
            });
        }
    });
    let n = (2 * r + 1).pow(2) as usize * 9 * rounds;
    t0.elapsed().as_nanos() as f64 / n as f64
}

/// ns per walk step (one row inserted, one removed), one thread.
fn walk(t: &mut dyn Table, r: i32, steps: i32) -> f64 {
    let t0 = Instant::now();
    for s in 0..steps {
        let x_new = CX + r + 1 + s;
        let x_old = CX - r + s;
        for z in CZ - r..=CZ + r {
            t.insert((x_new, z), 7);
        }
        for z in CZ - r..=CZ + r {
            t.remove((x_old, z));
        }
    }
    t0.elapsed().as_nanos() as f64 / steps as f64
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(f64::total_cmp);
    v[v.len() / 2]
}

fn make(kind: &str, r: i32) -> Box<dyn Table> {
    let mut t: Box<dyn Table> = match kind {
        "fold" => Box::new(Map(hashbrown::HashMap::<Key, u64>::default())),
        "fx" => Box::new(Map(hashbrown::HashMap::<Key, u64, FxBuild>::default())),
        "fxp" => Box::new(Packed(hashbrown::HashMap::<u64, u64, FxBuild>::default())),
        "ident" => Box::new(Packed(hashbrown::HashMap::<u64, u64, IdBuild>::default())),
        "grid" => Box::new(Grid::new(r, false)),
        "grid-fm" => Box::new(Grid::new(r, true)),
        _ => Box::new(Window::new(r)),
    };
    fill(t.as_mut(), CX, CZ, r);
    t
}

fn main() {
    // The fast modulo returns exactly `rem_euclid` (possible grid sizes 7..=259).
    for n in 7..=259 {
        let m = u64::MAX / n as u64 + 1;
        let edge = [i32::MIN, i32::MIN + 1, -(1 << 22) - 1, -(1 << 22), (1 << 22) - 1, 1 << 22, i32::MAX];
        for x in (-70_000..70_000).chain(edge).chain((0..2000).map(|i| i * 2_147_483 - 1_073_741_000)) {
            assert_eq!(fastmod(x, n, m), x.rem_euclid(n) as usize, "x {x} n {n}");
        }
    }
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let cap = std::env::var("RUSTCRAFT_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or(cores);
    let mut threads = vec![1usize];
    while threads.last().unwrap() * 2 <= cap {
        threads.push(threads.last().unwrap() * 2);
    }
    if *threads.last().unwrap() != cap {
        threads.push(cap);
    }
    let kinds = ["fold", "fx", "fxp", "ident", "window", "grid", "grid-fm"];
    println!("logical cores {cores}, threads {threads:?}; ns/operation, median of 5");
    for r in [6i32, 12, 32] {
        println!("\n## radius {r} ({} chunks)", (2 * r + 1).pow(2));
        print!("| load | threads |");
        for k in kinds {
            print!(" {k} |");
        }
        println!();
        for &th in &threads {
            print!("| lookup | {th} |");
            for k in kinds {
                let t = make(k, r);
                let v = median((0..5).map(|_| lookups(t.as_ref(), r, th, 2_000_000)).collect());
                print!(" {v:.2} |");
            }
            println!();
        }
        for &th in &threads {
            print!("| neighbours | {th} |");
            for k in kinds {
                let t = make(k, r);
                let rounds = (200_000 / (2 * r + 1).pow(2)).max(1) as usize;
                let v = median((0..5).map(|_| neighbours(t.as_ref(), r, th, rounds)).collect());
                print!(" {v:.2} |");
            }
            println!();
        }
        print!("| walk (ns/step) | 1 |");
        for k in kinds {
            let v = median(
                (0..5)
                    .map(|_| {
                        let mut t = make(k, r);
                        walk(t.as_mut(), r, 2000)
                    })
                    .collect(),
            );
            print!(" {v:.0} |");
        }
        println!();
    }
}
