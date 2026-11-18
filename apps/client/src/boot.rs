//! Startup milestones: "[startup] <step> at N ms", counted from the creation of the process
//! by the system (the "first frame" reading of `main.rs` starts, for its part, from the entry into `main`).
//! Written up to the first frame only: a pack reload no longer produces any.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Instant;

static DONE: AtomicBool = AtomicBool::new(false);

/// Age of the process at the entry into `main`, plus the monotonic clock started from there (outside Windows,
/// or if the system does not answer, the origin is the entry into `main`).
fn origin() -> &'static (f64, Instant) {
    static O: OnceLock<(f64, Instant)> = OnceLock::new();
    O.get_or_init(|| (crate::debug_sys::process_age_ms().unwrap_or(0.0), Instant::now()))
}

/// Sets the origin (first call from `main`).
pub fn start() {
    origin();
}

/// Milliseconds since the creation of the process.
pub fn now_ms() -> f64 {
    let (age, t) = origin();
    age + t.elapsed().as_secs_f64() * 1e3
}

/// Milestone `step` (no effect after the first frame).
pub fn mark(step: &str) {
    if !DONE.load(Ordering::Relaxed) {
        eprintln!("[startup] {step} at {:.0} ms", now_ms());
    }
}

/// Last milestone: the first frame.
pub fn finish() {
    if !DONE.swap(true, Ordering::Relaxed) {
        eprintln!("[startup] first frame at {:.0} ms", now_ms());
    }
}
