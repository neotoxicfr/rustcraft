//! Helper threads for the short parallel jobs of the server (the rays and the visible parts of an
//! explosion, the search of an exit portal's site, the parsing of the advancement tree).
//!
//! Two causes made those jobs lose on several threads: OS threads started for each job (50 to
//! 100 µs a thread on Windows, more than an explosion's rays take), and the global thread pool,
//! which chunk generation keeps busy (a job of a few hundred microseconds queued behind
//! generation tasks). Here the threads are started once and kept apart from generation, and the
//! calling thread runs a share itself instead of waiting idle.

use std::sync::{Mutex, OnceLock, PoisonError};

/// Threads the parallel jobs use: `RUSTCRAFT_THREADS`, otherwise every logical core; read once.
pub(crate) fn logical_cores() -> usize {
    static CORES: OnceLock<usize> = OnceLock::new();
    *CORES.get_or_init(|| {
        let forced = rc_data::env_str!("RUSTCRAFT_THREADS").and_then(|v| v.trim().parse().ok()).filter(|&n| n > 0);
        forced.unwrap_or_else(|| std::thread::available_parallelism().map_or(1, std::num::NonZero::get))
    })
}

/// The helper pool of `threads` threads, built on first use and kept for the process (one per
/// thread count: the benches compare several).
pub(crate) fn pool_of(threads: usize) -> &'static rayon::ThreadPool {
    static POOLS: Mutex<Vec<(usize, &'static rayon::ThreadPool)>> = Mutex::new(Vec::new());
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

/// Runs `jobs` and returns their results in order: the first on the calling thread, the others
/// on the helper pool of `threads` threads. Only as many threads wake as there are jobs beyond
/// the first, so the caller sizes `jobs` to the work.
pub(crate) fn run_in_order<T, F>(threads: usize, jobs: Vec<F>) -> Vec<T>
where
    T: Send,
    F: FnOnce() -> T + Send,
{
    let mut results: Vec<Option<T>> = std::iter::repeat_with(|| None).take(jobs.len()).collect();
    let mut jobs = jobs.into_iter();
    let (Some(first), Some((own, others))) = (jobs.next(), results.split_first_mut()) else { return Vec::new() };
    pool_of(threads).in_place_scope(|scope| {
        for (job, slot) in jobs.zip(others) {
            scope.spawn(move |_| *slot = Some(job()));
        }
        *own = Some(first());
    });
    results.into_iter().map(|r| r.expect("every helper job ran")).collect()
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

    /// The first job runs on the calling thread, the others on the helper pool.
    #[test]
    fn the_caller_runs_the_first_job() {
        let caller = std::thread::current().id();
        let work: Vec<Box<dyn FnOnce() -> std::thread::ThreadId + Send>> = (0..4)
            .map(|_| Box::new(|| std::thread::current().id()) as Box<dyn FnOnce() -> std::thread::ThreadId + Send>)
            .collect();
        let ran_on = run_in_order(4, work);
        assert_eq!(ran_on[0], caller);
        assert!(ran_on[1..].iter().all(|&id| id != caller), "{ran_on:?}");
    }
}
