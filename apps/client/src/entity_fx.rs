//! Particles emitted by entities on each client tick: persistent effect cloud
//! (`area_effect_cloud`: dragon breath, lingering potions) and wither (head smoke,
//! half-health armor effect, summoning aura), and local monster effects (`mob_fx`:
//! particles and sounds that the game client produces on its own, the server sends none).
//!
//! Cloud metadata: 8 = radius (3 by default), 9 = waiting, 10 = particle (white entity effect
//! by default). Wither: 9 = health, 19 = invulnerability ticks.

use rc_proto_v777::entity::{Meta, Particle};

use crate::entities::Entity;
use crate::level_events::WorldRandom;
use crate::particles::{Kind as PKind, Options as POptions};
use crate::sky_mesh::{table_cos, table_sin};

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
        let crate::entities::Entities { map, dims, .. } = &mut self.entities;
        let (ps, r) = (&mut self.particles, &mut self.world_random);
        let view =
            crate::environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
        let data = &self.data;
        let look = |p: [i32; 3]| {
            let s = view.state(p[0], p[1], p[2]);
            (s, data.block_of(s).map_or(Shape::Air, |b| shape_of(&b.name)))
        };
        let mut sounds = Vec::new();
        for e in map.values_mut() {
            if matches!(
                e.kind.as_str(),
                "blaze"
                    | "breeze"
                    | "enderman"
                    | "endermite"
                    | "evoker"
                    | "illusioner"
                    | "phantom"
                    | "ravager"
                    | "warden"
            ) {
                let size = e.size(dims);
                mob_fx(
                    e,
                    size,
                    r,
                    &look,
                    &mut |k, o, p, v| {
                        ps.spawn(k, o, p, v);
                    },
                    &mut sounds,
                );
                continue;
            }
            let e = &*e;
            match e.kind.as_str() {
                "area_effect_cloud" => cloud(ps, r, e),
                "wither" => wither(ps, r, e),
                "ominous_item_spawner" if game_time % 5 == 0 => ominous_spawner(ps, r, e),
                // Powered furnace minecart: big smoke one time in four.
                "furnace_minecart" if e.meta_bool(13) && r.f() < 0.25 => {
                    ps.spawn(PKind::LargeSmoke, &POptions::default(), [e.pos[0], e.pos[1] + 0.8, e.pos[2]], [0.0; 3]);
                }
                // Bee carrying nectar (flag 8 of data 18): 5% per tick, 1 or 2
                // nectar drops at mid-height, within ±0.3 in x and z.
                "bee" if e.meta_byte(18) & 8 != 0 && r.f() < 0.05 => {
                    let half = f64::from(e.size(dims).h) * 0.5;
                    for _ in 0..1 + (r.f() * 2.0) as i32 {
                        let x = e.pos[0] - 0.3 + f64::from(r.f()) * 0.6;
                        let z = e.pos[2] - 0.3 + f64::from(r.f()) * 0.6;
                        ps.spawn(PKind::FallingNectar, &POptions::default(), [x, e.pos[1] + half, z], [0.0; 3]);
                    }
                }
                _ => {}
            }
        }
        if !sounds.is_empty() {
            self.play_fx_sounds(&sounds);
        }
        self.water_splashes();
    }

    /// Plays client effect sounds (hostile category); a sound that follows its entity is handed
    /// to the entity sound tracking (`tick_audio`: position, cut if the entity disappears or goes
    /// quiet).
    pub(crate) fn play_fx_sounds(&mut self, sounds: &[FxSound]) {
        let Some(a) = self.audio.get() else { return };
        for s in sounds {
            let h = a.play(s.name, rc_audio::Category::Hostile, Some(s.at), s.volume, s.pitch, None);
            if let (Some(h), Some(id)) = (h, s.follow) {
                self.entity_sounds.push((h, id));
            }
        }
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
            let s = e.size(dims);
            let (w, h) = (f64::from(s.w), f64::from(s.h));
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

/// Sound of an entity client effect (hostile category).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct FxSound {
    pub name: &'static str,
    pub at: [f64; 3],
    pub volume: f32,
    pub pitch: f32,
    /// Entity the sound follows (id); otherwise the sound is placed at `at`.
    pub follow: Option<i32>,
}

/// Render shape of a block state, as seen by the ground particles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Shape {
    Air,
    /// No model rendering (fluids, barrier…): no ground particles.
    Invisible,
    Model,
}

/// Render shape of a block: the three airs; no rendering: barrier, light, structure void,
/// water, lava, bubble column, End portal and End gateway, moving piston.
fn shape_of(name: &str) -> Shape {
    match name {
        "minecraft:air" | "minecraft:cave_air" | "minecraft:void_air" => Shape::Air,
        "minecraft:barrier"
        | "minecraft:light"
        | "minecraft:structure_void"
        | "minecraft:water"
        | "minecraft:lava"
        | "minecraft:bubble_column"
        | "minecraft:end_portal"
        | "minecraft:end_gateway"
        | "minecraft:moving_piston" => Shape::Invisible,
        _ => Shape::Model,
    }
}

/// Emission of a particle (kind, options, position, velocity).
type Spawn<'a> = dyn FnMut(PKind, &POptions, [f64; 3], [f64; 3]) + 'a;
/// State and render shape of the block at a position.
type Look<'a> = dyn Fn([i32; 3]) -> (u32, Shape) + 'a;

/// Uniform integer in [0, n) drawn from the client random.
fn int(r: &mut WorldRandom, n: i32) -> i32 {
    ((r.f() * n as f32) as i32).min(n - 1)
}

fn unit(r: &mut WorldRandom) -> f64 {
    f64::from(r.f())
}

/// Random point in the box (feet `pos`, width `w`, height `h`): x and z within ± w/2,
/// y over the height, plus `dy`. Draws in the order x, y, z.
fn random_in(pos: [f64; 3], w: f64, h: f64, r: &mut WorldRandom, dy: f64) -> [f64; 3] {
    let x = pos[0] + w * (2.0 * unit(r) - 1.0) * 0.5;
    let y = pos[1] + h * unit(r) + dy;
    let z = pos[2] + w * (2.0 * unit(r) - 1.0) * 0.5;
    [x, y, z]
}

/// Enderman stare sound: allowed at most once per 400 ticks, timer advanced even when it is
/// silent (flag 4). True if the sound must be played.
pub(crate) fn stare_ready(e: &mut Entity) -> bool {
    let age = i64::from(e.age);
    if age < i64::from(e.fx[0]) {
        return false;
    }
    e.fx[0] = i32::try_from(age + 400).unwrap_or(i32::MAX);
    !e.meta_bool(4)
}

/// Spell colors of the spellcasting illagers by id (data 17); out of range: 0.
const SPELL_RGB: [[f32; 3]; 6] =
    [[0.0, 0.0, 0.0], [0.7, 0.7, 0.8], [0.4, 0.3, 0.35], [0.7, 0.5, 0.2], [0.3, 0.3, 0.8], [0.1, 0.1, 0.2]];

/// Client effects per tick of a monster (blaze, breeze, enderman, endermite, spellcasting
/// illagers, phantom, ravager, warden), after its tick: particles through `spawn`, sounds in
/// `sounds`. `size`: current box (scale included).
fn mob_fx(
    e: &mut Entity,
    size: crate::entities::Size,
    r: &mut WorldRandom,
    look: &Look<'_>,
    spawn: &mut Spawn<'_>,
    sounds: &mut Vec<FxSound>,
) {
    let (w, h) = (f64::from(size.w), f64::from(size.h));
    let silent = e.meta_bool(4);
    let none = POptions::default();
    match e.kind.as_str() {
        // Blaze: one time in 24 the ember sound, placed at +0.5 on the three axes (offset of the
        // game), volume 1 + r, pitch 0.3 + 0.7 r; 2 large smokes in the box.
        "blaze" => {
            if int(r, 24) == 0 && !silent {
                let volume = 1.0 + r.f();
                let pitch = r.f() * 0.7 + 0.3;
                let at = [e.pos[0] + 0.5, e.pos[1] + 0.5, e.pos[2] + 0.5];
                sounds.push(FxSound { name: "minecraft:entity.blaze.burn", at, volume, pitch, follow: None });
            }
            for _ in 0..2 {
                let p = random_in(e.pos, w, h, r, 0.0);
                spawn(PKind::LargeSmoke, &none, p, [0.0; 3]);
            }
        }
        // Enderman (0.25 lower) and endermite: 2 portals, velocity (±1, −[0, 1), ±1).
        "enderman" | "endermite" => {
            let dy = if e.kind == "enderman" { -0.25 } else { 0.0 };
            for _ in 0..2 {
                let p = random_in(e.pos, w, h, r, dy);
                let v = [(unit(r) - 0.5) * 2.0, -unit(r), (unit(r) - 0.5) * 2.0];
                spawn(PKind::Portal, &none, p, v);
            }
        }
        "phantom" => phantom(e, size, r, spawn, sounds),
        // Stunned ravager (counted down on this tick, including the one where it falls back to
        // 0): one time in 6 a grey effect in front of the head.
        "ravager" => {
            let was = e.fx[0];
            e.fx[0] = e.timers[1];
            if e.death_time == 0 && (was > 0 || e.timers[1] > 0) && int(r, 6) == 0 {
                let a = f64::from(e.body * (std::f32::consts::PI / 180.0));
                let x = e.pos[0] - w * a.sin() + (unit(r) * 0.6 - 0.3);
                let y = e.pos[1] + h - 0.3;
                let z = e.pos[2] + w * a.cos() + (unit(r) * 0.6 - 0.3);
                let rgb = [0.498_039_22, 0.513_725_5, 0.572_549_05];
                let o = POptions { argb: argb(rgb[0], rgb[1], rgb[2]), rgb, ..none };
                spawn(PKind::EntityEffect, &o, [x, y, z], [0.0; 3]);
            }
        }
        "breeze" => breeze(e, r, look, spawn, sounds),
        // Illager casting (data 17 > 0): an effect in the spell color at each hand.
        "evoker" | "illusioner" => {
            let spell = e.meta_byte(17);
            if spell <= 0 {
                return;
            }
            let rgb = SPELL_RGB.get(spell as usize).copied().unwrap_or([0.0; 3]);
            let o = POptions { argb: argb(rgb[0], rgb[1], rgb[2]), rgb, ..none };
            let a = e.body * (std::f32::consts::PI / 180.0) + table_cos(e.age as f32 * 0.6662) * 0.25;
            let (c, s) = (f64::from(table_cos(a)), f64::from(table_sin(a)));
            let (dist, up) = (0.6 * f64::from(e.scale), 1.8 * f64::from(e.scale));
            spawn(PKind::EntityEffect, &o, [e.pos[0] + c * dist, e.pos[1] + up, e.pos[2] + s * dist], [0.0; 3]);
            spawn(PKind::EntityEffect, &o, [e.pos[0] - c * dist, e.pos[1] + up, e.pos[2] - s * dist], [0.0; 3]);
        }
        // Warden: heart sound on the beat (placed, volume 5, pitch (r − r) × 0.2 + 1); while
        // digging or emerging, 30 particles of the supporting block within ± 0.7 for 4.5 s.
        "warden" => {
            if crate::entity_render::warden_beat(e) && !silent {
                let pitch = (r.f() - r.f()) * 0.2 + 1.0;
                sounds.push(FxSound {
                    name: "minecraft:entity.warden.heartbeat",
                    at: e.pos,
                    volume: 5.0,
                    pitch,
                    follow: None,
                });
            }
            if crate::entity_render::warden_dig_ticks(e).is_some_and(|t| ((t * 50.0) as i64) < 4500) {
                let (state, shape) = look(on_pos(e.pos));
                if shape == Shape::Model {
                    let o = POptions { state, ..none };
                    for _ in 0..30 {
                        let x = e.pos[0] + f64::from(r.f() * 1.4 - 0.7);
                        let z = e.pos[2] + f64::from(r.f() * 1.4 - 0.7);
                        spawn(PKind::Block, &o, [x, e.pos[1], z], [0.0; 3]);
                    }
                }
            }
        }
        _ => {}
    }
}

/// Supporting block: under the feet, within 1e-5 (without the game's remembered supporting block).
fn on_pos(p: [f64; 3]) -> [i32; 3] {
    [p[0].floor() as i32, (p[1] - f64::from(1.0e-5f32)).floor() as i32, p[2].floor() as i32]
}

/// Ground block of the breeze: the one it is in if it is not air, otherwise the supporting block.
fn breeze_ground(pos: [f64; 3], look: &Look<'_>) -> (u32, Shape) {
    let inside = look([pos[0].floor() as i32, pos[1].floor() as i32, pos[2].floor() as i32]);
    if inside.1 == Shape::Air { look(on_pos(pos)) } else { inside }
}

/// Breeze: ground particles by pose (1 when standing, shooting or inhaling, 20 when sliding,
/// none as a passenger or on a block with no rendering), jump trail (3 per tick for 5 ticks, at
/// the position + the velocity + 0.1), whistle every 1 to 80 ticks, local ambient sound at the
/// mob timer (draw out of 1000 below the counter, reset to −80). Counters: `fx` 0 whistle,
/// 1 trail, 2 ambient.
fn breeze(e: &mut Entity, r: &mut WorldRandom, look: &Look<'_>, spawn: &mut Spawn<'_>, sounds: &mut Vec<FxSound>) {
    let ground = |e: &Entity, n: i32, spawn: &mut Spawn<'_>| {
        if e.vehicle.is_some() {
            return;
        }
        let (state, shape) = breeze_ground(e.pos, look);
        if shape == Shape::Model {
            let o = POptions { state, ..POptions::default() };
            for _ in 0..n {
                spawn(PKind::Block, &o, e.pos, [0.0; 3]);
            }
        }
    };
    let silent = e.meta_bool(4);
    match e.pose() {
        // Standing, shooting, inhaling: 1 + a draw out of 1 (always 1).
        0 | 16 | 17 => {
            e.fx[1] = 0;
            let n = 1 + int(r, 1);
            ground(e, n, spawn);
        }
        15 => ground(e, 20, spawn),
        6 => {
            e.fx[1] += 1;
            if e.fx[1] <= 5 {
                let (state, _) = breeze_ground(e.pos, look);
                let o = POptions { state, ..POptions::default() };
                let v = e.velocity;
                let p = [e.pos[0] + v[0], e.pos[1] + v[1] + f64::from(0.1f32), e.pos[2] + v[2]];
                for _ in 0..3 {
                    spawn(PKind::Block, &o, p, [0.0; 3]);
                }
            }
        }
        _ => {}
    }
    e.fx[0] = if e.fx[0] == 0 { 1 + int(r, 80) } else { e.fx[0] - 1 };
    if e.fx[0] == 0 {
        let pitch = 0.7 + 0.4 * r.f();
        let volume = 0.8 + 0.2 * r.f();
        if !silent {
            sounds.push(FxSound { name: "minecraft:entity.breeze.whirl", at: e.pos, volume, pitch, follow: Some(e.id) });
        }
    }
    // Ambient sound: played locally (the server sends none); no target on the client side.
    if e.death_time == 0 {
        let t = e.fx[2];
        e.fx[2] += 1;
        if int(r, 1000) < t {
            e.fx[2] = -80;
            if !silent {
                let name =
                    if e.on_ground { "minecraft:entity.breeze.idle_ground" } else { "minecraft:entity.breeze.idle_air" };
                sounds.push(FxSound { name, at: e.pos, volume: 1.0, pitch: 1.0, follow: Some(e.id) });
            }
        }
    }
}

/// Phantom: flap sound when the wing cosine goes from positive to negative or zero (followed,
/// volume and pitch 0.95 + 0.05 r); 2 mycelium particles at the wing tips on each tick.
fn phantom(
    e: &Entity,
    size: crate::entities::Size,
    r: &mut WorldRandom,
    spawn: &mut Spawn<'_>,
    sounds: &mut Vec<FxSound>,
) {
    const DEG: f32 = std::f32::consts::PI / 180.0;
    let t = e.id.wrapping_mul(3).wrapping_add(e.age as i32);
    let wing = |t: i32| table_cos(t as f32 * 7.448_451 * DEG + std::f32::consts::PI);
    let (anim, next) = (wing(t), wing(t.wrapping_add(1)));
    if anim > 0.0 && next <= 0.0 {
        let volume = 0.95 + r.f() * 0.05;
        let pitch = 0.95 + r.f() * 0.05;
        if !e.meta_bool(4) {
            sounds.push(FxSound { name: "minecraft:entity.phantom.flap", at: e.pos, volume, pitch, follow: Some(e.id) });
        }
    }
    let width = size.w * 1.48;
    let (c, s) = (f64::from(table_cos(e.yaw * DEG) * width), f64::from(table_sin(e.yaw * DEG) * width));
    let y = e.pos[1] + f64::from((0.3 + anim * 0.45) * size.h * 2.5);
    let none = POptions::default();
    spawn(PKind::Mycelium, &none, [e.pos[0] + c, y, e.pos[2] + s], [0.0; 3]);
    spawn(PKind::Mycelium, &none, [e.pos[0] - c, y, e.pos[2] - s], [0.0; 3]);
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

#[cfg(test)]
mod mob_tests {
    use super::*;
    use crate::entities::Entities;
    use rc_client::EntityEvent;
    use rc_proto_v777::entity::AddEntity;

    /// An entity `kind` (id 7) with feet at (0.5, 64, 0.5), box `dims`.
    fn spawn(kind: &str, dims: [f32; 4]) -> Entities {
        let mut all =
            Entities { type_names: vec![format!("minecraft:{kind}")], dims: vec![dims], ..Default::default() };
        let add = AddEntity {
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
        all.apply(EntityEvent::Add(Box::new(add)));
        all
    }

    /// Stone floor (state 1) below y = 64, air above.
    fn stone(p: [i32; 3]) -> (u32, Shape) {
        if p[1] < 64 { (1, Shape::Model) } else { (0, Shape::Air) }
    }

    type Out = (Vec<(PKind, POptions, [f64; 3])>, Vec<FxSound>);

    /// Effects of one tick of entity 7 (after its tick if `tick`).
    fn run(all: &mut Entities, r: &mut WorldRandom, look: &Look<'_>, tick: bool) -> Out {
        if tick {
            all.tick();
        }
        let crate::entities::Entities { map, dims, .. } = all;
        let e = map.get_mut(&7).unwrap();
        let size = e.size(dims);
        let (mut ps, mut ss) = (Vec::new(), Vec::new());
        mob_fx(e, size, r, look, &mut |k, o, p, _| ps.push((k, *o, p)), &mut ss);
        (ps, ss)
    }

    /// Blaze: 2 large smokes per tick in the box, ember sound 1 tick in 24 placed at
    /// +0.5 (volume 1 to 2, pitch 0.3 to 1), mute if it is silent.
    #[test]
    fn blaze_smoke_and_burn_sound() {
        let mut all = spawn("blaze", [0.6, 1.8, 1.53, 1.8]);
        let mut r = WorldRandom::default();
        let (mut smoke, mut burn) = (0, Vec::new());
        for _ in 0..4800 {
            let (ps, ss) = run(&mut all, &mut r, &stone, false);
            for (k, _, p) in &ps {
                assert_eq!(*k, PKind::LargeSmoke);
                assert!((p[0] - 0.5).abs() <= 0.3 && (64.0..=65.8).contains(&p[1]), "{p:?}");
            }
            smoke += ps.len();
            burn.extend(ss);
        }
        assert_eq!(smoke, 9600);
        assert!((150..250).contains(&burn.len()), "{}", burn.len());
        for s in &burn {
            assert_eq!((s.name, s.at, s.follow), ("minecraft:entity.blaze.burn", [1.0, 64.5, 1.0], None));
            assert!((1.0..2.0).contains(&s.volume) && (0.3..1.0).contains(&s.pitch), "{s:?}");
        }
        all.map.get_mut(&7).unwrap().meta.insert(4, Meta::Bool(true));
        assert!((0..500).all(|_| run(&mut all, &mut r, &stone, false).1.is_empty()));
    }

    /// Portals: spread within ± width / 2 (0.3, not the height), enderman 0.25 lower,
    /// endermite over its height.
    #[test]
    fn enderman_and_endermite_portals() {
        for (kind, dims, lo) in [("enderman", [0.6, 2.9, 2.55, 2.9], 63.75), ("endermite", [0.4, 0.3, 0.13, 0.3], 64.0)]
        {
            let mut all = spawn(kind, dims);
            let mut r = WorldRandom::default();
            let mut max_dx = 0.0f64;
            for _ in 0..500 {
                let (ps, _) = run(&mut all, &mut r, &stone, false);
                assert_eq!(ps.len(), 2);
                for (k, _, p) in ps {
                    assert_eq!(k, PKind::Portal);
                    max_dx = max_dx.max((p[0] - 0.5).abs()).max((p[2] - 0.5).abs());
                    assert!(p[1] >= lo && p[1] <= lo + f64::from(dims[1]), "{kind} {p:?}");
                }
            }
            let half = f64::from(dims[0]) / 2.0;
            assert!(max_dx <= half && max_dx > half * 0.9, "{kind} {max_dx}");
        }
    }

    /// Enderman stare: data 17 received when 18 is already true sounds at the eyes, at most
    /// once per 400 ticks; received together at spawn: nothing; the calm-down (17 then 18 to
    /// false) also sounds (packet order, like the game).
    #[test]
    fn enderman_stare_sound_follows_packet_order() {
        let data = |all: &mut Entities, v: Vec<(u8, Meta)>| {
            all.apply(EntityEvent::Data { id: 7, values: v });
            std::mem::take(&mut all.pending_sounds)
        };
        let mut all = spawn("enderman", [0.6, 2.9, 2.55, 2.9]);
        assert!(data(&mut all, vec![(17, Meta::Bool(true)), (18, Meta::Bool(true))]).is_empty());
        let s = data(&mut all, vec![(17, Meta::Bool(true))]);
        assert_eq!(s.len(), 1);
        let eye = 64.0 + f64::from(2.55f32);
        assert_eq!(
            (s[0].name, s[0].at, s[0].volume, s[0].pitch, s[0].follow),
            ("minecraft:entity.enderman.stare", [0.5, eye, 0.5], 2.5, 1.0, None)
        );
        for _ in 0..399 {
            all.tick();
        }
        assert!(data(&mut all, vec![(17, Meta::Bool(true))]).is_empty(), "less than 400 ticks");
        all.tick();
        assert_eq!(data(&mut all, vec![(17, Meta::Bool(false)), (18, Meta::Bool(false))]).len(), 1);
        for _ in 0..400 {
            all.tick();
        }
        assert!(data(&mut all, vec![(17, Meta::Bool(true))]).is_empty(), "18 fell back to false");
    }

    /// Phantom: one flap (followed sound) per crossing of the wing cosine below zero, i.e. every
    /// ~48.3 ticks (period of 360 / 7.448451); 2 mycelium particles per tick at width × 1.48 on
    /// either side.
    #[test]
    fn phantom_flap_sound_and_wing_tips() {
        let mut all = spawn("phantom", [0.9, 0.5, 0.175, 0.5]);
        let mut r = WorldRandom::default();
        let mut flaps = 0;
        for _ in 0..2417 {
            let (ps, ss) = run(&mut all, &mut r, &stone, true);
            assert_eq!(ps.len(), 2);
            let d = ((ps[0].2[0] - ps[1].2[0]).powi(2) + (ps[0].2[2] - ps[1].2[2]).powi(2)).sqrt();
            assert!((d - 2.0 * 0.9 * 1.48).abs() < 1e-3 && ps[0].0 == PKind::Mycelium, "{d}");
            for s in ss {
                assert_eq!((s.name, s.follow), ("minecraft:entity.phantom.flap", Some(7)));
                assert!((0.95..1.0).contains(&s.volume) && (0.95..1.0).contains(&s.pitch));
                flaps += 1;
            }
        }
        assert!((49..=51).contains(&flaps), "{flaps}");
    }

    /// Stunned ravager (event 39): 1 draw in 6 on each of the 40 counted-down ticks,
    /// grey effect in front of the head, nothing outside the stun.
    #[test]
    fn ravager_stun_particles() {
        let mut all = spawn("ravager", [1.95, 2.2, 2.1, 2.2]);
        let mut r = WorldRandom::default();
        let (trials, mut n) = (20_000, 0usize);
        for _ in 0..trials {
            all.apply(EntityEvent::Event { id: 7, event: 39 });
            for t in 0..45 {
                let (ps, _) = run(&mut all, &mut r, &stone, true);
                assert!(t < 40 || ps.is_empty(), "after the stun");
                for (k, o, p) in &ps {
                    assert_eq!((*k, o.argb), (PKind::EntityEffect, argb(0.498_039_22, 0.513_725_5, 0.572_549_05)));
                    assert!((p[1] - 65.9).abs() < 1e-6 && (p[2] - 0.5 - 1.95).abs() <= 0.3, "{p:?}");
                }
                n += ps.len();
            }
        }
        let mean = n as f64 / f64::from(trials);
        assert!((mean - 40.0 / 6.0).abs() < 0.06, "{mean}");
    }

    /// Breeze: ground particles by pose, nothing on water or in the air, 5-tick jump trail,
    /// followed whistle and ambient sound.
    #[test]
    fn breeze_ground_particles_and_sounds() {
        let mut all = spawn("breeze", [0.6, 1.77, 1.3452, 1.77]);
        let mut r = WorldRandom::default();
        let pose = |all: &mut Entities, p: i32| {
            all.map.get_mut(&7).unwrap().meta.insert(6, Meta::Pose(p));
        };
        let count = |all: &mut Entities, r: &mut WorldRandom, look: &Look<'_>| run(all, r, look, false).0.len();
        assert_eq!(count(&mut all, &mut r, &stone), 1);
        pose(&mut all, 15);
        assert_eq!(count(&mut all, &mut r, &stone), 20);
        let water = |p: [i32; 3]| if p[1] < 64 { (2, Shape::Invisible) } else { (0, Shape::Air) };
        assert_eq!(count(&mut all, &mut r, &water), 0);
        assert_eq!(count(&mut all, &mut r, &|_| (0, Shape::Air)), 0);
        pose(&mut all, 6);
        let trail: Vec<usize> = (0..8).map(|_| count(&mut all, &mut r, &stone)).collect();
        assert_eq!(trail, [3, 3, 3, 3, 3, 0, 0, 0]);
        pose(&mut all, 0);
        let (mut whirl, mut idle) = (0, 0);
        for _ in 0..8000 {
            for s in run(&mut all, &mut r, &stone, false).1 {
                assert_eq!(s.follow, Some(7));
                match s.name {
                    "minecraft:entity.breeze.whirl" => {
                        assert!((0.7..1.1).contains(&s.pitch) && (0.8..1.0).contains(&s.volume));
                        whirl += 1;
                    }
                    "minecraft:entity.breeze.idle_air" => idle += 1,
                    n => panic!("{n}"),
                }
            }
        }
        assert!((170..230).contains(&whirl), "{whirl}");
        assert!((40..120).contains(&idle), "{idle}");
    }

    /// Spellcasting illager: an effect in the spell color at each hand (0.6 horizontally,
    /// 1.8 high); spell out of range: black; no spell: nothing.
    #[test]
    fn spellcaster_hand_particles() {
        let mut all = spawn("evoker", [0.6, 1.95, 1.6575, 1.95]);
        let mut r = WorldRandom::default();
        assert!(run(&mut all, &mut r, &stone, false).0.is_empty());
        for (id, rgb) in [(2i8, [0.4, 0.3, 0.35]), (9, [0.0; 3])] {
            all.map.get_mut(&7).unwrap().meta.insert(17, Meta::Byte(id));
            let ps = run(&mut all, &mut r, &stone, false).0;
            assert_eq!(ps.len(), 2);
            for (k, o, p) in ps {
                assert_eq!((k, o.argb), (PKind::EntityEffect, argb(rgb[0], rgb[1], rgb[2])));
                let d = ((p[0] - 0.5).powi(2) + (p[2] - 0.5).powi(2)).sqrt();
                assert!((d - 0.6).abs() < 1e-3 && (p[1] - 65.8).abs() < 1e-9, "{p:?}");
            }
        }
    }

    /// Warden: heart sound on the beats (age 40, 80 without anger), volume 5; while digging,
    /// 30 ground particles per tick for 4.5 s, nothing on a block with no rendering.
    #[test]
    fn warden_heartbeat_and_dig_particles() {
        let mut all = spawn("warden", [0.9, 2.9, 2.465, 2.9]);
        let mut r = WorldRandom::default();
        let mut beats = Vec::new();
        for _ in 0..80 {
            let (ps, ss) = run(&mut all, &mut r, &stone, true);
            assert!(ps.is_empty());
            for s in ss {
                assert_eq!((s.name, s.volume, s.follow), ("minecraft:entity.warden.heartbeat", 5.0, None));
                assert!((0.8..1.2).contains(&s.pitch));
                beats.push(all.map[&7].age);
            }
        }
        assert_eq!(beats, [40, 80]);
        all.map.get_mut(&7).unwrap().meta.insert(6, Meta::Pose(14));
        // Start at the tick of the data (80), particles while (age − start) × 50 < 4,500 ms:
        // 89 ticks.
        let n: Vec<usize> = (0..92).map(|_| run(&mut all, &mut r, &stone, true).0.len()).collect();
        assert!(n[..89].iter().all(|&k| k == 30) && n[89..] == [0, 0, 0], "{n:?}");
        all.map.get_mut(&7).unwrap().meta.insert(6, Meta::Pose(13));
        let water = |p: [i32; 3]| if p[1] < 64 { (2, Shape::Invisible) } else { (0, Shape::Air) };
        assert!(run(&mut all, &mut r, &water, true).0.is_empty());
        assert_eq!(run(&mut all, &mut r, &stone, true).0.len(), 30);
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
