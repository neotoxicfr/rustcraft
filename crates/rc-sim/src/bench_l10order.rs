//! L10-ORDER bench: server ticks of a flat world holding 1,000 entities of five families
//! (ground items, orbs, mobs without AI, arrows stuck in the ground, minecarts). Summoned in turn
//! ("interleaved": the ranks alternate, worst case of the merged walk, one run per entity) or
//! family after family ("grouped": one run per family, the order of the former walk); then
//! 1,000 primed TNT on the ground with a long fuse ("tnt": the copy ticked each tick). Run
//! alone: `cargo test --release -p rc-sim bench_l10order -- --ignored --nocapture`.

use crate::blocks::probe::Probe;
use crate::tests_g3f13::{finish, open};

fn cmd(p: &mut Probe, c: &str) {
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, c);
    assert!(ok, "command refused: {c}");
}

fn scene(tag: &str, family_of: impl Fn(usize) -> usize) {
    let (mut p, dir) = open(tag);
    let families = if tag == "tnt" {
        ["minecraft:tnt ~ ~ ~ {fuse:30000s}"; 5]
    } else {
        [
            "minecraft:item ~ ~ ~ {Item:{id:\"minecraft:stone\",count:1},PickupDelay:32767s}",
            "minecraft:experience_orb ~ ~ ~ {Value:1}",
            "minecraft:pig ~ ~ ~ {NoAI:1b}",
            "minecraft:arrow ~ ~1 ~",
            "minecraft:minecart ~ ~ ~",
        ]
    };
    let mut slot = [0usize; 5];
    for i in 0..1000usize {
        let f = family_of(i);
        // Same place for the k-th entity of a family in the two mixed scenes.
        let k = if tag.starts_with("only") { i } else { slot[f] * 5 + f };
        slot[f] += 1;
        let (x, z) = (12.5 + (k % 40) as f64 * 2.0, -14.5 + (k / 40) as f64 * 1.8);
        cmd(&mut p, &format!("execute positioned {x} -60 {z} run summon {}", families[f]));
    }
    for _ in 0..60 {
        p.s.game_tick();
        p.s.tick += 1;
    }
    let count = p.s.life.drops.len()
        + p.s.bs.entities.list.len()
        + p.s.life.orbs.len()
        + p.s.mobs.list.iter().count()
        + p.s.proj.list.len()
        + p.s.carts.list.len();
    let ticks = 400;
    let mut all = Vec::new();
    for _ in 0..5 {
        let t = std::time::Instant::now();
        for _ in 0..ticks {
            p.s.game_tick();
            p.s.tick += 1;
            let _ = p.take_events();
        }
        all.push(t.elapsed().as_secs_f64() * 1.0e6 / f64::from(ticks));
    }
    let shown: Vec<String> = all.iter().map(|us| format!("{us:.0}")).collect();
    eprintln!("bench_l10order {tag}: µs per server tick {} ({count} entities)", shown.join(" "));
    finish(p, dir);
}

/// 1,000 pigs (AI on), 25 of them in chunk 3, which leaves the ticking chunks and comes back every
/// other tick (simulation distance 2, then 3): on each return the mob list is put back in rank
/// order before the walk, then the walk looks mobs up.
fn rejoin_scene() {
    let (mut p, dir) = open("rejoin");
    for k in 0..1000usize {
        let (x, z) = if k < 25 {
            (50.5 + (k % 5) as f64 * 2.5, -8.5 + (k / 5) as f64 * 2.5)
        } else {
            (-20.5 + (k % 30) as f64 * 2.0, -24.5 + (k / 30) as f64 * 1.4)
        };
        cmd(&mut p, &format!("summon minecraft:pig {x} -60 {z}"));
    }
    let tick = |p: &mut Probe| {
        p.s.sim_dist = if p.s.tick.is_multiple_of(2) { 2 } else { 3 };
        p.s.game_tick();
        p.s.tick += 1;
        let _ = p.take_events();
    };
    for _ in 0..60 {
        tick(&mut p);
    }
    let ticks = 400;
    let mut all = Vec::new();
    for _ in 0..5 {
        let t = std::time::Instant::now();
        for _ in 0..ticks {
            tick(&mut p);
        }
        all.push(t.elapsed().as_secs_f64() * 1.0e6 / f64::from(ticks));
    }
    let shown: Vec<String> = all.iter().map(|us| format!("{us:.0}")).collect();
    eprintln!("bench_l10order rejoin: µs per server tick {}", shown.join(" "));
    finish(p, dir);
}

#[test]
#[ignore = "measurement bench"]
fn bench_l10order_rejoin() {
    crate::mobs::index::set_verify(false);
    for _ in 0..2 {
        rejoin_scene();
    }
}

#[test]
#[ignore = "measurement bench"]
fn bench_l10order_walk() {
    eprintln!(
        "bench_l10order sizes: item {} orb {} tracker {} stack {}",
        std::mem::size_of::<crate::drops::ItemEnt>(),
        std::mem::size_of::<crate::drops::Orb>(),
        std::mem::size_of::<crate::mobs::track::Tracker>(),
        std::mem::size_of::<crate::player::Stack>()
    );
    for _ in 0..2 {
        scene("interleaved", |i| i % 5);
        scene("grouped", |i| i / 200);
        scene("tnt", |i| i % 5);
    }
    // One family alone (1,000 of it): where the cost per entity goes.
    for (f, tag) in ["only-items", "only-orbs", "only-pigs", "only-arrows", "only-carts"].into_iter().enumerate() {
        scene(tag, |_| f);
    }
}
