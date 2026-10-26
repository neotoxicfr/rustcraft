//! Beehives and bee nests (§ 4.F11): occupants kept in the block entity (`bees`: entity
//! data, ticks spent, minimum ticks; `flower_pos`), a bee entering, exit at the
//! deadline (nectar → honey), emergency exit (breaking, harvest without smoke), anger of the
//! neighboring bees, harvest with shears or bottle, smoking by a campfire. The block entity
//! ticks with the hoppers (`tick_block_entities`), in their creation order.

use rc_proto::nbt::Nbt;

use super::super::mob::{Ent, Mob};
use super::super::species::Kind;
use crate::nbtio::{self, int};
use crate::player::Stack;
use crate::server::Server;

/// Entity fields removed on entry as on exit.
pub const IGNORED: [&str; 25] = [
    "Air",
    "drop_chances",
    "equipment",
    "Brain",
    "CanPickUpLoot",
    "DeathTime",
    "fall_distance",
    "FallFlying",
    "Fire",
    "HurtTime",
    "LeftHanded",
    "Motion",
    "NoGravity",
    "OnGround",
    "PortalCooldown",
    "Pos",
    "Rotation",
    "sleeping_pos",
    "CannotEnterHiveTicks",
    "TicksSincePollination",
    "CropsGrownSincePollination",
    "hive_pos",
    "Passengers",
    "leash",
    "UUID",
];

pub const MAX_OCCUPANTS: usize = 3;

/// Reason for the exit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Release {
    HoneyDelivered,
    BeeReleased,
    Emergency,
}

/// World state: ticking hives (position → ticker id), stingers planted in the
/// player and delay before removing the next.
#[derive(Debug, Default)]
pub struct HiveWorld {
    pub tickers: hashbrown::HashMap<[i32; 3], u64>,
    pub player_stingers: i32,
    pub remove_stinger_time: i32,
}

fn is_hive_block(name: &str) -> bool {
    matches!(name, "minecraft:beehive" | "minecraft:bee_nest")
}

/// Hive block entity present at the position (loaded chunk).
pub fn is_hive_entity(s: &Server, p: [i32; 3]) -> bool {
    s.block_entity(p).and_then(|b| b.get("id")).and_then(Nbt::as_str) == Some("minecraft:beehive")
}

fn occupants(s: &Server, p: [i32; 3]) -> Vec<Nbt> {
    match s.block_entity(p).and_then(|b| b.get("bees")) {
        Some(Nbt::List(l)) => l.clone(),
        _ => Vec::new(),
    }
}

pub fn occupant_count(s: &Server, p: [i32; 3]) -> usize {
    occupants(s, p).len()
}

pub fn is_full(s: &Server, p: [i32; 3]) -> bool {
    occupant_count(s, p) == MAX_OCCUPANTS
}

fn flower_of(s: &Server, p: [i32; 3]) -> Option<[i32; 3]> {
    match s.block_entity(p).and_then(|b| b.get("flower_pos")) {
        Some(Nbt::IntArray(a)) if a.len() == 3 => Some([a[0], a[1], a[2]]),
        _ => None,
    }
}

fn store(s: &mut Server, p: [i32; 3], bees: Vec<Nbt>, flower: Option<[i32; 3]>) {
    let Some(be) = s.block_entity_mut(p, "minecraft:beehive") else { return };
    nbtio::set(be, "bees", Nbt::List(bees));
    match flower {
        Some(f) => nbtio::set(be, "flower_pos", Nbt::IntArray(vec![f[0], f[1], f[2]])),
        None => nbtio::remove(be, "flower_pos"),
    }
}

/// Fire (`fire` block) in the 3 × 3 × 3 cube around the hive.
pub fn fire_nearby(s: &Server, p: [i32; 3]) -> bool {
    for x in -1..=1 {
        for y in -1..=1 {
            for z in -1..=1 {
                if s.bname([p[0] + x, p[1] + y, p[2] + z]) == "minecraft:fire" {
                    return true;
                }
            }
        }
    }
    false
}

fn lit_campfire(s: &Server, p: [i32; 3]) -> bool {
    s.btag_id(p, crate::tag_bits::TagId::Campfires) && s.data.state_properties(s.st(p)).any(|kv| kv == ("lit", "true"))
}

/// Campfire lit up to 5 blocks below; a block whose
/// collision touches the virtual pole (4-pixel column) stops the smoke.
pub fn smokey(s: &Server, p: [i32; 3]) -> bool {
    // A single box buffer for the 5 cells.
    let mut boxes = Vec::new();
    for i in 1..=5 {
        let q = [p[0], p[1] - i, p[2]];
        if lit_campfire(s, q) {
            return true;
        }
        boxes.clear();
        s.phys.collision(s.st(q), &mut boxes);
        let (lo, hi) = (6.0 / 16.0, 10.0 / 16.0);
        let blocked = boxes.iter().any(|b| {
            b.min[0] < hi && b.max[0] > lo && b.min[2] < hi && b.max[2] > lo && b.max[1] > 0.0 && b.min[1] < 1.0
        });
        if blocked {
            return lit_campfire(s, [q[0], q[1] - 1, q[2]]);
        }
    }
    false
}

fn facing(s: &Server, p: [i32; 3]) -> [i32; 3] {
    let st = s.st(p);
    match s.data.state_properties(st).find(|(k, _)| *k == "facing").map(|(_, v)| v) {
        Some("south") => [0, 0, 1],
        Some("west") => [-1, 0, 0],
        Some("east") => [1, 0, 0],
        _ => [0, 0, -1],
    }
}

fn honey_level(s: &Server, p: [i32; 3]) -> Option<i32> {
    s.data.state_properties(s.st(p)).find(|(k, _)| *k == "honey_level").and_then(|(_, v)| v.parse().ok())
}

fn set_honey(s: &mut Server, p: [i32; 3], level: i32) {
    let st = s.st(p);
    if let Some(ns) = s.with_prop(st, "honey_level", &level.to_string()) {
        s.set_block(p, ns, 3);
    }
}

// ---------- Entry ----------

/// Bee stored (data without the ignored fields), flower passed on, entry sound
/// at the corner of the block, bee removed from the world.
pub fn add_occupant(s: &mut Server, p: [i32; 3], m: &mut Mob) {
    let mut bees = occupants(s, p);
    if bees.len() >= MAX_OCCUPANTS {
        return;
    }
    let mut data = m.save();
    for k in IGNORED {
        nbtio::remove(&mut data, k);
    }
    let nectar = int(&data, "HasNectar").is_some_and(|v| v != 0);
    bees.push(Nbt::Compound(vec![
        ("entity_data".into(), data),
        ("ticks_in_hive".into(), Nbt::Int(0)),
        ("min_ticks_in_hive".into(), Nbt::Int(if nectar { 2400 } else { 600 })),
    ]));
    let mut flower = flower_of(s, p);
    if let Some(f) = m.wild.flower
        && (flower.is_none() || s.mobs.rng.next_bool())
    {
        flower = Some(f);
    }
    store(s, p, bees, flower);
    let at = [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])];
    s.world_sound("minecraft:block.beehive.enter", 4, at, 1.0, 1.0);
    m.removed = true;
}

// ---------- Exit ----------

/// Refused at night or in rain and in front of a block (except emergency);
/// bee recreated without gravity, hive and age updated, flower taken back (9 in 10), nectar
/// deposited (honey + 1, 1 in 100: + 2), placed in front of the hive; exit sound.
fn release_occupant(s: &mut Server, p: [i32; 3], occ: &Nbt, status: Release, flower: Option<[i32; 3]>) -> Option<i32> {
    if super::bee::stay_in_hive(s) && status != Release::Emergency {
        return None;
    }
    let f = facing(s, p);
    let front = [p[0] + f[0], p[1], p[2] + f[2]];
    let blocked = s.has_collision(front);
    if blocked && status != Release::Emergency {
        return None;
    }
    let mut data = occ.get("entity_data").cloned().unwrap_or(Nbt::Compound(Vec::new()));
    let kind = data.get("id").and_then(Nbt::as_str).and_then(Kind::from_name);
    // Only the hive inhabitants (`beehive_inhabitors`: the bee) come back out.
    if kind != Some(Kind::Bee) {
        return None;
    }
    for k in IGNORED {
        nbtio::remove(&mut data, k);
    }
    let ticks = occ.get("ticks_in_hive").and_then(Nbt::as_i64).unwrap_or(0) as i32;
    s.next_entity += 1;
    let id = s.next_entity;
    let mut rng = super::super::jrand::JRand::from_time(i64::from(id));
    let uuid = (rng.next_long() as u128) << 64 | rng.next_long() as u64 as u128;
    let uuid =
        (uuid & !(0xF000u128 << 64) | (0x4000u128 << 64)) & !(0xC000_0000_0000_0000u128) | 0x8000_0000_0000_0000u128;
    let mut m = Mob::new(Kind::Bee, id, uuid, [0.0; 3], 0.0, rng);
    m.load(&data);
    m.uuid = uuid;
    s.init_mob(&mut m);
    m.surf.no_gravity = true;
    m.wild.hive = Some(p);
    // Age brought closer to zero, love reduced by the time spent inside.
    if !m.mount.age_locked {
        if m.age < 0 {
            m.age = (m.age + ticks).min(0);
        } else if m.age > 0 {
            m.age = (m.age - ticks).max(0);
        }
    }
    m.in_love = (m.in_love - ticks).max(0);
    if flower.is_some() && m.wild.flower.is_none() && s.mobs.rng.next_float() < 0.9 {
        m.wild.flower = flower;
    }
    if status == Release::HoneyDelivered {
        super::bee::set_nectar(&mut m.wild, false);
        m.wild.crops = 0;
        if let Some(level) = honey_level(s, p)
            && level < 5
        {
            let mut inc = if s.mobs.rng.next_int_bound(100) == 0 { 2 } else { 1 };
            if level + inc > 5 {
                inc -= 1;
            }
            set_honey(s, p, level + inc);
        }
    }
    let delta = if blocked { 0.0 } else { 0.55 + f64::from(m.width() / 2.0) };
    m.pos = [
        f64::from(p[0]) + 0.5 + delta * f64::from(f[0]),
        f64::from(p[1]) + 0.5 - f64::from(m.height() / 2.0),
        f64::from(p[2]) + 0.5 + delta * f64::from(f[2]),
    ];
    m.old = m.pos;
    let at = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
    s.world_sound("minecraft:block.beehive.exit", 4, at, 1.0, 1.0);
    s.add_mob(m);
    Some(id)
}

/// Each occupant tries to exit, those that exit leave the hive.
fn release_all(s: &mut Server, p: [i32; 3], status: Release) -> Vec<i32> {
    let bees = occupants(s, p);
    let flower = flower_of(s, p);
    let mut kept = Vec::new();
    let mut out = Vec::new();
    for b in bees {
        match release_occupant(s, p, &b, status, flower) {
            Some(id) => out.push(id),
            None => kept.push(b),
        }
    }
    if !out.is_empty() {
        store(s, p, kept, flower);
    }
    out
}

/// All outside; those within 4 blocks of the player attack them (or
/// stay outside 400 ticks if the hive is smoked).
pub fn empty_all(s: &mut Server, p: [i32; 3], player: bool, status: Release) {
    let released = release_all(s, p, status);
    if !player {
        return;
    }
    let sedated = smokey(s, p);
    for id in released {
        let pp = s.pos;
        if let Some(i) = s.mob_index(id)
            && let Some(b) = s.mobs.list[i].as_mut()
            && super::super::mth::dist2(pp, b.pos) <= 16.0
        {
            if sedated {
                b.wild.stay_out = 400;
            } else {
                b.target = Some(Ent::Player);
            }
        }
    }
}

/// Bees without a target within ±8/±6/±8 → a random player of the area.
pub fn anger_nearby(s: &mut Server, p: [i32; 3]) {
    let area = rc_physics::Aabb {
        min: [f64::from(p[0]) - 8.0, f64::from(p[1]) - 6.0, f64::from(p[2]) - 8.0],
        max: [f64::from(p[0]) + 9.0, f64::from(p[1]) + 7.0, f64::from(p[2]) + 9.0],
    };
    let any_bee = s.mobs.of_kind(Kind::Bee).any(|o| o.touches(&area));
    if !any_bee || !s.player_aabb().intersects(&area) {
        return;
    }
    let ids: Vec<i32> =
        s.mobs.of_kind(Kind::Bee).filter(|o| o.touches(&area)).map(|o| o.id).collect();
    for id in ids {
        let Some(i) = s.mob_index(id) else { continue };
        if s.mobs.list[i].as_ref().is_some_and(|b| b.target.is_none()) {
            // The random draw of an element on the player list (a single one): one draw.
            let _ = s.mobs.rng.next_int_bound(1);
            if let Some(b) = s.mobs.list[i].as_mut() {
                b.target = Some(Ent::Player);
            }
        }
    }
}

// ---------- Tick ----------

impl Server {
    /// Hive that became a block entity (placement, generation, reading): it ticks.
    pub(crate) fn hive_register(&mut self, p: [i32; 3]) {
        if let Some(&id) = self.mobs.wild.tickers.get(&p)
            && self.bs.tickers.iter().chain(self.bs.pending_tickers.iter()).any(|t| *t == (id, p))
        {
            return;
        }
        let id = self.add_ticker(p);
        self.mobs.wild.tickers.insert(p, id);
    }

    /// Ticker still valid: hive block and block entity present.
    pub(crate) fn hive_ticker_alive(&self, id: u64, p: [i32; 3]) -> bool {
        self.mobs.wild.tickers.get(&p) == Some(&id) && is_hive_block(self.bname(p)) && is_hive_entity(self, p)
    }

    pub(crate) fn hive_is_ticker(&self, id: u64, p: [i32; 3]) -> bool {
        self.mobs.wild.tickers.get(&p) == Some(&id)
    }

    /// Occupants released at the deadline, buzzing (1 in
    /// 200, world random) if bees remain.
    pub(crate) fn hive_tick(&mut self, p: [i32; 3]) {
        let bees = occupants(self, p);
        if !bees.is_empty() {
            let flower = flower_of(self, p);
            let mut kept = Vec::with_capacity(bees.len());
            for mut b in bees {
                let t = int(&b, "ticks_in_hive").unwrap_or(0) as i32;
                let min = int(&b, "min_ticks_in_hive").unwrap_or(600) as i32;
                // The counter of time spent in the hive is compared to the minimum before its increment: the comparison sees the old value.
                nbtio::set(&mut b, "ticks_in_hive", Nbt::Int(t + 1));
                if t > min {
                    let nectar = b.get("entity_data").and_then(|e| int(e, "HasNectar")).is_some_and(|v| v != 0);
                    let st = if nectar { Release::HoneyDelivered } else { Release::BeeReleased };
                    if release_occupant(self, p, &b, st, flower).is_some() {
                        continue;
                    }
                }
                kept.push(b);
            }
            store(self, p, kept, flower);
        }
        if occupant_count(self, p) > 0 && self.mobs.rng.next_double() < 0.005 {
            let at = [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5];
            self.world_sound("minecraft:block.beehive.work", 4, at, 1.0, 1.0);
        }
    }

    /// Click with an item on a honey-full hive: shears (honeycombs,
    /// wear) or bottle (honey bottle); without smoke, anger and emergency exit. True if handled.
    pub(crate) fn hive_use_item_on(&mut self, slot: usize, p: [i32; 3], item: &str) -> bool {
        if !is_hive_block(self.bname(p)) || honey_level(self, p).unwrap_or(0) < 5 {
            return false;
        }
        match item {
            "minecraft:shears" => {
                let drops =
                    super::super::surface::loot(self, "minecraft:harvest/beehive", &crate::loot::Ctx::default());
                for d in drops {
                    self.pop_resource(p, d);
                }
                let pp = self.pos;
                self.world_sound("minecraft:block.beehive.shear", 4, pp, 1.0, 1.0);
                self.hurt_item(slot, 1);
                self.game_event_block("minecraft:shear", p, Some(super::super::mob::Ent::Player), None);
            }
            "minecraft:glass_bottle" => {
                // `shrink(1)` without looking at the mode; the fill sound excludes the player (sound
                // predicted by their client).
                if let Some(st) = &mut self.slots[slot] {
                    st.count -= 1;
                }
                let honey = Stack { id: "minecraft:honey_bottle".into(), count: 1, components: None };
                if self.slots[slot].as_ref().is_none_or(|s| s.count <= 0) {
                    self.slots[slot] = Some(honey);
                } else {
                    let creative = self.game_mode == 1;
                    let rest = crate::menus::inventory_add(
                        &mut self.slots,
                        &self.life.items,
                        self.held as usize,
                        honey,
                        creative,
                    );
                    if let Some(r) = rest {
                        self.drop_from_player(r, false);
                    }
                }
                self.game_event_block("minecraft:fluid_pickup", p, Some(super::super::mob::Ent::Player), None);
            }
            _ => return false,
        }
        // "Used" statistic of the tool (shears or bottle), counted like the game.
        self.award_stat("minecraft:used", item, 1);
        if smokey(self, p) {
            set_honey(self, p, 0);
        } else {
            if occupant_count(self, p) > 0 {
                anger_nearby(self, p);
            }
            set_honey(self, p, 0);
            empty_all(self, p, true, Release::Emergency);
        }
        self.broadcast_menus();
        true
    }

    /// Honey level of the hive or nest at `p` (`None`: other block).
    pub(crate) fn hive_honey(&self, p: [i32; 3]) -> Option<i32> {
        if !is_hive_block(self.bname(p)) {
            return None;
        }
        honey_level(self, p)
    }

    /// Hive emptied without a player (dispenser): honey to 0,
    /// bees released, without anger.
    pub(crate) fn hive_release_by_dispenser(&mut self, p: [i32; 3]) {
        set_honey(self, p, 0);
        empty_all(self, p, false, Release::BeeReleased);
    }

    /// Honeycombs of the harvested hive, placed at the center of the block.
    pub(crate) fn hive_drop_honeycomb(&mut self, p: [i32; 3]) {
        let drops = super::super::surface::loot(self, "minecraft:harvest/beehive", &crate::loot::Ctx::default());
        for d in drops {
            self.pop_resource(p, d);
        }
    }

    /// Hive broken by the player without silk touch: emergency exit,
    /// anger of the neighboring bees.
    pub(crate) fn hive_broken(&mut self, p: [i32; 3], block: &str, tool: Option<&Stack>) {
        if !is_hive_block(block) || !is_hive_entity(self, p) {
            return;
        }
        if tool.is_some_and(|t| self.life.items.enchantment(t, "minecraft:silk_touch") > 0) {
            return;
        }
        empty_all(self, p, true, Release::Emergency);
        anger_nearby(self, p);
    }

    /// Fire (simple, not soul fire) placed next to a hive: emergency exit, without anger.
    pub(crate) fn hive_fire_near(&mut self, p: [i32; 3]) {
        if is_hive_entity(self, p) {
            empty_all(self, p, false, Release::Emergency);
        }
    }

    /// Hive blown up, before its loot: emergency exit when the direct source of the explosion
    /// is a TNT, a creeper, a skull or the wither, a TNT minecart.
    pub(crate) fn hive_blown(&mut self, p: [i32; 3], releases: bool) {
        if releases && is_hive_entity(self, p) {
            empty_all(self, p, false, Release::Emergency);
        }
    }

    /// Stingers planted in the player (metadata 13): one removed every 20 × (30 − n) ticks.
    pub(crate) fn player_stingers_tick(&mut self) {
        let w = &mut self.mobs.wild;
        if w.player_stingers <= 0 {
            return;
        }
        if w.remove_stinger_time <= 0 {
            w.remove_stinger_time = 20 * (30 - w.player_stingers);
        }
        w.remove_stinger_time -= 1;
        if w.remove_stinger_time <= 0 {
            w.player_stingers -= 1;
            let n = w.player_stingers;
            self.send_player_stingers(n);
        }
    }

    fn send_player_stingers(&self, n: i32) {
        let values = vec![(13, rc_proto_v777::entity::Meta::Int(n))];
        let _ = self.send(rc_client::Event::Entity(rc_client::EntityEvent::Data { id: self.player_id, values }));
    }
}

/// Stinger planted in the player.
pub fn add_player_stinger(s: &mut Server) {
    s.mobs.wild.player_stingers += 1;
    let n = s.mobs.wild.player_stingers;
    s.send_player_stingers(n);
}
