//! Particles emitted by entities on each client tick: persistent effect cloud
//! (`area_effect_cloud`: dragon breath, lingering potions) and wither (head smoke,
//! half-health armor effect, summoning aura).
//!
//! Cloud metadata: 8 = radius (3 by default), 9 = waiting, 10 = particle (white entity effect
//! by default). Wither: 9 = health, 19 = invulnerability ticks.

use rc_proto_v777::entity::{Meta, Particle};

use crate::particles::{Kind as PKind, Options as POptions};

/// Render options of a protocol particle (same conversion as `level_particles`).
fn options(p: &Particle, force: bool) -> POptions {
    POptions::from_net(p, force)
}

/// Opaque ARGB color of an entity effect (float channels).
fn argb(r: f32, g: f32, b: f32) -> u32 {
    let q = |v: f32| (v * 255.0 + 0.5) as u32;
    0xff00_0000 | q(r) << 16 | q(g) << 8 | q(b)
}

thread_local! {
    /// Entities that touched the water at the previous tick (id; `LOCAL` for the player).
    static WET: std::cell::RefCell<hashbrown::HashMap<i32, bool>> = std::cell::RefCell::new(Default::default());
}

/// Key of the local player in `WET`.
const LOCAL: i32 = i32::MIN;

impl crate::App {
    /// Entity particles for one game tick (world not frozen).
    pub(crate) fn tick_entity_particles(&mut self) {
        self.tick_illusions();
        let game_time = self.env.env.clocks.game_time;
        let ents = &self.entities;
        let (ps, r) = (&mut self.particles, &mut self.world_random);
        for e in ents.map.values() {
            match e.kind.as_str() {
                "area_effect_cloud" => cloud(ps, r, e),
                "wither" => wither(ps, r, e),
                "ominous_item_spawner" if game_time % 5 == 0 => ominous_spawner(ps, r, e),
                // Enderman (client-side enderman animation tick): 2 portal particles per tick, at
                // ±0.5 width in x and z, at height × random − 0.25, velocity (±1, −[0, 1), ±1).
                "enderman" => {
                    let d = e.dims(&ents.dims);
                    let (w, h) = (f64::from(d[0]), f64::from(d[1]));
                    for _ in 0..2 {
                        let x = e.pos[0] + w * (2.0 * f64::from(r.f()) - 1.0) * 0.5;
                        let y = e.pos[1] + h * f64::from(r.f()) - 0.25;
                        let z = e.pos[2] + w * (2.0 * f64::from(r.f()) - 1.0) * 0.5;
                        let v = [(f64::from(r.f()) - 0.5) * 2.0, -f64::from(r.f()), (f64::from(r.f()) - 0.5) * 2.0];
                        ps.spawn(PKind::Portal, &POptions::default(), [x, y, z], v);
                    }
                }
                // Powered furnace minecart: big smoke one time in four.
                "furnace_minecart" if e.meta_bool(13) && r.f() < 0.25 => {
                    ps.spawn(PKind::LargeSmoke, &POptions::default(), [e.pos[0], e.pos[1] + 0.8, e.pos[2]], [0.0; 3]);
                }
                // Bee carrying nectar (flag 8 of data 18): 5% per tick, 1 or 2
                // nectar drops at mid-height, within ±0.3 in x and z.
                "bee" if e.meta_byte(18) & 8 != 0 && r.f() < 0.05 => {
                    let half = f64::from(e.dims(&ents.dims)[0]) * 0.5;
                    for _ in 0..1 + (r.f() * 2.0) as i32 {
                        let x = e.pos[0] - 0.3 + f64::from(r.f()) * 0.6;
                        let z = e.pos[2] - 0.3 + f64::from(r.f()) * 0.6;
                        ps.spawn(PKind::FallingNectar, &POptions::default(), [x, e.pos[1] + half, z], [0.0; 3]);
                    }
                }
                "wolf" | "glow_squid" | "panda" => animal(e, &ents.dims, r, &mut |k, o, p, v| {
                    ps.spawn(k, o, p, v);
                }),
                _ => {}
            }
        }
        self.water_splashes();
    }

    /// Invisible illusioners: illusion copies; on each dispersal, 16 clouds (x
    /// random over the width, y over the height, z at the edge + half-width like the game) and the
    /// mirror sound at the entity.
    fn tick_illusions(&mut self) {
        let dims = &self.entities.dims;
        let r = &mut self.world_random;
        let mut sounds = Vec::new();
        for e in self.entities.map.values_mut() {
            if e.kind != "illusioner" {
                continue;
            }
            if !crate::entity_render::illusion_tick(e, |n| ((r.f() * n as f32) as i32).min(n - 1)) {
                continue;
            }
            let d = e.dims(dims);
            let (w, h) = (f64::from(d[0]), f64::from(d[1]));
            for _ in 0..16 {
                let x = e.pos[0] + w * (2.0 * f64::from(r.f()) - 1.0) * 0.5;
                let y = e.pos[1] + h * f64::from(r.f());
                let z = e.pos[2] + w * 0.5;
                self.particles.spawn(PKind::Cloud, &POptions::default(), [x, y, z], [0.0; 3]);
            }
            sounds.push(e.pos);
        }
        if let Some(a) = self.audio.get() {
            for pos in sounds {
                a.play("minecraft:entity.illusioner.mirror_move", rc_audio::Category::Hostile, Some(pos), 1.0, 1.0, None);
            }
        }
    }

    /// Entering the water (`Entity`: transition from "out of water" to "in water", except on the first
    /// tick): bubbles and splashes over the entity's width, sound for the local player (the
    /// server plays it for the others).
    fn water_splashes(&mut self) {
        let w = crate::particles::TickWorld {
            grid: &self.world.grid,
            min_y: self.world.min_y,
            height: self.world.height,
            phys: &self.phys,
            player: None,
            scope: None,
        };
        let mut splashes: Vec<([f64; 3], [f64; 3], f64, bool)> = Vec::new();
        WET.with_borrow_mut(|wet| {
            let mut seen = hashbrown::HashSet::with_capacity(self.entities.map.len() + 1);
            let mut check = |id: i32, pos: [f64; 3], vel: [f64; 3], width: f64, height: f64, silent: bool| {
                seen.insert(id);
                let now = touches_water(&w, pos, width, height);
                if let Some(before) = wet.insert(id, now)
                    && now
                    && !before
                    && !silent
                {
                    splashes.push((pos, vel, width, id == LOCAL));
                }
            };
            for (&id, e) in &self.entities.map {
                // Never a splash: passengers, entities with no base tick (their "first tick"
                // flag never drops) and experience orb (empty effect).
                if e.vehicle.is_some() || no_splash_kind(&e.kind) {
                    continue;
                }
                // Spectator player: no splash.
                let silent = e.kind == "player" && self.players.map.get(&e.uuid).is_some_and(|p| p.game_mode == 3);
                let size = e.size(&self.entities.dims);
                let (width, height) = (f64::from(size.w), f64::from(size.h));
                check(id, e.pos, e.velocity, width, height, silent);
            }
            if self.positioned {
                let (pw, ph) = self.player.dimensions();
                check(LOCAL, self.player.pos, self.player.vel, pw, ph, self.env.game_mode == 3);
            }
            wet.retain(|id, _| seen.contains(id));
        });
        for (pos, vel, width, local) in splashes {
            let r = &mut self.world_random;
            // Sound: only for the local player (the server sends it to the others).
            if local && let Some(a) = self.audio.get() {
                let speed = ((vel[0] * vel[0] * 0.2 + vel[1] * vel[1] + vel[2] * vel[2] * 0.2) as f32).sqrt() * 0.2;
                let speed = speed.min(1.0);
                let pitch = 1.0 + (r.f() - r.f()) * 0.4;
                let name = if speed < 0.25 {
                    "minecraft:entity.player.splash"
                } else {
                    "minecraft:entity.player.splash.high_speed"
                };
                a.play(name, rc_audio::Category::Player, Some(pos), speed, pitch, None);
            }
            let y = pos[1].floor() + 1.0;
            // Bound in single precision like the game ("i < 1 + width × 20").
            let n = (1.0f32 + width as f32 * 20.0).ceil() as i32;
            let o = POptions::default();
            for _ in 0..n {
                let (xo, zo) = (f64::from(r.f() * 2.0 - 1.0) * width, f64::from(r.f() * 2.0 - 1.0) * width);
                let v = [vel[0], vel[1] - f64::from(r.f()) * f64::from(0.2f32), vel[2]];
                self.particles.spawn(PKind::Bubble, &o, [pos[0] + xo, y, pos[2] + zo], v);
            }
            for _ in 0..n {
                let (xo, zo) = (f64::from(r.f() * 2.0 - 1.0) * width, f64::from(r.f() * 2.0 - 1.0) * width);
                self.particles.spawn(PKind::Splash, &o, [pos[0] + xo, y, pos[2] + zo], vel);
            }
        }
    }
}

/// Entities that never make an entering-water splash.
fn no_splash_kind(kind: &str) -> bool {
    matches!(
        kind,
        "experience_orb"
            | "tnt"
            | "falling_block"
            | "block_display"
            | "item_display"
            | "text_display"
            | "interaction"
            | "marker"
            | "end_crystal"
            | "painting"
            | "item_frame"
            | "glow_item_frame"
            | "leash_knot"
    )
}

/// Does the box (width `width`, height `height`, feet at `pos`), shrunk by 0.001, touch
/// water (fluid top above the bottom of the box)?
fn touches_water(w: &crate::particles::TickWorld<'_>, pos: [f64; 3], width: f64, height: f64) -> bool {
    let e = 0.001;
    let (r, min_y) = (width / 2.0 - e, pos[1] + e);
    let lo = [(pos[0] - r).floor() as i32, min_y.floor() as i32, (pos[2] - r).floor() as i32];
    let hi = [(pos[0] + r).ceil() as i32 - 1, (pos[1] + height - e).ceil() as i32 - 1, (pos[2] + r).ceil() as i32 - 1];
    for x in lo[0]..=hi[0] {
        for y in lo[1]..=hi[1] {
            for z in lo[2]..=hi[2] {
                let (f, h) = w.fluid(x, y, z);
                if f == rc_physics::Fluid::Water && f64::from(y) + f64::from(h) >= min_y {
                    return true;
                }
            }
        }
    }
    false
}

fn cloud(ps: &mut crate::particles::Particles, r: &mut crate::level_events::WorldRandom, e: &crate::entities::Entity) {
    let waiting = e.meta_bool(9);
    let radius = match e.meta.get(&8) {
        Some(Meta::Float(v)) => *v,
        _ => 3.0,
    };
    if waiting && r.f() < 0.5 {
        return;
    }
    // Particle from the metadata; white entity effect by default.
    let default = POptions { argb: 0xffff_ffff, force: true, ..POptions::default() };
    let (kind, own) = match e.meta.get(&10) {
        Some(Meta::Particle(p)) => match ps.kind_of(p.kind) {
            Some(k) => (k, options(p, true)),
            None => return,
        },
        _ => (PKind::EntityEffect, default),
    };
    let (count, spread) =
        if waiting { (2, 0.2) } else { ((std::f32::consts::PI * radius * radius).ceil() as i32, radius) };
    for _ in 0..count.min(4096) {
        let angle = r.f() * std::f32::consts::TAU;
        let dist = r.f().sqrt() * spread;
        let pos = [e.pos[0] + f64::from(angle.cos() * dist), e.pos[1], e.pos[2] + f64::from(angle.sin() * dist)];
        if kind == PKind::EntityEffect {
            let o = if waiting && r.f() < 0.5 { default } else { own };
            ps.spawn(kind, &o, pos, [0.0; 3]);
        } else if waiting {
            ps.spawn(kind, &own, pos, [0.0; 3]);
        } else {
            let vel = [f64::from(0.5 - r.f()) * 0.15, f64::from(0.01f32), f64::from(0.5 - r.f()) * 0.15];
            ps.spawn(kind, &own, pos, vel);
        }
    }
}

/// Ominous item spawner (every 5 game ticks): 1 to 3 particles born at its position,
/// velocity = offset toward a point drawn at ±0.4 × (gaussian − gaussian) per axis.
fn ominous_spawner(
    ps: &mut crate::particles::Particles,
    r: &mut crate::level_events::WorldRandom,
    e: &crate::entities::Entity,
) {
    let (n, d) = ominous_offsets(r);
    for v in &d[..n] {
        ps.spawn(PKind::OminousSpawning, &POptions::default(), e.pos, *v);
    }
}

/// Count (1 to 3) and offsets of the particles of an ominous spawner: per axis,
/// 0.4 × (gaussian − gaussian).
fn ominous_offsets(r: &mut crate::level_events::WorldRandom) -> (usize, [[f64; 3]; 3]) {
    let n = 1 + ((r.f() * 3.0) as usize).min(2);
    let mut d = [[0.0f64; 3]; 3];
    for v in d.iter_mut().take(n) {
        for c in v.iter_mut() {
            *c = 0.4 * (r.gaussian() - r.gaussian());
        }
    }
    (n, d)
}

fn wither(ps: &mut crate::particles::Particles, r: &mut crate::level_events::WorldRandom, e: &crate::entities::Entity) {
    let powered = matches!(e.meta.get(&9), Some(Meta::Float(h)) if *h <= 150.0);
    let opts = POptions::default();
    for i in 0..3 {
        let (hx, hy, hz) = if i == 0 {
            (e.pos[0], e.pos[1] + 3.0, e.pos[2])
        } else {
            let a = (e.body + 180.0 * (i - 1) as f32).to_radians();
            (e.pos[0] + f64::from(a.cos()) * 1.3, e.pos[1] + 2.2, e.pos[2] + f64::from(a.sin()) * 1.3)
        };
        let at = |r: &mut crate::level_events::WorldRandom| {
            [hx + r.gaussian() * 0.3, hy + r.gaussian() * 0.3, hz + r.gaussian() * 0.3]
        };
        let p = at(r);
        ps.spawn(PKind::Smoke, &opts, p, [0.0; 3]);
        if powered && r.f() < 0.25 {
            let p = at(r);
            let o = POptions { argb: argb(0.7, 0.7, 0.5), rgb: [0.7, 0.7, 0.5], ..opts };
            ps.spawn(PKind::EntityEffect, &o, p, [0.0; 3]);
        }
    }
    if e.meta_int(19).unwrap_or(0) > 0 {
        let o = POptions { argb: argb(0.7, 0.7, 0.9), rgb: [0.7, 0.7, 0.9], ..opts };
        for _ in 0..3 {
            let p = [e.pos[0] + r.gaussian(), e.pos[1] + f64::from(r.f() * 3.3), e.pos[2] + r.gaussian()];
            ps.spawn(PKind::EntityEffect, &o, p, [0.0; 3]);
        }
    }
}

/// Tick particles of the wolf, glow squid and panda, emitted through `emit` (kind,
/// options, position, velocity) after the client tick of the families (`entity_render`).
fn animal(
    e: &crate::entities::Entity,
    dims: &[[f32; 4]],
    r: &mut crate::level_events::WorldRandom,
    emit: &mut impl FnMut(PKind, &POptions, [f64; 3], [f64; 3]),
) {
    let s = e.size(dims);
    let o = POptions::default();
    match e.kind.as_str() {
        // Shaking (extra 4 = in progress, extra 2 = progress, +0.05 per tick): past 0.4,
        // ⌊sin((progress − 0.4)·π)·7⌋ splashes 0.8 above the feet (single precision),
        // within ± half the width in x and z, thrown at the wolf's velocity.
        "wolf" => {
            let shake = e.extra[2];
            if e.death_time > 0 || e.extra[4] == 0.0 || shake <= 0.4 {
                return;
            }
            let n = (((shake - 0.4) * std::f32::consts::PI).sin() * 7.0) as i32;
            let y = f64::from(e.pos[1] as f32 + 0.8);
            for _ in 0..n {
                let xo = (r.f() * 2.0 - 1.0) * s.w * 0.5;
                let zo = (r.f() * 2.0 - 1.0) * s.w * 0.5;
                emit(PKind::Splash, &o, [e.pos[0] + f64::from(xo), y, e.pos[2] + f64::from(zo)], e.velocity);
            }
        }
        // Glow squid, alive or dying: one glow per tick, within ±0.6 width in x and z, at
        // height × random draw, motionless.
        "glow_squid" => {
            let (w, h) = (f64::from(s.w), f64::from(s.h));
            let x = e.pos[0] + (2.0 * f64::from(r.f()) - 1.0) * 0.6 * w;
            let y = e.pos[1] + f64::from(r.f()) * h;
            let z = e.pos[2] + (2.0 * f64::from(r.f()) - 1.0) * 0.6 * w;
            emit(PKind::Glow, &o, [x, y, z], [0.0; 3]);
        }
        // Panda (tick notes: extra 7, extra 8). Sneeze finished: one sneeze particle in front of the
        // muzzle ((width + 1)/2 along the body axis, 0.1 below the eyes), at the panda's
        // horizontal velocity. Eating: 6 bits of the held item, born 1 above the eyes in the
        // body frame, thrown in the gaze frame (+0.05 in y).
        "panda" => {
            let eye = e.pos[1] + f64::from(s.eye);
            if e.extra[7] != 0.0 {
                let d = f64::from(s.w + 1.0) * 0.5;
                let a = e.body.to_radians();
                let p = [e.pos[0] - d * f64::from(a.sin()), eye - f64::from(0.1f32), e.pos[2] + d * f64::from(a.cos())];
                emit(PKind::Sneeze, &o, p, [e.velocity[0], 0.0, e.velocity[2]]);
            }
            let Some(item) = e.equipment[0].as_ref().map(|s| s.item).filter(|_| e.extra[8] != 0.0) else {
                return;
            };
            let o = POptions { item, ..o };
            let rd = |r: &mut crate::level_events::WorldRandom| f64::from(r.f());
            for _ in 0..6 {
                let v = [(rd(r) - 0.5) * 0.1, rd(r) * 0.1 + 0.1, (rd(r) - 0.5) * 0.1];
                let v = crate::entity_events::rot_xy(v, e.pitch, e.yaw);
                let p = [(rd(r) - 0.5) * 0.8, -rd(r) * 0.6 - 0.3, 1.0 + (rd(r) - 0.5) * 0.4];
                let p = crate::entity_events::rot_xy(p, 0.0, e.body);
                emit(PKind::Item, &o, [p[0] + e.pos[0], p[1] + eye + 1.0, p[2] + e.pos[2]], [v[0], v[1] + 0.05, v[2]]);
            }
        }
        _ => {}
    }
}

/// Blocks with no rendering (invisible shape): no sprint particle on them.
pub(crate) fn invisible_shape(name: &str) -> bool {
    matches!(
        name,
        "minecraft:air"
            | "minecraft:cave_air"
            | "minecraft:void_air"
            | "minecraft:barrier"
            | "minecraft:light"
            | "minecraft:structure_void"
            | "minecraft:moving_piston"
            | "minecraft:water"
            | "minecraft:lava"
            | "minecraft:bubble_column"
            | "minecraft:end_portal"
            | "minecraft:end_gateway"
    )
}

/// Sprint particle (rabbit hop): position and velocity of a bit of the effect block `on`
/// for an entity with feet at `pos`, of width `width` and velocity `vel`; `rx`, `rz` in
/// [0, 1). Outside the entity's column, x or z is clamped to the effect block.
pub(crate) fn sprint_spot(
    pos: [f64; 3],
    width: f64,
    vel: [f64; 3],
    on: [i32; 3],
    rx: f64,
    rz: f64,
) -> ([f64; 3], [f64; 3]) {
    let mut x = pos[0] + (rx - 0.5) * width;
    let mut z = pos[2] + (rz - 0.5) * width;
    if pos[0].floor() as i32 != on[0] {
        x = x.clamp(f64::from(on[0]), f64::from(on[0]) + 1.0);
    }
    if pos[2].floor() as i32 != on[2] {
        z = z.clamp(f64::from(on[2]), f64::from(on[2]) + 1.0);
    }
    ([x, pos[1] + 0.1, z], [vel[0] * -4.0, 1.5, vel[2] * -4.0])
}

#[cfg(test)]
mod tests {
    use super::PKind;
    use crate::entities::Entities;

    /// An entity of kind `kind` (id 7) in an empty client world.
    fn spawn(kind: &str) -> Entities {
        let mut all = Entities { type_names: vec![format!("minecraft:{kind}")], ..Default::default() };
        let add = rc_proto_v777::entity::AddEntity {
            id: 7,
            uuid: 1,
            kind: 0,
            pos: [0.5, 64.0, 0.5],
            velocity: [0.0; 3],
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            data: 0,
        };
        all.apply(rc_client::EntityEvent::Add(Box::new(add)));
        all
    }

    /// Tick particles emitted for entity 7 (kind, position, velocity).
    fn emitted(all: &Entities, r: &mut crate::level_events::WorldRandom) -> Vec<(PKind, [f64; 3], [f64; 3])> {
        let mut out = Vec::new();
        super::animal(&all.map[&7], &all.dims, r, &mut |k, _, p, v| out.push((k, p, v)));
        out
    }

    /// Wolf shake (event 8): splashes per tick = ⌊sin((a − 0.4)·π)·7⌋ once the progress
    /// exceeds 0.4 (negative sine past 1.4: none), nothing after the stop (56).
    /// Before: no splashes.
    #[test]
    fn wolf_shake_splashes() {
        let mut all = spawn("wolf");
        let mut r = crate::level_events::WorldRandom::default();
        all.tick();
        all.apply(rc_client::EntityEvent::Event { id: 7, event: 8 });
        let mut counts = Vec::new();
        for _ in 0..42 {
            all.tick();
            let out = emitted(&all, &mut r);
            assert!(out.iter().all(|(k, p, v)| *k == PKind::Splash
                && (p[1] - 64.8).abs() < 1e-5
                && (p[0] - 0.5).abs() <= 0.3 + 1e-6
                && *v == [0.0; 3]));
            counts.push(out.len());
        }
        // Expected values stored (game sine, progress accumulated in single precision).
        let want: [usize; 42] = [
            0, 0, 0, 0, 0, 0, 0, 0, 1, 2, 3, 4, 4, 5, 6, 6, 6, 7, 6, 6, 6, 5, 4, 4, 3, 2, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0,
        ];
        assert_eq!(counts, want);
        // Stop through event 56: nothing more.
        all.apply(rc_client::EntityEvent::Event { id: 7, event: 8 });
        for _ in 0..12 {
            all.tick();
        }
        assert!(!emitted(&all, &mut r).is_empty());
        all.apply(rc_client::EntityEvent::Event { id: 7, event: 56 });
        all.tick();
        assert!(emitted(&all, &mut r).is_empty());
    }

    /// Glow squid: one glow per tick within ±0.6 width and over the height, motionless; the
    /// ordinary squid has none.
    #[test]
    fn glow_squid_glows_every_tick() {
        let mut r = crate::level_events::WorldRandom::default();
        let all = spawn("glow_squid");
        let s = all.map[&7].size(&all.dims);
        for _ in 0..50 {
            let out = emitted(&all, &mut r);
            assert_eq!(out.len(), 1);
            let (k, p, v) = out[0];
            assert_eq!((k, v), (PKind::Glow, [0.0; 3]));
            assert!((p[0] - 0.5).abs() <= 0.6 * f64::from(s.w) + 1e-9);
            assert!(p[1] >= 64.0 && p[1] <= 64.0 + f64::from(s.h));
        }
        assert!(emitted(&spawn("squid"), &mut r).is_empty());
    }

    /// Panda: sneeze finished (counter > 20) → one sneeze particle (width + 1)/2 in front of the
    /// muzzle (body at 0°: +z), 0.1 below the eyes; eating with bamboo → 6 bits every 5th
    /// counter value. Before: nothing.
    #[test]
    fn panda_sneeze_and_eating_bits() {
        use rc_proto_v777::entity::Meta;
        let mut r = crate::level_events::WorldRandom::default();
        let mut all = spawn("panda");
        all.tick();
        let e = all.map.get_mut(&7).unwrap();
        e.meta.insert(23, Meta::Byte(2));
        let mut sneezes = Vec::new();
        for t in 0..25 {
            all.tick();
            for (k, p, v) in emitted(&all, &mut r) {
                assert_eq!(k, PKind::Sneeze);
                sneezes.push((t, p, v));
            }
        }
        assert_eq!(sneezes.len(), 1, "{sneezes:?}");
        let e = &all.map[&7];
        let s = e.size(&all.dims);
        let (_, p, v) = sneezes[0];
        assert!((p[0] - 0.5).abs() < 1e-6, "{p:?}");
        assert!((p[2] - (0.5 + f64::from(s.w + 1.0) * 0.5)).abs() < 1e-6, "{p:?}");
        assert!((p[1] - (64.0 + f64::from(s.eye) - f64::from(0.1f32))).abs() < 1e-6, "{p:?}");
        assert_eq!(v, [0.0; 3]);
        // Eating: sitting (8) with an item, counter running at 3.
        let e = all.map.get_mut(&7).unwrap();
        e.meta.insert(23, Meta::Byte(8));
        e.equipment[0] =
            Some(rc_proto_v777::item::ItemStack { item: 42, count: 1, added: Vec::new(), removed: Vec::new() });
        e.meta.insert(20, Meta::Int(3));
        let mut per_tick = Vec::new();
        for _ in 0..12 {
            all.tick();
            let out = emitted(&all, &mut r);
            assert!(out.iter().all(|(k, p, _)| *k == PKind::Item && p[1] > 64.0 + f64::from(s.eye)));
            per_tick.push(out.len());
        }
        // Counter read 3, 4, 5, … 14: bits at 5 and 10.
        assert_eq!(per_tick, [0, 0, 6, 0, 0, 0, 0, 6, 0, 0, 0, 0]);
    }

    /// Sprint particle: ± half width around the feet, clamped to the effect block outside the
    /// column, 0.1 above the feet, velocity (vx·−4, 1.5, vz·−4).
    #[test]
    fn sprint_spot_clamps_to_effect_block() {
        let (p, v) = super::sprint_spot([0.5, 64.0, 0.5], 0.4, [0.1, 0.0, -0.2], [0, 63, 0], 1.0, 0.0);
        assert_eq!(p, [0.7, 64.1, 0.3]);
        assert_eq!(v, [-0.4, 1.5, 0.8]);
        // Feet at x = 0.95 on the neighbouring block x = 1 (supporting block): x clamped to [1, 2].
        let (p, _) = super::sprint_spot([0.95, 64.0, 0.5], 0.6, [0.0; 3], [1, 63, 0], 0.0, 0.5);
        assert_eq!(p[0], 1.0);
        assert!(super::invisible_shape("minecraft:water") && !super::invisible_shape("minecraft:grass_block"));
    }

    /// Ominous spawner: 1 to 3 particles (all three counts come out), offsets with standard deviation
    /// 0.4 × √2 per axis, centered.
    #[test]
    fn ominous_spawner_emits_one_to_three() {
        let mut r = crate::level_events::WorldRandom::default();
        let mut seen = [0usize; 4];
        let (mut sum, mut sq, mut k) = (0.0f64, 0.0f64, 0usize);
        for _ in 0..3000 {
            let (n, d) = super::ominous_offsets(&mut r);
            seen[n] += 1;
            for v in &d[..n] {
                for c in v {
                    sum += c;
                    sq += c * c;
                    k += 1;
                }
            }
            assert!(d[n..].iter().all(|v| *v == [0.0; 3]));
        }
        assert_eq!(seen[0], 0);
        assert!(seen[1..].iter().all(|&c| c > 800), "{seen:?}");
        let (mean, var) = (sum / k as f64, sq / k as f64 - (sum / k as f64).powi(2));
        assert!(mean.abs() < 0.03, "{mean}");
        assert!((var - 0.32).abs() < 0.03, "{var}");
    }
}
