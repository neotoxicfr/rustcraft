//! Dragon client effects (flap, growls, breaths, explosions) and a creaking tearing itself down
//! (no tilt, no red overlay, blinking eyes), following game 26.3.

use super::dragon::{Fx, client_fx};
use super::keyframes::testkit::{fire, spawn};
use super::*;
use crate::entities::Entities;
use crate::level_events::WorldRandom;
use crate::particles::Kind as PKind;
use rc_proto_v777::entity::Meta;

const FLAP: &str = "minecraft:entity.ender_dragon.flap";
const GROWL: &str = "minecraft:entity.ender_dragon.growl";

/// Dragon (id 7) in the given phase.
fn dragon(phase: i32) -> Entities {
    let mut all = spawn("ender_dragon");
    all.map.get_mut(&7).unwrap().meta.insert(16, Meta::Int(phase));
    all
}

/// One entity tick then the effects tick; `egg` = first free y at the podium.
fn step(all: &mut Entities, r: &mut WorldRandom, egg: i32) -> Vec<Fx> {
    all.tick();
    let mut out = Vec::new();
    client_fx(all.map.get_mut(&7).unwrap(), r, &mut || egg, &mut |f| out.push(f));
    out
}

fn sounds(fx: &[Fx], name: &str) -> Vec<(f32, f32)> {
    fx.iter()
        .filter_map(|f| match f {
            Fx::Sound(n, v, p) if *n == name => Some((*v, *p)),
            _ => None,
        })
        .collect()
}

fn particles(fx: &[Fx], kind: PKind) -> Vec<([f64; 3], [f64; 3])> {
    fx.iter()
        .filter_map(|f| match f {
            Fx::Particle(k, p, v) if *k == kind => Some((*p, *v)),
            _ => None,
        })
        .collect()
}

/// Flap: at rest, the phase advances by 0.2 per tick: a sound every 5 ticks, volume 5,
/// pitch 0.8 + [0, 0.3[. Growl: the first on the 101st tick outside a sitting phase, then 200 to 399
/// ticks later, volume 2.5. Silent: neither.
#[test]
fn dragon_flaps_and_growls_like_the_game() {
    let mut r = WorldRandom::default();
    let mut all = dragon(0);
    let (mut flaps, mut growls) = (Vec::new(), Vec::new());
    for t in 1..=700 {
        let fx = step(&mut all, &mut r, 0);
        for (v, p) in sounds(&fx, FLAP) {
            assert!(v == 5.0 && (0.8..1.1).contains(&p), "flap {v} {p}");
            flaps.push(t);
        }
        for (v, p) in sounds(&fx, GROWL) {
            assert!(v == 2.5 && (0.8..1.1).contains(&p), "growl {v} {p}");
            growls.push(t);
        }
    }
    assert!(flaps.len() >= 130 && flaps.windows(2).all(|w| w[1] - w[0] == 5), "flaps {flaps:?}");
    assert_eq!(growls[0], 101, "growls {growls:?}");
    assert!(growls.len() >= 2 && growls.windows(2).all(|w| (201..=400).contains(&(w[1] - w[0]))), "{growls:?}");

    let mut all = dragon(0);
    all.map.get_mut(&7).unwrap().meta.insert(4, Meta::Bool(true));
    let quiet: usize = (0..300).map(|_| step(&mut all, &mut r, 0).len()).sum();
    assert_eq!(quiet, 0, "silent dragon");
    // Sitting phase (hover): flaps, no periodic growl.
    let mut all = dragon(10);
    let fx: Vec<Fx> = (0..300).flat_map(|_| step(&mut all, &mut r, 0)).collect();
    assert!(sounds(&fx, GROWL).is_empty() && !sounds(&fx, FLAP).is_empty());
}

/// Sitting attack: roar every tick, even for a silent dragon.
#[test]
fn sitting_attack_roars_every_tick() {
    let mut r = WorldRandom::default();
    let mut all = dragon(7);
    all.map.get_mut(&7).unwrap().meta.insert(4, Meta::Bool(true));
    for _ in 0..20 {
        assert_eq!(sounds(&step(&mut all, &mut r, 0), GROWL).len(), 1);
    }
}

/// Dying (health 0): one explosion per tick around the body (±4, 2 ± 2, ±4); emitters on
/// death ticks 180 to 200; no phase effect.
#[test]
fn dying_dragon_explodes() {
    let mut r = WorldRandom::default();
    let mut all = dragon(3);
    all.map.get_mut(&7).unwrap().meta.insert(9, Meta::Float(0.0));
    let mut emitters = Vec::new();
    for t in 1..=210 {
        let fx = step(&mut all, &mut r, 0);
        let boom = particles(&fx, PKind::Explosion);
        assert_eq!(boom.len(), 1, "tick {t}");
        let (p, v) = boom[0];
        assert!((p[0] - 0.5).abs() <= 4.0 && (p[1] - 66.0).abs() <= 2.0 && (p[2] - 0.5).abs() <= 4.0 && v == [0.0; 3]);
        assert!(particles(&fx, PKind::DragonBreath).is_empty());
        if !particles(&fx, PKind::ExplosionEmitter).is_empty() {
            emitters.push(t);
        }
    }
    assert_eq!(emitters, (180..=200).collect::<Vec<_>>());
}

/// Flight to the podium to die (phase 9, still alive): emitter every 10 ticks, starting with the
/// first.
#[test]
fn death_phase_emits_every_ten_ticks() {
    let mut r = WorldRandom::default();
    let mut all = dragon(9);
    let at: Vec<i32> =
        (1..=25).filter(|_| !particles(&step(&mut all, &mut r, 0), PKind::ExplosionEmitter).is_empty()).collect();
    assert_eq!(at, vec![1, 11, 21]);
}

/// Sitting flames: breaths on phase ticks 2, 4, 6 and 8, 8 positions × 6; gaze lowered
/// by 45°: y velocity = −sin 45° × 0.6, x and z according to the rank (0 to 5); all
/// positions leave in the same direction (rotation lost by the game). New phase: the
/// counter restarts.
#[test]
fn sitting_flames_breathe_like_the_game() {
    let mut r = WorldRandom::default();
    let mut all = dragon(5);
    let counts: Vec<usize> =
        (0..12).map(|_| particles(&step(&mut all, &mut r, 0), PKind::DragonBreath).len()).collect();
    assert_eq!(counts, vec![0, 48, 0, 48, 0, 48, 0, 48, 0, 0, 0, 0]);
    all.map.get_mut(&7).unwrap().meta.insert(16, Meta::Int(6));
    step(&mut all, &mut r, 0);
    all.map.get_mut(&7).unwrap().meta.insert(16, Meta::Int(5));
    step(&mut all, &mut r, 0);
    let b = particles(&step(&mut all, &mut r, 0), PKind::DragonBreath);
    assert_eq!(b.len(), 48, "counter reset by the new phase");
    let s45 = std::f64::consts::FRAC_1_SQRT_2;
    let (f08, f06) = (f64::from(0.08f32), f64::from(0.6f32));
    for (i, (_, v)) in b.iter().enumerate() {
        let j = (i % 6) as f64;
        let want = [0.0, -s45 * f06, -s45 * f08 * j];
        assert!((0..3).all(|k| (v[k] - want[k]).abs() < 1e-3), "breath {i} {v:?} {want:?}");
    }
    // Head: yaw zero, sitting (−1), level flight: 6.5 in front (−z), y + 0.5 − 1; ± Gaussian/2.
    let n = b.len() as f64;
    let mean: [f64; 3] = std::array::from_fn(|k| b.iter().map(|(p, _)| p[k]).sum::<f64>() / n);
    assert!((mean[0] - 0.5).abs() < 0.6 && (mean[1] - 63.5).abs() < 0.6 && (mean[2] + 6.0).abs() < 0.6, "{mean:?}");
}

/// Landing: 8 breaths per tick, same velocity for all; near the egg (distance ≤ 4) the
/// gaze is lowered by 45°: y velocity = −sin 45° × 0.3 (motionless dragon).
#[test]
fn landing_breathes_toward_the_egg() {
    let mut r = WorldRandom::default();
    let mut all = dragon(3);
    let b = particles(&step(&mut all, &mut r, 63), PKind::DragonBreath);
    assert_eq!(b.len(), 8);
    assert!(b.iter().all(|(_, v)| v == &b[0].1), "rotation lost: same direction");
    let want = -std::f64::consts::FRAC_1_SQRT_2 * f64::from(0.3f32);
    assert!((b[0].1[1] - want).abs() < 1e-3, "{:?}", b[0].1);
    // Far from the egg (40 blocks lower): almost horizontal gaze, slower fall.
    let far = particles(&step(&mut all, &mut r, 24), PKind::DragonBreath);
    assert!(far[0].1[1] < 0.0 && far[0].1[1] > want / 2.0, "{:?}", far[0].1);
}

/// Dragon spawned with a given body yaw and head yaw, in the given phase.
fn dragon_turned(phase: i32, yaw: f32, head_yaw: f32) -> Entities {
    let mut all = Entities { type_names: vec!["minecraft:ender_dragon".into()], ..Default::default() };
    let add = rc_proto_v777::entity::AddEntity {
        id: 7,
        uuid: 1,
        kind: 0,
        pos: [0.5, 64.0, 0.5],
        velocity: [0.0; 3],
        pitch: 0.0,
        yaw,
        head_yaw,
        data: 0,
    };
    all.apply(rc_client::EntityEvent::Add(Box::new(add)));
    all.map.get_mut(&7).unwrap().meta.insert(16, Meta::Int(phase));
    all
}

/// Sitting flames of a still, turned dragon (body at 90°, head at 0), sloped flight
/// history, yaw that changes during the emission tick. Compared draw for draw with a
/// reference dragon (zero yaw, level flight, same seed):
/// - head at the previous tick: previous yaw (90°) → 6.5 × cos(tilt) towards +x, z unchanged;
///   tilt = (y from 6 ticks ago − y from 11 ticks ago) × 10°;
/// - gaze along the previous tick's head (0, towards −z: still, no catch-up), not the
///   body. The head packet received here would only play on the following ticks: its rejection
///   is checked by `turning_dragon_breathes_along_its_dragged_head`.
#[test]
fn still_dragon_breathes_along_its_head_yaw() {
    let run = |turned: bool| {
        let mut r = WorldRandom::default();
        let mut all = if turned { dragon_turned(5, 90.0, 0.0) } else { dragon_turned(5, 0.0, 0.0) };
        step(&mut all, &mut r, 0);
        if turned {
            // Head packet received: never applied to the dragon's gaze.
            all.apply(rc_client::EntityEvent::Head { id: 7, yaw: 90.0 });
            let e = all.map.get_mut(&7).unwrap();
            // After the next tick's advance, the sample from m ticks ago is 64 − 0.2 m.
            let h = e.extra[0] as usize;
            for k in 0..64 {
                e.history[(h + 64 - k) & 63][0] = 64.0 - 0.2 * (k + 1) as f64;
            }
        }
        all.tick();
        let e = all.map.get_mut(&7).unwrap();
        if turned {
            // The yaw went from 90° to 120° during this tick: previous 90°, current 120°.
            assert_eq!(e.yaw_o, 90.0);
            e.yaw = 120.0;
        }
        let mut out = Vec::new();
        client_fx(e, &mut r, &mut || 0, &mut |f| out.push(f));
        particles(&out, PKind::DragonBreath)
    };
    let (base, turned) = (run(false), run(true));
    assert_eq!((base.len(), turned.len()), (48, 48));
    let tilt = (1.0f64).to_radians() * 10.0;
    // Reference: head at (0.5; 63.5; 0.5 − 6.5); turned dragon: (0.5 + 6.5 cos; 63.5 + 6.5 sin; 0.5).
    let shift = [6.5 * tilt.cos(), 6.5 * tilt.sin(), 6.5];
    let s45 = std::f64::consts::FRAC_1_SQRT_2;
    let (f08, f06) = (f64::from(0.08f32), f64::from(0.6f32));
    for (i, ((pb, _), (pt, vt))) in base.iter().zip(&turned).enumerate() {
        assert!((0..3).all(|k| (pt[k] - pb[k] - shift[k]).abs() < 1e-3), "head {i} {pt:?} {pb:?}");
        let j = (i % 6) as f64;
        let want = [0.0, -s45 * f06, -s45 * f08 * j];
        assert!((0..3).all(|k| (vt[k] - want[k]).abs() < 1e-4), "gaze {i} {vt:?} {want:?}");
    }
}

/// Landing of a still, turned dragon (body at 90°, head at 0): breath towards −z.
#[test]
fn still_dragon_lands_along_its_head_yaw() {
    let mut r = WorldRandom::default();
    let mut all = dragon_turned(3, 90.0, 0.0);
    let b = particles(&step(&mut all, &mut r, 63), PKind::DragonBreath);
    assert_eq!(b.len(), 8);
    let v = b[0].1;
    let want = -std::f64::consts::FRAC_1_SQRT_2 * f64::from(0.08f32);
    assert!(v[0].abs() < 1e-6 && (v[2] - want).abs() < 1e-4, "{v:?}");
    // Reverse: body at 0, head at 90°: breath towards +x (gaze towards −x).
    let mut all = dragon_turned(3, 0.0, 90.0);
    let v = particles(&step(&mut all, &mut r, 63), PKind::DragonBreath)[0].1;
    assert!((v[0] + want).abs() < 1e-4 && v[2].abs() < 1e-6, "{v:?}");
}

/// Dragon turning in flight, moved by movement packets (interpolation path): yaw
/// from 0 to 180° in steps of 10°, spawn head at 0. The mobs' branch pulls the head to within 75° of
/// the body while it moves: it ends at 105° (the game, which brings the dragon's yaw back into
/// [−180, 180[, gets −255°, same angle). A head packet received afterwards (−30°) is never
/// applied. At every tick, the landing breath (near the egg: gaze lowered by 45°,
/// dragon velocity added) follows the previous tick's head: neither the body, nor the current head,
/// nor the spawn one. Dragon turned to 180°, game: (+0.055; −0.212; +0.015) + velocity.
#[test]
fn turning_dragon_breathes_along_its_dragged_head() {
    use rc_client::EntityEvent;
    use rc_proto_v777::entity::{Delta, Move};
    let mut r = WorldRandom::default();
    let mut all = dragon_turned(3, 0.0, 0.0);
    let v = [0.02, -0.01, 0.03];
    all.apply(EntityEvent::Motion { id: 7, velocity: v });
    let (f08, f03) = (f64::from(0.08f32), f64::from(0.3f32));
    // Expected breath (exact trigonometry) for a head yaw: gaze (−sin yaw × cos 45°,
    // sin 45°, cos yaw × cos 45°).
    let want = |yaw: f32| {
        let (s, c) = (-f64::from(yaw).to_radians().sin(), f64::from(yaw).to_radians().cos());
        let h = std::f64::consts::FRAC_1_SQRT_2;
        [-s * h * f08 + v[0], -h * f03 + v[1], -c * h * f08 + v[2]]
    };
    // One tick: breath along the head from before the tick; returns (head before, head after).
    let tick = |all: &mut Entities, r: &mut WorldRandom| {
        let before = all.map[&7].head;
        let b = particles(&step(all, r, 63), PKind::DragonBreath);
        assert_eq!(b.len(), 8);
        let w = want(before);
        assert!((0..3).all(|k| (b[0].1[k] - w[k]).abs() < 1e-4), "head before {before}: {:?} {w:?}", b[0].1);
        (before, all.map[&7].head)
    };
    let mv = |yaw: f32| Move { id: 7, on_ground: false, delta: Some(Delta::Linear([41, 0, 0])), rot: Some((yaw, 0.0)) };
    let mut turned = 0;
    // Turn, then a few packets at 180° to finish the turn while moving.
    for k in 1..=24 {
        all.apply(EntityEvent::Move(mv(10.0 * k.min(18) as f32)));
        let (before, after) = tick(&mut all, &mut r);
        turned += i32::from(before != after);
    }
    assert!(turned > 10, "head caught up at every tick of the turn: {turned}");
    // No more packets: the interpolation ends, the dragon stops, the head stays.
    for _ in 0..10 {
        tick(&mut all, &mut r);
    }
    let e = &all.map[&7];
    assert_eq!((e.yaw, e.head), (180.0, 105.0));
    all.apply(EntityEvent::Head { id: 7, yaw: -30.0 });
    for _ in 0..4 {
        assert_eq!(tick(&mut all, &mut r), (105.0, 105.0), "head packet applied to the dragon");
    }
    let b = particles(&step(&mut all, &mut r, 63), PKind::DragonBreath)[0].1;
    let game = [0.054_641 + v[0], -0.212_132 + v[1], 0.014_641 + v[2]];
    assert!((0..3).all(|k| (b[k] - game[k]).abs() < 1e-4), "{b:?} {game:?}");
}

/// Creaking tearing itself down (value 18): eyes lit on the first death tick, off on the
/// next, then stay off 2 to 8 ticks (+ 1) and lit t/4 to t/2 ticks (+ 1); no death
/// tilt and no red overlay. Without value 18: eyes = active (value 17), tilt and overlay.
#[test]
fn tearing_creaking_blinks_without_flip_or_red() {
    let mut all = spawn("creaking");
    all.tick();
    all.map.get_mut(&7).unwrap().meta.insert(18, Meta::Bool(true));
    fire(&mut all, 3);
    let mut lit = Vec::new();
    for _ in 0..45 {
        all.tick();
        lit.push(pulse_alpha(&all.map[&7], 5, 0.0) != 0.0);
    }
    assert!(lit[0] && !lit[1], "{lit:?}");
    let mut runs = Vec::new();
    let mut start = 0;
    for t in 1..lit.len() {
        if lit[t] != lit[t - 1] {
            runs.push((lit[start], start + 1, t - start));
            start = t;
        }
    }
    assert!(runs.len() >= 4, "{lit:?}");
    for &(on, at, len) in &runs[1..] {
        if on {
            assert!((at / 4 + 1..=at / 2 + 1).contains(&len), "lit at {at}: {len} {lit:?}");
        } else {
            assert!((3..=9).contains(&len), "off {len} {lit:?}");
        }
    }
    let e = &all.map[&7];
    assert!(e.death_time > 0 && !red_overlay(e));
    let ctx = RotCtx {
        shaking: false,
        upside_down: false,
        fall_flying: false,
        swim: 0.0,
        env: PoseEnv::default(),
        height: 2.7,
        shared_body_angle: true,
        skip_identity_factors: true,
    };
    let mut m = Mat4::IDENTITY;
    pose_rotations(e, 0.0, 0.0, &mut m, &ctx);
    let flat = m.transform_vector3(Vec3::Y);
    assert!((flat - Vec3::Y).length() < 1e-5, "no tilt: {flat}");

    let e = all.map.get_mut(&7).unwrap();
    e.meta.insert(18, Meta::Bool(false));
    e.meta.insert(17, Meta::Bool(true));
    assert_eq!(pulse_alpha(e, 5, 0.0), 1.0);
    assert!(red_overlay(e));
    let mut m = Mat4::IDENTITY;
    pose_rotations(e, 0.0, 0.0, &mut m, &ctx);
    assert!((m.transform_vector3(Vec3::Y) - Vec3::Y).length() > 0.5, "ordinary death tilt");
}
