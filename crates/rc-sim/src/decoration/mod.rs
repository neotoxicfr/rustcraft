//! Decorative entities simulated by the integrated server: paintings, frames (glowing or not),
//! armor stands, cushions, mannequins. Placement by the item, survival of what is hung,
//! breaking and loot, interactions, data sent to the client.
//!
//! The NBT compound kept in `Server::entities` remains the truth (save, `/data`, `/kill`,
//! selectors unchanged); `Decos` only holds the state the game does not save: counter of
//! the survival check and instant of the last hit dealt to an armor stand.

mod cushion;
mod hanging;
mod stand;
#[cfg(test)]
mod tests;

use std::collections::HashSet;

use rc_client::{EntityEvent, Event};
use rc_physics::Aabb;
use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use crate::blocks::{Dir, P};
use crate::nbtio::{self, int};
use crate::player::Stack;
use crate::server::Server;

/// Kind of decorative entity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DKind {
    Painting,
    Frame,
    GlowFrame,
    Stand,
    Cushion,
    Mannequin,
}

impl DKind {
    pub fn from_name(n: &str) -> Option<DKind> {
        Some(match n {
            "minecraft:painting" => DKind::Painting,
            "minecraft:item_frame" => DKind::Frame,
            "minecraft:glow_item_frame" => DKind::GlowFrame,
            "minecraft:armor_stand" => DKind::Stand,
            "minecraft:cushion" => DKind::Cushion,
            "minecraft:mannequin" => DKind::Mannequin,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            DKind::Painting => "minecraft:painting",
            DKind::Frame => "minecraft:item_frame",
            DKind::GlowFrame => "minecraft:glow_item_frame",
            DKind::Stand => "minecraft:armor_stand",
            DKind::Cushion => "minecraft:cushion",
            DKind::Mannequin => "minecraft:mannequin",
        }
    }

    /// Hung on a wall or ceiling (painting, frames).
    pub fn hanging(self) -> bool {
        matches!(self, DKind::Painting | DKind::Frame | DKind::GlowFrame)
    }

    pub fn frame(self) -> bool {
        matches!(self, DKind::Frame | DKind::GlowFrame)
    }

    /// Attached to a block: survival checked every 100 ticks.
    fn attached(self) -> bool {
        self.hanging() || self == DKind::Cushion
    }
}

/// Kind of an entity compound.
pub(crate) fn kind_of(e: &Nbt) -> Option<DKind> {
    e.get("id").and_then(Nbt::as_str).and_then(DKind::from_name)
}

/// Result of an item use on a block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Use {
    Fail,
    Consume,
    Success,
}

#[derive(Debug, Clone)]
struct Live {
    id: i32,
    kind: DKind,
    /// Chunk where the compound was last seen (fast lookup).
    chunk: (i32, i32),
    /// Ticks since the last survival check (not saved, 0 at creation).
    check: i32,
    /// Armor stand: instant of the last hit that made it wobble.
    last_hit: i64,
    removed: bool,
    /// Network tracking of a decoration that moves (armor stand, mannequin).
    track: crate::mobs::track::Tracker,
}

impl Live {
    fn new(id: i32, kind: DKind, chunk: (i32, i32)) -> Live {
        Live { id, kind, chunk, check: 0, last_hit: 0, removed: false, track: Default::default() }
    }
}

/// Fields of an entry read by the tick (without copying the network tracking).
struct Brief {
    id: i32,
    kind: DKind,
    check: i32,
    removed: bool,
}

/// Decorations of the open world (exchanged with the dimension).
#[derive(Default)]
pub(crate) struct Decos {
    live: Vec<Live>,
    /// Entity chunks already read (single registration).
    pub(crate) adopted: HashSet<(i32, i32)>,
    /// Cushion on which the player is sitting.
    pub seat: Option<i32>,
    /// Explosion about to go off: its non-mob direct source (TNT, projectile) is in
    /// water. Read and reset to false by the explosion.
    pub blast_wet: bool,
}

// ---------- Directions and boxes ----------

/// Direction read as a 3D value of the game (down, up, north, south, west, east), modulo 6.
pub(crate) fn dir_3d(v: i64) -> Dir {
    Dir::from_index(v.unsigned_abs() as usize % 6)
}

/// Direction read as a 2D value of the game (south, west, north, east), modulo 4.
pub(crate) fn dir_2d(v: i64) -> Dir {
    [Dir::South, Dir::West, Dir::North, Dir::East][v.unsigned_abs() as usize % 4]
}

/// 2D value of a horizontal direction (−1 for a vertical axis).
pub(crate) fn value_2d(d: Dir) -> i32 {
    match d {
        Dir::South => 0,
        Dir::West => 1,
        Dir::North => 2,
        Dir::East => 3,
        _ => -1,
    }
}

fn step(d: Dir) -> [f64; 3] {
    d.step().map(f64::from)
}

/// Centered box.
pub(crate) fn of_size(c: [f64; 3], s: [f64; 3]) -> Aabb {
    Aabb {
        min: [c[0] - s[0] / 2.0, c[1] - s[1] / 2.0, c[2] - s[2] / 2.0],
        max: [c[0] + s[0] / 2.0, c[1] + s[1] / 2.0, c[2] + s[2] / 2.0],
    }
}

/// Block center shifted towards the hanging wall (the center of a block).
fn against_wall(pos: P, d: Dir) -> [f64; 3] {
    let s = step(d);
    std::array::from_fn(|i| f64::from(pos[i]) + 0.5 + s[i] * -0.46875)
}

/// Box of a `w × h` painting hung at `pos`, face `d`.
pub(crate) fn painting_box(pos: P, d: Dir, w: i32, h: i32) -> Aabb {
    let off = |n: i32| if n % 2 == 0 { 0.5 } else { 0.0 };
    let wall = against_wall(pos, d);
    let left = step(d.counter_clockwise());
    let c = [wall[0] + left[0] * off(w), wall[1] + left[1] * off(w) + off(h), wall[2] + left[2] * off(w)];
    let x_axis = d.axis() == 0;
    let z_axis = d.axis() == 2;
    of_size(
        c,
        [if x_axis { 0.0625 } else { f64::from(w) }, f64::from(h), if z_axis { 0.0625 } else { f64::from(w) }],
    )
}

/// Box of a frame hung at `pos`, face `d`; a map enlarges it by a quarter.
pub(crate) fn frame_box(pos: P, d: Dir, map: bool) -> Aabb {
    let side = if map { 1.0 } else { 0.75 };
    let a = d.axis();
    of_size(
        against_wall(pos, d),
        [
            if a == 0 { 0.0625 } else { side },
            if a == 1 { 0.0625 } else { side },
            if a == 2 { 0.0625 } else { side },
        ],
    )
}

fn dbl(v: [f64; 3]) -> Nbt {
    Nbt::List(v.iter().map(|&x| Nbt::Double(x)).collect())
}

/// Common fields of a new entity in the game's order (writing without identifier):
/// identifier, position, zero motion, rotation, fire 0, air 300, UUID.
pub(crate) fn base_fields(k: DKind, pos: [f64; 3], yaw: f32, pitch: f32, uuid: u128) -> Vec<(String, Nbt)> {
    vec![
        ("id".into(), Nbt::String(k.name().into())),
        ("Pos".into(), dbl(pos)),
        ("Motion".into(), dbl([0.0; 3])),
        ("Rotation".into(), Nbt::List(vec![Nbt::Float(yaw), Nbt::Float(pitch)])),
        ("fall_distance".into(), Nbt::Double(0.0)),
        ("Fire".into(), Nbt::Short(0)),
        ("Air".into(), Nbt::Short(300)),
        ("OnGround".into(), Nbt::Byte(0)),
        ("Invulnerable".into(), Nbt::Byte(0)),
        ("PortalCooldown".into(), Nbt::Int(0)),
        ("UUID".into(), nbtio::uuid_nbt(uuid)),
    ]
}

/// Field inserted right after `after` (field order of the game), replaced if it exists.
fn put_after(n: &mut Nbt, after: &str, key: &str, v: Nbt) {
    let Nbt::Compound(f) = n else { return };
    if let Some(slot) = f.iter_mut().find(|(k, _)| k == key) {
        slot.1 = v;
        return;
    }
    let at = f.iter().position(|(k, _)| k == after).map_or(f.len(), |i| i + 1);
    f.insert(at, (key.to_owned(), v));
}

fn floor_pos(p: [f64; 3]) -> P {
    p.map(|v| v.floor() as i32)
}

fn chunk_of(p: [f64; 3]) -> (i32, i32) {
    ((p[0].floor() as i32) >> 4, (p[2].floor() as i32) >> 4)
}

/// Stack with the entity's custom name (custom name component).
fn named(id: &str, e: &Nbt) -> Stack {
    let mut s = Stack { id: id.to_owned(), count: 1, components: None };
    if let Some(n) = e.get("CustomName") {
        crate::items::set_component(&mut s, "minecraft:custom_name", n.clone());
    }
    s
}

fn flag(e: &Nbt, k: &str) -> bool {
    int(e, k).is_some_and(|v| v != 0)
}

/// Dye colors in the order of their identifiers.
pub(crate) const DYES: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];

impl Server {
    // ---------- Registry ----------

    /// Decorations read with entity chunks: registered (once per chunk).
    pub(crate) fn deco_adopt(&mut self, loaded: &[(i32, i32)]) {
        for p in loaded {
            if !self.deco.adopted.insert(*p) {
                continue;
            }
            let Some(list) = self.entities.get(p) else { continue };
            for (id, e) in list {
                if let Some(kind) = kind_of(e) {
                    self.deco.live.push(Live::new(*id, kind, *p));
                }
            }
        }
    }

    /// Entity summoned outside placement (`/summon`): registered if decorative.
    pub(crate) fn deco_register(&mut self, id: i32, e: &Nbt) {
        if let Some(kind) = kind_of(e) {
            let chunk = chunk_of(nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default());
            self.deco.live.push(Live::new(id, kind, chunk));
        }
    }

    /// Position of a decoration of the registry (searched first in its chunk).
    fn live_pos(&mut self, i: usize) -> Option<[f64; 3]> {
        let (lid, lchunk) = (self.deco.live[i].id, self.deco.live[i].chunk);
        let near = self.entities.get(&lchunk).and_then(|v| v.iter().find(|(x, _)| *x == lid));
        let pos = match near {
            Some((_, e)) => nbtio::f64s::<3>(e.get("Pos")),
            None => {
                let (c, e) = self
                    .entities
                    .iter()
                    .find_map(|(c, v)| v.iter().find(|(x, _)| *x == lid).map(|(_, e)| (*c, e)))?;
                let p = nbtio::f64s::<3>(e.get("Pos"));
                self.deco.live[i].chunk = c;
                p
            }
        };
        Some(pos.unwrap_or_default())
    }

    fn deco_live(&mut self, id: i32) -> Option<&mut Live> {
        self.deco.live.iter_mut().find(|l| l.id == id && !l.removed)
    }

    /// Compound of a present decoration.
    pub(crate) fn deco_nbt(&self, id: i32) -> Option<&Nbt> {
        let hint = self.deco.live.iter().find(|l| l.id == id).map(|l| l.chunk);
        self.deco_nbt_near(id, hint)
    }

    /// Compound of decoration `i` of the registry (known chunk first: called every tick).
    fn live_nbt(&self, i: usize) -> Option<&Nbt> {
        let l = &self.deco.live[i];
        self.deco_nbt_near(l.id, Some(l.chunk))
    }

    /// Searches first in chunk `hint`, otherwise in the whole world.
    fn deco_nbt_near(&self, id: i32, hint: Option<(i32, i32)>) -> Option<&Nbt> {
        let near = hint.and_then(|c| self.entities.get(&c)).and_then(|v| v.iter().find(|(i, _)| *i == id));
        near.or_else(|| self.entities.values().flat_map(|l| l.iter()).find(|(i, _)| *i == id)).map(|(_, n)| n)
    }

    /// Modifies the compound of a decoration; its chunk will be rewritten.
    fn deco_edit(&mut self, id: i32, f: impl FnOnce(&mut Nbt)) {
        let hint = self.deco.live.iter().find(|l| l.id == id).map(|l| l.chunk);
        if let Some(c) = hint
            && let Some((_, n)) = self.entities.get_mut(&c).and_then(|v| v.iter_mut().find(|(i, _)| *i == id))
        {
            f(n);
            self.life.drop_chunks.insert(c);
            return;
        }
        for (c, l) in self.entities.iter_mut() {
            if let Some((_, n)) = l.iter_mut().find(|(i, _)| *i == id) {
                f(n);
                self.life.drop_chunks.insert(*c);
                return;
            }
        }
    }

    /// Random version 4 UUID, IETF variant.
    pub(crate) fn deco_uuid(&mut self) -> u128 {
        let r = &mut self.life.rng;
        let u = (r.next_long() as u128) << 64 | r.next_long() as u64 as u128;
        let u = (u & !(0xF000u128 << 64)) | (0x4000u128 << 64);
        (u & !(0xC000_0000_0000_0000u128)) | 0x8000_0000_0000_0000u128
    }

    /// Adds a placed decoration: identifier, announcement to the client, registry.
    pub(crate) fn deco_spawn(&mut self, n: Nbt) -> i32 {
        self.next_entity += 1;
        let id = self.next_entity;
        let pos = nbtio::f64s::<3>(n.get("Pos")).unwrap_or_default();
        let c = chunk_of(pos);
        self.deco_announce(id, &n);
        self.deco_register(id, &n);
        self.entities.entry(c).or_default().push((id, n));
        self.life.drop_chunks.insert(c);
        id
    }

    /// Removes a decoration (`discard`/`kill`): compound, registry, client.
    pub(crate) fn deco_remove(&mut self, id: i32) {
        let mut at = None;
        for (c, l) in self.entities.iter_mut() {
            if let Some(i) = l.iter().position(|(x, _)| *x == id) {
                at = nbtio::f64s::<3>(l[i].1.get("Pos"));
                l.remove(i);
                self.life.drop_chunks.insert(*c);
                break;
            }
        }
        self.cushion_ejected(id, at);
        for l in self.deco.live.iter_mut().filter(|l| l.id == id) {
            l.removed = true;
        }
        let _ = self.send(Event::Entity(EntityEvent::Remove(vec![id])));
    }

    // ---------- Client ----------

    /// Appearance on the client of a decoration (or of any frozen entity): painting and frames
    /// announced at the corner of the hanging block with their 3D direction, like the game's server.
    pub(crate) fn deco_announce(&self, id: i32, e: &Nbt) {
        let types = self.data.registry("minecraft:entity_type");
        let Some(kind) =
            e.get("id").and_then(Nbt::as_str).and_then(|n| types?.entries.iter().position(|t| t == n))
        else {
            return;
        };
        let mut pos = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        let [yaw, pitch] = nbtio::f32s::<2>(e.get("Rotation")).unwrap_or_default();
        let mut data = int(e, "Facing").unwrap_or(0) as i32;
        if let Some(k) = kind_of(e)
            && k.hanging()
        {
            let (p, d) = self.hang_place(k, e);
            pos = p.map(f64::from);
            data = d as i32;
        }
        let add = rc_proto_v777::entity::AddEntity {
            id,
            uuid: nbtio::uuid(e.get("UUID")).unwrap_or(0),
            kind: kind as i32,
            pos,
            velocity: [0.0; 3],
            pitch,
            yaw,
            head_yaw: yaw,
            data,
        };
        let _ = self.send(Event::Entity(EntityEvent::Add(Box::new(add))));
        self.send_entity_meta(id, e);
    }

    /// Data specific to decorations, added to those of `send_entity_meta`.
    pub(crate) fn deco_meta(&self, e: &Nbt, values: &mut Vec<(u8, Meta)>) {
        let Some(k) = kind_of(e) else { return };
        match k {
            DKind::Painting | DKind::Frame | DKind::GlowFrame => {
                let (_, d) = self.hang_place(k, e);
                if d != Dir::South {
                    values.push((8, Meta::Direction(d as i32)));
                }
                if k == DKind::Painting {
                    // Default value of the game: first variant of the registry.
                    if let Some(v) = self.painting_variant_index(e).filter(|&v| v != 0) {
                        values.push((9, Meta::Variant(v as i32, 34)));
                    }
                } else {
                    if let Some(s) = e.get("Item").and_then(Stack::from_nbt) {
                        values.push((9, Meta::Item(self.net(Some(&s)))));
                    }
                    let r = int(e, "ItemRotation").unwrap_or(0) as i32 % 8;
                    if r != 0 {
                        values.push((10, Meta::Int(r)));
                    }
                }
            }
            DKind::Stand => stand::meta(e, values),
            DKind::Cushion => {
                let c = cushion::color(e);
                if c != 0 {
                    values.push((8, Meta::Variant(c as i32, 43)));
                }
            }
            DKind::Mannequin => mannequin_meta(e, values),
        }
    }

    /// Network data of a changed decoration.
    fn deco_data(&self, id: i32, values: Vec<(u8, Meta)>) {
        if !values.is_empty() {
            let _ = self.send(Event::Entity(EntityEvent::Data { id, values }));
        }
    }

    // ---------- Sounds, loot ----------

    /// Sound emitted by the entity: silent if `Silent`.
    fn deco_sound(&mut self, e: &Nbt, name: &str, category: i32, volume: f32, pitch: f32) {
        if flag(e, "Silent") {
            return;
        }
        let pos = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        self.world_sound(name, category, pos, volume, pitch);
    }

    /// Item dropped by a hung entity: shifted by 0.15 towards the face.
    fn deco_drop_at(&mut self, e: &Nbt, d: Option<Dir>, s: Stack) {
        let mut pos = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        if let Some(d) = d {
            let st = step(d);
            pos[0] += st[0] * f64::from(0.15f32);
            pos[2] += st[2] * f64::from(0.15f32);
        }
        self.spawn_item_random(pos, s, 10);
    }

    /// Block shards (`block` particle).
    fn block_particles(&self, state: u32, pos: [f64; 3], delta: [f32; 3], speed: f32, count: i32) {
        let Some(kind) = self
            .data
            .registry("minecraft:particle_type")
            .and_then(|r| r.entries.iter().position(|e| e == "minecraft:block"))
        else {
            return;
        };
        let p = rc_proto_v777::entity::LevelParticles {
            particle: rc_proto_v777::entity::Particle {
                kind: kind as i32,
                color: 0,
                state: state as i32,
                item: -1,
                scale: 1.0,
                color2: 0,
                vib: None,
            },
            force: false,
            always_visible: false,
            pos,
            delta,
            speed: [speed; 3],
            count,
            mode: 0,
        };
        let _ = self.send(Event::Particles(p));
    }

    // ---------- Tick ----------

    /// Tick of the decorations (entity phase): under the world, survival every 100 ticks.
    pub(crate) fn deco_tick(&mut self) {
        self.cushion_seat_tick(); // seated player: it ticks even when the world is frozen
        if !self.cmd.ticking.runs() || self.deco.live.is_empty() {
            return;
        }
        let floor = f64::from(self.dim.height.min_y - 64);
        for i in 0..self.deco.live.len() {
            let l = &self.deco.live[i];
            let l = Brief { id: l.id, kind: l.kind, check: l.check, removed: l.removed };
            if l.removed {
                continue;
            }
            let Some(pos) = self.live_pos(i) else {
                self.deco.live[i].removed = true;
                continue;
            };
            // "Entity ticking" chunk: simulated and its 8 neighbours loaded; otherwise a support
            // located in an absent neighbour would be read as void.
            let c = chunk_of(pos);
            let loaded = (-1..=1).all(|dx| (-1..=1).all(|dz| matches!(self.chunks.get(&(c.0 + dx, c.1 + dz)), Some(Some(_)))));
            if !self.mob_chunk_ticking(c) || !loaded {
                continue;
            }
            if pos[1] < floor {
                self.deco_remove(l.id);
                continue;
            }
            if !l.kind.attached() {
                self.living_tick(i, pos); // armor stand, mannequin: fall
                continue;
            }
            self.deco.live[i].check = l.check + 1;
            if l.check < 100 {
                continue;
            }
            self.deco.live[i].check = 0;
            if l.kind == DKind::Cushion {
                self.cushion_fire_check(l.id, true);
                if self.deco.live[i].removed {
                    continue;
                }
            }
            if !self.deco_survives(l.id) {
                let e = self.deco_nbt(l.id).cloned();
                self.deco_remove(l.id);
                if let Some(e) = e {
                    self.deco_drop_item(&e, None);
                }
            }
        }
        if self.deco.live.iter().any(|l| l.removed) {
            self.deco.live.retain(|l| !l.removed);
        }
    }

    /// `survives` of an attached entity.
    pub(crate) fn deco_survives(&self, id: i32) -> bool {
        let Some(e) = self.deco_nbt(id) else { return false };
        match kind_of(e) {
            Some(k) if k.hanging() => self.hang_survives(id, k, e),
            Some(DKind::Cushion) => self.cushion_survives(e),
            _ => true,
        }
    }

    /// Loot of a removed attached entity; `creative`: broken by a player in
    /// creative (nothing drops).
    fn deco_drop_item(&mut self, e: &Nbt, by: Option<crate::mobs::mob::Ent>) {
        let creative = by == Some(crate::mobs::mob::Ent::Player) && self.game_mode == 1;
        match kind_of(e) {
            Some(DKind::Painting) => {
                if self.rule("minecraft:entity_drops", true) {
                    self.deco_sound(e, "minecraft:entity.painting.break", 6, 1.0, 1.0);
                    if !creative {
                        let d = self.hang_place(DKind::Painting, e).1;
                        self.deco_drop_at(e, Some(d), named("minecraft:painting", e));
                    }
                }
            }
            Some(k) if k.frame() => self.frame_broken(k, e, by),
            Some(DKind::Cushion) => self.cushion_dropped(e, by),
            _ => {}
        }
    }

    // ---------- Explosions ----------

    /// Blast of an explosion on the decorations: hung items
    /// and cushions broken (item dropped), armor stands smashed (equipment
    /// dropped); `by`: author (entity behind the damage source); `wet`: direct source in water, nothing
    /// hung is then touched; `blocklike`: the explosion touches block-like entities.
    pub(crate) fn deco_explosion(
        &mut self,
        center: [f64; 3],
        radius: f32,
        by: Option<crate::mobs::mob::Ent>,
        wet: bool,
        blocklike: bool,
    ) {
        if radius < 1.0e-5 || self.deco.live.is_empty() {
            return;
        }
        let double = f64::from(radius * 2.0);
        let by_mob = matches!(by, Some(crate::mobs::mob::Ent::Mob(_)));
        let griefing = self.rule("minecraft:mob_griefing", true);
        let ids: Vec<(i32, DKind)> = self.deco.live.iter().filter(|l| !l.removed).map(|l| (l.id, l.kind)).collect();
        for (id, k) in ids {
            let Some(e) = self.deco_nbt(id).cloned() else { continue };
            let ignore = match k {
                DKind::Stand => !blocklike || flag(&e, "Invisible"),
                DKind::Mannequin => true,
                _ => wet || !blocklike,
            };
            if ignore {
                continue;
            }
            let Some(b) = self.deco_box(&e) else { continue };
            let pos = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
            let d = [pos[0] - center[0], pos[1] - center[1], pos[2] - center[2]];
            let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() / double;
            if dist > 1.0 {
                continue;
            }
            let eye = match k {
                DKind::Painting => f64::from(0.5f32 * 0.85),
                DKind::Frame | DKind::GlowFrame => 0.0,
                DKind::Cushion => f64::from(0.25f32 * 0.85),
                DKind::Stand => {
                    if flag(&e, "Small") {
                        f64::from(0.9875f32)
                    } else {
                        f64::from(1.7775f32)
                    }
                }
                DKind::Mannequin => f64::from(1.62f32),
            };
            let o = [pos[0] - center[0], pos[1] + eye - center[1], pos[2] - center[2]];
            let olen = (o[0] * o[0] + o[1] * o[1] + o[2] * o[2]).sqrt();
            let exposure = f64::from(self.seen_percent(center, &b));
            // The hit: refused for a mob without the `mob_griefing` rule, or invulnerable.
            let refused = (by_mob && !griefing) || flag(&e, "Invulnerable");
            if k == DKind::Stand {
                if !refused && !flag(&e, "Marker") {
                    let at = pos.map(|v| v.floor() as i32);
                    self.stand_broken(&e, at);
                    self.deco_remove(id);
                    self.game_event_at("minecraft:entity_die", pos, None);
                }
                continue;
            }
            if !refused {
                self.attached_kill_by(id, &e, by); // loot attributed to the author (creative: nothing)
                continue;
            }
            // Non-zero push: the hung item is knocked off (fixed frame excepted).
            let pushed = olen >= 1.0e-5 && (1.0 - dist) * exposure != 0.0;
            if pushed && !(k.frame() && flag(&e, "Fixed")) {
                self.attached_kill_by(id, &e, None);
            }
        }
    }

    // ---------- Player ----------

    /// Item that places an entity, used on a block; true if the item is one
    /// (even refused). `at`: absolute touched point.
    pub(crate) fn deco_use_on(&mut self, slot: usize, st: &Stack, clicked: P, face: Dir, at: [f64; 3]) -> bool {
        let r = match self.hanging_use_on(slot, st, clicked, face) {
            Some(r) => r,
            None => match self.stand_use_on(slot, st, clicked, face) {
                Some(r) => r,
                None => match self.cushion_use_on(slot, st, clicked, face, at) {
                    Some(r) => r,
                    None => return false,
                },
            },
        };
        // "Used" statistic for a successful or consumed use.
        if r != Use::Fail {
            self.award_stat("minecraft:used", &st.id, 1);
        }
        self.send_slot(slot);
        true
    }

    /// Entity removed by a command (`/kill`): leaves the registry.
    pub(crate) fn deco_forget(&mut self, id: i32) {
        for l in self.deco.live.iter_mut().filter(|l| l.id == id) {
            l.removed = true;
        }
    }

    /// Player hit on a decoration; true if the entity is one (handled).
    pub(crate) fn deco_attacked(&mut self, id: i32) -> bool {
        let Some(kind) = self.deco.live.iter().find(|l| l.id == id && !l.removed).map(|l| l.kind) else {
            return false;
        };
        if self.game_mode == 3 || self.life.vit.dead || !self.deco_in_reach(id) {
            return true;
        }
        match kind {
            DKind::Stand => self.stand_attacked(id),
            DKind::Mannequin => {}
            k => {
                // Hit at 0 if the block is reachable.
                let Some(e) = self.deco_nbt(id).cloned() else { return true };
                let pos = self.attached_pos(k, &e);
                if self.may_interact(pos) {
                    self.attached_hurt_by_player(id, k, &e);
                }
            }
        }
        true
    }

    /// Player right click on a decoration; true if the entity is one (handled).
    pub(crate) fn deco_interact(&mut self, id: i32, hand: i32, at: [f64; 3], secondary: bool) -> bool {
        let Some(kind) = self.deco.live.iter().find(|l| l.id == id && !l.removed).map(|l| l.kind) else {
            return false;
        };
        if self.life.vit.dead || !self.deco_in_reach(id) {
            return true;
        }
        let slot = if hand == 0 { 36 + self.held as usize } else { 45 };
        let used = match kind {
            DKind::Frame | DKind::GlowFrame if self.game_mode != 3 => self.frame_interact(id, kind, slot),
            DKind::Stand if self.game_mode != 3 => self.stand_interact(id, slot, at),
            DKind::Cushion if self.game_mode != 3 => self.cushion_interact(id, secondary),
            _ => false,
        };
        // Unconsumed click on a living entity: the held item acts on it
        // (the interaction with a mob); only the name tag concerns stand and mannequin.
        if !used && self.game_mode != 3 && matches!(kind, DKind::Stand | DKind::Mannequin) {
            self.deco_name_tag(id, slot);
        }
        true
    }

    /// Name tag on a stand or a mannequin: name set, tag consumed (except in
    /// creative), `entity_interact` event.
    fn deco_name_tag(&mut self, id: i32, slot: usize) {
        let Some(name) = self.slots[slot]
            .as_ref()
            .filter(|s| s.id == "minecraft:name_tag")
            .and_then(|s| crate::items::component(s, "minecraft:custom_name"))
            .cloned()
        else {
            return;
        };
        let Some(e) = self.deco_nbt(id) else { return };
        let pos = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        // Data sent only if it changes.
        if e.get("CustomName") != Some(&name) {
            let set = name.clone();
            self.deco_edit(id, |e| put_after(e, "UUID", "CustomName", set));
            self.deco_data(id, vec![(2, Meta::Text(Some(name)))]);
        }
        self.consume_held(slot);
        self.send_slot(slot);
        self.game_event_at("minecraft:entity_interact", pos, Some(crate::mobs::mob::Ent::Player));
    }

    /// Interaction range (margin of 3) from the eyes.
    fn deco_in_reach(&self, id: i32) -> bool {
        let Some(e) = self.deco_nbt(id) else { return false };
        let Some(b) = self.deco_box(e) else { return false };
        let range = self.player_attribute("minecraft:entity_interaction_range") + 3.0;
        let eye = [self.pos[0], self.pos[1] + self.ent_eye_height(crate::commands::ents::E::Player), self.pos[2]];
        let d2: f64 = (0..3)
            .map(|a| {
                let c = eye[a].clamp(b.min[a], b.max[a]);
                (eye[a] - c) * (eye[a] - c)
            })
            .sum();
        d2 < range * range
    }

    /// Box of a decoration (selectors, range, overlaps).
    pub(crate) fn deco_box(&self, e: &Nbt) -> Option<Aabb> {
        let k = kind_of(e)?;
        let p = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
        let around = |w: f64, h: f64| Aabb { min: [p[0] - w / 2.0, p[1], p[2] - w / 2.0], max: [p[0] + w / 2.0, p[1] + h, p[2] + w / 2.0] };
        Some(match k {
            DKind::Painting => {
                let (pos, d) = self.hang_place(k, e);
                let (w, h) = self.painting_size(e);
                painting_box(pos, d, w, h)
            }
            DKind::Frame | DKind::GlowFrame => {
                let (pos, d) = self.hang_place(k, e);
                frame_box(pos, d, hanging::has_map(e))
            }
            DKind::Stand => {
                let (w, h) = stand::size(e);
                around(w, h)
            }
            DKind::Cushion => around(1.0, 0.25),
            DKind::Mannequin => around(0.6, 1.8),
        })
    }

    /// Block position of an attached entity (`pos`).
    fn attached_pos(&self, k: DKind, e: &Nbt) -> P {
        if k.hanging() {
            self.hang_place(k, e).0
        } else {
            block_pos_of(e)
        }
    }
}

/// Saved `block_pos` if it is less than 16 blocks from the position, otherwise the block of
/// the position.
pub(crate) fn block_pos_of(e: &Nbt) -> P {
    let pos = nbtio::f64s::<3>(e.get("Pos")).unwrap_or_default();
    let own = floor_pos(pos);
    match e.get("block_pos") {
        Some(Nbt::IntArray(a)) if a.len() == 3 => {
            let d: i64 = (0..3).map(|i| i64::from(a[i] - own[i]).pow(2)).sum();
            if (d as f64) < 256.0 { [a[0], a[1], a[2]] } else { own }
        }
        _ => own,
    }
}

/// Saved profile (name alone, or compound: name, id, properties, texture, cape, elytra,
/// model) in the network encoding of the resolvable profile (serializer 41): full profile if name
/// and id are there, otherwise partial; then the four optional appearance fields. `None`: unreadable
/// form or empty profile (default value, not sent).
pub(crate) fn profile_bytes(p: &Nbt) -> Option<Vec<u8>> {
    use rc_proto::Encode;
    use rc_proto::varint::write_varint;
    use rc_proto::write_str;
    let opt_str = |w: &mut Vec<u8>, s: Option<&str>| {
        s.is_some().encode(w);
        if let Some(s) = s {
            write_str(w, s);
        }
    };
    let ident = |s: &str| if s.contains(':') { s.to_owned() } else { format!("minecraft:{s}") };
    let (name, id) = match p {
        Nbt::String(n) => (Some(n.as_str()), None),
        Nbt::Compound(_) => (p.get("name").and_then(Nbt::as_str), nbtio::uuid(p.get("id"))),
        _ => return None,
    };
    let mut w = Vec::new();
    if let (Some(n), Some(u)) = (name, id) {
        true.encode(&mut w);
        w.extend_from_slice(&u.to_be_bytes());
        write_str(&mut w, n);
    } else {
        false.encode(&mut w);
        opt_str(&mut w, name);
        id.is_some().encode(&mut w);
        if let Some(u) = id {
            w.extend_from_slice(&u.to_be_bytes());
        }
    }
    // Properties: list of {name, value, signature?} or table name → values.
    let mut props: Vec<(&str, &str, Option<&str>)> = Vec::new();
    match p.get("properties") {
        Some(Nbt::List(l)) => {
            for q in l {
                if let (Some(n), Some(v)) = (q.get("name").and_then(Nbt::as_str), q.get("value").and_then(Nbt::as_str)) {
                    props.push((n, v, q.get("signature").and_then(Nbt::as_str)));
                }
            }
        }
        Some(Nbt::Compound(m)) => {
            for (n, vs) in m {
                props.extend(nbtio::list(Some(vs)).iter().filter_map(Nbt::as_str).map(|v| (n.as_str(), v, None)));
            }
        }
        _ => {}
    }
    write_varint(&mut w, props.len() as i32);
    for (n, v, s) in props {
        write_str(&mut w, n);
        write_str(&mut w, v);
        opt_str(&mut w, s);
    }
    for k in ["texture", "cape", "elytra"] {
        opt_str(&mut w, p.get(k).and_then(Nbt::as_str).map(ident).as_deref());
    }
    match p.get("model").and_then(Nbt::as_str) {
        Some(m @ ("slim" | "wide")) => {
            true.encode(&mut w);
            (m == "slim").encode(&mut w);
        }
        _ => false.encode(&mut w),
    }
    // Empty profile: equal to the mannequin's default.
    if w.iter().all(|&b| b == 0) { None } else { Some(w) }
}

/// Mannequin data: pose, arms, visible layers, immobility, description.
fn mannequin_meta(e: &Nbt, values: &mut Vec<(u8, Meta)>) {
    let pose = match e.get("pose").and_then(Nbt::as_str) {
        Some("fall_flying") => 1,
        Some("sleeping") => 2,
        Some("swimming") => 3,
        Some("crouching") => 5,
        _ => 0,
    };
    if pose != 0 {
        values.push((6, Meta::Pose(pose)));
    }
    if e.get("main_hand").and_then(Nbt::as_str) == Some("left") {
        values.push((15, Meta::Variant(0, 42)));
    }
    // Layers: all visible except those listed (cape, jacket, sleeves, legs, hat).
    let mut mask = 0x7Fi8;
    for h in nbtio::list(e.get("hidden_layers")) {
        let bit = match h.as_str() {
            Some("cape") => 0x01,
            Some("jacket") => 0x02,
            Some("left_sleeve") => 0x04,
            Some("right_sleeve") => 0x08,
            Some("left_pants_leg") => 0x10,
            Some("right_pants_leg") => 0x20,
            Some("hat") => 0x40,
            _ => 0,
        };
        mask &= !bit;
    }
    values.push((16, Meta::Byte(mask)));
    if let Some(b) = e.get("profile").and_then(profile_bytes) {
        values.push((17, Meta::Profile(b.into())));
    }
    if flag(e, "immovable") {
        values.push((18, Meta::Bool(true)));
    }
    if flag(e, "hide_description") {
        values.push((19, Meta::Text(None)));
    } else if let Some(d) = e.get("description") {
        values.push((19, Meta::Text(Some(d.clone()))));
    }
}
