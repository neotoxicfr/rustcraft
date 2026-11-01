//! Server-side dragon head (mobs' body control after its AI step): caught up to 75° from the
//! body only when it moves horizontally; its yaw orients the dragon's gaze (takeoff, landing,
//! sitting phases, flight) and the strafing fireball.

use std::path::PathBuf;
use std::sync::Arc;

use super::dragon::turn_head;
use super::phases::Phase;
use crate::blocks::probe::Probe;
use crate::mobs::jrand::JRand;
use crate::mobs::mob::{Ent, Mob};
use crate::mobs::species::Kind;
use crate::newworld::{NewWorld, Preset, create_world};
use crate::projectiles::PKind;

/// Dragon with the given body and head yaw, tick-start position = current position.
fn dragon(yaw: f32, head: f32) -> Mob {
    let mut m = Mob::new(Kind::EnderDragon, 1, 1, [0.5, 64.0, 0.5], yaw, JRand::new(1));
    m.head_yaw = head;
    m.old = m.pos;
    m
}

/// Head after a tick where the dragon moved by `d` (body at 120°, head at 0).
fn after_move(d: [f64; 3]) -> f32 {
    let mut m = dragon(120.0, 0.0);
    m.pos = [m.pos[0] + d[0], m.pos[1] + d[1], m.pos[2] + d[2]];
    turn_head(&mut m);
    assert_eq!(m.body_yaw, 120.0, "body = yaw");
    m.head_yaw
}

/// Still (sitting dragon pivoting towards the player) or rising vertically (dead): head unchanged.
/// Moved horizontally beyond the game threshold (2.5000003e-7 squared, widened float): head
/// pulled back to 75° from the body, on either side.
#[test]
fn dragon_head_turns_only_while_moving() {
    assert_eq!(after_move([0.0; 3]), 0.0, "still");
    assert_eq!(after_move([0.0, 0.1, 0.0]), 0.0, "vertical only");
    // 5.0000001e-4 squared exceeds 2.5e-7 but not the game's float threshold.
    assert_eq!(after_move([5.000_000_1e-4, 0.0, 0.0]), 0.0, "below the threshold");
    assert_eq!(after_move([5.000_003e-4, 0.0, 0.0]), 45.0, "above the threshold: 120 − 75");
    assert_eq!(after_move([0.0, 0.0, 0.01]), 45.0);
    let mut m = dragon(-100.0, 0.0);
    m.pos[0] += 0.01;
    turn_head(&mut m);
    assert_eq!(m.head_yaw, -25.0, "−100 + 75");
    // Difference under 75°: head unchanged even when moving.
    let mut m = dragon(50.0, 0.0);
    m.pos[2] += 0.01;
    turn_head(&mut m);
    assert_eq!(m.head_yaw, 0.0);
}

/// New flat world, chunks around the origin generated, player at (0.5, −60, 0.5).
fn open(tag: &str) -> (Probe, PathBuf) {
    let data = Arc::new(rc_testdata::version_data().expect("26.3 version data missing"));
    assert!(crate::registries::client_jar(&data).is_some(), "26.3 client jar missing");
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-dragonhead-{tag}-{}", std::process::id())));
    let mut w = NewWorld::new(tag);
    w.seed = 1;
    w.preset = Preset::Flat;
    create_world(&dir, &data, &w).expect("world creation");
    let mut p = Probe::open(&dir, data);
    p.s.pos = [0.5, -60.0, 0.5];
    let want: Vec<(i32, i32)> = (-2..=2).flat_map(|x| (-2..=2).map(move |z| (x, z))).collect();
    p.s.ensure_loaded(&want);
    let t = std::time::Instant::now();
    while !want.iter().all(|c| p.s.chunks.get(c).is_some_and(Option::is_some)) {
        assert!(t.elapsed().as_secs() < 120, "flat world generation too slow");
        p.s.poll_gen();
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    (p, dir)
}

/// Body at 0 (towards −z), head at 60°: the gaze follows the head in every phase (horizontally:
/// (−sin 60°, cos 60°), whatever the pitch); when strafing, with the player straight ahead of
/// the body (angle < 10°) and the charge ready, the fireball starts from the head minus this gaze.
#[test]
fn dragon_looks_and_shoots_along_its_head_yaw() {
    let (mut p, dir) = open("look");
    let mut m = p.s.new_mob(Kind::EnderDragon, [0.5, -50.0, 20.5], 0.0);
    m.head_yaw = 60.0;
    let mut d = m.end.dragon.take().expect("dragon");
    let (s60, c60) = (60f64.to_radians().sin(), 60f64.to_radians().cos());
    for phase in [Phase::Takeoff, Phase::Landing, Phase::SittingScanning, Phase::HoldingPattern] {
        d.set_phase(phase);
        let l = p.s.dragon_head_look(&m, &d);
        let n = (l[0] * l[0] + l[2] * l[2]).sqrt();
        assert!((l[0] / n + s60).abs() < 1e-3 && (l[2] / n - c60).abs() < 1e-3, "{phase:?} {l:?}");
    }
    d.set_phase(Phase::StrafePlayer);
    d.ph.strafe_attack = Some(Ent::Player);
    d.ph.strafe_charge = 4;
    d.ph.strafe_target = Some([0.5, -50.0, 0.5]);
    d.parts = [[0.5, -50.0, 14.0]; 8];
    p.s.phase_tick(&mut m, &mut d);
    let f = p.s.proj.list.iter().find(|q| q.kind == PKind::DragonFireball).expect("fireball fired");
    // Head (0.5, −50, 14) minus the gaze (−sin 60°, 0, cos 60°); height = head + 0.5 + 0.5.
    let want = [0.5 + s60, -49.0, 14.0 - c60];
    assert!((0..3).all(|k| (f.pos[k] - want[k]).abs() < 1e-3), "{:?} {want:?}", f.pos);
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}
