//! Bodies on blocks: objects (ground items, orbs, falling blocks, primed TNT) stepping on a
//! redstone ore or a sculk sensor, and living entities sliding down a honey block's side (slowed,
//! fall forgotten, slide sound and particles). Fresh flat world of G6-F01: a missing 26.3 cache
//! or client jar makes the test FAIL.

use rc_client::{EntityEvent, Event};
use rc_data::PropKey;

use super::g6f01_tests::{W, Y, world};
use super::*;
use crate::inside_blocks::Who;
use crate::mobs::species::Kind as MobKind;

fn game_ticks(w: &mut W, n: i32) -> Vec<Event> {
    let mut out = Vec::new();
    for _ in 0..n {
        w.p.s.game_tick();
        w.p.s.tick += 1;
        w.p.s.flush_block_changes();
        out.extend(w.p.take_events());
    }
    out
}

fn lit(w: &W, at: P) -> bool {
    let s = w.p.s.get(at);
    w.p.s.view().get_bool(s, PropKey::Lit)
}

/// Event 53 (honey slide particles) sent for entity `id`.
fn slide_particles(evs: &[Event], id: i32) -> usize {
    evs.iter().filter(|e| matches!(e, Event::Entity(EntityEvent::Event { id: i, event: 53 }) if *i == id)).count()
}

/// A ground item, an orb, a primed TNT and a falling anvil resting on four redstone ores light
/// them, like any entity on the ground in the game; an item resting on a sculk sensor makes it
/// hear a step. Before, only players and mobs stepped on blocks.
#[test]
fn objects_resting_on_blocks_step_on_them() {
    let mut w = world("objects-step-on");
    let ores = [[2, Y, 2], [5, Y, 2], [8, Y, 2], [11, Y, 2]];
    for p in ores {
        w.set(p, "minecraft:redstone_ore");
    }
    let sensor = [14, Y, 2];
    w.set(sensor, "minecraft:sculk_sensor");
    let _ = w.events();
    let top = f64::from(Y) + 1.0;
    w.cmd(&format!(
        "summon minecraft:item 2.5 {top} 2.5 {{Item:{{id:\"minecraft:stone\",count:1}},OnGround:1b,PickupDelay:32767}}"
    ));
    w.cmd(&format!("summon minecraft:experience_orb 5.5 {top} 2.5 {{Value:1,OnGround:1b}}"));
    w.cmd(&format!("summon minecraft:tnt 8.5 {top} 2.5 {{fuse:80}}"));
    w.cmd(&format!("summon minecraft:falling_block 11.5 {top} 2.5 {{BlockState:{{Name:\"minecraft:anvil\"}},Time:1}}"));
    let half = f64::from(Y) + 0.5;
    w.cmd(&format!("summon minecraft:item 14.5 {half} 2.5 {{Item:{{id:\"minecraft:stone\",count:1}},OnGround:1b,PickupDelay:32767}}"));
    // Away from the bodies: the ores and the sensor react to them only.
    w.p.s.pos = [30.5, f64::from(Y), 30.5];
    let _ = game_ticks(&mut w, 3);
    for (p, who) in ores.iter().zip(["item", "orb", "primed TNT", "falling block"]) {
        assert!(lit(&w, *p), "redstone ore under the {who} not lit");
    }
    let s = w.p.s.get(sensor);
    assert_eq!(w.p.s.view().get(s, PropKey::SculkSensorPhase), Some("active"), "sensor under the item");
}

/// Bench (`cargo test --release -p rc-sim --lib -- --ignored bench_object_step_on --nocapture`):
/// cost per resting object and per tick of its step on an ordinary block, without and with a
/// sculk sensor loaded in the world (the state comes from the body's own read).
#[test]
#[ignore]
fn bench_object_step_on() {
    let mut w = world("objects-step-bench");
    let under = ([10, Y - 1, 10], w.p.s.get([10, Y - 1, 10]));
    let pos = [10.5, f64::from(Y), 10.5];
    let n = 2_000_000u32;
    let run = |w: &mut W, label: &str| {
        let t = std::time::Instant::now();
        for _ in 0..n {
            w.p.s.object_step_on(std::hint::black_box(under), pos, || crate::mobs::recent::vibration::Src::Other);
        }
        eprintln!("object step on stone ({label}): {:.1} ns/object", t.elapsed().as_nanos() as f64 / f64::from(n));
    };
    run(&mut w, "no vibration block");
    w.set([4, Y, 4], "minecraft:sculk_sensor");
    run(&mut w, "a sensor elsewhere");
}

/// Same world draws as the slide effects: sound (one in five), then particles (one in five).
fn expected_slide_draws(rng: &mut crate::mobs::jrand::JRand, n: usize) -> (usize, usize) {
    let (mut sounds, mut particles) = (0, 0);
    for _ in 0..n {
        sounds += usize::from(rng.next_int_bound(5) == 0);
        particles += usize::from(rng.next_int_bound(5) == 0);
    }
    (sounds, particles)
}

/// A zombie falling along the side of a honey column slides: its speed is the honey rule's,
/// its fall forgotten, and the world's random decides the slide sound and particles (event 53),
/// one draw in five each, once per slide (the second honey cell its box spans sees it already
/// slowed). Before, a mob fell past honey as past any block.
#[test]
fn mob_slides_down_honey_with_its_effects() {
    let mut w = world("mob-honey-slide");
    let column = [4, Y + 3, 4];
    for y in Y..=Y + 4 {
        w.set([4, y, 4], "minecraft:honey_block");
    }
    let _ = w.events();
    // Zombie (0.6 wide) touching the column's west side, its centre out of the slide reach.
    let pos = [3.73, f64::from(column[1]), 4.5];
    let mut m = w.p.s.new_mob(MobKind::Zombie, pos, 0.0);
    let _ = w.p.take_events();
    let mut expected = w.p.s.mobs.rng;
    let rounds = 25;
    let mut evs = Vec::new();
    for _ in 0..rounds {
        m.set_pos(pos);
        m.old = [pos[0], pos[1] + 0.4, pos[2]];
        m.vel = [0.02, -0.4, 0.0];
        m.on_ground = false;
        m.fall_distance = 6.0;
        let _ = w.p.s.inside_blocks(&mut Who::Mob(&mut m));
        let want = rc_physics::honey_slide(pos, [0.02, -0.4, 0.0], false, m.width(), column).expect("slides");
        assert_eq!(m.vel, want, "honey speed");
        assert_eq!(m.fall_distance, 0.0, "fall forgotten");
        evs.extend(w.p.take_events());
    }
    let (sounds, particles) = expected_slide_draws(&mut expected, rounds);
    let heard = (w.sounds(&evs, "minecraft:block.honey_block.slide"), slide_particles(&evs, m.id));
    assert_eq!(heard, (sounds, particles), "slide sounds and particles");
    assert!(sounds > 0 && particles > 0, "draws too few to show the effects: {sounds} {particles}");
}

/// The player going down along a honey column: the server forgets their fall at each slide, so
/// landing after a long slide does not hurt (before, the fall built up along the whole column).
#[test]
fn player_slide_down_honey_forgets_the_fall() {
    let mut w = world("player-honey-slide");
    for y in Y..=Y + 6 {
        w.set([4, y, 4], "minecraft:honey_block");
    }
    let _ = w.events();
    let s = &mut w.p.s;
    s.game_mode = 0;
    s.life.vit.health = 20.0;
    s.on_ground = false;
    s.life.vit.fall = 12.0;
    // Box overlapping the column's west side, centre out of the slide reach.
    let pos = [3.73, f64::from(Y + 4), 4.5];
    s.life.inside_old = Some([pos[0], pos[1] + 0.15, pos[2]]);
    s.pos = pos;
    let _ = s.inside_blocks(&mut Who::Player);
    assert_eq!(s.life.vit.fall, 0.0, "fall forgotten along the honey");
}
