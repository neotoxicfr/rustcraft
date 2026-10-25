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
