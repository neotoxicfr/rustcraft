//! Helper threads for the short parallel jobs of the server (the rays and the visible parts of an
//! explosion, the search of an exit portal's site, the parsing of the advancement tree).
//!
//! Two causes made those jobs lose on several threads: OS threads started for each job (50 to
//! 100 µs a thread on Windows, more than an explosion's rays take), and the global thread pool,
//! which chunk generation keeps busy (a job of a few hundred microseconds queued behind
//! generation tasks). Here the threads are started once, when a world opens, and kept apart from
//! generation, and the calling thread takes jobs itself instead of waiting idle.

use std::sync::{Mutex, OnceLock, PoisonError};

/// Threads the parallel jobs use: `RUSTCRAFT_THREADS`, otherwise every logical core; read once.
pub(crate) fn logical_cores() -> usize {
    static CORES: OnceLock<usize> = OnceLock::new();
    *CORES.get_or_init(|| {
        let forced = rc_data::env_str!("RUSTCRAFT_THREADS").and_then(|v| v.trim().parse().ok()).filter(|&n| n > 0);
        forced.unwrap_or_else(|| std::thread::available_parallelism().map_or(1, std::num::NonZero::get))
    })
}

/// Helper pools already built, one per thread count (the benches compare several).
static POOLS: Mutex<Vec<(usize, &'static rayon::ThreadPool)>> = Mutex::new(Vec::new());

/// Starts the helper threads of the game on a thread of their own, when a world opens: built on
/// the first explosion instead, they cost that tick one OS thread start per core (50 to 100 µs
/// each on Windows, a few milliseconds on 32 cores).
pub(crate) fn start_helpers() -> std::thread::JoinHandle<()> {
    std::thread::spawn(|| {
        pool_of(logical_cores());
    })
}

/// Whether the helper pool of `threads` threads is already built.
#[cfg(test)]
fn is_built(threads: usize) -> bool {
    POOLS.lock().unwrap_or_else(PoisonError::into_inner).iter().any(|(n, _)| *n == threads.max(1))
}

/// The helper pool of `threads` threads, built on first use and kept for the process.
pub(crate) fn pool_of(threads: usize) -> &'static rayon::ThreadPool {
    let threads = threads.max(1);
    let mut pools = POOLS.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(&(_, pool)) = pools.iter().find(|(n, _)| *n == threads) {
        return pool;
    }
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(threads)
        .thread_name(|i| format!("server-helper-{i}"))
        .build()
        .expect("server helper threads");
    let pool: &'static rayon::ThreadPool = Box::leak(Box::new(pool));
    pools.push((threads, pool));
    pool
}

/// Runs `jobs` on the calling thread and the helper pool of `threads` threads and returns their
/// results in job order. Only as many helpers wake as there are jobs beyond the first, so the
/// caller sizes `jobs` to the work.
///
/// Whoever is free takes the next job from one queue, the calling thread included: it keeps
/// taking jobs instead of waiting for a helper the system has not scheduled yet (cores busy
/// with chunk generation or another program), so a helper that starts late finds the queue
/// empty and the tick only waits for the jobs already running.
pub(crate) fn run_in_order<T, F>(threads: usize, jobs: Vec<F>) -> Vec<T>
where
    T: Send,
    F: FnOnce() -> T + Send,
{
    let count = jobs.len();
    if count < 2 {
        return jobs.into_iter().map(|job| job()).collect();
    }
    let queue = Mutex::new(jobs.into_iter().enumerate());
    let finished = Mutex::new(Vec::with_capacity(count));
    let take_jobs = || {
        while let Some((index, job)) = next_job(&queue) {
            let result = job();
            finished.lock().unwrap_or_else(PoisonError::into_inner).push((index, result));
        }
    };
    let take_jobs = &take_jobs;
    pool_of(threads).in_place_scope(|scope| {
        for _ in 1..count {
            scope.spawn(move |_| take_jobs());
        }
        take_jobs();
    });
    let mut finished = finished.into_inner().unwrap_or_else(PoisonError::into_inner);
    finished.sort_unstable_by_key(|&(index, _)| index);
    finished.into_iter().map(|(_, result)| result).collect()
}

/// The next job of `queue` with its index; the lock is released before the job runs.
fn next_job<J: Iterator>(queue: &Mutex<J>) -> Option<J::Item> {
    queue.lock().unwrap_or_else(PoisonError::into_inner).next()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Results come back in the order of the jobs, whatever thread ran them, for 0 to many jobs.
    #[test]
    fn results_keep_the_order_of_the_jobs() {
        for (threads, jobs) in [(1, 0), (1, 1), (1, 5), (4, 4), (4, 37), (8, 3)] {
            let work: Vec<_> = (0..jobs)
                .map(|i| {
                    move || {
                        // Uneven jobs, so that they finish out of order.
                        std::thread::sleep(std::time::Duration::from_micros(((jobs - i) * 50) as u64));
                        i * 3
                    }
                })
                .collect();
            let got = run_in_order(threads, work);
            assert_eq!(got, (0..jobs).map(|i| i * 3).collect::<Vec<_>>(), "{threads} threads, {jobs} jobs");
        }
    }

    /// With every helper busy, the calling thread runs every job itself instead of waiting for a
    /// helper to be free (the server thread during chunk generation).
    #[test]
    fn the_caller_takes_the_jobs_no_helper_has_started() {
        // A pool size no other test uses, so that keeping it busy slows nobody.
        const HELPERS: usize = 5;
        let started = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        // Every helper stays busy until the last job has been taken (not for a fixed 400 ms, which
        // a loaded machine could spend between the last start and the jobs); freed then, they only
        // find an empty queue. The 120 s bound is an anti-hang guard: a caller waiting for a
        // helper gets one after it, and the assertion below fails instead of the test hanging.
        let release = std::sync::Arc::new((std::sync::Mutex::new(false), std::sync::Condvar::new()));
        for _ in 0..HELPERS {
            let (started, release) = (started.clone(), release.clone());
            pool_of(HELPERS).spawn(move || {
                started.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let (done, cv) = &*release;
                let guard = done.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
                let _ = cv.wait_timeout_while(guard, std::time::Duration::from_secs(120), |done| !*done);
            });
        }
        while started.load(std::sync::atomic::Ordering::SeqCst) < HELPERS {
            std::thread::yield_now();
        }
        let caller = std::thread::current().id();
        let work: Vec<_> = (0..4)
            .map(|i| {
                let release = release.clone();
                move || {
                    if i == 3 {
                        // Last job out of the queue: nothing left for a freed helper.
                        *release.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = true;
                        release.1.notify_all();
                    }
                    std::thread::current().id()
                }
            })
            .collect();
        let ran_on = run_in_order(HELPERS, work);
        assert!(ran_on.iter().all(|&id| id == caller), "{ran_on:?}");
    }

    /// Lowers the calling thread below normal priority, as the client does for its generation
    /// pool (Windows; elsewhere nothing).
    #[cfg(windows)]
    #[allow(unsafe_code)]
    fn lower_this_thread_priority() {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentThread() -> isize;
            fn SetThreadPriority(thread: isize, priority: i32) -> i32;
        }
        // SAFETY: pseudo-handle of the current thread; -1 is the below-normal priority.
        unsafe {
            SetThreadPriority(GetCurrentThread(), -1);
        }
    }

    #[cfg(not(windows))]
    fn lower_this_thread_priority() {}

    /// Bench: how long a tick waits for 5 jobs of 15 µs (about a TNT's rays at a task per 4,096
    /// steps), on one thread and on the helpers, every core idle, then kept busy by as many
    /// spinning threads below normal priority (the client's generation pool), then at normal
    /// priority (another program); median, p99 and max of 2,000 rounds.
    #[test]
    #[ignore = "micro-bench: run alone on a quiet machine"]
    fn bench_helper_jobs_with_busy_cores() {
        use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
        use std::time::{Duration, Instant};
        let job = || {
            let start = Instant::now();
            while start.elapsed() < Duration::from_micros(15) {
                std::hint::spin_loop();
            }
        };
        for (busy, spinners_lowered) in [("idle", false), ("busy below normal", true), ("busy at normal", false)] {
            let stop = std::sync::Arc::new(AtomicBool::new(false));
            let spinners: Vec<_> = (0..if busy == "idle" { 0 } else { logical_cores() })
                .map(|_| {
                    let stop = stop.clone();
                    std::thread::spawn(move || {
                        if spinners_lowered {
                            lower_this_thread_priority();
                        }
                        while !stop.load(Relaxed) {
                            std::hint::spin_loop();
                        }
                    })
                })
                .collect();
            for (name, threads) in [("one thread", 1), ("helpers", logical_cores())] {
                let mut micros: Vec<f64> = (0..2000)
                    .map(|_| {
                        let start = Instant::now();
                        if threads == 1 {
                            (0..5).for_each(|_| job());
                        } else {
                            run_in_order(threads, vec![job; 5]);
                        }
                        start.elapsed().as_secs_f64() * 1e6
                    })
                    .collect();
                micros.sort_by(f64::total_cmp);
                let (median, p99, max) = (micros[1000], micros[1980], micros[1999]);
                eprintln!("helper jobs, cores {busy}, {name}: median {median:.1}, p99 {p99:.1}, max {max:.1} µs");
            }
            stop.store(true, Relaxed);
            spinners.into_iter().for_each(|s| s.join().expect("spinner"));
        }
    }

    /// Opening a world builds the helper pool, so that the first explosion does not.
    #[test]
    fn starting_the_helpers_builds_the_pool() {
        start_helpers().join().expect("helper start");
        assert!(is_built(logical_cores()));
    }
}
