//! Sustained sounds of the client: elytra loop and End flash sound, on an engine
//! with no output and a synthetic table.

use std::path::PathBuf;
use std::sync::Arc;

use rc_assets::Assets;
use rc_audio::{AudioEngine, SoundTable};

use super::*;

const JSON: &str = r#"{
  "item.elytra.flying": {"sounds": ["test/elytra"]},
  "weather.end_flash": {"sounds": ["test/flash"]}
}"#;

/// Engine with no output on minimal resources in `target/test-audio-client/<name>`.
fn engine(name: &str) -> AudioEngine {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/test-audio-client").join(name);
    let snd = dir.join("assets/minecraft/sounds/test");
    std::fs::create_dir_all(&snd).unwrap();
    for f in ["elytra", "flash"] {
        std::fs::write(snd.join(format!("{f}.ogg")), b"x").unwrap();
    }
    std::fs::write(dir.join("assets/minecraft/sounds.json"), JSON).unwrap();
    let a = Arc::new(Assets::new(vec![rc_assets::Source::dir(&dir)]));
    let t = SoundTable::load(&[&*a]);
    assert_eq!(t.len(), 2, "incomplete synthetic table");
    AudioEngine::headless(a, t, 48_000).0
}

/// Volume and pitch of the elytra loop (game values): silent 20 ticks, fade-in until
/// 40, speed² / 4 capped at 1, pitch raised beyond 0.8.
#[test]
fn elytra_volume_and_pitch() {
    assert_eq!(elytra_params(1, 4.0), (0.0, 1.0));
    assert_eq!(elytra_params(19, 4.0), (0.0, 1.0));
    assert_eq!(elytra_params(20, 4.0), (0.0, 1.0));
    assert_eq!(elytra_params(30, 2.0), (0.25, 1.0));
    assert_eq!(elytra_params(40, 2.0), (0.5, 1.0));
    let (v, p) = elytra_params(50, 3.6);
    assert!((v - 0.9).abs() < 1e-6 && (p - 1.1).abs() < 1e-6, "{v} {p}");
    assert_eq!(elytra_params(50, 100.0), (1.0, 1.2));
    assert_eq!(elytra_params(50, 0.5e-7), (0.0, 1.0));
}

/// The loop starts when gliding begins, follows the player, becomes audible after 20 ticks and
/// stops as soon as the flight has ceased past those 20 ticks; resuming starts a new loop.
#[test]
fn elytra_loop_follows_fall_flying() {
    let mut a = engine("elytra");
    let mut ws = WorldSounds::default();
    let mut p = rc_physics::Player::new([10.0, 80.0, -5.0], 0.0, 0.0);
    p.vel = [1.0, -1.0, 0.0];
    ws.tick_elytra(&mut a, &p);
    assert!(ws.elytra.is_empty(), "no sound without gliding");
    p.fall_flying = true;
    ws.tick_elytra(&mut a, &p);
    assert_eq!(ws.elytra.len(), 1, "loop started on gliding");
    let h = ws.elytra[0].0;
    let (pos, gain, _) = a.sound_params(h).unwrap();
    assert_eq!((pos, gain), ([10.0, 80.0, -5.0], 0.0), "placed on the player, silent at the start");
    for _ in 0..29 {
        ws.tick_elytra(&mut a, &p);
    }
    // Tick 30: speed² 2 → 0.5 × (10 / 20).
    assert_eq!(a.sound_params(h).unwrap().1, 0.25);
    assert_eq!(ws.elytra.len(), 1, "a single loop while the flight continues");
    p.fall_flying = false;
    ws.tick_elytra(&mut a, &p);
    assert!(ws.elytra.is_empty() && !a.is_playing(h), "stopped at the end of the flight");
    // Flight resumed then stopped before 20 ticks: the loop holds until the 21st tick.
    p.fall_flying = true;
    ws.tick_elytra(&mut a, &p);
    p.fall_flying = false;
    for _ in 0..19 {
        ws.tick_elytra(&mut a, &p);
    }
    assert_eq!(ws.elytra.len(), 1);
    ws.tick_elytra(&mut a, &p);
    assert!(ws.elytra.is_empty());
}

/// End flash sound: 30 ticks after the start, 10 blocks from the camera in the direction
/// of the flash, unattenuated, then repositioned with the camera.
#[test]
fn end_flash_sound_is_delayed_and_follows_camera() {
    let mut a = engine("end-flash");
    let mut ws = WorldSounds::default();
    // Pitch 0, yaw 0: direction (0, 0, 1) (yaw shifted by a half turn, cosine of the
    // pitch sign-flipped: the two compensate).
    ws.queue_end_flash((0.0, 0.0));
    let cam = [100.0, 64.0, 100.0];
    for _ in 0..29 {
        ws.tick_end_flash(&mut a, cam);
    }
    assert!(ws.end_flash.is_none(), "too early");
    ws.tick_end_flash(&mut a, cam);
    let (h, _) = ws.end_flash.expect("started at the 30th tick");
    let (pos, gain, pitch) = a.sound_params(h).unwrap();
    assert_eq!((gain, pitch), (1.0, 1.0));
    assert!((pos[0] - 100.0).abs() < 1e-5 && pos[1] == 64.0 && (pos[2] - 110.0).abs() < 1e-5, "{pos:?}");
    ws.tick_end_flash(&mut a, [0.0, 0.0, 0.0]);
    let pos = a.sound_params(h).unwrap().0;
    assert!(pos[0].abs() < 1e-5 && (pos[2] - 10.0).abs() < 1e-5, "follows the camera: {pos:?}");
}

/// Respawn in another dimension (leaving the End): the queued flash does not ring at the
/// 30th tick and the one playing stops; the elytra loops too.
#[test]
fn respawn_to_other_dimension_drops_end_flash_and_elytra() {
    let mut a = engine("respawn-dimension");
    let mut ws = WorldSounds::default();
    let cam = [0.0, 64.0, 0.0];
    ws.queue_end_flash((0.0, 0.0));
    for _ in 0..30 {
        ws.tick_end_flash(&mut a, cam);
    }
    let (playing, _) = ws.end_flash.expect("started at the 30th tick");
    ws.queue_end_flash((0.0, 0.0));
    let mut p = rc_physics::Player::new([0.0, 80.0, 0.0], 0.0, 0.0);
    p.fall_flying = true;
    ws.tick_elytra(&mut a, &p);
    let loop_h = ws.elytra[0].0;
    ws.respawn(Some(&mut a), true);
    assert!(!a.is_playing(playing) && !a.is_playing(loop_h), "sounds of the old world stopped");
    for _ in 0..30 {
        ws.tick_end_flash(&mut a, cam);
    }
    assert!(ws.end_flash.is_none(), "queued flash dropped");
    // Still gliding: the new player restarts a loop (silent for 20 ticks).
    ws.tick_elytra(&mut a, &p);
    assert_eq!(ws.elytra.len(), 1);
    assert_eq!(a.sound_params(ws.elytra[0].0).unwrap().1, 0.0);
}

/// Respawn in the same dimension: the queued flash still rings, only the elytra loop
/// of the old player stops and a new one starts.
#[test]
fn respawn_same_dimension_keeps_end_flash_restarts_elytra() {
    let mut a = engine("respawn-same");
    let mut ws = WorldSounds::default();
    ws.queue_end_flash((0.0, 0.0));
    let mut p = rc_physics::Player::new([0.0, 80.0, 0.0], 0.0, 0.0);
    p.fall_flying = true;
    ws.tick_elytra(&mut a, &p);
    let old = ws.elytra[0].0;
    ws.respawn(Some(&mut a), false);
    assert!(ws.elytra.is_empty() && !a.is_playing(old), "old player's loop stopped");
    ws.tick_elytra(&mut a, &p);
    assert_eq!(ws.elytra.len(), 1, "new loop for the new player");
    for _ in 0..30 {
        ws.tick_end_flash(&mut a, [0.0, 64.0, 0.0]);
    }
    assert!(ws.end_flash.is_some(), "same world: the flash rings");
}
