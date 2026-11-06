//! Global allocator: alignment and contents of every block whatever the path, byte counting,
//! option indices of the linked mimalloc, and the micro-bench of the paths.

use super::*;
use std::hint::black_box;

unsafe extern "C" {
    /// Bytes usable in a mimalloc block (its size class).
    fn mi_usable_size(p: *const std::ffi::c_void) -> usize;
}

/// Every power-of-two alignment `Layout` can take up to a page, and more.
const ALIGNMENTS: [usize; 13] = [1, 2, 4, 8, 16, 32, 64, 128, 256, 512, 1024, 2048, 4096];

/// Sizes 1 byte to 64 KiB: every one up to 300 (small classes), then a step that is not a
/// power of two (lands in every class above, on and off its bounds).
fn sizes() -> impl Iterator<Item = usize> {
    (1..=300).chain((300..=65_536).step_by(97)).chain([1024, 1025, 1536, 4096, 65_536])
}

/// Allocates, fills, grows and shrinks a block of `layout` through `alloc` / `realloc`,
/// checking its alignment and contents at each step.
///
/// # Safety
/// `alloc`, `alloc_zeroed`, `realloc` and `free` must form a valid allocator.
unsafe fn check_block(
    layout: Layout,
    alloc: unsafe fn(Layout, bool) -> *mut u8,
    realloc: unsafe fn(*mut u8, Layout, usize) -> *mut u8,
) {
    let (size, align) = (layout.size(), layout.align());
    let aligned = |p: *mut u8| !p.is_null() && (p as usize).is_multiple_of(align);
    // SAFETY: blocks of `layout` and its regrown sizes, written within their size, freed once.
    unsafe {
        let zeroed = alloc(layout, true);
        assert!(aligned(zeroed), "zeroed {size} B align {align}: {zeroed:p}");
        assert!(std::slice::from_raw_parts(zeroed, size).iter().all(|&b| b == 0), "{size} B align {align} not zeroed");
        mi::mi_free(zeroed.cast());

        let p = alloc(layout, false);
        assert!(aligned(p), "{size} B align {align}: {p:p}");
        let pattern = |i: usize| (i * 31 + 7) as u8;
        (0..size).for_each(|i| p.add(i).write(pattern(i)));
        let grown_size = size * 2 + 1;
        let grown = realloc(p, layout, grown_size);
        assert!(aligned(grown), "grown {size} -> {grown_size} B align {align}: {grown:p}");
        assert!((0..size).all(|i| grown.add(i).read() == pattern(i)), "{size} B align {align}: contents lost on growth");
        grown.add(grown_size - 1).write(1);
        let grown_layout = Layout::from_size_align_unchecked(grown_size, align);
        let shrunk_size = size.div_ceil(2);
        let shrunk = realloc(grown, grown_layout, shrunk_size);
        assert!(aligned(shrunk), "shrunk {grown_size} -> {shrunk_size} B align {align}: {shrunk:p}");
        assert!((0..shrunk_size).all(|i| shrunk.add(i).read() == pattern(i)), "{size} B align {align}: contents lost on shrink");
        mi::mi_free(shrunk.cast());
    }
}

/// The direct calls (plain mimalloc entry point when it gives the alignment) honour every
/// alignment from 1 to 4,096 bytes for every size from 1 byte to 64 KiB, zeroed or not, and
/// across growth and shrinking.
#[test]
fn direct_calls_honour_every_alignment_and_size() {
    for align in ALIGNMENTS {
        for size in sizes() {
            let layout = Layout::from_size_align(size, align).unwrap();
            // SAFETY: the direct calls form a valid allocator (what this test checks).
            unsafe { check_block(layout, mimalloc_alloc, mimalloc_realloc) };
        }
    }
}

/// Same check through the global allocator's own entry points under every switch combination
/// (direct or aligned calls, single or separate checks), counting on. Flips the process-wide
/// flags: every combination is a correct allocator, other tests running meanwhile are unharmed.
#[test]
fn every_switch_combination_honours_alignment() {
    unsafe fn alloc(layout: Layout, zeroed: bool) -> *mut u8 {
        // SAFETY: same contract as the caller's.
        unsafe { if zeroed { CountingAllocator.alloc_zeroed(layout) } else { CountingAllocator.alloc(layout) } }
    }
    unsafe fn realloc(p: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: same contract as the caller's.
        unsafe { CountingAllocator.realloc(p, layout, new_size) }
    }
    for flags in [ALIGNED_CALLS, SEPARATE_CHECKS, ALIGNED_CALLS | SEPARATE_CHECKS, COUNT_BYTES, 0] {
        set_allocator_flag(ALIGNED_CALLS, flags & ALIGNED_CALLS != 0);
        set_allocator_flag(SEPARATE_CHECKS, flags & SEPARATE_CHECKS != 0);
        for align in ALIGNMENTS {
            for size in sizes().step_by(7) {
                // SAFETY: the global allocator is valid (what this test checks).
                unsafe { check_block(Layout::from_size_align(size, align).unwrap(), alloc, realloc) };
            }
        }
    }
}

/// Why the plain call needs `align <= size`: 8-byte blocks of the plain call sit every 8 bytes,
/// so among 64 live ones at least one is not 16-aligned, while the direct call for 8 bytes
/// aligned on 16 (it must take the aligned entry point) gives 64 aligned ones. Fails if the
/// `align <= size` condition is dropped from `plain_call_aligns`.
#[test]
fn size_below_alignment_takes_the_aligned_call() {
    // SAFETY: blocks allocated and freed here, never written.
    unsafe {
        let plain: Vec<*mut std::ffi::c_void> = (0..64).map(|_| mi::mi_malloc(8)).collect();
        let misaligned = plain.iter().filter(|p| !(**p as usize).is_multiple_of(16)).count();
        plain.into_iter().for_each(|p| mi::mi_free(p));
        assert!(misaligned > 0, "the plain call happened to align 64 blocks of 8 bytes on 16");

        let layout = Layout::from_size_align(8, 16).unwrap();
        let direct: Vec<*mut u8> = (0..64).map(|_| mimalloc_alloc(layout, false)).collect();
        let misaligned = direct.iter().filter(|p| !(**p as usize).is_multiple_of(16)).count();
        direct.into_iter().for_each(|p| mi::mi_free(p.cast()));
        assert_eq!(misaligned, 0, "direct call: blocks of 8 bytes aligned on 16 came out on 8");
    }
}

/// Size class (usable bytes) of the block that a call gives for `layout`.
///
/// # Safety
/// `alloc` must be a valid mimalloc allocation for `layout`.
unsafe fn block_size(layout: Layout, alloc: unsafe fn(Layout) -> *mut u8) -> usize {
    // SAFETY: block allocated, measured and freed here.
    unsafe {
        let p = alloc(layout);
        let usable = mi_usable_size(p.cast());
        mi::mi_free(p.cast());
        usable
    }
}

unsafe fn direct_alloc(layout: Layout) -> *mut u8 {
    // SAFETY: same contract as the caller's.
    unsafe { mimalloc_alloc(layout, false) }
}

unsafe fn aligned_alloc(layout: Layout) -> *mut u8 {
    // SAFETY: same contract as the caller's.
    unsafe { mimalloc::MiMalloc.alloc(layout) }
}

/// Block bytes taken by the direct and the aligned calls over `layouts`, and how many layouts
/// the aligned call puts in a bigger size class. Fails if the direct call ever takes more.
fn block_bytes(layouts: impl Iterator<Item = Layout>) -> (usize, usize, usize) {
    let (mut direct_total, mut aligned_total, mut bigger) = (0, 0, 0);
    for layout in layouts {
        // SAFETY: valid mimalloc allocations of `layout`.
        let (direct, aligned) = unsafe { (block_size(layout, direct_alloc), block_size(layout, aligned_alloc)) };
        assert!(direct <= aligned, "{layout:?}: direct {direct} B, aligned {aligned} B");
        direct_total += direct;
        aligned_total += aligned;
        bigger += usize::from(direct < aligned);
    }
    (direct_total, aligned_total, bigger)
}

/// Memory taken above 1 KiB: the direct call never takes a bigger block than the aligned one.
/// The aligned call pads by `align - 1` bytes, so a size that sits exactly on a size-class bound
/// (classes go by quarters: 1,280, 1,536, 1,792, 2,048…) moves up one class. That is what a
/// `Vec` that doubles does for every element size that is not a power of two (12-byte elements:
/// 1,536 bytes at 128; mimalloc v3 only). Sweeps: every multiple of 8 from 1 KiB to 64 KiB (align 8), and the
/// doubling capacities of elements of 1 to 64 bytes (alignment: largest power of two dividing
/// the size, at most 8).
#[test]
fn direct_calls_take_no_bigger_blocks_above_one_kib() {
    let multiples_of_8 = (1032..=65_536).step_by(8).map(|size| Layout::from_size_align(size, 8).unwrap());
    let (direct, aligned, bigger) = block_bytes(multiples_of_8);
    eprintln!(
        "multiples of 8 above 1 KiB: direct {direct} B, aligned {aligned} B (+{:.2} %), {bigger} of 8,064 sizes one class up",
        (aligned as f64 / direct as f64 - 1.0) * 100.0
    );
    let vec_growth = (1..=64usize).flat_map(|element| {
        let align = (1 << element.trailing_zeros()).min(8);
        (0..16)
            .map(move |doubling| element << doubling)
            .filter(|&size| (1025..=65_536).contains(&size))
            .map(move |size| Layout::from_size_align(size, align).unwrap())
    });
    let (direct, aligned, bigger) = block_bytes(vec_growth.clone());
    eprintln!(
        "doubling Vec capacities above 1 KiB: direct {direct} B, aligned {aligned} B (+{:.2} %), {bigger} of {} one class up",
        (aligned as f64 / direct as f64 - 1.0) * 100.0,
        vec_growth.count()
    );
    // SAFETY: valid mimalloc allocations.
    let twelve_byte_elements = unsafe {
        let layout = Layout::from_size_align(1536, 4).unwrap();
        (block_size(layout, direct_alloc), block_size(layout, aligned_alloc))
    };
    // mimalloc v2 serves small alignments from the plain size class: only v3 pads.
    assert_eq!(twelve_byte_elements, (1536, if MIMALLOC_V3 { 1792 } else { 1536 }));
}

/// Bytes the fixed script allocates: 1,000 then 2,000 more by growth, then 40.
const SCRIPT_BYTES: u64 = 1000 + 2000 + 40;

/// Allocations of a known total, kept from the optimiser.
fn fixed_allocation_script() {
    let mut v: Vec<u8> = Vec::with_capacity(1000);
    black_box(&mut v);
    v.reserve_exact(3000);
    black_box(&mut v);
    black_box(Box::new([7u8; 40]));
}

/// With counting on, `ALLOCATED` grows by exactly what the script allocates (growth by
/// reallocation included), with the single flag and with the separate checks. The counter is
/// process-wide: tries again while other tests' allocations add to it or switch it off; a path
/// that miscounts never gives the exact total.
#[test]
fn allocated_bytes_counts_a_fixed_script() {
    for separate in [false, true] {
        set_allocator_flag(SEPARATE_CHECKS, separate);
        let exact = (0..1000).any(|_| allocated_bytes(fixed_allocation_script) == SCRIPT_BYTES);
        set_allocator_flag(SEPARATE_CHECKS, false);
        assert!(exact, "separate checks {separate}: never counted exactly {SCRIPT_BYTES} B");
    }
}

/// Option indices generated from the linked header, against those read by hand in the header of
/// each library version: a bump fails here until its `mimalloc.h` is read again (an inserted
/// option moves the following indices, and setting a wrong index changes another option).
#[test]
fn mimalloc_option_indices_were_checked_for_this_version() {
    use mimalloc_options as o;
    let expected = match o::LIBRARY_VERSION {
        30302 => (Some(40), Some(15)),
        20302 => (None, Some(15)),
        v => panic!("mimalloc {v}: read the option enumeration of its mimalloc.h, then note the indices here"),
    };
    assert_eq!((o::PAGE_COMMIT_ON_DEMAND, o::PURGE_DELAY), expected);
}

/// The old global allocator, as it was: two flags each loaded per allocation, then mimalloc's
/// aligned entry points (reference of the micro-bench).
struct TwoFlagAllocator;

static OLD_COUNT: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
static OLD_TRACE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

// SAFETY: delegates to mimalloc with the caller's arguments.
unsafe impl GlobalAlloc for TwoFlagAllocator {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if OLD_COUNT.load(Ordering::Relaxed) {
            ALLOCATED.fetch_add(layout.size() as u64, Ordering::Relaxed);
        }
        if OLD_TRACE.load(Ordering::Relaxed) {
            crate::alloc_trace::sample(layout.size());
        }
        // SAFETY: same contract as the caller's.
        unsafe { mimalloc::MiMalloc.alloc(layout) }
    }
    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: same contract as the caller's.
        unsafe { mimalloc::MiMalloc.dealloc(ptr, layout) }
    }
}

/// mimalloc's direct calls with no flag at all (lower bound of the micro-bench).
struct BareDirectCalls;

// SAFETY: delegates to mimalloc with the caller's arguments.
unsafe impl GlobalAlloc for BareDirectCalls {
    #[inline]
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: same contract as the caller's.
        unsafe { mimalloc_alloc(layout, false) }
    }
    #[inline]
    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        // SAFETY: same contract as the caller's.
        unsafe { mi::mi_free(ptr.cast()) }
    }
}

/// Blocks kept alive together in each round of the micro-bench (free lists in use, as in a frame).
const LIVE_BLOCKS: usize = 256;

/// Nanoseconds per allocation and free of `layout`, by rounds of `LIVE_BLOCKS` live blocks.
fn ns_per_allocation<A: GlobalAlloc>(allocator: &A, layout: Layout, allocations: usize) -> f64 {
    let mut live = [std::ptr::null_mut::<u8>(); LIVE_BLOCKS];
    let start = std::time::Instant::now();
    for _ in 0..allocations / LIVE_BLOCKS {
        for slot in &mut live {
            // SAFETY: valid layout; the block is freed below with the same layout.
            *slot = black_box(unsafe { allocator.alloc(layout) });
        }
        for &p in &live {
            // SAFETY: block allocated just above with `layout`.
            unsafe { allocator.dealloc(p, layout) };
        }
    }
    start.elapsed().as_nanos() as f64 / allocations as f64
}

/// Mean ns per allocation on each of `threads` threads running the same loop at once.
fn ns_per_allocation_on<A: GlobalAlloc + Sync>(allocator: &A, layout: Layout, threads: usize, allocations: usize) -> f64 {
    let total: f64 = std::thread::scope(|s| {
        let runs: Vec<_> = (0..threads).map(|_| s.spawn(|| ns_per_allocation(allocator, layout, allocations))).collect();
        runs.into_iter().map(|r| r.join().unwrap()).sum()
    });
    total / threads as f64
}

/// Micro-bench of the allocator paths, 10^7 allocations of 8 bytes (`Box::new(u64)`) and of
/// 1,536 bytes aligned on 8 (the padded case), on 1 thread then powers of 2 up to every logical
/// core. Run alone: `cargo test --release -p rc-app --bin rustcraft allocator_micro_bench --
/// --ignored --nocapture --test-threads=1`.
#[test]
#[ignore = "micro-bench"]
fn allocator_micro_bench() {
    const ALLOCATIONS: usize = 10_000_000;
    let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
    let thread_counts: Vec<usize> =
        std::iter::successors(Some(1), |&n| (n * 2 < cores).then_some(n * 2)).chain([cores]).collect();
    let with_flags = |flags: u8, layout: Layout, threads: usize| {
        ALLOCATOR_FLAGS.store(flags, Ordering::Relaxed);
        let ns = ns_per_allocation_on(&CountingAllocator, layout, threads, ALLOCATIONS);
        ALLOCATOR_FLAGS.store(0, Ordering::Relaxed);
        ns
    };
    /// Name of an allocator path and its timing for (layout, threads).
    type Variant<'a> = (&'a str, &'a dyn Fn(Layout, usize) -> f64);
    let variants: [Variant; 7] = [
        ("new", &|l, t| with_flags(0, l, t)),
        ("aligned calls", &|l, t| with_flags(ALIGNED_CALLS, l, t)),
        ("separate checks", &|l, t| with_flags(SEPARATE_CHECKS, l, t)),
        ("both old", &|l, t| with_flags(ALIGNED_CALLS | SEPARATE_CHECKS, l, t)),
        ("old code", &|l, t| ns_per_allocation_on(&TwoFlagAllocator, l, t, ALLOCATIONS)),
        ("bare mimalloc crate", &|l, t| ns_per_allocation_on(&mimalloc::MiMalloc, l, t, ALLOCATIONS)),
        ("bare direct", &|l, t| ns_per_allocation_on(&BareDirectCalls, l, t, ALLOCATIONS)),
    ];
    for (name, layout) in [("8 B", Layout::new::<u64>()), ("1536 B", Layout::from_size_align(1536, 8).unwrap())] {
        for &threads in &thread_counts {
            // Best of 5 rounds, the variants interleaved: the machine's noise hits them alike.
            let mut best = [f64::INFINITY; 7];
            for _ in 0..5 {
                for (b, (_, run)) in best.iter_mut().zip(&variants) {
                    *b = b.min(run(layout, threads));
                }
            }
            let row: Vec<String> = variants.iter().zip(best).map(|((v, _), ns)| format!("{v} {ns:.2}")).collect();
            eprintln!("{name}, {threads} thread(s), best ns per allocation: {}", row.join(", "));
        }
    }
}
