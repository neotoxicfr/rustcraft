//! Player projectiles (mission S8, `docs/specs/server-projectiles.md`): arrows (bow,
//! crossbow, spectral, effect), trident, snowballs, eggs, Ender pearls, thrown and
//! lingering potions (effect cloud), experience bottles, rockets (elytra included),
//! fireballs (dispenser; blaze and ghast F12, thrown by `mobs/fireballs.rs`), fishing bobber. Simulated like the official 26.3 server:
//! same order of operations per tick, same constants, same network tracking.
//!
//! Mobs shoot through this same engine: skeleton arrows (`mobs/arrows.rs`), crossbow bolts
//! (`mobs/illagers/crossbow.rs`), fireballs (`mobs/fireballs.rs`).

mod chorus;
pub(crate) mod cloud;
pub(crate) mod dispense;
pub(crate) mod ench;
pub(crate) mod fishing;
pub(crate) mod hits;
pub(crate) mod physics;
mod progress;
mod save;
pub(crate) mod weapons;

#[cfg(test)]
mod impact_tests;
#[cfg(test)]
mod progress_tests;
#[cfg(test)]
mod tests;

use rc_client::{EntityEvent, Event};
use rc_proto_v777::entity::{AddEntity, Delta, Meta, Move, PositionPath, PositionSync};

use crate::mobs::jrand::JRand;
use crate::mobs::mob::Ent;
use crate::mobs::mth::pack_degrees;
use crate::mobs::track::lp_quantize;
use crate::player::Stack;
use crate::server::Server;

/// Kind of projectile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PKind {
    Arrow,
    SpectralArrow,
    Trident,
    Snowball,
    Egg,
    EnderPearl,
    SplashPotion,
    LingeringPotion,
    XpBottle,
    Firework,
    SmallFireball,
    /// F12: ghast large fireball.
    LargeFireball,
    /// F14: wind charge of the player and the breeze.
    WindCharge,
    BreezeWindCharge,
    /// F15: dragon fireball.
    DragonFireball,
    /// F15: shulker bullet.
    ShulkerBullet,
    /// F16: wither skull (`mobs/wither/skull.rs`).
    WitherSkull,
    /// G5-F10: thrown Eye of Ender (not a game projectile: no owner or collision;
    /// `eye_of_ender.rs`).
    EyeOfEnder,
}

impl PKind {
    pub const ALL: [PKind; 18] = [
        PKind::Arrow,
        PKind::SpectralArrow,
        PKind::Trident,
        PKind::Snowball,
        PKind::Egg,
        PKind::EnderPearl,
        PKind::SplashPotion,
        PKind::LingeringPotion,
        PKind::XpBottle,
        PKind::Firework,
        PKind::SmallFireball,
        PKind::LargeFireball,
        PKind::WindCharge,
        PKind::BreezeWindCharge,
        PKind::DragonFireball,
        PKind::ShulkerBullet,
        PKind::WitherSkull,
        PKind::EyeOfEnder,
    ];

    pub fn name(self) -> &'static str {
        match self {
            PKind::Arrow => "minecraft:arrow",
            PKind::SpectralArrow => "minecraft:spectral_arrow",
            PKind::Trident => "minecraft:trident",
            PKind::Snowball => "minecraft:snowball",
            PKind::Egg => "minecraft:egg",
            PKind::EnderPearl => "minecraft:ender_pearl",
            PKind::SplashPotion => "minecraft:splash_potion",
            PKind::LingeringPotion => "minecraft:lingering_potion",
            PKind::XpBottle => "minecraft:experience_bottle",
            PKind::Firework => "minecraft:firework_rocket",
            PKind::SmallFireball => "minecraft:small_fireball",
            PKind::LargeFireball => "minecraft:fireball",
            PKind::WindCharge => "minecraft:wind_charge",
            PKind::BreezeWindCharge => "minecraft:breeze_wind_charge",
            PKind::DragonFireball => "minecraft:dragon_fireball",
            PKind::ShulkerBullet => "minecraft:shulker_bullet",
            PKind::WitherSkull => "minecraft:wither_skull",
            PKind::EyeOfEnder => "minecraft:eye_of_ender",
        }
    }

    pub fn from_name(n: &str) -> Option<PKind> {
        PKind::ALL.into_iter().find(|k| k.name() == n)
    }

    /// Arrow or trident.
    pub fn arrow_like(self) -> bool {
        matches!(self, PKind::Arrow | PKind::SpectralArrow | PKind::Trident)
    }

    /// Thrown item.
    pub fn throwable(self) -> bool {
        matches!(
            self,
            PKind::Snowball
                | PKind::Egg
                | PKind::EnderPearl
                | PKind::SplashPotion
                | PKind::LingeringPotion
                | PKind::XpBottle
        )
    }

    /// Width and height of the box.
    pub fn size(self) -> (f64, f64) {
        match self {
            PKind::Arrow | PKind::SpectralArrow | PKind::Trident => (0.5, 0.5),
            PKind::SmallFireball | PKind::ShulkerBullet | PKind::WitherSkull => (0.3125, 0.3125),
            PKind::DragonFireball => (1.0, 1.0),
            PKind::LargeFireball => (1.0, 1.0),
            PKind::WindCharge | PKind::BreezeWindCharge => (0.3125, 0.3125),
            _ => (0.25, 0.25),
        }
    }

    /// Default gravity.
    pub fn gravity(self) -> f64 {
        match self {
            PKind::Arrow | PKind::SpectralArrow | PKind::Trident => 0.05,
            PKind::SplashPotion | PKind::LingeringPotion => 0.05,
            PKind::XpBottle => 0.07,
            PKind::Firework
            | PKind::SmallFireball
            | PKind::LargeFireball
            | PKind::DragonFireball
            | PKind::WitherSkull => 0.0,
            PKind::WindCharge | PKind::BreezeWindCharge => 0.0,
            PKind::ShulkerBullet => 0.04,
            _ => 0.03,
        }
    }

    /// Fireball (small, large, dragon's, wither skull,
    /// wind charge).
    pub fn hurting(self) -> bool {
        matches!(self, PKind::SmallFireball | PKind::LargeFireball | PKind::DragonFireball | PKind::WitherSkull)
            || self.wind()
    }

    /// F14: wind charge (no fire, no item, inertia 1, box lowered by 0.15).
    pub fn wind(self) -> bool {
        matches!(self, PKind::WindCharge | PKind::BreezeWindCharge)
    }

    /// Network tracking interval.
    pub fn interval(self) -> i32 {
        match self {
            k if k.arrow_like() => 20,
            // F15: default interval of entities.
            PKind::ShulkerBullet => 3,
            PKind::EyeOfEnder => 4,
            _ => 10,
        }
    }

    /// Tracking range (in chunks): 4, 8 for the shulker bullet (F15).
    pub fn range(self) -> i32 {
        if self == PKind::ShulkerBullet { 8 } else { 4 }
    }

    /// Projectile that carries an item (data 8, saved `Item`).
    pub fn carries_item(self) -> bool {
        !matches!(self, PKind::DragonFireball | PKind::ShulkerBullet | PKind::WitherSkull)
    }

    /// Default item carried (the default item / the default pickup item).
    pub fn default_item(self) -> &'static str {
        match self {
            PKind::Arrow => "minecraft:arrow",
            PKind::SpectralArrow => "minecraft:spectral_arrow",
            PKind::Trident => "minecraft:trident",
            PKind::Snowball => "minecraft:snowball",
            PKind::Egg => "minecraft:egg",
            PKind::EnderPearl => "minecraft:ender_pearl",
            PKind::SplashPotion => "minecraft:splash_potion",
            PKind::LingeringPotion => "minecraft:lingering_potion",
            PKind::XpBottle => "minecraft:experience_bottle",
            PKind::Firework => "minecraft:firework_rocket",
            // Skull (F16): no item (never saved nor sent).
            PKind::SmallFireball | PKind::LargeFireball | PKind::WitherSkull => "minecraft:fire_charge",
            PKind::WindCharge | PKind::BreezeWindCharge => "minecraft:wind_charge",
            // F15: no item (no data, no save).
            PKind::DragonFireball | PKind::ShulkerBullet => "minecraft:air",
            PKind::EyeOfEnder => "minecraft:ender_eye",
        }
    }
}

/// Pickup of an arrow.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pickup {
    Disallowed = 0,
    Allowed = 1,
    CreativeOnly = 2,
}

/// Network tracking of a projectile (reduced: position, rotation, velocity, data).
#[derive(Debug, Clone, Default)]
pub struct PTrack {
    pub seen: bool,
    base: [f64; 3],
    last_yaw: i8,
    last_pitch: i8,
    last_vel: [f64; 3],
    tick: i32,
    teleport_delay: i32,
    was_on_ground: bool,
}

/// Live projectile.
#[derive(Debug, Clone)]
pub struct Proj {
    pub id: i32,
    pub uuid: u128,
    pub kind: PKind,
    pub pos: [f64; 3],
    /// Position at the start of the tick (`xo`, `yo`, `zo`).
    pub old_pos: [f64; 3],
    pub vel: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    /// Owner (UUID): the player or a mob.
    pub owner: Option<u128>,
    pub left_owner: bool,
    pub has_been_shot: bool,
    /// Tick counter of the entity (incremented before each tick).
    pub tick_count: i32,
    pub rng: JRand,
    pub removed: bool,
    pub fire: i32,
    pub in_water: bool,
    pub first_tick: bool,
    pub on_ground: bool,
    pub horizontal_collision: bool,
    pub no_gravity: bool,
    pub silent: bool,
    /// Immediate send at the next tracking.
    pub needs_sync: bool,
    // --- Arrows and trident. ---
    pub in_ground: bool,
    pub in_ground_time: i32,
    pub life: i32,
    pub shake: i32,
    pub damage: f64,
    pub crit: bool,
    pub pierce: u8,
    pub pickup: Pickup,
    /// Stack picked up (arrow), thrown (item), or rocket item.
    pub item: Stack,
    pub weapon: Option<Stack>,
    pub hit_sound: &'static str,
    pub last_state: Option<u32>,
    pub pierced: Vec<i32>,
    /// Victims killed by this piercing arrow (`killed_by_arrow`): created at the first hit
    /// dealt with a piercing level, emptied (not deleted) when the arrow sticks.
    pub pierce_killed: Option<Vec<Ent>>,
    pub no_physics: bool,
    pub dealt_damage: bool,
    pub loyalty: u8,
    pub return_ticks: i32,
    pub spectral_duration: i32,
    pub last_deflected_by: Option<Ent>,
    // --- Rockets. ---
    pub fw_life: i32,
    pub fw_lifetime: i32,
    pub attached: Option<Ent>,
    pub at_angle: bool,
    // --- Fireball. ---
    pub accel: f64,
    /// F12: explosion power of the large fireball (`ExplosionPower`).
    pub power: i32,
    /// F15: shulker bullet (target, current flight segment).
    pub bullet: Option<Box<crate::mobs::end::bullet::BulletState>>,
    /// F16: blue wither skull (`dangerous`, data 8).
    pub dangerous: bool,
    /// G5-F10: Eye of Ender (aimed point, never saved; survival at the end of the flight).
    pub eye_target: Option<[f64; 3]>,
    pub eye_survive: bool,
    /// Entity data (index, value, modified).
    meta: Vec<(u8, Meta, bool)>,
    pub track: PTrack,
}

/// State of the open world's projectiles (exchanged with the dimension, like mobs).
#[derive(Debug, Default)]
pub struct Projs {
    pub list: Vec<Proj>,
    pub clouds: Vec<cloud::Cloud>,
    /// Fishing bobber of the player (not saved).
    pub hook: Option<fishing::Hook>,
    /// Entity chunks whose projectiles were read.
    pub adopted: hashbrown::HashSet<(i32, i32)>,
    /// During the damage dealt directly by a thrown potion: true if it hurts those
    /// sensitive to water (the enderman only takes those, then teleports).
    pub direct_potion: Option<bool>,
}

/// An Ender pearl of `owner` flies among these projectiles (its ticket keeps the world active).
pub(crate) fn holds_player_pearl(p: &Projs, owner: u128) -> bool {
    p.list.iter().any(|q| !q.removed && q.kind == PKind::EnderPearl && q.owner == Some(owner))
}

/// State tied to the player (does not change with the dimension).
#[derive(Debug, Default)]
pub struct PlayerProj {
    /// Movement known to the client and received during this tick.
    pub known_move: [f64; 3],
    pub moved_this_tick: bool,
    /// Items on cooldown: group → (start, end) in player ticks.
    pub cooldowns: Vec<(String, i32, i32)>,
    pub ticks: i32,
    /// Crossbow: loading sounds already played (start, middle).
    pub xbow_sounds: (bool, bool),
    /// Spin attack (impulse): remaining ticks, damage, stack, targets hit.
    pub spin: i32,
    pub spin_damage: f32,
    pub spin_item: Option<Stack>,
    /// Last value sent of the player's data 8 (live flags).
    pub flags_sent: i8,
    /// Pearls read from the player's file, to put back into play as soon as the world is ready.
    pub pending_pearls: Vec<rc_proto::nbt::Nbt>,
    pub pearls_read: bool,
    /// Gliding (elytra) and flight ticks.
    pub fall_flying: bool,
    pub fall_fly_ticks: i32,
    /// Arrows stuck in the player (data 12) and delay before losing one.
    pub arrow_count: i32,
    pub arrow_remove: i32,
}

pub(crate) const DEG: f64 = (180.0f32 / std::f32::consts::PI) as f64;

/// Per-tick position trace (parity tests): active if `RUSTCRAFT_S8_TRACE` is set.
pub static TRACE: std::sync::Mutex<Vec<TraceRow>> = std::sync::Mutex::new(Vec::new());

/// Trace line: UUID, type, tick, position, velocity.
pub type TraceRow = (u128, &'static str, i32, [f64; 3], [f64; 3]);

fn tracing() -> bool {
    rc_data::env_flag!("RUSTCRAFT_S8_TRACE")
}

fn trace(p: &Proj) {
    if tracing()
        && let Ok(mut t) = TRACE.lock()
    {
        t.push((p.uuid, p.kind.name(), p.tick_count, p.pos, p.vel));
    }
}

/// Rotation interpolation of a projectile.
pub(crate) fn lerp_rotation(old: f32, new: f32) -> f32 {
    let mut o = old;
    while new - o < -180.0 {
        o -= 360.0;
    }
    while new - o >= 180.0 {
        o += 360.0;
    }
    o + 0.2 * (new - o)
}

fn unpack(b: i8) -> f32 {
    f32::from(b) * 360.0 / 256.0
}

fn enc(v: f64) -> i64 {
    rc_proto_v777::entity::delta_unit(v)
}

impl Proj {
    /// New projectile at `pos` (own random draw, UUID v4 drawn from that draw like mobs).
    pub fn new(kind: PKind, id: i32, pos: [f64; 3], item: Stack) -> Proj {
        let mut rng = JRand::from_time(i64::from(id) ^ 0x5338);
        let uuid = (rng.next_long() as u128) << 64 | rng.next_long() as u64 as u128;
        let uuid = (uuid & !(0xF000u128 << 64) | (0x4000u128 << 64)) & !(0xC000_0000_0000_0000u128)
            | 0x8000_0000_0000_0000u128;
        let mut p = Proj {
            id,
            uuid,
            kind,
            pos,
            old_pos: pos,
            vel: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            owner: None,
            left_owner: false,
            has_been_shot: false,
            tick_count: 0,
            rng,
            removed: false,
            fire: 0,
            in_water: false,
            first_tick: true,
            on_ground: false,
            horizontal_collision: false,
            no_gravity: false,
            silent: false,
            needs_sync: false,
            in_ground: false,
            in_ground_time: 0,
            life: 0,
            shake: 0,
            damage: 2.0,
            crit: false,
            pierce: 0,
            pickup: Pickup::Disallowed,
            item,
            weapon: None,
            hit_sound: if kind == PKind::Trident {
                "minecraft:item.trident.hit_ground"
            } else {
                "minecraft:entity.arrow.hit"
            },
            last_state: None,
            pierced: Vec::new(),
            pierce_killed: None,
            no_physics: false,
            dealt_damage: false,
            loyalty: 0,
            return_ticks: 0,
            spectral_duration: 200,
            last_deflected_by: None,
            fw_life: 0,
            fw_lifetime: 0,
            attached: None,
            at_angle: false,
            accel: 0.1,
            power: 1,
            bullet: None,
            dangerous: false,
            eye_target: None,
            eye_survive: false,
            meta: Vec::new(),
            track: PTrack::default(),
        };
        p.define_meta();
        p
    }

    /// Collision box (feet at bottom center).
    pub fn bbox(&self) -> rc_physics::Aabb {
        let (w, h) = self.kind.size();
        let r = w / 2.0;
        // F14: wind charge box lowered by 0.15.
        let dy = if self.kind.wind() { -f64::from(0.15f32) } else { 0.0 };
        rc_physics::Aabb {
            min: [self.pos[0] - r, self.pos[1] + dy, self.pos[2] - r],
            max: [self.pos[0] + r, self.pos[1] + dy + h, self.pos[2] + r],
        }
    }

    pub fn block_pos(&self) -> [i32; 3] {
        self.pos.map(|v| v.floor() as i32)
    }

    fn define(&mut self, i: u8, v: Meta) {
        self.meta.push((i, v, false));
    }

    /// Default values of the data.
    fn define_meta(&mut self) {
        // Entity: flags (0); the rest is only sent if it changes.
        self.define(0, Meta::Byte(0));
        match self.kind {
            PKind::Arrow | PKind::SpectralArrow | PKind::Trident => {
                self.define(8, Meta::Byte(0));
                self.define(9, Meta::Byte(0));
                self.define(10, Meta::Bool(false));
                match self.kind {
                    PKind::Arrow => self.define(11, Meta::Int(-1)),
                    PKind::Trident => {
                        self.define(11, Meta::Byte(0));
                        self.define(12, Meta::Bool(false));
                    }
                    _ => {}
                }
            }
            PKind::Firework => {
                self.define(8, Meta::Item(None));
                self.define(9, Meta::OptionalInt(None));
                self.define(10, Meta::Bool(false));
            }
            PKind::SmallFireball | PKind::LargeFireball => self.define(8, Meta::Item(None)),
            // F14: the wind charge has no item data.
            PKind::WindCharge | PKind::BreezeWindCharge => {}
            // F15: no data of its own.
            PKind::DragonFireball | PKind::ShulkerBullet => {}
            PKind::WitherSkull => self.define(8, Meta::Bool(false)),
            _ => self.define(8, Meta::Item(None)),
        }
    }

    /// Changes a data (marked if it differs).
    pub fn set_meta(&mut self, i: u8, v: Meta) {
        if let Some(s) = self.meta.iter_mut().find(|s| s.0 == i) {
            if s.1 != v {
                s.1 = v;
                s.2 = true;
            }
        } else {
            self.meta.push((i, v, true));
        }
    }

    fn take_dirty(&mut self) -> Vec<(u8, Meta)> {
        let mut out = Vec::new();
        for s in &mut self.meta {
            if s.2 {
                s.2 = false;
                out.push((s.0, s.1.clone()));
            }
        }
        out
    }

    /// Arrow flags (8): critical (1), no physics (2).
    pub fn sync_arrow_flags(&mut self) {
        if !self.kind.arrow_like() {
            return;
        }
        let f = i8::from(self.crit) | (i8::from(self.no_physics) << 1);
        self.set_meta(8, Meta::Byte(f));
        self.set_meta(9, Meta::Byte(self.pierce as i8));
        self.set_meta(10, Meta::Bool(self.in_ground));
    }

    /// Shared "on fire" flag (0, bit 0).
    pub fn sync_fire_flag(&mut self) {
        self.set_meta(0, Meta::Byte(i8::from(self.fire > 0)));
    }
}

impl Server {
    /// New network identifier.
    pub(crate) fn proj_new_id(&mut self) -> i32 {
        self.next_entity += 1;
        self.next_entity
    }

    /// Resolved owner (player or living mob).
    pub(crate) fn proj_owner(&self, p: &Proj) -> Option<Ent> {
        let u = p.owner?;
        if u == self.uuid {
            return Some(Ent::Player);
        }
        self.mobs
            .list
            .iter()
            .flatten()
            .find(|m| m.uuid == u && !m.removed)
            .map(|m| Ent::Mob(m.id))
            // Shooter taken out of the list during its own tick.
            .or_else(|| self.mobs.ticking.iter().rev().find(|t| t.0 == u).map(|t| Ent::Mob(t.1)))
    }

    /// Network identifier of a living entity.
    pub(crate) fn ent_net_id(&self, e: Ent) -> i32 {
        match e {
            Ent::Player => self.player_id,
            Ent::Mob(id) => id,
        }
    }

    /// Addition to the world: the player is paired at once.
    pub(crate) fn proj_add(&mut self, mut p: Proj) {
        p.sync_arrow_flags();
        if self.proj_visible(&p) {
            self.proj_pair(&mut p);
        }
        self.proj.list.push(p);
    }

    fn proj_visible(&self, p: &Proj) -> bool {
        let range = (f64::from(p.kind.range()) * 16.0).min(f64::from(self.view) * 16.0);
        let dx = self.pos[0] - p.pos[0];
        let dz = self.pos[2] - p.pos[2];
        let c = ((p.pos[0].floor() as i32) >> 4, (p.pos[2].floor() as i32) >> 4);
        dx * dx + dz * dz <= range * range && self.sent.contains(&c) && !p.removed
    }

    /// Pairing data (`AddEntity` + non-default data).
    fn proj_pair(&self, p: &mut Proj) {
        let owner = self.proj_owner(p);
        let data = match (p.kind, owner) {
            (_, Some(o)) => self.ent_net_id(o),
            _ => 0,
        };
        let add = AddEntity {
            id: p.id,
            uuid: p.uuid,
            kind: self.entity_kind_id(p.kind.name()),
            pos: p.pos,
            velocity: lp_quantize(p.vel),
            pitch: unpack(pack_degrees(p.pitch)),
            yaw: unpack(pack_degrees(p.yaw)),
            head_yaw: 0.0,
            data,
        };
        let _ = self.send(Event::Entity(EntityEvent::Add(Box::new(add))));
        // Data: all those that differ from the type's default value.
        let mut values = Vec::new();
        for (i, v, _) in &p.meta {
            let default = match (p.kind, *i) {
                (_, 0) => Meta::Byte(0),
                (k, 8) if k.arrow_like() => Meta::Byte(0),
                (k, 9) if k.arrow_like() => Meta::Byte(0),
                (k, 10) if k.arrow_like() => Meta::Bool(false),
                (PKind::Arrow, 11) => Meta::Int(-1),
                (PKind::Trident, 11) => Meta::Byte(0),
                (PKind::Trident, 12) => Meta::Bool(false),
                (PKind::Firework, 9) => Meta::OptionalInt(None),
                (PKind::Firework, 10) => Meta::Bool(false),
                _ => Meta::Item(None),
            };
            // The carried item always goes out: the client only knows the default item.
            let item_slot = *i == 8 && !p.kind.arrow_like();
            if *v != default || item_slot {
                values.push((*i, v.clone()));
            }
        }
        for s in &mut p.meta {
            s.2 = false;
        }
        if !values.is_empty() {
            let _ = self.send(Event::Entity(EntityEvent::Data { id: p.id, values }));
        }
        let t = &mut p.track;
        t.seen = true;
        t.base = p.pos;
        t.last_yaw = pack_degrees(p.yaw);
        t.last_pitch = pack_degrees(p.pitch);
        t.last_vel = p.vel;
        t.was_on_ground = p.on_ground;
    }

    /// End of tick.
    pub(crate) fn proj_send_changes(&self, p: &mut Proj) {
        let visible = self.proj_visible(p);
        if visible && !p.track.seen {
            self.proj_pair(p);
        } else if !visible && p.track.seen {
            let _ = self.send(Event::Entity(EntityEvent::Remove(vec![p.id])));
            p.track.seen = false;
        }
        if !p.track.seen {
            p.take_dirty();
            p.track.tick += 1;
            return;
        }
        let dirty = p.meta.iter().any(|s| s.2);
        let interval = p.kind.interval();
        if p.needs_sync || p.track.tick % interval == 0 || dirty {
            let yaw = pack_degrees(p.yaw);
            let pitch = pack_degrees(p.pitch);
            let rot = (i32::from(yaw) - i32::from(p.track.last_yaw)).abs() >= 1
                || (i32::from(pitch) - i32::from(p.track.last_pitch)).abs() >= 1;
            p.track.teleport_delay += 1;
            let d = [p.pos[0] - p.track.base[0], p.pos[1] - p.track.base[1], p.pos[2] - p.track.base[2]];
            let changed = d[0] * d[0] + d[1] * d[1] + d[2] * d[2] >= f64::from(7.629_394_5e-6f32);
            let arrow = p.kind.arrow_like();
            let (send_pos, send_rot) = if arrow { (true, true) } else { (changed || p.track.tick % 60 == 0, rot) };
            let full = p.track.teleport_delay > 400 || p.track.was_on_ground != p.on_ground;
            let sync = |s: &Server, p: &Proj| {
                let ps = PositionSync {
                    id: p.id,
                    path: PositionPath { linear: true, steps: vec![(p.pos, 0)] },
                    yaw: p.yaw,
                    pitch: p.pitch,
                    on_ground: p.on_ground,
                };
                let _ = s.send(Event::Entity(EntityEvent::Sync(ps)));
            };
            // Velocity before the movement packet (game order).
            let v = p.vel;
            let dv = [v[0] - p.track.last_vel[0], v[1] - p.track.last_vel[1], v[2] - p.track.last_vel[2]];
            let diff = dv[0] * dv[0] + dv[1] * dv[1] + dv[2] * dv[2];
            if diff > 1.0e-7 || (diff > 0.0 && v == [0.0; 3]) {
                p.track.last_vel = v;
                let _ = self.send(Event::Entity(EntityEvent::Motion { id: p.id, velocity: lp_quantize(v) }));
                if p.kind.hurting() {
                    let power = rc_client::misc::Misc::ProjectilePower { id: p.id, power: p.accel };
                    let _ = self.send(Event::Misc(power));
                }
            }
            let (mut sent_pos, mut sent_rot) = (false, false);
            if full {
                p.track.was_on_ground = p.on_ground;
                p.track.teleport_delay = 0;
                sync(self, p);
                (sent_pos, sent_rot) = (true, true);
            } else if send_pos {
                let e = [
                    enc(p.pos[0]) - enc(p.track.base[0]),
                    enc(p.pos[1]) - enc(p.track.base[1]),
                    enc(p.pos[2]) - enc(p.track.base[2]),
                ];
                if e.iter().any(|v| !(-32768..=32767).contains(v)) {
                    sync(self, p);
                    sent_rot = true;
                } else {
                    let mv = Move {
                        id: p.id,
                        on_ground: p.on_ground,
                        delta: Some(Delta::Linear([e[0] as i16, e[1] as i16, e[2] as i16])),
                        rot: send_rot.then(|| (unpack(yaw), unpack(pitch))),
                    };
                    let _ = self.send(Event::Entity(EntityEvent::Move(mv)));
                    sent_rot = send_rot;
                }
                sent_pos = true;
            } else if send_rot {
                let mv =
                    Move { id: p.id, on_ground: p.on_ground, delta: None, rot: Some((unpack(yaw), unpack(pitch))) };
                let _ = self.send(Event::Entity(EntityEvent::Move(mv)));
                sent_rot = true;
            }
            let values = p.take_dirty();
            if !values.is_empty() {
                let _ = self.send(Event::Entity(EntityEvent::Data { id: p.id, values }));
            }
            if sent_pos {
                p.track.base = p.pos;
            }
            if sent_rot {
                p.track.last_yaw = yaw;
                p.track.last_pitch = pitch;
            }
            p.needs_sync = false;
        }
        p.track.tick += 1;
    }

    /// Tick of the open world's projectiles, then clouds, bobber, pickup and tracking.
    pub(crate) fn projectiles_tick(&mut self) {
        self.proj_restore_pearls();
        self.proj_player_tick();
        // Frozen (`tick freeze`): projectiles, clouds and floats wait.
        let runs = self.cmd.ticking.runs();
        // Network tracking BEFORE the projectile tick, like the official server: the client
        // receives the state of the previous tick then does the current tick itself.
        let mut list = std::mem::take(&mut self.proj.list);
        for p in list.iter_mut().filter(|p| !p.removed) {
            self.proj_send_changes(p);
        }
        let added = std::mem::take(&mut self.proj.list);
        list.extend(added);
        self.proj.list = list;
        if runs {
            self.proj_list_tick();
            self.clouds_tick();
            self.fishing_tick();
        }
        self.proj_player_touch();
        self.spin_attack_tick();
        self.proj_retain();
    }

    /// Projectiles of a parked dimension (portal ticket or player's pearl in flight): tracking
    /// (sends cut off), tick, removal. The player's pearls keep their chunk loaded.
    pub(crate) fn proj_world_tick(&mut self) {
        let mut list = std::mem::take(&mut self.proj.list);
        for p in list.iter_mut().filter(|p| !p.removed) {
            self.proj_send_changes(p);
        }
        let added = std::mem::take(&mut self.proj.list);
        list.extend(added);
        self.proj.list = list;
        let pearls: Vec<(i32, i32)> = self
            .proj
            .list
            .iter()
            .filter(|p| !p.removed && p.kind == PKind::EnderPearl && p.owner == Some(self.uuid))
            .map(|p| ((p.pos[0].floor() as i32) >> 4, (p.pos[2].floor() as i32) >> 4))
            .collect();
        if !pearls.is_empty() {
            // Read from disk or put into generation, without waiting: a projectile whose chunk
            // is not ready does not tick (entity outside a loaded chunk).
            self.ensure_loaded(&pearls);
        }
        self.proj_list_tick();
        self.proj_retain();
    }

    /// The projectile's chunk is ready (parked dimension only: the player's ticks everything).
    fn proj_chunk_ready(&self, p: &Proj) -> bool {
        let c = ((p.pos[0].floor() as i32) >> 4, (p.pos[2].floor() as i32) >> 4);
        self.dims.home.is_none() || matches!(self.chunks.get(&c), Some(Some(_)))
    }

    /// Tick of each projectile present at the start.
    fn proj_list_tick(&mut self) {
        // The list stays in place: during a projectile's tick, the others stay
        // findable by identifier (bullets hit by an arrow or by the explosion of a
        // fireball). Projectiles added during the tick (crossbow rockets…) go
        // at the end and wait for the next tick.
        let n = self.proj.list.len();
        for i in 0..n {
            let Some(slot) = self.proj.list.get(i) else { break };
            if slot.removed || !self.proj_chunk_ready(slot) {
                continue;
            }
            let mut p = slot.clone();
            let id = p.id;
            p.tick_count += 1;
            p.old_pos = p.pos;
            self.proj_tick(&mut p);
            trace(&p);
            match self.proj.list.get_mut(i) {
                Some(slot) if slot.id == id => {
                    // Destroyed by another during its own tick (explosion it caused).
                    p.removed |= slot.removed;
                    *slot = p;
                }
                // List replaced during the tick (dimension change): stop.
                _ => break,
            }
        }
    }

    /// Removed projectiles: taken off the list, forgotten by the client that saw them.
    fn proj_retain(&mut self) {
        let mut list = std::mem::take(&mut self.proj.list);
        let mut gone = Vec::new();
        for p in list.iter_mut() {
            if p.removed && p.track.seen {
                gone.push(p.id);
            }
        }
        if self.cmd.scores.has_entries() {
            for p in list.iter().filter(|p| p.removed) {
                self.entity_scores_removed(p.uuid);
            }
        }
        list.retain(|p| !p.removed);
        let added = std::mem::take(&mut self.proj.list);
        list.extend(added);
        self.proj.list = list;
        if !gone.is_empty() {
            let _ = self.send(Event::Entity(EntityEvent::Remove(gone)));
        }
    }

    /// Player state tied to projectiles: cooldowns, stuck arrows, gliding, use.
    pub(crate) fn proj_player_tick(&mut self) {
        let pp = &mut self.proj_player;
        pp.ticks += 1;
        let now = pp.ticks;
        // Expired cooldown: removed, and its end announced to the player (duration 0) to clear the veil.
        // The test first: no allocation at the ordinary tick.
        if pp.cooldowns.iter().any(|c| c.2 <= now) {
            let mut ended = Vec::new();
            pp.cooldowns.retain(|c| {
                let keep = c.2 > now;
                if !keep {
                    ended.push(c.0.clone());
                }
                keep
            });
            for group in ended {
                let _ = self.send(rc_client::Event::Cooldown { group, ticks: 0 });
            }
        }
        let pp = &mut self.proj_player;
        if pp.arrow_count > 0 {
            if pp.arrow_remove <= 0 {
                pp.arrow_remove = 20 * (30 - pp.arrow_count);
            }
            pp.arrow_remove -= 1;
            if pp.arrow_remove <= 0 {
                pp.arrow_count -= 1;
                let c = pp.arrow_count;
                let _ = self
                    .send(Event::Entity(EntityEvent::Data { id: self.player_id, values: vec![(12, Meta::Int(c))] }));
            }
        }
        if self.life.vit.dead {
            self.proj_player.arrow_count = 0;
            self.proj_player.fall_flying = false;
        }
        self.fall_flying_tick();
        self.proj_use_tick();
    }

    /// Movement received from the client.
    pub(crate) fn proj_known_move(&mut self, before: [f64; 3]) {
        self.proj_player.known_move = [self.pos[0] - before[0], self.pos[1] - before[1], self.pos[2] - before[2]];
        self.proj_player.moved_this_tick = true;
        if self.on_ground {
            self.proj_player.fall_flying = false;
        }
    }

    /// End of the client's tick: with no movement received, known movement zero.
    pub(crate) fn proj_tick_end(&mut self) {
        if !self.proj_player.moved_this_tick {
            self.proj_player.known_move = [0.0; 3];
        }
        self.proj_player.moved_this_tick = false;
    }

    /// Dimension change: projectiles of both worlds forgotten by the client.
    pub(crate) fn proj_forget_tracking(&mut self) {
        for p in &mut self.proj.list {
            p.track.seen = false;
        }
        for c in &mut self.proj.clouds {
            c.seen = false;
        }
        self.proj.hook = None;
    }

    /// One tick of a projectile according to its kind.
    fn proj_tick(&mut self, p: &mut Proj) {
        match p.kind {
            k if k.arrow_like() => self.arrow_tick(p),
            PKind::Firework => self.firework_tick(p),
            k if k.hurting() => self.fireball_tick(p),
            PKind::ShulkerBullet => self.shulker_bullet_tick(p), // F15
            PKind::EnderPearl => self.pearl_tick(p),
            PKind::EyeOfEnder => self.eye_tick(p), // G5-F10
            _ => self.throwable_tick(p),
        }
    }

    /// The player touches the stuck arrows (box inflated by 1 / 0.5 / 1).
    pub(crate) fn proj_player_touch(&mut self) {
        if self.life.vit.dead || self.game_mode == 3 || !self.player_ready() {
            return;
        }
        let b = self.player_aabb();
        let reach = rc_physics::Aabb {
            min: [b.min[0] - 1.0, b.min[1] - 0.5, b.min[2] - 1.0],
            max: [b.max[0] + 1.0, b.max[1] + 0.5, b.max[2] + 1.0],
        };
        let mut list = std::mem::take(&mut self.proj.list);
        let mut changed = false;
        for p in list.iter_mut().filter(|p| !p.removed && p.kind.arrow_like()) {
            if !p.bbox().intersects(&reach) || !(p.in_ground || p.no_physics) || p.shake > 0 {
                continue;
            }
            // Trident: only its owner (or no owner).
            if p.kind == PKind::Trident && p.owner.is_some_and(|u| u != self.uuid) {
                continue;
            }
            let creative = self.game_mode == 1;
            let ok = match p.pickup {
                Pickup::Disallowed => false,
                Pickup::Allowed => self.proj_give(p.item.clone()),
                Pickup::CreativeOnly => creative,
            } || (p.kind == PKind::Trident
                && p.no_physics
                && p.owner == Some(self.uuid)
                && self.proj_give(p.item.clone()));
            if ok {
                let _ = self.send(Event::Misc(rc_client::misc::Misc::Take {
                    item: p.id,
                    collector: self.player_id,
                    amount: 1,
                }));
                p.removed = true;
                changed = true;
            }
        }
        list.extend(std::mem::take(&mut self.proj.list));
        self.proj.list = list;
        if changed {
            self.broadcast_menus();
        }
    }

    /// Adding an item to the inventory of a whole stack; true if everything fit.
    pub(crate) fn proj_give(&mut self, s: Stack) -> bool {
        let creative = self.game_mode == 1;
        let held = self.held as usize;
        self.pickup_into_inventory(s, creative, held).is_none()
    }
}
