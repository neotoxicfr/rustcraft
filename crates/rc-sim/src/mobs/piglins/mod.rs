//! Piglins and hoglins (family F13, `server-mobs.md` § 4.F13): piglin, brute, zombified
//! piglin, hoglin, zoglin. Info sheets, own state, metadata, NBT and hooks called by the
//! common engine (`mobs/`). The brains (piglin, brute, hoglin, zoglin) go through F10's brain
//! engine (`brain.rs` here for the types, `behave.rs` for the behaviors); gold, bartering and
//! inventory are in `items.rs`; the zombified piglin keeps the zombie's goals
//! (`zombified.rs`).

pub(crate) mod behave;
pub(crate) mod brain;
pub(crate) mod items;
pub(crate) mod portal;
pub(crate) mod spear;
pub(crate) mod zombified;

#[cfg(test)]
mod probe_tests;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod world_tests;

use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::brainy::brain::{Act, Brain};
use super::mob::{Ent, Mob};
use super::nav::PathType;
use super::species::{Category, Info, Kind, info as base_info};
use super::villager::brain::{DimKey, GPos};
use crate::nbtio::{self, int};
use crate::player::Stack;
use crate::server::Server;

// ---------- Info sheets ----------

/// The game's monster: 5 experience points, tracking 8 chunks.
const fn monster(name: &'static str, width: f32, height: f32, eye: f32) -> Info {
    let mut i = base_info(name, Category::Monster, width, height, eye);
    i.xp = 5;
    i.tracking = 8;
    i.not_in_peaceful = true;
    i.hurt = "minecraft:entity.hostile.hurt";
    i.death = "minecraft:entity.hostile.death";
    i
}

/// Piglin: 0.6 × 1.95 (eyes 1.79), HP 16, speed 0.35, damage 5; present in peaceful; ambient
/// sound by activity.
static PIGLIN: Info = {
    let mut i = monster("minecraft:piglin", 0.6, 1.95, 1.79);
    i.health = 16.0;
    i.speed = 0.35f32 as f64;
    i.attack = 5.0;
    i.not_in_peaceful = false;
    i.hurt = "minecraft:entity.piglin.hurt";
    i.death = "minecraft:entity.piglin.death";
    i.step_sound = Some("minecraft:entity.piglin.step");
    i
};

/// Brute: HP 50, speed 0.35, damage 7, tracking 12, 20 experience.
static PIGLIN_BRUTE: Info = {
    let mut i = monster("minecraft:piglin_brute", 0.6, 1.95, 1.79);
    i.health = 50.0;
    i.speed = 0.35f32 as f64;
    i.attack = 7.0;
    i.follow = 12.0;
    i.xp = 20;
    i.ambient = Some("minecraft:entity.piglin_brute.ambient");
    i.hurt = "minecraft:entity.piglin_brute.hurt";
    i.death = "minecraft:entity.piglin_brute.death";
    i.step_sound = Some("minecraft:entity.piglin_brute.step");
    i
};

/// Zombified piglin: zombie attributes (tracking 35, armor 2), speed 0.23, damage 5,
/// fire-immune (so never burned by the sun), the zombie's step.
static ZOMBIFIED_PIGLIN: Info = {
    let mut i = monster("minecraft:zombified_piglin", 0.6, 1.95, 1.79);
    i.speed = 0.23f32 as f64;
    i.attack = 5.0;
    i.follow = 35.0;
    i.armor = 2.0;
    i.ambient = Some("minecraft:entity.zombified_piglin.ambient");
    i.hurt = "minecraft:entity.zombified_piglin.hurt";
    i.death = "minecraft:entity.zombified_piglin.death";
    i.step_sound = Some("minecraft:entity.zombie.step");
    i.fire_immune = true;
    i
};

/// Hoglin: game animal ranked among monsters, 1.3964844 × 1.4, HP 40, speed 0.3, damage 6,
/// interval of animal ambient sounds (120).
static HOGLIN: Info = {
    let mut i = monster("minecraft:hoglin", 1.396_484_4, 1.4, 1.4 * 0.85);
    i.health = 40.0;
    i.speed = 0.3f32 as f64;
    i.attack = 6.0;
    i.ambient_interval = 120;
    i.hurt = "minecraft:entity.hoglin.hurt";
    i.death = "minecraft:entity.hoglin.death";
    i.step_sound = Some("minecraft:entity.hoglin.step");
    i
};

/// Zoglin: same dimensions and attributes as the hoglin, fire-immune.
static ZOGLIN: Info = {
    let mut i = monster("minecraft:zoglin", 1.396_484_4, 1.4, 1.4 * 0.85);
    i.health = 40.0;
    i.speed = 0.3f32 as f64;
    i.attack = 6.0;
    i.hurt = "minecraft:entity.zoglin.hurt";
    i.death = "minecraft:entity.zoglin.death";
    i.step_sound = Some("minecraft:entity.zoglin.step");
    i.fire_immune = true;
    i
};

pub fn info(k: Kind) -> &'static Info {
    match k {
        Kind::Piglin => &PIGLIN,
        Kind::PiglinBrute => &PIGLIN_BRUTE,
        Kind::ZombifiedPiglin => &ZOMBIFIED_PIGLIN,
        Kind::Hoglin => &HOGLIN,
        _ => &ZOGLIN,
    }
}

/// Species of the family.
pub fn handles(k: Kind) -> bool {
    matches!(k, Kind::Piglin | Kind::PiglinBrute | Kind::ZombifiedPiglin | Kind::Hoglin | Kind::Zoglin)
}

/// The "common piglin" layer (piglin, brute).
pub fn is_abstract_piglin(k: Kind) -> bool {
    matches!(k, Kind::Piglin | Kind::PiglinBrute)
}

/// Brain families (piglin, brute, hoglin, zoglin).
pub fn has_brain(k: Kind) -> bool {
    matches!(k, Kind::Piglin | Kind::PiglinBrute | Kind::Hoglin | Kind::Zoglin)
}

/// Baby dimensions: piglins 0.49 × 0.98 (eyes 0.78), hoglin and zoglin 0.75 × 0.85 (0.625).
pub fn baby_dims(k: Kind) -> (f32, f32, f32) {
    match k {
        Kind::Hoglin | Kind::Zoglin => (0.75, 0.85, 0.625),
        _ => (0.49, 0.98, 0.78),
    }
}

/// "Vehicle" attachment of the passenger (−0.7; babies: own attachment 0.1875).
pub fn vehicle_attach(r: &Mob) -> Option<f64> {
    matches!(r.kind, Kind::Piglin | Kind::PiglinBrute | Kind::ZombifiedPiglin)
        .then(|| if r.is_baby() { 0.1875 } else { 0.7 })
}

/// Declared "passenger" attachment point (hoglin, zoglin: 1.49375; babies 0.875; piglins).
pub fn passenger_point(v: &Mob) -> Option<[f64; 3]> {
    let baby = v.is_baby();
    Some(match v.kind {
        Kind::Hoglin | Kind::Zoglin => [0.0, if baby { 0.875 } else { 1.493_75 }, 0.0],
        Kind::Piglin | Kind::PiglinBrute if !baby => [0.0, 2.0125, 0.0],
        Kind::ZombifiedPiglin if !baby => [0.0, 2.0, 0.0],
        _ => return None,
    })
}

// ---------- Own state ----------

/// Piglin inventory.
pub const INVENTORY_SIZE: usize = 8;

/// State of the family (`pig` field of the mob).
#[derive(Debug, Clone, Default)]
pub struct Pig {
    /// Zombification immunity data, time spent in the normal world.
    pub immune: bool,
    pub time_in_overworld: i32,
    /// Piglin: "does not hunt" flag; hoglin: "cannot be hunted" flag.
    pub cannot_hunt: bool,
    /// Piglin: crossbow charging, dance (indices 18, 19).
    pub charging: bool,
    pub dancing: bool,
    /// Hoglin, zoglin: attack animation (event 4).
    pub attack_anim: i32,
    /// Hoglin: experience (5, baby 3).
    pub xp_reward: i32,
    /// Hoglin: "baby" state at the last age threshold applied (adult at construction).
    pub boundary_baby: bool,
    pub inventory: Vec<Stack>,
    /// Activity at the previous tick (sound on change).
    pub last_act: Option<Act>,
    /// Zombified piglin: deferred anger sound, next call to the others.
    pub first_anger_sound_in: i32,
    pub ticks_until_alert: i32,
    /// Zombified piglin: target at the previous tick, UUID of the anger target (`angry_at`).
    pub had_target: bool,
    pub anger_uuid: Option<u128>,
    /// Hits received from a mob that was acting (out of the list during its tick): reaction
    /// deferred to the start of the next tick.
    pub pending_hurt_by: Vec<Ent>,
    /// Spear: position at the start of the previous tick, known velocity, recent victims
    /// of the tip (entity, game tick).
    pub last_known: Option<[f64; 3]>,
    pub known_speed: [f64; 3],
    pub stabbed: Vec<(Ent, i64)>,
}

pub fn define_data(m: &mut Mob) {
    match m.kind {
        Kind::Piglin => {
            m.define(16, Meta::Bool(false));
            m.define(17, Meta::Bool(false));
            m.define(18, Meta::Bool(false));
            m.define(19, Meta::Bool(false));
        }
        Kind::PiglinBrute => m.define(16, Meta::Bool(false)),
        // `Zombie`: baby, special type, conversion to drowned.
        Kind::ZombifiedPiglin => {
            m.define(16, Meta::Bool(false));
            m.define(17, Meta::Int(0));
            m.define(18, Meta::Bool(false));
        }
        // The "growing mob" layer (baby, locked age) then immunity.
        Kind::Hoglin => {
            m.define(16, Meta::Bool(false));
            m.define(17, Meta::Bool(false));
            m.define(18, Meta::Bool(false));
        }
        _ => m.define(16, Meta::Bool(false)),
    }
}

pub fn sync_data(m: &mut Mob) {
    // Item in use (crossbow drawn, spear lowered): flag 1 of index 8, right hand.
    if matches!(m.kind, Kind::Piglin | Kind::ZombifiedPiglin) {
        m.set(8, Meta::Byte(i8::from(m.use_ticks >= 0)));
    }
    match m.kind {
        Kind::Piglin => {
            m.set(16, Meta::Bool(m.pig.immune));
            m.set(17, Meta::Bool(m.baby));
            m.set(18, Meta::Bool(m.pig.charging));
            m.set(19, Meta::Bool(m.pig.dancing));
        }
        Kind::PiglinBrute => m.set(16, Meta::Bool(m.pig.immune)),
        Kind::ZombifiedPiglin => {
            m.set(16, Meta::Bool(m.baby));
            m.set(18, Meta::Bool(m.x.conv_time >= 0));
        }
        Kind::Hoglin => {
            m.set(16, Meta::Bool(m.age < 0));
            m.set(17, Meta::Bool(m.mount.age_locked));
            m.set(18, Meta::Bool(m.pig.immune));
        }
        _ => m.set(16, Meta::Bool(m.baby)),
    }
}

fn read_gpos(v: &Nbt) -> Option<GPos> {
    let pos = match v.get("pos")? {
        Nbt::IntArray(a) if a.len() == 3 => [a[0], a[1], a[2]],
        _ => return None,
    };
    let Nbt::String(d) = v.get("dimension")? else { return None };
    let dim = rc_data::ids::with_full_id(d, DimKey::of);
    Some(GPos { dim, pos })
}

pub fn load(m: &mut Mob, e: &Nbt) {
    let flag = |k: &str| int(e, k).is_some_and(|v| v != 0);
    match m.kind {
        Kind::Piglin | Kind::PiglinBrute | Kind::Hoglin => {
            // `CanPickUpLoot` true by default (constructor), read back as is.
            if is_abstract_piglin(m.kind) {
                m.can_pickup = int(e, "CanPickUpLoot").is_none_or(|v| v != 0);
            }
            m.pig.immune = flag("IsImmuneToZombification");
            m.pig.time_in_overworld = int(e, "TimeInOverworld").unwrap_or(0) as i32;
        }
        _ => {}
    }
    match m.kind {
        Kind::Piglin => {
            m.baby = flag("IsBaby");
            set_piglin_baby(m, m.baby);
            m.pig.cannot_hunt = flag("CannotHunt");
            m.pig.inventory = match e.get("Inventory") {
                Some(Nbt::List(l)) => l.iter().filter_map(Stack::from_nbt).take(INVENTORY_SIZE).collect(),
                _ => Vec::new(),
            };
        }
        Kind::Hoglin => {
            m.pig.cannot_hunt = flag("CannotBeHunted");
            m.mount.age_locked = flag("AgeLocked");
            // Age threshold crossed at load only for a baby (age starts at 0).
            if m.age < 0 {
                age_boundary(m);
            }
        }
        Kind::Zoglin => {
            m.baby = flag("IsBaby");
            if m.baby {
                m.attrs.attack.base = 0.5;
            }
        }
        Kind::ZombifiedPiglin => zombified::load(m, e),
        _ => {}
    }
    // Brain: registered memories.
    if let Some(b) = m.bm.brain.as_mut()
        && let Some(Nbt::Compound(f)) = e.get("Brain").and_then(|b| b.get("memories"))
    {
        let p = &mut b.mem.pig;
        for (k, v) in f {
            let ttl = v.get("ttl").and_then(Nbt::as_i64).unwrap_or(super::brainy::brain::NEVER);
            let Some(val) = v.get("value") else { continue };
            let b = val.as_i64().map(|i| i != 0);
            match k.trim_start_matches("minecraft:") {
                "angry_at" => {
                    if let Some(u) = nbtio::uuid(Some(val)) {
                        p.angry_at.set_exp(u, ttl);
                    }
                }
                "universal_anger" => {
                    if let Some(b) = b {
                        p.universal_anger.set_exp(b, ttl);
                    }
                }
                "admiring_item" => {
                    if let Some(b) = b {
                        p.admiring_item.set_exp(b, ttl);
                    }
                }
                "admiring_disabled" => {
                    if let Some(b) = b {
                        p.admiring_disabled.set_exp(b, ttl);
                    }
                }
                "hunted_recently" => {
                    if let Some(b) = b
                        && m.kind != Kind::PiglinBrute
                    {
                        p.hunted_recently.set_exp(b, ttl);
                    }
                }
                "home" => {
                    if let Some(g) = read_gpos(val) {
                        p.home.set_exp(g, ttl);
                    }
                }
                _ => {}
            }
        }
    }
}

fn entry(v: Nbt, ttl: i64) -> Nbt {
    let mut f = vec![("value".to_owned(), v)];
    if ttl != super::brainy::brain::NEVER {
        f.push(("ttl".to_owned(), Nbt::Long(ttl)));
    }
    Nbt::Compound(f)
}

pub fn save(m: &Mob, e: &mut Nbt) {
    let set = |e: &mut Nbt, k: &str, v: Nbt| nbtio::set(e, k, v);
    if matches!(m.kind, Kind::Piglin | Kind::PiglinBrute | Kind::Hoglin) {
        set(e, "IsImmuneToZombification", Nbt::Byte(i8::from(m.pig.immune)));
        set(e, "TimeInOverworld", Nbt::Int(m.pig.time_in_overworld));
    }
    match m.kind {
        Kind::Piglin => {
            set(e, "IsBaby", Nbt::Byte(i8::from(m.baby)));
            set(e, "CannotHunt", Nbt::Byte(i8::from(m.pig.cannot_hunt)));
            let inv: Vec<Nbt> = m.pig.inventory.iter().map(|s| s.to_nbt(None)).collect();
            if inv.is_empty() {
                nbtio::remove(e, "Inventory");
            } else {
                set(e, "Inventory", Nbt::List(inv));
            }
        }
        Kind::Hoglin => {
            set(e, "CannotBeHunted", Nbt::Byte(i8::from(m.pig.cannot_hunt)));
            set(e, "AgeLocked", Nbt::Byte(i8::from(m.mount.age_locked)));
        }
        Kind::Zoglin => set(e, "IsBaby", Nbt::Byte(i8::from(m.baby))),
        Kind::ZombifiedPiglin => zombified::save(m, e),
        _ => {}
    }
    // Base damage (hoglin: 0.5 baby / 6; baby zoglin: 0.5) registered with the attributes.
    if matches!(m.kind, Kind::Hoglin | Kind::Zoglin)
        && let Some(Nbt::List(list)) = nbtio::get_mut(e, "attributes")
    {
        list.retain(|a| a.get("id").and_then(Nbt::as_str) != Some("minecraft:attack_damage"));
        list.push(Nbt::Compound(vec![
            ("id".into(), Nbt::String("minecraft:attack_damage".into())),
            ("base".into(), Nbt::Double(m.attrs.attack.base)),
        ]));
    }
    if m.kind != Kind::Hoglin {
        for k in ["Age", "ForcedAge", "InLove"] {
            nbtio::remove(e, k);
        }
    }
    if let Some(b) = m.bm.brain.as_ref() {
        let p = &b.mem.pig;
        let mut f: Vec<(String, Nbt)> = Vec::new();
        let flag = |f: &mut Vec<(String, Nbt)>, n: &str, s: &super::brainy::brain::Slot<bool>| {
            if let Some(v) = s.v {
                f.push((format!("minecraft:{n}"), entry(Nbt::Byte(i8::from(v)), s.ttl)));
            }
        };
        match m.kind {
            Kind::Piglin => {
                if let Some(u) = p.angry_at.v {
                    f.push(("minecraft:angry_at".into(), entry(nbtio::uuid_nbt(u), p.angry_at.ttl)));
                }
                flag(&mut f, "universal_anger", &p.universal_anger);
                flag(&mut f, "admiring_item", &p.admiring_item);
                flag(&mut f, "admiring_disabled", &p.admiring_disabled);
                flag(&mut f, "hunted_recently", &p.hunted_recently);
            }
            Kind::PiglinBrute => {
                if let Some(u) = p.angry_at.v {
                    f.push(("minecraft:angry_at".into(), entry(nbtio::uuid_nbt(u), p.angry_at.ttl)));
                }
                if let Some(g) = p.home.v {
                    let gp = Nbt::Compound(vec![
                        ("pos".into(), Nbt::IntArray(g.pos.to_vec())),
                        ("dimension".into(), Nbt::String(g.dim.name())),
                    ]);
                    f.push(("minecraft:home".into(), entry(gp, p.home.ttl)));
                }
            }
            _ => {}
        }
        set(e, "Brain", Nbt::Compound(vec![("memories".into(), Nbt::Compound(f))]));
    }
}

// ---------- Construction ----------

/// Construction: own attributes, brain (sensor delays drawn
/// from the entity random), piglin pickup.
pub fn on_new(m: &mut Mob) {
    match m.kind {
        Kind::Hoglin | Kind::Zoglin => {
            m.attrs.knockback_resistance.base = f64::from(0.6f32);
            m.attrs.attack_knockback.base = 1.0;
        }
        Kind::Piglin | Kind::PiglinBrute => m.can_pickup = true,
        _ => {}
    }
    m.pig.xp_reward = 5;
    let b: Option<Brain> = match m.kind {
        Kind::Piglin => Some(brain::piglin(&mut m.rng)),
        Kind::PiglinBrute => Some(brain::brute(&mut m.rng)),
        Kind::Hoglin => Some(brain::hoglin(&mut m.rng)),
        Kind::Zoglin => Some(brain::zoglin(&mut m.rng)),
        _ => None,
    };
    m.bm.brain = b.map(Box::new);
}

/// Navigation (constructor of the "common piglin" layer): doors open, fire avoided.
pub fn init_nav(m: &mut Mob) {
    if is_abstract_piglin(m.kind) {
        m.nav.can_open_doors = true;
    }
}

/// Own path maluses (nearby fire 16, fire refused; zombified piglin:
/// lava 8).
pub fn malus(k: Kind, t: PathType) -> Option<f32> {
    use PathType::*;
    Some(match (k, t) {
        (Kind::Piglin | Kind::PiglinBrute, FireInNeighbor) => 16.0,
        (Kind::Piglin | Kind::PiglinBrute, Fire) => -1.0,
        (Kind::ZombifiedPiglin, Lava) => 8.0,
        _ => return None,
    })
}

/// +20% speed for the baby (passenger modifier).
pub fn set_piglin_baby(m: &mut Mob, baby: bool) {
    m.baby = baby;
    m.attrs.speed.remove_mod("minecraft:baby");
    if baby {
        m.attrs.speed.set_mod("minecraft:baby", f64::from(0.2f32), 1);
    }
}

/// Damage 0.5 and 3 experience for a baby, 6 and 5 otherwise.
pub fn age_boundary(m: &mut Mob) {
    if m.kind != Kind::Hoglin {
        return;
    }
    m.pig.boundary_baby = m.is_baby();
    if m.is_baby() {
        m.pig.xp_reward = 3;
        m.attrs.attack.base = 0.5;
    } else {
        m.pig.xp_reward = 5;
        m.attrs.attack.base = 6.0;
    }
}

// ---------- Engine hooks ----------

/// Own walk value: hoglin (repellent −1, crimson nylium 10,
/// otherwise 0).
pub fn walk_target_value(s: &Server, m: &Mob, p: [i32; 3]) -> Option<f32> {
    if m.kind != Kind::Hoglin {
        return None;
    }
    let near = m.bm.brain.as_ref().and_then(|b| b.mem.pig.nearest_repellent.get()).is_some_and(|r| {
        let d = [f64::from(r[0] - p[0]), f64::from(r[1] - p[1]), f64::from(r[2] - p[2])];
        d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < 64.0
    });
    if near {
        return Some(-1.0);
    }
    Some(if s.bname([p[0], p[1] - 1, p[2]]) == "minecraft:crimson_nylium" { 10.0 } else { 0.0 })
}

/// Own ambient sound; `None`: no sound (piglin outside idle, etc.).
pub fn ambient_sound(s: &Server, m: &Mob) -> Option<String> {
    match m.kind {
        // Only at idle, the activity's sound.
        Kind::Piglin => {
            let b = m.bm.brain.as_ref()?;
            if b.act != Act::Idle {
                return None;
            }
            Some(behave::piglin_activity_sound(s, m, b).into())
        }
        Kind::PiglinBrute => {
            let b = m.bm.brain.as_ref()?;
            (b.act == Act::Idle).then(|| "minecraft:entity.piglin_brute.ambient".into())
        }
        Kind::Hoglin => {
            let b = m.bm.brain.as_ref()?;
            Some(behave::hoglin_activity_sound(s, m, b).into())
        }
        Kind::Zoglin => {
            let angry = m.bm.brain.as_ref().is_some_and(|b| b.mem.attack.has());
            Some(if angry { "minecraft:entity.zoglin.angry" } else { "minecraft:entity.zoglin.ambient" }.into())
        }
        _ => {
            let angry = zombified::is_angry(s, m);
            Some(
                if angry {
                    "minecraft:entity.zombified_piglin.angry"
                } else {
                    "minecraft:entity.zombified_piglin.ambient"
                }
                .into(),
            )
        }
    }
}

/// Despawning far from players.
pub fn remove_when_far_away(m: &Mob) -> Option<bool> {
    // Hoglin: always (despite the animal class).
    (m.kind == Kind::Hoglin).then_some(true)
}

/// Own spawn rule (world random); `None`: common rule.
pub fn spawn_rules(s: &mut Server, k: Kind, p: [i32; 3]) -> Option<bool> {
    let below = s.bname([p[0], p[1] - 1, p[2]]) == "minecraft:nether_wart_block";
    match k {
        // The piglins' spawn rules, the hoglins' spawn rules: not on a wart block.
        Kind::Piglin | Kind::Hoglin => Some(!below),
        // Difficulty then wart.
        Kind::ZombifiedPiglin => Some(s.level.difficulty_id() != 0 && !below),
        // Brute and zoglin: monster rules, common.
        _ => None,
    }
}

/// Obstruction (of the zombified piglin): space free of entities and
/// liquid; `None`: common rule.
pub fn obstruction_ok(s: &Server, m: &Mob) -> Option<bool> {
    if m.kind != Kind::ZombifiedPiglin {
        return None;
    }
    let b = m.bbox();
    let blocked = s.mobs.list.iter().flatten().any(|o| o.id != m.id && o.alive() && o.touches(&b))
        || (s.game_mode != 3 && s.player_aabb().intersects(&b));
    if blocked {
        return Some(false);
    }
    Some(!s.box_has_liquid(&b))
}

/// Own experience: piglin 5 and hoglin (5, baby 3) without equipment
/// bonus.
pub fn xp(m: &Mob) -> Option<i32> {
    match m.kind {
        Kind::Piglin => Some(5),
        Kind::Hoglin => Some(m.pig.xp_reward),
        _ => None,
    }
}

/// Death loot: the hoglin is an animal (never a baby).
pub fn drops_loot(m: &Mob) -> Option<bool> {
    (m.kind == Kind::Hoglin).then(|| !m.is_baby())
}

/// Attribute `gameplay/piglins_zombify` (biome, otherwise dimension; true by default).
pub fn zombifies_here(s: &Server, p: [f64; 3]) -> bool {
    env_bool(s, super::mth::block_of(p), "minecraft:gameplay/piglins_zombify", true)
}

/// Boolean environment attribute: biome, otherwise dimension type, otherwise default value.
pub fn env_bool(s: &Server, bp: [i32; 3], key: &str, default: bool) -> bool {
    let biome = s.biome_str(bp);
    if let Some(v) = s.life.pack.get("worldgen/biome", biome)
        && let Some(b) = v["attributes"][key].as_bool()
    {
        return b;
    }
    s.dim_attribute(&s.dim.name, key).and_then(|v| v.as_bool()).unwrap_or(default)
}

/// Neither immune nor without AI, "zombified piglins" attribute true (Overworld, End).
pub fn is_converting(s: &Server, m: &Mob) -> bool {
    matches!(m.kind, Kind::Piglin | Kind::PiglinBrute | Kind::Hoglin)
        && !m.pig.immune
        && !m.no_ai
        && zombifies_here(s, m.pos)
}

/// Start of the AI step: attack animation of the hoglin and the zoglin.
pub fn pre_ai_step(m: &mut Mob) {
    if matches!(m.kind, Kind::Hoglin | Kind::Zoglin) && m.pig.attack_anim > 0 {
        m.pig.attack_anim -= 1;
    }
}

/// Brain, activity, zombification (piglins, hoglin); anger of the zombified
/// piglin.
pub fn custom_ai_step(s: &mut Server, m: &mut Mob) {
    // Called for every mob: the other brains (F10, F14) have their own tick.
    if !handles(m.kind) {
        return;
    }
    if m.kind == Kind::ZombifiedPiglin {
        zombified::custom_ai_step(s, m);
        return;
    }
    for by in std::mem::take(&mut m.pig.pending_hurt_by) {
        if s.ent_alive(by) {
            react_to_hurt(s, m, by);
        }
    }
    let Some(mut b) = m.bm.brain.take() else { return };
    s.bz_brain_tick(m, &mut b);
    match m.kind {
        Kind::Piglin => s.piglin_update_activity(m, &mut b),
        Kind::PiglinBrute => s.brute_update_activity(m, &mut b),
        Kind::Hoglin => s.hoglin_update_activity(m, &mut b),
        _ => s.zoglin_update_activity(m, &mut b),
    }
    m.target = b.mem.attack.get();
    m.bm.brain = Some(b);
    if m.kind == Kind::Zoglin {
        return;
    }
    // The piglins' server AI step / the hoglin's server AI step: time outside the Nether.
    if is_converting(s, m) {
        m.pig.time_in_overworld += 1;
    } else {
        m.pig.time_in_overworld = 0;
    }
    if m.pig.time_in_overworld > 300 {
        s.pig_finish_conversion(m);
    }
}

/// End of the common tick: age threshold of the hoglin, only when the "baby" state
/// flips (otherwise the base set by `/attribute` would be overwritten every tick).
pub fn post_tick(_s: &mut Server, m: &mut Mob) {
    if m.is_baby() != m.pig.boundary_baby {
        age_boundary(m);
    }
}

/// World state of the family: range of the targeting conditions shared by all the
/// game's sensors (static: the last mob whose sensor ticked sets it).
#[derive(Debug, Clone)]
pub struct PigWorld {
    pub sensor_range: f64,
}

impl Default for PigWorld {
    fn default() -> PigWorld {
        PigWorld { sensor_range: 16.0 }
    }
}

/// After the common finalization; own finalization (world random).
pub fn finalize_pre(s: &mut Server, m: &mut Mob, reason: super::spawn::Reason) {
    match m.kind {
        Kind::Piglin => {
            if reason != super::spawn::Reason::Structure {
                if s.mobs.rng.next_float() < 0.2 {
                    set_piglin_baby(m, true);
                } else {
                    // Entity random.
                    let w = if f64::from(m.rng.next_float()) < 0.5 {
                        "minecraft:crossbow"
                    } else if m.rng.next_int_bound(10) == 0 {
                        "minecraft:golden_spear"
                    } else {
                        "minecraft:golden_sword"
                    };
                    m.equipment[0] = Some(Stack { id: w.into(), count: 1, components: None });
                }
            }
            // First hunt in 30 to 120 s (world random).
            let t = s.mobs.rng.int_between(600, 2400);
            if let Some(b) = m.bm.brain.as_mut() {
                b.mem.pig.hunted_recently.set_exp(true, i64::from(t));
            }
            // Gold armor (10% per piece, head first), then equipment enchantments.
            if !m.is_baby() {
                for (slot, id) in [
                    (5, "minecraft:golden_helmet"),
                    (4, "minecraft:golden_chestplate"),
                    (3, "minecraft:golden_leggings"),
                    (2, "minecraft:golden_boots"),
                ] {
                    if s.mobs.rng.next_float() < 0.1 {
                        m.equipment[slot] = Some(Stack { id: id.into(), count: 1, components: None });
                    }
                }
            }
            let (_, special) = s.local_difficulty(m.block_pos());
            s.enchant_equipment(m, special);
        }
        Kind::PiglinBrute => {
            // Home at the spawn place.
            let g = GPos { dim: DimKey::of(&s.dim.name), pos: m.block_pos() };
            if let Some(b) = m.bm.brain.as_mut() {
                b.mem.pig.home.set(g);
            }
            m.equipment[0] = Some(Stack { id: "minecraft:golden_axe".into(), count: 1, components: None });
        }
        Kind::Hoglin => {
            if s.mobs.rng.next_float() < 0.2 {
                m.age = -24000;
                age_boundary(m);
            }
        }
        // The draw happens only for the zoglin (arm guard).
        Kind::Zoglin if s.mobs.rng.next_float() < 0.2 => {
            m.baby = true;
            m.attrs.attack.base = 0.5;
        }
        _ => {}
    }
}

/// After damage dealt by a living entity (server-side damage computation then the damage reaction).
pub fn after_hurt(s: &mut Server, m: &mut Mob, by: Option<Ent>) {
    let Some(by) = by else { return };
    if !s.ent_alive(by) && by == Ent::Player {
        return;
    }
    // The acting attacker is out of the list: reaction on the next tick.
    if let Ent::Mob(id) = by
        && s.mob_ref(id).is_none()
    {
        if has_brain(m.kind) {
            m.pig.pending_hurt_by.push(by);
        }
        return;
    }
    react_to_hurt(s, m, by);
}

fn react_to_hurt(s: &mut Server, m: &mut Mob, by: Ent) {
    match m.kind {
        Kind::Piglin => s.piglin_was_hurt_by(m, by),
        Kind::PiglinBrute => s.brute_was_hurt_by(m, by),
        Kind::Hoglin => s.hoglin_was_hurt_by(m, by),
        Kind::Zoglin => s.zoglin_was_hurt_by(m, by),
        _ => {}
    }
}

/// Death: piglin inventory dropped (rule `mob_drops`).
pub fn on_death(s: &mut Server, m: &mut Mob) {
    if m.kind == Kind::Piglin && s.rule("minecraft:mob_drops", true) {
        for st in std::mem::take(&mut m.pig.inventory) {
            s.pig_spawn_at_location(m, st);
        }
    }
}

impl Server {
    /// Piglin and brute → zombified piglin, hoglin → zoglin (nausea 10 s).
    pub(crate) fn pig_finish_conversion(&mut self, m: &mut Mob) {
        if m.removed {
            return;
        }
        match m.kind {
            Kind::Piglin | Kind::PiglinBrute => {
                if self.level.difficulty_id() != 0 {
                    let snd = if m.kind == Kind::Piglin {
                        "minecraft:entity.piglin.converted_to_zombified"
                    } else {
                        "minecraft:entity.piglin_brute.converted_to_zombified"
                    };
                    self.make_sound(m, snd);
                }
                if m.kind == Kind::Piglin {
                    self.piglin_cancel_admiring(m);
                    for st in std::mem::take(&mut m.pig.inventory) {
                        self.pig_spawn_at_location(m, st);
                    }
                }
                let mut n = self.converted(m, Kind::ZombifiedPiglin, true, true);
                self.give_mob_effect(&mut n, "minecraft:nausea", 200, 0);
                self.add_mob(n);
                m.removed = true;
            }
            Kind::Hoglin => {
                self.make_sound(m, "minecraft:entity.hoglin.converted_to_zombified");
                let mut n = self.converted(m, Kind::Zoglin, true, false);
                if n.is_baby() || m.is_baby() {
                    n.baby = true;
                    n.attrs.attack.base = 0.5;
                }
                self.give_mob_effect(&mut n, "minecraft:nausea", 200, 0);
                self.add_mob(n);
                m.removed = true;
            }
            _ => {}
        }
    }

    /// Spawn of a ground item (world random for the speed).
    pub(crate) fn pig_spawn_at_location(&mut self, m: &Mob, st: Stack) {
        let r = &mut self.mobs.rng;
        let v = [r.next_double() * 0.2 - 0.1, 0.2, r.next_double() * 0.2 - 0.1];
        self.spawn_item(m.pos, v, st, 10);
    }

    /// Right click on a mob of the family; `None`: common handling.
    pub(crate) fn pig_interact(&mut self, m: &mut Mob, slot: usize, item: &str) -> Option<bool> {
        match m.kind {
            Kind::Piglin => self.piglin_interact(m, slot, item),
            Kind::Hoglin => {
                // The player's interaction with an animal (feeding) if the hoglin can fall in love.
                let pacified = m.bm.brain.as_ref().is_some_and(|b| b.mem.pig.pacified.has());
                // Soothed: cannot fall in love; adult → no action (the common animal code must
                // not put it in love).
                if pacified && self.item_tag(item, "minecraft:hoglin_food") && m.age == 0 {
                    return Some(false);
                }
                let r = self.bz_animal_interact(m, slot, item);
                if r == Some(true) {
                    m.persistent = true;
                }
                r
            }
            _ => None,
        }
    }
}

/// Own random of a test (fixed seed).
#[cfg(test)]
pub fn test_rng() -> super::jrand::JRand {
    super::jrand::JRand::new(1)
}
