//! Memory measurement (`--features memstats`): live bytes on our heap, counted around
//! mimalloc. Without this option, no cost.

#[cfg(feature = "memstats")]
pub use counting::{LIVE, step};

#[cfg(feature = "memstats")]
#[allow(unsafe_code)]
mod counting {
    use std::alloc::{GlobalAlloc, Layout};
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub static LIVE: AtomicUsize = AtomicUsize::new(0);
    pub struct Counting;

    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            LIVE.fetch_add(l.size(), Ordering::Relaxed);
            // SAFETY: same contract as the wrapped allocator.
            unsafe { mimalloc::MiMalloc.alloc(l) }
        }
        unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
            LIVE.fetch_add(l.size(), Ordering::Relaxed);
            // SAFETY: same contract as the wrapped allocator (fresh pages already zero, as without
            // the measurement: private memory stays comparable).
            unsafe { mimalloc::MiMalloc.alloc_zeroed(l) }
        }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            LIVE.fetch_sub(l.size(), Ordering::Relaxed);
            // SAFETY: `p` comes from `alloc` with the same layout.
            unsafe { mimalloc::MiMalloc.dealloc(p, l) }
        }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
            LIVE.fetch_add(n, Ordering::Relaxed);
            LIVE.fetch_sub(l.size(), Ordering::Relaxed);
            // SAFETY: same contract as the wrapped allocator.
            unsafe { mimalloc::MiMalloc.realloc(p, l, n) }
        }
    }

    #[global_allocator]
    static ALLOC: Counting = Counting;

    /// Live heap after a startup step.
    pub fn step(what: &str) {
        eprintln!("heap after {what}: {:.1} MB", LIVE.load(Ordering::Relaxed) as f64 / 1_048_576.0);
    }
}

#[cfg(not(feature = "memstats"))]
pub fn step(_: &str) {}
