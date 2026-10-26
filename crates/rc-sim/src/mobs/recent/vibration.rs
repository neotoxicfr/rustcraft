//! Game events and vibrations (`server-mobs.md` § 4.F14): broadcast of an
//! event to the listeners of neighboring sections (radius 16, 32 for the shrieker's shriek), "valid
//! vibration" filter (listening tag, spectator, careful step, dampening entity or block), wool
//! occlusion (six offset rays), selection of the closest candidate
//! at the next tick, one-tick-per-block travel (vibration particle at selection and at
//! reload), reception. Listeners: warden and allay (entities, radius 16, at eye
//! height), shrieker and sculk sensor (blocks, radius 8), calibrated sensor (block, radius 16).

use hashbrown::HashMap;

use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::{LevelParticles, Particle, VibDest};

use super::super::mob::{Ent, Mob};
use super::super::species::Kind;
use crate::nbtio;
use crate::server::Server;

/// Entity at the origin of an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Src {
    /// Living entity (player or mob).
    Living(Ent),
    /// Projectile, with its shooter.
    Projectile(Option<Ent>),
    /// Ground item (dampens if it is wool), with its owner.
    Item { owner: Option<Ent>, dampens: bool },
    /// Other entity (falling block, TNT…).
    Other,
}

impl Src {
    pub fn living(self) -> Option<Ent> {
        match self {
            Src::Living(e) => Some(e),
            _ => None,
        }
    }

    pub fn projectile_owner(self) -> Option<Ent> {
        match self {
            Src::Projectile(o) => o,
            _ => None,
        }
    }
}

/// Vibration.
#[derive(Debug, Clone, PartialEq)]
pub struct VibInfo {
    pub event: String,
    pub distance: f32,
    pub pos: [f64; 3],
    pub src: Option<Src>,
    pub src_uuid: Option<u128>,
    pub owner_uuid: Option<u128>,
}

/// Data of a vibration system.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VibData {
    pub current: Option<VibInfo>,
    pub travel: i32,
    /// Selected candidate and tick of its arrival.
    pub selector: Option<(VibInfo, i64)>,
    pub reload_particle: bool,
}

/// Listener registered in a section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Who {
    Mob(i32),
    Block([i32; 3]),
}

#[derive(Debug, Clone, Copy)]
pub struct Listener {
    pub who: Who,
    pub section: [i32; 3],
    pub order: u64,
}

/// Tests: event emitted (name, position, source, state).
#[cfg(test)]
pub type LoggedEvent = (String, [f64; 3], Option<Src>, Option<u32>);

/// Registry of listeners and state of vibration blocks.
#[derive(Debug, Clone, Default)]
pub struct Vibrations {
    pub listeners: Vec<Listener>,
    next: u64,
    /// Event tags resolved.
    tags: HashMap<String, Vec<String>>,
    /// Data of vibration blocks (shriekers) by position.
    pub blocks: HashMap<[i32; 3], VibData>,
    /// Tests: all events emitted (name, position, source, state).
    #[cfg(test)]
    pub log: Vec<LoggedEvent>,
}

fn section_of(p: [f64; 3]) -> [i32; 3] {
    [(p[0].floor() as i32) >> 4, (p[1].floor() as i32) >> 4, (p[2].floor() as i32) >> 4]
}

fn block_of(p: [f64; 3]) -> [i32; 3] {
    p.map(|v| v.floor() as i32)
}

fn dist_sqr_i(a: [i32; 3], b: [i32; 3]) -> f64 {
    (0..3).map(|i| f64::from(a[i] - b[i]).powi(2)).sum()
}

/// Frequency of an event.
pub fn frequency(event: &str) -> i32 {
    match event.trim_start_matches("minecraft:") {
        "step" | "swim" | "flap" => 1,
        "projectile_land" | "hit_ground" | "splash" | "bounce" => 2,
        "item_interact_finish" | "projectile_shoot" | "instrument_play" => 3,
        "entity_action" | "elytra_glide" | "unequip" => 4,
        "entity_dismount" | "equip" => 5,
        "entity_interact" | "shear" | "entity_mount" => 6,
        "entity_damage" => 7,
        "drink" | "eat" => 8,
        "container_close" | "block_close" | "block_deactivate" | "block_detach" => 9,
        "container_open" | "block_open" | "block_activate" | "block_attach" | "prime_fuse" | "note_block_play" => 10,
        "block_change" => 11,
        "block_destroy" | "fluid_pickup" => 12,
        "block_place" | "fluid_place" => 13,
        "entity_place" | "lightning_strike" | "teleport" => 14,
        "entity_die" | "explode" => 15,
        n => n.strip_prefix("resonate_").and_then(|v| v.parse().ok()).unwrap_or(0),
    }
}

/// Block that listens to vibrations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockEar {
    Shrieker,
    Sensor,
    Calibrated,
}

impl BlockEar {
    /// Listening radius: 8, 16 for the calibrated sensor.
    pub fn radius(self) -> i32 {
        if self == BlockEar::Calibrated { 16 } else { 8 }
    }

    /// Tag of the events listened to.
    fn tag(self) -> &'static str {
        if self == BlockEar::Shrieker { "minecraft:shrieker_can_listen" } else { "minecraft:vibrations" }
    }
}

/// Vibration block from its name (or the id of its block entity, identical).
pub fn block_ear(name: &str) -> Option<BlockEar> {
    match name {
        "minecraft:sculk_shrieker" => Some(BlockEar::Shrieker),
        "minecraft:sculk_sensor" => Some(BlockEar::Sensor),
        "minecraft:calibrated_sculk_sensor" => Some(BlockEar::Calibrated),
        _ => None,
    }
}

/// Resonance event of a frequency (1 to 15).
pub fn resonance_event(freq: i32) -> String {
    format!("minecraft:resonate_{freq}")
}

/// Notification radius of the event (shriek 32, jukebox 10, otherwise 16).
fn notification_radius(event: &str) -> i32 {
    match event {
        "minecraft:shriek" => 32,
        "minecraft:jukebox_play" | "minecraft:jukebox_stop_play" => 10,
        _ => 16,
    }
}

impl VibData {
    /// Same tick only, the closest, at equal distance
    /// the highest frequency.
    fn add_candidate(&mut self, v: VibInfo, tick: i64) {
        let replace = match &self.selector {
            None => true,
            Some((prev, t)) => {
                if tick != *t {
                    false
                } else if v.distance < prev.distance {
                    true
                } else if v.distance > prev.distance {
                    false
                } else {
                    frequency(&v.event) > frequency(&prev.event)
                }
            }
        };
        if replace {
            self.selector = Some((v, tick));
        }
    }

    // ---------- NBT (`listener`) ----------

    pub fn load(n: Option<&Nbt>) -> VibData {
        let Some(n) = n else { return VibData::default() };
        let info = |e: &Nbt| -> Option<VibInfo> {
            let event = e.get("game_event").and_then(Nbt::as_str)?.to_owned();
            let distance = match e.get("distance") {
                Some(Nbt::Float(f)) => *f,
                _ => 0.0,
            };
            let pos = match e.get("pos") {
                Some(Nbt::List(l)) if l.len() == 3 => {
                    let d = |x: &Nbt| if let Nbt::Double(v) = x { *v } else { 0.0 };
                    [d(&l[0]), d(&l[1]), d(&l[2])]
                }
                _ => return None,
            };
            let uuid = |k: &str| match e.get(k) {
                Some(Nbt::IntArray(v)) if v.len() == 4 => {
                    Some(v.iter().fold(0u128, |a, &x| (a << 32) | u128::from(x as u32)))
                }
                _ => None,
            };
            Some(VibInfo {
                event,
                distance,
                pos,
                src: None,
                src_uuid: uuid("source"),
                owner_uuid: uuid("projectile_owner"),
            })
        };
        let current = n.get("event").and_then(info);
        let selector = n.get("selector").and_then(|s| {
            let i = s.get("event").and_then(info)?;
            Some((i, nbtio::int(s, "tick").unwrap_or(-1)))
        });
        let travel = nbtio::int(n, "event_delay").unwrap_or(0) as i32;
        // Data read back: the vibration particle in progress is sent again.
        VibData { current, travel, selector, reload_particle: true }
    }

    pub fn save(&self) -> Nbt {
        let info = |i: &VibInfo| -> Nbt {
            let mut c = vec![
                ("game_event".into(), Nbt::String(i.event.clone())),
                ("distance".into(), Nbt::Float(i.distance)),
                ("pos".into(), Nbt::List(i.pos.iter().map(|v| Nbt::Double(*v)).collect())),
            ];
            let ints = |u: u128| Nbt::IntArray(vec![(u >> 96) as i32, (u >> 64) as i32, (u >> 32) as i32, u as i32]);
            if let Some(u) = i.src_uuid {
                c.push(("source".into(), ints(u)));
            }
            if let Some(u) = i.owner_uuid {
                c.push(("projectile_owner".into(), ints(u)));
            }
            Nbt::Compound(c)
        };
        let mut out = Vec::new();
        if let Some(c) = &self.current {
            out.push(("event".into(), info(c)));
        }
        let mut sel = Vec::new();
        if let Some((i, _)) = &self.selector {
            sel.push(("event".into(), info(i)));
        }
        sel.push(("tick".into(), Nbt::Long(self.selector.as_ref().map_or(-1, |s| s.1))));
        out.push(("selector".into(), Nbt::Compound(sel)));
        out.push(("event_delay".into(), Nbt::Int(self.travel)));
        Nbt::Compound(out)
    }
}

impl Server {
    // ---------- Tags ----------

    /// Event in a `#minecraft:…` tag (references resolved).
    pub(crate) fn game_event_tag(&mut self, tag: &str, event: &str) -> bool {
        if !self.mobs.rec.vib.tags.contains_key(tag) {
            let list = self.resolve_event_tag(tag, 0);
            self.mobs.rec.vib.tags.insert(tag.to_owned(), list);
        }
        self.mobs.rec.vib.tags.get(tag).is_some_and(|l| l.iter().any(|e| e == event))
    }

    fn resolve_event_tag(&self, tag: &str, depth: u32) -> Vec<String> {
        let id = tag.trim_start_matches('#').trim_start_matches("minecraft:");
        let Some(v) = self.life.pack.get("tags/game_event", id) else { return Vec::new() };
        let mut out = Vec::new();
        for e in v["values"].as_array().into_iter().flatten() {
            let Some(s) = e.as_str().or_else(|| e["id"].as_str()) else { continue };
            if let Some(sub) = s.strip_prefix('#') {
                if depth < 8 {
                    out.extend(self.resolve_event_tag(sub, depth + 1));
                }
            } else {
                out.push(s.to_owned());
            }
        }
        out
    }

    // ---------- Registry ----------

    /// Registration (or move) of a listener in the section of its position.
    pub(crate) fn listener_move(&mut self, who: Who, pos: [f64; 3]) {
        let section = section_of(pos);
        let v = &mut self.mobs.rec.vib;
        if let Some(l) = v.listeners.iter().find(|l| l.who == who)
            && l.section == section
        {
            return;
        }
        v.listeners.retain(|l| l.who != who);
        v.next += 1;
        let order = v.next;
        v.listeners.push(Listener { who, section, order });
    }

    pub(crate) fn listener_remove(&mut self, who: Who) {
        self.mobs.rec.vib.listeners.retain(|l| l.who != who);
    }

    // ---------- Emission ----------

    /// Event emitted by a mob at its position.
    pub(crate) fn mob_game_event(&mut self, m: &Mob, event: &str) {
        let pos = m.pos;
        self.game_event_src(event, pos, Some(Src::Living(Ent::Mob(m.id))), None);
    }

    /// Event at a position, with a possible living entity.
    pub(crate) fn game_event_at(&mut self, event: &str, pos: [f64; 3], src: Option<Ent>) {
        self.game_event_src(event, pos, src.map(Src::Living), None);
    }

    /// Block event (block center), with the touched state.
    pub(crate) fn game_event_block(&mut self, event: &str, p: [i32; 3], src: Option<Ent>, state: Option<u32>) {
        let pos = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
        self.game_event_src(event, pos, src.map(Src::Living), state);
    }

    /// Listeners of the sections within the radius (chunk order
    /// in X then Z, sections in Y, then registration order), each within range of its radius.
    pub(crate) fn game_event_src(&mut self, event: &str, pos: [f64; 3], src: Option<Src>, state: Option<u32>) {
        #[cfg(test)]
        self.mobs.rec.vib.log.push((event.to_owned(), pos, src, state));
        // Allays' jukebox listener: delivered before the vibrations (queue by distance).
        match event {
            "minecraft:jukebox_play" => self.bz_allays_hear_jukebox(block_of(pos), true),
            "minecraft:jukebox_stop_play" => self.bz_allays_hear_jukebox(block_of(pos), false),
            _ => {}
        }
        if self.mobs.rec.vib.listeners.is_empty() {
            return;
        }
        let r = notification_radius(event);
        let c = block_of(pos);
        let lo = [(c[0] - r) >> 4, (c[1] - r) >> 4, (c[2] - r) >> 4];
        let hi = [(c[0] + r) >> 4, (c[1] + r) >> 4, (c[2] + r) >> 4];
        let mut cands: Vec<Listener> = self
            .mobs
            .rec
            .vib
            .listeners
            .iter()
            .copied()
            .filter(|l| (0..3).all(|i| l.section[i] >= lo[i] && l.section[i] <= hi[i]))
            .collect();
        cands.sort_by_key(|l| (l.section[0], l.section[2], l.section[1], l.order));
        for l in cands {
            match l.who {
                Who::Mob(id) => {
                    let Some(i) = self.mob_index(id) else { continue };
                    let Some(mut m) = self.mobs.list[i].take() else { continue };
                    // Warden and allay: radius 16 at the eyes.
                    if matches!(m.kind, Kind::Warden | Kind::Allay) && !m.removed {
                        let lp = [m.pos[0], m.eye_y(), m.pos[2]];
                        if dist_sqr_i(block_of(lp), c) <= 16.0 * 16.0 {
                            let mut data = std::mem::take(&mut m.rec.vib);
                            self.vib_handle(Who::Mob(id), Some(&mut m), &mut data, lp, event, pos, src, state);
                            m.rec.vib = data;
                        }
                    }
                    self.mobs.list[i] = Some(m);
                }
                Who::Block(p) => {
                    let lp = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
                    let Some(radius) = block_ear(self.bname(p)).map(BlockEar::radius) else { continue };
                    if dist_sqr_i(block_of(lp), c) > f64::from(radius * radius) {
                        continue;
                    }
                    let mut data = self.mobs.rec.vib.blocks.remove(&p).unwrap_or_default();
                    self.vib_handle(Who::Block(p), None, &mut data, lp, event, pos, src, state);
                    self.mobs.rec.vib.blocks.insert(p, data);
                }
            }
        }
    }

    /// Reception of a game event by the vibration listener.
    #[allow(clippy::too_many_arguments)]
    fn vib_handle(
        &mut self,
        who: Who,
        m: Option<&mut Mob>,
        data: &mut VibData,
        dest: [f64; 3],
        event: &str,
        origin: [f64; 3],
        src: Option<Src>,
        state: Option<u32>,
    ) -> bool {
        if data.current.is_some() {
            return false;
        }
        let ear = match who {
            Who::Block(p) => block_ear(self.bname(p)),
            Who::Mob(_) => None,
        };
        let allay = m.as_deref().is_some_and(|w| w.kind == Kind::Allay);
        let listen_tag = match (who, ear) {
            (Who::Mob(_), _) if allay => "minecraft:allay_can_listen",
            (Who::Mob(_), _) => "minecraft:warden_can_listen",
            (Who::Block(_), Some(e)) => e.tag(),
            (Who::Block(_), None) => return false,
        };
        // "Avoid vibration" advancement: warden and sensors only.
        let avoid = match who {
            Who::Mob(_) => !allay,
            Who::Block(_) => ear != Some(BlockEar::Shrieker),
        };
        if !self.vib_valid(listen_tag, event, src, state, m.as_deref(), avoid) {
            return false;
        }
        let ok = match (who, m) {
            (Who::Mob(_), Some(w)) if w.kind == Kind::Allay => {
                super::super::brainy::allay::can_receive_vibration(self, w, block_of(origin))
            }
            (Who::Mob(_), Some(w)) => super::warden::can_receive_vibration(self, w, src),
            (Who::Block(p), _) if ear == Some(BlockEar::Shrieker) => super::shrieker::can_receive(self, p, src),
            (Who::Block(p), _) => crate::blocks::sculk_sensor::can_receive(self, p, block_of(origin), event),
            _ => false,
        };
        if !ok {
            return false;
        }
        if self.vib_occluded(origin, dest) {
            return false;
        }
        let d = ((0..3).map(|i| (origin[i] - dest[i]).powi(2)).sum::<f64>()).sqrt() as f32;
        let (src_uuid, owner_uuid) = self.vib_uuids(src);
        let info = VibInfo { event: event.to_owned(), distance: d, pos: origin, src, src_uuid, owner_uuid };
        let t = self.time;
        data.add_candidate(info, t);
        true
    }

    fn vib_uuids(&self, src: Option<Src>) -> (Option<u128>, Option<u128>) {
        let uuid = |e: Ent| match e {
            Ent::Player => Some(self.uuid),
            Ent::Mob(id) => self.mob_ref(id).map(|m| m.uuid),
        };
        match src {
            Some(Src::Living(e)) => (uuid(e), None),
            Some(Src::Projectile(o)) => (None, o.and_then(uuid)),
            _ => (None, None),
        }
    }

    /// A listener's valid-vibration test.
    fn vib_valid(
        &mut self,
        tag: &str,
        event: &str,
        src: Option<Src>,
        state: Option<u32>,
        me: Option<&Mob>,
        avoid: bool,
    ) -> bool {
        if !self.game_event_tag(tag, event) {
            return false;
        }
        match src {
            Some(Src::Living(Ent::Player)) => {
                if self.game_mode == 3 {
                    return false;
                }
                if self.sneaking && self.game_event_tag("minecraft:ignore_vibrations_sneaking", event) {
                    // The warden and the sensors trigger the "avoid vibration" advancement.
                    if avoid {
                        self.trig_simple("minecraft:avoid_vibration");
                    }
                    return false;
                }
            }
            Some(Src::Living(Ent::Mob(id))) => {
                if self.mob_ref(id).is_some_and(|o| o.kind == Kind::Warden) || me.is_some_and(|m| m.id == id) {
                    // The warden dampens vibrations (its own included).
                    return false;
                }
            }
            Some(Src::Item { dampens: true, .. }) => return false,
            _ => {}
        }
        if let Some(s) = state {
            let block = self.data.state(s).map_or(u32::MAX, |i| u32::from(i.block));
            if self.block_tags.get("minecraft:dampens_vibrations").is_some_and(|v| v.binary_search(&block).is_ok()) {
                return false;
            }
        }
        true
    }

    /// The six rays (start offset by 1e-5 toward each face) all meet
    /// a wool block.
    fn vib_occluded(&self, origin: [f64; 3], dest: [f64; 3]) -> bool {
        let from = origin.map(|v| v.floor() + 0.5);
        let to = dest.map(|v| v.floor() + 0.5);
        const DIRS: [[f64; 3]; 6] =
            [[0.0, -1.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, -1.0], [0.0, 0.0, 1.0], [-1.0, 0.0, 0.0], [1.0, 0.0, 0.0]];
        for d in DIRS {
            let nudged = [
                from[0] + d[0] * f64::from(1.0e-5f32),
                from[1] + d[1] * f64::from(1.0e-5f32),
                from[2] + d[2] * f64::from(1.0e-5f32),
            ];
            if !self.block_in_line(nudged, to, "minecraft:occludes_vibration_signals") {
                return false;
            }
        }
        true
    }

    /// Does a traversed cell carry the tag?
    fn block_in_line(&self, from: [f64; 3], to: [f64; 3], tag: &str) -> bool {
        if from == to {
            return false;
        }
        let Some(set) = self.block_tags.get(tag) else { return false };
        let hit = |p: [i32; 3]| {
            let block = self.data.state(self.st(p)).map_or(u32::MAX, |i| u32::from(i.block));
            set.binary_search(&block).is_ok()
        };
        let lerp = |k: f64, a: f64, b: f64| a + k * (b - a);
        let te: [f64; 3] = std::array::from_fn(|i| lerp(-1.0e-7, to[i], from[i]));
        let fe: [f64; 3] = std::array::from_fn(|i| lerp(-1.0e-7, from[i], to[i]));
        let mut cell = fe.map(|v| v.floor() as i32);
        if hit(cell) {
            return true;
        }
        let d: [f64; 3] = std::array::from_fn(|i| te[i] - fe[i]);
        let sign = d.map(|v| {
            if v > 0.0 {
                1
            } else if v < 0.0 {
                -1
            } else {
                0
            }
        });
        let delta: [f64; 3] = std::array::from_fn(|i| if sign[i] == 0 { f64::MAX } else { f64::from(sign[i]) / d[i] });
        let frac = |v: f64| v - v.floor();
        let mut t: [f64; 3] =
            std::array::from_fn(|i| delta[i] * if sign[i] > 0 { 1.0 - frac(fe[i]) } else { frac(fe[i]) });
        while t[0] <= 1.0 || t[1] <= 1.0 || t[2] <= 1.0 {
            let a = if t[0] < t[1] {
                if t[0] < t[2] { 0 } else { 2 }
            } else if t[1] < t[2] {
                1
            } else {
                2
            };
            cell[a] += sign[a];
            t[a] += delta[a];
            if hit(cell) {
                return true;
            }
        }
        false
    }

    // ---------- Tick ----------

    /// Tick of the vibrations of a listening mob (warden at the start of its tick, allay at the
    /// end): listener at the eyes, particle following the mob.
    pub(crate) fn vib_tick_mob(&mut self, m: &mut Mob) {
        let mut data = std::mem::take(&mut m.rec.vib);
        let eye = m.eye_y();
        let dest = [m.pos[0], eye, m.pos[2]];
        let target = VibDest::Entity { id: m.id, y_offset: (eye - m.pos[1]) as f32 };
        let (_, received) = self.vib_tick(&mut data, dest, false, target);
        if let Some((info, owner)) = received {
            if m.kind == Kind::Allay {
                super::super::brainy::allay::on_receive_vibration(self, m, &info);
            } else {
                super::warden::on_receive_vibration(self, m, info, owner, &data);
            }
        }
        m.rec.vib = data;
    }

    /// Tick of the vibrations of a block (shrieker, sensor); its neighbors must tick. The
    /// changed data (selection, travel, reception) are copied back into the block entity.
    pub(crate) fn vib_tick_block(&mut self, p: [i32; 3]) {
        let mut data = self.mobs.rec.vib.blocks.remove(&p).unwrap_or_default();
        let dest = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
        let (changed, received) = self.vib_tick(&mut data, dest, true, VibDest::Block(p));
        self.mobs.rec.vib.blocks.insert(p, data);
        if let Some((info, owner)) = received {
            match block_ear(self.bname(p)) {
                Some(BlockEar::Shrieker) => super::shrieker::receive(self, p, &info, owner),
                Some(_) => crate::blocks::sculk_sensor::receive(self, p, &info, dest),
                None => {}
            }
        }
        if changed {
            self.vib_block_store(p);
        }
    }

    /// Forced vibration (step on a sensor): candidate added without validity or
    /// occlusion test, even if a vibration is already on its way.
    pub(crate) fn vib_force_schedule(&mut self, p: [i32; 3], event: &str, origin: [f64; 3], src: Option<Src>) {
        let dest = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
        let d = ((0..3).map(|i| (origin[i] - dest[i]).powi(2)).sum::<f64>()).sqrt() as f32;
        let (src_uuid, owner_uuid) = self.vib_uuids(src);
        let info = VibInfo { event: event.to_owned(), distance: d, pos: origin, src, src_uuid, owner_uuid };
        let t = self.time;
        self.mobs.rec.vib.blocks.entry(p).or_default().add_candidate(info, t);
    }

    /// Vibration particle toward `target`, starting from `at`, `ticks` of travel; true if a
    /// player within 32 blocks (center of their block) received it.
    fn vib_particle(&self, at: [f64; 3], target: VibDest, ticks: i32) -> bool {
        let feet = self.pos.map(|v| v.floor() + 0.5);
        let d2: f64 = (0..3).map(|i| (feet[i] - at[i]).powi(2)).sum();
        // Comparison false for a NaN position (zero initial travel): nothing is sent.
        if d2.partial_cmp(&(32.0 * 32.0)) != Some(std::cmp::Ordering::Less) {
            return false;
        }
        let Some(kind) = self
            .data
            .registry("minecraft:particle_type")
            .and_then(|r| r.entries.iter().position(|e| e == "minecraft:vibration"))
        else {
            return false;
        };
        let p = LevelParticles {
            particle: Particle {
                kind: kind as i32,
                color: 0,
                state: 0,
                item: -1,
                scale: 1.0,
                color2: 0,
                vib: Some((target, ticks)),
            },
            force: false,
            always_visible: false,
            pos: at,
            delta: [0.0; 3],
            speed: [0.0; 3],
            count: 1,
            mode: 0,
        };
        let _ = self.send(rc_client::Event::Particles(p));
        true
    }

    /// Selection, travel then reception: (data changed, vibration received).
    fn vib_tick(
        &mut self,
        data: &mut VibData,
        dest: [f64; 3],
        adjacent: bool,
        target: VibDest,
    ) -> (bool, Option<(VibInfo, Option<Ent>)>) {
        let now = self.time;
        let mut changed = false;
        if data.current.is_none()
            && let Some((info, t)) = data.selector.clone()
            && t < now
        {
            data.travel = info.distance.floor() as i32;
            self.vib_particle(info.pos, target, data.travel);
            data.current = Some(info);
            data.selector = None;
            changed = true;
        }
        let Some(info) = data.current.clone() else { return (changed, None) };
        changed |= data.travel > 0;
        if data.reload_particle {
            // Data read back: the particle restarts from the position reached.
            let initial = info.distance.floor() as i32;
            let alpha = 1.0 - f64::from(data.travel) / f64::from(initial);
            let at: [f64; 3] = std::array::from_fn(|i| info.pos[i] + alpha * (dest[i] - info.pos[i]));
            if self.vib_particle(at, target, data.travel) {
                data.reload_particle = false;
            }
        }
        data.travel = (data.travel - 1).max(0);
        if data.travel > 0 {
            return (changed, None);
        }
        if adjacent {
            let c = block_of(dest);
            let (cx, cz) = (c[0] >> 4, c[2] >> 4);
            let ok = (cx - 1..=cx + 1)
                .all(|x| (cz - 1..=cz + 1).all(|z| self.chunks.get(&(x, z)).is_some_and(Option::is_some)));
            if !ok {
                return (changed, None);
            }
        }
        let owner = info.src.and_then(Src::projectile_owner).or_else(|| {
            info.owner_uuid.and_then(|u| {
                if u == self.uuid { Some(Ent::Player) } else { self.mobs.get_uuid(u).map(|m| Ent::Mob(m.id)) }
            })
        });
        data.current = None;
        (true, Some((info, owner)))
    }

    /// Living source vibration, found again by its UUID if needed.
    pub(crate) fn vib_source(&self, info: &VibInfo) -> Option<Src> {
        if info.src.is_some() {
            return info.src;
        }
        let u = info.src_uuid?;
        if u == self.uuid {
            return Some(Src::Living(Ent::Player));
        }
        self.mobs.get_uuid(u).map(|m| Src::Living(Ent::Mob(m.id)))
    }
}
