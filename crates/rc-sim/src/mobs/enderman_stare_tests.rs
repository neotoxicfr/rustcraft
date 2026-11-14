//! Staring at an enderman makes it hostile, on a copy of the test world (`RUSTCRAFT_SOLO_WORLD`,
//! zone 200..231) driven by the probe.

use super::mob::{Ent, Mob};
use super::species::Kind;
use crate::blocks::probe::{Probe, state_string};

/// Stone floor at −60, air above, survival player at night (normal difficulty).
fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world(), rc_testdata::version_data());
    let (Some(world), Some(data)) = (world.clone(), data) else {
        eprintln!("test world {world:?} or data missing: test skipped");
        return None;
    };
    let dir = super::tests::prep(&world, tag, 0, "normal", &[]);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run(
        "fill 200 -60 200 231 -60 231 minecraft:stone\nfill 200 -59 200 231 -45 231 minecraft:air\nplayer 216.5 -59 202.5 0 0\n",
    );
    if state_string(&p.s, p.s.get([216, -60, 216])) != "minecraft:stone" {
        eprintln!("zone 200..231 missing from the copy");
        return None;
    }
    p.s.game_mode = 0;
    p.s.loaded = true;
    p.s.on_ground = true;
    // Night: the enderman stays out of the daylight teleports.
    match p.s.clocks.iter_mut().find(|(n, _)| n == "minecraft:overworld") {
        Some(c) => c.1 = 18000,
        None => p.s.clocks.push(("minecraft:overworld".into(), 18000)),
    }
    p.s.time = 18000;
    Some((p, dir))
}

fn spawn_enderman(p: &mut Probe, pos: [f64; 3]) -> i32 {
    let mut m = p.s.new_mob(Kind::Enderman, pos, 0.0);
    let mut g = super::spawn::Group::default();
    p.s.finalize_spawn(&mut m, super::spawn::Reason::SpawnEgg, &mut g);
    let id = m.id;
    p.s.add_mob(m);
    id
}

fn enderman(p: &Probe, id: i32) -> &Mob {
    p.s.mob_ref(id).expect("enderman")
}

/// The player turns its gaze to the enderman's eyes (the game's yaw and pitch).
fn stare_at(p: &mut Probe, id: i32) {
    let (pos, eye) = (enderman(p, id).pos(), enderman(p, id).eye_y());
    let from = [p.s.pos[0], p.s.player_eye_y(), p.s.pos[2]];
    let (dx, dy, dz) = (pos[0] - from[0], eye - from[1], pos[2] - from[2]);
    p.s.yaw = (dz.atan2(dx).to_degrees() - 90.0) as f32;
    p.s.pitch = (-(dy.atan2((dx * dx + dz * dz).sqrt())).to_degrees()) as f32;
}

/// Ticks while the player keeps staring at the enderman, until it takes the player as its target;
/// returns the ticks it took, `None` after `limit` ticks.
fn ticks_until_hostile(p: &mut Probe, id: i32, limit: u32) -> Option<u32> {
    for tick in 1..=limit {
        stare_at(p, id);
        p.tick();
        p.s.mobs_world_tick();
        if enderman(p, id).target == Some(Ent::Player) {
            return Some(tick);
        }
    }
    None
}

/// The player stares an enderman in the eyes 8 blocks away: after the wait of 5 reduced ticks
/// (three passes of its target objective, every second tick), it takes the player as its target
/// and becomes menacing (open mouth, stared-at flag). Fails on the code before (the objective
/// stopped at its first continuation test, having no target yet during the wait, then started
/// again with the wait reset: never hostile in 60 ticks); sabotage verified (failure): the
/// waiting branch of the continuation test removed.
#[test]
fn staring_at_an_enderman_makes_it_hostile() {
    let Some((mut p, dir)) = probe("enderman-stare") else { return };
    let id = spawn_enderman(&mut p, [216.5, -59.0, 210.5]);
    let ticks = ticks_until_hostile(&mut p, id, 60).expect("the stared-at enderman never became hostile");
    // The wait spans three passes of the objective (two ticks apart, but the first two ticks of a
    // new mob are both full passes).
    assert!((4..=8).contains(&ticks), "hostile after {ticks} ticks of staring");
    let m = enderman(&p, id);
    assert!(m.creepy && m.stared_at, "the hostile enderman is menacing (creepy {}, stared {})", m.creepy, m.stared_at);
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

/// A carved pumpkin on the head hides the stare: the enderman stays calm.
#[test]
fn a_carved_pumpkin_hides_the_stare() {
    let Some((mut p, dir)) = probe("enderman-pumpkin") else { return };
    p.s.slots[5] = Some(crate::player::Stack { id: "minecraft:carved_pumpkin".into(), count: 1, components: None });
    let id = spawn_enderman(&mut p, [216.5, -59.0, 210.5]);
    assert_eq!(ticks_until_hostile(&mut p, id, 60), None, "the enderman saw the stare through the pumpkin");
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}
