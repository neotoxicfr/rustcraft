//! Singleplayer's air supply published to the client (data 1: HUD bubbles): drops by one per
//! tick underwater, rises by four on dry land, new player on respawn. Test world
//! (`RUSTCRAFT_SOLO_WORLD`) absent: test ignored.

use rc_client::{EntityEvent, Event};
use rc_proto_v777::entity::Meta;

use crate::blocks::probe::Probe;
use crate::tests::{fresh_copy, test_world};

/// Air values sent for the player since the last read.
fn airs(p: &Probe) -> Vec<i32> {
    let me = p.s.player_id;
    p.take_events()
        .into_iter()
        .filter_map(|e| match e {
            Event::Entity(EntityEvent::Data { id, values }) if id == me => {
                values.into_iter().find_map(|(i, m)| match (i, m) {
                    (1, Meta::Int(v)) => Some(v),
                    _ => None,
                })
            }
            _ => None,
        })
        .collect()
}

fn ticks(p: &mut Probe, n: i32) {
    for _ in 0..n {
        p.s.vitals_tick();
    }
}

#[test]
fn air_supply_reaches_hud_and_resets_on_respawn() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else {
        eprintln!("test world or data absent: test skipped");
        return;
    };
    let mut p = Probe::open(&fresh_copy(&world, "air"), std::sync::Arc::new(data));
    p.run(
        "fill 200 -60 200 210 -60 210 minecraft:stone\nfill 200 -59 200 210 -55 210 minecraft:water\nplayer 205.5 -59 205.5 0 0\n",
    );
    p.s.game_mode = 0;
    p.s.loaded = true;
    let _ = airs(&p);
    // Underwater: one air tick fewer per tick, each value published.
    ticks(&mut p, 40);
    let under = airs(&p);
    assert_eq!(under, (260..300).rev().collect::<Vec<_>>(), "air underwater");
    // On dry land: +4 per tick up to 300.
    p.run("fill 200 -59 200 210 -55 210 minecraft:air\n");
    let _ = airs(&p);
    ticks(&mut p, 12);
    assert_eq!(airs(&p), vec![264, 268, 272, 276, 280, 284, 288, 292, 296, 300], "air on dry land");
    // Death at half breath then respawn: new player, full air, nothing to republish (the
    // client also restarts from the default value).
    p.s.life.vit.air = 120;
    ticks(&mut p, 1);
    assert_eq!(airs(&p), vec![124]);
    p.s.life.vit.dead = true;
    p.s.life.vit.health = 0.0;
    p.s.perform_respawn();
    // Wait for the announced chunks (otherwise the client's loading screen lasts 30 s).
    let evs = p.take_events();
    assert!(evs.iter().any(|e| matches!(e, Event::GameEvent { kind: 13, .. })), "event 13 absent");
    assert_eq!(p.s.life.vit.air, 300);
    assert_eq!(p.s.life.vit.ticks_frozen, 0);
    p.s.loaded = true;
    ticks(&mut p, 5);
    assert!(airs(&p).is_empty(), "full air republished after respawn");
}

/// Scores published for the player (data 18, death screen) since the last read.
fn scores(p: &Probe) -> Vec<i32> {
    let me = p.s.player_id;
    p.take_events()
        .into_iter()
        .filter_map(|e| match e {
            Event::Entity(EntityEvent::Data { id, values }) if id == me => {
                values.into_iter().find_map(|(i, m)| match (i, m) {
                    (18, Meta::Int(v)) => Some(v),
                    _ => None,
                })
            }
            _ => None,
        })
        .collect()
}

#[test]
fn score_reaches_death_screen_and_resets_on_respawn() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else {
        eprintln!("test world or data absent: test skipped");
        return;
    };
    let mut p = Probe::open(&fresh_copy(&world, "score"), std::sync::Arc::new(data));
    p.s.game_mode = 0;
    p.s.loaded = true;
    let _ = scores(&p);
    ticks(&mut p, 3);
    assert!(scores(&p).is_empty(), "zero score published for no reason");
    p.s.life.vit.give_points(7);
    ticks(&mut p, 2);
    assert_eq!(scores(&p), vec![7]);
    // Death: the value stays the client's until respawn (new player, score 0).
    p.s.life.vit.dead = true;
    p.s.life.vit.health = 0.0;
    p.s.perform_respawn();
    assert_eq!(p.s.life.vit.score, 0);
    p.s.loaded = true;
    ticks(&mut p, 3);
    assert!(scores(&p).is_empty(), "score 0 republished after respawn");
    p.s.life.vit.give_points(3);
    ticks(&mut p, 1);
    assert_eq!(scores(&p), vec![3]);
}
