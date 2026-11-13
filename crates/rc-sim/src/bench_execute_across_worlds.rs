//! Micro-bench of `execute` across worlds (`commands::entity_worlds`): what a step costs in one
//! world, with another world open but holding nothing that counts, and with another world holding
//! as many entities, on the flat world. Cases, each with 100 overworld pigs:
//! - `as_if_self`: `execute as @e[type=pig] if entity @s` (every-world search, read of each
//!   found source, a plain `@s` per source);
//! - `as_at_run`: `execute as @e[type=pig] at @s run scoreboard players add @s b 1` (the read
//!   of `at` and the final command in each entity's world);
//! - `if_score_self`: `execute as @e[type=pig] if score @s b matches 0..` (`@s` as a score
//!   holder).
//!
//! Worlds: `one` (the Nether never opened), `nether_empty` (open and forced, no entity),
//! `nether_held` (100 pigs in its forced chunk). Figures: µs per command (median and best of
//! `RUNS`), world exchanges per command (a deterministic count), allocations with `memstats`.
//!
//! One thread: commands run on the server thread. Run alone: `cargo test --release -p rc-sim
//! [--features memstats] bench_execute_across_worlds -- --ignored --nocapture --test-threads=1`

use crate::bench_kit::{self, cmd};
use crate::blocks::probe::Probe;
use crate::dims::NETHER;

const RUNS: usize = 5;
const PIGS: usize = 100;
const REPEAT: u32 = 200;

const CASES: [(&str, &str); 3] = [
    ("as_if_self", "execute as @e[type=minecraft:pig] if entity @s"),
    ("as_at_run", "execute as @e[type=minecraft:pig] at @s run scoreboard players add @s b 1"),
    ("if_score_self", "execute as @e[type=minecraft:pig] if score @s b matches 0.."),
];

/// `PIGS` frozen pigs on a 10-wide band from (0, 70, 0), each command prefixed by `prefix`.
fn summon_pigs(p: &mut Probe, prefix: &str) {
    for i in 0..PIGS {
        cmd(p, &format!("{prefix}summon minecraft:pig {} 70 {} {{NoAI:1b,NoGravity:1b,Silent:1b}}", i % 10, i / 10));
    }
}

/// Every case on the world as it stands: µs (median and best), exchanges and allocations per run.
fn run_cases(p: &mut Probe, worlds: &str) {
    let src = p.s.server_source();
    for (name, command) in CASES {
        let _ = p.s.perform(src.clone(), command);
        let (mut us, mut allocs, mut exchanges) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..RUNS {
            let (start, a, x) = (std::time::Instant::now(), bench_kit::allocations(), p.s.chunk_map_swaps);
            for _ in 0..REPEAT {
                std::hint::black_box(p.s.perform(src.clone(), command));
            }
            us.push(start.elapsed().as_secs_f64() * 1e6 / f64::from(REPEAT));
            allocs.push((bench_kit::allocations() - a) as f64 / f64::from(REPEAT));
            exchanges.push((p.s.chunk_map_swaps - x) as f64 / f64::from(REPEAT));
        }
        let case = format!("{name}_{worlds}");
        bench_kit::report("execute_across_worlds", &case, "us", bench_kit::median(&us));
        bench_kit::report("execute_across_worlds", &case, "us_best", us.iter().copied().fold(f64::INFINITY, f64::min));
        bench_kit::report("execute_across_worlds", &case, "world_exchanges", bench_kit::median(&exchanges));
        if cfg!(feature = "memstats") {
            bench_kit::report("execute_across_worlds", &case, "allocs", bench_kit::median(&allocs));
        }
    }
}

#[test]
#[ignore = "measurement bench"]
fn bench_execute_across_worlds() {
    crate::mobs::index::set_verify(false);
    let (mut p, _dir) = crate::other_dimension_send_tests::new_world("bench-across-worlds");
    cmd(&mut p, "scoreboard objectives add b dummy");
    summon_pigs(&mut p, "");
    run_cases(&mut p, "one");
    cmd(&mut p, "execute in minecraft:the_nether run forceload add 0 0");
    p.s.in_world(NETHER, |s| s.load_now(&[(0, 0)]));
    run_cases(&mut p, "nether_empty");
    summon_pigs(&mut p, "execute in minecraft:the_nether run ");
    run_cases(&mut p, "nether_held");
}
