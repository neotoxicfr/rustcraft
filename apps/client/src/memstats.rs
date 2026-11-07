//! Memory measurement (`--features memstats`): live bytes on our heap, counted around
//! mimalloc. Without this option, no cost.

#[cfg(feature = "memstats")]
pub use counting::{LIVE, step};

#[cfg(feature = "memstats")]
#[allow(unsafe_code)]
mod counting {
    use crate::debug_sys::CountingAllocator;
    use std::alloc::{GlobalAlloc, Layout};
    use std::sync::atomic::{AtomicUsize, Ordering};

    pub static LIVE: AtomicUsize = AtomicUsize::new(0);
    /// The game's allocator (same mimalloc calls, switches and tracer) behind the live-bytes
    /// counter, so that the measured heap is the one of the game.
    pub struct Counting;

    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            LIVE.fetch_add(layout.size(), Ordering::Relaxed);
            // SAFETY: same contract as the wrapped allocator.
            unsafe { CountingAllocator.alloc(layout) }
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            LIVE.fetch_add(layout.size(), Ordering::Relaxed);
            // SAFETY: same contract as the wrapped allocator (fresh pages already zero, as without
            // the measurement: private memory stays comparable).
            unsafe { CountingAllocator.alloc_zeroed(layout) }
        }
        unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            // SAFETY: `ptr` comes from `alloc` with the same layout.
            unsafe { CountingAllocator.dealloc(ptr, layout) }
        }
        unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
            LIVE.fetch_add(new_size, Ordering::Relaxed);
            LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
            // SAFETY: same contract as the wrapped allocator.
            unsafe { CountingAllocator.realloc(ptr, layout, new_size) }
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
