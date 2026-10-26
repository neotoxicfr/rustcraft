//! Allocation tracer (`RUSTCRAFT_ALLOCTRACE=1`): one stack sample every
//! `STEP` bytes allocated per thread, accumulated by (thread, callers) and flushed to the log
//! every 10 s, sorted by bytes. Diagnostic tool: costly when active, a single
//! atomic read per allocation otherwise.
#![allow(unsafe_code)]

use hashbrown::HashMap;
use std::cell::Cell;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Tracer active.
pub static TRACE: AtomicBool = AtomicBool::new(false);

/// Bytes between two samples of the same thread.
const STEP: u64 = 256 * 1024;

/// Callers kept per sample.
const DEPTH: usize = 6;

thread_local! {
    static ACC: Cell<u64> = const { Cell::new(0) };
    static BUSY: Cell<bool> = const { Cell::new(false) };
}

static TABLE: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);

/// Enables the tracer if the environment variable is set.
pub fn init() {
    if rc_data::env_flag!("RUSTCRAFT_ALLOCTRACE") {
        TRACE.store(true, Ordering::Relaxed);
    }
}

/// Called by the global allocator for each allocation while the tracer is active.
#[inline]
pub fn sample(size: usize) {
    // Allocations made by the tracer itself: neither counted nor sampled.
    if BUSY.try_with(Cell::get).unwrap_or(true) {
        return;
    }
    TOTAL.fetch_add(size as u64, Ordering::Relaxed);
    let Ok(n) = ACC.try_with(|a| {
        let v = a.get() + size as u64;
        a.set(v % STEP);
        v / STEP
    }) else {
        return;
    };
    if n == 0 {
        return;
    }
    BUSY.with(|b| b.set(true));
    let key = stack_key();
    if let Ok(mut t) = TABLE.lock() {
        *t.get_or_insert_with(HashMap::new).entry(key).or_insert(0) += n * STEP;
    }
    BUSY.with(|b| b.set(false));
}

/// Thread name, then the first callers outside the allocator and the standard library.
fn stack_key() -> String {
    let bt = std::backtrace::Backtrace::force_capture().to_string();
    let thread = std::thread::current().name().unwrap_or("?").to_owned();
    let mut frames = Vec::new();
    for line in bt.lines() {
        let l = line.trim_start();
        let Some((num, sym)) = l.split_once(": ") else { continue };
        if num.parse::<u32>().is_err() {
            continue;
        }
        let skip =
            ["std::", "alloc::", "core::", "mimalloc", "hashbrown::", "rc_app::alloc_trace", "rc_app::debug_sys"]
                .iter()
                .any(|p| sym.starts_with(p) || sym.starts_with(&format!("<{p}")))
                || sym.contains("__rust")
                || sym.starts_with("<alloc::")
                || sym.starts_with("<std::")
                || sym.starts_with("<core::");
        if skip {
            continue;
        }
        // Strips the trailing hash from the symbols.
        let s = sym.rsplit_once("::h").filter(|(_, h)| h.len() == 16).map_or(sym, |(a, _)| a);
        frames.push(s.to_owned());
        if frames.len() == DEPTH {
            break;
        }
    }
    format!("[{thread}] {}", frames.join(" <- "))
}

/// Bytes allocated since the tracer started (all threads).
static TOTAL: AtomicU64 = AtomicU64::new(0);

/// State of the periodic report: total at the previous report, accumulated seconds.
static WINDOW: Mutex<(u64, f64)> = Mutex::new((0, 0.0));

/// One line per second (bytes per frame, all threads) and the detail every 10 s.
pub fn per_second(since: f64, frames: u32) {
    if !TRACE.load(Ordering::Relaxed) {
        return;
    }
    {
        let total = TOTAL.load(Ordering::Relaxed);
        let Ok(mut w) = WINDOW.lock() else { return };
        let frames = frames.max(1);
        let per_s = total.saturating_sub(w.0) as f64 / since;
        eprintln!(
            "allocations: {:.1} MB/s, {:.1} KB per frame",
            per_s / 1_048_576.0,
            total.saturating_sub(w.0) as f64 / f64::from(frames) / 1024.0
        );
        w.0 = total;
        w.1 += since;
        if w.1 >= 10.0 {
            let win = w.1;
            w.1 = 0.0;
            drop(w);
            dump(win);
        }
    }
}

/// Writes the accumulated report (bytes per second over the window) and resets it.
pub fn dump(window_s: f64) {
    BUSY.with(|b| b.set(true));
    let taken = TABLE.lock().ok().and_then(|mut t| t.take());
    if let Some(t) = taken {
        let mut v: Vec<_> = t.into_iter().collect();
        v.sort_unstable_by_key(|x| std::cmp::Reverse(x.1));
        let total: u64 = v.iter().map(|x| x.1).sum();
        eprintln!("allocation tracer: {:.1} MB/s in total", total as f64 / window_s / 1_048_576.0);
        for (k, b) in v.iter().take(25) {
            eprintln!("  {:>8.2} MB/s  {k}", *b as f64 / window_s / 1_048_576.0);
        }
    }
    BUSY.with(|b| b.set(false));
}
