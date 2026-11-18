//! Mob index in a real world (26.3 version data mandatory, fresh flat world): a `/kill` of the
//! last mobs of the list goes through the tail re-index (counted), TNT in the crowd goes through
//! whichever path its draw gives, and zombie alerts made after each removal query the per-species
//! index; every indexed read of the tick is checked against the scan (`index::verify`).

use crate::blocks::probe::Probe;
use crate::mobs::index::stats::{kind_queries, refreshes};
use crate::mobs::species::Kind;
use crate::tests_g3f13::{finish, open};

fn cmd(p: &mut Probe, c: &str) {
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, c);
    assert!(ok, "command refused: {c}");
}

fn game_ticks(p: &mut Probe, n: i32) {
    for _ in 0..n {
        p.s.game_tick();
        p.s.tick += 1;
    }
}

fn count(p: &Probe, k: Kind) -> usize {
    p.s.mobs.list.iter().filter(|m| m.kind == k && m.alive()).count()
}

/// A zombie hurt by a pig calls its class for help: two per-species queries in the next ticks.
fn zombie_alert(p: &mut Probe) {
    let by = "@e[type=minecraft:pig,limit=1,sort=nearest]";
    cmd(p, &format!("damage @e[type=minecraft:zombie,limit=1,sort=nearest] 1 minecraft:mob_attack by {by}"));
    game_ticks(p, 3);
}

/// Per-species queries since `from`: (served by the index, sent to the scan).
fn queries_since(from: (u64, u64)) -> (u64, u64) {
    let now = kind_queries();
    (now.0 - from.0, now.1 - from.1)
}

#[test]
fn mass_removals_keep_the_index_serving() {
    let (mut p, dir) = open("index-tail");
    // 60 pigs and 60 zombies, species interleaved in the list (pig i at slot 2i); the last 15
    // pigs are tagged so that their removal leaves the first 90 slots in place.
    cmd(&mut p, "time set midnight");
    for i in 0..60 {
        let (x, z) = (f64::from(i % 10) * 1.5 - 7.0, f64::from(i / 10) * 1.5 + 3.0);
        let tags = if i >= 45 { ",Tags:[\"late\"]" } else { "" };
        cmd(&mut p, &format!("summon minecraft:pig {x} -60 {z} {{PersistenceRequired:1b{tags}}}"));
        cmd(&mut p, &format!("summon minecraft:zombie {x} -60 {} {{PersistenceRequired:1b}}", z + 0.7));
    }
    game_ticks(&mut p, 5);
    assert_eq!((count(&p, Kind::Pig), count(&p, Kind::Zombie)), (60, 60));
    let kind_on = crate::mobs::index::kind_on();
    let tail_on = crate::mobs::index::tail_on();
    let start = kind_queries();

    // `/kill` of the last pigs: first removed slot 90 of 120, a tail the re-index takes.
    let before_kill = refreshes();
    cmd(&mut p, "kill @e[type=minecraft:pig,tag=late]");
    assert_eq!(count(&p, Kind::Pig), 45);
    // Bodies leave the list once their death animation is over.
    game_ticks(&mut p, 25);
    let pigs_listed = p.s.mobs.list.iter().filter(|m| m.kind == Kind::Pig).count();
    assert_eq!(pigs_listed, 45, "dead pigs still in the list");
    let tails = refreshes().0 - before_kill.0;
    eprintln!("refreshes during the kill: {tails} by tail re-index");
    if tail_on {
        assert!(tails > 0, "the removal of the last pigs did not go through the tail re-index");
    }
    let after_kill = kind_queries();
    zombie_alert(&mut p);
    let (served_kill, fallback_kill) = queries_since(after_kill);

    // TNT in the crowd: a burst of deaths in one tick, at slots its draw decides.
    let before_tnt = refreshes();
    cmd(&mut p, "summon minecraft:tnt -0.5 -60 6.5 {fuse:1s}");
    game_ticks(&mut p, 40);
    assert!(count(&p, Kind::Pig) + count(&p, Kind::Zombie) < 105, "the TNT killed nobody: scene to fix");
    let (tail_tnt, whole_tnt) = (refreshes().0 - before_tnt.0, refreshes().1 - before_tnt.1);
    let after_tnt = kind_queries();
    zombie_alert(&mut p);
    let (served_tnt, fallback_tnt) = queries_since(after_tnt);

    // Read before the test's own query below, which the counter would include.
    let (served, fallback) = queries_since(start);
    eprintln!(
        "TNT refreshes: {tail_tnt} tail, {whole_tnt} whole; per-species queries: after the kill \
         {served_kill} served / {fallback_kill} scan, after the TNT {served_tnt} / {fallback_tnt}, \
         whole test {served} / {fallback}"
    );
    if kind_on {
        assert!(served_kill > 0 && served_tnt > 0, "an alert made no per-species query: scene to fix");
        assert_eq!(fallback, 0, "per-species index out of step during the tick");
    }
    // The index is serving at the end of the tick.
    assert!(
        p.s.mobs.index.kind_slots(p.s.mobs.list.slots(), Kind::Pig).is_some() == kind_on,
        "per-species index out of step after the removals"
    );
    finish(p, dir);
}
