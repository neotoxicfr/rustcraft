//! Common kit of the rc-sim micro-benches (each bench `#[ignore]`d, run alone on a quiet machine):
//! flat world, optionally with the scene bench's `perf` data pack, commands, timed ticks (µs per
//! tick; allocations per tick with the `memstats` feature) and `[bench] <bench> <case> <metric>
//! <value>` lines, tabled by `python -I tools/bench/scenes.py micro <pass1.txt> <pass2.txt>…`.
//!
//! One file per bench, named by what it measures (`bench_<content>.rs`). Run:
//! `cargo test --release -p rc-sim [--features memstats] bench_<content> -- --ignored --nocapture --test-threads=1`.
//! On a loaded machine, a timing is only a hint: back it with a deterministic count (allocations
//! here, instructions under cachegrind) and settle on the quiet bench.

use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicU64;

use crate::blocks::probe::Probe;

/// Allocation calls of the whole test binary (feature `memstats`): the game's allocator
/// (mimalloc) wrapped by a counter; replaces the plain test allocator of `lib.rs`.
pub(crate) static ALLOCS: AtomicU64 = AtomicU64::new(0);

// A global allocator is an `unsafe` trait: allowed here only, like the other counters of the repository.
#[cfg(feature = "memstats")]
#[allow(unsafe_code)]
mod counting {
    use std::alloc::{GlobalAlloc, Layout};
    use std::sync::atomic::Ordering::Relaxed;

    pub struct Counting;

    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            super::ALLOCS.fetch_add(1, Relaxed);
            // SAFETY: same contract as the wrapped allocator.
            unsafe { mimalloc::MiMalloc.alloc(l) }
        }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            // SAFETY: `p` comes from this allocator with the same layout.
            unsafe { mimalloc::MiMalloc.dealloc(p, l) }
        }
        unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
            super::ALLOCS.fetch_add(1, Relaxed);
            // SAFETY: same contract as the wrapped allocator.
            unsafe { mimalloc::MiMalloc.alloc_zeroed(l) }
        }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
            super::ALLOCS.fetch_add(1, Relaxed);
            // SAFETY: `p` comes from this allocator with layout `l`.
            unsafe { mimalloc::MiMalloc.realloc(p, l, n) }
        }
    }

    #[global_allocator]
    static COUNTING: Counting = Counting;
}

/// Allocation calls so far (always 0 without the `memstats` feature).
pub(crate) fn allocations() -> u64 {
    ALLOCS.load(std::sync::atomic::Ordering::Relaxed)
}

/// Folder of the `perf` pack written for the flat world (`RUSTCRAFT_PERF_PACK`); explicit
/// failure with the command to run when it is missing.
pub(crate) fn perf_pack_dir() -> PathBuf {
    let hint = "write it first: python -I tools/bench/scenes.py pack-files target/perf-pack, \
                then RUSTCRAFT_PERF_PACK=<repo>/target/perf-pack/datapacks/perf";
    let dir = PathBuf::from(rc_data::env_os!("RUSTCRAFT_PERF_PACK").unwrap_or_else(|| panic!("RUSTCRAFT_PERF_PACK missing: {hint}")));
    assert!(dir.join("pack.mcmeta").is_file(), "no perf pack in {}: {hint}", dir.display());
    dir
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("pack folder");
    for e in std::fs::read_dir(from).expect("pack read").flatten() {
        let target = to.join(e.file_name());
        if e.path().is_dir() {
            copy_tree(&e.path(), &target);
        } else {
            std::fs::copy(e.path(), target).expect("pack copy");
        }
    }
}

/// Flat world of `tests_g3f13::open` reopened with `pack` installed as `datapacks/perf`
/// (chunks -2..=9 × -2..=2 loaded, player at (0.5, -60, 0.5)).
pub(crate) fn flat_world_with_pack(tag: &str, pack: &Path) -> (Probe, PathBuf) {
    let (p, dir) = crate::tests_g3f13::open(tag);
    let data = p.s.data.clone();
    drop(p);
    copy_tree(pack, &dir.join("datapacks").join("perf"));
    (crate::tests_g3f13::reopen(&dir, data), dir)
}

/// Runs a command as the server; panics if it is refused.
pub(crate) fn cmd(p: &mut Probe, c: &str) {
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, c);
    assert!(ok, "command refused: {c}");
}

/// Runs `function <name>` as the server. A function without `return` gives no result (like the
/// game), hence no success either: the caller checks what the function did.
pub(crate) fn function(p: &mut Probe, name: &str) {
    assert!(p.s.cmd.functions.contains_key(name), "function {name} not loaded");
    let src = p.s.server_source();
    let _ = p.s.perform(src, &format!("function {name}"));
}

/// `n` server ticks, the events to the client thrown away (as the bench's scenes would send them).
pub(crate) fn run_ticks(p: &mut Probe, n: u32) {
    for _ in 0..n {
        p.s.game_tick();
        p.s.tick += 1;
        let _ = p.take_events();
    }
}

/// Measured runs of a bench case: µs and allocations per tick of each run.
pub(crate) struct Timing {
    pub us_per_tick: Vec<f64>,
    pub allocs_per_tick: Vec<f64>,
}

/// `runs` runs of `ticks` ticks after `warmup` unmeasured ticks.
pub(crate) fn time_ticks(p: &mut Probe, warmup: u32, ticks: u32, runs: usize) -> Timing {
    run_ticks(p, warmup);
    let mut t = Timing { us_per_tick: Vec::new(), allocs_per_tick: Vec::new() };
    for _ in 0..runs {
        let (start, allocs) = (std::time::Instant::now(), allocations());
        run_ticks(p, ticks);
        t.us_per_tick.push(start.elapsed().as_secs_f64() * 1e6 / f64::from(ticks));
        t.allocs_per_tick.push((allocations() - allocs) as f64 / f64::from(ticks));
    }
    t
}

pub(crate) fn median(v: &[f64]) -> f64 {
    let mut s = v.to_vec();
    s.sort_by(f64::total_cmp);
    s.get(s.len() / 2).copied().unwrap_or(f64::NAN)
}

/// One `[bench]` line (read by `scenes.py micro`).
pub(crate) fn report(bench: &str, case: &str, metric: &str, value: f64) {
    eprintln!("[bench] {bench} {case} {metric} {value:.3}");
}

/// Median and best µs per tick, median allocations per tick (with `memstats`).
pub(crate) fn report_timing(bench: &str, case: &str, t: &Timing) {
    report(bench, case, "us_per_tick", median(&t.us_per_tick));
    report(bench, case, "us_per_tick_best", t.us_per_tick.iter().copied().fold(f64::INFINITY, f64::min));
    if cfg!(feature = "memstats") {
        report(bench, case, "allocs_per_tick", median(&t.allocs_per_tick));
    }
}
