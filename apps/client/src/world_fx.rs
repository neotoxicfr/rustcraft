//! World effects on the client (`docs/specs/particles.md` § 1.4-1.5): particles of the
//! world events (1500-3xxx, apart from breaking and mining already in `interact.rs`), follower
//! emitters of critical hits (`animate` 1 and 2), jukebox (1010/1011: positioned song,
//! "Now Playing" message, nearby parrots dancing), music notification
//! (`musicToast` setting).

use hashbrown::HashMap;

use rc_audio::{Category, SoundHandle, SoundRequest};
use rc_proto::nbt::Nbt;
use rc_text::Component;

use crate::gui::Gui;
use crate::particles::{Kind as PKind, Options as POptions};

/// Song of the `jukebox_song` registry (network order).
struct Song {
    sound: String,
    description: Component,
}

/// Follower emitter: entity, type, ticks lived (duration 3).
struct Tracker {
    entity: i32,
    kind: PKind,
    life: i32,
    /// Duration in ticks: 3 for hits, 30 for the totem.
    lifetime: i32,
}

#[derive(Default)]
pub struct WorldFx {
    songs: Vec<Song>,
    /// Jukebox playing (block position → sound).
    jukebox: HashMap<[i32; 3], SoundHandle>,
    trackers: Vec<Tracker>,
    pub music: MusicToast,
    /// Tutorial (`tutorial.rs`).
    pub tutorial: crate::tutorial::Tutorial,
}

/// Directions in the game's order: down, up, north, south, west, east.
const DIRS: [[i32; 3]; 6] = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];

impl WorldFx {
    /// `jukebox_song` registry: omitted content (known pack) = the game's song, named after
    /// the entry (`jukebox_song.minecraft.<name>`, sound `music_disc.<name>`).
    pub fn set_songs(&mut self, entries: &[(String, Option<Nbt>)]) {
        self.songs = entries
            .iter()
            .map(|(name, nbt)| {
                let (ns, path) = name.split_once(':').unwrap_or(("minecraft", name));
                let fallback = || Song {
                    sound: format!("{ns}:music_disc.{path}"),
                    description: Component::translate(format!("jukebox_song.{ns}.{path}"), vec![]),
                };
                let Some(n) = nbt else { return fallback() };
                let sound = n.get("sound_event").and_then(|s| match s {
                    Nbt::String(s) => Some(s.clone()),
                    c => c.get("sound_id").and_then(Nbt::as_str).map(str::to_owned),
                });
                let description = n.get("description").and_then(rc_text::from_nbt);
                match (sound, description) {
                    (Some(sound), Some(description)) => Song { sound, description },
                    _ => fallback(),
                }
            })
            .collect();
    }
}

/// RGB color (no alpha) of a hue, like the game's conversion: the fraction is not
/// brought back into [0, 1) beyond one turn (saturated colors at the start of the fade, reproduced).
pub fn hsv_rgb(hue: f32, sat: f32, val: f32) -> u32 {
    let h = ((hue * 6.0) as i32).rem_euclid(6);
    let f = hue * 6.0 - h as f32;
    let p = val * (1.0 - sat);
    let q = val * (1.0 - f * sat);
    let t = val * (1.0 - (1.0 - f) * sat);
    let (r, g, b) = match h {
        0 => (val, t, p),
        1 => (q, val, p),
        2 => (p, val, t),
        3 => (p, q, val),
        4 => (t, p, val),
        _ => (val, p, q),
    };
    let c = |v: f32| ((v * 255.0) as i32).clamp(0, 255) as u32;
    c(r) << 16 | c(g) << 8 | c(b)
}

impl crate::App {
    fn fx_r(&mut self) -> f64 {
        f64::from(self.world_random.f())
    }

    fn fx_g(&mut self) -> f64 {
        self.world_random.gaussian()
    }

    fn fx_state(&self, p: [i32; 3]) -> u32 {
        let view =
            crate::environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
        view.state(p[0], p[1], p[2])
    }

    fn fx_block(&self, state: u32) -> &str {
        self.data.block_of(state).map_or("", |b| b.name.strip_prefix("minecraft:").unwrap_or(&b.name))
    }

    /// Top of the block's shape (1 for air), like the "in the block" scatter.
    fn fx_top(&self, state: u32) -> f64 {
        let Some(info) = self.data.state(state) else { return 1.0 };
        if info.has(rc_data::flags::AIR) {
            return 1.0;
        }
        self.data.shape(info.outline).iter().map(|b| b.max[1]).fold(0.0, f64::max)
    }

    fn fx_item(&self, name: &str) -> i32 {
        self.item_names.iter().position(|n| n.strip_prefix("minecraft:").unwrap_or(n) == name).map_or(-1, |i| i as i32)
    }

    fn fx_add(&mut self, kind: PKind, pos: [f64; 3], vel: [f64; 3]) {
        self.particles.spawn(kind, &POptions::default(), pos, vel);
    }

    /// Scatter of `n` particles in/around a block (width `w`, height `h`); without
    /// `floating`, only above a block that is not air.
    fn fx_seed(&mut self, kind: PKind, p: [i32; 3], n: i32, w: f64, h: f64, floating: bool) {
        for _ in 0..n {
            let v = [self.fx_g() * 0.02, self.fx_g() * 0.02, self.fx_g() * 0.02];
            let x = f64::from(p[0]) + 0.5 - w + self.fx_r() * w * 2.0;
            let y = f64::from(p[1]) + self.fx_r() * h;
            let z = f64::from(p[2]) + 0.5 - w + self.fx_r() * w * 2.0;
            if !floating {
                let below = [x.floor() as i32, y.floor() as i32 - 1, z.floor() as i32];
                let s = self.fx_state(below);
                if self.data.state(s).is_none_or(|i| i.has(rc_data::flags::AIR)) {
                    continue;
                }
            }
            self.fx_add(kind, [x, y, z], v);
        }
    }

    /// `k ∈ [lo, hi]` particles per face, at 0.55 from the center (helper H).
    fn fx_faces(&mut self, kind: PKind, p: [i32; 3], lo: i32, hi: i32) {
        let c = p.map(|v| f64::from(v) + 0.5);
        for d in DIRS {
            let k = lo + (self.fx_r() * f64::from(hi - lo + 1)) as i32;
            for _ in 0..k {
                let speed = [self.fx_r() - 0.5, self.fx_r() - 0.5, self.fx_r() - 0.5];
                let mut pos = [0.0; 3];
                let mut vel = [0.0; 3];
                for a in 0..3 {
                    if d[a] == 0 {
                        pos[a] = c[a] + self.fx_r() - 0.5;
                        vel[a] = speed[a];
                    } else {
                        pos[a] = c[a] + f64::from(d[a]) * 0.55;
                    }
                }
                self.fx_add(kind, pos, vel);
            }
        }
    }

    /// Smoke thrown by a dispenser (helper B); `data` = face.
    fn fx_shoot(&mut self, kind: PKind, p: [i32; 3], data: i32) {
        let n = DIRS[data.clamp(0, 5) as usize].map(f64::from);
        for _ in 0..10 {
            let pow = self.fx_r() * 0.2 + 0.01;
            let x = f64::from(p[0]) + n[0] * 0.6 + 0.5 + n[0] * 0.01 + (self.fx_r() - 0.5) * n[2] * 0.5;
            // Spread in y according to the y component itself: like the game.
            let y = f64::from(p[1]) + n[1] * 0.6 + 0.5 + n[1] * 0.01 + (self.fx_r() - 0.5) * n[1] * 0.5;
            let z = f64::from(p[2]) + n[2] * 0.6 + 0.5 + n[2] * 0.01 + (self.fx_r() - 0.5) * n[0] * 0.5;
            let v = [n[0] * pow + self.fx_g() * 0.01, n[1] * pow + self.fx_g() * 0.01, n[2] * pow + self.fx_g() * 0.01];
            self.fx_add(kind, [x, y, z], v);
        }
    }

    /// Potion splash (helper D): vial pieces then 100 spell particles.
    fn fx_splash(&mut self, kind: PKind, p: [i32; 3], data: i32) {
        let c = [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5];
        let o = POptions { item: self.fx_item("splash_potion"), ..Default::default() };
        for _ in 0..8 {
            let v = [self.fx_g() * 0.15, self.fx_r() * 0.2, self.fx_g() * 0.15];
            self.particles.spawn(PKind::Item, &o, c, v);
        }
        let rgb = [(data >> 16) & 0xFF, (data >> 8) & 0xFF, data & 0xFF].map(|v| v as f32 / 255.0);
        for _ in 0..100 {
            let dist = self.fx_r() * 4.0;
            let angle = self.fx_r() * std::f64::consts::TAU;
            let v = [angle.cos() * dist, 0.01 + self.fx_r() * 0.5, angle.sin() * dist];
            let b = 0.75 + self.fx_r() as f32 * 0.25;
            let o = POptions { rgb: rgb.map(|c| c * b), power: dist as f32, ..Default::default() };
            self.particles.spawn(kind, &o, [c[0] + v[0] * 0.1, c[1] + 0.3, c[2] + v[2] * 0.1], v);
        }
    }

    /// Teleportation along a segment (helper E); `half` = half-width (block: 0.5).
    fn fx_teleport(&mut self, p: [i32; 3], data: i32, r: [i32; 3], spread: (f64, f64)) {
        let to = [p[0] + ((data >> 16) & 0xFF) - r[0], p[1] + ((data >> 8) & 0xFF) - r[1], p[2] + (data & 0xFF) - r[2]];
        let lerp = |t: f64, a: i32, b: i32| f64::from(a) + t * f64::from(b - a);
        for _ in 0..128 {
            let t = self.fx_r();
            let v = [(self.fx_r() - 0.5) * 0.2, (self.fx_r() - 0.5) * 0.2, (self.fx_r() - 0.5) * 0.2];
            let pos = if spread.0 == 0.0 {
                [
                    lerp(t, to[0], p[0]) + (self.fx_r() - 0.5) + 0.5,
                    lerp(t, to[1], p[1]) + self.fx_r() - 0.5,
                    lerp(t, to[2], p[2]) + (self.fx_r() - 0.5) + 0.5,
                ]
            } else {
                // Enderman: width 0.6, height 2.9.
                [
                    lerp(t, to[0], p[0]) + (self.fx_r() - 0.5) * spread.0 * 2.0,
                    lerp(t, to[1], p[1]) + self.fx_r() * spread.1,
                    lerp(t, to[2], p[2]) + (self.fx_r() - 0.5) * spread.0 * 2.0,
                ]
            };
            self.fx_add(PKind::Portal, pos, v);
        }
    }

    /// Particles and sounds of the world events not handled elsewhere; `data` = data.
    pub fn fx_level_event(&mut self, kind: i32, p: [i32; 3], data: i32) {
        let b = p.map(f64::from);
        match kind {
            1010 => self.fx_jukebox_play(p, data),
            1011 => self.fx_jukebox_stop_notify(p),
            1500 => {
                // Composter: 10 × `composter` above the contents.
                let s = self.fx_state(p);
                let top = self.fx_center_top(s) + 0.03125;
                for _ in 0..10 {
                    let v = [self.fx_g() * 0.02, self.fx_g() * 0.02, self.fx_g() * 0.02];
                    let x = b[0] + 0.1875 + 0.625 * self.fx_r();
                    let y = b[1] + top + self.fx_r() * (1.0 - top);
                    let z = b[2] + 0.1875 + 0.625 * self.fx_r();
                    self.fx_add(PKind::Composter, [x, y, z], v);
                }
            }
            1501 => {
                for _ in 0..8 {
                    let pos = [b[0] + self.fx_r(), b[1] + 1.2, b[2] + self.fx_r()];
                    self.fx_add(PKind::LargeSmoke, pos, [0.0; 3]);
                }
            }
            1502 => {
                for _ in 0..5 {
                    let pos = [0, 1, 2].map(|a| b[a] + 0.2);
                    let pos = [pos[0] + self.fx_r() * 0.6, pos[1] + self.fx_r() * 0.6, pos[2] + self.fx_r() * 0.6];
                    self.fx_add(PKind::Smoke, pos, [0.0; 3]);
                }
            }
            1503 => {
                for _ in 0..16 {
                    let x = b[0] + (5.0 + self.fx_r() * 6.0) / 16.0;
                    let z = b[2] + (5.0 + self.fx_r() * 6.0) / 16.0;
                    self.fx_add(PKind::Smoke, [x, b[1] + 0.8125, z], [0.0; 3]);
                }
            }
            1505 => self.fx_bone_meal(p, data),
            2000 => self.fx_shoot(PKind::Smoke, p, data),
            2002 => self.fx_splash(PKind::Effect, p, data),
            2007 => self.fx_splash(PKind::InstantEffect, p, data),
            2003 => {
                let c = [b[0] + 0.5, b[1], b[2] + 0.5];
                let o = POptions { item: self.fx_item("ender_eye"), ..Default::default() };
                for _ in 0..8 {
                    let v = [self.fx_g() * 0.15, self.fx_r() * 0.2, self.fx_g() * 0.15];
                    self.particles.spawn(PKind::Item, &o, c, v);
                }
                let mut a = 0.0f64;
                while a < std::f64::consts::TAU {
                    let pos = [c[0] + a.cos() * 5.0, c[1] - 0.4, c[2] + a.sin() * 5.0];
                    self.fx_add(PKind::Portal, pos, [a.cos() * -5.0, 0.0, a.sin() * -5.0]);
                    self.fx_add(PKind::Portal, pos, [a.cos() * -7.0, 0.0, a.sin() * -7.0]);
                    a += std::f64::consts::PI / 20.0;
                }
            }
            2004 => {
                for _ in 0..20 {
                    let pos = [0, 1, 2].map(|a| b[a] + 0.5);
                    let pos = [
                        pos[0] + (self.fx_r() - 0.5) * 2.0,
                        pos[1] + (self.fx_r() - 0.5) * 2.0,
                        pos[2] + (self.fx_r() - 0.5) * 2.0,
                    ];
                    self.fx_add(PKind::Smoke, pos, [0.0; 3]);
                    self.fx_add(PKind::Flame, pos, [0.0; 3]);
                }
            }
            2008 => self.fx_add(PKind::Explosion, b.map(|v| v + 0.5), [0.0; 3]),
            2009 => {
                for _ in 0..8 {
                    let pos = [b[0] + self.fx_r(), b[1] + 1.2, b[2] + self.fx_r()];
                    self.fx_add(PKind::Cloud, pos, [0.0; 3]);
                }
            }
            2011 | 2012 => {
                let top = self.fx_top(self.fx_state(p));
                self.fx_seed(PKind::HappyVillager, p, data, 0.5, top, true);
            }
            2013 => {
                let state = self.fx_state(p);
                let o = POptions { state, ..Default::default() };
                let c = [b[0] + 0.5, b[1] + 1.0, b[2] + 0.5];
                let n = data as f32;
                let mut i = 0;
                while (i as f32) < n / 3.0 {
                    let pos = [c[0] + self.fx_g() / 2.0, c[1], c[2] + self.fx_g() / 2.0];
                    let v = [self.fx_g() * 0.2, self.fx_g() * 0.2, self.fx_g() * 0.2];
                    self.particles.spawn(PKind::DustPillar, &o, pos, v);
                    i += 1;
                }
                i = 0;
                while (i as f32) < n / 1.5 {
                    let k = f64::from(i);
                    let pos =
                        [c[0] + 3.5 * k.cos() + self.fx_g() / 2.0, c[1], c[2] + 3.5 * k.sin() + self.fx_g() / 2.0];
                    let v = [self.fx_g() * 0.05, self.fx_g() * 0.05, self.fx_g() * 0.05];
                    self.particles.spawn(PKind::DustPillar, &o, pos, v);
                    i += 1;
                }
            }
            2015 => self.fx_teleport(p, data, [16, 8, 16], (0.0, 0.0)),
            2016 => self.fx_teleport(p, data, [8, 8, 8], (0.0, 0.0)),
            2017 => self.fx_teleport(p, data, [127, 127, 127], (0.0, 0.0)),
            2018 => self.fx_teleport(p, data, [127, 127, 127], (0.6, 2.9)),
            3000 => {
                let o = POptions { force: true, ..Default::default() };
                self.particles.spawn(PKind::ExplosionEmitter, &o, b.map(|v| v + 0.5), [0.0; 3]);
                let pitch = (1.0 + (self.world_random.f() - self.world_random.f()) * 0.2) * 0.7;
                self.fx_sound("minecraft:block.end_gateway.spawn", Category::Block, p, 10.0, pitch);
            }
            3001 => {
                let pitch = 0.8 + self.world_random.f() * 0.3;
                self.fx_sound("minecraft:entity.ender_dragon.growl", Category::Hostile, p, 64.0, pitch);
            }
            3002 if (0..3).contains(&data) => {
                let axis = data as usize;
                let c = b.map(|v| v + 0.5);
                let n = 10 + (self.fx_r() * 10.0) as i32;
                for _ in 0..n {
                    let mut pos = [0.0; 3];
                    let mut vel = [0.0; 3];
                    for a in 0..3 {
                        pos[a] = c[a] + (self.fx_r() * 2.0 - 1.0) * if a == axis { 0.5 } else { 0.125 };
                    }
                    vel[axis] = self.fx_r() * 2.0 - 1.0;
                    self.fx_add(PKind::ElectricSpark, pos, vel);
                }
            }
            3002 => self.fx_faces(PKind::ElectricSpark, p, 3, 5),
            3003 => self.fx_faces(PKind::WaxOn, p, 3, 5),
            3004 => self.fx_faces(PKind::WaxOff, p, 3, 5),
            3005 => self.fx_faces(PKind::Scrape, p, 3, 5),
            3006 => {
                let count = data >> 6;
                if count > 0 {
                    let r = self.world_random.f();
                    if r < 0.3 + count as f32 * 0.1 {
                        let v = 0.15 + 0.02 * (count * count) as f32 * self.world_random.f();
                        let pitch = 0.4 + 0.3 * count as f32 * self.world_random.f();
                        self.fx_sound("minecraft:block.sculk.charge", Category::Block, p, v, pitch);
                    }
                    // Charges on the faces: all six of a full block (data 0), otherwise those of the
                    // mask; each face receives 0 to `count` particles.
                    let mask = (data & 63) as u8;
                    for (i, d) in DIRS.iter().enumerate() {
                        let (roll, factor) = if mask == 0 {
                            (if i == 0 { std::f32::consts::PI } else { 0.0 }, if d[1] != 0 { 0.65 } else { 0.57 })
                        } else if mask & (1 << i) != 0 {
                            (if i == 1 { std::f32::consts::PI } else { 0.0 }, 0.35)
                        } else {
                            continue;
                        };
                        let n = (self.fx_r() * f64::from(count + 1)) as i32;
                        for _ in 0..n {
                            let speed = [
                                (self.fx_r() * 2.0 - 1.0) * 0.005,
                                (self.fx_r() * 2.0 - 1.0) * 0.005,
                                (self.fx_r() * 2.0 - 1.0) * 0.005,
                            ];
                            let mut pos = [0.0; 3];
                            let mut vel = [0.0; 3];
                            for a in 0..3 {
                                pos[a] =
                                    b[a] + 0.5 + if d[a] == 0 { self.fx_r() - 0.5 } else { f64::from(d[a]) * factor };
                                vel[a] = if d[a] == 0 { speed[a] } else { 0.0 };
                            }
                            let o = POptions { scale: roll, ..Default::default() };
                            self.particles.spawn(PKind::SculkCharge, &o, pos, vel);
                        }
                    }
                } else {
                    self.fx_sound("minecraft:block.sculk.charge", Category::Block, p, 1.0, 1.0);
                    let s = self.fx_state(p);
                    let solid = self.data.state(s).is_some_and(|i| i.sturdy == 0x3f);
                    let (n, spread) = if solid { (40, 0.45) } else { (20, 0.25) };
                    for _ in 0..n {
                        let v = [self.fx_r() * 2.0 - 1.0, self.fx_r() * 2.0 - 1.0, self.fx_r() * 2.0 - 1.0];
                        let pos = [0, 1, 2].map(|a| b[a] + 0.5 + v[a] * spread);
                        self.fx_add(PKind::SculkChargePop, pos, v.map(|c| c * 0.07));
                    }
                }
            }
            2006 => {
                for _ in 0..200 {
                    let dist = self.fx_r() as f32 * 4.0;
                    let angle = self.fx_r() * std::f64::consts::TAU;
                    let v = [angle.cos() * f64::from(dist), 0.01 + self.fx_r() * 0.5, angle.sin() * f64::from(dist)];
                    let o = POptions { power: dist, ..Default::default() };
                    self.particles.spawn(
                        PKind::DragonBreath,
                        &o,
                        [b[0] + v[0] * 0.1, b[1] + 0.3, b[2] + v[2] * 0.1],
                        v,
                    );
                }
            }
            2010 => self.fx_shoot(PKind::WhiteSmoke, p, data),
            1504 => self.fx_dripstone(p),
            3007 => {
                // Shrieker scream: ten rings offset by 5 ticks, then sound above the block if it is
                // not waterlogged.
                for i in 0..10 {
                    let o = POptions { state: i * 5, ..Default::default() };
                    self.particles.spawn(PKind::Shriek, &o, [b[0] + 0.5, b[1] + 0.5, b[2] + 0.5], [0.0; 3]);
                }
                let s = self.fx_state(p);
                let wet = self.data.state(s).is_some_and(|i| i.has(rc_data::flags::FLUID_SOURCE));
                if !wet && let Some(a) = self.audio.get() {
                    let pitch = 0.6 + self.world_random.f() * 0.4;
                    let at = [b[0] + 0.5, b[1] + 0.5, b[2] + 0.5];
                    a.play("minecraft:block.sculk_shrieker.shriek", Category::Block, Some(at), 2.0, pitch, None);
                }
            }
            3008 => {
                // Brushing finished: sound of the brushed block, then break pieces (helper C).
                let state = u32::try_from(data).unwrap_or(0);
                let sound = match self.fx_block(state) {
                    "suspicious_sand" => Some("minecraft:item.brush.brushing.sand.complete"),
                    "suspicious_gravel" => Some("minecraft:item.brush.brushing.gravel.complete"),
                    _ => None,
                };
                if let Some(s) = sound {
                    self.fx_sound(s, Category::Player, p, 1.0, 1.0);
                }
                self.act_level_particles(2014, p, data);
            }
            3009 => self.fx_faces(PKind::EggCrack, p, 3, 6),
            3011 | 3012 | 3021 => {
                let flame = if data == 1 { PKind::SoulFireFlame } else { PKind::Flame };
                self.fx_trial_spawn(p, flame);
            }
            3013 | 3019 | 3020 => {
                // Player detection: 30 particles + 5 per detected player (10 at most),
                // scattered over the spawner's face; becoming ominous (3020): 20 × (omen,
                // soul flame) more.
                let (particle, n) = match kind {
                    3013 => (PKind::TrialDetection, 30 + data.clamp(0, 10) * 5),
                    3019 => (PKind::TrialDetectionOminous, 30 + data.clamp(0, 10) * 5),
                    _ => (PKind::TrialDetectionOminous, 30),
                };
                for _ in 0..n {
                    let pos = [
                        b[0] + 0.5 + (self.fx_r() * 2.0 - 1.0) * 0.65,
                        b[1] + 0.1 + self.fx_r() * 0.8,
                        b[2] + 0.5 + (self.fx_r() * 2.0 - 1.0) * 0.65,
                    ];
                    self.fx_add(particle, pos, [0.0; 3]);
                }
                if kind == 3020 {
                    for _ in 0..20 {
                        let pos = [
                            b[0] + 0.5 + (self.fx_r() - 0.5) * 2.0,
                            b[1] + 0.5 + (self.fx_r() - 0.5) * 2.0,
                            b[2] + 0.5 + (self.fx_r() - 0.5) * 2.0,
                        ];
                        let v = [self.fx_g() * 0.02, self.fx_g() * 0.02, self.fx_g() * 0.02];
                        self.fx_add(PKind::TrialOmen, pos, v);
                        self.fx_add(PKind::SoulFireFlame, pos, v);
                    }
                }
            }
            3014 | 3017 => {
                for _ in 0..20 {
                    let pos = [
                        b[0] + 0.4 + self.fx_r() * 0.2,
                        b[1] + 0.4 + self.fx_r() * 0.2,
                        b[2] + 0.4 + self.fx_r() * 0.2,
                    ];
                    let v = [self.fx_g() * 0.02, self.fx_g() * 0.02, self.fx_g() * 0.02];
                    self.fx_add(PKind::SmallFlame, pos, [v[0], v[1], v[2] * 0.25]);
                    self.fx_add(PKind::Smoke, pos, v);
                }
            }
            3015 if self.fx_block(self.fx_state(p)) == "vault" => {
                let flame = if data == 0 { PKind::SmallFlame } else { PKind::SoulFireFlame };
                for _ in 0..20 {
                    let pos = [
                        b[0] + 0.1 + self.fx_r() * 0.8,
                        b[1] + 0.25 + self.fx_r() * 0.5,
                        b[2] + 0.1 + self.fx_r() * 0.8,
                    ];
                    self.fx_add(PKind::Smoke, pos, [0.0; 3]);
                    self.fx_add(flame, pos, [0.0; 3]);
                }
            }
            3016 => {
                let flame = if data == 0 { PKind::SmallFlame } else { PKind::SoulFireFlame };
                for _ in 0..20 {
                    let pos = [
                        b[0] + 0.4 + self.fx_r() * 0.2,
                        b[1] + 0.4 + self.fx_r() * 0.2,
                        b[2] + 0.4 + self.fx_r() * 0.2,
                    ];
                    let v = [self.fx_g() * 0.02, self.fx_g() * 0.02, self.fx_g() * 0.02];
                    self.fx_add(flame, pos, v);
                }
            }
            3018 => {
                for _ in 0..10 {
                    let v = [self.fx_g() * 0.02, self.fx_g() * 0.02, self.fx_g() * 0.02];
                    let pos = [b[0] + self.fx_r(), b[1] + self.fx_r(), b[2] + self.fx_r()];
                    self.fx_add(PKind::Poof, pos, v);
                }
            }
            _ => {}
        }
    }

    /// Drip at the tip of a stalactite (1504): it forms when a fluid (or mud that
    /// moistens it) sits above the column; without a fluid, the dimension's default drip.
    fn fx_dripstone(&mut self, p: [i32; 3]) {
        let s = self.fx_state(p);
        if self.fx_block(s) != "pointed_dripstone" {
            return;
        }
        // Fluid read ABOVE the root of the column (same rule as the block animation).
        let mud = self.data.blocks.iter().position(|b| b.name == "minecraft:mud").map_or(u16::MAX, |i| i as u16);
        let top = self.world.min_y + self.world.height;
        let nether = self.dimension.ends_with("the_nether");
        let state = |q: [i32; 3]| self.fx_state(q);
        let Some((kind, _)) =
            crate::block_anim::stalactite_fluid(&self.data, &state, &self.phys, top, mud, p, s, nether)
        else {
            return;
        };
        // Horizontal offset of the stalactite model (maximum 0.125).
        let off = crate::crack::model_offset(&self.data, s, p);
        let at = [
            f64::from(p[0]) + 0.5 + f64::from(off.x),
            f64::from(p[1]) + 0.25,
            f64::from(p[2]) + 0.5 + f64::from(off.z),
        ];
        self.fx_add(kind, at, [0.0; 3]);
    }

    /// Spawn by a trial spawner: 20 × (smoke, flame).
    fn fx_trial_spawn(&mut self, p: [i32; 3], flame: PKind) {
        let c = p.map(|v| f64::from(v) + 0.5);
        for _ in 0..20 {
            let pos =
                [c[0] + (self.fx_r() - 0.5) * 2.0, c[1] + (self.fx_r() - 0.5) * 2.0, c[2] + (self.fx_r() - 0.5) * 2.0];
            self.fx_add(PKind::Smoke, pos, [0.0; 3]);
            self.fx_add(flame, pos, [0.0; 3]);
        }
    }

    /// Top of the shape in the center column of the block (composter).
    fn fx_center_top(&self, state: u32) -> f64 {
        let Some(info) = self.data.state(state) else { return 0.0 };
        self.data
            .shape(info.outline)
            .iter()
            .filter(|b| b.min[0] <= 0.5 && b.max[0] >= 0.5 && b.min[2] <= 0.5 && b.max[2] >= 0.5)
            .map(|b| b.max[1])
            .fold(0.0, f64::max)
    }

    /// Bone meal (1505): "spreading" plant (grass, nylium, moss…) or water → wide scatter
    /// above, otherwise scatter in the block.
    fn fx_bone_meal(&mut self, p: [i32; 3], n: i32) {
        let state = self.fx_state(p);
        let block = self.fx_block(state).to_owned();
        let spreader = matches!(
            block.as_str(),
            "grass_block" | "netherrack" | "crimson_nylium" | "warped_nylium" | "moss_block" | "pale_moss_block"
        );
        if spreader {
            self.fx_seed(PKind::HappyVillager, [p[0], p[1] + 1, p[2]], n * 3, 3.0, 1.0, false);
        } else if block == "water" {
            self.fx_seed(PKind::HappyVillager, p, n * 3, 3.0, 1.0, false);
        } else if !block.is_empty() && block != "air" {
            let at =
                if matches!(block.as_str(), "mangrove_leaves" | "rooted_dirt") { [p[0], p[1] - 1, p[2]] } else { p };
            let top = self.fx_top(self.fx_state(at));
            self.fx_seed(PKind::HappyVillager, at, n, 0.5, top, true);
        }
    }

    fn fx_sound(&mut self, name: &str, cat: Category, p: [i32; 3], volume: f32, pitch: f32) {
        if let Some(a) = self.audio.get() {
            let c = p.map(|v| f64::from(v) + 0.5);
            a.play(name, cat, Some(c), volume, pitch, None);
        }
    }

    /// 1010: jukebox song (registry, data = index), positioned at the block center,
    /// volume 4 (64 blocks); animated message above the hotbar.
    fn fx_jukebox_play(&mut self, p: [i32; 3], data: i32) {
        let Some(song) = usize::try_from(data).ok().and_then(|i| self.fx.songs.get(i)) else { return };
        let (sound, description) = (song.sound.clone(), song.description.clone());
        self.fx_jukebox_stop(p);
        if let Some(a) = self.audio.get() {
            let c = p.map(|v| f64::from(v) + 0.5);
            let req = SoundRequest::new(&sound, Category::Record).at(c).volume(4.0);
            if let Some(h) = a.play_request(req) {
                self.fx.jukebox.insert(p, h);
            }
        }
        self.chat.now_playing(description);
        crate::entity_render::jukebox_notify(&mut self.entities, &self.data.entity_dims, p, true);
    }

    /// 1011: song stopped, nearby parrots notified.
    fn fx_jukebox_stop_notify(&mut self, p: [i32; 3]) {
        self.fx_jukebox_stop(p);
        crate::entity_render::jukebox_notify(&mut self.entities, &self.data.entity_dims, p, false);
    }

    fn fx_jukebox_stop(&mut self, p: [i32; 3]) {
        if let (Some(h), Some(a)) = (self.fx.jukebox.remove(&p), self.audio.get()) {
            a.stop_handle(h);
        }
    }

    /// `animate` 1 (critical hit) and 2 (magic hit): follower emitter of 3 ticks, whose
    /// first one runs on creation.
    pub fn fx_animate(&mut self, entity: i32, action: u8) {
        let kind = match action {
            1 => PKind::Crit,
            2 => PKind::EnchantedHit,
            _ => return,
        };
        self.fx_emitter(entity, kind, 3);
    }

    /// Emitter that follows an entity for `lifetime` ticks (first tick immediate).
    pub fn fx_emitter(&mut self, entity: i32, kind: PKind, lifetime: i32) {
        if self.fx_entity_box(entity).is_none() {
            return;
        }
        let mut t = Tracker { entity, kind, life: 0, lifetime };
        if self.fx_track(&mut t) {
            self.fx.trackers.push(t);
        }
    }

    /// Position and size (width, height) of an entity, local player included.
    fn fx_entity_box(&self, id: i32) -> Option<([f64; 3], f64, f64)> {
        if id == self.player_id {
            let (w, h) = self.player.dimensions();
            return Some((self.player.pos, w, h));
        }
        let e = self.entities.map.get(&id)?;
        let table = self.renderer.as_ref().and_then(|r| r.entities.as_ref()).map_or(&[][..], |e| e.dims.as_slice());
        let s = e.size(table);
        Some((e.pos, f64::from(s.w), f64::from(s.h)))
    }

    /// One emitter tick: 16 attempts in the unit ball; false when it has finished.
    fn fx_track(&mut self, t: &mut Tracker) -> bool {
        let Some((pos, w, h)) = self.fx_entity_box(t.entity) else { return false };
        for _ in 0..16 {
            let a = [self.fx_r() * 2.0 - 1.0, self.fx_r() * 2.0 - 1.0, self.fx_r() * 2.0 - 1.0];
            if a[0] * a[0] + a[1] * a[1] + a[2] * a[2] > 1.0 {
                continue;
            }
            let at = [pos[0] + w * (a[0] / 4.0), pos[1] + h * (0.5 + a[1] / 4.0), pos[2] + w * (a[2] / 4.0)];
            self.fx_add(t.kind, at, [a[0], a[1] + 0.2, a[2]]);
        }
        t.life += 1;
        t.life < t.lifetime
    }

    /// Tick of the follower emitters, after that of the particles.
    pub fn fx_tick(&mut self) {
        let mut list = std::mem::take(&mut self.fx.trackers);
        list.retain_mut(|t| self.fx_track(t));
        list.append(&mut self.fx.trackers);
        self.fx.trackers = list;
        // Music track: start detected → notification (§ musicToast).
        // Called by the game tick, after the world sounds that joined the engine.
        if let Some(a) = self.audio.peek() {
            let music = self.world_sounds.music();
            let h = music.current_handle();
            let key = music.now_playing_key(a);
            let audible = self.options.f("soundCategory_master") * self.options.f("soundCategory_music") > 0.0;
            self.fx.music.observe(h, key, audible, self.options.f("notificationDisplayTime"));
        }
    }
}

/// "Now Playing" notification (`musicToast` setting): 600 ms slide from
/// the left at the top, visible 5 s × notification duration; also drawn fixed in the pause
/// menu ("pause" and "pause and notification" settings).
pub struct MusicToast {
    current: Option<SoundHandle>,
    /// Translation key of the song (`music.game.sweden`).
    key: Option<String>,
    anim_start: f64,
    became_visible: f64,
    shown: bool,
    wanted: bool,
    updating: bool,
    finished: bool,
    portion: f32,
    mult: f64,
    /// Start to display (audible sound) at the next draw.
    pending: bool,
    note_tick: i32,
    note_ms: f64,
    note_color: u32,
}

impl Default for MusicToast {
    fn default() -> Self {
        MusicToast {
            current: None,
            key: None,
            anim_start: -1.0,
            became_visible: -1.0,
            shown: false,
            wanted: false,
            updating: false,
            finished: true,
            portion: 0.0,
            mult: 1.0,
            pending: false,
            note_tick: 0,
            note_ms: 0.0,
            note_color: 0xffff_ffff,
        }
    }
}

/// Note colors (white, light gray, light blue, blue, cyan, green, light green, yellow, orange,
/// pink, red, magenta), brightened ×1.25; white is fixed at E6E6E6.
const NOTE_COLORS: [u32; 12] = {
    const fn ch(c: u32, s: u32) -> u32 {
        let v = ((c >> s) & 0xFF) * 125 / 100;
        if v > 255 { 255 } else { v }
    }
    const fn up(c: u32) -> u32 {
        0xFF00_0000 | ch(c, 16) << 16 | ch(c, 8) << 8 | ch(c, 0)
    }
    [
        0xFFE6_E6E6,
        up(0x9D9D97),
        up(0x3AB3DA),
        up(0x3C44AA),
        up(0x169C9C),
        up(0x5E7C16),
        up(0x80C71F),
        up(0xFED83D),
        up(0xF9801D),
        up(0xF38BAA),
        up(0xB02E26),
        up(0xC74EBD),
    ]
};

impl MusicToast {
    /// Current song seen on every tick: a new audible song restarts the notification;
    /// no more song → hidden.
    fn observe(&mut self, h: Option<SoundHandle>, key: Option<String>, audible: bool, mult: f64) {
        if h != self.current {
            if h.is_some() && audible {
                self.pending = true;
                self.mult = mult;
            } else if h.is_none() {
                self.wanted = false;
            }
            self.current = h;
        }
        self.key = if h.is_some() { key } else { None };
    }

    fn tick_notes(&mut self, now: f64) {
        if self.key.is_some() && now > self.note_ms + 25.0 {
            self.note_tick += 1;
            self.note_ms = now;
            let v = self.note_tick / 30;
            let (c1, c2) = (NOTE_COLORS[v as usize % 12], NOTE_COLORS[(v as usize + 1) % 12]);
            let f = (self.note_tick % 30) as f32 / 30.0;
            let ch = |s: u32| {
                let (a, b) = (((c1 >> s) & 0xFF) as f32, ((c2 >> s) & 0xFF) as f32);
                ((a + (b - a) * f) as u32) << s
            };
            self.note_color = 0xFF00_0000 | ch(16) | ch(8) | ch(0);
        }
    }

    /// Update (on every frame, `now` in ms).
    pub fn update(&mut self, now: f64) {
        if std::mem::take(&mut self.pending) {
            self.anim_start = -1.0;
            self.became_visible = -1.0;
            self.shown = false;
            self.portion = 0.0;
            self.finished = false;
            self.updating = true;
            self.wanted = true;
        }
        if self.anim_start < 0.0 {
            self.anim_start = now;
            self.shown = true;
        }
        if self.shown && now - self.anim_start <= 600.0 {
            self.became_visible = now;
        }
        let fully = now - self.became_visible;
        let x = (((now - self.anim_start) / 600.0) as f32).clamp(0.0, 1.0);
        let x = x * x;
        self.portion = if self.shown { x } else { 1.0 - x };
        if self.updating {
            self.wanted = fully < 5000.0 * self.mult;
            self.tick_notes(now);
        }
        if self.wanted != self.shown {
            self.anim_start = now - f64::from((1.0 - self.portion) * 600.0);
            self.shown = self.wanted;
        }
        let was = self.finished;
        self.finished = !self.shown && now - self.anim_start > 600.0;
        if self.finished && !was {
            self.updating = false;
        }
    }

    /// Drawing at `(x, 0)`: background, colored notes, song title in light gray.
    fn draw_at(&self, gui: &mut Gui, queue: &wgpu::Queue, x: f32) {
        let Some(key) = &self.key else { return };
        let title = rc_text::styled_chars(&rc_text::flatten(&Component::translate(key.clone(), vec![]), &gui.lang));
        let w = 30 + rc_text::width(gui, &title) + 7;
        gui.next_layer();
        gui.sprite("toast/now_playing", x, 0.0, w as f32, 30.0, 0xffff_ffff);
        // Animated notes (8 frames, 2 ticks each, `.png.mcmeta`) on the atlas clock.
        const NOTES: [&str; 8] = [
            "icon/music_notes",
            "icon/music_notes#1",
            "icon/music_notes#2",
            "icon/music_notes#3",
            "icon/music_notes#4",
            "icon/music_notes#5",
            "icon/music_notes#6",
            "icon/music_notes#7",
        ];
        let notes = NOTES[crate::sprite_anim::gui_frame(8, 2) as usize];
        gui.sprite(notes, x + 7.0, 7.0, 16.0, 16.0, self.note_color);
        gui.text(queue, &title, x + 30.0, 11.0, 0xFFD3_D3D3, true, 0);
    }

    fn width(&self, gui: &Gui) -> f32 {
        let Some(key) = &self.key else { return 0.0 };
        let title = rc_text::styled_chars(&rc_text::flatten(&Component::translate(key.clone(), vec![]), &gui.lang));
        (30 + rc_text::width(gui, &title) + 7) as f32
    }

    /// Sliding notification ("pause and notification" setting), outside the pause menu.
    pub fn draw_toast(&self, gui: &mut Gui, queue: &wgpu::Queue) {
        if self.finished {
            return;
        }
        let w = self.width(gui);
        self.draw_at(gui, queue, w * self.portion - w);
    }

    /// In the pause menu: fixed at the top left, animated notes.
    pub fn draw_pause(&mut self, gui: &mut Gui, queue: &wgpu::Queue, now: f64) {
        self.tick_notes(now);
        self.draw_at(gui, queue, 0.0);
    }
}

impl crate::App {
    /// Connection to a server that does not enforce secure chat: system notification of
    /// 10 s (once per connection, not on every reconfiguration; never in singleplayer).
    pub(crate) fn unsecure_toast(&mut self, enforces_secure_chat: bool) {
        if !unsecure_due(&mut self.misc.unsecure_seen, enforces_secure_chat, self.solo.is_some()) {
            return;
        }
        let Some(gui) = self.renderer.as_ref().map(|r| &r.gui) else { return };
        let title = Component::translate("multiplayer.unsecureserver.toast.title", vec![]);
        let message = Component::translate("multiplayer.unsecureserver.toast", vec![]);
        self.toasts.system(gui, &title, Some(&message), 10_000.0);
    }
}

/// The "insecure server" notification is due on this connection packet: server without secure
/// chat, outside singleplayer, not yet seen on this connection (then marked seen).
fn unsecure_due(seen: &mut bool, enforces_secure_chat: bool, solo: bool) -> bool {
    if enforces_secure_chat || solo || *seen {
        return false;
    }
    *seen = true;
    true
}

#[cfg(test)]
mod tests {
    use super::unsecure_due;

    /// Shown at the first connection packet, not after a reconfiguration (same connection),
    /// again after a new connection or a transfer (flag reset to false).
    #[test]
    fn unsecure_toast_once_per_connection() {
        let mut seen = false;
        assert!(unsecure_due(&mut seen, false, false));
        assert!(!unsecure_due(&mut seen, false, false), "reconfiguration: already seen");
        seen = false;
        assert!(unsecure_due(&mut seen, false, false), "new connection or transfer");
        let mut other = false;
        assert!(!unsecure_due(&mut other, true, false));
        assert!(!unsecure_due(&mut other, false, true));
        assert!(!other);
    }
}
