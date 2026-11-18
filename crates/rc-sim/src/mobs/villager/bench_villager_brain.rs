//! Micro-benches of the villager code on the scene of the equivalence proof (`brain_tests`: the
//! village of the bench model world, twenty villagers): per game tick, the whole tick and the
//! villager brains alone (time and allocations), for the old path, each switch alone and all of
//! them; then the pieces alone: the gaze search among the visible entities, the farmland sensing of
//! ten farmers, the breeding test of twenty villagers, the offer lookup among 100 offers.
//!
//! `cargo test --release -p rc-sim bench_villager -- --ignored --nocapture --test-threads=1` with
//! `RUSTCRAFT_OPT_WORLD=<model world>`. On a loaded machine the times are hints; the allocation
//! counts are exact (this thread's, from the test allocator).

use std::time::Instant;

use super::behave::{LookKind, Trig};
use super::brain::FarmlandSearch;
use super::brain_tests::{scene, set_day_time};
use super::switches::{FORCED, VillagerSwitches};
use crate::bench_kit::{median, report};
use crate::mobs::species::Kind;
use crate::mobs::trades::{Cost, Offer, recipe_for};
use crate::player::Stack;
use crate::test_alloc::allocations;

/// One measured run of the scene: µs per game tick, µs and allocations of the brains per tick.
struct SceneRun {
    tick_us: f64,
    brain_us: f64,
    brain_allocs: f64,
}

/// The scene with `switches` on its own thread: 50 unmeasured ticks then 150 measured ones in each
/// of the three stretches.
fn scene_run(tag: String, switches: VillagerSwitches) -> Option<SceneRun> {
    let run = move || {
        FORCED.set(Some(switches));
        crate::mobs::index::set_verify(false);
        let mut s = scene(&tag)?;
        let (mut ticks, mut tick_ns) = (0u32, 0u128);
        let mut brain_measured = (0u64, 0u64);
        for &day in &super::brain_tests::STRETCHES {
            set_day_time(&mut s, day);
            for t in 0..200 {
                let start = Instant::now();
                let before = super::brain_cost::read();
                s.game_tick();
                s.tick += 1;
                if t >= 50 {
                    tick_ns += start.elapsed().as_nanos();
                    ticks += 1;
                    let after = super::brain_cost::read();
                    brain_measured.0 += after.0 - before.0;
                    brain_measured.1 += after.1 - before.1;
                }
            }
        }
        let n = f64::from(ticks);
        Some(SceneRun {
            tick_us: tick_ns as f64 / 1e3 / n,
            brain_us: brain_measured.0 as f64 / 1e3 / n,
            brain_allocs: brain_measured.1 as f64 / n,
        })
    };
    std::thread::Builder::new().stack_size(32 << 20).spawn(run).expect("bench thread").join().expect("bench run")
}

fn bench_cases() -> Vec<(&'static str, VillagerSwitches)> {
    let old = VillagerSwitches::OLD;
    vec![
        ("old", old),
        ("visible_by_index", VillagerSwitches { visible_by_index: true, ..old }),
        ("sensed_kind", VillagerSwitches { sensed_kind: true, ..old }),
        ("no_tick_allocations", VillagerSwitches { no_tick_allocations: true, ..old }),
        ("move_paths", VillagerSwitches { move_paths: true, ..old }),
        ("food_cache", VillagerSwitches { food_cache: true, ..old }),
        ("trade_borrow", VillagerSwitches { trade_borrow: true, ..old }),
        ("block_ids", VillagerSwitches { block_ids: true, ..old }),
        ("farmland_box_scan", VillagerSwitches { farmland_box_scan: true, ..old }),
        ("all_new", VillagerSwitches::ALL_NEW),
    ]
}

#[test]
#[ignore = "micro-bench"]
fn bench_villager_scene_ticks() {
    let cases = bench_cases();
    let mut runs: Vec<Vec<SceneRun>> = cases.iter().map(|_| Vec::new()).collect();
    // Three rounds, every case once per round: a drift of the machine spreads over all of them.
    for round in 0..3 {
        for (i, (name, switches)) in cases.iter().enumerate() {
            let Some(r) = scene_run(format!("villager-bench-{name}-{round}"), *switches) else { return };
            runs[i].push(r);
        }
    }
    for ((name, _), r) in cases.iter().zip(&runs) {
        let pick = |f: fn(&SceneRun) -> f64| median(&r.iter().map(f).collect::<Vec<_>>());
        report("villager_scene", name, "tick_us", pick(|x| x.tick_us));
        report("villager_scene", name, "brain_us_per_tick", pick(|x| x.brain_us));
        report("villager_scene", name, "brain_allocs_per_tick", pick(|x| x.brain_allocs));
    }
}

/// Nanoseconds per call of `f` (repeated `n` times) and allocations per call.
fn per_call(n: u32, mut f: impl FnMut()) -> (f64, f64) {
    let (start, allocs) = (Instant::now(), allocations());
    for _ in 0..n {
        f();
    }
    let calls = f64::from(n);
    (start.elapsed().as_nanos() as f64 / calls, (allocations() - allocs) as f64 / calls)
}

fn report_call(bench: &str, case: &str, (ns, allocs): (f64, f64)) {
    report(bench, case, "ns_per_call", ns);
    report(bench, case, "allocs_per_call", allocs);
}

const GAZES: [LookKind; 9] = [
    LookKind::Cat,
    LookKind::Villager,
    LookKind::Player,
    LookKind::Category(0),
    LookKind::Category(1),
    LookKind::Category(2),
    LookKind::Category(3),
    LookKind::Category(4),
    LookKind::Category(5),
];

/// The nine gaze searches of a villager brain (look memory cleared before each), and the pieces
/// that read blocks, food and offers.
#[test]
#[ignore = "micro-bench"]
fn bench_villager_pieces() {
    let run = || {
        crate::mobs::index::set_verify(false);
        let mut s = scene("villager-bench-pieces")?;
        set_day_time(&mut s, super::brain_tests::STRETCHES[1]);
        for _ in 0..40 {
            s.game_tick();
            s.tick += 1;
        }
        gaze_searches(&mut s);
        farmland_sensing(&s);
        breeding_tests(&s);
        offer_lookup(&s);
        Some(())
    };
    let _ = std::thread::Builder::new().stack_size(32 << 20).spawn(run).expect("bench thread").join().expect("bench");
}

fn gaze_searches(s: &mut crate::server::Server) {
    // The villager that perceives the most entities.
    let slot = (0..s.mobs.list.len())
        .filter(|&i| s.mobs.list.get(i).is_some_and(|m| m.kind == Kind::Villager))
        .max_by_key(|&i| {
            let m = s.mobs.list.get(i);
            m.and_then(|m| m.npc.as_ref()?.brain.as_ref()?.mem.visible.as_ref().map(Vec::len)).unwrap_or(0)
        })
        .expect("a villager");
    let mut m = s.mobs.list.take(slot).expect("villager");
    let mut b = m.npc.as_mut().and_then(|n| n.brain.take()).expect("brain");
    let seen = b.mem.visible.as_ref().map_or(0, Vec::len);
    let t = s.time;
    for (name, switches) in [
        ("old", VillagerSwitches::OLD),
        ("visible_by_index", VillagerSwitches { visible_by_index: true, ..VillagerSwitches::OLD }),
        ("sensed_kind", VillagerSwitches { sensed_kind: true, ..VillagerSwitches::OLD }),
        ("both", VillagerSwitches { visible_by_index: true, sensed_kind: true, ..VillagerSwitches::OLD }),
    ] {
        FORCED.set(Some(switches));
        let r = per_call(20_000, || {
            for k in GAZES {
                b.mem.look = None;
                let _ = s.trig(&mut m, &mut b, &mut Trig::Look(k), t);
            }
        });
        report_call("villager_gaze_nine", name, r);
    }
    report("villager_gaze_nine", "visible_entities", "count", seen as f64);
    if let Some(n) = m.npc.as_mut() {
        n.brain = Some(b);
    }
    s.mobs.list.put(slot, m);
}

fn farmland_sensing(s: &crate::server::Server) {
    let farmers: Vec<[i32; 3]> =
        s.mobs.of_kind(Kind::Villager).filter(|m| m.villager[1] == super::PROF_FARMER).map(|m| m.block_pos()).collect();
    let mut out = Vec::new();
    for (name, how) in
        [("by_name", FarmlandSearch::ByName), ("by_id", FarmlandSearch::ById), ("box_scan", FarmlandSearch::BoxScan)]
    {
        let r = per_call(2_000, || {
            for &c in farmers.iter().take(10) {
                out.clear();
                s.farmland_around(c, how, &mut out);
            }
        });
        report_call("villager_farmland_ten_farmers", name, r);
    }
}

fn breeding_tests(s: &crate::server::Server) {
    let villagers: Vec<&crate::mobs::mob::Mob> = s.mobs.of_kind(Kind::Villager).collect();
    for (name, food_cache) in [("old", false), ("food_cache", true)] {
        FORCED.set(Some(VillagerSwitches { food_cache, ..VillagerSwitches::OLD }));
        let mut yes = 0;
        let r = per_call(2_000, || {
            for m in &villagers {
                yes += usize::from(s.can_breed(m));
            }
        });
        report_call("villager_can_breed_twenty", name, r);
        assert!(yes > 0, "no villager can breed");
    }
}

fn offer_lookup(s: &crate::server::Server) {
    // Every offer asks for carrots, too many for the payment but the last one: each test reads the
    // stack limit of its cost.
    let offer = |n: i32| Offer {
        cost_a: Cost { id: "minecraft:carrot".into(), count: 11 + n % 50, components: None },
        cost_b: None,
        result: Stack { id: "minecraft:emerald".into(), count: 1, components: None },
        uses: 0,
        max_uses: 16,
        reward_exp: true,
        special: 0,
        demand: n % 7,
        mult: 0.05,
        xp: 2,
    };
    let mut offers: Vec<Offer> = (0..100).map(offer).collect();
    offers[99].cost_a.count = 5;
    offers[99].demand = 0;
    let pay = Stack { id: "minecraft:carrot".into(), count: 10, components: None };
    let items = &s.life.items;
    for (name, trade_borrow) in [("old", false), ("trade_borrow", true)] {
        FORCED.set(Some(VillagerSwitches { trade_borrow, ..VillagerSwitches::OLD }));
        let r = per_call(20_000, || {
            assert_eq!(recipe_for(items, &offers, Some(&pay), None, 0), Some(99));
        });
        report_call("villager_recipe_for_100_offers", name, r);
    }
}
