//! Mob index in a real world (26.3 version data mandatory, fresh flat world): mass removals
//! (`/kill` of a part of a crowd, TNT in the crowd) go through the tail re-index while every
//! indexed read of the tick is checked against the scan (`index::verify`), and the per-species
//! queries of the tick are counted: served by the index or sent back to the scan.

use crate::blocks::probe::Probe;
use crate::mobs::index::stats::kind_queries;
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
    p.s.mobs.list.iter().flatten().filter(|m| m.kind == k && m.alive()).count()
}

#[test]
fn mass_removals_keep_the_index_serving() {
    let (mut p, dir) = open("index-tail");
    // A crowd of pigs and zombies around the player, species interleaved in the list; with their
    // goals on, so that the tick makes per-species queries (breeding, alerts, spawning).
    cmd(&mut p, "time set midnight");
    for i in 0..60 {
        let (x, z) = (f64::from(i % 10) * 1.5 - 7.0, f64::from(i / 10) * 1.5 + 3.0);
        cmd(&mut p, &format!("summon minecraft:pig {x} -60 {z} {{PersistenceRequired:1b}}"));
        cmd(&mut p, &format!("summon minecraft:zombie {x} -60 {} {{PersistenceRequired:1b}}", z + 0.7));
    }
    game_ticks(&mut p, 5);
    assert_eq!((count(&p, Kind::Pig), count(&p, Kind::Zombie)), (60, 60));
    let (served0, fallback0) = kind_queries();
    // Zombies hurt by a pig call their class for help (a per-species query of the walk).
    for sort in ["nearest", "furthest"] {
        let by = "@e[type=minecraft:pig,limit=1,sort=nearest]";
        cmd(&mut p, &format!("damage @e[type=minecraft:zombie,limit=1,sort={sort}] 1 minecraft:mob_attack by {by}"));
    }
    game_ticks(&mut p, 3);

    // `/kill` of half the pigs: removals spread over the whole list.
    cmd(&mut p, "kill @e[type=minecraft:pig,limit=30,sort=furthest]");
    assert_eq!(count(&p, Kind::Pig), 30);
    // Bodies leave the list once their death animation is over.
    game_ticks(&mut p, 25);
    let pigs_listed = p.s.mobs.list.iter().flatten().filter(|m| m.kind == Kind::Pig).count();
    assert_eq!(pigs_listed, 30, "dead pigs still in the list");

    // TNT in the crowd: a burst of deaths in one tick.
    cmd(&mut p, "summon minecraft:tnt -0.5 -60 6.5 {fuse:1s}");
    game_ticks(&mut p, 40);
    assert!(count(&p, Kind::Pig) + count(&p, Kind::Zombie) < 90, "the TNT killed nobody: scene to fix");

    // The index is serving again at the end of the tick, and the walk's queries used it.
    assert!(
        p.s.mobs.index.kind_slots(&p.s.mobs.list, Kind::Pig).is_some() == crate::mobs::index::kind_on(),
        "per-species index out of step after the removals"
    );
    let (served, fallback) = (kind_queries().0 - served0, kind_queries().1 - fallback0);
    eprintln!("per-species queries during the removals: {served} served, {fallback} sent to the scan");
    if crate::mobs::index::kind_on() {
        assert!(served > 1, "the alerts made no per-species query: scene to fix");
        assert_eq!(fallback, 0, "per-species index out of step during the tick");
    }
    finish(p, dir);
}
