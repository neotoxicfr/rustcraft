//! Squid and glow squid: swimming by impulses (movement vector applied at the strong beat
//! of the tentacle flapping), fall out of water, fleeing the aggressor, ink; the glow
//! squid darkens for 100 ticks when hit.

use std::f32::consts::PI;

use super::super::mob::{Ent, Mob};
use super::super::mth::{atan2, cos, sin};
use super::super::species::Kind;
use super::goals::AGoal;
use crate::server::Server;

/// Flapping and orientation of the squid (game fields, useful to the draws and movement).
#[derive(Debug, Clone, Default)]
pub struct SquidState {
    pub x_rot: f32,
    pub x_rot_o: f32,
    pub z_rot: f32,
    pub z_rot_o: f32,
    pub tentacle: f32,
    pub tentacle_o: f32,
    pub angle: f32,
    pub angle_o: f32,
    pub tentacle_speed: f32,
    pub rotate_speed: f32,
    pub movement: [f64; 3],
}

/// Flapping speed drawn on the entity random: 1 / (U + 1) × 0.2.
fn random_tentacle_speed(m: &mut Mob) -> f32 {
    1.0 / (m.rng.next_float() + 1.0) * 0.2
}

/// Constructor: initial flapping speed.
pub fn on_new(m: &mut Mob) {
    m.sea.squid.tentacle_speed = random_tentacle_speed(m);
}

/// Random movement (0), flee (1).
pub fn register(m: &mut Mob) {
    m.goals.add(0, AGoal::SquidMove.wrap());
    m.goals.add(1, AGoal::SquidFlee { ticks: 0 }.wrap());
}

/// End of the AI step: flapping, impulse in water, damped vertical fall out of water;
/// the glow squid counts down its darkening (and draws three doubles for its glows).
pub fn post_ai_step(s: &mut Server, m: &mut Mob) {
    let drag = (1.0 - (1.0 - 0.98f32) * m.air_drag_modifier()).clamp(0.0, 1.0);
    let sq = &mut m.sea.squid;
    sq.x_rot_o = sq.x_rot;
    sq.z_rot_o = sq.z_rot;
    sq.tentacle_o = sq.tentacle;
    sq.angle_o = sq.angle;
    sq.tentacle += sq.tentacle_speed;
    if f64::from(sq.tentacle) > std::f64::consts::PI * 2.0 {
        sq.tentacle -= PI * 2.0;
        if m.rng.next_int_bound(10) == 0 {
            m.sea.squid.tentacle_speed = random_tentacle_speed(m);
        }
        s.mob_event(m, 19);
    }
    let sq = &mut m.sea.squid;
    if m.in_water {
        if sq.tentacle < PI {
            let f = sq.tentacle / PI;
            sq.angle = sin(f * f * PI) * PI * 0.25;
            if f64::from(f) > 0.75 {
                m.vel = sq.movement;
                sq.rotate_speed = 1.0;
            } else {
                sq.rotate_speed *= 0.8;
            }
        } else {
            sq.angle = 0.0;
            m.vel = m.vel.map(|v| v * 0.9);
            sq.rotate_speed *= 0.99;
        }
        let v = m.vel;
        let h = (v[0] * v[0] + v[2] * v[2]).sqrt();
        let deg = 180.0 / PI;
        m.body_yaw += (-(atan2(v[0], v[2]) as f32) * deg - m.body_yaw) * 0.1;
        m.yaw = m.body_yaw;
        sq.z_rot += PI * sq.rotate_speed * 1.5;
        sq.x_rot += (-(atan2(h, v[1]) as f32) * deg - sq.x_rot) * 0.1;
    } else {
        sq.angle = sin(sq.tentacle).abs() * PI * 0.25;
        let mut yd = m.vel[1];
        match m.x.effects.iter().find(|e| e.id == "minecraft:levitation") {
            Some(e) => yd = 0.05 * f64::from(e.amp + 1),
            None => yd -= m.attrs.gravity.value(),
        }
        m.vel = [0.0, yd * f64::from(drag), 0.0];
        sq.x_rot += (-90.0 - sq.x_rot) * 0.02;
    }
    if m.kind == Kind::GlowSquid {
        if m.sea.dark > 0 {
            m.sea.dark -= 1;
        }
        // Ambient glow: particle with no server-side effect, but its coordinates draw three
        // doubles on the entity random at each tick.
        let _ = (m.rng.next_double(), m.rng.next_double(), m.rng.next_double());
    }
}

/// Tick of the squid's random movement goal: motionless after 100 ticks of inaction, otherwise new heading at
/// random (1 in 25), out of water or without a vector.
pub fn random_move_tick(m: &mut Mob) {
    if m.no_action_time > 100 {
        m.sea.squid.movement = [0.0; 3];
        return;
    }
    let mv = m.sea.squid.movement;
    let has = mv[0] * mv[0] + mv[1] * mv[1] + mv[2] * mv[2] > f64::from(1.0e-5f32);
    if m.rng.next_int_bound(super::super::mth::reduced(50)) == 0 || !m.in_water || !has {
        let a = m.rng.next_float() * (PI * 2.0);
        let x = cos(a) * 0.2;
        let y = -0.1 + m.rng.next_float() * 0.2;
        let z = sin(a) * 0.2;
        m.sea.squid.movement = [f64::from(x), f64::from(y), f64::from(z)];
    }
}

/// Flee: in water, aggressor known within 10 blocks.
pub fn flee_can_use(s: &Server, m: &Mob) -> bool {
    let Some((e, _)) = m.last_hurt_by_mob else { return false };
    m.in_water && s.ent_pos(e).is_some_and(|p| super::super::mth::dist2(m.pos, p) < 100.0)
}

/// Tick of the squid's flee goal: vector opposite to the aggressor (never normalized: the result of the
/// normalization is thrown away in the game), slowed beyond 5 blocks, flat toward the air.
pub fn flee_tick(s: &Server, m: &mut Mob) {
    let Some((e, _)) = m.last_hurt_by_mob else { return };
    let Some(a) = s.ent_pos(e) else { return };
    let mut to = [m.pos[0] - a[0], m.pos[1] - a[1], m.pos[2] - a[2]];
    let p = super::super::mth::block_of([m.pos[0] + to[0], m.pos[1] + to[1], m.pos[2] + to[2]]);
    let water = s.mob_fluid(p) == super::super::env::Fluid::Water;
    let air = s.is_air(p);
    if water || air {
        let len = (to[0] * to[0] + to[1] * to[1] + to[2] * to[2]).sqrt();
        if len > 0.0 {
            let mut speed = 3.0;
            if len > 5.0 {
                speed -= (len - 5.0) / 5.0;
            }
            if speed > 0.0 {
                to = to.map(|v| v * speed);
            }
        }
        if air {
            to[1] = 0.0;
        }
        m.sea.squid.movement = to.map(|v| v / 20.0);
    }
    let _ = Ent::Player;
}

/// Rotation of a vector by the body's pitch then yaw.
fn rotate(m: &Mob, v: [f64; 3]) -> [f64; 3] {
    let a = m.sea.squid.x_rot_o * (PI / 180.0);
    let (c, s) = (f64::from(cos(a)), f64::from(sin(a)));
    let v = [v[0], v[1] * c + v[2] * s, v[2] * c - v[1] * s];
    let b = -m.body_yaw * (PI / 180.0);
    let (c, s) = (f64::from(cos(b)), f64::from(sin(b)));
    [v[0] * c + v[2] * s, v[1], v[2] * c - v[0] * s]
}

/// Ink: spray sound, then 30 particles (three draws per particle on the entity random).
pub fn spawn_ink(s: &mut Server, m: &mut Mob) {
    let snd = if m.kind == Kind::GlowSquid { "minecraft:entity.glow_squid.squirt" } else { "minecraft:entity.squid.squirt" };
    s.make_sound(m, snd);
    let base = rotate(m, [0.0, -1.0, 0.0]);
    let pos = [base[0] + m.pos[0], base[1] + m.pos[1] + 0.5, base[2] + m.pos[2]];
    let name = if m.kind == Kind::GlowSquid { "minecraft:glow_squid_ink" } else { "minecraft:squid_ink" };
    let off = if m.is_baby() { 0.1f32 } else { 0.3 };
    for _ in 0..30 {
        let x = f64::from(m.rng.next_float()) * 0.6 - 0.3;
        let z = f64::from(m.rng.next_float()) * 0.6 - 0.3;
        let d = rotate(m, [x, -1.0, z]);
        let k = f64::from(off + m.rng.next_float() * 2.0);
        let d = d.map(|v| (v * k) as f32);
        s.particles(name, pos, d, 0.1, 0);
    }
}
