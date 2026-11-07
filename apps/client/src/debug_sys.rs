//! System readings of the debug screen (`debug-f3.md` § 3.4, § 3.5) and clipboard
//! (F3+C, F3+I): direct calls to the system, no dependencies. Outside Windows: null values.
#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout};
use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

use libmimalloc_sys as mi;

#[cfg(test)]
mod allocator_tests;

/// Bytes allocated while counting is on (`memory` entry of the debug screen, benches).
pub static ALLOCATED: AtomicU64 = AtomicU64::new(0);

// Bits of `ALLOCATOR_FLAGS`. All bits clear is the usual case and the fast path: one load, one
// branch, then a direct mimalloc call.
/// Allocated bytes added to `ALLOCATED`.
const COUNT_BYTES: u8 = 1;
/// Sampling by the allocation tracer (`alloc_trace.rs`).
const TRACE: u8 = 1 << 1;
/// `RUSTCRAFT_OPT_ALLOC_DIRECT=0`: mimalloc's aligned entry points for every request (old path).
const ALIGNED_CALLS: u8 = 1 << 2;
/// `RUSTCRAFT_OPT_ALLOCATOR_SINGLE_FLAG=0`: counting and tracing each re-read before use, as when
/// they were two separate flags (old path).
const SEPARATE_CHECKS: u8 = 1 << 3;

/// Starts with every bit clear (new paths): allocations happen before `main`, and the allocator
/// never reads the environment itself (that would allocate).
static ALLOCATOR_FLAGS: AtomicU8 = AtomicU8::new(0);

fn set_allocator_flag(flag: u8, on: bool) {
    if on {
        ALLOCATOR_FLAGS.fetch_or(flag, Ordering::Relaxed);
    } else {
        ALLOCATOR_FLAGS.fetch_and(!flag, Ordering::Relaxed);
    }
}

/// Counts the allocated bytes into `ALLOCATED` (allocation rate of the `memory` entry).
pub fn count_allocated_bytes(on: bool) {
    set_allocator_flag(COUNT_BYTES, on);
}

/// Hands every allocation to the allocation tracer.
pub fn trace_allocations(on: bool) {
    set_allocator_flag(TRACE, on);
}

/// The allocation tracer is on.
pub fn tracing_allocations() -> bool {
    ALLOCATOR_FLAGS.load(Ordering::Relaxed) & TRACE != 0
}

/// Applies the allocator switches; to call at the start of `main`. Returns (direct calls,
/// single flag), both `true` by default.
pub fn read_allocator_switches() -> (bool, bool) {
    let direct = rc_data::opt_switch!("RUSTCRAFT_OPT_ALLOC_DIRECT");
    let single_flag = rc_data::opt_switch!("RUSTCRAFT_OPT_ALLOCATOR_SINGLE_FLAG");
    set_allocator_flag(ALIGNED_CALLS, !direct);
    set_allocator_flag(SEPARATE_CHECKS, !single_flag);
    (direct, single_flag)
}

/// Tests that change `ALLOCATOR_FLAGS` hold this, so that no other one flips a bit under them.
#[cfg(test)]
static ALLOCATOR_FLAGS_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
fn lock_allocator_flags() -> std::sync::MutexGuard<'static, ()> {
    // A test that failed while holding it leaves nothing half-done: the flags are set again.
    ALLOCATOR_FLAGS_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
}

/// Benches: bytes allocated while `f` runs (counter of the whole process: run the bench alone,
/// other threads would count too).
#[cfg(test)]
pub fn allocated_bytes(f: impl FnOnce()) -> u64 {
    let _flags = lock_allocator_flags();
    allocated_bytes_unlocked(f)
}

/// `allocated_bytes` for a caller that already holds the flag lock.
#[cfg(test)]
fn allocated_bytes_unlocked(f: impl FnOnce()) -> u64 {
    count_allocated_bytes(true);
    let before = ALLOCATED.load(Ordering::Relaxed);
    f();
    let after = ALLOCATED.load(Ordering::Relaxed);
    count_allocated_bytes(false);
    after - before
}

/// Global allocator: mimalloc, plus the optional byte counter and allocation tracer (wrapped by
/// the live-bytes counter of `memstats.rs` in that build).
pub struct CountingAllocator;

// SAFETY: every call goes to mimalloc with the caller's layout (see `mimalloc_alloc`).
unsafe impl GlobalAlloc for CountingAllocator {
    // The old paths stay inline (no call out of line), so that one switch at 0 costs about what the
    // old code cost (+0.2 ns at 8 B); counting or tracing alone goes out of line. Both switches at 0
    // still pay the dispatch (+0.5 to 1.8 ns at 8 B): the scene reference for the old code is an
    // executable built from `main`, not this arm. One copy of each old path, flags read at run
    // time: a copy per flag value, or one exact arm for both at 0, made every path slower, the new
    // one included (bigger body inlined at each site, micro-benches of 06/11 and 07/11).
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY (every arm): same contract as the caller's, passed on unchanged.
        match ALLOCATOR_FLAGS.load(Ordering::Relaxed) {
            0 => unsafe { mimalloc_alloc(layout, false) },
            ALIGNED_CALLS => unsafe { mimalloc::MiMalloc.alloc(layout) },
            flags if flags & SEPARATE_CHECKS != 0 => unsafe { alloc_after_separate_checks(flags, layout, false) },
            flags => unsafe { alloc_with_flags(flags, layout, false) },
        }
    }
    #[inline]
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY (every arm): same contract as the caller's, passed on unchanged.
        match ALLOCATOR_FLAGS.load(Ordering::Relaxed) {
            0 => unsafe { mimalloc_alloc(layout, true) },
            ALIGNED_CALLS => unsafe { mimalloc::MiMalloc.alloc_zeroed(layout) },
            flags if flags & SEPARATE_CHECKS != 0 => unsafe { alloc_after_separate_checks(flags, layout, true) },
            flags => unsafe { alloc_with_flags(flags, layout, true) },
        }
    }
    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        // SAFETY: `ptr` comes from mimalloc (plain or aligned entry point: both are freed alike).
        unsafe { mi::mi_free(ptr.cast()) }
    }
    #[inline]
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY (every arm): same contract as the caller's, passed on unchanged.
        match ALLOCATOR_FLAGS.load(Ordering::Relaxed) {
            0 => unsafe { mimalloc_realloc(ptr, layout, new_size) },
            ALIGNED_CALLS => unsafe { mimalloc::MiMalloc.realloc(ptr, layout, new_size) },
            flags if flags & SEPARATE_CHECKS != 0 => unsafe { realloc_after_separate_checks(flags, ptr, layout, new_size) },
            flags => unsafe { realloc_with_flags(flags, ptr, layout, new_size) },
        }
    }
}

/// `RUSTCRAFT_OPT_ALLOCATOR_SINGLE_FLAG=0`: the old sequence, inline as it was (no call out of
/// line): counting and tracing each loaded and tested on its own, then the call.
#[inline(always)]
unsafe fn alloc_after_separate_checks(flags: u8, layout: Layout, zeroed: bool) -> *mut u8 {
    record_with_separate_checks(layout.size(), layout.size());
    // SAFETY: same contract as the caller's, passed on unchanged.
    unsafe { mimalloc_alloc_by_flags(flags, layout, zeroed) }
}

/// Reallocation counterpart of `alloc_after_separate_checks`.
#[inline(always)]
unsafe fn realloc_after_separate_checks(flags: u8, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
    if new_size > layout.size() {
        record_with_separate_checks(new_size - layout.size(), new_size);
    }
    // SAFETY: same contract as the caller's, passed on unchanged.
    unsafe { mimalloc_realloc_by_flags(flags, ptr, layout, new_size) }
}

/// Counts and traces `new_bytes` (the block now has `block_size` bytes), one flag load per check
/// as when counting and tracing were two flags.
#[inline(always)]
fn record_with_separate_checks(new_bytes: usize, block_size: usize) {
    if ALLOCATOR_FLAGS.load(Ordering::Relaxed) & COUNT_BYTES != 0 {
        ALLOCATED.fetch_add(new_bytes as u64, Ordering::Relaxed);
    }
    if ALLOCATOR_FLAGS.load(Ordering::Relaxed) & TRACE != 0 {
        crate::alloc_trace::sample(new_bytes, block_size);
    }
}

/// Allocation while counting or tracing is on: out of line, so that the usual path stays a load,
/// a branch and a call.
#[cold]
#[inline(never)]
unsafe fn alloc_with_flags(flags: u8, layout: Layout, zeroed: bool) -> *mut u8 {
    record_allocation(flags, layout.size(), layout.size());
    // SAFETY: same contract as the caller's, passed on unchanged.
    unsafe { mimalloc_alloc_by_flags(flags, layout, zeroed) }
}

/// Reallocation while counting or tracing is on (see `alloc_with_flags`).
#[cold]
#[inline(never)]
unsafe fn realloc_with_flags(flags: u8, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
    if new_size > layout.size() {
        record_allocation(flags, new_size - layout.size(), new_size);
    }
    // SAFETY: same contract as the caller's, passed on unchanged.
    unsafe { mimalloc_realloc_by_flags(flags, ptr, layout, new_size) }
}

/// Counts and traces `new_bytes` newly allocated, in a block now of `block_size` bytes (the
/// tracer sorts requests by block size).
fn record_allocation(flags: u8, new_bytes: usize, block_size: usize) {
    if flags & COUNT_BYTES != 0 {
        ALLOCATED.fetch_add(new_bytes as u64, Ordering::Relaxed);
    }
    if flags & TRACE != 0 {
        crate::alloc_trace::sample(new_bytes, block_size);
    }
}

/// Direct or aligned allocation, as `ALIGNED_CALLS` says.
///
/// # Safety
/// Same contract as [`GlobalAlloc::alloc`].
#[inline(always)]
unsafe fn mimalloc_alloc_by_flags(flags: u8, layout: Layout, zeroed: bool) -> *mut u8 {
    // SAFETY: same contract as the caller's, passed on unchanged.
    unsafe {
        match (flags & ALIGNED_CALLS != 0, zeroed) {
            (false, _) => mimalloc_alloc(layout, zeroed),
            (true, false) => mimalloc::MiMalloc.alloc(layout),
            (true, true) => mimalloc::MiMalloc.alloc_zeroed(layout),
        }
    }
}

/// Direct or aligned reallocation, as `ALIGNED_CALLS` says.
///
/// # Safety
/// Same contract as [`GlobalAlloc::realloc`].
#[inline(always)]
unsafe fn mimalloc_realloc_by_flags(flags: u8, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
    // SAFETY: same contract as the caller's, passed on unchanged.
    unsafe {
        if flags & ALIGNED_CALLS == 0 {
            mimalloc_realloc(ptr, layout, new_size)
        } else {
            mimalloc::MiMalloc.realloc(ptr, layout, new_size)
        }
    }
}

/// Largest alignment that mimalloc's plain entry points give every block of at least that size:
/// size classes from 16 bytes up are multiples of 16 (the 8-byte class is 8-aligned), like the
/// system `malloc`.
const PLAIN_CALL_ALIGN: usize = 16;

/// A plain mimalloc call already returns `align` for a block of `size` bytes. The aligned entry
/// points take a slower, out-of-line path above 1 KiB, which pads the request by `align - 1`
/// bytes: 1,536 bytes aligned on 8 then come from the 1,792-byte size class.
#[inline]
fn plain_call_aligns(size: usize, align: usize) -> bool {
    align <= PLAIN_CALL_ALIGN && align <= size
}

/// Block of mimalloc for `layout`, zeroed or not: plain call when it gives the alignment.
///
/// # Safety
/// Same contract as [`GlobalAlloc::alloc`].
#[inline]
unsafe fn mimalloc_alloc(layout: Layout, zeroed: bool) -> *mut u8 {
    let (size, align) = (layout.size(), layout.align());
    // SAFETY: mimalloc takes any size; the aligned entry points any power-of-two alignment,
    // which `Layout` guarantees.
    let block = unsafe {
        match (plain_call_aligns(size, align), zeroed) {
            (true, false) => mi::mi_malloc(size),
            (true, true) => mi::mi_zalloc(size),
            (false, false) => mi::mi_malloc_aligned(size, align),
            (false, true) => mi::mi_zalloc_aligned(size, align),
        }
    };
    block.cast()
}

/// Reallocation of a mimalloc block: plain call when a fresh plain block of `new_size` bytes
/// would have the alignment (a block that stays in place keeps the alignment it had).
///
/// # Safety
/// Same contract as [`GlobalAlloc::realloc`].
#[inline]
unsafe fn mimalloc_realloc(ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
    // SAFETY: `ptr` comes from mimalloc; the alignment is a power of two (`Layout`).
    let block = unsafe {
        if plain_call_aligns(new_size, layout.align()) {
            mi::mi_realloc(ptr.cast(), new_size)
        } else {
            mi::mi_realloc_aligned(ptr.cast(), new_size, layout.align())
        }
    };
    block.cast()
}

/// Indices of the mimalloc options we set, read from the linked header by `build.rs`.
mod mimalloc_options {
    include!(concat!(env!("OUT_DIR"), "/mimalloc_options.rs"));
}

#[cfg(windows)]
mod win {
    #[repr(C)]
    pub struct MemoryStatusEx {
        pub length: u32,
        pub memory_load: u32,
        pub total_phys: u64,
        pub avail_phys: u64,
        pub total_page_file: u64,
        pub avail_page_file: u64,
        pub total_virtual: u64,
        pub avail_virtual: u64,
        pub avail_extended_virtual: u64,
    }

    #[repr(C)]
    pub struct ProcessMemoryCountersEx {
        pub cb: u32,
        pub page_fault_count: u32,
        pub peak_working_set_size: usize,
        pub working_set_size: usize,
        pub quota_peak_paged_pool_usage: usize,
        pub quota_paged_pool_usage: usize,
        pub quota_peak_non_paged_pool_usage: usize,
        pub quota_non_paged_pool_usage: usize,
        pub pagefile_usage: usize,
        pub peak_pagefile_usage: usize,
        pub private_usage: usize,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn GlobalMemoryStatusEx(buffer: *mut MemoryStatusEx) -> i32;
        pub fn GetCurrentProcess() -> isize;
        pub fn K32GetProcessMemoryInfo(process: isize, counters: *mut ProcessMemoryCountersEx, cb: u32) -> i32;
        pub fn GlobalAlloc(flags: u32, bytes: usize) -> isize;
        pub fn GlobalLock(mem: isize) -> *mut u8;
        pub fn GlobalUnlock(mem: isize) -> i32;
        pub fn GlobalFree(mem: isize) -> isize;
        pub fn GetProcessTimes(p: isize, c: *mut u64, e: *mut u64, k: *mut u64, u: *mut u64) -> i32;
        pub fn GetSystemTimePreciseAsFileTime(t: *mut u64);
        pub fn GetCurrentThread() -> isize;
        pub fn SetThreadPriority(thread: isize, priority: i32) -> i32;
    }

    /// Times of a logical processor since boot (100 ns): idle, kernel (idle included), user, then
    /// two reserved times and a counter.
    #[repr(C)]
    pub struct ProcessorTimes {
        pub idle: i64,
        pub kernel: i64,
        pub user: i64,
        pub _reserved: [i64; 2],
        pub _count: u32,
    }

    #[link(name = "ntdll")]
    unsafe extern "system" {
        pub fn NtQuerySystemInformation(class: u32, info: *mut std::ffi::c_void, len: u32, ret: *mut u32) -> i32;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        pub fn OpenClipboard(owner: isize) -> i32;
        pub fn EmptyClipboard() -> i32;
        pub fn SetClipboardData(format: u32, mem: isize) -> isize;
        pub fn CloseClipboard() -> i32;
    }
}

/// Country of the user's locale as the game reads it, three-letter ISO 3166 code (`FRA`, `KOR`);
/// `None` without one (`syslocale.rs`: Windows, macOS and the POSIX variables alike).
pub fn country_iso3() -> Option<String> {
    crate::syslocale::country_iso3().map(str::to_owned)
}

/// Priority of the calling thread: one step below normal (compute pool threads: the render thread and
/// the integrated server get ahead when all the cores are busy). No effect outside Windows.
pub fn lower_thread_priority() {
    #[cfg(windows)]
    // SAFETY: pseudo-handle of the current thread; THREAD_PRIORITY_BELOW_NORMAL = −1.
    unsafe {
        win::SetThreadPriority(win::GetCurrentThread(), -1);
    }
}

/// Total physical memory of the machine (bytes).
pub fn total_memory() -> u64 {
    #[cfg(windows)]
    {
        let mut s = win::MemoryStatusEx {
            length: std::mem::size_of::<win::MemoryStatusEx>() as u32,
            memory_load: 0,
            total_phys: 0,
            avail_phys: 0,
            total_page_file: 0,
            avail_page_file: 0,
            total_virtual: 0,
            avail_virtual: 0,
            avail_extended_virtual: 0,
        };
        // SAFETY: structure of the size announced in `length`.
        if unsafe { win::GlobalMemoryStatusEx(&mut s) } != 0 {
            return s.total_phys;
        }
    }
    0
}

/// Process memory: (committed private, working set), bytes.
pub fn process_memory() -> (u64, u64) {
    #[cfg(windows)]
    {
        let mut c: win::ProcessMemoryCountersEx =
            // SAFETY: structure of integer counters, zero is valid.
            unsafe { std::mem::zeroed() };
        c.cb = std::mem::size_of::<win::ProcessMemoryCountersEx>() as u32;
        // SAFETY: pseudo-handle of the current process, structure of the announced size.
        if unsafe { win::K32GetProcessMemoryInfo(win::GetCurrentProcess(), &mut c, c.cb) } != 0 {
            return (c.private_usage as u64, c.working_set_size as u64);
        }
    }
    (0, 0)
}

/// Sets a mimalloc option by its index in the linked header (`mimalloc_options`).
fn set_mimalloc_option(index: i32, value: i64) {
    unsafe extern "C" {
        fn mi_option_set(option: i32, value: std::ffi::c_long);
    }
    // SAFETY: function of the linked mimalloc library (global allocator); the index comes from
    // the header of that very library (`build.rs`). `c_long` is 32 bits on Windows: our values fit.
    unsafe { mi_option_set(index, value as std::ffi::c_long) }
}

/// What `commit_on_demand` did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommitOnDemand {
    /// Switch value applied (`false`: library default kept).
    Applied(bool),
    /// The user's `MIMALLOC_PAGE_COMMIT_ON_DEMAND` decides (mimalloc already read it).
    UserVariable,
    /// The linked library has no such option (mimalloc v2, feature `mimalloc-v2`).
    NotInLibrary,
}

/// mimalloc pages committed on demand instead of entirely at creation (equivalent of
/// `MIMALLOC_PAGE_COMMIT_ON_DEMAND=1`). To call first thing in `main`, before the large
/// allocations. Switch `RUSTCRAFT_OPT_COMMITDEMAND` (default 1, `=0`: behavior of the
/// library). A `MIMALLOC_PAGE_COMMIT_ON_DEMAND` variable set by the user always
/// wins (mimalloc already read it). Quiet-machine bench (`target/perf/ram-table.md`): private
/// memory when paused 1666 → 931 MB, flight 2376 → 1612, cave 2066 → 1164, fps, p99 and CPU equal.
pub fn commit_on_demand() -> CommitOnDemand {
    let Some(index) = mimalloc_options::PAGE_COMMIT_ON_DEMAND else { return CommitOnDemand::NotInLibrary };
    if rc_data::env_os!("MIMALLOC_PAGE_COMMIT_ON_DEMAND").is_some() {
        return CommitOnDemand::UserVariable;
    }
    let on = rc_data::opt_switch!("RUSTCRAFT_OPT_COMMITDEMAND");
    if on {
        set_mimalloc_option(index, 1);
    }
    CommitOnDemand::Applied(on)
}

/// Variables under which mimalloc reads its purge delay: the current name, then the legacy one it
/// still honours (v2 and v3).
const PURGE_DELAY_VARIABLES: [&str; 2] = ["MIMALLOC_PURGE_DELAY", "MIMALLOC_RESET_DELAY"];

/// The user set mimalloc's purge delay under one of its names (`is_set` reads one variable).
fn user_set_purge_delay(is_set: impl Fn(&str) -> bool) -> bool {
    PURGE_DELAY_VARIABLES.into_iter().any(is_set)
}

/// How much longer `RUSTCRAFT_OPT_MI_PURGE_LONG` keeps freed pages committed than the library
/// does: relative, because the library's delay differs by version (v3 1 s, v2 10 ms).
const PURGE_DELAY_FACTOR: i64 = 10;

/// Long delay for a library delay of `library_ms`; none when the library never purges (negative)
/// or purges at once (0): there is nothing to lengthen.
fn long_purge_delay_from(library_ms: i64) -> Option<i64> {
    (library_ms > 0).then(|| library_ms * PURGE_DELAY_FACTOR)
}

/// Purge delay of the linked library, milliseconds (its default unless set).
// `c_long` is 32 bits on Windows and 64 on Linux: the conversion to i64 is only useless on Linux.
#[allow(clippy::useless_conversion)]
fn library_purge_delay(index: i32) -> i64 {
    unsafe extern "C" {
        fn mi_option_get(option: i32) -> std::ffi::c_long;
    }
    // SAFETY: function of the linked mimalloc library; index read from its header (`build.rs`).
    i64::from(unsafe { mi_option_get(index) })
}

/// Longer purge delay of mimalloc, OFF by default (`RUSTCRAFT_OPT_MI_PURGE_LONG=1` turns it on):
/// freed pages stay committed ten times the library's delay (v3 1 s → 10 s, v2 10 ms → 100 ms),
/// fewer decommit/recommit cycles while chunks come and go. It can only trade memory for
/// processor time, to decide on the scene bench. A purge delay set by the user under either of
/// its names wins. Returns the delay set, if any.
pub fn long_purge_delay() -> Option<i64> {
    let index = mimalloc_options::PURGE_DELAY?;
    // Read once, at startup: names from a fixed list, hence the raw read.
    if user_set_purge_delay(|name| rc_data::env_flags::read_os(name).is_some())
        || !rc_data::opt_switch!("RUSTCRAFT_OPT_MI_PURGE_LONG", default_off)
    {
        return None;
    }
    let delay = long_purge_delay_from(library_purge_delay(index))?;
    set_mimalloc_option(index, delay);
    Some(delay)
}

/// Build with `mimalloc-win-direct-tls`: mimalloc reads its thread heap from one of the 64 direct
/// TLS slots only, and reports an error if the one it took at its start was higher. Returns the
/// lowest slot still free now (taken and given back): below 64, mimalloc's slots, taken earlier
/// while fewer were in use, are below 64 too, unless one was freed in between.
#[cfg(all(windows, feature = "mimalloc-win-direct-tls"))]
pub fn free_tls_slot_probe() -> Option<u32> {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn TlsAlloc() -> u32;
        fn TlsFree(index: u32) -> i32;
    }
    const TLS_OUT_OF_INDEXES: u32 = u32::MAX;
    // SAFETY: a slot taken and given back at once, never used.
    let index = unsafe { TlsAlloc() };
    if index == TLS_OUT_OF_INDEXES {
        return None;
    }
    // SAFETY: slot obtained just above.
    unsafe { TlsFree(index) };
    Some(index)
}

/// The calling thread reclaims the blocks that other threads handed back to it (mimalloc keeps them in a
/// deferred list until its next allocation on the same page) and returns the empty pages.
pub fn reclaim_freed() {
    unsafe extern "C" {
        fn mi_collect(force: bool);
    }
    // SAFETY: function of the linked mimalloc library (global allocator), no precondition.
    unsafe { mi_collect(false) }
}

/// mimalloc v3 or later is linked (v2 with the feature `mimalloc-v2`).
const MIMALLOC_V3: bool = mimalloc_options::LIBRARY_VERSION >= 30_000;

/// Version of `mi_stats_t` (header `mimalloc-stats.h`): 5 in v3, 4 in v2.
const MI_STATS_VERSION: usize = if MIMALLOC_V3 { 5 } else { 4 };

/// 64-bit words of `mi_stats_t` after the `committed` counter: v2 has fewer counters.
const MI_STATS_REST_WORDS: usize = if MIMALLOC_V3 { 535 } else { 510 };

/// Global statistics of mimalloc (header `mimalloc-stats.h`): only the first three counters are
/// named, the rest is opaque. The library refuses the copy if the size or the version do not
/// match, so a wrong layout shows.
#[repr(C)]
struct MiStats {
    _size: usize,
    _version: usize,
    _pages: [i64; 3],
    _reserved: [i64; 3],
    /// Bytes committed by the allocator: total, peak, current.
    committed: [i64; 3],
    _rest: [i64; MI_STATS_REST_WORDS],
}
// Size of `mi_stats_t` of the linked library (libmimalloc-sys 0.1.49: v3 4,368 bytes, v2 4,168).
const _: () = assert!(std::mem::size_of::<MiStats>() == if MIMALLOC_V3 { 4368 } else { 4168 });

/// Memory committed by mimalloc: (current, peak), bytes (`[perf] ram` reading). Reads the
/// allocator's internal counter: on Windows, mimalloc's "process info" call replaces this
/// counter with the private memory of the whole process, and "outside the allocator" was then 0.
/// `(0, 0)` if the library refuses the structure.
pub fn allocator_commit() -> (u64, u64) {
    unsafe extern "C" {
        fn mi_stats_get(stats: *mut MiStats) -> bool;
    }
    let mut s = MiStats {
        _size: std::mem::size_of::<MiStats>(),
        _version: MI_STATS_VERSION,
        _pages: [0; 3],
        _reserved: [0; 3],
        committed: [0; 3],
        _rest: [0; MI_STATS_REST_WORDS],
    };
    // SAFETY: function of the linked mimalloc library (global allocator); it checks the
    // announced size and version before writing into the structure.
    if !unsafe { mi_stats_get(&mut s) } {
        return (0, 0);
    }
    (s.committed[2].max(0) as u64, s.committed[1].max(0) as u64)
}

/// (busy, total) time of each logical processor since boot, in the system's units; empty if the
/// system does not tell (F3+L processor loads).
pub fn cpu_times() -> Vec<(u64, u64)> {
    #[cfg(windows)]
    {
        const PROCESSOR_TIMES: u32 = 8;
        let mut v: Vec<win::ProcessorTimes> = Vec::with_capacity(256);
        let size = std::mem::size_of::<win::ProcessorTimes>();
        let mut got = 0u32;
        // SAFETY: buffer of 256 entries of the announced byte length; the system writes at most
        // that many entries and returns the byte count actually written.
        let status = unsafe {
            win::NtQuerySystemInformation(PROCESSOR_TIMES, v.as_mut_ptr().cast(), (256 * size) as u32, &mut got)
        };
        if status < 0 {
            return Vec::new();
        }
        // SAFETY: `got` bytes were written, whole entries of integers.
        unsafe { v.set_len((got as usize / size).min(256)) };
        v.iter()
            .map(|t| {
                let total = (t.kernel + t.user).max(0) as u64;
                (total.saturating_sub(t.idle.max(0) as u64), total)
            })
            .collect()
    }
    #[cfg(not(windows))]
    {
        // `cpuN user nice system idle iowait irq softirq steal …`: idle = idle + iowait.
        let Ok(stat) = std::fs::read_to_string("/proc/stat") else { return Vec::new() };
        stat.lines()
            .filter(|l| l.starts_with("cpu") && l.as_bytes().get(3).is_some_and(u8::is_ascii_digit))
            .map(|l| {
                let v: Vec<u64> = l.split_whitespace().skip(1).take(8).map(|x| x.parse().unwrap_or(0)).collect();
                let total: u64 = v.iter().sum();
                let idle = v.get(3).copied().unwrap_or(0) + v.get(4).copied().unwrap_or(0);
                (total.saturating_sub(idle), total)
            })
            .collect()
    }
}

/// CPU time of the whole process (kernel + user), in nanoseconds (measurements).
pub fn process_cpu_ns() -> u64 {
    #[cfg(windows)]
    {
        let (mut c, mut e, mut k, mut u) = (0u64, 0u64, 0u64, 0u64);
        // SAFETY: pseudo-handle of the current process, four valid outputs.
        if unsafe { win::GetProcessTimes(win::GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) } != 0 {
            return (k + u) * 100;
        }
    }
    0
}

/// Age of the process in milliseconds, counted from its creation by the system (loading
/// of the executable and libraries included); `None` outside Windows or if the call fails.
pub fn process_age_ms() -> Option<f64> {
    #[cfg(windows)]
    {
        let (mut c, mut e, mut k, mut u, mut now) = (0u64, 0u64, 0u64, 0u64, 0u64);
        // SAFETY: pseudo-handle of the current process, valid outputs.
        unsafe {
            if win::GetProcessTimes(win::GetCurrentProcess(), &mut c, &mut e, &mut k, &mut u) == 0 {
                return None;
            }
            win::GetSystemTimePreciseAsFileTime(&mut now);
        }
        // Units of 100 ns.
        Some(now.saturating_sub(c) as f64 / 1e4)
    }
    #[cfg(not(windows))]
    {
        None
    }
}

/// Copies `text` to the clipboard; empty string: nothing.
pub fn set_clipboard(text: &str) -> bool {
    if text.is_empty() {
        return false;
    }
    #[cfg(windows)]
    {
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        // SAFETY: movable block allocated to the copied size; handed to the system by SetClipboardData,
        // freed here only if the handover fails.
        unsafe {
            if win::OpenClipboard(0) == 0 {
                return false;
            }
            win::EmptyClipboard();
            let mem = win::GlobalAlloc(0x0002, wide.len() * 2);
            let mut ok = false;
            if mem != 0 {
                let p = win::GlobalLock(mem);
                if !p.is_null() {
                    std::ptr::copy_nonoverlapping(wide.as_ptr().cast::<u8>(), p, wide.len() * 2);
                    win::GlobalUnlock(mem);
                    ok = win::SetClipboardData(13, mem) != 0;
                }
                if !ok {
                    win::GlobalFree(mem);
                }
            }
            win::CloseClipboard();
            return ok;
        }
    }
    // Linux: desktop tools (Wayland then X11), text passed on standard input.
    #[cfg(all(unix, not(target_os = "macos")))]
    for cmd in [&["wl-copy"][..], &["xclip", "-selection", "clipboard"], &["xsel", "-bi"]] {
        use std::io::Write;
        let Ok(mut child) =
            std::process::Command::new(cmd[0]).args(&cmd[1..]).stdin(std::process::Stdio::piped()).spawn()
        else {
            continue;
        };
        let wrote = child.stdin.take().is_some_and(|mut i| i.write_all(text.as_bytes()).is_ok());
        if wrote && child.wait().is_ok_and(|s| s.success()) {
            return true;
        }
    }
    #[allow(unreachable_code)]
    false
}

/// Commercial name of the processor (multiple spaces collapsed), `<unknown>` if unreadable.
pub fn cpu_name() -> String {
    #[cfg(target_arch = "x86_64")]
    {
        use std::arch::x86_64::__cpuid;
        let max = __cpuid(0x8000_0000).eax;
        if max >= 0x8000_0004 {
            let mut bytes = Vec::with_capacity(48);
            for leaf in 0x8000_0002u32..=0x8000_0004 {
                let r = __cpuid(leaf);
                for v in [r.eax, r.ebx, r.ecx, r.edx] {
                    bytes.extend_from_slice(&v.to_le_bytes());
                }
            }
            let s = String::from_utf8_lossy(&bytes).trim_matches(char::from(0)).to_owned();
            let s = s.split_whitespace().collect::<Vec<_>>().join(" ");
            if !s.is_empty() {
                return s;
            }
        }
    }
    "<unknown>".into()
}

#[cfg(all(test, windows))]
mod tests {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn VirtualAlloc(addr: *mut u8, size: usize, kind: u32, protect: u32) -> *mut u8;
        fn VirtualFree(addr: *mut u8, size: usize, kind: u32) -> i32;
    }

    const MIB: u64 = 1 << 20;

    /// Memory outside mimalloc: private memory of the process minus what the allocator has committed.
    fn outside() -> i64 {
        let (private, _) = super::process_memory();
        let (commit, _) = super::allocator_commit();
        private as i64 - commit as i64
    }

    /// 64 MB committed directly with the system (like a graphics driver) must appear
    /// as "outside the allocator", and 64 MB taken from the heap must appear in mimalloc's committed.
    #[test]
    fn allocator_commit_separates_heap_from_the_rest() {
        let n = 64 * MIB as usize;
        let before = outside();
        // SAFETY: reservation + commit of a new range, freed below.
        let p = unsafe { VirtualAlloc(std::ptr::null_mut(), n, 0x1000 | 0x2000, 0x04) };
        assert!(!p.is_null());
        let grown = outside() - before;
        // SAFETY: range obtained above, freed entirely.
        unsafe { VirtualFree(p, 0, 0x8000) };
        assert!((48 * MIB as i64..=96 * MIB as i64).contains(&grown), "outside the allocator +{} MB", grown / MIB as i64);

        let (c0, _) = super::allocator_commit();
        // mimalloc can serve a block from memory it already committed for the earlier tests of
        // this process: asking for more than all of it forces the commit to grow.
        let v = vec![1u8; c0 as usize + n];
        let (c1, peak) = super::allocator_commit();
        assert!(c1 >= c0 + 48 * MIB, "mimalloc committed {} -> {} MB", c0 / MIB, c1 / MIB);
        assert!(peak >= c1);
        drop(v);
    }
}
