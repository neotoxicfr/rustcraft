//! Projectiles simulated on the client between two server resyncs: the game moves
//! arrows, tridents, thrown items, fireballs, wind charges, spits, shulker
//! bullets, eyes of ender, rockets and fishing bobbers on every tick (same gravity, same
//! inertia, same order, current push and effects of the blocks crossed included); the server
//! only sends their position every 10 to 20 ticks. Blocks stop the projectile at the
//! exact point of the ray (collision shapes); entity hits stay with the server (it
//! resyncs or removes the projectile).

use std::f64::consts::PI;

use rc_physics::body::{Bobber, BobberTick, Body, Follow, clip_blocks};

use crate::entities::{Entities, Entity, rot_lerp};
use crate::particles::{Kind as PKind, Options as POptions, Particles};

/// Movement family of a type.
#[derive(Clone, Copy, PartialEq)]
enum Family {
    /// Arrows and tridents: inertia in water, gravity.
    Arrow {
        water: f32,
    },
    /// Thrown items: gravity then inertia.
    Thrown {
        gravity: f64,
    },
    /// Propelled projectiles: acceleration along the axis and inertia.
    Hurting {
        inertia: f32,
        liquid: f32,
        accel: f64,
    },
    Spit,
    Bullet,
    Eye,
    Firework,
    Bobber,
}

fn family(kind: &str) -> Option<Family> {
    Some(match kind {
        "arrow" | "spectral_arrow" => Family::Arrow { water: 0.6 },
        "trident" => Family::Arrow { water: 0.99 },
        "snowball" | "egg" | "ender_pearl" => Family::Thrown { gravity: 0.03 },
        "splash_potion" | "lingering_potion" => Family::Thrown { gravity: 0.05 },
        "experience_bottle" => Family::Thrown { gravity: 0.07 },
        "fireball" | "small_fireball" | "dragon_fireball" | "wither_skull" => {
            Family::Hurting { inertia: 0.95, liquid: 0.8, accel: 0.1 }
        }
        "wind_charge" | "breeze_wind_charge" => Family::Hurting { inertia: 1.0, liquid: 1.0, accel: 0.0 },
        "llama_spit" => Family::Spit,
        "shulker_bullet" => Family::Bullet,
        "eye_of_ender" => Family::Eye,
        "firework_rocket" => Family::Firework,
        "fishing_bobber" => Family::Bobber,
        _ => return None,
    })
}

fn f(x: f32) -> f64 {
    f64::from(x)
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// Orientation following the velocity, smoothed by a fifth (common rule of projectiles).
fn face_motion(e: &mut Entity, v: [f64; 3]) {
    let sd = (v[0] * v[0] + v[2] * v[2]).sqrt();
    let pitch = (v[1].atan2(sd) * 180.0 / PI) as f32;
    let yaw = (v[0].atan2(v[2]) * 180.0 / PI) as f32;
    e.pitch = rot_lerp(0.2, e.pitch, pitch);
    e.yaw = rot_lerp(0.2, e.yaw, yaw);
}

/// "Toward the movement" orientation of propelled projectiles and bullets (velocity `speed`).
fn turn_towards(e: &mut Entity, v: [f64; 3], speed: f32) {
    if v[0] * v[0] + v[1] * v[1] + v[2] * v[2] == 0.0 {
        return;
    }
    let sd = (v[0] * v[0] + v[2] * v[2]).sqrt();
    let yaw = (v[2].atan2(v[0]) * 180.0 / PI) as f32 + 90.0;
    let pitch = (sd.atan2(v[1]) * 180.0 / PI) as f32 - 90.0;
    e.pitch = rot_lerp(speed, e.pitch, pitch);
    e.yaw = rot_lerp(speed, e.yaw, yaw);
}

/// Physics kept for the entity (created at the first tick), resynced to the state received from the server.
fn take_body(e: &mut Entity, dims: &[[f32; 4]], fast_lava: bool) -> Box<Body> {
    let d = usize::try_from(e.kind_id).ok().and_then(|i| dims.get(i)).copied().unwrap_or([0.25, 0.25, 0.2125, 0.25]);
    let mut b = e
        .motion
        .take()
        .unwrap_or_else(|| Box::new(Body::new(e.pos, e.velocity, f64::from(d[0]), f64::from(d[1]), f64::from(d[2]))));
    b.pos = e.pos;
    b.vel = e.velocity;
    b.on_ground = e.on_ground;
    b.no_gravity = e.meta_bool(5);
    b.fast_lava = fast_lava;
    b
}

fn put_body(e: &mut Entity, b: Box<Body>) {
    e.pos = b.pos;
    e.velocity = b.vel;
    e.on_ground = b.on_ground;
    e.motion = Some(b);
}

/// Return of a trident with Loyalty (level `loyalty`): the height advances by a hundred-and-fiftieth
/// of the vertical gap per level, then the velocity decays by 5% and gains the acceleration
/// `0.05 × level` toward the thrower's eyes. Returns the new position and the new velocity.
fn loyalty_pull(pos: [f64; 3], vel: [f64; 3], eye: [f64; 3], loyalty: i8) -> ([f64; 3], [f64; 3]) {
    let l = f64::from(loyalty);
    let d = [eye[0] - pos[0], eye[1] - pos[1], eye[2] - pos[2]];
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    let n = if len < 1.0e-5 { [0.0; 3] } else { d.map(|c| c / len) };
    let accel = 0.05 * l;
    ([pos[0], pos[1] + d[1] * 0.015 * l, pos[2]], std::array::from_fn(|i| vel[i] * 0.95 + n[i] * accel))
}

/// Height of a player from the height of its eyes (standing, crouching, swimming or gliding).
fn player_height(eye: f64) -> f64 {
    if (eye - 1.62).abs() < 1.0e-3 {
        1.8
    } else if (eye - 1.27).abs() < 1.0e-3 {
        1.5
    } else if (eye - 0.4).abs() < 1.0e-3 {
        0.6
    } else {
        eye / 0.9
    }
}

/// Pseudo-random draw in [0, 1): the entity's own randomness (specific to each client
/// in the game too).
fn rand01(seed: &mut u64) -> f32 {
    *seed ^= *seed << 13;
    *seed ^= *seed >> 7;
    *seed ^= *seed << 17;
    (*seed >> 40) as f32 / (1u64 << 24) as f32
}

/// Standard Gaussian drawn from the entity's randomness.
fn gauss(seed: &mut u64) -> f64 {
    let u = f64::from(rand01(seed)).max(1e-12);
    let v = f64::from(rand01(seed));
    (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
}

/// Trail particles emitted by the projectiles' client tick.
struct Trail<'a> {
    ps: &'a mut Particles,
}

impl Trail<'_> {
    fn add(&mut self, k: PKind, pos: [f64; 3], vel: [f64; 3]) {
        self.ps.spawn(k, &POptions::default(), pos, vel);
    }

    /// Four bubbles a quarter of a movement backward, at the speed of the movement.
    fn bubbles(&mut self, pos: [f64; 3], mv: [f64; 3]) {
        for _ in 0..4 {
            self.add(PKind::Bubble, [pos[0] - mv[0] * 0.25, pos[1] - mv[1] * 0.25, pos[2] - mv[2] * 0.25], mv);
        }
    }

    /// Effect particles of an effect arrow (color of data 11, −1: none), at random
    /// in its box (0.5 × 0.5).
    fn arrow_effect(&mut self, e: &Entity, n: u32, seed: &mut u64) {
        let Some(color) = e.meta_int(11).filter(|&c| c != -1) else { return };
        let o = POptions { argb: color as u32, ..POptions::default() };
        for _ in 0..n {
            let x = e.pos[0] + f64::from(2.0 * rand01(seed) - 1.0) * 0.25;
            let y = e.pos[1] + 0.5 * f64::from(rand01(seed));
            let z = e.pos[2] + f64::from(2.0 * rand01(seed) - 1.0) * 0.25;
            self.ps.spawn(PKind::EntityEffect, &o, [x, y, z], [0.0; 3]);
        }
    }
}

/// One client tick of the known projectiles. `me`: local player (possible thrower of a trident,
/// holder of a hooked bobber); `game_time`: game time (bobber seed);
/// `fast_lava`: fast lava in this dimension (current push); `power`: acceleration power
/// received from the propelled projectiles; `ps`: particle trails of the tick.
#[allow(clippy::too_many_arguments)]
pub(crate) fn tick(
    ents: &mut Entities,
    phys: &rc_physics::Tables,
    w: &dyn rc_physics::World,
    me: Follow,
    game_time: i64,
    fast_lava: bool,
    power: &hashbrown::HashMap<i32, f64>,
    ps: &mut Particles,
) {
    let mut fx = Trail { ps };
    static SEED: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0x2545_f491_4f6c_dd1d);
    let mut seed = SEED.load(std::sync::atomic::Ordering::Relaxed);
    let mut boxes = Vec::new();
    let mut gone = Vec::new();
    // Eyes of the Loyalty trident throwers (id given at spawn).
    let owners: Vec<(i32, [f64; 3])> = ents
        .map
        .values()
        .filter(|e| e.kind == "trident" && e.meta_byte(11) > 0)
        .filter_map(|e| {
            let id = e.data;
            let eye = if id == me.id {
                me.eligible().then(|| [me.pos[0], me.pos[1] + me.eye, me.pos[2]])
            } else {
                let o = ents.map.get(&id)?;
                (o.death_time == 0).then(|| [o.pos[0], o.pos[1] + f64::from(o.dims(&ents.dims)[1]), o.pos[2]])
            };
            eye.map(|p| (id, p))
        })
        .collect();
    // Bobber hooks (id + 1 in data 8): feet of the entity at 0.8 of
    // its height.
    let hooks: Vec<(i32, [f64; 3])> = ents
        .map
        .values()
        .filter(|e| e.kind == "fishing_bobber")
        .filter_map(|e| {
            let id = e.meta_int(8).filter(|&v| v > 0)? - 1;
            let p = if id == me.id {
                [me.pos[0], me.pos[1] + player_height(me.eye) * 0.8, me.pos[2]]
            } else {
                let o = ents.map.get(&id)?;
                let h = f64::from(o.dims(&ents.dims)[0]);
                [o.pos[0], o.pos[1] + h * 0.8, o.pos[2]]
            };
            Some((id, p))
        })
        .collect();
    for (&id, e) in ents.map.iter_mut() {
        let Some(fam) = family(&e.kind) else { continue };
        if e.vehicle.is_some() {
            continue;
        }
        let no_gravity = e.meta_bool(5);
        let mut b = take_body(e, &ents.dims, fast_lava);
        match fam {
            Family::Arrow { water } => {
                let flags = match e.meta.get(&8) {
                    Some(rc_proto_v777::entity::Meta::Byte(b)) => *b,
                    _ => 0,
                };
                let physics = flags & 2 == 0;
                // Loyalty trident without physics: pulled toward its thrower's eyes, from
                // both sides, on every tick (the server only resyncs every 20 ticks).
                let loyalty = if e.kind == "trident" { e.meta_byte(11) } else { 0 };
                if loyalty > 0
                    && !physics
                    && let Some(&(_, eye)) = owners.iter().find(|(id, _)| *id == e.data)
                {
                    (b.pos, b.vel) = loyalty_pull(b.pos, b.vel, eye, loyalty);
                }
                let movement = b.vel;
                // Tip in a block: stuck.
                if physics {
                    let c = b.pos.map(|v| v.floor() as i32);
                    boxes.clear();
                    w.collision(c[0], c[1], c[2], &mut boxes);
                    let inside = boxes.iter().any(|x| {
                        (0..3).all(|k| {
                            let p = b.pos[k] - f64::from(c[k]);
                            p >= x.min[k] && p < x.max[k]
                        })
                    });
                    if inside {
                        b.vel = [0.0; 3];
                        e.meta.insert(10, rc_proto_v777::entity::Meta::Bool(true));
                    }
                }
                if e.meta_bool(10) && physics {
                    // Stuck: neither movement nor tick base (so neither currents nor fluids);
                    // an effect particle every 5 ticks when stuck.
                    put_body(e, b);
                    if e.kind == "arrow" && e.age.is_multiple_of(5) {
                        fx.arrow_effect(e, 1, &mut seed);
                    }
                    continue;
                }
                // Water inertia from the state of the previous tick's base, then bubbles.
                let origin = b.pos;
                let in_water = b.was_in_water();
                if in_water {
                    b.vel = b.vel.map(|v| v * f(water));
                    fx.bubbles(origin, b.vel);
                }
                // Critical arrow: 4 `crit` along the movement, backward and a little up.
                if flags & 1 != 0 {
                    for i in 0..4 {
                        let k = f64::from(i) / 4.0;
                        let at =
                            [origin[0] + movement[0] * k, origin[1] + movement[1] * k, origin[2] + movement[2] * k];
                        fx.add(PKind::Crit, at, [-movement[0], -movement[1] + 0.2, -movement[2]]);
                    }
                }
                let (yaw, pitch) = {
                    let m = if physics { movement } else { movement.map(|v| -v) };
                    let sd = (movement[0] * movement[0] + movement[2] * movement[2]).sqrt();
                    ((m[0].atan2(m[2]) * 180.0 / PI) as f32, (movement[1].atan2(sd) * 180.0 / PI) as f32)
                };
                e.pitch = rot_lerp(0.2, e.pitch, pitch);
                e.yaw = rot_lerp(0.2, e.yaw, yaw);
                let from = b.pos;
                let to = add(from, movement);
                let hit = if physics { clip_blocks(w, from, to) } else { None };
                b.pos = hit.unwrap_or(to);
                b.effects_after_set(w, phys, from);
                if hit.is_some() {
                    // Impact: 0.05 recoil against the movement, stop, stuck, shaking.
                    let v = b.vel;
                    b.pos =
                        std::array::from_fn(|i| b.pos[i] - v[i].signum() * f(0.05) * f64::from(u8::from(v[i] != 0.0)));
                    b.vel = [0.0; 3];
                    e.shake = 7;
                    e.meta.insert(10, rc_proto_v777::entity::Meta::Bool(true));
                }
                if !in_water {
                    b.vel = b.vel.map(|v| v * f(0.99));
                }
                let planted = e.meta_bool(10);
                if physics && !planted && !no_gravity {
                    b.vel[1] -= 0.05;
                }
                // Tick base: current push as long as it is not stuck.
                b.base_fluids(w, phys, !planted);
                put_body(e, b);
                match e.kind.as_str() {
                    "arrow" if planted && e.age.is_multiple_of(5) => fx.arrow_effect(e, 1, &mut seed),
                    "arrow" if !planted => fx.arrow_effect(e, 2, &mut seed),
                    "spectral_arrow" if !planted => {
                        let o = POptions { rgb: [1.0; 3], power: 1.0, ..POptions::default() };
                        fx.ps.spawn(PKind::Effect, &o, e.pos, [0.0; 3]);
                    }
                    _ => {}
                }
                continue;
            }
            Family::Thrown { gravity } => {
                if b.take_first_tick() {
                    b.bubble_columns_inside(w, phys);
                }
                if !no_gravity {
                    b.vel[1] -= gravity;
                }
                let k = if b.was_in_water() { 0.8 } else { 0.99 };
                b.vel = b.vel.map(|v| v * f(k));
                let from = b.pos;
                let to = add(from, b.vel);
                b.pos = clip_blocks(w, from, to).unwrap_or(to);
                face_motion(e, b.vel);
                b.effects_after_set(w, phys, from);
                b.base_fluids(w, phys, true);
            }
            Family::Hurting { inertia, liquid, accel } => {
                // Power received from the server with each velocity (otherwise that of the type).
                let accel = power.get(&id).copied().unwrap_or(accel);
                let in_water = b.was_in_water();
                let mut k = if in_water { liquid } else { inertia };
                if e.kind == "wither_skull" && e.meta_bool(8) && !in_water {
                    k = 0.73;
                }
                let v = b.vel;
                if in_water {
                    fx.bubbles(b.pos, v);
                }
                let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
                let n = if len < f(1.0e-5) { [0.0; 3] } else { v.map(|c| c / len) };
                b.vel = std::array::from_fn(|i| (v[i] + n[i] * accel) * f(k));
                let from = b.pos;
                let to = add(from, b.vel);
                let hit = clip_blocks(w, from, to);
                turn_towards(e, b.vel, 0.2);
                b.pos = hit.unwrap_or(to);
                b.effects_after_set(w, phys, from);
                b.base_fluids(w, phys, true);
                // Trail above the center: smoke, dragon breath; nothing for the wind.
                let at = [b.pos[0], b.pos[1] + 0.5, b.pos[2]];
                match e.kind.as_str() {
                    "dragon_fireball" => fx.add(PKind::DragonBreath, at, [0.0; 3]),
                    "fireball" | "small_fireball" | "wither_skull" => fx.add(PKind::Smoke, at, [0.0; 3]),
                    _ => {}
                }
            }
            Family::Spit => {
                // Tick base first: currents, then "in water" state of this tick.
                b.base_fluids(w, phys, true);
                let movement = b.vel;
                let to = add(b.pos, movement);
                face_motion(e, movement);
                // Touches something other than air (block, plant, fluid) or is in water: the
                // spit disappears.
                let bb = b.bounding_box();
                let lo = bb.min.map(|v| v.floor() as i32);
                let hi = bb.max.map(|v| v.floor() as i32);
                let solid = (lo[0]..=hi[0]).any(|x| {
                    (lo[1]..=hi[1]).any(|y| (lo[2]..=hi[2]).any(|z| !phys.is_air(w.state(x, y, z).unwrap_or(0))))
                });
                if solid || b.was_in_water() {
                    gone.push(id);
                    put_body(e, b);
                    continue;
                }
                b.vel = movement.map(|v| v * f(0.99));
                if !no_gravity {
                    b.vel[1] -= 0.06;
                }
                b.pos = to;
            }
            Family::Bullet => {
                b.base_fluids(w, phys, true);
                let v = b.vel;
                let from = b.pos;
                b.pos = add(from, v);
                b.effects_after_set(w, phys, from);
                turn_towards(e, v, 0.5);
                fx.add(PKind::EndRod, [b.pos[0] - v[0], b.pos[1] - v[1] + 0.15, b.pos[2] - v[2]], [0.0; 3]);
            }
            Family::Eye => {
                b.base_fluids(w, phys, true);
                let v = b.vel;
                let to = add(b.pos, v);
                // Trail a quarter of a movement backward: bubbles in water, otherwise portal.
                let origin = [to[0] - v[0] * 0.25, to[1] - v[1] * 0.25, to[2] - v[2] * 0.25];
                if b.was_in_water() {
                    for _ in 0..4 {
                        fx.add(PKind::Bubble, origin, v);
                    }
                } else {
                    let dx = f64::from(rand01(&mut seed)) * 0.6 - 0.3;
                    let dz = f64::from(rand01(&mut seed)) * 0.6 - 0.3;
                    fx.add(PKind::Portal, [origin[0] + dx, origin[1] - 0.5, origin[2] + dz], v);
                }
                b.pos = to;
            }
            Family::Firework => {
                b.base_fluids(w, phys, true);
                // Hooked to a holder: it follows the holder (rendered separately).
                let attached = matches!(e.meta.get(&9), Some(rc_proto_v777::entity::Meta::OptionalInt(Some(_))));
                if !attached {
                    let angle = e.meta_bool(10);
                    b.tick_firework(w, phys, angle);
                    face_motion(e, b.vel);
                }
                // Flight spark on every tick, at the rocket's position.
                let v = [gauss(&mut seed) * 0.05, -b.vel[1] * 0.5, gauss(&mut seed) * 0.05];
                fx.add(PKind::Firework, b.pos, v);
            }
            Family::Bobber => {
                let biting = e.meta_bool(9);
                let hooked =
                    e.meta_int(8).filter(|&v| v > 0).and_then(|v| hooks.iter().find(|h| h.0 == v - 1)).map(|h| h.1);
                let inp =
                    BobberTick { hooked, biting, seed: (e.uuid as u64 as i64) ^ game_time, rand: rand01(&mut seed) };
                let bob: &mut Bobber = e.bobber.get_or_insert_with(Default::default);
                // Bite received since the last tick: dive drawn from the synchronized randomness.
                if biting && !bob.biting {
                    b.vel[1] = bob.bite_velocity();
                }
                bob.biting = biting;
                let start = b.pos;
                b.tick_bobber(w, phys, bob, &inp);
                face_motion(e, b.vel);
                // Linear interpolation in progress: the target follows the simulated movement.
                let moved: [f64; 3] = std::array::from_fn(|i| b.pos[i] - start[i]);
                if moved != [0.0; 3] {
                    let (r, h) = (b.width / 2.0, b.height);
                    let free = |p: [f64; 3]| {
                        let bb =
                            rc_physics::Aabb { min: [p[0] - r, p[1], p[2] - r], max: [p[0] + r, p[1] + h, p[2] + r] };
                        rc_physics::is_free(w, &bb)
                    };
                    e.predict(moved, free);
                }
            }
        }
        put_body(e, b);
    }
    SEED.store(seed, std::sync::atomic::Ordering::Relaxed);
    for id in gone {
        ents.map.remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::loyalty_pull;

    #[test]
    fn loyalty_pull_accelerates_towards_owner_eye() {
        // Trident flying toward +x at 1, thrower 10 blocks behind toward -x and 2 higher, Loyalty 3.
        let (p, v) = loyalty_pull([0.0; 3], [1.0, 0.0, 0.0], [-10.0, 2.0, 0.0], 3);
        assert!((p[1] - 2.0 * 0.015 * 3.0).abs() < 1e-12);
        assert_eq!((p[0], p[2]), (0.0, 0.0));
        let len = (104.0f64).sqrt();
        assert!((v[0] - (0.95 - 10.0 / len * 0.15)).abs() < 1e-12);
        assert!((v[1] - 2.0 / len * 0.15).abs() < 1e-12);
        assert!(v[0] < 1.0, "slowed then pulled backward");
    }

    #[test]
    fn loyalty_pull_on_owner_keeps_damped_velocity() {
        let (p, v) = loyalty_pull([1.0, 2.0, 3.0], [1.0, 0.0, 0.0], [1.0, 2.0, 3.0], 1);
        assert_eq!(p, [1.0, 2.0, 3.0]);
        assert!((v[0] - 0.95).abs() < 1e-12 && v[1] == 0.0 && v[2] == 0.0);
    }
}
