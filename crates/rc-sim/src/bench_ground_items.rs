//! Ground items micro-bench (`docs/perf.md`, ground items): server ticks of a flat world with
//! - "pile2000": 2,000 mergeable items of four kinds dropped in layers on a grid 0.4 apart (the
//!   bench scene `items-merge-1`), dropped again every 100 ticks; "pile64": 64 of them;
//! - "kept": 500 items that never merge (pickup blocked, eternal: the scene `dropped-items`);
//! - "farm": 1,000 items born and removed every tick (they expire on their first tick).
//!
//! Each with the switches off (the former code), each lead alone, then all on. Run alone: `cargo test --release -p rc-sim [--features memstats] bench_ground_items -- --ignored
//! --nocapture --test-threads=1`.

use crate::bench_kit::{cmd, report, report_timing, run_ticks, time_ticks};
use crate::blocks::probe::Probe;
use crate::drops::DropSwitches;
use crate::tests_g3f13::{finish, open};

const KINDS: [&str; 4] = ["cobblestone", "dirt", "oak_log", "wheat_seeds"];

/// The `n` items of a pile, 50 a row, 0.4 apart, at five heights.
fn drop_pile(p: &mut Probe, n: usize) {
    // Refused when no item is left (nothing to kill): the result does not matter.
    let src = p.s.server_source();
    let _ = p.s.perform(src, "kill @e[type=minecraft:item]");
    for k in 0..n {
        let (x, z) = (2.0 + (k % 50) as f64 * 0.4, -8.0 + (k / 50) as f64 * 0.4);
        let y = -54 + (k % 5) as i32;
        let item = format!("{{id:\"minecraft:{}\",count:1}}", KINDS[k % 4]);
        cmd(p, &format!("summon minecraft:item {x:.2} {y} {z:.2} {{Item:{item},Motion:[0.0d,0.0d,0.0d]}}"));
    }
}

/// µs per tick of a pile of `n` items over 4 × 100 ticks (a fresh drop at the start of each 100):
/// median and best, and the items read by the merges per tick (a count the load does not move).
fn pile(case: &str, n: usize) {
    let (mut p, dir) = open(&format!("bench-pile-{n}-{case}"));
    let mut us = Vec::new();
    let pairs_before = crate::drops::MERGE_PAIRS.with(std::cell::Cell::get);
    for _ in 0..4 {
        drop_pile(&mut p, n);
        let start = std::time::Instant::now();
        run_ticks(&mut p, 100);
        us.push(start.elapsed().as_secs_f64() * 1e6 / 100.0);
    }
    let pairs = crate::drops::MERGE_PAIRS.with(std::cell::Cell::get) - pairs_before;
    let tag = format!("pile{n}-{case}");
    report("ground_items", &tag, "us_per_tick", crate::bench_kit::median(&us));
    report("ground_items", &tag, "us_per_tick_best", us.iter().copied().fold(f64::INFINITY, f64::min));
    report("ground_items", &tag, "items_read_per_tick", pairs as f64 / 400.0);
    report("ground_items", &tag, "items_left", p.s.life.drops.len() as f64);
    finish(p, dir);
}

/// 500 items that never merge, spread 1.5 apart: what their due merges cost.
fn kept(case: &str) {
    let (mut p, dir) = open(&format!("bench-kept-{case}"));
    for k in 0..500usize {
        let (x, z) = (2.0 + (k % 25) as f64 * 1.5, -20.0 + (k / 25) as f64 * 1.5);
        let item = format!("{{id:\"minecraft:{}\",count:1}}", KINDS[k % 4]);
        cmd(&mut p, &format!("summon minecraft:item {x} -60 {z} {{Item:{item},PickupDelay:32767s,Age:-32768s}}"));
    }
    report_timing("ground_items", &format!("kept-{case}"), &time_ticks(&mut p, 60, 400, 5));
    finish(p, dir);
}

/// 1,000 items born each tick that expire on their first tick: a farm's births and removals.
fn farm(case: &str) {
    let (mut p, dir) = open(&format!("bench-farm-{case}"));
    let tick = |p: &mut Probe| {
        for k in 0..1000usize {
            let pos = [2.5 + (k % 40) as f64, -59.0, -10.5 + (k / 40) as f64];
            let s = crate::player::Stack { id: format!("minecraft:{}", KINDS[k % 4]), count: 1, components: None };
            p.s.spawn_item(pos, [0.0; 3], s, 0);
            if let Some(d) = p.s.life.drops.last_mut() {
                d.age = 5999;
            }
        }
        run_ticks(p, 1);
    };
    for _ in 0..20 {
        tick(&mut p);
    }
    let mut t = crate::bench_kit::Timing { us_per_tick: Vec::new(), allocs_per_tick: Vec::new() };
    for _ in 0..5 {
        let (start, allocs) = (std::time::Instant::now(), crate::bench_kit::allocations());
        for _ in 0..50 {
            tick(&mut p);
        }
        t.us_per_tick.push(start.elapsed().as_secs_f64() * 1e6 / 50.0);
        t.allocs_per_tick.push((crate::bench_kit::allocations() - allocs) as f64 / 50.0);
    }
    report_timing("ground_items", &format!("farm-{case}"), &t);
    finish(p, dir);
}

#[test]
#[ignore = "measurement bench"]
fn bench_ground_items() {
    eprintln!(
        "bench_ground_items sizes: item {} orb {} tracker {} body {}",
        std::mem::size_of::<crate::drops::ItemEnt>(),
        std::mem::size_of::<crate::drops::Orb>(),
        std::mem::size_of::<crate::mobs::track::Tracker>(),
        std::mem::size_of::<rc_physics::body::Body>()
    );
    let early = DropSwitches { merge_early_exit: true, ..DropSwitches::ALL_OFF };
    let cases = [
        ("off", DropSwitches::ALL_OFF),
        ("early-exit", early),
        ("live-prefilter", DropSwitches { merge_live_prefilter: true, ..early }),
        ("grid", DropSwitches { merge_grid: true, ..early }),
        ("body-pool", DropSwitches { body_pool: true, ..DropSwitches::ALL_OFF }),
        ("on", DropSwitches::ALL_ON),
    ];
    // Two alternating passes over the cases (each case in a fresh thread: same seeds).
    for _ in 0..2 {
        for (case, switches) in cases {
            let run = move || {
                crate::drops::FORCED_SWITCHES.with(|c| c.set(Some(switches)));
                pile(case, 2000);
                // Around the length from which a run's merges use the grid.
                pile(case, 64);
                kept(case);
                farm(case);
            };
            std::thread::spawn(run).join().expect("bench");
        }
    }
}
