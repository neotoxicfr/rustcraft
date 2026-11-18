//! Micro-bench of the selectors that walk the players (`@a`, `@p`, a name, player options) and of
//! a 1,000-zombie `@e` search, in singleplayer and in a world open to seven LAN guests (the guests
//! are candidates since L16: their cheap options are read in their kept state, their NBT with each
//! installed in turn). One thread: commands run on the server thread. Run alone:
//! `cargo test --release -p rc-sim [--features memstats] bench_player_selectors -- --ignored --nocapture --test-threads=1`

use crate::bench_kit::{self, cmd};
use crate::blocks::probe::Probe;

const RUNS: usize = 5;
const REPEAT: u32 = 2_000;

/// µs (median of `RUNS`) and allocations per run of `REPEAT` runs of `command`.
fn time_command(p: &mut Probe, command: &str) -> (f64, f64) {
    let src = p.s.server_source();
    let _ = p.s.perform(src.clone(), command);
    let (mut us, mut allocs) = (Vec::new(), Vec::new());
    for _ in 0..RUNS {
        let (start, a) = (std::time::Instant::now(), bench_kit::allocations());
        for _ in 0..REPEAT {
            std::hint::black_box(p.s.perform(src.clone(), command));
        }
        us.push(start.elapsed().as_secs_f64() * 1e6 / f64::from(REPEAT));
        allocs.push((bench_kit::allocations() - a) as f64 / f64::from(REPEAT));
    }
    (bench_kit::median(&us), bench_kit::median(&allocs))
}

#[test]
#[ignore = "measurement bench"]
fn bench_player_selectors() {
    crate::mobs::index::set_verify(false);
    let (mut p, dir) = crate::tests_g3f13::open("bench-player-selectors");
    for i in 0..1_000 {
        let (x, z) = ((i % 128) as f64 / 2.0, (i / 128) as f64 / 2.0 - 16.0);
        cmd(&mut p, &format!("summon minecraft:zombie {x} -60 {z} {{NoAI:1b,Silent:1b,PersistenceRequired:1b}}"));
    }
    let host = p.s.name.clone();
    let cases = [
        ("all", "execute if entity @a".to_owned()),
        ("nearest", "execute positioned 8 -60 8 if entity @p".to_owned()),
        ("options", "execute if entity @a[gamemode=survival,level=0..,tag=!x]".to_owned()),
        ("nbt", "execute if entity @a[nbt={XpLevel:0}]".to_owned()),
        ("name", format!("execute if entity {host}")),
        ("zombies", "execute if entity @e[type=minecraft:zombie]".to_owned()),
        ("everything_near", "execute positioned 16 -60 0 if entity @e[distance=..2]".to_owned()),
    ];
    let mut guests = Vec::new();
    for world in ["solo", "lan7"] {
        if world == "lan7" {
            for k in 0..7 {
                let pos = [f64::from(k) * 2.0 + 0.5, -60.0, 20.5];
                guests.push(crate::mobs::tests_spawn_paths::add_guest(&mut p.s, &format!("Guest{k}"), pos));
            }
        }
        for (case, command) in &cases {
            let (us, allocs) = time_command(&mut p, command);
            bench_kit::report("player_selectors", &format!("{case}_{world}"), "us", us);
            if cfg!(feature = "memstats") {
                bench_kit::report("player_selectors", &format!("{case}_{world}"), "allocs", allocs);
            }
        }
    }
    p.s.lan_close();
    drop(guests);
    crate::tests_g3f13::finish(p, dir);
}
