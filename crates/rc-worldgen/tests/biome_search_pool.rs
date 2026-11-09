//! A biome search split in parallel batches (`/locate biome`), called from outside any thread pool
//! like the server's thread, must not depend on the global pool: chunk generation keeps that pool
//! busy, and a batch handed to it would wait behind the generation tasks. Its own test binary,
//! because the test holds every thread of the process's global pool.
//! (`cargo test --release -p rc-worldgen --test biome_search_pool`).

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use rc_worldgen::Generator;
use rc_worldgen::biome::{BiomeSource, Searcher, SplitPaths};

/// Points of a search around (900, −1,700): columns of step 32 over a square of 25 × 25, 7 heights,
/// in quarts (4,375 points, about 70 batches of 64).
fn search_point(i: usize) -> [i32; 3] {
    const YS: [i32; 7] = [64, 0, 128, -64, 192, 256, 316];
    let c = i / YS.len();
    let (dx, dz) = ((c % 25) as i32 - 12, (c / 25) as i32 - 12);
    [(900 + dx * 32) >> 2, YS[i % YS.len()] >> 2, (-1_700 + dz * 32) >> 2]
}
const POINTS: usize = 25 * 25 * 7;

/// Every thread of the global pool parked until [`GlobalPoolHeld::release`].
struct GlobalPoolHeld(Arc<(Mutex<bool>, Condvar)>);

impl GlobalPoolHeld {
    fn hold_every_thread() -> Self {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let parked = Arc::new(AtomicUsize::new(0));
        let (g, p) = (gate.clone(), parked.clone());
        rayon::spawn_broadcast(move |_| {
            p.fetch_add(1, Ordering::SeqCst);
            let (open, wake) = &*g;
            let mut open = open.lock().unwrap();
            while !*open {
                open = wake.wait(open).unwrap();
            }
        });
        while parked.load(Ordering::SeqCst) < rayon::current_num_threads() {
            std::thread::yield_now();
        }
        GlobalPoolHeld(gate)
    }

    fn release(&self) {
        let (open, wake) = &*self.0;
        *open.lock().unwrap() = true;
        wake.notify_all();
    }
}

#[test]
fn split_search_runs_while_the_global_pool_is_held() {
    let Some(pack) = common::data_pack() else { return };
    let g = Arc::new(Generator::new(&pack, "overworld", 0).unwrap());
    let (src, _) = BiomeSource::from_preset(&pack, "normal", "overworld").unwrap();
    let src = Arc::new(src);
    // No accepted biome: the walk goes through every batch.
    let ok = Arc::new(vec![false; src.names().len()]);
    let own = SplitPaths { split: true, hint: true, cand: true, own_pool: true };
    let search = |paths: SplitPaths| {
        let (g, src, ok) = (g.clone(), src.clone(), ok.clone());
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = tx.send(src.first_match(&g, POINTS, search_point, &ok, &mut Searcher::default(), paths));
        });
        rx
    };
    let want = search(SplitPaths::OLD).recv().unwrap();

    let held = GlobalPoolHeld::hold_every_thread();
    let on_global_pool = search(SplitPaths { own_pool: false, ..own });
    let on_own_threads = search(own).recv_timeout(Duration::from_secs(120));
    // Checked once the global pool is free again, so that a failure does not leave its threads parked.
    let global_still_waiting = matches!(on_global_pool.try_recv(), Err(mpsc::TryRecvError::Empty));
    held.release();
    let on_global_pool = on_global_pool.recv().unwrap();

    assert_eq!(on_own_threads, Ok(want), "the split search on its own threads waited for the held global pool");
    assert!(global_still_waiting, "the split search on the global pool ran while every pool thread was held: this test proves nothing");
    assert_eq!(on_global_pool, want);
}
