//! Reference trajectories computed by hand from the constants of `mounts.md` § 3.

use super::*;
use crate::Tables;

const AIR: u32 = 0;
const STONE: u32 = 1;
const WATER: u32 = 2;
const ICE: u32 = 3;

fn tables() -> Tables {
    let full = vec![Aabb { min: [0.0; 3], max: [1.0; 3] }];
    let mut fluids = FluidTables::from_parts(
        vec![Fluid::None, Fluid::None, Fluid::Water, Fluid::None],
        vec![0.0, 0.0, 8.0 / 9.0, 0.0],
    );
    fluids.source[2] = true;
    Tables {
        shapes: vec![vec![], full.clone(), vec![], full],
        top: Vec::new(),
        oversized: vec![false; 4],
        friction: vec![0.6, 0.6, 0.6, 0.98],
        speed_factor: vec![1.0; 4],
        jump_factor: vec![1.0; 4],
        fluids,
        bounce: vec![0.0; 4],
        inside: vec![0; 4],
        block: vec![0, 1, 2, 3],
        climbable: vec![false; 4],
        support_keep: vec![0; 4],
        no_bounce: vec![false; 4],
        lily_pad: u16::MAX,
        suff_rule: vec![crate::SUFF_NEVER; 4],
        suffocating: vec![false; 4],
        air: Vec::new(),
        special: crate::Special::default(),
        piston: Default::default(),
    }
}

use crate::FluidTables;

/// Ground `floor` under y = 0; `water`: a layer of source water at y = 0 above the stone.
struct Flat {
    floor: u32,
    water: bool,
}

impl World for Flat {
    fn state(&self, _x: i32, y: i32, _z: i32) -> Option<u32> {
        Some(match y {
            0 if self.water => WATER,
            -1 if self.water => STONE,
            y if y < 0 => self.floor,
            _ => AIR,
        })
    }

    fn collision(&self, x: i32, y: i32, z: i32, out: &mut Vec<Aabb>) -> bool {
        if matches!(self.state(x, y, z), Some(STONE | ICE)) {
            out.push(Aabb { min: [0.0; 3], max: [1.0; 3] });
        }
        false
    }
}

/// Two ticks without input: the mount lands (gravity follows the first movement).
fn settle(m: &mut Mount, w: &Flat, t: &Tables) {
    for _ in 0..2 {
        m.tick(&pilot(Controls::default()), w, t);
    }
}

fn pilot(c: Controls) -> Pilot {
    Pilot { yaw: 0.0, pitch: 0.0, controls: c, sprinting: false }
}

fn forward() -> Controls {
    Controls::from_input(Input { forward: true, ..Default::default() })
}

fn f(x: f32) -> f64 {
    f64::from(x)
}

/// Boat at equilibrium in calm water: buoyancy 0.65 → surface − y = 0.65 × 0.5625.
fn floating_boat() -> Mount {
    let level = f(8.0 / 9.0);
    let mut m = Mount::new(Kind::Boat, [0.5, level - 0.65 * 0.5625, 0.5], 0.0, 0.0, 1.375, 0.5625);
    m.status = Status::InWater;
    m
}

#[test]
fn boat_accelerates_on_calm_water() {
    let t = tables();
    let w = Flat { floor: STONE, water: true };
    let mut m = floating_boat();
    let (a, k) = (f(0.04), f(0.9));
    let mut v = 0.0;
    let mut z = m.pos[2];
    for _ in 0..5 {
        let out = m.tick(&pilot(forward()), &w, &t);
        assert_eq!(out.paddles, Some((true, true)));
        assert_eq!(m.status, Status::InWater);
        // Damping (0.9) then push (0.04), then movement.
        v = v * k + a;
        z += v;
        assert!((m.pos[2] - z).abs() < 1e-12, "z {} expected {z}", m.pos[2]);
    }
    for _ in 0..300 {
        m.tick(&pilot(forward()), &w, &t);
    }
    // Limit step: 0.04 / (1 − 0.9) = 0.4 block per tick (0.36 after damping).
    let z0 = m.pos[2];
    m.tick(&pilot(forward()), &w, &t);
    assert!((m.pos[2] - z0 - a / (1.0 - k)).abs() < 1e-6);
    assert!((m.pos[1] - (f(8.0 / 9.0) - 0.365_625)).abs() < 1e-3, "floats at {}", m.pos[1]);
}

#[test]
fn boat_turns_left_in_place() {
    let t = tables();
    let w = Flat { floor: STONE, water: true };
    let mut m = floating_boat();
    let left = Controls::from_input(Input { left: true, ..Default::default() });
    let mut r = 0.0f32;
    let mut yaw = 0.0f32;
    for _ in 0..4 {
        let out = m.tick(&pilot(left), &w, &t);
        // Right paddle alone (meta 12).
        assert_eq!(out.paddles, Some((false, true)));
        r = r * 0.9 - 1.0;
        yaw += r;
        assert!((m.yaw - yaw).abs() < 1e-5);
    }
    // Small forward push (0.005) during the turn: z grows.
    assert!(m.pos[2] > 0.5);
}

#[test]
fn boat_on_land_and_ice() {
    let t = tables();
    for (floor, k) in [(STONE, 0.6f32), (ICE, 0.98)] {
        let w = Flat { floor, water: false };
        let mut m = Mount::new(Kind::Boat, [0.5, 0.0, 0.5], 0.0, 0.0, 1.375, 0.5625);
        // First tick: landed, "on land" state.
        settle(&mut m, &w, &t);
        assert_eq!(m.status, Status::OnLand);
        assert!(m.on_ground);
        let (a, k) = (f(0.04), f(k));
        let mut v = 0.0;
        let mut z = m.pos[2];
        for _ in 0..20 {
            m.tick(&pilot(forward()), &w, &t);
            v = v * k + a;
            z += v;
        }
        assert!((m.pos[2] - z).abs() < 1e-5, "ground {floor}: z {} expected {z}", m.pos[2]);
        assert_eq!(m.pos[1], 0.0);
    }
}

#[test]
fn boat_falls_in_air() {
    let t = tables();
    let w = Flat { floor: STONE, water: false };
    let mut m = Mount::new(Kind::Boat, [0.5, 10.0, 0.5], 0.0, 0.0, 1.375, 0.5625);
    let mut vy = 0.0;
    let mut y = 10.0;
    for _ in 0..5 {
        m.tick(&pilot(Controls::default()), &w, &t);
        assert_eq!(m.status, Status::InAir);
        // Gravity 0.04, no vertical drag.
        vy -= 0.04;
        y += vy;
        assert!((m.pos[1] - y).abs() < 1e-12);
    }
}

#[test]
fn boat_bounce_keeps_eight_tenths_of_block_restitution() {
    // Block that gives back all the momentum (slime): the boat, a non-living entity, keeps only 0.8 of it.
    let mut t = tables();
    t.bounce[ICE as usize] = 1.0;
    let w = Flat { floor: ICE, water: false };
    let mut m = Mount::new(Kind::Boat, [0.5, 0.3, 0.5], 0.0, 0.0, 1.375, 0.5625);
    m.vel[1] = -0.46;
    m.tick(&pilot(Controls::default()), &w, &t);
    assert_eq!(m.pos[1], 0.0);
    // Speed before movement −0.5 (gravity 0.04), travelled share 0.3 / 0.5, drag 1.
    let p = 0.3 / 0.5;
    let expected = (p * 0.04 + 0.5) * f(0.8f32);
    assert!((m.vel[1] - expected).abs() < 1e-9, "bounce {} expected {expected}", m.vel[1]);
}

#[test]
fn boat_speed_factor_ignores_waterlogged_blocks() {
    // Flooded block (not a water block): the slowing block's factor still applies.
    let mut t = tables();
    t.speed_factor[ICE as usize] = 0.4;
    t.fluids = FluidTables::from_parts(
        vec![Fluid::Water, Fluid::None, Fluid::Water, Fluid::None],
        vec![0.0, 0.0, 8.0 / 9.0, 0.0],
    );
    let w = Flat { floor: ICE, water: false };
    let m = Mount::new(Kind::Boat, [0.5, 0.0, 0.5], 0.0, 0.0, 1.375, 0.5625);
    assert_eq!(m.speed_factor(&w, &t), 0.4);
}

fn horse() -> Mount {
    let mut m =
        Mount::new(Kind::Equine { float: true, water_slowdown: 0.8 }, [0.5, 0.0, 0.5], 0.0, 0.0, f(1.396_484_4), 1.6);
    m.attrs.movement_speed = 0.225;
    m.attrs.jump_strength = 0.7;
    m.attrs.step_height = 1.0;
    m
}

#[test]
fn horse_walks_at_reference_speed() {
    let t = tables();
    let w = Flat { floor: STONE, water: false };
    let mut m = horse();
    settle(&mut m, &w, &t);
    assert!(m.on_ground);
    let c = forward();
    assert_eq!(c.zza, 0.98);
    // On the ground: acceleration = speed × input; friction 0.6 × 0.91.
    let a = f(0.98) * f(0.225f64 as f32);
    let k = f(0.6 * 0.91);
    let mut v = 0.0;
    let mut z = m.pos[2];
    for _ in 0..10 {
        m.tick(&pilot(c), &w, &t);
        v += a;
        z += v;
        v *= k;
        assert!((m.pos[2] - z).abs() < 1e-9, "z {} expected {z}", m.pos[2]);
    }
    // Limit: 0.2205 / (1 − 0.546) ≈ 0.4857 block per tick (9.7 m/s).
    for _ in 0..300 {
        m.tick(&pilot(c), &w, &t);
    }
    let z0 = m.pos[2];
    m.tick(&pilot(c), &w, &t);
    assert!((m.pos[2] - z0 - a / (1.0 - k)).abs() < 1e-6);
    // Slowed backward walk (× 0.25) and sideways step (× 0.5).
    let back = Controls::from_input(Input { backward: true, ..Default::default() });
    assert_eq!(m.ridden_input(&pilot(back)), [0.0, 0.0, f(-0.98 * 0.25)]);
}

#[test]
fn horse_charged_jump() {
    let t = tables();
    let w = Flat { floor: STONE, water: false };
    let mut m = horse();
    settle(&mut m, &w, &t);
    assert_eq!(pending_scale(100), 1.0);
    assert!((pending_scale(45) - 0.6).abs() < 1e-6);
    m.on_jump(100);
    assert!(m.allow_slide);
    let y0 = m.pos[1];
    m.tick(&pilot(Controls::default()), &w, &t);
    assert_eq!(m.pending_jump, 0.0);
    // v_y = strength × delay = 0.7; then (0.7 − 0.08) × 0.98.
    let vy0 = f(0.7f64 as f32 * 1.0);
    assert!((m.pos[1] - y0 - vy0).abs() < 1e-9);
    m.tick(&pilot(Controls::default()), &w, &t);
    let vy1 = (vy0 - 0.08) * f(0.98);
    assert!((m.pos[1] - y0 - vy0 - vy1).abs() < 1e-9);
    // Jump while moving forward: push 0.4 × delay towards the yaw.
    let mut m = horse();
    settle(&mut m, &w, &t);
    m.on_jump(100);
    m.tick(&pilot(forward()), &w, &t);
    assert!(m.vel[2] > 0.4 * 0.5);
}

#[test]
fn pig_follows_the_look() {
    let t = tables();
    let w = Flat { floor: STONE, water: false };
    let mut m = Mount::new(Kind::Pig, [0.5, 0.0, 0.5], 0.0, 0.0, 0.9, 0.9);
    m.attrs.movement_speed = 0.25;
    settle(&mut m, &w, &t);
    // Speed 0.25 × 0.225; input (0, 0, 1) even without a key; player's view at 90°.
    let p = Pilot { yaw: 90.0, ..pilot(Controls::default()) };
    let a = f((0.25 * 0.225) as f32);
    let x0 = m.pos[0];
    m.tick(&p, &w, &t);
    assert_eq!(m.yaw, 90.0);
    assert!((m.pos[0] - x0 + a).abs() < 1e-7, "dx {}", m.pos[0] - x0);
    // Acceleration: factor 1 + 1.15 sin(t/T π).
    m.start_boost(140);
    m.tick(&p, &w, &t);
    assert_eq!(m.boost, Some((1, 140)));
    assert!((m.boost_factor() - (1.0 + 1.15 * sin_t(1.0 / 140.0 * std::f32::consts::PI))).abs() < 1e-6);
}

#[test]
fn happy_ghast_level_flight() {
    let t = tables();
    let w = Flat { floor: STONE, water: false };
    let mut m = Mount::new(Kind::HappyGhast, [0.5, 10.0, 0.5], 0.0, 0.0, 4.0, 4.0);
    m.attrs.flying_speed = 0.05;
    let c = forward();
    let s = f(0.05f32 * 5.0 / 3.0);
    let input = f(3.9f32) * 0.05;
    let mut v = 0.0;
    let mut z = m.pos[2];
    for _ in 0..200 {
        m.tick(&pilot(c), &w, &t);
        v += input * s;
        z += v;
        v *= f(0.91);
    }
    assert!((m.pos[2] - z).abs() < 1e-9);
    assert_eq!(m.pos[1], 10.0);
    // ≈ 0.18 block per tick on the plateau.
    assert!((v / f(0.91) - input * s / (1.0 - f(0.91))).abs() < 1e-6);
    // Slow turn: 8 % of the yaw difference per tick.
    let p = Pilot { yaw: 100.0, ..pilot(Controls::default()) };
    m.tick(&p, &w, &t);
    assert!((m.yaw - 8.0).abs() < 1e-5);
}

#[test]
fn camel_dash() {
    let t = tables();
    let w = Flat { floor: STONE, water: false };
    let mut m = Mount::new(Kind::Camel, [0.5, 0.0, 0.5], 0.0, 0.0, 1.7, 2.375);
    m.attrs.movement_speed = 0.09;
    m.attrs.jump_strength = 0.42;
    settle(&mut m, &w, &t);
    m.on_jump(100);
    m.tick(&pilot(Controls::default()), &w, &t);
    assert_eq!(m.dash_cooldown, 54);
    assert!(m.dashing);
    // Dash: 22.2222 × 0.09 forward, 1.4285 × 0.42 upward, then the movement.
    let vz = f(22.2222) * 0.09;
    assert!((m.pos[2] - 0.5 - vz).abs() < 1e-6, "dz {}", m.pos[2] - 0.5);
    // The dash adds to the residual vertical speed on the ground (−0.08 × 0.98).
    assert!((m.pos[1] - (f(1.4285) * f(0.42) - 0.08 * f(0.98))).abs() < 1e-9, "y {}", m.pos[1]);
    // During the cooldown, a new strength is ignored.
    m.on_jump(100);
    assert_eq!(m.pending_jump, 0.0);
}
