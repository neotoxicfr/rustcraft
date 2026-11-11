//! Player effect data micro-bench (`docs/perf.md`, player effect data), on a flat world, a player
//! carrying four visible effects:
//! - "sync-unchanged": the effects marked as touched (what an effect renewed every tick does, like
//!   the turtle helmet's) and the particle list unchanged: one `sync_effect_data`, nothing to send;
//! - "list-built-then-compared": the same check done the simpler way, the list built
//!   (`effect_particles`) then compared with a kept copy: what comparing in place saves;
//! - "sync-changing": one effect shown and hidden in turn: the list goes out at every sync;
//! - "particle-id-indexed" / "particle-id-walked": the id of the swirl particle through the
//!   registry's shared name index, or by walking its entries (`RUSTCRAFT_OPT_NAMEINDEX=0`);
//! - "spectator-tick": one player tick of a spectator (it sends its shared flags outside the
//!   environment pass), and "spectator-shared-flags": that added check alone.
//!
//! Nanoseconds and allocations per call (the test allocator's count for this thread; not with
//! `memstats`, which replaces it). The cases take turns within each round, so that a loaded
//! machine disturbs them alike. Run alone: `cargo test --release -p rc-sim
//! bench_player_effect_data -- --ignored --nocapture --test-threads=1`.

use std::hint::black_box;
use std::time::Instant;

use crate::bench_kit::{median, report};
use crate::blocks::probe::Probe;
use crate::effects::Inst;
use crate::test_alloc::allocations;
use crate::tests_g3f13::{finish, open};

const BENCH: &str = "player-effect-data";
const ROUNDS: usize = 9;

/// One case: its name, its calls per round and what one call does (given the call's number).
type Case<'a> = (&'static str, u32, Box<dyn FnMut(&mut Probe, u32) + 'a>);

/// Runs the cases in turn, `ROUNDS` times; median ns, best ns and median allocations per call.
fn report_cases(p: &mut Probe, mut cases: Vec<Case<'_>>) {
    let mut ns = vec![Vec::new(); cases.len()];
    let mut allocs = vec![Vec::new(); cases.len()];
    for _ in 0..ROUNDS {
        for (k, (_, calls, call)) in cases.iter_mut().enumerate() {
            let _ = p.take_events();
            let (start, before) = (Instant::now(), allocations());
            for n in 0..*calls {
                call(p, n);
                // The client's queue is emptied as it goes, never grown by the bench itself.
                if n % 256 == 0 {
                    let _ = p.take_events();
                }
            }
            ns[k].push(start.elapsed().as_nanos() as f64 / f64::from(*calls));
            allocs[k].push((allocations() - before) as f64 / f64::from(*calls));
        }
    }
    for (k, (case, ..)) in cases.iter().enumerate() {
        report(BENCH, case, "ns_per_call", median(&ns[k]));
        report(BENCH, case, "ns_per_call_best", ns[k].iter().copied().fold(f64::INFINITY, f64::min));
        if !cfg!(feature = "memstats") {
            report(BENCH, case, "allocs_per_call", median(&allocs[k]));
        }
    }
}

fn sync_touched_effects(p: &mut Probe) {
    p.s.life.fx.dirty = true;
    p.s.sync_effect_data();
}

#[test]
#[ignore]
fn bench_player_effect_data() {
    let (mut p, dir) = open("bench-player-effect-data");
    for id in ["minecraft:speed", "minecraft:haste", "minecraft:regeneration", "minecraft:infested"] {
        p.s.add_effect(Inst::new(id, 100_000_000, 0));
    }
    p.s.sync_effect_data();
    let (kept_list, _) = p.s.effect_particles(&p.s.life.fx.list);
    let particle_types = p.s.data.particle_types().expect("particle registry").clone();
    let swirl_id = |indexed: bool| particle_types.id_with(black_box("minecraft:entity_effect"), indexed);
    assert_eq!(swirl_id(true), swirl_id(false), "same id both ways");
    // What the walk costs: the entries compared before the swirl particle is found.
    report(BENCH, "particle-id-walked", "entries_compared", f64::from(swirl_id(false).map_or(0, |id| id + 1)));
    report_cases(
        &mut p,
        vec![
            ("sync-unchanged", 400_000, Box::new(|p: &mut Probe, _| sync_touched_effects(p))),
            (
                "list-built-then-compared",
                400_000,
                Box::new(|p: &mut Probe, _| {
                    let (list, _) = p.s.effect_particles(&p.s.life.fx.list);
                    black_box(list == kept_list);
                }),
            ),
            (
                "sync-changing",
                400_000,
                // Odd calls show the effect again: every round ends with the four of them visible.
                Box::new(|p: &mut Probe, n| {
                    p.s.life.fx.list[0].visible = n % 2 == 1;
                    sync_touched_effects(p);
                }),
            ),
            (
                "particle-id-indexed",
                2_000_000,
                Box::new(|_: &mut Probe, _| {
                    black_box(swirl_id(true));
                }),
            ),
            (
                "particle-id-walked",
                2_000_000,
                Box::new(|_: &mut Probe, _| {
                    black_box(swirl_id(false));
                }),
            ),
        ],
    );
    p.s.set_game_mode(3);
    report_cases(
        &mut p,
        vec![
            ("spectator-tick", 200_000, Box::new(|p: &mut Probe, _| p.s.vitals_tick())),
            // The part of that tick the spectator's flags add: the check alone, nothing to send.
            ("spectator-shared-flags", 2_000_000, Box::new(|p: &mut Probe, _| p.s.send_shared_flags(false))),
        ],
    );
    finish(p, dir);
}
