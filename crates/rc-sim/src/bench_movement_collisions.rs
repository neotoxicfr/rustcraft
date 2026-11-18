//! Movement micro-bench (`docs/perf.md`, L14-PLACE: the movements meet the solid entities), on the
//! flat world of `tests_g3f13`: a zombie-sized box walking over the grass (a step and gravity), from
//! sixteen spots in turn:
//! - "plain-world": the movement on the blocks and the border only, what every server movement but
//!   the dropped items' and the orbs' made before;
//! - "world-for-the-mob": the movement as the game makes it, the solid entities the mob collides
//!   with found through the per-species index of the mob list, the border judged from the mob.
//!
//! Three populations, in turn: no mob, 200 ordinary mobs, 200 ordinary mobs and 20 shulkers away
//! from the boxes. Then a deterministic count, the solid entity searches of a tick of 200 walking
//! cows (one per movement), and that tick's time with the 20 shulkers. Nanoseconds per movement,
//! median of the rounds; the cases take turns within each round. Run alone:
//! `cargo test --release -p rc-sim bench_movement_collisions -- --ignored --nocapture --test-threads=1`.

use std::time::Instant;

use rc_physics::Aabb;

use crate::bench_kit::{median, report, run_ticks};
use crate::blocks::probe::Probe;
use crate::mobs::mob::Mob;
use crate::mobs::species::Kind;
use crate::play::SimWorld;

const BENCH: &str = "movement-collisions";
const ROUNDS: usize = 9;
const CALLS: u32 = 20_000;
const GROUND: f64 = -60.0;
/// A walking step with gravity, as a mob's travel gives it.
const STEP: [f64; 3] = [0.1, -0.0784, 0.05];

/// The start boxes, zombie-sized, standing on the grass near the player.
fn boxes() -> Vec<Aabb> {
    (0..16).map(|i| Mob::bbox_at([0.5 + f64::from(i % 4) * 3.0, GROUND, 0.5 + f64::from(i / 4) * 3.0], 0.6, 1.95)).collect()
}

fn add_mobs(p: &mut Probe, kind: Kind, n: i32, x0: i32, no_ai: bool) {
    for i in 0..n {
        let mut m = p.s.new_mob(kind, [f64::from(x0 + (i % 20) * 3) + 0.5, GROUND, f64::from((i / 20) * 3 - 24) + 0.5], 0.0);
        m.no_ai = no_ai;
        p.s.add_mob(m);
    }
}

/// One case: its name and the world the movement is made in.
type Case<'a> = (&'static str, &'a dyn Fn() -> SimWorld<'a>);

/// The cases in turn; `mover` is the mob that moves (out of the list, as during its tick).
fn time_cases(p: &Probe, mover: &Mob, population: &str) {
    let bs = boxes();
    let cases: [Case<'_>; 2] =
        [("plain-world", &|| p.s.world()), ("world-for-the-mob", &|| p.s.world_for_room(p.s.room_for_mob(mover)))];
    let mut ns = vec![Vec::new(); cases.len()];
    for _ in 0..ROUNDS {
        for (k, (_, world)) in cases.iter().enumerate() {
            let start = Instant::now();
            let mut landed = 0u32;
            for n in 0..CALLS {
                let moved = rc_physics::collide::resolve(&world(), &bs[n as usize % bs.len()], STEP, true, 0.6);
                landed += u32::from(moved[1] == 0.0);
            }
            ns[k].push(start.elapsed().as_nanos() as f64 / f64::from(CALLS));
            assert_eq!(landed, CALLS, "every step stays on the grass");
        }
    }
    for (k, (case, _)) in cases.iter().enumerate() {
        report(BENCH, &format!("{population}/{case}"), "ns_per_movement", median(&ns[k]));
    }
}

#[test]
#[ignore]
fn bench_movement_collisions() {
    let (mut p, _dir) = crate::tests_g3f13::open("bench-movement-collisions");
    // The tests check every per-species answer against a scan of the list: not measured here.
    crate::mobs::index::set_verify(false);
    let mover = p.s.new_mob(Kind::Zombie, [0.5, GROUND, 0.5], 0.0);
    time_cases(&p, &mover, "no-mob");
    add_mobs(&mut p, Kind::Cow, 200, 20, false);
    time_cases(&p, &mover, "200-mobs");
    add_mobs(&mut p, Kind::Shulker, 20, 100, true);
    time_cases(&p, &mover, "200-mobs-20-shulkers");

    // The 200 cows walk about: one solid entity search per movement.
    run_ticks(&mut p, 40);
    let before = crate::solid_entities::searches();
    run_ticks(&mut p, 20);
    report(BENCH, "200-walking-cows", "entity_searches_per_tick", (crate::solid_entities::searches() - before) as f64 / 20.0);
    let t = crate::bench_kit::time_ticks(&mut p, 0, 20, ROUNDS);
    report(BENCH, "200-walking-cows-20-shulkers", "us_per_tick", median(&t.us_per_tick));
}
