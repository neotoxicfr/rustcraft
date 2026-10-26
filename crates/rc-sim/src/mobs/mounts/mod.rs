//! Mounts (family F04, `server-mobs.md` § 4.F04): horse, donkey, mule, llama, trader
//! llama, skeleton horse, zombie horse, camel, camel husk. Info sheets, own state, metadata, NBT,
//! sounds, food and dispatch of the hooks called by the common engine.

pub(crate) mod camel;
pub(crate) mod feed;
pub(crate) mod goals;
pub(crate) mod llama;
pub(crate) mod menu;
pub(crate) mod riding;
pub(crate) mod trap;

pub use llama::World;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod world_tests;

use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::mob::{Ent, Mob};
use super::species::{Category, Info, Kind, info as base_info};
use crate::nbtio::{self, int};
use crate::player::Stack;
use crate::server::Server;

// ---------- Info sheets ----------

/// Common base of equines: walk 1, volume 0.8, ambient sound every 400 ticks at most.
const fn equine(name: &'static str, cat: Category, w: f32, h: f32, eye: f32, health: f64, speed: f64) -> Info {
    let mut i = base_info(name, cat, w, h, eye);
    i.health = health;
    i.speed = speed;
    i.step = 1.0;
    i.volume = 0.8;
    i.ambient_interval = 400;
    i
}

const HORSE_W: f32 = 1.396_484_4;

static HORSE: Info = {
    let mut i = equine("minecraft:horse", Category::Creature, HORSE_W, 1.6, 1.52, 53.0, 0.225f32 as f64);
    i.ambient = Some("minecraft:entity.horse.ambient");
    i.hurt = "minecraft:entity.horse.hurt";
    i.death = "minecraft:entity.horse.death";
    i
};

static DONKEY: Info = {
    let mut i = equine("minecraft:donkey", Category::Creature, HORSE_W, 1.5, 1.425, 53.0, 0.175f32 as f64);
    i.ambient = Some("minecraft:entity.donkey.ambient");
    i.hurt = "minecraft:entity.donkey.hurt";
    i.death = "minecraft:entity.donkey.death";
    i
};

static MULE: Info = {
    let mut i = equine("minecraft:mule", Category::Creature, HORSE_W, 1.6, 1.52, 53.0, 0.175f32 as f64);
    i.tracking = 8;
    i.ambient = Some("minecraft:entity.mule.ambient");
    i.hurt = "minecraft:entity.mule.hurt";
    i.death = "minecraft:entity.mule.death";
    i
};

static LLAMA: Info = {
    let mut i = equine("minecraft:llama", Category::Creature, 0.9, 1.87, 1.7765, 53.0, 0.175f32 as f64);
    i.ambient = Some("minecraft:entity.llama.ambient");
    i.hurt = "minecraft:entity.llama.hurt";
    i.death = "minecraft:entity.llama.death";
    i
};

static TRADER_LLAMA: Info = {
    let mut i = equine("minecraft:trader_llama", Category::Creature, 0.9, 1.87, 1.7765, 53.0, 0.175f32 as f64);
    i.ambient = Some("minecraft:entity.llama.ambient");
    i.hurt = "minecraft:entity.llama.hurt";
    i.death = "minecraft:entity.llama.death";
    i
};

static SKELETON_HORSE: Info = {
    let mut i = equine("minecraft:skeleton_horse", Category::Creature, HORSE_W, 1.6, 1.52, 15.0, 0.2f32 as f64);
    i.ambient = Some("minecraft:entity.skeleton_horse.ambient");
    i.hurt = "minecraft:entity.skeleton_horse.hurt";
    i.death = "minecraft:entity.skeleton_horse.death";
    i
};

/// Zombie horse: "monster" category (spawning) but an animal by its class.
static ZOMBIE_HORSE: Info = {
    let mut i = equine("minecraft:zombie_horse", Category::Monster, HORSE_W, 1.6, 1.52, 25.0, 0.225f32 as f64);
    i.ambient = Some("minecraft:entity.zombie_horse.ambient");
    i.hurt = "minecraft:entity.zombie_horse.hurt";
    i.death = "minecraft:entity.zombie_horse.death";
    i.burns = true;
    i
};

static CAMEL: Info = {
    let mut i = equine("minecraft:camel", Category::Creature, 1.7, 2.375, 2.275, 32.0, 0.09f32 as f64);
    i.step = 1.5;
    i.ambient = Some("minecraft:entity.camel.ambient");
    i.hurt = "minecraft:entity.camel.hurt";
    i.death = "minecraft:entity.camel.death";
    i
};

static CAMEL_HUSK: Info = {
    let mut i = equine("minecraft:camel_husk", Category::Monster, 1.7, 2.375, 2.275, 32.0, 0.09f32 as f64);
    i.step = 1.5;
    i.ambient = Some("minecraft:entity.camel_husk.ambient");
    i.hurt = "minecraft:entity.camel_husk.hurt";
    i.death = "minecraft:entity.camel_husk.death";
    i
};

pub fn info(k: Kind) -> &'static Info {
    match k {
        Kind::Horse => &HORSE,
        Kind::Donkey => &DONKEY,
        Kind::Mule => &MULE,
        Kind::Llama => &LLAMA,
        Kind::TraderLlama => &TRADER_LLAMA,
        Kind::SkeletonHorse => &SKELETON_HORSE,
        Kind::ZombieHorse => &ZOMBIE_HORSE,
        Kind::Camel => &CAMEL,
        _ => &CAMEL_HUSK,
    }
}

/// Species of the family.
pub fn handles(k: Kind) -> bool {
    matches!(
        k,
        Kind::Horse
            | Kind::Donkey
            | Kind::Mule
            | Kind::Llama
            | Kind::TraderLlama
            | Kind::SkeletonHorse
            | Kind::ZombieHorse
            | Kind::Camel
            | Kind::CamelHusk
    )
}

pub fn is_llama(k: Kind) -> bool {
    matches!(k, Kind::Llama | Kind::TraderLlama)
}

pub fn is_camel(k: Kind) -> bool {
    matches!(k, Kind::Camel | Kind::CamelHusk)
}

/// Tag `dismounts_underwater` (simulated species): the passenger dismounts when their eyes go
/// under water. Neither the skeleton horse nor the camel husk is part of it.
pub fn dismounts_underwater(k: Kind) -> bool {
    matches!(
        k,
        Kind::Camel
            | Kind::Chicken
            | Kind::Donkey
            | Kind::Horse
            | Kind::Llama
            | Kind::Mule
            | Kind::Pig
            | Kind::Spider
            | Kind::TraderLlama
            | Kind::ZombieHorse
            | Kind::Ravager
            | Kind::HappyGhast
            | Kind::Strider
    )
}

/// Equine carrying a chest (donkey, mule, llamas).
pub fn chested(k: Kind) -> bool {
    matches!(k, Kind::Donkey | Kind::Mule | Kind::Llama | Kind::TraderLlama)
}

/// Undead of the family (`#minecraft:skeletons`, `#minecraft:zombies`).
pub fn undead(k: Kind) -> bool {
    matches!(k, Kind::SkeletonHorse | Kind::ZombieHorse | Kind::CamelHusk)
}

/// Dimensions of babies (width, height, eyes): ×0.7 horse, ×0.5 chest carriers and
/// llamas, camel its own.
pub fn baby_dims(k: Kind) -> (f32, f32, f32) {
    let i = info(k);
    match k {
        Kind::Horse | Kind::SkeletonHorse | Kind::ZombieHorse => (i.width * 0.7, i.height * 0.7, i.eye * 0.7),
        Kind::Camel | Kind::CamelHusk => (0.95, 1.4, 1.38),
        _ => (i.width * 0.5, i.height * 0.5, i.eye * 0.5),
    }
}

/// Dimensions by posture (sitting camel); `None`: common dimensions.
pub fn pose_dims(m: &Mob) -> Option<(f32, f32, f32)> {
    if !is_camel(m.kind) || !m.mount.camel_sitting_pose {
        return None;
    }
    Some(if m.is_baby() { (0.95, 0.425, 0.41) } else { (1.7, 2.375 - 1.43, 0.845) })
}

/// Age scale: 0.6 camel, 0.5 otherwise.
pub fn age_scale(m: &Mob) -> f32 {
    if !m.is_baby() {
        1.0
    } else if is_camel(m.kind) {
        0.6
    } else {
        0.5
    }
}

// ---------- Own state ----------

/// Equine flags (index 18).
pub const FLAG_TAME: i8 = 2;
pub const FLAG_BRED: i8 = 8;
pub const FLAG_EATING: i8 = 16;
pub const FLAG_STANDING: i8 = 32;
pub const FLAG_MOUTH: i8 = 64;

/// Equipment slots (engine order).
pub const BODY: usize = 6;
pub const SADDLE: usize = 7;

#[derive(Debug, Clone, Default)]
pub struct Mount {
    // Passengers (all species): ordered list, direct vehicle.
    pub passengers: Vec<Ent>,
    pub vehicle: Option<i32>,
    /// Passengers of non-simulated species (NBT kept, rewritten on save).
    pub foreign_passengers: Vec<Nbt>,
    /// Mob passengers of the `non_controlling_rider` tag (cubes): never pilots.
    pub passive_riders: Vec<i32>,
    /// List seen at the last `set_passengers` send.
    pub sent_passengers: Vec<i32>,
    pub boarding_cooldown: i32,
    // Equine.
    pub flags: i8,
    pub temper: i32,
    pub owner: Option<u128>,
    pub eating_counter: i32,
    pub mouth_counter: i32,
    pub stand_counter: i32,
    pub tail_counter: i32,
    pub sprint_counter: i32,
    pub eat_anim: f32,
    pub eat_anim_o: f32,
    pub stand_anim: f32,
    pub stand_anim_o: f32,
    pub mouth_anim: f32,
    pub mouth_anim_o: f32,
    pub allow_stand_sliding: bool,
    pub gallop_counter: i32,
    pub chest: bool,
    pub inventory: Vec<Option<Stack>>,
    // Llama.
    pub strength: i32,
    pub did_spit: bool,
    pub caravan_head: Option<i32>,
    pub caravan_tail: Option<i32>,
    pub despawn_delay: i32,
    // Skeleton horse.
    pub trap: bool,
    pub trap_time: i32,
    // Camel.
    pub dash: bool,
    pub dash_cooldown: i32,
    pub last_pose_tick: i64,
    /// "Sitting" posture (dimensions): follows the network pose.
    pub camel_sitting_pose: bool,
    pub brain: Option<Box<camel::Brain>>,
    /// Age lock level (golden dandelion) read back from the NBT.
    pub age_locked: bool,
}

impl Mount {
    pub fn tamed(&self) -> bool {
        self.flags & FLAG_TAME != 0
    }

    pub fn flag(&self, f: i8) -> bool {
        self.flags & f != 0
    }

    pub fn set_flag(&mut self, f: i8, v: bool) {
        if v {
            self.flags |= f;
        } else {
            self.flags &= !f;
        }
    }
}

/// The camel is always "tamed" (overridden).
pub fn is_tamed(m: &Mob) -> bool {
    // F09: tamed nautiluses (common flag of tameable animals).
    is_camel(m.kind) || m.mount.tamed() || (super::sealife::is_nautilus(m.kind) && m.tame)
}

pub fn saddled(m: &Mob) -> bool {
    m.equipment[SADDLE].is_some()
}

/// Inventory columns: 5 donkey and mule, llama strength.
pub fn inventory_columns(m: &Mob) -> i32 {
    if !chested(m.kind) || !m.mount.chest {
        return 0;
    }
    if is_llama(m.kind) { m.mount.strength } else { 5 }
}

/// Container size: 3 × columns.
pub fn inventory_size(m: &Mob) -> usize {
    (inventory_columns(m) * 3) as usize
}

/// New container, items copied within the common limit.
pub fn create_inventory(m: &mut Mob) {
    let n = inventory_size(m);
    let mut inv = vec![None; n];
    for (i, s) in m.mount.inventory.iter().enumerate().take(n) {
        inv[i] = s.clone();
    }
    m.mount.inventory = inv;
}

/// Anger cap.
pub fn max_temper(k: Kind) -> i32 {
    if is_llama(k) { 30 } else { 100 }
}

pub fn modify_temper(m: &mut Mob, amount: i32) -> i32 {
    let t = (m.mount.temper + amount).clamp(0, max_temper(m.kind));
    m.mount.temper = t;
    t
}

/// Can rear: not llamas or camels.
pub fn can_rear(k: Kind) -> bool {
    !is_llama(k) && !is_camel(k)
}

/// Controlled by a mob: zombie horse and camel husk ridden
/// by a mob.
pub fn mob_controlled(m: &Mob) -> bool {
    matches!(m.kind, Kind::ZombieHorse | Kind::CamelHusk) && matches!(m.mount.passengers.first(), Some(Ent::Mob(_)))
}

/// Immobile: eating or rearing (llama: eating only), or dying.
pub fn immobile(m: &Mob) -> bool {
    if !handles(m.kind) {
        return false;
    }
    if is_llama(m.kind) {
        return m.mount.flag(FLAG_EATING);
    }
    m.mount.flag(FLAG_EATING) || m.mount.flag(FLAG_STANDING)
}

/// Does not grow from food (false).
pub fn never_ages_up(k: Kind) -> bool {
    matches!(k, Kind::SkeletonHorse | Kind::ZombieHorse)
}

pub fn can_age_up(m: &Mob) -> bool {
    m.is_baby() && !m.mount.age_locked && !never_ages_up(m.kind)
}

/// Age advanced, clamped to 0. `forced` (food given to a baby): the advance is accumulated and
/// becomes the age (delay before being able to breed) when passing to adulthood, and the
/// growth particles start.
pub fn age_up(m: &mut Mob, secs: i32, forced: bool) {
    let old = m.age;
    m.age = (old + secs * 20).min(0);
    if forced {
        m.x.forced_age += m.age - old;
        if m.x.forced_timer == 0 {
            m.x.forced_timer = 40;
        }
    }
    if m.age == 0 {
        m.age = m.x.forced_age;
    }
}

/// Seconds gained by feeding a baby at `ticks` from adult age (integer division, then float
/// product).
pub fn feeding_secs(ticks: i32) -> i32 {
    ((ticks / 20) as f32 * 0.1f32) as i32
}

/// Constructor: own attributes, container, camel (pose), llama (navigation 40).
pub fn on_new(m: &mut Mob) {
    let a = &mut m.attrs;
    a.safe_fall.base = 6.0;
    a.fall_mult.base = 0.5;
    a.jump.base = match m.kind {
        Kind::Donkey | Kind::Mule | Kind::Llama | Kind::TraderLlama => 0.5,
        Kind::Camel | Kind::CamelHusk => f64::from(0.42f32),
        _ => 0.7,
    };
    m.mount.despawn_delay = 47999;
    if is_llama(m.kind) {
        m.nav.required_length = 40.0;
    }
    if is_camel(m.kind) {
        camel::init_brain(m);
    }
    create_inventory(m);
}

// ---------- Metadata ----------

pub fn define_data(m: &mut Mob) {
    m.define(16, Meta::Bool(false));
    m.define(17, Meta::Bool(false));
    m.define(18, Meta::Byte(0));
    match m.kind {
        Kind::Horse => m.define(19, Meta::Int(0)),
        Kind::Donkey | Kind::Mule => m.define(19, Meta::Bool(false)),
        Kind::Llama | Kind::TraderLlama => {
            m.define(19, Meta::Bool(false));
            m.define(20, Meta::Int(0));
            m.define(21, Meta::Int(0));
        }
        Kind::Camel | Kind::CamelHusk => {
            m.define(19, Meta::Bool(false));
            m.define(20, Meta::Long(0));
        }
        _ => {}
    }
}

pub fn sync_data(m: &mut Mob) {
    if !m.dead && is_camel(m.kind) {
        m.set(6, Meta::Pose(if m.mount.camel_sitting_pose { 10 } else { 0 }));
    }
    m.set(16, Meta::Bool(m.age < 0));
    m.set(17, Meta::Bool(m.mount.age_locked));
    m.set(18, Meta::Byte(m.mount.flags));
    match m.kind {
        Kind::Horse => m.set(19, Meta::Int(m.variant)),
        Kind::Donkey | Kind::Mule => m.set(19, Meta::Bool(m.mount.chest)),
        Kind::Llama | Kind::TraderLlama => {
            m.set(19, Meta::Bool(m.mount.chest));
            m.set(20, Meta::Int(m.mount.strength));
            m.set(21, Meta::Int(m.variant));
        }
        Kind::Camel | Kind::CamelHusk => {
            m.set(19, Meta::Bool(m.mount.dash));
            m.set(20, Meta::Long(m.mount.last_pose_tick));
        }
        _ => {}
    }
}

// ---------- NBT ----------

pub fn load(m: &mut Mob, e: &Nbt) {
    let flag = |k: &str| int(e, k).is_some_and(|v| v != 0);
    // Llama: strength is read before the rest (container size).
    if is_llama(m.kind) {
        m.mount.strength = (int(e, "Strength").unwrap_or(0) as i32).clamp(1, 5);
    }
    m.mount.set_flag(FLAG_EATING, flag("EatingHaystack"));
    m.mount.set_flag(FLAG_BRED, flag("Bred"));
    m.mount.temper = int(e, "Temper").unwrap_or(0) as i32;
    m.mount.set_flag(FLAG_TAME, flag("Tame"));
    m.mount.owner = nbtio::uuid(e.get("Owner"));
    m.mount.age_locked = flag("AgeLocked");
    // Saved jump strength (drawn at spawn).
    if let Some(Nbt::List(list)) = e.get("attributes") {
        for a in list {
            if a.get("id").and_then(Nbt::as_str) == Some("minecraft:jump_strength")
                && let Some(b) = a.get("base").and_then(super::mob::f64_of)
            {
                m.attrs.jump.base = b;
            }
        }
    }
    match m.kind {
        Kind::Horse => m.variant = int(e, "Variant").unwrap_or(0) as i32,
        Kind::Llama | Kind::TraderLlama => {
            m.variant = (int(e, "Variant").unwrap_or(0) as i32).clamp(0, 3);
            if m.kind == Kind::TraderLlama {
                m.mount.despawn_delay = int(e, "DespawnDelay").map_or(47999, |v| v as i32);
            }
        }
        Kind::SkeletonHorse => {
            m.mount.trap = flag("SkeletonTrap");
            m.mount.trap_time = int(e, "SkeletonTrapTime").unwrap_or(0) as i32;
        }
        Kind::Camel | Kind::CamelHusk => {
            let t = int(e, "LastPoseTick").unwrap_or(0);
            m.mount.camel_sitting_pose = t < 0;
            m.mount.last_pose_tick = t;
        }
        _ => {}
    }
    if chested(m.kind) {
        m.mount.chest = flag("ChestedHorse");
        create_inventory(m);
        if m.mount.chest {
            for it in nbtio::list(e.get("Items")) {
                let slot = int(it, "Slot").unwrap_or(-1) as i32;
                if slot >= 0 && (slot as usize) < m.mount.inventory.len() {
                    m.mount.inventory[slot as usize] = Stack::from_nbt(it);
                }
            }
        }
    }
    // Saddle and armor placed by a player: certain to drop (drop chances read back).
    for (slot, name) in [(BODY, "body"), (SADDLE, "saddle")] {
        let chance = e.get("drop_chances").and_then(|d| d.get(name)).and_then(super::mob::f64_of);
        if let Some(c) = chance {
            m.drop_chance[slot] = c as f32;
        }
    }
}

pub fn save(m: &Mob, e: &mut Nbt) {
    let set = |e: &mut Nbt, k: &str, v: Nbt| nbtio::set(e, k, v);
    set(e, "EatingHaystack", Nbt::Byte(i8::from(m.mount.flag(FLAG_EATING))));
    set(e, "Bred", Nbt::Byte(i8::from(m.mount.flag(FLAG_BRED))));
    set(e, "Temper", Nbt::Int(m.mount.temper));
    set(e, "Tame", Nbt::Byte(i8::from(m.mount.tamed())));
    match m.mount.owner {
        Some(o) => set(e, "Owner", nbtio::uuid_nbt(o)),
        None => nbtio::remove(e, "Owner"),
    }
    if m.mount.age_locked {
        set(e, "AgeLocked", Nbt::Byte(1));
    }
    // Jump strength added to the attributes registered by the engine.
    if let Some(Nbt::List(list)) = e.get("attributes").cloned() {
        let mut list: Vec<Nbt> =
            list.into_iter().filter(|a| a.get("id").and_then(Nbt::as_str) != Some("minecraft:jump_strength")).collect();
        list.push(Nbt::Compound(vec![
            ("id".into(), Nbt::String("minecraft:jump_strength".into())),
            ("base".into(), Nbt::Double(m.attrs.jump.base)),
        ]));
        set(e, "attributes", Nbt::List(list));
    }
    match m.kind {
        Kind::Horse => set(e, "Variant", Nbt::Int(m.variant)),
        Kind::Llama | Kind::TraderLlama => {
            set(e, "Variant", Nbt::Int(m.variant));
            set(e, "Strength", Nbt::Int(m.mount.strength));
            if m.kind == Kind::TraderLlama {
                set(e, "DespawnDelay", Nbt::Int(m.mount.despawn_delay));
            }
        }
        Kind::SkeletonHorse => {
            set(e, "SkeletonTrap", Nbt::Byte(i8::from(m.mount.trap)));
            set(e, "SkeletonTrapTime", Nbt::Int(m.mount.trap_time));
        }
        Kind::Camel | Kind::CamelHusk => set(e, "LastPoseTick", Nbt::Long(m.mount.last_pose_tick)),
        _ => {}
    }
    if chested(m.kind) {
        set(e, "ChestedHorse", Nbt::Byte(i8::from(m.mount.chest)));
        if m.mount.chest {
            let items: Vec<Nbt> = m
                .mount
                .inventory
                .iter()
                .enumerate()
                .filter_map(|(i, s)| Some(s.as_ref()?.to_nbt(Some(i as i8))))
                .collect();
            set(e, "Items", Nbt::List(items));
        } else {
            nbtio::remove(e, "Items");
        }
    }
    let mut chances: Vec<(String, Nbt)> = Vec::new();
    for (slot, name) in [(BODY, "body"), (SADDLE, "saddle")] {
        if m.drop_chance[slot] > 1.0 {
            chances.push((name.into(), Nbt::Float(2.0)));
        }
    }
    if !chances.is_empty() {
        set(e, "drop_chances", Nbt::Compound(chances));
    }
}

// ---------- Sounds ----------

fn baby_horse(m: &Mob) -> bool {
    m.kind == Kind::Horse && m.is_baby()
}

/// Ambient sound.
pub fn ambient_sound(m: &Mob) -> Option<String> {
    Some(
        match m.kind {
            Kind::Horse if m.is_baby() => "minecraft:entity.baby_horse.ambient",
            Kind::SkeletonHorse if m.eyes_in_water => "minecraft:entity.skeleton_horse.ambient_water",
            k => info(k).ambient?,
        }
        .into(),
    )
}

/// Own hurt / death (foal).
pub fn hurt_sound(m: &Mob, death: bool) -> Option<&'static str> {
    if !handles(m.kind) {
        return None;
    }
    Some(match (baby_horse(m), death) {
        (true, true) => "minecraft:entity.baby_horse.death",
        (true, false) => "minecraft:entity.baby_horse.hurt",
        (false, true) => info(m.kind).death,
        (false, false) => info(m.kind).hurt,
    })
}

/// Eating sound; `None`: the skeleton horse has none.
pub fn eating_sound(m: &Mob) -> Option<&'static str> {
    Some(match m.kind {
        Kind::Horse if m.is_baby() => "minecraft:entity.baby_horse.eat",
        Kind::Horse => "minecraft:entity.horse.eat",
        Kind::Donkey => "minecraft:entity.donkey.eat",
        Kind::Mule => "minecraft:entity.mule.eat",
        Kind::Llama | Kind::TraderLlama => "minecraft:entity.llama.eat",
        Kind::ZombieHorse => "minecraft:entity.zombie_horse.eat",
        Kind::Camel => "minecraft:entity.camel.eat",
        Kind::CamelHusk => "minecraft:entity.camel_husk.eat",
        _ => return None,
    })
}

/// Anger sound.
pub fn angry_sound(m: &Mob) -> Option<&'static str> {
    Some(match m.kind {
        Kind::Horse if m.is_baby() => "minecraft:entity.baby_horse.angry",
        Kind::Horse => "minecraft:entity.horse.angry",
        Kind::Donkey => "minecraft:entity.donkey.angry",
        Kind::Mule => "minecraft:entity.mule.angry",
        Kind::Llama | Kind::TraderLlama => "minecraft:entity.llama.angry",
        Kind::ZombieHorse => "minecraft:entity.zombie_horse.angry",
        _ => return None,
    })
}

/// Sound category: all these animals are "neutral", even the zombie horse.
pub fn sound_source(m: &Mob) -> Option<i32> {
    handles(m.kind).then_some(6)
}

impl Server {
    /// Sound played by the entity (given volume and pitch).
    pub(crate) fn mount_sound(&mut self, m: &Mob, name: &str, volume: f32, pitch: f32) {
        self.mob_sound(m, name, volume, pitch);
    }

    /// `eating()`: open mouth, eating sound (entity random for the pitch).
    pub(crate) fn mount_eating(&mut self, m: &mut Mob) {
        m.mount.mouth_counter = 1;
        m.mount.set_flag(FLAG_MOUTH, true);
        if let Some(s) = eating_sound(m) {
            let p = 1.0 + (m.rng.next_float() - m.rng.next_float()) * 0.2;
            self.mount_sound(m, s, 1.0, p);
        }
    }

    /// Rears and lets out its anger cry.
    pub(crate) fn mount_make_mad(&mut self, m: &mut Mob) {
        if !m.mount.flag(FLAG_STANDING) {
            stand_if_possible(m);
            if let Some(s) = angry_sound(m) {
                self.make_sound(m, s);
            }
        }
    }

    /// Step noise; true if it replaces the engine's.
    pub(crate) fn mount_step_sound(&mut self, m: &mut Mob, on: [i32; 3]) -> bool {
        if !handles(m.kind) {
            return false;
        }
        let state = self.st(on);
        let (mut vol, mut pitch, mut step, liquid) = self.sound_type_of(state);
        let above = [on[0], on[1] + 1, on[2]];
        match m.kind {
            Kind::Llama | Kind::TraderLlama => {
                self.mount_sound(m, "minecraft:entity.llama.step", 0.15, 1.0);
                return true;
            }
            Kind::Camel | Kind::CamelHusk => {
                let sand = self.btag_id(on, crate::tag_bits::TagId::CamelSandStepSoundBlocks);
                let (s, v) = match (m.kind == Kind::Camel, sand) {
                    (true, true) => ("minecraft:entity.camel.step_sand", 1.0),
                    (true, false) => ("minecraft:entity.camel.step", 1.0),
                    (false, true) => ("minecraft:entity.camel_husk.step_sand", 0.4),
                    (false, false) => ("minecraft:entity.camel_husk.step", 0.4),
                };
                self.mount_sound(m, s, v, 1.0);
                return true;
            }
            _ => {}
        }
        if liquid {
            return true;
        }
        if self.bname(above) == "minecraft:snow" {
            (vol, pitch, step) = {
                let (v, p, s, _) = self.sound_type_of(self.st(above));
                (v, p, s)
            };
        }
        let vehicle = !m.mount.passengers.is_empty();
        let can_gallop = !chested(m.kind);
        let wood = matches!(
            step.as_str(),
            "minecraft:block.wood.step"
                | "minecraft:block.nether_wood.step"
                | "minecraft:block.stem.step"
                | "minecraft:block.cherry_wood.step"
                | "minecraft:block.bamboo_wood.step"
        );
        if vehicle && can_gallop {
            m.mount.gallop_counter += 1;
            if m.mount.gallop_counter > 5 && m.mount.gallop_counter % 3 == 0 {
                self.mount_sound(m, "minecraft:entity.horse.gallop", vol * 0.15, pitch);
                if m.kind == Kind::Horse && m.rng.next_int_bound(10) == 0 {
                    let s = if m.is_baby() {
                        "minecraft:entity.baby_horse.breathe"
                    } else {
                        "minecraft:entity.horse.breathe"
                    };
                    self.mount_sound(m, s, vol * 0.6, pitch);
                }
            } else if m.mount.gallop_counter <= 5 {
                self.mount_sound(m, "minecraft:entity.horse.step_wood", vol * 0.15, pitch);
            }
        } else if wood {
            self.mount_sound(m, "minecraft:entity.horse.step_wood", vol * 0.15, pitch);
        } else {
            let s = if m.is_baby() { "minecraft:entity.baby_horse.step" } else { "minecraft:entity.horse.step" };
            self.mount_sound(m, s, vol * 0.15, pitch);
        }
        true
    }

    /// Sound set of a state: volume, pitch, step sound, liquid.
    fn sound_type_of(&self, state: u32) -> (f32, f32, String, bool) {
        let liquid = self.data.states.get(state as usize).is_some_and(|i| i.fluid != 0);
        let t = self
            .data
            .states
            .get(state as usize)
            .and_then(|i| self.data.sound_types.get(usize::from(i.sound)))
            .cloned()
            .unwrap_or_default();
        (t.volume, t.pitch, t.step, liquid)
    }

    /// Equine fall; true if handled.
    pub(crate) fn mount_fall(&mut self, m: &mut Mob, dist: f64, mult: f64) -> bool {
        if !handles(m.kind) {
            return false;
        }
        let llama = is_llama(m.kind);
        if !llama && dist > 1.0 {
            let s = if m.is_baby() { "minecraft:entity.baby_horse.land" } else { "minecraft:entity.horse.land" };
            self.mount_sound(m, s, 0.4, 1.0);
        }
        let safe = m.attrs.safe_fall.value();
        let dmg = ((dist + 1.0e-6 - safe) * mult * m.attrs.fall_mult.value()).floor() as i32;
        if dmg <= 0 {
            return true;
        }
        if !llama || dist >= 6.0 {
            self.mob_hurt(m, "minecraft:fall", dmg as f32, None, None);
            self.propagate_fall(m, dist, mult);
        }
        self.mob_block_fall_sound(m);
        true
    }

    /// Fall passed on to the passengers.
    fn propagate_fall(&mut self, m: &Mob, dist: f64, mult: f64) {
        self.propagate_fall_to(&m.mount.passengers.clone(), dist, mult, "minecraft:fall");
    }

    /// Fall of `dist` (factor `mult`, damage type `ty`) taken by each passenger.
    pub(crate) fn propagate_fall_to(&mut self, passengers: &[Ent], dist: f64, mult: f64, ty: &str) {
        for &p in passengers {
            if p == Ent::Player {
                // The mounted player takes the same fall (unless they can fly: creative, spectator).
                if !self.abilities().may_fly {
                    let dmg = ((dist + 1.0e-6 - self.safe_fall_distance()) * mult).floor();
                    if dmg > 0.0 {
                        let sound = if dmg > 4.0 {
                            "minecraft:entity.player.big_fall"
                        } else {
                            "minecraft:entity.player.small_fall"
                        };
                        self.play_sound(sound, 7, self.pos, 1.0, 1.0);
                        self.hurt(ty, dmg as f32);
                    }
                }
                continue;
            }
            if let Ent::Mob(id) = p
                && let Some(i) = self.mob_index(id)
                && let Some(mut o) = self.mobs.list[i].take()
            {
                let safe = o.attrs.safe_fall.value();
                let dmg = ((dist + 1.0e-6 - safe) * mult * o.attrs.fall_mult.value()).floor() as i32;
                if dmg > 0 && !o.kind.info().fall_immune {
                    self.mob_hurt(&mut o, ty, dmg as f32, None, None);
                }
                self.mobs.list[i] = Some(o);
            }
        }
    }
}

// ---------- Ticks ----------

/// Rears for 20 ticks (if the species can).
pub fn stand_if_possible(m: &mut Mob) {
    if can_rear(m.kind) {
        m.mount.set_flag(FLAG_EATING, false);
        m.mount.set_flag(FLAG_STANDING, true);
        m.mount.stand_counter = 20;
    }
}

pub fn clear_standing(m: &mut Mob) {
    m.mount.set_flag(FLAG_STANDING, false);
    m.mount.stand_counter = 0;
}

/// End of `tick` (after the common tick): counters and animations (server included).
pub fn post_tick(s: &mut Server, m: &mut Mob) {
    if !handles(m.kind) {
        return;
    }
    let st = &mut m.mount;
    if st.mouth_counter > 0 {
        st.mouth_counter += 1;
        if st.mouth_counter > 30 {
            st.mouth_counter = 0;
            st.set_flag(FLAG_MOUTH, false);
        }
    }
    if st.stand_counter > 0 {
        st.stand_counter -= 1;
        if st.stand_counter <= 0 {
            st.set_flag(FLAG_STANDING, false);
            st.stand_counter = 0;
        }
    }
    if st.tail_counter > 0 {
        st.tail_counter += 1;
        if st.tail_counter > 8 {
            st.tail_counter = 0;
        }
    }
    if st.sprint_counter > 0 {
        st.sprint_counter += 1;
        if st.sprint_counter > 300 {
            st.sprint_counter = 0;
        }
    }
    st.eat_anim_o = st.eat_anim;
    if st.flag(FLAG_EATING) {
        st.eat_anim += (1.0 - st.eat_anim) * 0.4 + 0.05;
        if st.eat_anim > 1.0 {
            st.eat_anim = 1.0;
        }
    } else {
        st.eat_anim += (0.0 - st.eat_anim) * 0.4 - 0.05;
        if st.eat_anim < 0.0 {
            st.eat_anim = 0.0;
        }
    }
    st.stand_anim_o = st.stand_anim;
    if st.flag(FLAG_STANDING) {
        st.eat_anim = 0.0;
        st.eat_anim_o = st.eat_anim;
        st.stand_anim += (1.0 - st.stand_anim) * 0.4 + 0.05;
        if st.stand_anim > 1.0 {
            st.stand_anim = 1.0;
        }
    } else {
        st.allow_stand_sliding = false;
        st.stand_anim += (0.8 * st.stand_anim * st.stand_anim * st.stand_anim - st.stand_anim) * 0.6 - 0.05;
        if st.stand_anim < 0.0 {
            st.stand_anim = 0.0;
        }
    }
    st.mouth_anim_o = st.mouth_anim;
    if st.flag(FLAG_MOUTH) {
        st.mouth_anim += (1.0 - st.mouth_anim) * 0.7 + 0.05;
        if st.mouth_anim > 1.0 {
            st.mouth_anim = 1.0;
        }
    } else {
        st.mouth_anim += (0.0 - st.mouth_anim) * 0.7 - 0.05;
        if st.mouth_anim < 0.0 {
            st.mouth_anim = 0.0;
        }
    }
    if is_camel(m.kind) {
        camel::post_tick(s, m);
    }
}

/// Start of the AI step: the tail swishes 1 time in 200.
pub fn pre_ai_step(m: &mut Mob) {
    if handles(m.kind) && m.rng.next_int_bound(200) == 0 {
        m.mount.tail_counter = 1;
    }
}

/// End of the AI step: slow healing, grazing, foal following its mother; trader llama,
/// skeleton horse trap.
pub fn post_ai_step(s: &mut Server, m: &mut Mob) {
    if !handles(m.kind) {
        return;
    }
    if m.alive() {
        if m.rng.next_int_bound(900) == 0 && m.death_time == 0 {
            m.health = (m.health + 1.0).min(m.max_health());
        }
        if !is_llama(m.kind) {
            let below = [m.block_pos()[0], m.block_pos()[1] - 1, m.block_pos()[2]];
            if !m.mount.flag(FLAG_EATING)
                && m.mount.passengers.is_empty()
                && m.rng.next_int_bound(300) == 0
                && s.bname(below) == "minecraft:grass_block"
            {
                m.mount.set_flag(FLAG_EATING, true);
            }
            if m.mount.flag(FLAG_EATING) {
                m.mount.eating_counter += 1;
                if m.mount.eating_counter > 50 {
                    m.mount.eating_counter = 0;
                    m.mount.set_flag(FLAG_EATING, false);
                }
            }
        }
        follow_mommy(s, m);
    }
    match m.kind {
        Kind::TraderLlama => llama::maybe_despawn(s, m),
        Kind::SkeletonHorse if !m.persistent && m.mount.trap => {
            let t = m.mount.trap_time;
            m.mount.trap_time += 1;
            if t >= 18000 {
                m.removed = true;
            }
        }
        _ => {}
    }
}

/// Only a "bred" foal (flag never set by the game) follows.
fn follow_mommy(s: &mut Server, m: &mut Mob) {
    let bred_ok = m.mount.flag(FLAG_BRED) && m.is_baby() && !m.mount.flag(FLAG_EATING);
    let llama_ok = !is_llama(m.kind) || m.mount.caravan_head.is_none();
    if !bred_ok || !llama_ok {
        return;
    }
    let bb = m.bbox();
    let area = rc_physics::Aabb { min: bb.min.map(|v| v - 16.0), max: bb.max.map(|v| v + 16.0) };
    let mut best: Option<(f64, [f64; 3])> = None;
    for o in s.mobs.list.iter().flatten() {
        if o.id == m.id || !handles(o.kind) || !o.mount.flag(FLAG_BRED) || !o.alive() {
            continue;
        }
        if !o.touches(&area) || super::mth::dist2(m.pos, o.pos) > 256.0 {
            continue;
        }
        let d = super::mth::dist2(m.pos, o.pos);
        if best.is_none_or(|b| d < b.0) {
            best = Some((d, o.pos));
        }
    }
    // Path computed then thrown away (the game does not follow it): only the navigation state changes.
    if let Some((d, p)) = best
        && d > 4.0
    {
        let target = super::mth::block_of(p);
        let _ = s.create_path(m, target, 0);
    }
}

// ---------- Combat ----------

/// After damage dealt: an equine rears 1 time in 3; love drops.
pub fn on_hurt(m: &mut Mob, hurt: bool) {
    if !handles(m.kind) {
        return;
    }
    m.reset_love();
    if hurt && m.rng.next_int_bound(3) == 0 {
        stand_if_possible(m);
    }
}

/// Before damage (of the camel): gets up at once.
pub fn before_damage(s: &Server, m: &mut Mob) {
    if is_camel(m.kind) {
        camel::stand_up_instantly(s, m);
    }
}

impl Server {
    /// Death: chest and inventory dropped (of the equine), passengers dismounted.
    pub(crate) fn mount_drop_inventory(&mut self, m: &mut Mob) {
        if !handles(m.kind) {
            return;
        }
        for s in std::mem::take(&mut m.mount.inventory).into_iter().flatten() {
            self.mount_spawn_at(m, s);
        }
        if chested(m.kind) && m.mount.chest {
            self.mount_spawn_at(m, Stack { id: "minecraft:chest".into(), count: 1, components: None });
            m.mount.chest = false;
        }
        create_inventory(m);
    }

    /// Item dropped at the position, random velocity (world random).
    pub(crate) fn mount_spawn_at(&mut self, m: &Mob, s: Stack) {
        let r = &mut self.mobs.rng;
        let v = [r.next_double() * 0.2 - 0.1, 0.2, r.next_double() * 0.2 - 0.1];
        self.spawn_item(m.pos, v, s, 10);
    }
}

// ---------- Spawning ----------

/// 15 + U[0,8[ + U[0,9[.
pub fn gen_health(r: &mut super::jrand::JRand) -> f64 {
    f64::from(15 + r.next_int_bound(8) + r.next_int_bound(9))
}

/// 0.4 + 3 × U × 0.2 (first term in `f32`).
pub fn gen_jump(r: &mut super::jrand::JRand) -> f64 {
    f64::from(0.4f32) + r.next_double() * 0.2 + r.next_double() * 0.2 + r.next_double() * 0.2
}

/// (0.45 + 3 × U × 0.3) × 0.25.
pub fn gen_speed(r: &mut super::jrand::JRand) -> f64 {
    (f64::from(0.45f32) + r.next_double() * 0.3 + r.next_double() * 0.3 + r.next_double() * 0.3) * 0.25
}

/// Bounds of the draws (`MIN_*`/`MAX_*`, `float` values).
pub fn health_range() -> (f64, f64) {
    (15.0, f64::from(15 + 7 + 8))
}

pub fn jump_range() -> (f64, f64) {
    let lo = f64::from(0.4f32) as f32;
    let hi = (f64::from(0.4f32) + 0.2 + 0.2 + 0.2) as f32;
    (f64::from(lo), f64::from(hi))
}

pub fn speed_range() -> (f64, f64) {
    let lo = (f64::from(0.45f32) * 0.25) as f32;
    let hi = ((f64::from(0.45f32) + 0.3 + 0.3 + 0.3) * 0.25) as f32;
    (f64::from(lo), f64::from(hi))
}

/// Max HP set: the current health is brought under the maximum.
pub fn set_max_health(m: &mut Mob, v: f64) {
    m.attrs.max_health.base = v;
    if m.health > m.max_health() {
        m.health = m.max_health();
    }
}

/// The attribute draw (world random).
pub fn randomize_attributes(m: &mut Mob, r: &mut super::jrand::JRand) {
    match m.kind {
        Kind::Horse => {
            let h = gen_health(r);
            set_max_health(m, h);
            m.attrs.speed.base = gen_speed(r);
            m.attrs.jump.base = gen_jump(r);
        }
        Kind::Donkey | Kind::Mule | Kind::Llama | Kind::TraderLlama => {
            let h = gen_health(r);
            set_max_health(m, h);
        }
        Kind::SkeletonHorse => m.attrs.jump.base = gen_jump(r),
        Kind::ZombieHorse => {
            let k = 0.066_666_666_666_666_67;
            m.attrs.jump.base = 0.5 + r.next_double() * k + r.next_double() * k + r.next_double() * k;
            m.attrs.speed.base = (9.0 + r.next_double() + r.next_double() + r.next_double()) / f64::from(42.16f32);
        }
        _ => {}
    }
}

/// Baby attribute: mean, spread, quality, reflection.
pub fn offspring_attribute(a: f64, b: f64, lo: f64, hi: f64, r: &mut super::jrand::JRand) -> f64 {
    let a = a.clamp(lo, hi);
    let b = b.clamp(lo, hi);
    let margin = 0.15 * (hi - lo);
    let range = (a - b).abs() + margin * 2.0;
    let avg = (a + b) / 2.0;
    let q = (r.next_double() + r.next_double() + r.next_double()) / 3.0 - 0.5;
    let v = avg + range * q;
    if v > hi {
        hi - (v - hi)
    } else if v < lo {
        lo + (lo - v)
    } else {
        v
    }
}

/// Own spawn rule; `None`: common rule (animal, monster).
pub fn spawn_rules(s: &Server, k: Kind, p: [i32; 3]) -> Option<bool> {
    let below = [p[0], p[1] - 1, p[2]];
    match k {
        Kind::Camel => {
            let (a, b) = s.light(p);
            Some(s.btag_id(below, crate::tag_bits::TagId::CamelsSpawnableOn) && a.max(b) > 8)
        }
        _ => None,
    }
}

/// Despawning far from players.
pub fn remove_when_far_away(m: &Mob) -> Option<bool> {
    Some(match m.kind {
        Kind::ZombieHorse | Kind::CamelHusk => true,
        k if handles(k) => false,
        _ => return None,
    })
}

/// Maximum size of a group.
pub fn max_cluster(k: Kind) -> Option<i32> {
    handles(k).then_some(6)
}

/// Own navigation (construction): the camel floats and crosses fences.
pub fn init_nav(m: &mut Mob) {
    if is_camel(m.kind) {
        m.nav.can_float = true;
    }
    if is_llama(m.kind) {
        m.nav.required_length = 40.0;
    }
}

/// Food block: item tag of the species.
pub fn food_tag(k: Kind) -> &'static str {
    match k {
        Kind::Llama | Kind::TraderLlama => "minecraft:llama_food",
        Kind::ZombieHorse => "minecraft:zombie_horse_food",
        Kind::Camel => "minecraft:camel_food",
        Kind::CamelHusk => "minecraft:camel_husk_food",
        _ => "minecraft:horse_food",
    }
}

/// Walk value: all are animals (grass 10, otherwise light − 0.5).
pub fn walk_target_value(s: &Server, m: &Mob, p: [i32; 3]) -> Option<f32> {
    if !handles(m.kind) {
        return None;
    }
    Some(if s.bname([p[0], p[1] - 1, p[2]]) == "minecraft:grass_block" { 10.0 } else { s.magic_light(p) - 0.5 })
}

/// Slowdown in water: skeleton horse 0.96.
pub fn water_slow_down(m: &Mob) -> Option<f32> {
    (m.kind == Kind::SkeletonHorse).then_some(0.96)
}

/// Pushable: not when carrying someone.
pub fn pushable(m: &Mob) -> bool {
    !handles(m.kind) || m.mount.passengers.is_empty()
}
