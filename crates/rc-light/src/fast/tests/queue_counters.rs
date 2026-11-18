//! Queue counters of the perf log (`stats::queues`): exact on a known case, and without any
//! effect on the light (sheets and touched sections identical with the counting on or off, on
//! the sequential and multi-core paths); a counting that disturbs the queues is caught.

use super::*;
use crate::stats;

/// The same suite relit twice from the same world, queue counting off then on.
fn counted_matches_uncounted(case: &Case, o: Opts) -> Result<(), String> {
    let mut rng = Rng(case.seed);
    let mut off = world(case.height, case.garbage, &mut rng);
    let mut on = off.clone();
    for step in 0..case.steps {
        let ps = changes(&mut rng, case.height);
        for &p in &ps {
            let s = (case.state)(&mut rng);
            set(&mut off, p, s);
            set(&mut on, p, s);
        }
        let (mut touched_off, mut touched_on) = ([Touched::new()], [Touched::new()]);
        stats::force_queue_counting(Some(false));
        relight_zones(&mut off, case.height, case.t, case.has_sky, &ps, None, o, &mut touched_off);
        stats::force_queue_counting(Some(true));
        relight_zones(&mut on, case.height, case.t, case.has_sky, &ps, None, o, &mut touched_on);
        stats::force_queue_counting(None);
        if off != on {
            return Err(format!("sheets differ at step {step} ({} changes)", ps.len()));
        }
        if touched_off != touched_on {
            return Err(format!("touched sections differ at step {step}"));
        }
    }
    Ok(())
}

/// A torch alone in air without sky: one seed at 14, then each level holds the cells of one
/// distance shell (4 d² + 2 cells at distance d, each pushed once), level 1 the last shell;
/// cells = the block zone (33 × 33 × 33 with its margins). Tests run without `RUSTCRAFT_PERFLOG`
/// (with it, every test thread would add to the counters).
fn torch_queues_are_exact(t: &LightTables) {
    let mut w = world(64, false, &mut Rng(1));
    let torch = [8, 32, 8];
    set(&mut w, torch, TORCH);
    stats::force_queue_counting(Some(true));
    let before = stats::queues();
    let mut out = [Touched::new()];
    // The zone's queues: the incremental (checked or not) counts none.
    relight_zones(&mut w, 64, t, false, &[torch], None, shipped_zone(), &mut out);
    let got = stats::queues().since(&before);
    stats::force_queue_counting(None);
    let mut seeded = [0u64; 16];
    seeded[14] = 1;
    let mut queued = [0u64; 16];
    queued[14] = 1;
    for d in 1..=13u64 {
        queued[14 - d as usize] = 4 * d * d + 2;
    }
    assert_eq!(got.seeded[stats::BLOCK], seeded, "block seeds");
    assert_eq!(got.queued[stats::BLOCK], queued, "block entries per level");
    assert_eq!(got.cells[stats::BLOCK], 33 * 33 * 33, "block zone cells");
    assert_eq!((got.seeded[stats::SKY], got.cells[stats::SKY]), ([0; 16], 0), "no sky resolution");
}

/// One test (the counters are global to the process): exact case, then equivalence on every path,
/// then the sabotage (level 14 seeds dropped by the counting) caught.
#[test]
fn queue_counters_are_exact_and_change_nothing() {
    let t = tables();
    torch_queues_are_exact(&t);
    let sequential = Opts::from_bits(!DEDUP & !PAR_BITS);
    let parallel = Opts { join_min: 0, par_min: 0, chunk: 64, ..Opts::from_bits(!DEDUP) };
    for (seed, has_sky, garbage) in [(51u64, true, false), (52, true, true), (53, false, true)] {
        let case = |t| Case { t, height: 64, has_sky, garbage, steps: 300, seed, state: &state };
        counted_matches_uncounted(&case(&t), sequential).unwrap_or_else(|e| panic!("sequential, seed {seed}: {e}"));
        for threads in [1, 8] {
            pool(threads)
                .install(|| counted_matches_uncounted(&case(&t), parallel))
                .unwrap_or_else(|e| panic!("{threads} threads, seed {seed}: {e}"));
        }
    }
    let caught = (0..4u64).any(|seed| {
        SABOTAGE.with(|s| s.set(27));
        let case = Case { t: &t, height: 64, has_sky: true, garbage: seed % 2 == 1, steps: 300, seed, state: &state };
        let r = counted_matches_uncounted(&case, sequential);
        SABOTAGE.with(|s| s.set(0));
        r.is_err()
    });
    assert!(caught, "counting that drops the level 14 seeds not detected");
}
