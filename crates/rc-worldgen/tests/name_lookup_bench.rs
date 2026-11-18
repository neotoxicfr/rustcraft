//! Micro-bench of the lookups by name (ignored by default): tags, boolean properties and
//! directions, shape updates; time per operation and allocations per operation, 1 thread then
//! powers of 2 up to the number of cores. The paths are chosen by the environment
//! (read once): run with nothing then with `RUSTCRAFT_OPT_TAG_BORROW=0`,
//! `RUSTCRAFT_OPT_PROP_IDX=0`, `RUSTCRAFT_OPT_SHAPE_KEYS=0`.
//! `cargo test --release -p rc-worldgen --test name_lookup_bench -- --ignored --nocapture`.
#![allow(clippy::disallowed_methods)]
// Counting allocator (bench only): delegates as is to the system's.
#![allow(unsafe_code)]

mod common;

use std::alloc::{GlobalAlloc, Layout, System};
use std::hint::black_box;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use rc_worldgen::decor_blocks::{Blocks, Dir, State};
use rc_worldgen::decor_data::DecorData;
use rc_worldgen::decor_region::{DecoChunk, Region};

/// System allocator, allocations counted.
struct Counting;

static ALLOCS: AtomicU64 = AtomicU64::new(0);

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.alloc(l) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        ALLOCS.fetch_add(1, Ordering::Relaxed);
        unsafe { System.realloc(p, l, n) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

const TAGS: [&str; 10] = [
    "leaves",
    "minecraft:logs",
    "fences",
    "minecraft:walls",
    "#minecraft:stairs",
    "features_cannot_replace",
    "minecraft:features_cannot_replace",
    "supports_chorus_plant",
    "beneath_bamboo_podzol_replaceable",
    "minecraft:ice",
];

/// (ns per operation, allocations per operation) of one round of `f` repeated `reps` times, best of 5.
fn time(reps: usize, ops: usize, mut f: impl FnMut() -> u64) -> (f64, f64) {
    let mut best = f64::MAX;
    let mut allocs = 0;
    for _ in 0..5 {
        let a0 = ALLOCS.load(Ordering::Relaxed);
        let t = Instant::now();
        for _ in 0..reps {
            black_box(f());
        }
        let dt = t.elapsed().as_secs_f64() * 1e9 / (reps * ops) as f64;
        allocs = ALLOCS.load(Ordering::Relaxed) - a0;
        best = best.min(dt);
    }
    (best, allocs as f64 / (reps * ops) as f64)
}

fn tags(b: &Blocks) -> u64 {
    TAGS.iter().map(|t| b.tag(t).map_or(0, u64::from)).sum()
}

fn props(b: &Blocks) -> u64 {
    let mut acc = 0u64;
    for s in 0..b.state_count() as State {
        acc += b.get_bool(s, b.p.waterlogged).map_or(3, u64::from);
        acc += b.with_bool(s, b.p.waterlogged, true) as u64;
        acc += b.get_dir(s, b.p.facing).map_or(7, |d| d as u64);
        acc += b.with_dir(s, b.p.facing, Dir::East) as u64;
        acc += b.has(s, b.p.axis) as u64;
    }
    acc
}

fn shapes(b: &Blocks, r: &Region) -> u64 {
    let p = [8, 8, 8];
    let mut acc = 0u64;
    for s in (0..b.state_count() as State).step_by(3) {
        for d in Dir::ALL {
            acc += rc_worldgen::shape_update::update(r, s, p, d, s.wrapping_mul(2_654_435_761) % b.state_count() as u32) as u64;
        }
    }
    acc
}

fn region<'a>(chunks: &'a mut [DecoChunk], b: &'a Blocks) -> Region<'a> {
    let arr: [&mut DecoChunk; 9] = {
        let mut it = chunks.iter_mut();
        std::array::from_fn(|_| it.next().unwrap())
    };
    let mut r = Region::new(arr, b, 4, 1);
    // Varied neighbourhood: states spread over the whole registry.
    let n = b.state_count() as u32;
    for x in 6..11 {
        for y in 6..11 {
            for z in 6..11 {
                r.set_p([x, y, z], ((x * 31 + y * 17 + z * 7) as u32).wrapping_mul(40_503) % n, 2);
            }
        }
    }
    r
}

/// Throughput over `t` threads (operations per µs, all threads combined).
fn threads(t: usize, b: &Blocks, which: u8) -> f64 {
    let start = Instant::now();
    let reps = 20;
    std::thread::scope(|sc| {
        for _ in 0..t {
            sc.spawn(|| {
                let mut chunks: Vec<DecoChunk> = (0..9).map(|k| DecoChunk::new(k % 3 - 1, k / 3 - 1, 0, 32)).collect();
                let r = region(&mut chunks, b);
                for _ in 0..reps {
                    black_box(match which {
                        0 => (0..10_000).map(|_| tags(b)).sum(),
                        1 => props(b),
                        _ => shapes(b, &r),
                    });
                }
            });
        }
    });
    let ops = match which {
        0 => 10_000 * TAGS.len(),
        1 => b.state_count() * 5,
        _ => b.state_count().div_ceil(3) * 6,
    };
    (t * reps * ops) as f64 / start.elapsed().as_secs_f64() / 1e6
}

#[test]
#[ignore = "benchmark"]
fn name_lookup_timing() {
    let jar = common::client_jar().expect("jar client 26.3");
    let v = rc_data::load_or_prepare("26.3", |m| eprintln!("rc-data : {m}")).expect("rc-data");
    let data = DecorData::from_jar(&jar).expect("decoration data");
    let b = Blocks::new(&v, &data).expect("blocks");
    let mode = ["TAG_BORROW", "PROP_IDX", "SHAPE_KEYS"]
        .map(|k| format!("{k}={}", rc_data::env_flags::read_os(&format!("RUSTCRAFT_OPT_{k}")).and_then(|v| v.into_string().ok()).unwrap_or_else(|| "1".into())))
        .join(" ");
    println!("mode : {mode}");
    let (ns, al) = time(20_000, TAGS.len(), || tags(&b));
    println!("tags            : {ns:7.2} ns/op, {al:.3} alloc/op");
    let (ns, al) = time(10, b.state_count() * 5, || props(&b));
    println!("properties      : {ns:7.2} ns/op, {al:.3} alloc/op");
    let mut chunks: Vec<DecoChunk> = (0..9).map(|k| DecoChunk::new(k % 3 - 1, k / 3 - 1, 0, 32)).collect();
    let r = region(&mut chunks, &b);
    let (ns, al) = time(10, b.state_count().div_ceil(3) * 6, || shapes(&b, &r));
    println!("shapes          : {ns:7.2} ns/op, {al:.3} alloc/op");
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let mut t = 1;
    while t <= cores {
        let v = [0, 1, 2].map(|w| threads(t, &b, w));
        println!("{t:2} threads: tags {:8.1} op/µs, properties {:8.1} op/µs, shapes {:8.1} op/µs", v[0], v[1], v[2]);
        if t == cores {
            break;
        }
        t = (t * 2).min(cores);
    }
}
