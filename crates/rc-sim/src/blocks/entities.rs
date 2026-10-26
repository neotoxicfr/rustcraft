//! Entities and block entities needed by block behaviours: falling blocks
//! (`entities.md`, falling block), primed TNT and explosion, container inventories
//! (comparator outputs, hoppers, droppers).

use rc_client::{EntityEvent, Event};
use rc_physics::Aabb;
use rc_physics::body::Body;
use rc_proto::nbt::Nbt;

use super::{Dir, Kind, P, above, below, flag, rel};
use crate::mobs::recent::vibration::Src;
use crate::server::Server;

/// Entity simulated by the block behaviours.
#[derive(Debug, Clone)]
pub(crate) struct SimEntity {
    pub id: i32,
    pub kind: EntKind,
    pub pos: [f64; 3],
    pub old: [f64; 3],
    pub vel: [f64; 3],
    pub on_ground: bool,
    pub time: i32,
    pub removed: bool,
    /// Physics shared with the client (`rc_physics::body`).
    pub body: Option<Box<Body>>,
    /// Network tracking (official server rules).
    pub track: crate::mobs::track::Tracker,
    /// Falling block: fall distance.
    pub fall: f64,
    /// Falling block: dropped as an item when it cannot be placed (`DropItem`).
    pub drop_item: bool,
    /// Falling block: broken on arrival, without an item (`CancelDrop`).
    pub cancel_drop: bool,
    /// Falling block: damage per block of fall and cap (`HurtEntities`, anvil).
    pub hurt: Option<(f32, i32)>,
    /// Primed TNT: who primed it (the explosion is attributed to them).
    pub owner: Option<crate::mobs::mob::Ent>,
    /// Primed TNT that went through a portal ("took a portal" flag, not saved): its explosion does not
    /// break the Nether portal and loses no power there.
    pub used_portal: bool,
}

impl SimEntity {
    fn new(id: i32, kind: EntKind, pos: [f64; 3], vel: [f64; 3]) -> SimEntity {
        SimEntity {
            id,
            kind,
            pos,
            old: pos,
            vel,
            on_ground: false,
            time: 0,
            removed: false,
            body: None,
            track: Default::default(),
            fall: 0.0,
            drop_item: true,
            cancel_drop: false,
            hurt: None,
            owner: None,
            used_portal: false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub(crate) enum EntKind {
    /// Falling block: carried state.
    Falling(u32),
    /// Primed TNT: remaining fuse.
    Tnt(i32),
}

#[derive(Default)]
pub(crate) struct Entities {
    pub list: Vec<SimEntity>,
    pub next_id: i32,
}

/// Block-entity id of a block (those the behaviours create).
pub(crate) fn block_entity_id(block: &str) -> Option<&'static str> {
    let n = block.strip_prefix("minecraft:").unwrap_or(block);
    Some(match n {
        "comparator" => "minecraft:comparator",
        "hopper" => "minecraft:hopper",
        "dispenser" => "minecraft:dispenser",
        "dropper" => "minecraft:dropper",
        "chest" => "minecraft:chest",
        "trapped_chest" => "minecraft:trapped_chest",
        "barrel" => "minecraft:barrel",
        "furnace" => "minecraft:furnace",
        "blast_furnace" => "minecraft:blast_furnace",
        "smoker" => "minecraft:smoker",
        "daylight_detector" => "minecraft:daylight_detector",
        "crafter" => "minecraft:crafter",
        _ if n.ends_with("shulker_box") => "minecraft:shulker_box",
        // S5: families whose block entity does not carry the block's name.
        "command_block" | "chain_command_block" | "repeating_command_block" => "minecraft:command_block",
        _ if n.ends_with("_hanging_sign") => "minecraft:hanging_sign",
        _ if n.ends_with("_sign") => "minecraft:sign",
        _ if n.ends_with("_bed") => "minecraft:bed",
        _ if n.ends_with("_banner") => "minecraft:banner",
        _ if n.ends_with("_skull") || n.ends_with("_head") => "minecraft:skull",
        "campfire" | "soul_campfire" => "minecraft:campfire",
        "beehive" | "bee_nest" => "minecraft:beehive",
        "spawner" => "minecraft:mob_spawner",
        "suspicious_sand" | "suspicious_gravel" => "minecraft:brushable_block",
        "beacon"
        | "brewing_stand"
        | "enchanting_table"
        | "jukebox"
        | "lectern"
        | "structure_block"
        | "jigsaw"
        | "bell"
        | "conduit"
        | "decorated_pot"
        | "chiseled_bookshelf"
        | "sculk_sensor"
        | "calibrated_sculk_sensor"
        | "sculk_catalyst"
        | "sculk_shrieker"
        | "trial_spawner"
        | "vault"
        | "creaking_heart"
        | "end_gateway"
        | "end_portal"
        | "ender_chest"
        | "test_block"
        | "test_instance_block"
        | "potent_sulfur" => return Some(leak_name(n)),
        // Wooden shelves: a single block-entity type for all wood species.
        _ if n.ends_with("_shelf") => "minecraft:shelf",
        // F06: copper chests (block entity "chest") and copper golem statues.
        _ if n.ends_with("copper_chest") => "minecraft:chest",
        _ if n.ends_with("copper_golem_statue") => "minecraft:copper_golem_statue",
        // G6-F01: wooden shelves.
        _ if n.ends_with("_shelf") => "minecraft:shelf",
        _ => return None,
    })
}

/// Container family: its own face and placement rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Cont {
    Plain,
    Furnace,
    Brewing,
    Shulker,
    Jukebox,
    Pot,
    Crafter,
}

/// All slots, in order (container without a face rule).
const FLAT: [usize; 27] = {
    let mut a = [0; 27];
    let mut i = 0;
    while i < 27 {
        a[i] = i;
        i += 1;
    }
    a
};

/// Slots offered by a face; without a face, or an ordinary container:
/// all of them. Furnace: top the ingredient, bottom the result then the fuel, sides the
/// fuel. Brewing stand: top the reagent, bottom the bottles then the reagent, sides the
/// bottles and the powder.
fn face_slots(kind: Cont, size: usize, face: Option<Dir>) -> &'static [usize] {
    match (kind, face) {
        (Cont::Furnace, Some(Dir::Up)) => &[0],
        (Cont::Furnace, Some(Dir::Down)) => &[2, 1],
        (Cont::Furnace, Some(_)) => &[1],
        (Cont::Brewing, Some(Dir::Up)) => &[3],
        (Cont::Brewing, Some(Dir::Down)) => &[0, 1, 2, 3],
        (Cont::Brewing, Some(_)) => &[0, 1, 2, 4],
        _ => &FLAT[..size.min(27)],
    }
}

/// NBT key of single-slot containers (jukebox: disc, decorated pot: item); `Items`
/// for the others.
fn single_item_key(kind: Cont) -> Option<&'static str> {
    match kind {
        Cont::Jukebox => Some("RecordItem"),
        Cont::Pot => Some("item"),
        _ => None,
    }
}

/// `minecraft:<name>` id with a static lifetime (small fixed set of names).
fn leak_name(n: &str) -> &'static str {
    use hashbrown::HashMap;
    use std::sync::{Mutex, OnceLock};
    static NAMES: OnceLock<Mutex<HashMap<String, &'static str>>> = OnceLock::new();
    let m = NAMES.get_or_init(|| Mutex::new(HashMap::new()));
    let mut g = m.lock().unwrap_or_else(|e| e.into_inner());
    g.entry(n.to_owned()).or_insert_with(|| Box::leak(format!("minecraft:{n}").into_boxed_str()))
}

const FALLING_SIZE: f64 = 0.98;

impl Server {
    pub(crate) fn alloc_entity(&mut self) -> i32 {
        let e = &mut self.bs.entities;
        if e.next_id == 0 {
            e.next_id = 0x4000_0000;
        }
        e.next_id += 1;
        e.next_id
    }

    fn entity_type(&self, name: &str) -> i32 {
        self.data.registry("minecraft:entity_type").and_then(|r| r.entries.iter().position(|e| e == name)).unwrap_or(0)
            as i32
    }

    pub(crate) fn add_entity_packet(&self, e: &SimEntity, kind: &str, data: i32) {
        let add = rc_proto_v777::entity::AddEntity {
            id: e.id,
            uuid: (e.id as u128) << 64 | 0x5333,
            kind: self.entity_type(kind),
            pos: e.pos,
            velocity: e.vel,
            pitch: 0.0,
            yaw: 0.0,
            head_yaw: 0.0,
            data,
        };
        let _ = self.send(Event::Entity(EntityEvent::Add(Box::new(add))));
    }

    /// The block at `p` starts to fall.
    pub(crate) fn spawn_falling_block(&mut self, p: P, s: u32) {
        let carried = if self.v().get_bool(s, "waterlogged") { self.v().set_bool(s, "waterlogged", false) } else { s };
        let id = self.alloc_entity();
        let pos = [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5];
        let mut e = SimEntity::new(id, EntKind::Falling(carried), pos, [0.0; 3]);
        // The anvil hurts what it crushes (2 per block, 40 at most); suspicious sand and gravel
        // always break on arrival.
        match self.state_name(carried) {
            "minecraft:anvil" | "minecraft:chipped_anvil" | "minecraft:damaged_anvil" => e.hurt = Some((2.0, 40)),
            "minecraft:suspicious_sand" | "minecraft:suspicious_gravel" => e.cancel_drop = true,
            _ => {}
        }
        let f = self.fluid_of(s);
        let rest = if f.is_empty() { self.air() } else { self.legacy_block(f) };
        self.set_block(p, rest, flag::ALL);
        self.add_entity_packet(&e, "minecraft:falling_block", carried as i32);
        self.bs.entities.list.push(e);
    }

    /// Priming of a TNT at `p`.
    pub(crate) fn tnt_prime(&mut self, p: P) {
        self.spawn_tnt(p, 80, None);
    }

    /// Priming attributed to `by` (the explosion will be attributed to them).
    pub(crate) fn tnt_prime_by(&mut self, p: P, by: Option<crate::mobs::mob::Ent>) {
        self.spawn_tnt(p, 80, by);
    }

    fn spawn_tnt(&mut self, p: P, fuse: i32, owner: Option<crate::mobs::mob::Ent>) {
        let rot = self.bs.rng.next_double() * std::f64::consts::TAU;
        let id = self.alloc_entity();
        let pos = [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5];
        let vel = [-rot.sin() * 0.02, 0.2f32 as f64, -rot.cos() * 0.02];
        let mut e = SimEntity::new(id, EntKind::Tnt(fuse), pos, vel);
        e.owner = owner;
        self.add_entity_packet(&e, "minecraft:tnt", 0);
        // Fuse different from the default value (chain: 10 to 29 ticks): it leaves with the
        // pairing data, otherwise the client believes 80 ticks remain.
        if fuse != 80 {
            let _ = self.send(Event::Entity(EntityEvent::Data {
                id,
                values: vec![(8, rc_proto_v777::entity::Meta::Int(fuse))],
            }));
        }
        self.bs.entities.list.push(e);
    }

    /// Primed TNT or falling block summoned (`/summon`): simulated like those of the blocks, with the
    /// velocity (`Motion`, component beyond 10 read as 0), the fuse (`fuse`, 80) or the state
    /// (`BlockState`, sand by default and in place of air) and the duration (`Time`) of the NBT.
    pub(crate) fn summon_sim_entity(&mut self, ty: &str, pos: [f64; 3], nbt: Option<&Nbt>) -> Option<i32> {
        let kind = match ty {
            "minecraft:tnt" => EntKind::Tnt(nbt.and_then(|n| crate::nbtio::int(n, "fuse")).unwrap_or(80) as i32),
            "minecraft:falling_block" => {
                let sand = self.states.state_of(&self.data, &Nbt::String("minecraft:sand".into()))?;
                let s = nbt
                    .and_then(|n| n.get("BlockState"))
                    .and_then(|e| self.states.state_of(&self.data, e))
                    .filter(|&s| !self.v().is_air(s))
                    .unwrap_or(sand);
                EntKind::Falling(s)
            }
            _ => return None,
        };
        let vel = nbt.and_then(|n| crate::nbtio::motion(n.get("Motion"))).unwrap_or_default();
        let id = self.alloc_entity();
        let mut e = SimEntity::new(id, kind, pos, vel);
        e.on_ground = nbt.and_then(|n| n.get("OnGround")).is_some_and(|v| matches!(v, Nbt::Byte(1)));
        e.time = nbt.and_then(|n| crate::nbtio::int(n, "Time")).unwrap_or(0) as i32;
        if let EntKind::Falling(s) = kind {
            // Falling-block flags: the anvil hurts by default, but at 0 per block without
            // `FallHurtAmount`.
            let flag = |k: &str| nbt.and_then(|n| n.get(k)).map(|v| matches!(v, Nbt::Byte(b) if *b != 0));
            let anvil =
                matches!(self.state_name(s), "minecraft:anvil" | "minecraft:chipped_anvil" | "minecraft:damaged_anvil");
            if flag("HurtEntities").unwrap_or(anvil) {
                let per = match nbt.and_then(|n| n.get("FallHurtAmount")) {
                    Some(Nbt::Float(f)) => *f,
                    _ => 0.0,
                };
                let max = nbt.and_then(|n| crate::nbtio::int(n, "FallHurtMax")).unwrap_or(40) as i32;
                e.hurt = Some((per, max));
            }
            e.drop_item = flag("DropItem").unwrap_or(true);
            e.cancel_drop = flag("CancelDrop").unwrap_or(false);
            e.fall = match nbt.and_then(|n| n.get("fall_distance")) {
                Some(Nbt::Double(d)) => *d,
                _ => 0.0,
            };
        }
        match kind {
            EntKind::Tnt(_) => self.add_entity_packet(&e, "minecraft:tnt", 0),
            EntKind::Falling(s) => self.add_entity_packet(&e, "minecraft:falling_block", s as i32),
        }
        if let EntKind::Tnt(fuse) = kind {
            let _ = self.send(Event::Entity(EntityEvent::Data {
                id,
                values: vec![(8, rc_proto_v777::entity::Meta::Int(fuse))],
            }));
        }
        self.bs.entities.list.push(e);
        Some(id)
    }

    fn entity_box(e: &SimEntity) -> Aabb {
        let h = FALLING_SIZE / 2.0;
        Aabb { min: [e.pos[0] - h, e.pos[1], e.pos[2] - h], max: [e.pos[0] + h, e.pos[1] + FALLING_SIZE, e.pos[2] + h] }
    }

    /// Physics shared with the client (`rc_physics::body`), re-synced onto the entity.
    pub(super) fn body_of(&self, e: &mut SimEntity) -> Box<Body> {
        let eye = if matches!(e.kind, EntKind::Tnt(_)) { 0.15 } else { 0.833 };
        let mut b = e
            .body
            .take()
            .unwrap_or_else(|| Box::new(Body::new(e.pos, e.vel, FALLING_SIZE, FALLING_SIZE, f64::from(eye as f32))));
        (b.pos, b.vel, b.on_ground) = (e.pos, e.vel, e.on_ground);
        b.fast_lava = self.fast_lava();
        b.server = true;
        b
    }

    pub(super) fn put_body(e: &mut SimEntity, b: Box<Body>) {
        (e.pos, e.vel, e.on_ground) = (b.pos, b.vel, b.on_ground);
        e.body = Some(b);
    }

    /// Entity phase (§ 1.2, step 11b) for the entities simulated here.
    pub(crate) fn tick_sim_entities(&mut self) {
        let n = self.bs.entities.list.len();
        for i in 0..n {
            let mut e = self.bs.entities.list[i].clone();
            if e.removed {
                continue;
            }
            // Official server tracking, before the tick: TNT every 10 ticks, the falling
            // block every 20; the client simulates the rest with the same physics.
            let interval = if matches!(e.kind, EntKind::Tnt(_)) { 10 } else { 20 };
            let mut t = std::mem::take(&mut e.track);
            self.track_object(&mut t, e.id, e.pos, e.vel, e.on_ground, interval);
            e.track = t;
            e.old = e.pos;
            match e.kind {
                EntKind::Falling(s) => self.tick_falling(&mut e, s),
                EntKind::Tnt(fuse) => self.tick_tnt(&mut e, fuse),
            }
            self.bs.entities.list[i] = e;
        }
        let gone: Vec<i32> = self.bs.entities.list.iter().filter(|e| e.removed).map(|e| e.id).collect();
        if !gone.is_empty() {
            let _ = self.send(Event::Entity(EntityEvent::Remove(gone)));
            self.bs.entities.list.retain(|e| !e.removed);
        }
    }

    fn tick_falling(&mut self, e: &mut SimEntity, s: u32) {
        if self.v().is_air(s) {
            e.removed = true;
            return;
        }
        e.time += 1;
        let from = e.pos;
        let mut b = self.body_of(e);
        b.fall_move(&self.world(), &self.phys);
        let (mv, below, support) = (b.moved, b.below(&self.phys, &self.world(), 0.2), b.support());
        Self::put_body(e, b);
        // Fall distance, during the move as in the game (the falling block never
        // updates its "in water" state: the fall counts in water too, except when reset
        // by the ray cast of a move of one block or more). On
        // landing: damage from the anvil or the dripstone, anvil wear.
        if self.fall_step(&mut e.fall, from, mv, false, e.on_ground) {
            self.falling_fall_on(e);
            self.landing_effects(e.pos, below, support, Src::Other);
        }
        if e.on_ground {
            e.fall = 0.0;
        }
        // The original block (`s`) is used for loot and for breaking even if the anvil got damaged on
        // landing (26.3); the current state (possibly damaged) is used for placement.
        let orig = s;
        let s = match e.kind {
            EntKind::Falling(s) => s,
            EntKind::Tnt(_) => s,
        };
        // S6: portal; to or from the End, the old one keeps its last tick (26.3 duplication).
        if let Some(dupe) = self.falling_portal(e)
            && !dupe
        {
            return;
        }
        let mut p = [e.pos[0].floor() as i32, e.pos[1].floor() as i32, e.pos[2].floor() as i32];
        let concrete = self.v().kind(s) == Kind::ConcretePowder;
        let mut solidify = concrete && self.fluid_at(p).kind == 1;
        // Fast powder (more than one block per tick): ray from the position before the tick
        // (collision shapes, fluid sources); a water source it touches freezes it there.
        let v2 = e.vel[0] * e.vel[0] + e.vel[1] * e.vel[1] + e.vel[2] * e.vel[2];
        if concrete && v2 > 1.0 {
            let ray = crate::fluid_use::SimRay(self, true);
            let hit = rc_physics::ray::clip(&ray, e.old, e.pos, rc_physics::ray::Fluids::SourceOnly);
            if !hit.miss && self.fluid_at(hit.pos).kind == 1 {
                p = hit.pos;
                solidify = true;
            }
        }
        if !e.on_ground && !solidify {
            if (e.time > 100 && (p[1] <= self.dim.height.min_y || p[1] >= self.top_y())) || e.time > 600 {
                // Lost (out of the world or too long): dropped as an item according to `entity_drops`.
                if e.drop_item && self.rule("minecraft:entity_drops", true) {
                    self.falling_spawn_item(e, orig);
                }
                e.removed = true;
            }
        } else {
            let cur = self.get(p);
            e.vel = [e.vel[0] * 0.7, e.vel[1] * -0.5, e.vel[2] * 0.7];
            if self.v().kind(cur) != Kind::MovingPiston {
                let drops = e.drop_item && self.rule("minecraft:entity_drops", true);
                if e.cancel_drop {
                    // Suspicious sand or gravel, anvil worn down to the end: broken on arrival.
                    e.removed = true;
                    self.falling_broken(e, orig, p);
                } else {
                    let may_replace = self.v().replaceable(cur) || self.v().is_air(cur);
                    let continue_falling = self.is_free_below(p) && (!concrete || !solidify);
                    let mut st =
                        if solidify { self.v().default_state(self.bs.t.pair[self.v().block(s) as usize]) } else { s };
                    let survive = self.can_survive(st, p) && !continue_falling;
                    if may_replace && survive {
                        if self.v().has(st, "waterlogged") && self.fluid_at(p).kind == 1 && self.fluid_at(p).source {
                            st = self.v().set_bool(st, "waterlogged", true);
                        }
                        if self.set_block(p, st, flag::ALL) {
                            self.send_block(p);
                            e.removed = true;
                            self.on_land(p, st, cur);
                        } else if drops {
                            e.removed = true;
                            self.falling_broken(e, orig, p);
                            self.falling_spawn_item(e, orig);
                        }
                    } else {
                        // Occupied slot (torch, slab...) or block that does not fit: item dropped.
                        e.removed = true;
                        if drops {
                            self.falling_broken(e, orig, p);
                            self.falling_spawn_item(e, orig);
                        }
                    }
                }
            }
        }
        e.vel = [e.vel[0] * 0.98, e.vel[1] * 0.98, e.vel[2] * 0.98];
    }

    /// Fall distance of a non-living entity after a move `mv` starting from `from`
    /// (the move of an entity with collisions then the fall-damage check): reset when the move reaches a
    /// block and the ray cast touches water or a block of the tag
    /// `fall_damage_resetting`; accumulation of the descent outside water. Returns true when
    /// the entity is on the ground with a positive distance (the landing fix follows).
    pub(crate) fn fall_step(
        &self,
        fall: &mut f64,
        from: [f64; 3],
        mv: [f64; 3],
        in_water: bool,
        on_ground: bool,
    ) -> bool {
        let len2 = mv[0] * mv[0] + mv[1] * mv[1] + mv[2] * mv[2];
        if *fall != 0.0 && len2 >= 1.0 {
            let len = len2.sqrt();
            let k = len.min(8.0) / len;
            let to = [from[0] + mv[0] * k, from[1] + mv[1] * k, from[2] + mv[2] * k];
            if self.fall_ray_resets(from, to) {
                *fall = 0.0;
            }
        }
        if !in_water && mv[1] < 0.0 {
            *fall -= f64::from(mv[1] as f32);
        }
        on_ground && *fall > 0.0
    }

    /// Does the segment cross water or a block of the tag `fall_damage_resetting` (climbables,
    /// sweet berry bush, cobweb)? (`clip` in "fall reset" mode, fluid: water.)
    fn fall_ray_resets(&self, from: [f64; 3], to: [f64; 3]) -> bool {
        let d = [to[0] - from[0], to[1] - from[1], to[2] - from[2]];
        let mut cell = from.map(|v| v.floor() as i32);
        let step = d.map(|v| {
            if v > 0.0 {
                1
            } else if v < 0.0 {
                -1
            } else {
                0
            }
        });
        let delta: [f64; 3] = std::array::from_fn(|i| if step[i] == 0 { f64::MAX } else { 1.0 / d[i].abs() });
        let mut tmax: [f64; 3] = std::array::from_fn(|i| match step[i] {
            1 => (f64::from(cell[i]) + 1.0 - from[i]) * delta[i],
            -1 => (from[i] - f64::from(cell[i])) * delta[i],
            _ => f64::MAX,
        });
        for _ in 0..64 {
            if self.fall_reset_cell(cell, from, d) {
                return true;
            }
            let a = if tmax[0] < tmax[1] {
                if tmax[0] < tmax[2] { 0 } else { 2 }
            } else if tmax[1] < tmax[2] {
                1
            } else {
                2
            };
            if tmax[a] > 1.0 {
                return false;
            }
            cell[a] += step[a];
            tmax[a] += delta[a];
        }
        false
    }

    /// Does the segment `from + t·d` (t in [0, 1]) touch the cell's reset shape?
    /// Tag block: full cube; water (also inside a water-filled block): box of the height of the
    /// fluid (1 under water).
    fn fall_reset_cell(&self, cell: P, from: [f64; 3], d: [f64; 3]) -> bool {
        let st = self.get(cell);
        if self.v().is_air(st) {
            return false;
        }
        let mut height = 0.0f64;
        if self.phys.climbable(st) || matches!(self.state_name(st), "minecraft:sweet_berry_bush" | "minecraft:cobweb") {
            height = 1.0;
        }
        let f = self.fluid_of(st);
        if f.kind == 1 {
            let h = if self.fluid_at(above(cell)).kind == 1 { 1.0 } else { f64::from(f.own_height()) };
            height = height.max(h);
        }
        if height <= 0.0 {
            return false;
        }
        let c = cell.map(f64::from);
        let (lo, hi) = ([c[0], c[1], c[2]], [c[0] + 1.0, c[1] + height, c[2] + 1.0]);
        let (mut t0, mut t1) = (0.0f64, 1.0f64);
        for i in 0..3 {
            if d[i].abs() < 1.0e-12 {
                if from[i] < lo[i] || from[i] > hi[i] {
                    return false;
                }
            } else {
                let (a, b) = ((lo[i] - from[i]) / d[i], (hi[i] - from[i]) / d[i]);
                t0 = t0.max(a.min(b));
                t1 = t1.min(a.max(b));
                if t0 > t1 {
                    return false;
                }
            }
        }
        true
    }

    /// Rest of the fall-damage check on landing of a non-living entity after a fall:
    /// effects of the block hit (farmland draws a random roll before testing the entity
    /// type, honey plays its slide), then the `hit_ground` game event (state of the main
    /// carrier block, otherwise of the block hit).
    pub(crate) fn landing_effects(&mut self, pos: [f64; 3], below: P, support: Option<P>, src: Src) {
        let on = self.get(below);
        match self.state_name(on) {
            "minecraft:farmland" => {
                let _ = self.bs.rng.next_float();
            }
            // "Neutral" category (non-living entity).
            "minecraft:honey_block" => self.play_sound("minecraft:block.honey_block.slide", 6, pos, 1.0, 1.0),
            _ => {}
        }
        let st = support.map_or(on, |p| self.get(p));
        self.game_event_src("minecraft:hit_ground", pos, Some(src), Some(st));
    }

    /// Falling block that reached the ground after a fall (the reaction to a fall onto a block of the block hit, then
    /// the fall damage): only the anvil and the dripstone hurt, the anvil wears down.
    fn falling_fall_on(&mut self, e: &mut SimEntity) {
        let EntKind::Falling(s) = e.kind else { return };
        let below = {
            let b = e.body.as_deref().map(|b| b.below(&self.phys, &self.world(), 0.2));
            b.unwrap_or_else(|| [e.pos[0].floor() as i32, (e.pos[1] - 0.2).floor() as i32, e.pos[2].floor() as i32])
        };
        let on = self.get(below);
        // Distance passed on by the block hit (the damage factor is ignored here).
        let Some((fall, ..)) = self.fall_rule(on, e.fall, false) else { return };
        let Some((per, max)) = e.hurt else { return };
        let n = (fall - 1.0).ceil() as i32;
        if n < 0 {
            return;
        }
        let block = self.state_name(s).to_owned();
        let anvil = matches!(block.as_str(), "minecraft:anvil" | "minecraft:chipped_anvil" | "minecraft:damaged_anvil");
        let ty = if anvil {
            "minecraft:falling_anvil"
        } else if block == "minecraft:pointed_dripstone" || block == "minecraft:sulfur_spike" {
            "minecraft:falling_stalactite"
        } else {
            "minecraft:falling_block"
        };
        let damage = ((n as f32 * per).floor()).min(max as f32);
        // Living mobs and player (neither creative nor spectator) in the block's box.
        let bb = Self::entity_box(e);
        let ids: Vec<i32> = self.mobs.list.iter().flatten().filter(|m| m.alive()).map(|m| m.id).collect();
        for id in ids {
            let Some(mb) = self.ent_bbox(crate::mobs::mob::Ent::Mob(id)) else { continue };
            if !mb.intersects(&bb) {
                continue;
            }
            let Some(i) = self.mob_index(id) else { continue };
            let Some(mut o) = self.mobs.list[i].take() else { continue };
            self.mob_hurt(&mut o, ty, damage, None, Some(e.pos));
            self.mobs.list[i] = Some(o);
        }
        if !matches!(self.game_mode, 1 | 3)
            && !self.life.vit.dead
            && self.ent_bbox(crate::mobs::mob::Ent::Player).is_some_and(|pb| pb.intersects(&bb))
        {
            self.hurt(ty, damage);
        }
        if anvil && damage > 0.0 && self.bs.rng.next_float() < 0.05 + n as f32 * 0.05 {
            let next = match block.as_str() {
                "minecraft:anvil" => Some("minecraft:chipped_anvil"),
                "minecraft:chipped_anvil" => Some("minecraft:damaged_anvil"),
                _ => None,
            };
            match next {
                None => e.cancel_drop = true,
                Some(n) => {
                    let v = self.v();
                    let facing = v.get(s, "facing").unwrap_or("north").to_owned();
                    let d = v.default_state(self.bs.t.block_id(n));
                    e.kind = EntKind::Falling(v.set(d, "facing", &facing));
                }
            }
        }
    }

    /// Falling block dropped as an item at its position (pickup delay 10).
    fn falling_spawn_item(&mut self, e: &SimEntity, s: u32) {
        let name = self.state_name(s).to_owned();
        let known = self.data.registry("minecraft:item").is_some_and(|r| r.entries.contains(&name));
        if known && name != "minecraft:air" {
            let stack = crate::player::Stack { id: name, count: 1, components: None };
            self.spawn_item_random(e.pos, stack, 10);
        }
    }

    /// Falling block broken on arrival: sound of the anvil, of the
    /// dripstone, or breaking of suspicious sand and gravel.
    fn falling_broken(&mut self, e: &SimEntity, s: u32, p: P) {
        let name = self.state_name(s).to_owned();
        match name.as_str() {
            "minecraft:anvil" | "minecraft:chipped_anvil" | "minecraft:damaged_anvil" => self.level_event(1029, p, 0),
            "minecraft:pointed_dripstone" => self.level_event(1045, p, 0),
            "minecraft:sulfur_spike" => self.level_event(1052, p, 0),
            "minecraft:suspicious_sand" | "minecraft:suspicious_gravel" => {
                let b = Self::entity_box(e);
                let c = [(b.min[0] + b.max[0]) / 2.0, (b.min[1] + b.max[1]) / 2.0, (b.min[2] + b.max[2]) / 2.0];
                let at = [c[0].floor() as i32, c[1].floor() as i32, c[2].floor() as i32];
                self.level_event(2001, at, s as i32);
                self.game_event_src(
                    "minecraft:block_destroy",
                    c,
                    Some(crate::mobs::recent::vibration::Src::Other),
                    None,
                );
            }
            _ => {}
        }
    }

    fn is_free_below(&self, p: P) -> bool {
        let s = self.get(below(p));
        let v = self.v();
        v.is_air(s)
            || matches!(v.kind(s), Kind::Fire | Kind::SoulFire)
            || v.info(s).has(rc_data::flags::LIQUID)
            || v.replaceable(s)
    }

    fn on_land(&mut self, p: P, s: u32, replaced: u32) {
        // Anvil placed: landing sound.
        if matches!(self.state_name(s), "minecraft:anvil" | "minecraft:chipped_anvil" | "minecraft:damaged_anvil") {
            self.level_event(1031, p, 0);
        }
        // Block replaced in water, or water touching a face (except through a
        // full face).
        if self.v().kind(s) == Kind::ConcretePowder && (self.fluid_of(replaced).kind == 1 || self.touches_liquid(p)) {
            let c = self.v().default_state(self.bs.t.pair[self.v().block(s) as usize]);
            self.set_block(p, c, flag::ALL);
        }
    }

    fn tick_tnt(&mut self, e: &mut SimEntity, fuse: i32) {
        // Portal first (mark of the previous tick). Gone elsewhere, the old one
        // still finishes its tick (fuse, explosion at 0 like game 26.3) then disappears.
        let gone = self.tnt_portal(e);
        // Same tick as the client: gravity, movement, friction, bounce, fuse, currents.
        let from = e.pos;
        let mut b = self.body_of(e);
        let mut fuse = fuse;
        // "In water" state read before the tick (updated at the end of the previous tick).
        let wet = b.was_in_water();
        let alive = b.tick_tnt(&self.world(), &self.phys, &mut fuse);
        let (mv, below, support) = (b.moved, b.below(&self.phys, &self.world(), 0.2), b.support());
        Self::put_body(e, b);
        e.kind = EntKind::Tnt(fuse);
        // Fall distance and landing (inside the movement, so before the explosion).
        if self.fall_step(&mut e.fall, from, mv, wet, e.on_ground) {
            self.landing_effects(e.pos, below, support, Src::Other);
        }
        if e.on_ground {
            e.fall = 0.0;
        }
        // Portals touched by this movement, handled on the next tick.
        if !gone {
            self.sim_entity_portal_mark(e);
        }
        if !alive {
            e.removed = true;
            // Without the `tnt_explodes` rule, the TNT fizzles out without computing anything.
            if self.rule("minecraft:tnt_explodes", true) {
                let c = [e.pos[0], e.pos[1] + FALLING_SIZE * 0.0625, e.pos[2]];
                // `explode` event emitted by the primed TNT (non-living entity), then
                // the common explosion: damage, knockback, loot, packet (particles and sound).
                self.game_event_src("minecraft:explode", c, Some(crate::mobs::recent::vibration::Src::Other), None);
                // In water: state at the end of the previous tick, like the game.
                self.tnt_explode(e.id, c, 4.0, e.owner, wet, e.used_portal);
            }
        }
    }

    /// TNT block blown up by an explosion: primed with a short fuse (10 to 29 ticks).
    pub(crate) fn tnt_was_exploded(&mut self, p: P) {
        if self.rule("minecraft:tnt_explodes", true) {
            let r = self.bs.rng.next_int_bound(80 / 4) + 80 / 8;
            self.spawn_tnt(p, r, None);
        }
    }

    /// Blast of an explosion on non-living entities:
    /// primed TNT (pushed from its feet, never hurt), falling block (pushed), item
    /// on the ground and orb (hurt, destroyed at 0 health, pushed). `exclude`: the TNT that
    /// explodes; `by`: author; `direct_wet`: direct source in water; `blocklike`: items
    /// on the ground that are hit (mob rule or destroyed blocks).
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn explosion_hits_objects(
        &mut self,
        center: [f64; 3],
        radius: f32,
        exclude: Option<i32>,
        ty: &str,
        by: Option<crate::mobs::mob::Ent>,
        direct_wet: bool,
        blocklike: bool,
    ) {
        let by_mob = matches!(by, Some(crate::mobs::mob::Ent::Mob(_)));
        let double = f64::from(radius * 2.0);
        // Relative distance, direction (from `origin`) and visible part.
        let blast = |s: &Server, pos: [f64; 3], origin: [f64; 3], b: &Aabb| -> Option<(f64, [f64; 3], f64)> {
            let d = [pos[0] - center[0], pos[1] - center[1], pos[2] - center[2]];
            let dist = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() / double;
            if dist > 1.0 {
                return None;
            }
            let mut dir = [origin[0] - center[0], origin[1] - center[1], origin[2] - center[2]];
            let l = (dir[0] * dir[0] + dir[1] * dir[1] + dir[2] * dir[2]).sqrt();
            dir = if l < 1.0e-4 { [0.0; 3] } else { dir.map(|v| v / l) };
            Some((dist, dir, f64::from(s.seen_percent(center, b))))
        };
        let damage = |dist: f64, exposure: f64| {
            let pow = (1.0 - dist) * exposure;
            ((pow * pow + pow) / 2.0 * 7.0 * double + 1.0) as f32
        };
        for i in 0..self.bs.entities.list.len() {
            let e = &self.bs.entities.list[i];
            if e.removed || Some(e.id) == exclude {
                continue;
            }
            let tnt = matches!(e.kind, EntKind::Tnt(_));
            let eye = if tnt { 0.0 } else { f64::from(0.833f32) };
            let Some((dist, dir, exposure)) =
                blast(self, e.pos, [e.pos[0], e.pos[1] + eye, e.pos[2]], &Self::entity_box(e))
            else {
                continue;
            };
            let kb = dir.map(|v| v * (1.0 - dist) * exposure);
            let e = &mut self.bs.entities.list[i];
            e.vel = [e.vel[0] + kb[0], e.vel[1] + kb[1], e.vel[2] + kb[2]];
            e.track.needs_sync = true;
            // The falling block registers the hit (velocity returned right away); the TNT does not.
            e.track.sync_velocity |= !tnt;
        }
        // Only the item on the ground escapes the blast when neither the rule nor the explosion touch
        // the blocks; the falling block and the orb always take it.
        let items: Vec<(i32, [f64; 3])> = if blocklike {
            self.life.drops.iter().filter(|d| !d.is_removed()).map(|d| (d.id, d.pos)).collect()
        } else {
            Vec::new()
        };
        for (id, pos) in items {
            let h = 0.125;
            let b = Aabb { min: [pos[0] - h, pos[1], pos[2] - h], max: [pos[0] + h, pos[1] + 0.25, pos[2] + h] };
            let Some((dist, dir, exposure)) = blast(self, pos, [pos[0], pos[1] + f64::from(0.2125f32), pos[2]], &b)
            else {
                continue;
            };
            let hurt = self.item_hurt(id, ty, damage(dist, exposure), by_mob);
            let kb = dir.map(|v| v * (1.0 - dist) * exposure);
            if let Some(d) = self.life.drops.iter_mut().find(|d| d.id == id && !d.is_removed()) {
                d.push_sync(kb, hurt);
            }
        }
        let orbs: Vec<(i32, [f64; 3])> =
            self.life.orbs.iter().filter(|o| !o.is_removed()).map(|o| (o.id, o.pos)).collect();
        for (id, pos) in orbs {
            let h = 0.25;
            let b = Aabb { min: [pos[0] - h, pos[1], pos[2] - h], max: [pos[0] + h, pos[1] + 0.5, pos[2] + h] };
            let Some((dist, dir, exposure)) = blast(self, pos, [pos[0], pos[1] + f64::from(0.425f32), pos[2]], &b)
            else {
                continue;
            };
            let hurt = self.orb_hurt(id, damage(dist, exposure));
            let kb = dir.map(|v| v * (1.0 - dist) * exposure);
            if let Some(o) = self.life.orbs.iter_mut().find(|o| o.id == id && !o.is_removed()) {
                o.push_sync(kb, hurt);
            }
        }
        // Paintings, frames, armor stands, cushions.
        self.deco_explosion(center, radius, by, direct_wet, blocklike);
    }

    /// Entities that trigger blocks (plates, tripwire) whose box touches `b`; `living`:
    /// living entities only (stone plates).
    pub(crate) fn count_entities_in(&self, b: &Aabb, living: bool) -> usize {
        self.entities_in(b, living, None, true)
    }

    /// Non-spectator entities whose box touches `b`: player, mobs,
    /// and, unless `living`, falling blocks, TNT, items on the ground, orbs, projectiles, effect
    /// clouds, llama spit. `except`: mob left out; `triggers`: without those that
    /// ignore trigger blocks (bats). Boats, minecarts and armor stands are not
    /// simulated.
    pub(crate) fn entities_in(&self, b: &Aabb, living: bool, except: Option<i32>, triggers: bool) -> usize {
        let hit = |o: &Aabb| (0..3).all(|i| o.max[i] > b.min[i] && o.min[i] < b.max[i]);
        let around = |pos: [f64; 3], w: f64, h: f64| Aabb {
            min: [pos[0] - w / 2.0, pos[1], pos[2] - w / 2.0],
            max: [pos[0] + w / 2.0, pos[1] + h, pos[2] + w / 2.0],
        };
        let mut n = 0;
        if self.game_mode != 3 && self.ent_bbox(crate::mobs::mob::Ent::Player).is_some_and(|pb| hit(&pb)) {
            n += 1;
        }
        n += self
            .mobs
            .list
            .iter()
            .flatten()
            .filter(|m| {
                !m.removed
                    && Some(m.id) != except
                    && !(triggers && m.kind == crate::mobs::species::Kind::Bat)
                    && hit(&m.bbox())
            })
            .count();
        if !living {
            n += self.bs.entities.list.iter().filter(|e| !e.removed && hit(&Self::entity_box(e))).count();
            n += self.life.drops.iter().filter(|d| !d.is_removed() && hit(&around(d.pos, 0.25, 0.25))).count();
            n += self.life.orbs.iter().filter(|o| !o.is_removed() && hit(&around(o.pos, 0.5, 0.5))).count();
            n += self.proj.list.iter().filter(|p| !p.removed && hit(&p.bbox())).count();
            n += self.proj.clouds.iter().filter(|c| hit(&c.bbox())).count();
            n += self.mobs.mounts.spits.iter().filter(|s| hit(&around(s.pos, 0.25, 0.25))).count();
            // G5-F06: minecarts, the one being ticked included.
            n += self.carts.list.iter().flatten().filter(|c| !c.removed && hit(&c.bbox())).count();
            n += usize::from(self.carts.ticking.is_some_and(|p| hit(&crate::minecart::bbox_at(p))));
        }
        n
    }

    /// Plates under the player after an accepted movement.
    pub(crate) fn player_touch_blocks(&mut self) {
        let p = [self.pos[0].floor() as i32, self.pos[1].floor() as i32, self.pos[2].floor() as i32];
        for q in [p, below(p)] {
            if matches!(self.v().kind(self.get(q)), Kind::Plate { .. } | Kind::WeightedPlate { .. }) {
                self.plate_entity_inside(q);
            }
        }
        self.tripwire_entity_inside(p);
        // G5-F09: big dripleaf under the feet (the full pass over crossed blocks, G5-F02,
        // calls it for all entities; a second call has no effect).
        if self.game_mode != 3 && self.v().kind(self.get(p)) == Kind::BigDripleaf {
            let (g, y) = (self.on_ground, self.pos[1]);
            self.dripleaf_entity_inside(p, g, y);
        }
    }

    // --- Container inventories. ---
    //
    // Single source of the contents: the simulated furnace or brewing stand (its block entity is only
    // rewritten before the save), otherwise the block entity's NBT (`Items`, or the single slot of the
    // jukebox and the decorated pot). The open menu rereads this source before every click and every
    // send (`pull_menu`): no copy can overwrite another.

    pub(crate) fn container_size(&self, s: u32) -> Option<usize> {
        let n = self.data.block_of(s).map_or("", |b| b.name.as_str());
        let n = n.strip_prefix("minecraft:").unwrap_or(n);
        Some(match n {
            "chest" | "trapped_chest" | "barrel" => 27,
            "hopper" => 5,
            "dispenser" | "dropper" | "crafter" => 9,
            "furnace" | "blast_furnace" | "smoker" => 3,
            "brewing_stand" => 5,
            "decorated_pot" | "jukebox" => 1,
            _ if n.ends_with("shulker_box") || n.ends_with("copper_chest") => 27,
            _ => return None,
        })
    }

    /// Items of the container in slot order (comparators).
    pub(crate) fn be_item_counts(&self, p: P) -> Vec<(String, i32)> {
        self.be_item_counts_full(p).into_iter().map(|e| (e.0, e.1)).collect()
    }

    /// Contents by increasing slot: id, count, stack size from the stack's
    /// `max_stack_size` component if set (clamped to 1..99 like the game).
    pub(crate) fn be_item_counts_full(&self, p: P) -> Vec<(String, i32, Option<i32>)> {
        self.slots(p)
            .into_iter()
            .flatten()
            .filter_map(|it| {
                let id = it.get("id").and_then(Nbt::as_str).unwrap_or("").to_owned();
                let count = Self::item_count(&it);
                let own = it
                    .get("components")
                    .and_then(|c| c.get("minecraft:max_stack_size"))
                    .and_then(Nbt::as_i64)
                    .map(|n| n.clamp(1, 99) as i32);
                (!id.is_empty() && count > 0).then_some((id, count, own))
            })
            .collect()
    }

    /// Family of the container at `s` (face and placement rules).
    fn cont_kind(&self, s: u32) -> Cont {
        let n = self.data.block_of(s).map_or("", |b| b.name.as_str());
        match n.strip_prefix("minecraft:").unwrap_or(n) {
            "furnace" | "blast_furnace" | "smoker" => Cont::Furnace,
            "brewing_stand" => Cont::Brewing,
            "jukebox" => Cont::Jukebox,
            "decorated_pot" => Cont::Pot,
            "crafter" => Cont::Crafter,
            n if n.ends_with("shulker_box") => Cont::Shulker,
            _ => Cont::Plain,
        }
    }

    /// Live slots of the simulated furnace or brewing stand at `p` (their only source of truth).
    pub(crate) fn live_items(&self, p: P) -> Option<&[Option<crate::player::Stack>]> {
        match self.cont_kind(self.get(p)) {
            Cont::Furnace => self.life.furnaces.get(&p).map(|f| &f.items[..]),
            Cont::Brewing => self.life.brewers.get(&p).map(|b| &b.items[..]),
            _ => None,
        }
    }

    pub(crate) fn max_stack(&self, item: &str) -> i32 {
        self.data
            .registry("minecraft:item")
            .and_then(|r| r.entries.iter().position(|e| e == item))
            .and_then(|i| self.data.items.get(i))
            .map_or(64, |i| i32::from(i.max_stack_size.max(1)))
    }

    /// Contents of the container at `p`, slot by slot (container size, empty otherwise).
    pub(crate) fn slots(&self, p: P) -> Vec<Option<Nbt>> {
        let s = self.get(p);
        let Some(size) = self.container_size(s) else { return Vec::new() };
        if let Some(live) = self.live_items(p) {
            return live.iter().map(|x| x.as_ref().map(|st| st.to_nbt(None))).collect();
        }
        let mut out = vec![None; size];
        let Some(be) = self.be_get(p) else { return out };
        match single_item_key(self.cont_kind(s)) {
            Some(k) => out[0] = be.get(k).filter(|it| crate::player::Stack::from_nbt(it).is_some()).cloned(),
            None => {
                for it in crate::nbtio::list(be.get("Items")) {
                    let slot = crate::nbtio::int(it, "Slot").unwrap_or(-1);
                    if (0..size as i64).contains(&slot) {
                        out[slot as usize] = Some(it.clone());
                    }
                }
            }
        }
        out
    }

    /// Contents written back to their source (placing an item then the "to save" mark): comparators notified.
    pub(crate) fn store_slots(&mut self, p: P, slots: &[Option<Nbt>]) {
        let s = self.get(p);
        let kind = self.cont_kind(s);
        let stack = |i: usize| slots.get(i).cloned().flatten().as_ref().and_then(crate::player::Stack::from_nbt);
        // Jukebox: same placement as the click: `has_record` state, play or stop.
        if kind == Cont::Jukebox {
            self.jukebox_set_item(p, stack(0), s);
            self.container_changed(p);
            return;
        }
        let live = match kind {
            Cont::Furnace if self.life.furnaces.contains_key(&p) => {
                let input = stack(0);
                let (recipes, items) = (&self.life.recipes, &self.life.items);
                if let Some(f) = self.life.furnaces.get_mut(&p) {
                    f.set_input(input, recipes, items);
                    (f.items[1], f.items[2]) = (stack(1), stack(2));
                    f.dirty = true;
                }
                true
            }
            Cont::Brewing if self.life.brewers.contains_key(&p) => {
                if let Some(b) = self.life.brewers.get_mut(&p) {
                    for (k, it) in b.items.iter_mut().enumerate() {
                        *it = stack(k);
                    }
                    b.dirty = true;
                }
                true
            }
            _ => false,
        };
        if !live && let Some(be) = self.be_get_mut(p) {
            match single_item_key(kind) {
                Some(k) => match slots.first().cloned().flatten() {
                    Some(mut it) => {
                        crate::nbtio::remove(&mut it, "Slot");
                        crate::nbtio::set(be, k, it);
                    }
                    None => crate::nbtio::remove(be, k),
                },
                None => {
                    let list: Vec<Nbt> = slots
                        .iter()
                        .enumerate()
                        .filter_map(|(i, s)| {
                            let mut it = s.clone()?;
                            crate::nbtio::set(&mut it, "Slot", Nbt::Byte(i as i8));
                            Some(it)
                        })
                        .collect();
                    crate::nbtio::set(be, "Items", Nbt::List(list));
                }
            }
        }
        self.container_changed(p);
    }

    /// The "to save" mark of a block entity: neighbouring comparators reread the signal.
    pub(crate) fn container_changed(&mut self, p: P) {
        let s = self.get(p);
        if self.v().has_analog(s) {
            let b = self.v().block(s);
            self.update_neighbour_for_output_signal(p, b);
        }
    }

    pub(super) fn item_count(it: &Nbt) -> i32 {
        crate::nbtio::int(it, "count").unwrap_or(1) as i32
    }

    pub(super) fn same_item(a: &Nbt, b: &Nbt) -> bool {
        a.get("id") == b.get("id") && a.get("components") == b.get("components")
    }

    /// Maximum stack size of item `it` (`max_stack_size` component included).
    fn max_of(&self, it: &Nbt) -> i32 {
        crate::player::Stack::from_nbt(it).map_or(64, |s| self.life.items.max_stack(&s))
    }

    /// The item carries component `name` (on the stack, otherwise by default and not removed).
    pub(crate) fn stack_has(&self, s: &crate::player::Stack, name: &str) -> bool {
        crate::items::component(s, name).is_some()
            || (crate::items::component(s, &format!("!{name}")).is_none()
                && !self.life.items.defaults(&s.id)[name].is_null())
    }

    /// Placement rule of a container (and the insertion-through-a-face test of Shulker
    /// boxes) for slot `i` of container `q`.
    fn can_place(&self, q: P, kind: Cont, i: usize, item: &crate::player::Stack, slots: &[Option<Nbt>]) -> bool {
        let count = |k: usize| slots.get(k).and_then(Option::as_ref).map_or(0, Self::item_count);
        match kind {
            Cont::Shulker => !item.id.ends_with("shulker_box"),
            Cont::Furnace => match i {
                2 => false,
                1 => {
                    let bucket_in = slots[1].as_ref().is_some_and(|s| s.get("id").and_then(Nbt::as_str) == Some("minecraft:bucket"));
                    self.stack_has(item, "minecraft:cooking_fuel") || (item.id == "minecraft:bucket" && !bucket_in)
                }
                _ => true,
            },
            Cont::Brewing => match i {
                4 => self.stack_has(item, "minecraft:brewing_fuel"),
                3 => self.life.stations.is_reagent(&self.life.items, item),
                _ => self.life.stations.is_potion_input(&self.life.items, item) && slots[i].is_none(),
            },
            Cont::Jukebox => self.stack_has(item, "minecraft:jukebox_playable") && slots[i].is_none(),
            Cont::Crafter => {
                let disabled = |k: usize| self.crafter_slot_disabled(q, k);
                let Some(cur) = slots[i].as_ref() else { return !disabled(i) };
                let c = Self::item_count(cur);
                // Smallest stack first: a following slot that is empty or smaller (same item)
                // refuses this one.
                !disabled(i)
                    && c < self.max_of(cur)
                    && !(i + 1..slots.len().min(9)).any(|k| {
                        !disabled(k)
                            && match slots[k].as_ref() {
                                None => true,
                                Some(o) => count(k) < c && Self::same_item(o, cur),
                            }
                    })
            }
            Cont::Pot | Cont::Plain => true,
        }
    }

    /// Disabled slot of the crafter (`disabled_slots`).
    fn crafter_slot_disabled(&self, q: P, k: usize) -> bool {
        match self.be_get(q).and_then(|be| be.get("disabled_slots")) {
            Some(Nbt::IntArray(a)) => a.contains(&(k as i32)),
            Some(Nbt::List(l)) => l.iter().any(|v| v.as_i64() == Some(k as i64)),
            _ => false,
        }
    }

    /// Removal rule through a face (the removal test then the through-a-face removal test); `into`:
    /// receiving container (jukebox: it needs an empty slot).
    fn can_take(&self, kind: Cont, i: usize, it: &Nbt, face: Dir, into: P) -> bool {
        let id = it.get("id").and_then(Nbt::as_str).unwrap_or("");
        match kind {
            Cont::Furnace => {
                !(face == Dir::Down && i == 1)
                    || self.life.items.has_tag("minecraft:furnace_fuel_bottom_takeable", id)
            }
            Cont::Brewing => i != 3 || id == "minecraft:glass_bottle",
            Cont::Jukebox => self.slots(into).iter().any(Option::is_none),
            _ => true,
        }
    }

    /// Container at the position (half of a double chest: both, left first).
    pub(crate) fn container_at(&self, p: P) -> Option<Vec<P>> {
        let s = self.get(p);
        self.container_size(s)?;
        if self.v().kind(s) == Kind::Chest && self.v().get(s, "type") != Some("single") {
            let other = self.chest_other_half(s, p)?;
            return Some(if self.v().get(s, "type") == Some("right") { vec![other, p] } else { vec![p, other] });
        }
        Some(vec![p])
    }

    /// Inserts one unit of `item` into the container `dest` through
    /// face `face` (none: all slots, like entering a hopper): the face's slots
    /// in their order, placement rules, empty slot or identical non-full stack. True if
    /// accepted.
    pub(crate) fn insert_one(&mut self, dest: &[P], item: &Nbt, face: Option<Dir>) -> bool {
        let Some(stack) = crate::player::Stack::from_nbt(item) else { return false };
        let max = self.life.items.max_stack(&stack);
        for &q in dest {
            let kind = self.cont_kind(self.get(q));
            let mut slots = self.slots(q);
            for &i in face_slots(kind, slots.len(), face) {
                if i >= slots.len() || !self.can_place(q, kind, i, &stack, &slots) {
                    continue;
                }
                match &mut slots[i] {
                    None => {
                        let mut it = item.clone();
                        crate::nbtio::set(&mut it, "count", Nbt::Int(1));
                        slots[i] = Some(it);
                        self.store_slots(q, &slots);
                        return true;
                    }
                    Some(cur) if Self::same_item(cur, item) && Self::item_count(cur) < max => {
                        let c = Self::item_count(cur) + 1;
                        crate::nbtio::set(cur, "count", Nbt::Int(c));
                        self.store_slots(q, &slots);
                        return true;
                    }
                    _ => {}
                }
            }
        }
        false
    }

    /// Container full on the face's side: each slot of the face holds
    /// a full stack (no slot: full).
    fn container_full(&self, dest: &[P], face: Option<Dir>) -> bool {
        dest.iter().all(|&q| {
            let kind = self.cont_kind(self.get(q));
            let slots = self.slots(q);
            face_slots(kind, slots.len(), face).iter().all(|&i| {
                slots.get(i).and_then(Option::as_ref).is_some_and(|it| Self::item_count(it) >= self.max_of(it))
            })
        })
    }

    // --- Composter, a container without a block entity. ---

    /// Item entering the composter `c` through face `face` (hopper or dropper above):
    /// below level 7, a compostable item from the top adds its layer (event 1500).
    /// `None`: not a composter; otherwise true if the item is taken.
    fn composter_put(&mut self, c: P, item: &Nbt, face: Dir) -> Option<bool> {
        let s = self.get(c);
        if self.state_name(s) != "minecraft:composter" {
            return None;
        }
        let id = item.get("id").and_then(Nbt::as_str).unwrap_or("").to_owned();
        let compostable = crate::player::Stack::from_nbt(item).is_some_and(|st| self.stack_has(&st, "minecraft:compostable"));
        if face != Dir::Up || self.v().get_int(s, "level") >= 7 || !compostable {
            return Some(false);
        }
        let ns = self.composter_add_layer(c, s, &id, None);
        self.level_event(1500, c, i32::from(ns != s));
        Some(true)
    }

    /// Can the composter `c` receive through face `face`? (Hopper: otherwise it is
    /// "full" on that side.)
    fn composter_open(&self, c: P, face: Dir) -> bool {
        face == Dir::Up && self.v().get_int(self.get(c), "level") < 7
    }

    /// Composter `c` above the hopper `hopper`: at level 8, the bone meal goes down and
    /// the composter empties (placement, `block_change` event, no sound). `None`: not a
    /// composter; otherwise true if an item moved (no suction of items on the ground).
    fn composter_take(&mut self, c: P, hopper: P) -> Option<bool> {
        let s = self.get(c);
        if self.state_name(s) != "minecraft:composter" {
            return None;
        }
        if self.v().get_int(s, "level") != 8 {
            return Some(false);
        }
        // Like the game: the bone meal is first removed from the output, which already empties the
        // composter (level 0, `block_change`), then only offered to the hopper. Hopper
        // that refuses it: the bone meal is lost (quirk). Success and failure both re-emit a
        // second `block_change` (new change signal of the output).
        let ns = self.v().set_int(s, "level", 0);
        self.set_update(c, ns);
        self.game_event_block("minecraft:block_change", c, None, Some(ns));
        let meal = crate::player::Stack { id: "minecraft:bone_meal".into(), count: 1, components: None }.to_nbt(None);
        let moved = self.insert_one(&[hopper], &meal, None);
        self.game_event_block("minecraft:block_change", c, None, Some(ns));
        Some(moved)
    }

    // --- Hoppers (§ 3.17). ---

    // --- Shulker box lid (animation also held by the server). ---

    /// Lid event received (number of openers `count`): 0 closes,
    /// 1 opens; the box ticks from then on (like its block entity in the game).
    pub(crate) fn shulker_lid_event(&mut self, p: P, count: i32) {
        let status = match count {
            0 => 3,
            1 => 1,
            _ => return,
        };
        match self.bs.shulkers.get_mut(&p) {
            Some(e) => e.1 = status,
            None => {
                let id = self.add_ticker(p);
                self.bs.shulkers.insert(p, (id, status, if status == 3 { 1.0 } else { 0.0 }));
            }
        }
    }

    /// Shulker lid ticker: `None` if it is not one, otherwise alive as long as the
    /// box is there.
    pub(crate) fn shulker_ticker(&self, id: u64, p: P) -> Option<bool> {
        let &(own, ..) = self.bs.shulkers.get(&p)?;
        (own == id).then(|| self.state_name(self.get(p)).ends_with("shulker_box"))
    }

    /// Lid tick (step of 0.1 in 32-bit float): at the start and at
    /// the end of an opening or a closing, neighbouring shapes then neighbours notified.
    pub(crate) fn shulker_tick(&mut self, p: P) {
        let Some(&(id, status, progress)) = self.bs.shulkers.get(&p) else { return };
        let old = progress;
        let (mut status, mut progress) = (status, progress);
        let mut updates = 0;
        match status {
            1 => {
                progress += 0.1;
                updates += i32::from(old == 0.0);
                if progress >= 1.0 {
                    (status, progress) = (2, 1.0);
                    updates += 1;
                }
            }
            3 => {
                progress -= 0.1;
                updates += i32::from(old == 1.0);
                if progress <= 0.0 {
                    (status, progress) = (0, 0.0);
                    updates += 1;
                }
            }
            0 => progress = 0.0,
            _ => progress = 1.0,
        }
        self.bs.shulkers.insert(p, (id, status, progress));
        let s = self.get(p);
        let b = self.v().block(s);
        for _ in 0..updates {
            self.update_neighbour_shapes(s, p, flag::ALL, 512);
            self.update_neighbors_at(p, b);
        }
    }

    pub(crate) fn hopper_ticker_alive(&self, id: u64, p: P) -> bool {
        self.bs.hoppers.get(&p).is_some_and(|h| h.0 == id)
            && matches!(self.v().kind(self.get(p)), Kind::Hopper | Kind::DaylightDetector)
    }

    /// Block entity created: the hoppers tick.
    pub(crate) fn after_block_entity_created(&mut self, p: P, s: u32) {
        if matches!(self.v().kind(s), Kind::Hopper | Kind::DaylightDetector) {
            let id = self.add_ticker(p);
            self.bs.hoppers.insert(p, (id, 0, i64::MIN));
        }
        // Furnaces and brewing stands tick as soon as they are created (and not on first opening): their
        // simulated state becomes the source of their contents.
        match self.cont_kind(s) {
            Cont::Furnace => self.ensure_furnace_at(p),
            Cont::Brewing => self.ensure_brewer(p),
            _ => {}
        }
        self.cmdblock_created(p, s); // S5: command blocks
        // F11: beehive or bee nest placed.
        if block_entity_id(self.state_name(s)) == Some("minecraft:beehive") {
            self.hive_register(p);
        }
        self.rec_block_entity_created(p, s); // F14: creaking heart, shrieker
        self.spawner_blocks_register(p); // W9: cages, trials, vaults
        match self.v().kind(s) {
            Kind::Campfire => self.campfire_ticker_register(p), // G6-F01
            Kind::Beacon => self.beacon_ticker_register(p),     // G5-F11
            _ if self.v().block(s) == self.bs.t.id.conduit => self.conduit_ticker_register(p), // G6-F12
            _ => {}
        }
        if self.state_name(s) == "minecraft:end_gateway" {
            self.gateway_ticker_register(p); // G5-F10
        }
    }

    /// Block-entity data replaced after its creation (command, components of the placed
    /// item): the simulated state of the furnace or brewing stand is reread from it, otherwise the empty state
    /// created with the block would mask then overwrite the given contents.
    pub(crate) fn reload_live_be(&mut self, p: P) {
        match self.cont_kind(self.get(p)) {
            Cont::Furnace => {
                self.life.furnaces.remove(&p);
                self.ensure_furnace_at(p);
            }
            Cont::Brewing => {
                self.life.brewers.remove(&p);
                self.ensure_brewer(p);
            }
            _ => {}
        }
    }

    pub(crate) fn register_chunk_tickers(&mut self, ch: (i32, i32)) {
        let Some(Some(c)) = self.chunks.get(&ch) else { return };
        let hoppers: Vec<(P, i32)> = c
            .block_entities
            .iter()
            .filter(|b| {
                matches!(b.get("id").and_then(Nbt::as_str), Some("minecraft:hopper" | "minecraft:daylight_detector"))
            })
            .map(|b| {
                let i = |k: &str| crate::nbtio::int(b, k).unwrap_or(0) as i32;
                ([i("x"), i("y"), i("z")], i("TransferCooldown"))
            })
            .collect();
        for (p, cd) in hoppers {
            let id = self.add_ticker(p);
            self.bs.hoppers.insert(p, (id, cd, i64::MIN));
        }
        // F11: beehives and bee nests of the chunk.
        let hives: Vec<P> = match self.chunks.get(&ch) {
            Some(Some(c)) => c
                .block_entities
                .iter()
                .filter(|b| b.get("id").and_then(Nbt::as_str) == Some("minecraft:beehive"))
                .map(|b| {
                    let i = |k: &str| crate::nbtio::int(b, k).unwrap_or(0) as i32;
                    [i("x"), i("y"), i("z")]
                })
                .collect(),
            _ => Vec::new(),
        };
        for p in hives {
            self.hive_register(p);
        }
        // G6-F01: campfires and jukeboxes (with a disc) of the chunk.
        let ticking: Vec<P> = match self.chunks.get(&ch) {
            Some(Some(c)) => c
                .block_entities
                .iter()
                .filter(|b| {
                    matches!(
                        b.get("id").and_then(Nbt::as_str),
                        Some(
                            "minecraft:campfire"
                                | "minecraft:jukebox"
                                | "minecraft:end_gateway"
                                | "minecraft:beacon"
                                | "minecraft:conduit"
                        )
                    )
                })
                .map(|b| {
                    let i = |k: &str| crate::nbtio::int(b, k).unwrap_or(0) as i32;
                    [i("x"), i("y"), i("z")]
                })
                .collect(),
            _ => Vec::new(),
        };
        for p in ticking {
            let s = self.get(p);
            match self.v().kind(s) {
                Kind::Campfire => self.campfire_ticker_register(p),
                _ if self.state_name(s) == "minecraft:end_gateway" => self.gateway_ticker_register(p),
                Kind::Beacon => self.beacon_ticker_register(p), // G5-F11
                _ if self.v().block(s) == self.bs.t.id.conduit => self.conduit_ticker_register(p), // G6-F12
                Kind::Jukebox => {
                    self.jukebox_loaded_from_save(p);
                    self.jukebox_ticker_sync(p, s);
                }
                _ => {}
            }
        }
        self.rec_register_chunk(ch); // F14: creaking hearts, shriekers
        self.spawner_blocks_register_chunk(ch); // W9: cages, trials, vaults
    }

    pub(crate) fn hopper_tick(&mut self, p: P) {
        if self.v().kind(self.get(p)) == Kind::DaylightDetector {
            if self.time % 20 == 0 {
                self.daylight_update(p);
            }
            return;
        }
        let Some(&(id, cd, _)) = self.bs.hoppers.get(&p) else { return };
        let cd = cd - 1;
        self.bs.hoppers.insert(p, (id, cd, self.time));
        if cd > 0 {
            return;
        }
        self.bs.hoppers.insert(p, (id, 0, self.time));
        self.hopper_try_move(p, None);
    }

    /// Out of cooldown and lit: empty forward, then fill (from above, or from the item on the
    /// ground `item` that just entered the hopper); cooldown of 8 if something moved.
    pub(crate) fn hopper_try_move(&mut self, p: P, item: Option<&mut crate::drops::ItemEnt>) -> bool {
        let Some(&(_, cd, _)) = self.bs.hoppers.get(&p) else { return false };
        let s = self.get(p);
        if cd > 0 || !self.v().get_bool(s, "enabled") {
            return false;
        }
        let mut changed = false;
        if !self.be_item_counts(p).is_empty() {
            changed = self.hopper_eject(p, s);
        }
        if !self.container_full(&[p], None) {
            changed |= match item {
                Some(d) => self.hopper_add_item(p, d),
                None => self.hopper_suck(p),
            };
        }
        if changed {
            if let Some(h) = self.bs.hoppers.get_mut(&p) {
                h.1 = 8;
            }
            self.be_set_int(p, "TransferCooldown", 8);
        }
        changed
    }

    fn hopper_eject(&mut self, p: P, s: u32) -> bool {
        let f = self.v().facing(s);
        let t = rel(p, f);
        let face = f.opposite();
        let composter = self.state_name(self.get(t)) == "minecraft:composter";
        let dest = if composter {
            if !self.composter_open(t, face) {
                return false;
            }
            Vec::new()
        } else {
            let Some(dest) = self.container_at(t) else {
                // G5-F06: chest or hopper minecart in the targeted slot.
                return self.cart_container_at(t).is_some_and(|cid| self.hopper_push_into_cart(p, cid));
            };
            self.unseal_all(&dest); // S8: sealed loot drawn
            if self.container_full(&dest, Some(face)) {
                return false;
            }
            dest
        };
        let mut slots = self.slots(p);
        for i in 0..slots.len() {
            let Some(it) = slots[i].clone() else { continue };
            let was_empty = self.hopper_is_empty(t);
            let taken = match self.composter_put(t, &it, face) {
                Some(taken) => taken,
                None => self.insert_one(&dest, &it, Some(face)),
            };
            if taken {
                let c = Self::item_count(&it) - 1;
                if c <= 0 {
                    slots[i] = None;
                } else if let Some(x) = &mut slots[i] {
                    crate::nbtio::set(x, "count", Nbt::Int(c));
                }
                self.store_slots(p, &slots);
                if was_empty {
                    self.hopper_filled(t, Some(p));
                }
                return true;
            }
        }
        false
    }

    /// Hopper at `t` is empty (false if it is not one).
    fn hopper_is_empty(&self, t: P) -> bool {
        self.v().kind(self.get(t)) == Kind::Hopper && self.slots(t).iter().all(Option::is_none)
    }

    /// Item entered the hopper `t` that was empty, coming from the hopper `from` or another
    /// container (dropper): transfer cooldown of 8, or 7 if `t` already ticked this tick after
    /// the source hopper; a longer cooldown (set by the data) is kept.
    fn hopper_filled(&mut self, t: P, from: Option<P>) {
        let mine = from.and_then(|p| self.bs.hoppers.get(&p)).map(|h| h.2);
        let Some(h) = self.bs.hoppers.get_mut(&t) else { return };
        if h.1 > 8 {
            return;
        }
        let skip = i32::from(mine.is_some_and(|m| h.2 >= m));
        h.1 = 8 - skip;
        self.be_set_int(t, "TransferCooldown", 8 - skip);
    }

    fn hopper_suck(&mut self, p: P) -> bool {
        let a = above(p);
        // Composter above: only its bone meal (never the items on the ground).
        if let Some(moved) = self.composter_take(a, p) {
            return moved;
        }
        // Without a container above: container minecart (G5-F06), otherwise items on the ground resting on
        // the hopper or just above.
        let Some(src) = self.container_at(a) else {
            if let Some(cid) = self.cart_container_at(a) {
                return self.hopper_pull_from_cart(p, cid);
            }
            return self.hopper_suck_items(p);
        };
        self.unseal_all(&src); // S8: sealed loot drawn
        for q in src {
            let kind = self.cont_kind(self.get(q));
            let mut slots = self.slots(q);
            for &i in face_slots(kind, slots.len(), Some(Dir::Down)) {
                let Some(it) = slots.get(i).cloned().flatten() else { continue };
                if !self.can_take(kind, i, &it, Dir::Down, p) {
                    continue;
                }
                if self.insert_one(&[p], &it, None) {
                    let c = Self::item_count(&it) - 1;
                    if c <= 0 {
                        slots[i] = None;
                    } else if let Some(x) = &mut slots[i] {
                        crate::nbtio::set(x, "count", Nbt::Int(c));
                    }
                    self.store_slots(q, &slots);
                    return true;
                }
            }
        }
        false
    }

    // --- Dispensers and droppers (§ 3.13). ---

    /// Tick of the triggered dispenser or dropper: slot drawn at random
    /// among the non-empty ones, item dispensed, slot rewritten with what remains.
    pub(crate) fn dispense_from(&mut self, s: u32, p: P) {
        let kind = self.v().kind(s);
        if kind == Kind::Crafter {
            return;
        }
        let slots = self.slots(p);
        // Uniform draw among the non-empty slots (reservoir).
        let mut chosen = None;
        let mut seen = 1;
        for (i, it) in slots.iter().enumerate() {
            if it.is_some() {
                if self.bs.rng.next_int_bound(seen) == 0 {
                    chosen = Some(i);
                }
                seen += 1;
            }
        }
        let Some(i) = chosen else {
            // Empty reservoir: failure click; the dispenser also emits `block_activate`.
            self.level_event(1001, p, 0);
            if kind != Kind::Dropper {
                self.game_event_block("minecraft:block_activate", p, None, Some(s));
            }
            return;
        };
        let Some(stack) = slots[i].as_ref().and_then(crate::player::Stack::from_nbt) else { return };
        let f = self.v().facing(s);
        let rest = if kind == Kind::Dropper { self.drop_from(p, f, stack) } else { self.dispense_item(p, f, i, stack) };
        let mut slots = self.slots(p);
        if let Some(slot) = slots.get_mut(i) {
            *slot = rest.filter(|r| r.count > 0).map(|r| r.to_nbt(None));
            self.store_slots(p, &slots);
        }
    }

    /// Dropper: one copy poured into the container in front
    /// (no sound, stack untouched if refused), otherwise thrown like by the dispenser.
    fn drop_from(&mut self, p: P, f: Dir, stack: crate::player::Stack) -> Option<crate::player::Stack> {
        let front = rel(p, f);
        let one = crate::player::Stack { count: 1, ..stack.clone() }.to_nbt(None);
        let taken = match self.composter_put(front, &one, f.opposite()) {
            Some(t) => Some(t),
            None => self.container_at(front).map(|dest| {
                self.unseal_all(&dest); // S8: sealed loot drawn
                let was_empty = self.hopper_is_empty(front);
                let ok = self.insert_one(&dest, &one, Some(f.opposite()));
                if ok && was_empty {
                    self.hopper_filled(front, None);
                }
                ok
            }),
        };
        match taken {
            Some(true) => super::dispense_behaviors::shrink(stack),
            Some(false) => Some(stack),
            None => self.dispense_default(p, f, stack),
        }
    }

    // --- Rails: survival (powering of powered rails remains to be done). ---

    fn rail_survives(&self, s: u32, p: P) -> bool {
        let v = self.v();
        if !v.rigid_up(self.support_state(below(p))) {
            return false;
        }
        let shape = v.get(s, "shape").unwrap_or("");
        let up = match shape {
            "ascending_east" => Some(Dir::East),
            "ascending_west" => Some(Dir::West),
            "ascending_north" => Some(Dir::North),
            "ascending_south" => Some(Dir::South),
            _ => None,
        };
        up.is_none_or(|d| v.rigid_up(self.support_state(rel(p, d))))
    }

    pub(crate) fn rail_on_place(&mut self, p: P, s: u32, moved: bool) {
        self.rail_placed(p, s, moved);
    }

    pub(crate) fn rail_neighbor_changed(&mut self, s: u32, p: P, from: u16) {
        if self.v().block(self.get(p)) != self.v().block(s) {
            return;
        }
        if !self.rail_survives(s, p) {
            // The loot of a destroyed block then removal: the rail falls as an item (without break particles).
            self.drop_resources(p, s, None);
            self.remove_block(p, false);
        } else {
            match self.v().kind(s) {
                Kind::PoweredRail => self.powered_rail_update(s, p),
                Kind::Rail => self.plain_rail_update(s, p, from),
                _ => {}
            }
        }
    }

    /// Powered / activator rail (§ 3.10): powered directly or linked to a powered rail.
    fn powered_rail_update(&mut self, s: u32, p: P) {
        let powered = self.v().get_bool(s, "powered");
        let should = self.has_neighbor_signal(p) || self.rail_signal(p, s, true, 0) || self.rail_signal(p, s, false, 0);
        if should != powered {
            let ns = self.v().set_bool(s, "powered", should);
            self.set_update(p, ns);
            let b = self.v().block(s);
            self.update_neighbors_at(below(p), b);
            if self.v().get(s, "shape").is_some_and(|x| x.starts_with("ascending")) {
                self.update_neighbors_at(above(p), b);
            }
        }
    }

    fn rail_signal(&self, p: P, s: u32, forward: bool, depth: i32) -> bool {
        if depth >= 8 {
            return false;
        }
        let [mut x, mut y, mut z] = p;
        let mut below_ok = true;
        let shape = self.v().get(s, "shape").unwrap_or("north_south").to_owned();
        let ns_axis = match shape.as_str() {
            "north_south" => {
                z += if forward { 1 } else { -1 };
                true
            }
            "east_west" => {
                x += if forward { -1 } else { 1 };
                false
            }
            "ascending_east" => {
                if forward {
                    x -= 1;
                } else {
                    x += 1;
                    y += 1;
                    below_ok = false;
                }
                false
            }
            "ascending_west" => {
                if forward {
                    x -= 1;
                    y += 1;
                    below_ok = false;
                } else {
                    x += 1;
                }
                false
            }
            "ascending_north" => {
                if forward {
                    z += 1;
                } else {
                    z -= 1;
                    y += 1;
                    below_ok = false;
                }
                true
            }
            "ascending_south" => {
                if forward {
                    z += 1;
                    y += 1;
                    below_ok = false;
                } else {
                    z -= 1;
                }
                true
            }
            _ => return false,
        };
        self.rail_same_powered([x, y, z], s, forward, depth, ns_axis)
            || (below_ok && self.rail_same_powered([x, y - 1, z], s, forward, depth, ns_axis))
    }

    fn rail_same_powered(&self, q: P, me: u32, forward: bool, depth: i32, ns_axis: bool) -> bool {
        let st = self.get(q);
        if self.v().block(st) != self.v().block(me) {
            return false;
        }
        let v = self.v();
        let shape = v.get(st, "shape").unwrap_or("");
        let cross = if ns_axis {
            matches!(shape, "east_west" | "ascending_east" | "ascending_west")
        } else {
            matches!(shape, "north_south" | "ascending_north" | "ascending_south")
        };
        if cross || !self.v().get_bool(st, "powered") {
            return false;
        }
        self.has_neighbor_signal(q) || self.rail_signal(q, st, forward, depth + 1)
    }

    pub(crate) fn rail_after_removal(&mut self, s: u32, p: P, moved: bool) {
        if !moved {
            let b = self.v().block(s);
            if self.v().get(s, "shape").is_some_and(|x| x.starts_with("ascending")) {
                self.update_neighbors_at(above(p), b);
            }
            if matches!(self.v().kind(s), Kind::PoweredRail | Kind::DetectorRail) {
                self.update_neighbors_at(p, b);
                self.update_neighbors_at(below(p), b);
            }
        }
    }
}

#[cfg(test)]
#[path = "entities_tests.rs"]
mod tests;
