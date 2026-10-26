//! Brainy animals (family F10, `server-mobs.md` § 4.F10): goat, frog, tadpole, armadillo,
//! sniffer, axolotl, allay. Info sheets, own state, metadata, NBT (brain memories included),
//! spawning, hooks into the common engine (`mobs/`). The generic brain is in `brain.rs`, each
//! species in its own file.

pub mod allay;
pub mod armadillo;
pub mod axolotl;
pub mod brain;
mod eggs;
pub mod frog;
pub mod goat;
pub(crate) mod nbt;
pub mod sniffer;

#[cfg(test)]
mod tests;

use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::env::Fluid;
use super::mob::{Ent, Mob};
use super::species::{Category, Info, Kind, animal, info as base_info};
use crate::nbtio::{self, int};
use crate::player::Stack;
use crate::server::Server;
use brain::Brain;

// ---------- Poses ----------

pub const POSE_STANDING: i32 = 0;
pub const POSE_LONG_JUMPING: i32 = 6;
pub const POSE_CROAKING: i32 = 8;
pub const POSE_USING_TONGUE: i32 = 9;

// ---------- Info sheets ----------

static GOAT: Info = {
    let mut i = animal("minecraft:goat", 0.9, 1.3, 1.3 * 0.85, 10.0, 0.2f32 as f64);
    i.attack = 2.0;
    i.ambient = Some("minecraft:entity.goat.ambient");
    i.hurt = "minecraft:entity.goat.hurt";
    i.death = "minecraft:entity.goat.death";
    i.step_sound = Some("minecraft:entity.goat.step");
    i
};

static FROG: Info = {
    let mut i = animal("minecraft:frog", 0.5, 0.5, 0.5 * 0.85, 10.0, 1.0);
    i.attack = 10.0;
    i.step = 1.0;
    i.ambient = Some("minecraft:entity.frog.ambient");
    i.hurt = "minecraft:entity.frog.hurt";
    i.death = "minecraft:entity.frog.death";
    i.step_sound = Some("minecraft:entity.frog.step");
    i
};

/// Tadpole: "fish" base (the game's aquatic ambient mob, creature category).
static TADPOLE: Info = {
    let mut i = animal("minecraft:tadpole", 0.4, 0.3, 0.195, 6.0, 1.0);
    i.hurt = "minecraft:entity.tadpole.hurt";
    i.death = "minecraft:entity.tadpole.death";
    i
};

static ARMADILLO: Info = {
    let mut i = animal("minecraft:armadillo", 0.7, 0.65, 0.26, 12.0, 0.14);
    i.ambient = Some("minecraft:entity.armadillo.ambient");
    i.hurt = "minecraft:entity.armadillo.hurt";
    i.death = "minecraft:entity.armadillo.death";
    i.step_sound = Some("minecraft:entity.armadillo.step");
    i
};

static SNIFFER: Info = {
    let mut i = animal("minecraft:sniffer", 1.9, 1.75, 1.05, 14.0, 0.1f32 as f64);
    i.ambient = Some("minecraft:entity.sniffer.idle");
    i.hurt = "minecraft:entity.sniffer.hurt";
    i.death = "minecraft:entity.sniffer.death";
    i.step_sound = Some("minecraft:entity.sniffer.step");
    i
};

static AXOLOTL: Info = {
    let mut i = animal("minecraft:axolotl", 0.75, 0.42, 0.2751, 14.0, 1.0);
    i.category = Category::Axolotls;
    i.attack = 2.0;
    i.step = 1.0;
    i.hurt = "minecraft:entity.axolotl.hurt";
    i.death = "minecraft:entity.axolotl.death";
    i
};

/// Allay: flying mob (speed and flight 0.1), immune to falls, volume 0.4.
static ALLAY: Info = {
    let mut i = base_info("minecraft:allay", Category::Creature, 0.35, 0.6, 0.36);
    i.speed = 0.1f32 as f64;
    i.attack = 2.0;
    i.tracking = 8;
    i.volume = 0.4;
    i.fall_immune = true;
    i.hurt = "minecraft:entity.allay.hurt";
    i.death = "minecraft:entity.allay.death";
    i
};

pub fn info(k: Kind) -> &'static Info {
    match k {
        Kind::Goat => &GOAT,
        Kind::Frog => &FROG,
        Kind::Tadpole => &TADPOLE,
        Kind::Armadillo => &ARMADILLO,
        Kind::Sniffer => &SNIFFER,
        Kind::Axolotl => &AXOLOTL,
        _ => &ALLAY,
    }
}

pub fn handles(k: Kind) -> bool {
    matches!(k, Kind::Goat | Kind::Frog | Kind::Tadpole | Kind::Armadillo | Kind::Sniffer | Kind::Axolotl | Kind::Allay)
}

/// The game's animals: all except the tadpole (fish) and the allay.
pub fn is_animal(k: Kind) -> bool {
    matches!(k, Kind::Goat | Kind::Frog | Kind::Armadillo | Kind::Sniffer | Kind::Axolotl)
}

/// Age of animals (the frog is never a baby: the "can be a baby" test is false).
pub fn can_be_baby(k: Kind) -> bool {
    matches!(k, Kind::Goat | Kind::Armadillo | Kind::Sniffer | Kind::Axolotl)
}

/// Age of a baby at birth: sniffer −48,000.
pub fn baby_start_age(k: Kind) -> i32 {
    if k == Kind::Sniffer { -48000 } else { -24000 }
}

/// Dimensions of babies (width, height, eyes).
pub fn baby_dims(k: Kind) -> (f32, f32, f32) {
    match k {
        Kind::Goat => (0.45, 0.65, 0.59375),
        Kind::Armadillo => (0.7 * 0.6, 0.65 * 0.6, 0.21875),
        Kind::Axolotl => (0.375, 0.21, 0.09375),
        Kind::Sniffer => (1.9 * 0.5, 1.75 * 0.5, 1.05 * 0.5),
        k => {
            let i = info(k);
            (i.width, i.height, i.eye)
        }
    }
}

/// Pose dimensions: goat long jump (× 0.7), sniffer digging.
pub fn pose_dims(m: &Mob) -> Option<(f32, f32, f32)> {
    match m.kind {
        Kind::Goat if m.bm.pose == POSE_LONG_JUMPING => {
            let (w, h, e) = super::species::dims(m.kind, m.is_baby());
            Some((w * 0.7, h * 0.7, e * 0.7))
        }
        Kind::Sniffer if m.bm.sniffer == sniffer::DIGGING => {
            let s = if m.is_baby() { 0.5 } else { 1.0 };
            Some((1.9 * s, (1.75 - 0.4) * s, 0.81 * s))
        }
        _ => None,
    }
}

/// Food (item tag) of the animals of the family.
pub fn food_tag(k: Kind) -> Option<&'static str> {
    Some(match k {
        Kind::Goat => "minecraft:goat_food",
        Kind::Frog => "minecraft:frog_food",
        Kind::Armadillo => "minecraft:armadillo_food",
        Kind::Sniffer => "minecraft:sniffer_food",
        Kind::Axolotl => "minecraft:axolotl_food",
        _ => return None,
    })
}

/// The per-species head pitch rotation limit (axolotl: 1).
pub fn max_head_x(k: Kind) -> Option<f32> {
    (k == Kind::Axolotl).then_some(1.0)
}

/// The per-species head yaw rotation limit (goat 15, frog 5, sniffer 50, axolotl 1; armadillo: 32,
/// 0 when rolled up, set by the state).
pub fn max_head_y(k: Kind) -> Option<f32> {
    Some(match k {
        Kind::Goat => 15.0,
        Kind::Frog => 5.0,
        Kind::Sniffer => 50.0,
        Kind::Axolotl => 1.0,
        Kind::Armadillo => 32.0,
        _ => return None,
    })
}

/// Frog 35, otherwise 10.
pub fn head_rot_speed(k: Kind) -> f32 {
    match k {
        Kind::Frog => 35.0,
        // F14: breeze (25).
        Kind::Breeze => 25.0,
        _ => 10.0,
    }
}

// ---------- State ----------

/// State specific to the family (fields unused for the other mobs).
#[derive(Debug, Clone)]
pub struct Bm {
    pub brain: Option<Box<Brain>>,
    pub pose: i32,
    /// The discarded-friction setting (long jump).
    pub discard_friction: bool,
    // Goat.
    pub screaming: bool,
    pub left_horn: bool,
    pub right_horn: bool,
    // Frog.
    pub tongue: Option<i32>,
    pub frog_variant: Option<String>,
    // Tadpole (own age) and age lock.
    pub tadpole_age: i32,
    pub lock_timer: i32,
    /// Goat: "baby" state at the last age threshold applied (adult at construction).
    pub boundary_baby: bool,
    // Armadillo.
    pub armadillo: u8,
    pub in_state: i64,
    pub scute_time: i32,
    // Sniffer.
    pub sniffer: u8,
    pub drop_seed_at: i32,
    // Axolotl.
    pub playing_dead: bool,
    pub from_bucket: bool,
    // Allay.
    pub inventory: Option<Stack>,
    pub dancing: bool,
    pub dup_cooldown: i64,
    pub jukebox: Option<[i32; 3]>,
}

impl Default for Bm {
    fn default() -> Bm {
        Bm {
            brain: None,
            pose: POSE_STANDING,
            discard_friction: false,
            screaming: false,
            left_horn: true,
            right_horn: true,
            tongue: None,
            frog_variant: None,
            tadpole_age: 0,
            lock_timer: 0,
            boundary_baby: false,
            armadillo: 0,
            in_state: 0,
            scute_time: 0,
            sniffer: 0,
            drop_seed_at: 0,
            playing_dead: false,
            from_bucket: false,
            inventory: None,
            dancing: false,
            dup_cooldown: 0,
            jukebox: None,
        }
    }
}

/// Constructor: brain (sensor delays from the entity's random), then own draws.
pub fn on_new(m: &mut Mob) {
    let b = match m.kind {
        Kind::Goat => goat::brain(&mut m.rng),
        Kind::Frog => frog::brain(&mut m.rng),
        Kind::Tadpole => frog::tadpole_brain(&mut m.rng),
        Kind::Armadillo => armadillo::brain(&mut m.rng),
        Kind::Sniffer => sniffer::brain(&mut m.rng),
        Kind::Axolotl => axolotl::brain(&mut m.rng),
        _ => allay::brain(&mut m.rng),
    };
    m.bm.brain = Some(Box::new(b));
    if m.kind == Kind::Armadillo {
        m.bm.scute_time = armadillo::next_scute_time(m);
    }
}

pub fn pose(m: &Mob) -> i32 {
    m.bm.pose
}

pub fn set_pose(m: &mut Mob, p: i32) {
    m.bm.pose = p;
}

pub fn set_discard_friction(m: &mut Mob, v: bool) {
    m.bm.discard_friction = v;
}

/// Friction ignored (long jump in progress).
pub fn discard_friction(m: &Mob) -> bool {
    handles(m.kind) && m.bm.discard_friction
}

// ---------- Server-side construction ----------

/// Navigation by species.
pub fn init(m: &mut Mob) {
    match m.kind {
        Kind::Goat | Kind::Armadillo | Kind::Sniffer => m.nav.can_float = true,
        Kind::Frog | Kind::Axolotl => {
            m.nav.amphibious = true;
            m.nav.can_float = false;
        }
        Kind::Tadpole => {
            m.nav.swim = true;
            m.nav.breach = false;
            m.nav.can_float = false;
        }
        Kind::Allay => {
            m.nav.fly = true;
            m.nav.can_open_doors = false;
            m.nav.can_float = true;
            m.nav.required_length = 48.0;
        }
        _ => {}
    }
}

/// "Classic" goals: the tadpole keeps those of the fish (panic, flee, swim).
pub fn register_goals(m: &mut Mob) {
    if m.kind == Kind::Tadpole {
        super::sealife::fish::register(m);
    }
}

/// Own path maluses; `None`: common rule.
pub fn malus(k: Kind, t: super::nav::PathType) -> Option<f32> {
    use super::nav::PathType::*;
    Some(match (k, t) {
        (Kind::Goat, PowderSnow | OnTopOfPowderSnow) => -1.0,
        (Kind::Frog, Water) => 4.0,
        (Kind::Frog, Trapdoor) => -1.0,
        (Kind::Sniffer, Water | OnTopOfPowderSnow | DamageCautious) => -1.0,
        (Kind::Axolotl | Kind::Tadpole, Water) => 0.0,
        _ => return None,
    })
}

// ---------- Metadata ----------

pub fn define_data(m: &mut Mob) {
    match m.kind {
        Kind::Allay => {
            m.define(16, Meta::Bool(false));
            m.define(17, Meta::Bool(true));
        }
        Kind::Tadpole => {
            m.define(16, Meta::Bool(false));
            m.define(17, Meta::Bool(false));
        }
        k => {
            // Age: baby, age lock.
            m.define(16, Meta::Bool(false));
            m.define(17, Meta::Bool(false));
            match k {
                Kind::Goat => {
                    m.define(18, Meta::Bool(false));
                    m.define(19, Meta::Bool(true));
                    m.define(20, Meta::Bool(true));
                }
                Kind::Frog => {
                    m.define(18, Meta::Variant(frog::default_variant(m), 27));
                    m.define(19, Meta::OptionalInt(None));
                }
                Kind::Armadillo => m.define(18, Meta::Variant(0, 36)),
                Kind::Sniffer => {
                    m.define(18, Meta::Variant(0, 35));
                    m.define(19, Meta::Int(0));
                }
                Kind::Axolotl => {
                    m.define(18, Meta::Int(0));
                    m.define(19, Meta::Bool(false));
                    m.define(20, Meta::Bool(false));
                }
                _ => {}
            }
        }
    }
}

pub fn sync_data(m: &mut Mob) {
    match m.kind {
        Kind::Allay => {
            m.set(16, Meta::Bool(m.bm.dancing));
            m.set(17, Meta::Bool(m.bm.dup_cooldown == 0));
        }
        Kind::Tadpole => {
            // The "taken from a bucket" mark neutralized: the synchronized value stays false.
            m.set(16, Meta::Bool(false));
            m.set(17, Meta::Bool(m.mount.age_locked));
        }
        k => {
            m.set(16, Meta::Bool(m.is_baby()));
            m.set(17, Meta::Bool(m.mount.age_locked));
            match k {
                Kind::Goat => {
                    m.set(18, Meta::Bool(m.bm.screaming));
                    m.set(19, Meta::Bool(m.bm.left_horn));
                    m.set(20, Meta::Bool(m.bm.right_horn));
                }
                Kind::Frog => {
                    m.set(18, Meta::Variant(m.variant, 27));
                    m.set(19, Meta::OptionalInt(m.bm.tongue));
                }
                Kind::Armadillo => m.set(18, Meta::Variant(i32::from(m.bm.armadillo), 36)),
                Kind::Sniffer => {
                    m.set(18, Meta::Variant(i32::from(m.bm.sniffer), 35));
                    m.set(19, Meta::Int(m.bm.drop_seed_at));
                }
                Kind::Axolotl => {
                    m.set(18, Meta::Int(m.variant));
                    m.set(19, Meta::Bool(m.bm.playing_dead));
                    m.set(20, Meta::Bool(m.bm.from_bucket));
                }
                _ => {}
            }
        }
    }
}

// ---------- NBT ----------

pub fn load(m: &mut Mob, e: &Nbt) {
    let flag = |k: &str| int(e, k).is_some_and(|v| v != 0);
    if can_be_baby(m.kind) {
        m.mount.age_locked = flag("AgeLocked");
    }
    match m.kind {
        Kind::Goat => {
            m.bm.screaming = flag("IsScreamingGoat");
            m.bm.left_horn = int(e, "HasLeftHorn").is_none_or(|v| v != 0);
            m.bm.right_horn = int(e, "HasRightHorn").is_none_or(|v| v != 0);
            // Age threshold crossed at load only for a baby (age starts at 0): an
            // adult keeps the recorded damage base.
            if m.age < 0 {
                goat::age_boundary(m);
            }
        }
        Kind::Frog => {
            m.age = 0;
            m.bm.frog_variant = e.get("variant").and_then(Nbt::as_str).map(str::to_owned);
        }
        Kind::Tadpole => {
            m.bm.tadpole_age = int(e, "Age").unwrap_or(0) as i32;
            m.mount.age_locked = flag("AgeLocked");
        }
        Kind::Armadillo => {
            m.bm.armadillo = match e.get("state").and_then(Nbt::as_str) {
                Some("rolling") => 1,
                Some("scared") => 2,
                Some("unrolling") => 3,
                _ => 0,
            };
            if let Some(t) = int(e, "scute_time") {
                m.bm.scute_time = t as i32;
            }
        }
        Kind::Axolotl => {
            m.variant = int(e, "Variant").map_or(0, |v| if (0..5).contains(&v) { v as i32 } else { 0 });
            m.bm.from_bucket = flag("FromBucket");
        }
        Kind::Allay => {
            m.bm.inventory = match e.get("Inventory") {
                Some(Nbt::List(l)) => l.iter().find_map(Stack::from_nbt),
                _ => None,
            };
            m.rec.vib = crate::mobs::recent::vibration::VibData::load(e.get("listener"));
            m.bm.dup_cooldown = int(e, "DuplicationCooldown").unwrap_or(0);
        }
        _ => {}
    }
    if let Some(b) = m.bm.brain.as_mut() {
        nbt::load_memories(&mut b.mem, e.get("Brain"));
    }
}

pub fn save(m: &Mob, e: &mut Nbt) {
    let set = |e: &mut Nbt, k: &str, v: Nbt| nbtio::set(e, k, v);
    match m.kind {
        Kind::Frog => {
            // The "can be a baby" test false: neither age nor lock.
            for k in ["Age", "ForcedAge", "AgeLocked"] {
                nbtio::remove(e, k);
            }
        }
        k if can_be_baby(k) => set(e, "AgeLocked", Nbt::Byte(i8::from(m.mount.age_locked))),
        _ => {}
    }
    match m.kind {
        Kind::Goat => {
            set(e, "IsScreamingGoat", Nbt::Byte(i8::from(m.bm.screaming)));
            set(e, "HasLeftHorn", Nbt::Byte(i8::from(m.bm.left_horn)));
            set(e, "HasRightHorn", Nbt::Byte(i8::from(m.bm.right_horn)));
        }
        Kind::Frog => {
            if let Some(v) = frog::variant_name(m) {
                set(e, "variant", Nbt::String(v));
            }
        }
        Kind::Tadpole => {
            set(e, "Age", Nbt::Int(m.bm.tadpole_age));
            set(e, "AgeLocked", Nbt::Byte(i8::from(m.mount.age_locked)));
            set(e, "FromBucket", Nbt::Byte(1));
        }
        Kind::Armadillo => {
            set(e, "state", Nbt::String(armadillo::state(m).name().into()));
            set(e, "scute_time", Nbt::Int(m.bm.scute_time));
        }
        Kind::Axolotl => {
            set(e, "Variant", Nbt::Int(m.variant));
            set(e, "FromBucket", Nbt::Byte(i8::from(m.bm.from_bucket)));
        }
        Kind::Allay => {
            let inv: Vec<Nbt> = m.bm.inventory.iter().map(|s| s.to_nbt(None)).collect();
            if inv.is_empty() {
                nbtio::remove(e, "Inventory");
            } else {
                set(e, "Inventory", Nbt::List(inv));
            }
            set(e, "listener", m.rec.vib.save());
            set(e, "DuplicationCooldown", Nbt::Long(m.bm.dup_cooldown));
        }
        _ => {}
    }
    // Love only exists for animals.
    if !is_animal(m.kind) {
        nbtio::remove(e, "InLove");
        if !can_be_baby(m.kind) && m.kind != Kind::Tadpole {
            nbtio::remove(e, "Age");
            nbtio::remove(e, "ForcedAge");
        }
    }
    if let Some(b) = m.bm.brain.as_ref() {
        set(e, "Brain", nbt::save_memories(m.kind, &b.mem));
    }
}

// ---------- Sounds ----------

pub fn ambient_sound(m: &Mob) -> Option<String> {
    Some(
        match m.kind {
            Kind::Goat if m.bm.screaming => "minecraft:entity.goat.screaming.ambient",
            Kind::Goat => "minecraft:entity.goat.ambient",
            Kind::Frog => "minecraft:entity.frog.ambient",
            Kind::Tadpole => return None,
            Kind::Armadillo if armadillo::is_scared(m) => return None,
            Kind::Armadillo => "minecraft:entity.armadillo.ambient",
            Kind::Sniffer if matches!(m.bm.sniffer, sniffer::DIGGING | sniffer::SEARCHING) => return None,
            Kind::Sniffer => "minecraft:entity.sniffer.idle",
            // The axolotl that plays dead stays quiet (the draw happened).
            Kind::Axolotl if m.bm.playing_dead => return None,
            Kind::Axolotl if m.in_water => "minecraft:entity.axolotl.idle_water",
            Kind::Axolotl => "minecraft:entity.axolotl.idle_air",
            _ if m.equipment[0].is_some() => "minecraft:entity.allay.ambient_with_item",
            _ => "minecraft:entity.allay.ambient_without_item",
        }
        .into(),
    )
}

pub fn hurt_sound(m: &Mob, death: bool) -> Option<&'static str> {
    Some(match (m.kind, death) {
        (Kind::Goat, false) if m.bm.screaming => "minecraft:entity.goat.screaming.hurt",
        (Kind::Goat, true) if m.bm.screaming => "minecraft:entity.goat.screaming.death",
        (Kind::Armadillo, false) if armadillo::is_scared(m) => "minecraft:entity.armadillo.hurt_reduced",
        _ => return None,
    })
}

// ---------- Spawning ----------

/// Placement in the water of the axolotl; `None`: common ground placement.
pub fn placement_ok(s: &Server, k: Kind, p: [i32; 3]) -> Option<bool> {
    if k != Kind::Axolotl {
        return None;
    }
    let above = [p[0], p[1] + 1, p[2]];
    Some(s.mob_fluid(p) == Fluid::Water && !s.v().conductor(s.st(above)))
}

/// Own spawn rules.
pub fn spawn_rules(s: &mut Server, k: Kind, p: [i32; 3]) -> Option<bool> {
    let below = [p[0], p[1] - 1, p[2]];
    let bright = |s: &Server| {
        let (sky, block) = s.light(p);
        i32::from(sky.max(block)) > 8
    };
    Some(match k {
        Kind::Goat => s.btag_id(below, crate::tag_bits::TagId::GoatsSpawnableOn) && bright(s),
        Kind::Frog => s.btag_id(below, crate::tag_bits::TagId::FrogsSpawnableOn) && bright(s),
        Kind::Armadillo => s.btag_id(below, crate::tag_bits::TagId::ArmadilloSpawnableOn) && bright(s),
        Kind::Axolotl => s.btag_id(below, crate::tag_bits::TagId::AxolotlsSpawnableOn),
        k if handles(k) => false,
        _ => return None,
    })
}

/// Obstruction of the axolotl (entities only).
pub fn obstruction_ok(s: &Server, m: &Mob) -> Option<bool> {
    if m.kind != Kind::Axolotl {
        return None;
    }
    Some(s.unobstructed(&m.bbox(), m.id))
}

/// Despawning far from players.
pub fn remove_when_far_away(m: &Mob) -> Option<bool> {
    Some(match m.kind {
        Kind::Axolotl => !m.bm.from_bucket && m.custom_name.is_none(),
        k if handles(k) => false,
        _ => return None,
    })
}

/// Own persistence: axolotl taken out of a bucket, tadpole (always "from a bucket").
pub fn requires_persistence(m: &Mob) -> bool {
    (m.kind == Kind::Axolotl && m.bm.from_bucket) || m.kind == Kind::Tadpole
}

/// Finalization before the common part; true: finalization interrupted (axolotl bucket).
pub fn finalize_pre(
    s: &mut Server,
    m: &mut Mob,
    reason: super::spawn::Reason,
    group: &mut super::spawn::Group,
) -> bool {
    match m.kind {
        Kind::Goat => goat::finalize(s, m),
        Kind::Frog => frog::finalize(s, m),
        Kind::Axolotl => {
            if reason == super::spawn::Reason::Bucket {
                return true;
            }
            axolotl::finalize(s, m, group);
        }
        _ => {}
    }
    false
}

// ---------- Tick hooks ----------

/// Before the common tick: sniffer seeds.
pub fn pre_tick(s: &mut Server, m: &mut Mob) {
    if m.kind == Kind::Sniffer {
        sniffer::pre_tick(s, m);
    }
}

/// Air after the base tick: axolotl (6,000, rain included), tadpole (fish).
pub fn handle_air(s: &mut Server, m: &mut Mob, before: i32) {
    match m.kind {
        Kind::Axolotl => axolotl::handle_air(s, m, before),
        Kind::Tadpole => {
            if m.alive() && !m.in_water {
                m.air = before - 1;
                if s.should_take_drowning_damage(m) {
                    m.air = 0;
                    s.mob_hurt(m, "minecraft:drown", 2.0, None, None);
                }
            } else {
                m.air = 300;
            }
        }
        _ => {}
    }
}

/// Maximum air (axolotl: 6,000).
pub fn max_air(m: &Mob) -> Option<i32> {
    (m.kind == Kind::Axolotl).then_some(axolotl::MAX_AIR)
}

/// Start of the AI step: tadpole flopping on the ground.
pub fn pre_ai_step(s: &mut Server, m: &mut Mob) {
    if m.kind == Kind::Tadpole && !m.in_water && m.on_ground && m.vertical_collision {
        let dx = f64::from((m.rng.next_float() * 2.0 - 1.0) * 0.05);
        let dz = f64::from((m.rng.next_float() * 2.0 - 1.0) * 0.05);
        m.vel[0] += dx;
        m.vel[1] += f64::from(0.4f32);
        m.vel[2] += dz;
        m.on_ground = false;
        m.needs_sync = true;
        s.make_sound(m, "minecraft:entity.tadpole.flop");
    }
}

/// End of the AI step: tadpole age, allay healing and dance.
pub fn post_ai_step(s: &mut Server, m: &mut Mob) {
    match m.kind {
        Kind::Tadpole => frog::tadpole_ai_step(s, m),
        Kind::Allay => allay::post_ai_step(s, m),
        // Age lock particle timer.
        _ => age_lock_particles(s, m),
    }
}

/// Age lock particle timer: one tick in two, three draws from the entity random (X, Y, Z) then a
/// growth pause or reset particle.
pub(super) fn age_lock_particles(s: &mut Server, m: &mut Mob) {
    if m.bm.lock_timer <= 0 {
        return;
    }
    if m.bm.lock_timer % 2 == 0 {
        let locked = m.mount.age_locked;
        let (w, h) = (f64::from(m.width()), f64::from(m.height()));
        let x = m.pos[0] + w * ((2.0 * m.rng.next_double() - 1.0) * 1.0);
        let y = m.pos[1] + h * ((2.0 * m.rng.next_double() - 1.0) * 0.2);
        let z = m.pos[2] + w * ((2.0 * m.rng.next_double() - 1.0) * 1.0);
        let y = y + h + if locked { f64::from(0.2_f32) } else { 0.0 };
        let name = if locked { "minecraft:pause_mob_growth" } else { "minecraft:reset_mob_growth" };
        s.particles_full(name, 0, [x, y, z], [0.0; 3], [0.0; 3], 1, 0);
    }
    m.bm.lock_timer -= 1;
}

/// Brain, activity, own mechanics.
pub fn custom_ai_step(s: &mut Server, m: &mut Mob) {
    // Called for every mob: piglins (F13) and recent mobs (F14) tick their
    // brain themselves, only once per tick as in the game.
    if !handles(m.kind) {
        return;
    }
    let Some(mut b) = m.bm.brain.take() else { return };
    s.bz_brain_tick(m, &mut b);
    match m.kind {
        Kind::Goat => b.first_valid(&[brain::Act::Ram, brain::Act::LongJump, brain::Act::Idle]),
        Kind::Frog => b.first_valid(&[
            brain::Act::Tongue,
            brain::Act::LaySpawn,
            brain::Act::LongJump,
            brain::Act::Swim,
            brain::Act::Idle,
        ]),
        Kind::Tadpole | Kind::Allay => b.first_valid(&[brain::Act::Idle]),
        Kind::Armadillo => b.first_valid(&[brain::Act::Panic, brain::Act::Idle]),
        Kind::Sniffer => b.first_valid(&[brain::Act::Dig, brain::Act::Sniff, brain::Act::Idle]),
        Kind::Axolotl => axolotl::update_activity(&mut b),
        _ => {}
    }
    // Attack target of the brain (frog, axolotl).
    if matches!(m.kind, Kind::Frog | Kind::Axolotl) {
        m.target = b.mem.attack.get();
    }
    match m.kind {
        Kind::Armadillo => armadillo::custom_ai_step(s, m),
        Kind::Axolotl if !m.no_ai => m.bm.playing_dead = b.mem.play_dead.get().is_some_and(|t| t > 0),
        _ => {}
    }
    m.bm.brain = Some(b);
}

/// End of the common tick: armadillo state, vibrations and allay dance.
pub fn post_tick(s: &mut Server, m: &mut Mob) {
    match m.kind {
        Kind::Armadillo => armadillo::post_tick(m),
        // Goat damage, only when the "baby" state flips
        // (otherwise the base set by `/attribute` would be overwritten every tick).
        Kind::Goat if m.is_baby() != m.bm.boundary_baby => goat::age_boundary(m),
        Kind::Allay => {
            allay::post_tick(s, m); // G6-F10: note block heard by vibration
            if is_panicking(m) {
                allay::set_dancing(m, false);
            }
        }
        _ => {}
    }
}

/// Own movement control; true if it replaces the common one.
pub fn move_control(m: &mut Mob) -> bool {
    match m.kind {
        Kind::Frog | Kind::Tadpole => {
            smooth_swim_move(m, 85.0, 10.0, 0.02, 0.1, true);
            true
        }
        Kind::Axolotl => {
            if !m.bm.playing_dead {
                smooth_swim_move(m, 85.0, 10.0, 0.1, 0.5, false);
            }
            true
        }
        Kind::Allay => {
            allay::fly_move_control(m);
            true
        }
        _ => false,
    }
}

/// Own gaze control; true if it replaces the common one.
pub fn look_control(s: &Server, m: &mut Mob) -> bool {
    match m.kind {
        Kind::Tadpole => {
            smooth_swim_look(m, 10.0);
            true
        }
        Kind::Axolotl => {
            if !m.bm.playing_dead {
                smooth_swim_look(m, 20.0);
            }
            true
        }
        Kind::Frog => {
            // Pitch reset to 0 except with the tongue out.
            let reset = m.bm.tongue.is_none();
            plain_look(m, reset);
            let _ = s;
            true
        }
        _ => false,
    }
}

/// The gaze control with optional pitch reset to zero.
fn plain_look(m: &mut Mob, reset: bool) {
    use super::mth::{pitch_to, rotate_if_necessary, rotate_towards, yaw_to};
    if reset {
        m.pitch = 0.0;
    }
    if m.look.cooldown > 0 {
        m.look.cooldown -= 1;
        let w = m.look.wanted;
        let (xd, zd) = (w[0] - m.pos[0], w[2] - m.pos[2]);
        if zd.abs() > f64::from(1.0e-5f32) || xd.abs() > f64::from(1.0e-5f32) {
            m.head_yaw = rotate_towards(m.head_yaw, yaw_to(xd, zd), m.look.y_max);
        }
        let yd = w[1] - m.eye_y();
        let sd = (xd * xd + zd * zd).sqrt();
        if yd.abs() > f64::from(1.0e-5f32) || sd.abs() > f64::from(1.0e-5f32) {
            m.pitch = rotate_towards(m.pitch, pitch_to(yd, sd), m.look.x_max);
        }
    } else {
        m.head_yaw = rotate_towards(m.head_yaw, m.body_yaw, 10.0);
    }
    if !m.nav.is_done() {
        m.head_yaw = rotate_if_necessary(m.head_yaw, m.body_yaw, super::goals::max_head_y(m.kind));
    }
}

/// Body rotation: frozen on a rolled-up armadillo.
pub fn freezes_body(m: &Mob) -> bool {
    m.kind == Kind::Armadillo && armadillo::is_scared(m)
}

/// The smooth swim control.
pub fn smooth_swim_move(m: &mut Mob, max_x: f32, max_y: f32, in_water: f32, outside: f32, gravity: bool) {
    use super::mob::MoveOp;
    use super::mth::{DEG, cos, pitch_to, rotate_towards, rotlerp_360, sin, wrap, yaw_to};
    if gravity && m.in_water {
        m.vel[1] += 0.005;
    }
    if m.move_ctl.op == MoveOp::MoveTo && !m.nav.is_done() {
        let w = m.move_ctl.wanted;
        let (xd, yd, zd) = (w[0] - m.pos[0], w[1] - m.pos[1], w[2] - m.pos[2]);
        let dd = xd * xd + yd * yd + zd * zd;
        if dd < f64::from(2.500_000_3e-7f32) {
            m.zza = 0.0;
            return;
        }
        let yr = yaw_to(xd, zd);
        m.yaw = rotlerp_360(m.yaw, yr, max_y);
        m.body_yaw = m.yaw;
        m.head_yaw = m.yaw;
        let speed = (m.move_ctl.speed_mod * m.movement_speed()) as f32;
        if m.in_water {
            m.speed = speed * in_water;
            m.zza = m.speed;
            let sd = (xd * xd + zd * zd).sqrt();
            if yd.abs() > f64::from(1.0e-5f32) || sd.abs() > f64::from(1.0e-5f32) {
                let xr = wrap(pitch_to(yd, sd)).clamp(-max_x, max_x);
                m.pitch = rotate_towards(m.pitch, xr, 5.0);
            }
            let (c, s) = (cos(m.pitch * DEG), sin(m.pitch * DEG));
            m.zza = c * speed;
            m.yya = -s * speed;
        } else {
            let left = wrap(m.yaw - yr).abs();
            let factor = 1.0 - ((left - 10.0) / 50.0).clamp(0.0, 1.0);
            m.speed = speed * outside * factor;
            m.zza = m.speed;
        }
    } else {
        m.speed = 0.0;
        m.xxa = 0.0;
        m.yya = 0.0;
        m.zza = 0.0;
    }
}

/// The smooth swim gaze control (head/body difference clamped, the body follows by 4°).
pub fn smooth_swim_look(m: &mut Mob, max_from_center: f32) {
    use super::mth::{pitch_to, rotate_towards, wrap, yaw_to};
    if m.look.cooldown > 0 {
        m.look.cooldown -= 1;
        let w = m.look.wanted;
        let (xd, zd) = (w[0] - m.pos[0], w[2] - m.pos[2]);
        if zd.abs() > f64::from(1.0e-5f32) || xd.abs() > f64::from(1.0e-5f32) {
            m.head_yaw = rotate_towards(m.head_yaw, yaw_to(xd, zd) + 20.0, m.look.y_max);
        }
        let yd = w[1] - m.eye_y();
        let sd = (xd * xd + zd * zd).sqrt();
        if yd.abs() > f64::from(1.0e-5f32) || sd.abs() > f64::from(1.0e-5f32) {
            m.pitch = rotate_towards(m.pitch, pitch_to(yd, sd) + 10.0, m.look.x_max);
        }
    } else {
        if m.nav.is_done() {
            m.pitch = rotate_towards(m.pitch, 0.0, 5.0);
        }
        m.head_yaw = rotate_towards(m.head_yaw, m.body_yaw, m.look.y_max);
    }
    let d = wrap(m.head_yaw - m.body_yaw);
    if d < -max_from_center {
        m.body_yaw -= 4.0;
    } else if d > max_from_center {
        m.body_yaw += 4.0;
    }
}

/// Own movement; true if it replaces the common one (allay flight, swimming).
pub fn travel(s: &mut Server, m: &mut Mob) -> bool {
    match m.kind {
        Kind::Allay => {
            allay::travel(s, m);
            true
        }
        Kind::Frog | Kind::Axolotl if m.in_water => {
            let sp = m.speed;
            s.move_relative_pub(m, sp);
            let v = m.vel;
            s.mob_move(m, v);
            m.vel = m.vel.map(|x| x * 0.9);
            true
        }
        Kind::Tadpole if m.in_water => {
            s.move_relative_pub(m, 0.01);
            let v = m.vel;
            s.mob_move(m, v);
            m.vel = m.vel.map(|x| x * 0.9);
            if m.target.is_none() {
                m.vel[1] -= 0.005;
            }
            true
        }
        _ => false,
    }
}

// ---------- Combat ----------

/// Before damage (overridden): `Some(r)` stops processing.
pub fn before_hurt(s: &mut Server, m: &mut Mob, ty: &str, amount: &mut f32, by: Option<Ent>) -> Option<bool> {
    match m.kind {
        Kind::Armadillo if armadillo::is_scared(m) => {
            *amount = (*amount - 1.0) / 2.0;
            None
        }
        Kind::Axolotl => {
            axolotl::maybe_play_dead(s, m, *amount, by);
            None
        }
        Kind::Allay if by == Some(Ent::Player) && allay::likes_player(s, m) => Some(false),
        _ => {
            let _ = ty;
            None
        }
    }
}

/// After damage dealt (damage application of an animal).
pub fn after_hurt(s: &mut Server, m: &mut Mob, ty: &str, by: Option<Ent>) {
    if is_animal(m.kind) {
        m.reset_love();
    }
    if m.kind == Kind::Armadillo {
        armadillo::after_hurt(s, m, ty, by);
    }
}

/// Reduced fall damage: goat −10, frog −5.
pub fn fall_reduction(k: Kind) -> i32 {
    match k {
        Kind::Goat => 10,
        Kind::Frog => 5,
        _ => 0,
    }
}

/// Death: the sniffer returns to idle.
pub fn on_death(s: &mut Server, m: &mut Mob) {
    if m.kind == Kind::Sniffer {
        sniffer::transition(s, m, sniffer::IDLING);
    }
    if m.kind == Kind::Allay {
        allay::drop_inventory(s, m);
    }
}

/// Experience: animals 1 to 3; tadpole and allay: nothing.
pub fn xp(m: &mut Mob) -> Option<i32> {
    match m.kind {
        Kind::Tadpole | Kind::Allay => Some(0),
        k if is_animal(k) => Some(1 + m.rng.next_int_bound(3)),
        _ => None,
    }
}

/// The axolotl that plays dead is not a target.
pub fn hidden_from_enemies(m: &Mob) -> bool {
    m.kind == Kind::Axolotl && m.bm.playing_dead
}

// ---------- World ----------

/// World state specific to the family: killer in progress (id, components read by the loot
/// tables: frog variant).
#[derive(Debug, Clone, Default)]
pub struct World {
    pub killer: Option<(i32, Vec<(String, String)>)>,
}

// ---------- Buckets (tadpole, axolotl) ----------

/// Pickup with a water bucket; `None`: other item.
pub fn bucket_pickup(s: &mut Server, m: &mut Mob, slot: usize, item: &str) -> Option<bool> {
    if item != "minecraft:water_bucket" || !m.alive() {
        return None;
    }
    let snd = if m.kind == Kind::Tadpole {
        "minecraft:item.bucket.fill_tadpole"
    } else {
        "minecraft:item.bucket.fill_axolotl"
    };
    s.mob_sound(m, snd, 1.0, 1.0);
    let filled = frog::bucket_of(m);
    // The exchange for the filled item without creative limit: the water bucket stays in creative.
    super::surface::mooshroom::fill_held(s, slot, filled, false);
    m.removed = true;
    Some(true)
}

/// Mob taken out of a bucket: bucket data, "from bucket" flag (axolotl).
pub fn load_from_bucket(m: &mut Mob, bucket: &Stack) {
    let data =
        crate::items::component(bucket, "minecraft:bucket_entity_data").cloned().unwrap_or(Nbt::Compound(Vec::new()));
    match m.kind {
        Kind::Tadpole => frog::load_bucket_data(m, &data),
        Kind::Axolotl => {
            axolotl::load_bucket_data(m, bucket, &data);
            m.bm.from_bucket = true;
        }
        _ => {}
    }
}

/// Sound of the emptied bucket.
pub fn bucket_empty_sound(k: Kind) -> Option<&'static str> {
    match k {
        Kind::Tadpole => Some("minecraft:item.bucket.empty_tadpole"),
        Kind::Axolotl => Some("minecraft:item.bucket.empty_axolotl"),
        _ => None,
    }
}

// ---------- Common tools ----------

/// Dimensions of a pose (long jump: goat × 0.7; frog unchanged).
pub fn pose_dims_for(m: &Mob, pose: i32) -> (f32, f32, f32) {
    let (w, h, e) = super::species::dims(m.kind, m.is_baby());
    if m.kind == Kind::Goat && pose == POSE_LONG_JUMPING { (w * 0.7, h * 0.7, e * 0.7) } else { (w, h, e) }
}

/// Pushed by currents: neither the frog, nor the axolotl, nor the tadpole.
pub fn pushed_by_fluid(k: Kind) -> bool {
    !matches!(k, Kind::Frog | Kind::Axolotl | Kind::Tadpole)
}

/// Breathes underwater (tag `can_breathe_under_water`): axolotl, frog, tadpole.
pub fn breathes_underwater(k: Kind) -> bool {
    matches!(k, Kind::Axolotl | Kind::Frog | Kind::Tadpole)
}

/// After a jump: push of the almost motionless sniffer.
pub fn after_jump(m: &mut Mob) {
    if m.kind == Kind::Sniffer {
        sniffer::after_jump(m);
    }
}

/// Server-side construction: navigation, frog variant.
pub fn init_server(s: &Server, m: &mut Mob) {
    if !handles(m.kind) {
        return;
    }
    init(m);
    if m.kind == Kind::Frog {
        frog::init(s, m);
    }
}

/// Breeding in progress.
pub fn is_breeding(b: &Brain) -> bool {
    b.mem.breed.has()
}

/// A mob's panic test: the brain's panic memory (brain families), otherwise the common engine's
/// goal panic.
pub fn is_panicking(m: &Mob) -> bool {
    m.bm.brain.as_ref().is_some_and(|b| b.mem.panicking.has())
}

/// The possible-mating test (+ overrides: idle sniffer).
pub fn can_mate(_s: &Server, m: &Mob, o: &Mob) -> bool {
    if o.id == m.id || o.kind != m.kind || !m.is_in_love() || !o.is_in_love() {
        return false;
    }
    if m.kind == Kind::Sniffer {
        let ok = |st: u8| matches!(st, sniffer::IDLING | sniffer::SCENTING | sniffer::FEELING_HAPPY);
        return ok(m.bm.sniffer) && ok(o.bm.sniffer);
    }
    true
}

/// Melee hit and attack sound of the axolotl.
pub fn melee_hit(s: &mut Server, m: &mut Mob, target: Ent) {
    let hit = s.mob_attack(m, target);
    if hit && m.kind == Kind::Axolotl {
        s.mob_sound(m, "minecraft:entity.axolotl.attack", 1.0, 1.0);
    }
}

/// Birth by species.
pub fn spawn_child(s: &mut Server, m: &mut Mob, partner: i32) {
    match m.kind {
        Kind::Frog => {
            finalize_breeding(s, m, partner, None);
            if let Some(b) = m.bm.brain.as_mut() {
                b.mem.pregnant.set(());
            }
        }
        Kind::Sniffer => sniffer::spawn_egg(s, m, partner),
        _ => {
            let baby = match m.kind {
                Kind::Goat => goat::offspring(s, m, partner),
                Kind::Axolotl => axolotl::offspring(s, m, partner),
                k => s.new_mob(k, m.pos, 0.0),
            };
            let mut baby = baby;
            baby.age = baby_start_age(baby.kind);
            baby.pos = m.pos;
            baby.old = m.pos;
            baby.yaw = 0.0;
            baby.pitch = 0.0;
            // F13: the baby hoglin is persistent.
            if baby.kind == Kind::Hoglin {
                baby.persistent = true;
            }
            finalize_breeding(s, m, partner, Some(&baby));
            s.add_mob(baby);
        }
    }
}

/// Breeding statistic and trigger (player who caused the love), ages, love forgotten, event 18,
/// orbs (1 to 7).
pub fn finalize_breeding(s: &mut Server, m: &mut Mob, partner: i32, child: Option<&Mob>) {
    s.s9_bred(m, partner, child);
    m.age = 6000;
    m.reset_love();
    if let Some(i) = s.mob_index(partner)
        && let Some(p) = s.mobs.list[i].as_mut()
    {
        p.age = 6000;
        p.reset_love();
    }
    s.mob_event(m, 18);
    if s.rule("minecraft:mob_drops", true) {
        let xp = m.rng.next_int_bound(7) + 1;
        s.spawn_orbs(m.pos, xp);
    }
}

/// Path computed between the start and the end of the pathfinding (sniffer: water becomes
/// walkable if it burns or swims).
pub fn path_begin<T>(s: &Server, m: &mut Mob, f: impl FnOnce(&Server, &mut Mob) -> T) -> T {
    let water_ok = m.kind == Kind::Sniffer && (m.on_fire() || m.in_water);
    if water_ok {
        SNIFFER_WATER.with(|c| c.set(true));
    }
    let r = f(s, m);
    if water_ok {
        SNIFFER_WATER.with(|c| c.set(false));
    }
    r
}

thread_local! {
    /// Sniffer water malus lifted during a path computation.
    pub static SNIFFER_WATER: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// Sniffer water malus during the current computation.
pub fn sniffer_water_ok() -> bool {
    SNIFFER_WATER.with(std::cell::Cell::get)
}

/// Traversal by increasing Manhattan distance: x increasing,
/// then y, then positive z before its mirror; first accepted block.
pub fn manhattan_find(o: [i32; 3], rh: i32, rv: i32, mut f: impl FnMut([i32; 3]) -> bool) -> Option<[i32; 3]> {
    for d in 0..=rh + rv + rh {
        let mx = rh.min(d);
        for x in -mx..=mx {
            let my = rv.min(d - x.abs());
            for y in -my..=my {
                let z = d - x.abs() - y.abs();
                if z > rh {
                    continue;
                }
                let zs: &[i32] = if z == 0 { &[0] } else { &[z, -z] };
                for &zz in zs {
                    let p = [o[0] + x, o[1] + y, o[2] + zz];
                    if f(p) {
                        return Some(p);
                    }
                }
            }
        }
    }
    None
}

impl Server {
    /// Entity type in a tag (`tags/entity_type`).
    pub(crate) fn bz_etag(&self, kind: Kind, tag: &str) -> bool {
        let Some(idx) = self.entity_kind_index(kind.info().name) else { return false };
        self.regs
            .tags
            .iter()
            .find(|(r, _)| r == "minecraft:entity_type")
            .and_then(|(_, t)| t.iter().find(|(n, _)| n == tag))
            .is_some_and(|(_, ids)| ids.contains(&(idx as i32)))
    }

    /// The possible-attack test of a mob of the family (living target, player outside peaceful and
    /// creative, axolotl playing dead excluded).
    pub(crate) fn bz_mob_can_attack(&self, _m: &Mob, target: Ent) -> bool {
        match target {
            Ent::Player => {
                self.game_mode != 1 && self.game_mode != 3 && self.level.difficulty_id() != 0 && self.ent_alive(target)
            }
            Ent::Mob(id) => self.mob_ref(id).is_some_and(|o| o.alive() && !hidden_from_enemies(o)),
        }
    }

    /// Sturdy top face.
    pub(crate) fn bz_face_sturdy_up(&self, p: [i32; 3]) -> bool {
        self.v().sturdy(self.st(p), crate::blocks::Dir::Up)
    }

    /// Exchanging an empty item for its filled result for a filled bucket (creative: the original stays).
    pub(crate) fn bz_fill_bucket(&mut self, slot: usize, id: &str) {
        let st = Stack { id: id.into(), count: 1, components: None };
        super::surface::mooshroom::fill_held(self, slot, st, true);
    }

    /// Source entity of the damage for the loot tables: known type and components.
    pub(crate) fn bz_loot_source(&self, by: Option<Ent>) -> Option<(String, Vec<(String, String)>)> {
        match by? {
            Ent::Player => Some(("minecraft:player".into(), Vec::new())),
            Ent::Mob(id) => {
                let kind = self
                    .mob_ref(id)
                    .map(|o| o.kind)
                    .or_else(|| self.mobs.killer.as_ref().filter(|k| k.2 == id).map(|k| k.1))?;
                let comps =
                    self.mobs.brainy.killer.as_ref().filter(|k| k.0 == id).map(|k| k.1.clone()).unwrap_or_default();
                Some((kind.info().name.to_owned(), comps))
            }
        }
    }

    /// Right click on a mob of the family; `Some(r)`: handled.
    pub(crate) fn bz_brainy_interact(&mut self, m: &mut Mob, slot: usize, item: &str) -> Option<bool> {
        match m.kind {
            Kind::Goat => goat::interact(self, m, slot, item),
            Kind::Tadpole => frog::tadpole_interact(self, m, slot, item),
            Kind::Armadillo => armadillo::interact(self, m, slot, item),
            Kind::Axolotl => axolotl::interact(self, m, slot, item),
            Kind::Allay => allay::interact(self, m, slot, item),
            _ => self.bz_animal_interact(m, slot, item),
        }
    }

    /// The player's interaction with an animal (feed, grow up) then golden dandelion.
    pub(crate) fn bz_animal_interact(&mut self, m: &mut Mob, slot: usize, item: &str) -> Option<bool> {
        if let Some(tag) = food_tag(m.kind)
            && self.item_tag(item, tag)
        {
            let can_love = m.in_love <= 0 && !(m.kind == Kind::Armadillo && armadillo::is_scared(m));
            if m.age == 0 && can_love {
                self.bz_use_player_item(m, slot, item);
                m.in_love = 600;
                m.love_cause = true;
                self.mob_event(m, 18);
                eating_sound(self, m);
                return Some(true);
            }
            let can_age = m.is_baby() && !m.mount.age_locked;
            if can_age {
                self.bz_use_player_item(m, slot, item);
                let secs = super::mounts::feeding_secs(-m.age);
                super::mounts::age_up(m, secs, true);
                eating_sound(self, m);
                return Some(true);
            }
        }
        if item == "minecraft:golden_dandelion" && can_be_baby(m.kind) && m.is_baby() && m.bm.lock_timer == 0 {
            m.mount.age_locked = !m.mount.age_locked;
            m.age = baby_start_age(m.kind);
            m.bm.lock_timer = 40;
            self.consume_held(slot);
            if m.mount.age_locked {
                m.persistent = true;
            }
            let snd = if m.mount.age_locked {
                "minecraft:item.golden_dandelion.use"
            } else {
                "minecraft:item.golden_dandelion.unuse"
            };
            let p = m.block_pos().map(|v| f64::from(v) + 0.5);
            self.world_sound(snd, 7, p, 1.0, 1.0);
            return Some(true);
        }
        None
    }

    /// Consumption of the player's item (axolotl: tropical fish bucket returned as a water bucket).
    fn bz_use_player_item(&mut self, m: &Mob, slot: usize, item: &str) {
        if m.kind == Kind::Axolotl && item == "minecraft:tropical_fish_bucket" {
            if self.game_mode != 1 {
                self.slots[slot] = Some(Stack { id: "minecraft:water_bucket".into(), count: 1, components: None });
            }
            return;
        }
        self.consume_held(slot);
    }
}

/// Eating sound.
fn eating_sound(s: &mut Server, m: &mut Mob) {
    let pitch = |s: &mut Server| {
        let r = s.mobs.rng.next_float();
        0.8 + r * (1.2 - 0.8)
    };
    match m.kind {
        Kind::Goat => {
            let p = pitch(s);
            let snd = if m.bm.screaming { "minecraft:entity.goat.screaming.eat" } else { "minecraft:entity.goat.eat" };
            s.entity_sound(m.id, snd, 6, 1.0, p);
        }
        Kind::Sniffer => {
            let p = pitch(s);
            s.entity_sound(m.id, "minecraft:entity.sniffer.eat", 6, 1.0, p);
        }
        Kind::Armadillo => s.make_sound(m, "minecraft:entity.armadillo.eat"),
        Kind::Frog => s.entity_sound(m.id, "minecraft:entity.frog.eat", 6, 2.0, 1.0),
        _ => {}
    }
}
