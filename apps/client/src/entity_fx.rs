//! Particles emitted by entities on each client tick: persistent effect cloud
//! (`area_effect_cloud`: dragon breath, lingering potions) and wither (head smoke,
//! half-health armor effect, summoning aura); cube landing, sniffer digging, guardian bubbles
//! and flop, copper golem head spin (local particles and sounds).
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
        let (data, world) = (&self.data, &self.world);
        let state = |x: i32, y: i32, z: i32| {
            let c = world.grid.get(x >> 4, z >> 4)?;
            let ly = y - world.min_y;
            if !(0..world.height).contains(&ly) {
                return Some(0);
            }
            c.block((x & 15) as usize, ly as usize, (z & 15) as usize)
        };
        let me = (self.player_id, self.player.pos, self.player.dimensions().1);
        let mut sounds: Vec<Sound> = Vec::new();
        for (&id, e) in &ents.map {
            match e.kind.as_str() {
                "slime" | "magma_cube" | "sulfur_cube" if e.extra[7] != 0.0 => {
                    cube_landing(e, e.size(&ents.dims).w, r, &mut |k, p| {
                        ps.spawn(k, &POptions::default(), p, [0.0; 3]);
                    });
                }
                "copper_golem" if e.extra[6] != 0.0 && !e.meta_bool(4) => {
                    sounds.push(Sound::At(copper_spin_sound(e), rc_audio::Category::Neutral, e.pos));
                }
                "sniffer" => {
                    let spawn = &mut |k, o: &POptions, p| {
                        ps.spawn(k, o, p, [0.0; 3]);
                    };
                    sniffer(id, e, data, &state, spawn, &mut sounds);
                }
                "guardian" | "elder_guardian" if e.death_time == 0 => {
                    let target = guardian_target(ents, me, e.meta_int(17).unwrap_or(0));
                    let size = e.size(&ents.dims);
                    let spawn = &mut |p| {
                        ps.spawn(PKind::Bubble, &POptions::default(), p, [0.0; 3]);
                    };
                    guardian(e, (f64::from(size.w), f64::from(size.h), f64::from(size.eye)), target, r, spawn);
                    if e.extra[10] != 0.0 {
                        let flop = if e.kind == "elder_guardian" {
                            "minecraft:entity.elder_guardian.flop"
                        } else {
                            "minecraft:entity.guardian.flop"
                        };
                        sounds.push(Sound::At(flop, rc_audio::Category::Hostile, e.pos_o));
                    }
                }
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
                _ => {}
            }
        }
        for s in sounds {
            match s {
                Sound::At(name, cat, pos) => {
                    if let Some(a) = self.audio.get() {
                        a.play(name, cat, Some(pos), 1.0, 1.0, None);
                    }
                }
                Sound::Bound(id, name, volume, pitch) => {
                    self.ev_bound_sound(id, &name, rc_audio::Category::Neutral, volume, pitch);
                }
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

/// Sound requested by a mob during the particle tick, played after the loop.
enum Sound {
    /// Local sound placed at a position (volume and pitch 1).
    At(&'static str, rc_audio::Category, [f64; 3]),
    /// Sound bound to the entity (id, name, volume, pitch), neutral category.
    Bound(i32, String, f32, f32),
}

/// Cube landed at this tick (extra 7, noted by the mob tick): 16 × size particles
/// (size = 2 × width), on a ring of radius size/2 × [0.5, 1) around the feet, with no
/// velocity; slime item, flame or sulfur goo depending on the kind. The squash sound comes from the server.
fn cube_landing(
    e: &crate::entities::Entity,
    width: f32,
    r: &mut crate::level_events::WorldRandom,
    spawn: &mut impl FnMut(PKind, [f64; 3]),
) {
    let kind = match e.kind.as_str() {
        "slime" => PKind::ItemSlime,
        "magma_cube" => PKind::Flame,
        _ => PKind::SulfurCubeGoo,
    };
    let size = width * 2.0;
    let radius = size / 2.0;
    let mut i = 0u32;
    while (i as f32) < size * 16.0 {
        let dir = r.f() * std::f32::consts::TAU;
        let d = r.f() * 0.5 + 0.5;
        let (xd, zd) = (dir.sin() * radius * d, dir.cos() * radius * d);
        spawn(kind, [e.pos[0] + f64::from(xd), e.pos[1], e.pos[2] + f64::from(zd)]);
        i += 1;
    }
}

/// Head spin sound of the copper golem according to its oxidation (entity data 16).
fn copper_spin_sound(e: &crate::entities::Entity) -> &'static str {
    match e.meta_int(16).unwrap_or(0) {
        2 => "minecraft:entity.copper_golem_weathered.spin",
        3 => "minecraft:entity.copper_golem_oxidized.spin",
        _ => "minecraft:entity.copper_golem.spin",
    }
}

/// Blocks with an invisible render shape (air, fluids, barrier, light, structure void, moving
/// piston, End portals, bubble column): list taken from the 26.3 game, no extracted data
/// carries it.
fn invisible_render(data: &rc_data::VersionData, state: u32) -> bool {
    data.block_of(state).is_none_or(|b| {
        matches!(
            b.name.as_str(),
            "minecraft:air"
                | "minecraft:cave_air"
                | "minecraft:void_air"
                | "minecraft:water"
                | "minecraft:lava"
                | "minecraft:barrier"
                | "minecraft:light"
                | "minecraft:structure_void"
                | "minecraft:moving_piston"
                | "minecraft:end_portal"
                | "minecraft:end_gateway"
                | "minecraft:bubble_column"
        )
    })
}

/// Block at the sniffer's head: 2.25 in front of it (full orientation, pitch included), at
/// 0.2 above the feet; position and orientation from the start of the tick.
fn sniffer_head_block(e: &crate::entities::Entity) -> [i32; 3] {
    use rc_physics::{cos_t, sin_t};
    const DEG: f32 = std::f32::consts::PI / 180.0;
    let (y, x) = (-e.yaw_o * DEG - std::f32::consts::PI, -e.pitch_o * DEG);
    let xc = -cos_t(x);
    // Only the head's x and z matter; the height is the feet's + 0.2.
    let (fx, fz) = (f64::from(sin_t(y) * xc), f64::from(cos_t(y) * xc));
    [
        (e.pos_o[0] + fx * 2.25).floor() as i32,
        (e.pos_o[1] + f64::from(0.2f32)).floor() as i32,
        (e.pos_o[2] + fz * 2.25).floor() as i32,
    ]
}

/// Sniffer: while searching (state 4 of entity data 18), searching sound every 20 ticks; while
/// digging (5), between 1.7 s and 6 s of animation, 30 particles of the block under the head
/// (unless its render is invisible) at 0.65 below the center of the head, and that block's
/// hit sound every 10 ticks.
fn sniffer(
    id: i32,
    e: &crate::entities::Entity,
    data: &rc_data::VersionData,
    state: &impl Fn(i32, i32, i32) -> Option<u32>,
    spawn: &mut impl FnMut(PKind, &POptions, [f64; 3]),
    sounds: &mut Vec<Sound>,
) {
    match e.meta_int(18) {
        Some(4) if e.age.is_multiple_of(20) => {
            sounds.push(Sound::Bound(id, "minecraft:entity.sniffer.searching".to_owned(), 1.0, 1.0));
        }
        Some(5) => {
            let Some(start) = crate::entity_render::sniffer_dig_start(e) else { return };
            let ms = ((e.age as f32 - start) * 50.0) as i64;
            if ms <= 1700 || ms >= 6000 {
                return;
            }
            let head = sniffer_head_block(e);
            let below = state(head[0], head[1] - 1, head[2]).unwrap_or(0);
            if invisible_render(data, below) {
                return;
            }
            let o = POptions { state: below, ..POptions::default() };
            let c = [f64::from(head[0]) + 0.5, f64::from(head[1]) + 0.5 - f64::from(0.65f32), f64::from(head[2]) + 0.5];
            for _ in 0..30 {
                spawn(PKind::Block, &o, c);
            }
            if e.age.is_multiple_of(10)
                && let Some(t) = crate::world_sounds::sound_type(data, below)
            {
                sounds.push(Sound::Bound(id, t.hit.clone(), 0.5, 0.5));
            }
        }
        _ => {}
    }
}

/// Living target of the guardian's beam (entity data 17, 0 = none): feet and height; `me` =
/// (id, feet, height) of the local player.
fn guardian_target(
    ents: &crate::entities::Entities,
    me: (i32, [f64; 3], f64),
    id: i32,
) -> Option<([f64; 3], f64)> {
    if id == 0 {
        return None;
    }
    if id == me.0 {
        return Some((me.1, me.2));
    }
    let t = ents.map.get(&id).filter(|t| t.class != crate::entities::Kind::Other)?;
    Some((t.pos, f64::from(t.size(&ents.dims).h)))
}

/// Living guardian (position and orientation from the start of the tick): while swimming in
/// water, 2 bubbles per tick in its box, moved back by 1.5 × its view direction; with a target,
/// bubbles along the beam, spaced by 1.8 − charge + [0, 1.7 − charge). `size` = (width, height, eyes).
fn guardian(
    e: &crate::entities::Entity,
    size: (f64, f64, f64),
    target: Option<([f64; 3], f64)>,
    r: &mut crate::level_events::WorldRandom,
    spawn: &mut impl FnMut([f64; 3]),
) {
    let (w, h, eye) = size;
    let p = e.pos_o;
    if e.meta_bool(16) && e.in_water {
        let v = rc_physics::ray::view_vector(e.pitch_o, e.head_o);
        for _ in 0..2 {
            let x = p[0] + (2.0 * f64::from(r.f()) - 1.0) * w * 0.5 - v[0] * 1.5;
            let y = p[1] + h * f64::from(r.f()) - v[1] * 1.5;
            let z = p[2] + (2.0 * f64::from(r.f()) - 1.0) * w * 0.5 - v[2] * 1.5;
            spawn([x, y, z]);
        }
    }
    let Some((t, th)) = target else { return };
    let dur = if e.kind == "elder_guardian" { 60.0 } else { 80.0 };
    let at = f64::from(e.extra[5] / dur);
    let eye_y = p[1] + eye;
    let mut d = [t[0] - p[0], t[1] + th * 0.5 - eye_y, t[2] - p[2]];
    let dd = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    d = d.map(|c| c / dd);
    let mut dist = f64::from(r.f());
    while dist < dd {
        dist += 1.8 - at + f64::from(r.f()) * (1.7 - at);
        spawn([p[0] + d[0] * dist, eye_y + d[1] * dist, p[2] + d[2] * dist]);
    }
}

#[cfg(test)]
mod creature_tests {
    use super::*;
    use crate::entities::{Entities, Entity};
    use rc_client::EntityEvent;

    fn spawn(kind: &str, pos: [f64; 3]) -> Entities {
        let mut all = Entities { type_names: vec![format!("minecraft:{kind}")], ..Default::default() };
        let add = rc_proto_v777::entity::AddEntity {
            id: 7,
            uuid: 1,
            kind: 0,
            pos,
            velocity: [0.0; 3],
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            data: 0,
        };
        all.apply(EntityEvent::Add(Box::new(add)));
        all
    }

    fn one(kind: &str, pos: [f64; 3]) -> Entity {
        spawn(kind, pos).map.remove(&7).expect("spawned")
    }

    /// Air (0), stone (1, stone sounds), water (2).
    fn data() -> rc_data::VersionData {
        let block = |name: &str, first: u32| rc_data::BlockInfo {
            name: name.into(),
            first_state: first,
            state_count: 1,
            default_state: first,
            friction: 0.6,
            speed_factor: 1.0,
            jump_factor: 1.0,
            bounce: 0.0,
            inside: 0,
            offset_kind: 0,
            offset_h: 0.0,
            offset_v: 0.0,
            dynamic_shape: false,
            properties: Vec::new(),
        };
        rc_data::VersionData {
            blocks: vec![block("minecraft:air", 0), block("minecraft:stone", 1), block("minecraft:water", 2)],
            states: vec![
                rc_data::StateInfo { block: 0, ..Default::default() },
                rc_data::StateInfo { block: 1, sound: 1, ..Default::default() },
                rc_data::StateInfo { block: 2, ..Default::default() },
            ],
            sound_types: vec![
                rc_data::SoundType::default(),
                rc_data::SoundType { hit: "minecraft:block.stone.hit".into(), ..Default::default() },
            ],
            ..Default::default()
        }
    }

    /// Cube landing: ceil(32 × width) particles (loop "i < size × 16") of the cube's own kind,
    /// at the feet, on the ring [radius/2, radius] (radius = width). Before: none.
    #[test]
    fn cube_landing_ring() {
        let mut r = crate::level_events::WorldRandom::default();
        for (kind, want, width) in
            [("slime", PKind::ItemSlime, 1.04f32), ("magma_cube", PKind::Flame, 2.04), ("sulfur_cube", PKind::SulfurCubeGoo, 0.5)]
        {
            let e = one(kind, [0.5, 64.0, 0.5]);
            let mut pts = Vec::new();
            cube_landing(&e, width, &mut r, &mut |k, p| {
                assert_eq!(k, want, "{kind}");
                pts.push(p);
            });
            assert_eq!(pts.len(), (width * 32.0).ceil() as usize, "{kind}");
            for p in pts {
                let d = ((p[0] - 0.5).powi(2) + (p[2] - 0.5).powi(2)).sqrt();
                assert!(p[1] == 64.0 && d >= f64::from(width) * 0.5 - 1e-5 && d <= f64::from(width) + 1e-5, "{kind} {p:?}");
            }
        }
    }

    /// Copper golem spin sound according to oxidation (entity data 16).
    #[test]
    fn copper_spin_sound_follows_oxidation() {
        let mut e = one("copper_golem", [0.0; 3]);
        let mut names = Vec::new();
        for v in 0..4 {
            e.meta.insert(16, rc_proto_v777::entity::Meta::Variant(v, 0));
            names.push(copper_spin_sound(&e));
        }
        assert_eq!(
            names,
            [
                "minecraft:entity.copper_golem.spin",
                "minecraft:entity.copper_golem.spin",
                "minecraft:entity.copper_golem_weathered.spin",
                "minecraft:entity.copper_golem_oxidized.spin",
            ]
        );
    }

    /// Digging sniffer (state 5) facing +z: 30 particles of the stone under the head
    /// (block 2.25 in front, 0.65 below the center) at every tick between 35 and 119 ticks of
    /// animation (1.7 s excluded to 6 s excluded), stone hit sound every 10 ticks in that window;
    /// nothing over water. Searching (4): searching sound every 20 ticks. Before: nothing.
    #[test]
    fn sniffer_digging_and_searching_effects() {
        let d = data();
        for (below, want_particles) in [(1u32, true), (2, false)] {
            let mut all = spawn("sniffer", [0.5, 64.0, 0.5]);
            all.tick();
            all.map.get_mut(&7).unwrap().meta.insert(18, rc_proto_v777::entity::Meta::Variant(5, 0));
            let state = |x: i32, y: i32, z: i32| Some(if [x, y, z] == [0, 63, 2] { below } else { 0 });
            let (mut ticks, mut sounds) = (Vec::new(), Vec::new());
            let mut start = 0;
            for _ in 0..200 {
                all.tick();
                let e = &all.map[&7];
                start = crate::entity_render::sniffer_dig_start(e).expect("digging started") as u32;
                let mut n = 0;
                sniffer(7, e, &d, &state, &mut |k, o, p| {
                    assert_eq!((k, o.state), (PKind::Block, 1));
                    assert!((p[0] - 0.5).abs() < 1e-9 && (p[1] - (64.5 - f64::from(0.65f32))).abs() < 1e-9 && (p[2] - 2.5).abs() < 1e-9, "{p:?}");
                    n += 1;
                }, &mut sounds);
                if n > 0 {
                    assert_eq!(n, 30);
                    ticks.push(e.age - start);
                }
            }
            if want_particles {
                assert_eq!(ticks, (35..=119).collect::<Vec<_>>());
                let hits: Vec<u32> = (35..=119).filter(|t| (t + start) % 10 == 0).collect();
                assert_eq!(sounds.len(), hits.len());
                assert!(sounds.iter().all(|s| matches!(s, Sound::Bound(7, n, v, p) if n == "minecraft:block.stone.hit" && *v == 0.5 && *p == 0.5)));
            } else {
                assert!(ticks.is_empty() && sounds.is_empty());
            }
        }
        let mut all = spawn("sniffer", [0.5, 64.0, 0.5]);
        all.map.get_mut(&7).unwrap().meta.insert(18, rc_proto_v777::entity::Meta::Variant(4, 0));
        let mut sounds = Vec::new();
        for _ in 0..100 {
            all.tick();
            sniffer(7, &all.map[&7], &d, &|_, _, _| Some(1), &mut |_, _, _| panic!("particle while searching"), &mut sounds);
        }
        assert_eq!(sounds.len(), 5);
        assert!(sounds.iter().all(|s| matches!(s, Sound::Bound(7, n, ..) if n == "minecraft:entity.sniffer.searching")));
    }

    /// Guardian: 2 bubbles per tick if it swims in water, none out of water or still; with
    /// a target, bubbles aligned on the beam from the eyes to the middle of the target, spaced by
    /// [1.8, 3.5) at the start of the charge. Before: no bubbles.
    #[test]
    fn guardian_swim_and_beam_bubbles() {
        let mut r = crate::level_events::WorldRandom::default();
        let mut e = one("guardian", [0.5, 64.0, 0.5]);
        e.pos_o = e.pos;
        let size = (0.85, 0.85, 0.425);
        let count = |e: &Entity, r: &mut crate::level_events::WorldRandom| {
            let mut n = 0;
            guardian(e, size, None, r, &mut |_| n += 1);
            n
        };
        assert_eq!(count(&e, &mut r), 0);
        e.meta.insert(16, rc_proto_v777::entity::Meta::Bool(true));
        assert_eq!(count(&e, &mut r), 0);
        e.in_water = true;
        assert_eq!(count(&e, &mut r), 2);
        e.meta.insert(16, rc_proto_v777::entity::Meta::Bool(false));
        let target = ([0.5, 64.0, 20.5], 1.8);
        let mut pts = Vec::new();
        guardian(&e, size, Some(target), &mut r, &mut |p| pts.push(p));
        assert!((5..=13).contains(&pts.len()), "{}", pts.len());
        let eye = 64.425;
        let dz = 20.0;
        let dy = 64.9 - eye;
        let mut last = None;
        for p in &pts {
            assert!((p[0] - 0.5).abs() < 1e-9);
            let t = (p[2] - 0.5) / dz;
            assert!(((p[1] - eye) - dy * t).abs() < 1e-9, "{p:?}");
            let dist = (p[2] - 0.5) * (dz * dz + dy * dy).sqrt() / dz;
            match last {
                None => assert!((1.8 - 1e-6..4.5).contains(&dist), "{dist}"),
                Some(l) => assert!((1.8 - 1e-6..3.5).contains(&(dist - l)), "{dist} {l}"),
            }
            last = Some(dist);
        }
    }
}

#[cfg(test)]
mod tests {
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
