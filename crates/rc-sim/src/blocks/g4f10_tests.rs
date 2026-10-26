//! G4-F10: allay dance near a jukebox (own listener, radius 10 at eye level), link with
//! duplication, parrot imitation (silence of the imitator and the imitated, sound
//! category). New flat world of G6-F01: a missing 26.3 cache or client jar makes it FAIL.

use rc_client::Event;

use super::g6f01_tests::{W, Y, stack, world};
use super::*;
use crate::mobs::mob::Mob;
use crate::mobs::species::Kind;

fn allays(w: &W) -> Vec<&Mob> {
    w.p.s.mobs.list.iter().flatten().map(|m| &**m).filter(|m| m.kind == Kind::Allay && !m.removed).collect()
}

fn dancing_at(w: &W, x: f64) -> bool {
    allays(w).iter().find(|m| (m.pos[0] - x).abs() < 1e-9).expect("allay").bm.dancing
}

/// Jukebox at [4, Y, 4]: allays whose eye block is at distance² ≤ 100 dance from the
/// first play event (tick 0), not beyond; another jukebox stopping does not cut
/// the dance; ejection stops it; while dancing, amethyst duplicates.
#[test]
fn allay_dances_to_jukebox_and_duplicates() {
    let mut w = world("g4f10-allay");
    let (j, k) = ([4, Y, 4], [4, Y, 20]);
    w.set(j, "minecraft:jukebox");
    w.set(k, "minecraft:jukebox");
    // distinct x to find them again; eyes at y = Y (0.36 above the feet).
    // a: 8 blocks (64); c: exactly 10 blocks (100); b: 11 blocks (121).
    for (x, z) in [("4.5", "12.5"), ("4.25", "14.5"), ("4.75", "15.5")] {
        w.cmd(&format!("summon minecraft:allay {x} {Y} {z}"));
    }
    assert_eq!(allays(&w).len(), 3, "allays summoned");
    w.hold(stack("minecraft:music_disc_13", 1));
    w.click(j, Dir::Up, [0.5, 1.0, 0.5]);
    let mut evs = Vec::new();
    w.ticks(1, &mut evs);
    assert!(dancing_at(&w, 4.5), "8 blocks: dance");
    assert!(dancing_at(&w, 4.25), "10 blocks: dance (≤ 100)");
    assert!(!dancing_at(&w, 4.75), "11 blocks: out of range");
    let a = allays(&w).iter().find(|m| m.pos[0] == 4.5).map(|m| (m.id, m.bm.jukebox)).unwrap();
    assert_eq!(a.1, Some(j), "jukebox picked up");
    // Second jukebox k, in range of all three: b (which was not dancing) picks it up and dances; a and
    // c already dance for j. Stopping k cuts only b.
    w.hold(stack("minecraft:music_disc_cat", 1));
    w.click(k, Dir::Up, [0.5, 1.0, 0.5]);
    w.ticks(1, &mut evs);
    assert!(dancing_at(&w, 4.75), "b dances for k");
    w.hold(None);
    w.click(k, Dir::Up, [0.5, 1.0, 0.5]);
    w.ticks(1, &mut evs);
    assert!(!dancing_at(&w, 4.75), "b stops with k");
    assert!(dancing_at(&w, 4.5) && dancing_at(&w, 4.25), "stop of a jukebox that was not picked up is ignored");
    // Duplication: amethyst while dancing.
    let slot = 36 + w.p.s.held as usize;
    w.hold(stack("minecraft:amethyst_shard", 3));
    assert!(w.p.s.player_interact(a.0, 0), "amethyst accepted");
    assert_eq!(allays(&w).len(), 4, "allay duplicated");
    assert_eq!(w.p.s.slots[slot].as_ref().map(|s| s.count), Some(2), "one shard taken");
    assert!(allays(&w).iter().filter(|m| m.bm.dup_cooldown == 6000).count() == 2, "cooldowns set");
    // Ejection: end of play, the dance stops for the picked-up jukebox.
    w.hold(None);
    w.click(j, Dir::Up, [0.5, 1.0, 0.5]);
    w.ticks(1, &mut evs);
    assert!(!dancing_at(&w, 4.5) && !dancing_at(&w, 4.25), "stop at ejection");
    assert_eq!(allays(&w).iter().find(|m| m.id == a.0).unwrap().bm.jukebox, None);
}

/// Cost of a jukebox event for the allays' listener (500 zombies, 10 allays).
#[test]
#[ignore = "bench"]
fn bench_allays_hear_jukebox() {
    let mut w = world("g4f10-bench");
    for i in 0..510 {
        let kind = if i < 10 { "allay" } else { "zombie" };
        w.cmd(&format!("summon minecraft:{kind} {}.5 {Y} {}.5 {{NoAI:1b}}", i % 16, (i / 16) % 16));
    }
    let n = 10_000;
    let t = std::time::Instant::now();
    for i in 0..n {
        w.p.s.bz_allays_hear_jukebox([4, Y, 4], i % 2 == 0);
    }
    eprintln!("allays: {:.0} ns/event", t.elapsed().as_secs_f64() / f64::from(n) * 1e9);
}

/// Imitation: silent imitator = nothing, without a random draw; silent drawn mob = false and
/// no sound; otherwise the sound goes out in the imitator's category (carrying player: 7).
#[test]
fn parrot_imitation_respects_silence() {
    use crate::mobs::surface::parrot::{Imitator, imitate_nearby};
    let mut w = world("g4f10-parrot");
    w.cmd(&format!("summon minecraft:zombie 6.5 {Y} 6.5 {{Silent:1b,NoAI:1b}}"));
    let bbox = rc_physics::Aabb { min: [8.2, f64::from(Y), 8.2], max: [8.8, f64::from(Y) + 1.8, 8.8] };
    let mut who = Imitator { center: [8.5, f64::from(Y), 8.5], bbox, alive: true, silent: true, source: 7 };
    let before = w.p.s.mobs.rng;
    assert!(!imitate_nearby(&mut w.p.s, &who));
    let (mut x, mut y) = (before, w.p.s.mobs.rng);
    assert_eq!(x.next_long(), y.next_long(), "no random draw for a silent imitator");
    who.silent = false;
    let _ = w.events();
    for _ in 0..64 {
        assert!(!imitate_nearby(&mut w.p.s, &who), "silent zombie: never imitated");
    }
    assert!(!w.events().iter().any(|e| matches!(e, Event::Sound(_))), "no sound");
    w.cmd(&format!("summon minecraft:zombie 6.5 {Y} 10.5 {{NoAI:1b}}"));
    let mut played = false;
    for _ in 0..64 {
        played |= imitate_nearby(&mut w.p.s, &who);
    }
    assert!(played, "noisy zombie imitated");
    let cats: Vec<i32> =
        w.events().iter().filter_map(|e| if let Event::Sound(s) = e { Some(s.category) } else { None }).collect();
    assert!(!cats.is_empty() && cats.iter().all(|c| *c == 7), "imitator's category: {cats:?}");
}
