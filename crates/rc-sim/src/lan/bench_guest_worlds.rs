//! Worlds ticked away from the host micro-bench (`docs/perf.md`, LAN guests in their own worlds),
//! on fresh flat worlds, one whole server tick (the host's world and the server-wide parts, then
//! the guests) per call:
//! - "guest-in-host-world": one guest standing by the host (nothing ticked away from the host);
//! - "guest-in-nether": the guest standing in the Nether, which ticks in full with it installed;
//! - "forced-nether-chunk": no guest, one force-loaded Nether chunk ticking with nobody there;
//! - "host-alone-in-nether": no guest, the host standing in the Nether (the cost of that world's
//!   tick by itself, to set against "guest-in-nether");
//! - "world-visit-round-trip": one visit to the Nether and back (two world exchanges), per call.
//!
//! Nanoseconds and allocations per tick (the test allocator's count for this thread), and the
//! world exchanges per tick (deterministic count). The cases take turns within each round, so
//! that a loaded machine disturbs them alike. Run alone:
//! `cargo test --release -p rc-sim bench_guest_worlds -- --ignored --nocapture --test-threads=1`.

use std::time::Instant;

use crate::blocks::probe::Probe;
use crate::bench_kit::{median, report};
use crate::dims::NETHER;
use crate::other_dimension_send_tests::{new_world, run};
use crate::test_alloc::allocations;

const BENCH: &str = "guest-worlds";
const ROUNDS: usize = 9;
const TICKS: u32 = 40;
const VISITS: u32 = 2_000;

fn server_tick(p: &mut Probe) {
    p.s.game_tick();
    p.s.lan_tick();
    p.s.tick += 1;
    let _ = p.take_events();
}

/// A world whose guest stands at the origin of `world`, its chunks around loaded.
fn world_with_guest(tag: &str, world: &str) -> (Probe, std::path::PathBuf, impl Sized) {
    let (mut p, dir) = new_world(tag);
    let keep = crate::mobs::tests_spawn_paths::add_guest_with_packets(&mut p.s, "Guest", [0.5, 100.0, 0.5]);
    let around: Vec<(i32, i32)> = (-2..=2).flat_map(|x| (-2..=2).map(move |z| (x, z))).collect();
    p.s.in_world(world, |s| s.load_now(&around));
    if let Some(c) = p.s.lan.as_mut().and_then(|l| l.guests[0].ctx.as_mut()) {
        c.client_world = world.to_owned();
        c.pos = if world == NETHER { [0.5, 100.0, 0.5] } else { [0.5, -60.0, 0.5] };
    }
    p.s.player_enters_active_world(); // the players' worlds noted apart
    (p, dir, keep)
}

#[test]
#[ignore]
fn bench_guest_worlds() {
    let (mut by_host, _d1, _k1) = world_with_guest("bench-guest-host-world", crate::dims::OVERWORLD);
    let (mut in_nether, _d2, _k2) = world_with_guest("bench-guest-nether", NETHER);
    let (mut forced, _d3) = new_world("bench-forced-nether");
    let around: Vec<(i32, i32)> = (-2..=2).flat_map(|x| (-2..=2).map(move |z| (x, z))).collect();
    forced.s.in_world(NETHER, |s| s.load_now(&around));
    let _ = run(&mut forced, "execute in minecraft:the_nether run forceload add 0 0");
    let (mut host_in_nether, _d4) = new_world("bench-host-nether");
    host_in_nether.s.in_world(NETHER, |s| s.load_now(&around));
    let _ = run(&mut host_in_nether, "execute in minecraft:the_nether run tp @a 0.5 100 0.5");
    host_in_nether.s.loaded = true;
    let mut cases: [(&str, &mut Probe); 4] = [
        ("guest-in-host-world", &mut by_host),
        ("guest-in-nether", &mut in_nether),
        ("forced-nether-chunk", &mut forced),
        ("host-alone-in-nether", &mut host_in_nether),
    ];
    let (mut ns, mut allocs, mut swaps) = (vec![Vec::new(); 4], vec![Vec::new(); 4], vec![0.0; 4]);
    let mut visit_ns = Vec::new();
    for _ in 0..ROUNDS {
        let start = Instant::now();
        for _ in 0..VISITS {
            cases[0].1.s.in_world(NETHER, |_| ());
        }
        visit_ns.push(start.elapsed().as_nanos() as f64 / f64::from(VISITS));
        for (k, (_, p)) in cases.iter_mut().enumerate() {
            let (start, before, swaps_before) = (Instant::now(), allocations(), p.s.chunk_map_swaps);
            for _ in 0..TICKS {
                server_tick(p);
            }
            ns[k].push(start.elapsed().as_nanos() as f64 / f64::from(TICKS));
            allocs[k].push((allocations() - before) as f64 / f64::from(TICKS));
            swaps[k] = (p.s.chunk_map_swaps - swaps_before) as f64 / f64::from(TICKS);
        }
    }
    for (k, (case, p)) in cases.iter_mut().enumerate() {
        report(BENCH, case, "ns_per_tick", median(&ns[k]));
        report(BENCH, case, "ns_per_tick_best", ns[k].iter().copied().fold(f64::INFINITY, f64::min));
        report(BENCH, case, "allocs_per_tick", median(&allocs[k]));
        report(BENCH, case, "world_exchanges_per_tick", swaps[k]);
        p.s.lan_close();
    }
    report(BENCH, "world-visit-round-trip", "ns_per_call", median(&visit_ns));
}
