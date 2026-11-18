//! Spawn obstruction micro-bench (`docs/perf.md`, L14-PLACE: the default test sees the entities),
//! on the flat world of `tests_g3f13`, a zombie standing on the grass at sixteen spots in turn:
//! - "liquids-only": the default test before, the liquids in the box;
//! - "liquids-and-entities": the game's default test, the liquids then every entity that blocks
//!   building (a walk of the whole mob list);
//! - "mob-walk-id-first" / "mob-walk-bound-first": that walk alone, each mob's id and removal flag
//!   read first (the walk the fluid dwellers made before), or its species bound first.
//!
//! - "stone-placement": a stone placed in the zombie's cell, the same walk held against the
//!   block's shape (L17-POSE: a placement sees every entity that blocks building).
//!
//! Four populations, in turn: no mob, 200 mobs, 1,000 mobs away from the spots, then 200 item
//! frames more (decorations the walk passes over). Then a
//! deterministic count: the entity obstruction tests per tick of a night of natural spawning with
//! the 1,000 mobs, and that tick's time. Nanoseconds per test, median of the rounds; the cases take
//! turns within each round. Run alone:
//! `cargo test --release -p rc-sim bench_spawn_obstruction -- --ignored --nocapture --test-threads=1`.

use std::time::Instant;

use crate::bench_kit::{median, report, run_ticks};
use crate::blocks::probe::Probe;
use crate::mobs::mob::Mob;
use crate::mobs::spawn::{SpawnRegion, UNOBSTRUCTED_TESTS};
use crate::mobs::species::Kind;

const BENCH: &str = "spawn-obstruction";
const ROUNDS: usize = 9;
const CALLS: u32 = 20_000;
const GROUND: f64 = -60.0;

fn add_cows(p: &mut Probe, n: i32) {
    for i in 0..n {
        let mut m = p.s.new_mob(Kind::Cow, [f64::from(20 + (i % 40) * 2) + 0.5, GROUND, f64::from((i / 40) * 2 - 24) + 0.5], 0.0);
        m.no_ai = true;
        p.s.add_mob(m);
    }
}

/// One case: its name and the test it makes for a zombie (true: no obstruction).
type Case<'a> = (&'static str, &'a dyn Fn(&Mob) -> bool);

/// Item frames hung in a row away from the spots: decorations the obstruction walk passes over
/// (only armor stands and mannequins block building).
fn add_frames(p: &mut Probe, n: i32) {
    for i in 0..n {
        let c = format!("summon minecraft:item_frame {} {} {} {{Facing:1b}}", -20 - (i % 40), GROUND, 20 + i / 40);
        assert!(p.s.perform(p.s.server_source(), &c).0, "command refused: {c}");
    }
}

fn time_cases(p: &Probe, zombies: &[Mob], population: &str) {
    let stone = p.s.default_state("minecraft:stone");
    let cases: [Case<'_>; 5] = [
        ("liquids-only", &|m| !p.s.box_has_liquid(&m.bbox())),
        ("liquids-and-entities", &|m| p.s.spawn_obstruction(m, SpawnRegion::LiveWorld)),
        ("mob-walk-id-first", &|m| !p.s.mob_in_the_way(&m.bbox(), m.id, false, |_| true)),
        ("mob-walk-bound-first", &|m| !p.s.mob_in_the_way(&m.bbox(), m.id, true, |_| true)),
        // A stone placed in the zombie's cell (L17: every entity that blocks building is seen).
        ("stone-placement", &|m| !p.s.placement_obstructed(stone, m.block_pos())),
    ];
    let mut ns = vec![Vec::new(); cases.len()];
    for _ in 0..ROUNDS {
        for (k, (_, case)) in cases.iter().enumerate() {
            let start = Instant::now();
            let mut free = 0u32;
            for n in 0..CALLS {
                free += u32::from(case(&zombies[n as usize % zombies.len()]));
            }
            ns[k].push(start.elapsed().as_nanos() as f64 / f64::from(CALLS));
            assert_eq!(free, CALLS, "every spot is free");
        }
    }
    for (k, (case, _)) in cases.iter().enumerate() {
        report(BENCH, &format!("{population}/{case}"), "ns_per_test", median(&ns[k]));
    }
}

#[test]
#[ignore]
fn bench_spawn_obstruction() {
    let (mut p, _dir) = crate::tests_g3f13::open("bench-spawn-obstruction");
    // The tests check every species bound against the mob's box: not measured here.
    crate::mobs::index::set_verify(false);
    let zombies: Vec<Mob> =
        (0..16).map(|i| p.s.new_mob(Kind::Zombie, [3.5 + f64::from(i % 4) * 3.0, GROUND, 3.5 + f64::from(i / 4) * 3.0], 0.0)).collect();
    time_cases(&p, &zombies, "no-mob");
    add_cows(&mut p, 200);
    time_cases(&p, &zombies, "200-mobs");
    add_cows(&mut p, 800);
    time_cases(&p, &zombies, "1000-mobs");
    add_frames(&mut p, 200);
    time_cases(&p, &zombies, "1000-mobs-200-frames");

    // Natural spawning at night around the player, the 1,000 mobs standing still: the spawn phase
    // of the tick alone, run in turn.
    for c in ["time set midnight", "gamerule minecraft:spawn_mobs true"] {
        assert!(p.s.perform(p.s.server_source(), c).0, "command refused: {c}");
    }
    run_ticks(&mut p, 2);
    let before = UNOBSTRUCTED_TESTS.with(std::cell::Cell::get);
    let mut us = Vec::new();
    for _ in 0..ROUNDS {
        let start = Instant::now();
        for _ in 0..10 {
            p.s.natural_spawn_tick();
        }
        us.push(start.elapsed().as_secs_f64() * 1e6 / 10.0);
    }
    let calls = (ROUNDS * 10) as f64;
    let per_call = (UNOBSTRUCTED_TESTS.with(std::cell::Cell::get) - before) as f64 / calls;
    report(BENCH, "night-1000-mobs", "entity_obstruction_tests_per_spawn_phase", per_call);
    report(BENCH, "night-1000-mobs", "us_per_spawn_phase", median(&us));
    report(BENCH, "night-1000-mobs", "mobs_after", p.s.mobs.list.iter().count() as f64);
}
