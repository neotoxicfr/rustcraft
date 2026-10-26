//! System readings of the debug screen (`debug-f3.md` § 3.4, § 3.5) and clipboard
//! (F3+C, F3+I): direct calls to the system, no dependencies. Outside Windows: null values.
#![allow(unsafe_code)]

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// Allocation rate: counter of allocated bytes, kept only when the `memory` entry
/// is active (otherwise one boolean read per allocation).
pub static COUNT_ALLOCS: AtomicBool = AtomicBool::new(false);
pub static ALLOCATED: AtomicU64 = AtomicU64::new(0);

/// Global allocator: mimalloc, plus the allocated-bytes counter when requested.
#[cfg_attr(feature = "memstats", allow(dead_code))]
pub struct Gated;

// SAFETY: delegates every call to mimalloc with the same arguments.
unsafe impl std::alloc::GlobalAlloc for Gated {
    #[inline]
    unsafe fn alloc(&self, l: std::alloc::Layout) -> *mut u8 {
        if COUNT_ALLOCS.load(Ordering::Relaxed) {
            ALLOCATED.fetch_add(l.size() as u64, Ordering::Relaxed);
        }
        if crate::alloc_trace::TRACE.load(Ordering::Relaxed) {
            crate::alloc_trace::sample(l.size());
        }
        // SAFETY: same contract as the wrapped allocator.
        unsafe { mimalloc::MiMalloc.alloc(l) }
    }
    #[inline]
    unsafe fn alloc_zeroed(&self, l: std::alloc::Layout) -> *mut u8 {
        if COUNT_ALLOCS.load(Ordering::Relaxed) {
            ALLOCATED.fetch_add(l.size() as u64, Ordering::Relaxed);
        }
        if crate::alloc_trace::TRACE.load(Ordering::Relaxed) {
            crate::alloc_trace::sample(l.size());
        }
        // SAFETY: same contract as the wrapped allocator.
        unsafe { mimalloc::MiMalloc.alloc_zeroed(l) }
    }
    #[inline]
    unsafe fn dealloc(&self, p: *mut u8, l: std::alloc::Layout) {
        // SAFETY: `p` comes from `alloc` with the same layout.
        unsafe { mimalloc::MiMalloc.dealloc(p, l) }
    }
    #[inline]
    unsafe fn realloc(&self, p: *mut u8, l: std::alloc::Layout, n: usize) -> *mut u8 {
        if COUNT_ALLOCS.load(Ordering::Relaxed) && n > l.size() {
            ALLOCATED.fetch_add((n - l.size()) as u64, Ordering::Relaxed);
        }
        if n > l.size() && crate::alloc_trace::TRACE.load(Ordering::Relaxed) {
            crate::alloc_trace::sample(n - l.size());
        }
        // SAFETY: same contract as the wrapped allocator.
        unsafe { mimalloc::MiMalloc.realloc(p, l, n) }
    }
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
        pub fn GetLocaleInfoEx(name: *const u16, lctype: u32, data: *mut u16, len: i32) -> i32;
    }

    #[link(name = "user32")]
    unsafe extern "system" {
        pub fn OpenClipboard(owner: isize) -> i32;
        pub fn EmptyClipboard() -> i32;
        pub fn SetClipboardData(format: u32, mem: isize) -> isize;
        pub fn CloseClipboard() -> i32;
    }
}

/// Country of the user's regional language, three-letter ISO 3166 code (`FRA`, `KOR`);
/// `None` outside Windows or if the system does not provide it.
pub fn country_iso3() -> Option<String> {
    #[cfg(windows)]
    {
        // Three-letter country code of a regional language.
        const LOCALE_SISO3166CTRYNAME2: u32 = 0x68;
        let mut buf = [0u16; 16];
        // SAFETY: null name = user's regional language; 16-character buffer.
        let n = unsafe { win::GetLocaleInfoEx(std::ptr::null(), LOCALE_SISO3166CTRYNAME2, buf.as_mut_ptr(), 16) };
        (n > 1).then(|| String::from_utf16_lossy(&buf[..n as usize - 1]))
    }
    #[cfg(not(windows))]
    None
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

/// Index of `mi_option_page_commit_on_demand` in the `mi_option_t` enumeration of the linked
/// library (libmimalloc-sys 0.1.49, mimalloc v3, `include/mimalloc.h`): to recheck on every bump.
const MI_OPTION_PAGE_COMMIT_ON_DEMAND: i32 = 40;

/// mimalloc pages committed on demand instead of entirely at creation (equivalent of
/// `MIMALLOC_PAGE_COMMIT_ON_DEMAND=1`). To call first thing in `main`, before the large
/// allocations. Switch `RUSTCRAFT_OPT_COMMITDEMAND` (default 1, `=0`: behavior of the
/// library). A `MIMALLOC_PAGE_COMMIT_ON_DEMAND` variable set by the user always
/// wins (mimalloc already read it). Quiet-machine bench (`target/perf/ram-table.md`): private
/// memory when paused 1666 → 931 MB, flight 2376 → 1612, cave 2066 → 1164, fps, p99 and CPU equal.
/// Returns the applied value (`None`: nothing touched).
pub fn commit_on_demand() -> Option<bool> {
    unsafe extern "C" {
        fn mi_option_set(option: i32, value: std::ffi::c_long);
    }
    if rc_data::env_os!("MIMALLOC_PAGE_COMMIT_ON_DEMAND").is_some() {
        return None;
    }
    let on = rc_data::env_os!("RUSTCRAFT_OPT_COMMITDEMAND").is_none_or(|v| v != "0");
    if on {
        // SAFETY: function of the linked mimalloc library (global allocator); the index is
        // that of the option in the header of the linked version, the value 1 is documented.
        unsafe { mi_option_set(MI_OPTION_PAGE_COMMIT_ON_DEMAND, 1) }
    }
    Some(on)
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

/// Global statistics of mimalloc v3 (header `mimalloc-stats.h`, version 5): only the first three
/// counters are named, the rest (4280 bytes) is opaque. The library refuses the
/// copy if the size or the version do not match, so a wrong layout shows.
#[repr(C)]
struct MiStats {
    _size: usize,
    _version: usize,
    _pages: [i64; 3],
    _reserved: [i64; 3],
    /// Bytes committed by the allocator: total, peak, current.
    committed: [i64; 3],
    _rest: [i64; 535],
}
// Size of `mi_stats_t` of the linked library (libmimalloc-sys 0.1.49, mimalloc v3).
const _: () = assert!(std::mem::size_of::<MiStats>() == 4368);

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
        _version: 5,
        _pages: [0; 3],
        _reserved: [0; 3],
        committed: [0; 3],
        _rest: [0; 535],
    };
    // SAFETY: function of the linked mimalloc library (global allocator); it checks the
    // announced size and version before writing into the structure.
    if !unsafe { mi_stats_get(&mut s) } {
        return (0, 0);
    }
    (s.committed[2].max(0) as u64, s.committed[1].max(0) as u64)
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
        let v = vec![1u8; n];
        let (c1, peak) = super::allocator_commit();
        assert!(c1 >= c0 + 48 * MIB, "mimalloc committed {} -> {} MB", c0 / MIB, c1 / MIB);
        assert!(peak >= c1);
        drop(v);
    }
}
