//! Free-room test micro-bench (`docs/perf.md`, solid entities in the free-room tests), on the flat
//! world of `tests_g3f13`, one spawn box in the open (free of blocks, so the entity search runs):
//! - "blocks-only": the plain world's test, what those sites made before (blocks and border);
//! - "room-for-no-entity" / "room-for-a-mob": the game's test with its solid entities, found
//!   through the per-species index of the mob list;
//! - "solid-scan-whole-list": the other lead, the solid mobs looked for by walking the whole list
//!   (kind read first, then the same position bound).
//!
//! Three populations, in turn: no mob, 200 ordinary mobs, 200 ordinary mobs and 20 shulkers away
//! from the box. Then a deterministic count: the entity searches of a tick of 2,000 dropped items
//! resting on the ground (the sites that run the most), and that tick's time. The index without the
//! position bound was measured before the bound went in (`docs/perf.md`). Nanoseconds per call,
//! median of the rounds;
//! the cases take turns within each round, so that a loaded machine disturbs them alike. Run alone:
//! `cargo test --release -p rc-sim bench_free_room -- --ignored --nocapture --test-threads=1`.

use std::time::Instant;

use rc_physics::Aabb;

use crate::bench_kit::{median, report, run_ticks};
use crate::blocks::probe::Probe;
use crate::mobs::mob::Mob;
use crate::mobs::species::Kind;
use crate::solid_entities::{RoomFor, box_may_touch};

const BENCH: &str = "free-room";
const ROUNDS: usize = 9;
const CALLS: u32 = 20_000;
const GROUND: f64 = -60.0;

/// The spawn boxes tested in turn: zombie-sized, in the open over the grass near the player.
fn boxes() -> Vec<Aabb> {
    (0..16).map(|i| Mob::bbox_at([0.5 + f64::from(i % 4) * 3.0, GROUND, 0.5 + f64::from(i / 4) * 3.0], 0.6, 1.95)).collect()
}

fn add_mobs(p: &mut Probe, kind: Kind, n: i32, x0: i32) {
    for i in 0..n {
        let mut m = p.s.new_mob(kind, [f64::from(x0 + (i % 20) * 3) + 0.5, GROUND, f64::from((i / 20) * 3 - 24) + 0.5], 0.0);
        m.no_ai = true;
        p.s.add_mob(m);
    }
}

/// The solid mobs touching `b` found by walking the whole list, the kind read first, then the
/// position bound.
fn solid_by_scan(p: &Probe, b: &Aabb) -> bool {
    let zone = b.deflate(-1.0e-7);
    let solid_kind = |m: &Mob| matches!(m.kind, Kind::Shulker | Kind::HappyGhast);
    p.s.mobs.list.iter().any(|m| solid_kind(m) && box_may_touch(m, &zone) && m.bbox().intersects(&zone))
}

/// One case: its name and the test it makes of a box (true: free).
type Case<'a> = (&'static str, &'a dyn Fn(&Aabb) -> bool);

/// The cases in turn; `asker` is a mob out of the list (one being spawned).
fn time_cases(p: &Probe, asker: &Mob, population: &str) {
    let bs = boxes();
    let cases: [Case<'_>; 4] = [
        ("blocks-only", &|b| rc_physics::is_free(&p.s.world(), b)),
        ("room-for-no-entity", &|b| p.s.room_free(RoomFor::NoEntity, b)),
        ("room-for-a-mob", &|b| p.s.room_free(p.s.room_for_mob(asker), b)),
        ("solid-scan-whole-list", &|b| rc_physics::is_free(&p.s.world(), b) && !solid_by_scan(p, b)),
    ];
    let mut ns = vec![Vec::new(); cases.len()];
    for _ in 0..ROUNDS {
        for (k, (_, case)) in cases.iter().enumerate() {
            let start = Instant::now();
            let mut free = 0u32;
            for n in 0..CALLS {
                free += u32::from(case(&bs[n as usize % bs.len()]));
            }
            ns[k].push(start.elapsed().as_nanos() as f64 / f64::from(CALLS));
            assert_eq!(free, CALLS, "every box is free");
        }
    }
    for (k, (case, _)) in cases.iter().enumerate() {
        report(BENCH, &format!("{population}/{case}"), "ns_per_call", median(&ns[k]));
    }
}

#[test]
#[ignore]
fn bench_free_room() {
    let (mut p, _dir) = crate::tests_g3f13::open("bench-free-room");
    // The tests check every per-species answer against a scan of the list: not measured here.
    crate::mobs::index::set_verify(false);
    let asker = p.s.new_mob(Kind::Zombie, [0.5, GROUND, 0.5], 0.0);
    time_cases(&p, &asker, "no-mob");
    add_mobs(&mut p, Kind::Cow, 200, 20);
    time_cases(&p, &asker, "200-mobs");
    add_mobs(&mut p, Kind::Shulker, 20, 100);
    time_cases(&p, &asker, "200-mobs-20-shulkers");

    // 2,000 items resting on the grass, spread over the loaded chunks: entity searches per tick.
    let stone = crate::player::Stack { id: "minecraft:stone".into(), count: 1, components: None };
    for i in 0..2000 {
        let at = [f64::from(i % 50) * 2.0 + 0.5, GROUND, f64::from(i / 50) - 20.0 + 0.5];
        p.s.spawn_item(at, [0.0; 3], stone.clone(), 32767);
    }
    run_ticks(&mut p, 40);
    let before = crate::solid_entities::searches();
    run_ticks(&mut p, 20);
    report(BENCH, "2000-resting-items", "entity_searches_per_tick", (crate::solid_entities::searches() - before) as f64 / 20.0);
    let t = crate::bench_kit::time_ticks(&mut p, 0, 20, ROUNDS);
    report(BENCH, "2000-resting-items-200-mobs-20-shulkers", "us_per_tick", median(&t.us_per_tick));
}
