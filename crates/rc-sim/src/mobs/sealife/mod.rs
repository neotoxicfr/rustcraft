//! Aquatic fauna and nautiluses (family F09, `server-mobs.md` § 4.F09): cod, salmon,
//! pufferfish, tropical fish, squid, glow squid, dolphin, nautilus and zombie nautilus.
//! Info sheets, own state, metadata, NBT, spawning in water and dispatch of the hooks
//! called by the common engine (`mobs/`).

pub(crate) mod dolphin;
pub(crate) mod fish;
pub(crate) mod goals;
pub(crate) mod nautilus;
pub(crate) mod squid;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod world_tests;

use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::env::Fluid;
use super::mob::{Ent, Mob};
use super::species::{Category, Info, Kind, info as base_info};
use crate::nbtio::{self, int};
use crate::server::Server;

// ---------- Info sheets ----------

const fn water(name: &'static str, cat: Category, w: f32, h: f32, eye: f32, health: f64) -> Info {
    let mut i = base_info(name, cat, w, h, eye);
    i.health = health;
    i.ambient_interval = 120;
    i
}

const fn fish_info(name: &'static str, w: f32, h: f32, eye: f32, sounds: [&'static str; 3]) -> Info {
    let mut i = water(name, Category::WaterAmbient, w, h, eye, 3.0);
    i.tracking = 4;
    i.ambient = if sounds[0].is_empty() { None } else { Some(sounds[0]) };
    i.hurt = sounds[1];
    i.death = sounds[2];
    i
}

static COD: Info =
    fish_info("minecraft:cod", 0.5, 0.3, 0.195, ["minecraft:entity.cod.ambient", "minecraft:entity.cod.hurt", "minecraft:entity.cod.death"]);

static SALMON: Info = fish_info(
    "minecraft:salmon",
    0.7,
    0.4,
    0.26,
    ["minecraft:entity.salmon.ambient", "minecraft:entity.salmon.hurt", "minecraft:entity.salmon.death"],
);

/// The pufferfish has no ambient sound.
static PUFFERFISH: Info =
    fish_info("minecraft:pufferfish", 0.7, 0.7, 0.455, ["", "minecraft:entity.puffer_fish.hurt", "minecraft:entity.puffer_fish.death"]);

static TROPICAL_FISH: Info = fish_info(
    "minecraft:tropical_fish",
    0.5,
    0.4,
    0.26,
    ["minecraft:entity.tropical_fish.ambient", "minecraft:entity.tropical_fish.hurt", "minecraft:entity.tropical_fish.death"],
);

static SQUID: Info = {
    let mut i = water("minecraft:squid", Category::WaterCreature, 0.8, 0.8, 0.4, 10.0);
    i.tracking = 8;
    i.volume = 0.4;
    i.ambient = Some("minecraft:entity.squid.ambient");
    i.hurt = "minecraft:entity.squid.hurt";
    i.death = "minecraft:entity.squid.death";
    i
};

static GLOW_SQUID: Info = {
    let mut i = water("minecraft:glow_squid", Category::UndergroundWaterCreature, 0.8, 0.8, 0.4, 10.0);
    i.tracking = 10;
    i.volume = 0.4;
    i.ambient = Some("minecraft:entity.glow_squid.ambient");
    i.hurt = "minecraft:entity.glow_squid.hurt";
    i.death = "minecraft:entity.glow_squid.death";
    i
};

static DOLPHIN: Info = {
    let mut i = water("minecraft:dolphin", Category::WaterCreature, 0.9, 0.6, 0.3, 10.0);
    i.speed = 1.2f32 as f64;
    i.attack = 3.0;
    i.tracking = 5;
    i.ambient = Some("minecraft:entity.dolphin.ambient");
    i.hurt = "minecraft:entity.dolphin.hurt";
    i.death = "minecraft:entity.dolphin.death";
    i
};

static NAUTILUS: Info = {
    let mut i = water("minecraft:nautilus", Category::WaterCreature, 0.875, 0.95, 0.2751, 15.0);
    i.speed = 1.0;
    i.attack = 3.0;
    i.ambient = Some("minecraft:entity.nautilus.ambient");
    i.hurt = "minecraft:entity.nautilus.hurt";
    i.death = "minecraft:entity.nautilus.death";
    i
};

/// Zombie nautilus: monster category, animal class (neutral sounds, stays in peaceful), burns
/// in the sun (body armor as protection).
static ZOMBIE_NAUTILUS: Info = {
    let mut i = water("minecraft:zombie_nautilus", Category::Monster, 0.875, 0.95, 0.2751, 15.0);
    i.speed = 1.1f32 as f64;
    i.attack = 3.0;
    i.burns = true;
    i.ambient = Some("minecraft:entity.zombie_nautilus.ambient");
    i.hurt = "minecraft:entity.zombie_nautilus.hurt";
    i.death = "minecraft:entity.zombie_nautilus.death";
    i
};

pub fn info(k: Kind) -> &'static Info {
    match k {
        Kind::Cod => &COD,
        Kind::Salmon => &SALMON,
        Kind::Pufferfish => &PUFFERFISH,
        Kind::TropicalFish => &TROPICAL_FISH,
        Kind::Squid => &SQUID,
        Kind::GlowSquid => &GLOW_SQUID,
        Kind::Dolphin => &DOLPHIN,
        Kind::Nautilus => &NAUTILUS,
        _ => &ZOMBIE_NAUTILUS,
    }
}

/// Species of the family.
pub fn handles(k: Kind) -> bool {
    matches!(
        k,
        Kind::Cod
            | Kind::Salmon
            | Kind::Pufferfish
            | Kind::TropicalFish
            | Kind::Squid
            | Kind::GlowSquid
            | Kind::Dolphin
            | Kind::Nautilus
            | Kind::ZombieNautilus
    )
}

/// Fish.
pub fn is_fish(k: Kind) -> bool {
    matches!(k, Kind::Cod | Kind::Salmon | Kind::Pufferfish | Kind::TropicalFish)
}

/// Schooling fish.
pub fn is_schooling(k: Kind) -> bool {
    matches!(k, Kind::Cod | Kind::Salmon | Kind::TropicalFish)
}

pub fn is_squid(k: Kind) -> bool {
    matches!(k, Kind::Squid | Kind::GlowSquid)
}

/// Nautiluses (tameable animals).
pub fn is_nautilus(k: Kind) -> bool {
    matches!(k, Kind::Nautilus | Kind::ZombieNautilus)
}

/// Aged aquatic mobs: squids and dolphin.
pub fn ageable_water(k: Kind) -> bool {
    matches!(k, Kind::Squid | Kind::GlowSquid | Kind::Dolphin)
}

/// Tag `can_breathe_under_water`: all except the dolphin.
pub fn breathes_underwater(k: Kind) -> bool {
    handles(k) && k != Kind::Dolphin
}

/// Baby dimensions.
pub fn baby_dims(k: Kind) -> (f32, f32, f32) {
    match k {
        Kind::Squid | Kind::GlowSquid => (0.5, 0.5, 0.37),
        Kind::Dolphin => (0.9 * 0.65, 0.6 * 0.65, 0.093_75),
        _ => (0.875 * 0.5, 0.95 * 0.5, 0.2751 * 0.5),
    }
}

/// Dimension scale: pufferfish puffing, salmon size.
pub fn dim_scale(m: &Mob) -> f32 {
    match m.kind {
        Kind::Pufferfish => fish::puff_scale(m.sea.puff),
        Kind::Salmon => fish::salmon_scale(m.variant),
        _ => 1.0,
    }
}

/// Maximum air (dolphin: 4,800).
pub fn max_air(m: &Mob) -> i32 {
    if m.kind == Kind::Dolphin { dolphin::MAX_AIR } else { 300 }
}

/// Air regained out of water: the dolphin refills at once.
pub fn increase_air(m: &Mob) -> i32 {
    if m.kind == Kind::Dolphin { dolphin::MAX_AIR } else { (m.air + 4).min(max_air(m)) }
}

// ---------- Own state ----------

/// State of the family's species (fields unused for the others).
#[derive(Debug, Clone)]
pub struct Aqua {
    /// Fish taken out of a bucket (persistent, does not despawn).
    pub from_bucket: bool,
    // Pufferfish.
    pub puff: i32,
    pub inflate: i32,
    pub deflate: i32,
    // Schools.
    pub leader: Option<i32>,
    pub school: i32,
    /// Tropical fish outside a school.
    pub not_school: bool,
    // Squids.
    pub squid: squid::SquidState,
    pub dark: i32,
    // Dolphin.
    pub got_fish: bool,
    pub moist: i32,
    pub treasure: Option<[i32; 3]>,
    // Nautiluses.
    pub brain: Option<Box<nautilus::Brain>>,
    /// Return area of a tamed nautilus (center, radius), read by the position draws.
    pub home: Option<([i32; 3], i32)>,
    pub jump_pending: f32,
    /// Zombie nautilus: variant name (registry), read back at construction.
    pub variant_name: Option<String>,
}

/// World state specific to the family: player swimming.
#[derive(Debug, Clone, Default)]
pub struct World {
    pub player_swimming: bool,
}

impl Default for Aqua {
    fn default() -> Aqua {
        Aqua {
            from_bucket: false,
            puff: 0,
            inflate: 0,
            deflate: 0,
            leader: None,
            school: 1,
            not_school: false,
            squid: squid::SquidState::default(),
            dark: 0,
            got_fish: false,
            moist: dolphin::MAX_MOIST,
            treasure: None,
            brain: None,
            home: None,
            jump_pending: 0.0,
            variant_name: None,
        }
    }
}

/// Constructor (`new`): entity random draws, own attributes.
pub fn on_new(m: &mut Mob) {
    match m.kind {
        Kind::Salmon => m.variant = fish::SALMON_MEDIUM,
        Kind::Squid | Kind::GlowSquid => squid::on_new(m),
        Kind::Dolphin => {
            // The loot pickup setting.
            m.can_pickup = true;
        }
        Kind::Nautilus | Kind::ZombieNautilus => nautilus::on_new(m),
        _ => {}
    }
}

// ---------- Metadata ----------

/// Own values (after those of the mob, indices 0 to 15).
pub fn define_data(m: &mut Mob) {
    match m.kind {
        k if is_fish(k) => {
            m.define(16, Meta::Bool(false));
            match k {
                Kind::Pufferfish => m.define(17, Meta::Int(0)),
                Kind::Salmon => m.define(17, Meta::Int(fish::SALMON_MEDIUM)),
                Kind::TropicalFish => m.define(17, Meta::Int(0)),
                _ => {}
            }
        }
        _ => {
            // Age: baby, age lock.
            m.define(16, Meta::Bool(false));
            m.define(17, Meta::Bool(false));
            match m.kind {
                Kind::GlowSquid => m.define(18, Meta::Int(0)),
                Kind::Dolphin => {
                    m.define(18, Meta::Bool(false));
                    m.define(19, Meta::Int(dolphin::MAX_MOIST));
                }
                Kind::Nautilus | Kind::ZombieNautilus => {
                    m.define(18, Meta::Byte(0));
                    m.define(19, Meta::Entity(None));
                    m.define(20, Meta::Bool(false));
                    if m.kind == Kind::ZombieNautilus {
                        m.define(21, Meta::Variant(0, nautilus::VARIANT_SERIALIZER));
                    }
                }
                _ => {}
            }
        }
    }
}

pub fn sync_data(m: &mut Mob) {
    if is_fish(m.kind) {
        m.set(16, Meta::Bool(m.sea.from_bucket));
        match m.kind {
            Kind::Pufferfish => m.set(17, Meta::Int(m.sea.puff)),
            Kind::Salmon | Kind::TropicalFish => m.set(17, Meta::Int(m.variant)),
            _ => {}
        }
        return;
    }
    m.set(16, Meta::Bool(m.is_baby()));
    m.set(17, Meta::Bool(m.mount.age_locked));
    match m.kind {
        Kind::GlowSquid => m.set(18, Meta::Int(m.sea.dark)),
        Kind::Dolphin => {
            m.set(18, Meta::Bool(m.sea.got_fish));
            m.set(19, Meta::Int(m.sea.moist));
        }
        Kind::Nautilus | Kind::ZombieNautilus => {
            let f = i8::from(m.sitting) | if m.tame { 4 } else { 0 };
            m.set(18, Meta::Byte(f));
            m.set(19, Meta::Entity(m.owner));
            m.set(20, Meta::Bool(m.mount.dash));
            if m.kind == Kind::ZombieNautilus {
                m.set(21, Meta::Variant(m.variant, nautilus::VARIANT_SERIALIZER));
            }
        }
        _ => {}
    }
}

// ---------- NBT ----------

pub fn load(m: &mut Mob, e: &Nbt) {
    let flag = |k: &str| int(e, k).is_some_and(|v| v != 0);
    match m.kind {
        k if is_fish(k) => {
            m.sea.from_bucket = flag("FromBucket");
            match k {
                Kind::Pufferfish => m.sea.puff = int(e, "PuffState").map_or(0, |v| (v as i32).min(2)),
                Kind::Salmon => {
                    m.variant = match e.get("type").and_then(Nbt::as_str) {
                        Some("small") => 0,
                        Some("large") => 2,
                        _ => fish::SALMON_MEDIUM,
                    };
                }
                Kind::TropicalFish => m.variant = int(e, "Variant").map_or(0, |v| v as i32),
                _ => {}
            }
        }
        _ => {
            if m.kind != Kind::ZombieNautilus {
                m.mount.age_locked = flag("AgeLocked");
            }
            match m.kind {
                Kind::GlowSquid => m.sea.dark = int(e, "DarkTicksRemaining").unwrap_or(0) as i32,
                Kind::Dolphin => {
                    m.sea.got_fish = flag("GotFish");
                    m.sea.moist = int(e, "Moistness").map_or(dolphin::MAX_MOIST, |v| v as i32);
                }
                Kind::Nautilus | Kind::ZombieNautilus => nautilus::load(m, e),
                _ => {}
            }
        }
    }
}

pub fn save(m: &Mob, e: &mut Nbt) {
    let set = |e: &mut Nbt, k: &str, v: Nbt| nbtio::set(e, k, v);
    match m.kind {
        k if is_fish(k) => {
            set(e, "FromBucket", Nbt::Byte(i8::from(m.sea.from_bucket)));
            match k {
                Kind::Pufferfish => set(e, "PuffState", Nbt::Int(m.sea.puff)),
                Kind::Salmon => set(e, "type", Nbt::String(fish::salmon_name(m.variant).into())),
                Kind::TropicalFish => set(e, "Variant", Nbt::Int(m.variant)),
                _ => {}
            }
        }
        _ => {
            if m.kind == Kind::ZombieNautilus {
                // The "can be a baby" test false: neither age nor lock saved.
                for k in ["Age", "ForcedAge", "AgeLocked"] {
                    nbtio::remove(e, k);
                }
            } else {
                set(e, "AgeLocked", Nbt::Byte(i8::from(m.mount.age_locked)));
            }
            // Love only exists for animals (nautiluses).
            if !is_nautilus(m.kind) {
                nbtio::remove(e, "InLove");
            }
            match m.kind {
                Kind::GlowSquid => set(e, "DarkTicksRemaining", Nbt::Int(m.sea.dark)),
                Kind::Dolphin => {
                    set(e, "GotFish", Nbt::Byte(i8::from(m.sea.got_fish)));
                    set(e, "Moistness", Nbt::Int(m.sea.moist));
                }
                Kind::Nautilus | Kind::ZombieNautilus => nautilus::save(m, e),
                _ => {}
            }
        }
    }
}

// ---------- Goals and navigation ----------

/// Goals of each species, in the game's insertion order.
pub fn register_goals(m: &mut Mob) {
    match m.kind {
        k if is_fish(k) => fish::register(m),
        Kind::Squid | Kind::GlowSquid => squid::register(m),
        Kind::Dolphin => dolphin::register(m),
        _ => {}
    }
}

/// Server-side construction: swimming navigation, zombie nautilus variant.
pub fn init(s: &Server, m: &mut Mob) {
    if !handles(m.kind) {
        return;
    }
    init_nav(m);
    nautilus::init(s, m);
}

/// Swimming navigation; breach allowed for the dolphin.
pub fn init_nav(m: &mut Mob) {
    if !handles(m.kind) {
        return;
    }
    m.nav.swim = true;
    m.nav.breach = m.kind == Kind::Dolphin;
    m.nav.can_open_doors = false;
    m.nav.can_float = false;
}

// ---------- Sounds ----------

/// Eyes in water and in water.
pub fn under_water(m: &Mob) -> bool {
    m.eyes_in_water && m.in_water
}

/// Own ambient sound; `None`: silence.
pub fn ambient_sound(m: &Mob) -> Option<String> {
    let under = under_water(m);
    let land = |s: &str| if under { s.to_owned() } else { format!("{s}_land") };
    match m.kind {
        Kind::Dolphin => Some(
            if m.in_water { "minecraft:entity.dolphin.ambient_water" } else { "minecraft:entity.dolphin.ambient" }.into(),
        ),
        Kind::Nautilus => Some(land(if m.is_baby() {
            "minecraft:entity.baby_nautilus.ambient"
        } else {
            "minecraft:entity.nautilus.ambient"
        })),
        Kind::ZombieNautilus => Some(land("minecraft:entity.zombie_nautilus.ambient")),
        k => info(k).ambient.map(Into::into),
    }
}

/// Own hurt / death (nautiluses out of water, baby nautiluses).
pub fn hurt_sound(m: &Mob, death: bool) -> Option<&'static str> {
    let under = under_water(m);
    Some(match (m.kind, m.is_baby(), death, under) {
        (Kind::Nautilus, true, false, true) => "minecraft:entity.baby_nautilus.hurt",
        (Kind::Nautilus, true, false, false) => "minecraft:entity.baby_nautilus.hurt_land",
        (Kind::Nautilus, true, true, true) => "minecraft:entity.baby_nautilus.death",
        (Kind::Nautilus, true, true, false) => "minecraft:entity.baby_nautilus.death_land",
        (Kind::Nautilus, false, false, true) => "minecraft:entity.nautilus.hurt",
        (Kind::Nautilus, false, false, false) => "minecraft:entity.nautilus.hurt_land",
        (Kind::Nautilus, false, true, true) => "minecraft:entity.nautilus.death",
        (Kind::Nautilus, false, true, false) => "minecraft:entity.nautilus.death_land",
        (Kind::ZombieNautilus, _, false, true) => "minecraft:entity.zombie_nautilus.hurt",
        (Kind::ZombieNautilus, _, false, false) => "minecraft:entity.zombie_nautilus.hurt_land",
        (Kind::ZombieNautilus, _, true, true) => "minecraft:entity.zombie_nautilus.death",
        (Kind::ZombieNautilus, _, true, false) => "minecraft:entity.zombie_nautilus.death_land",
        _ => return None,
    })
}

/// Swim sound.
pub fn swim_sound(m: &Mob) -> &'static str {
    match m.kind {
        k if is_fish(k) => "minecraft:entity.fish.swim",
        Kind::Dolphin => "minecraft:entity.dolphin.swim",
        Kind::Nautilus if m.is_baby() => "minecraft:entity.baby_nautilus.swim",
        Kind::Nautilus => "minecraft:entity.nautilus.swim",
        Kind::ZombieNautilus => "minecraft:entity.zombie_nautilus.swim",
        _ => "minecraft:entity.generic.swim",
    }
}

/// Sound source: neutral for the whole family (none is a "Monster" of the game).
pub fn sound_source(m: &Mob) -> Option<i32> {
    handles(m.kind).then_some(6)
}

// ---------- Spawning ----------

/// "In water" placement: water in the cell, nothing conductive
/// above; `None`: common ground placement.
pub fn placement_ok(s: &Server, k: Kind, p: [i32; 3]) -> Option<bool> {
    if !handles(k) {
        return None;
    }
    let above = [p[0], p[1] + 1, p[2]];
    Some(s.mob_fluid(p) == Fluid::Water && !s.v().conductor(s.st(above)))
}

/// Own spawn rule; `None`: common rule.
pub fn spawn_rules(s: &mut Server, k: Kind, p: [i32; 3]) -> Option<bool> {
    if !handles(k) {
        return None;
    }
    let sea = super::surface::sea_level(s);
    let below_water = s.mob_fluid([p[0], p[1] - 1, p[2]]) == Fluid::Water;
    let above_water = s.bname([p[0], p[1] + 1, p[2]]) == "minecraft:water";
    // Surface water: 13 blocks below sea level at most.
    let surface = |p: [i32; 3]| p[1] >= sea - 13 && p[1] <= sea && below_water && above_water;
    Some(match k {
        Kind::TropicalFish => {
            below_water
                && above_water
                && (s.biome_tag(p, "minecraft:allows_tropical_fish_spawns_at_any_height") || surface(p))
        }
        Kind::GlowSquid => {
            let (sky, block) = s.light(p);
            p[1] <= sea - 33 && sky.max(block) == 0 && s.bname(p) == "minecraft:water"
        }
        Kind::Nautilus | Kind::ZombieNautilus => {
            p[1] >= sea - 25 && p[1] <= sea - 5 && below_water && above_water
        }
        _ => surface(p),
    })
}

/// Obstruction (the spawn obstruction test = the no-obstacle test): no entity in the box.
pub fn obstruction_ok(s: &Server, m: &Mob) -> Option<bool> {
    if !handles(m.kind) {
        return None;
    }
    Some(s.unobstructed(&m.bbox(), m.id))
}

/// Maximum size of a spawn group.
pub fn max_cluster(k: Kind) -> Option<i32> {
    Some(match k {
        Kind::Salmon => 5,
        k if is_fish(k) => 8,
        _ => return None,
    })
}

/// The tropical fish outside a school stops the group.
pub fn max_group_reached(m: &Mob, _size: i32) -> bool {
    m.kind == Kind::TropicalFish && m.sea.not_school
}

/// Despawning far from players.
pub fn remove_when_far_away(m: &Mob) -> Option<bool> {
    Some(match m.kind {
        k if is_fish(k) => !m.sea.from_bucket && m.custom_name.is_none(),
        Kind::Nautilus | Kind::ZombieNautilus => true,
        k if handles(k) => m.custom_name.is_none(),
        _ => return None,
    })
}

/// Own persistence: bucket fish, tamed nautilus.
pub fn requires_persistence(m: &Mob) -> bool {
    (is_fish(m.kind) && m.sea.from_bucket) || (is_nautilus(m.kind) && m.tame)
}

/// Finalization before the mob's (overridden, "before" part).
pub fn finalize_pre(s: &mut Server, m: &mut Mob, group: &mut super::spawn::Group) {
    match m.kind {
        // Size drawn on the entity random (small 30, medium 50, large 15).
        Kind::Salmon => m.variant = fish::random_salmon_size(&mut m.rng),
        Kind::Dolphin => {
            m.air = dolphin::MAX_AIR;
            m.pitch = 0.0;
            if !group.ageable {
                group.baby_chance = Some(0.1);
            }
        }
        Kind::Nautilus | Kind::ZombieNautilus => {
            if m.kind == Kind::ZombieNautilus {
                nautilus::select_variant(s, m);
            }
            nautilus::init_memories(s, m);
        }
        _ => {}
    }
}

/// Finalization after the mob's: schools, tropical fish patterns.
pub fn finalize_post(s: &mut Server, m: &mut Mob, group: &mut super::spawn::Group) {
    if is_schooling(m.kind) {
        fish::school_finalize(s, m, group);
    }
    if m.kind == Kind::TropicalFish {
        fish::tropical_finalize(s, m, group);
    }
}

// ---------- Tick hooks ----------

/// Before the common tick (`tick` overridden before `super.tick`): pufferfish puffing.
pub fn pre_tick(s: &mut Server, m: &mut Mob) {
    if m.kind == Kind::Pufferfish {
        fish::puff_tick(s, m);
    }
}

/// Air after the base tick: out of water, air drops and the mob suffocates; in water, it
/// returns to 300. The dolphin does not have this rule.
pub fn handle_air(s: &mut Server, m: &mut Mob, before: i32) {
    // The zombie nautilus does not have this rule (only the nautilus overrides it).
    if !handles(m.kind) || m.kind == Kind::Dolphin || m.kind == Kind::ZombieNautilus {
        return;
    }
    // Nautilus: only with AI, death by drying out at −20.
    let naut = m.kind == Kind::Nautilus;
    if naut && m.no_ai {
        return;
    }
    if m.alive() && !m.in_water {
        m.air = before - 1;
        let drown = if naut { m.air <= -20 } else { s.should_take_drowning_damage(m) };
        if drown {
            m.air = 0;
            let ty = if naut { "minecraft:dry_out" } else { "minecraft:drown" };
            s.mob_hurt(m, ty, 2.0, None, None);
        }
    } else {
        m.air = 300;
    }
}

/// Start of the AI step: fish jump out of water.
pub fn pre_ai_step(s: &mut Server, m: &mut Mob) {
    if is_fish(m.kind) {
        fish::flop(s, m);
    }
}

/// End of the AI step: pufferfish stings, squid movement, darkening,
/// dolphin pickup.
pub fn post_ai_step(s: &mut Server, m: &mut Mob) {
    match m.kind {
        Kind::Pufferfish => fish::puff_touch(s, m),
        Kind::Squid | Kind::GlowSquid => squid::post_ai_step(s, m),
        Kind::Dolphin => dolphin::pickup(s, m),
        _ => {}
    }
}

/// Brain of the nautiluses.
pub fn custom_ai_step(s: &mut Server, m: &mut Mob) {
    if is_nautilus(m.kind) {
        s.nautilus_brain_tick(m);
    }
}

/// End of the tick: schools, dolphin moisture, dash and effects of the nautiluses.
pub fn post_tick(s: &mut Server, m: &mut Mob) {
    match m.kind {
        k if is_schooling(k) => fish::school_tick(s, m),
        Kind::Dolphin => dolphin::post_tick(s, m),
        Kind::Nautilus | Kind::ZombieNautilus => nautilus::post_tick(s, m),
        _ => {}
    }
}

/// Own movement control; true if it replaces the common one.
pub fn move_control(m: &mut Mob) -> bool {
    match m.kind {
        k if is_fish(k) => {
            fish::move_control(m);
            true
        }
        // Squids have no active movement control (no goal uses it).
        Kind::Squid | Kind::GlowSquid => true,
        Kind::Dolphin => {
            dolphin::smooth_move_control(m, 0.02, 0.1);
            true
        }
        Kind::Nautilus | Kind::ZombieNautilus => {
            dolphin::smooth_move_control(m, 0.011, 0.0);
            true
        }
        _ => false,
    }
}

/// Own look control; true if it replaces the common one.
pub fn look_control(m: &mut Mob) -> bool {
    if m.kind == Kind::Dolphin || is_nautilus(m.kind) {
        dolphin::smooth_look_control(m);
        return true;
    }
    false
}

/// Own movement; true if it replaces the common one (squids always, swimmers in water).
pub fn travel(s: &mut Server, m: &mut Mob) -> bool {
    if is_squid(m.kind) {
        let v = m.vel;
        s.mob_move(m, v);
        return true;
    }
    if !handles(m.kind) || !m.in_water {
        return false;
    }
    let speed = if is_fish(m.kind) { 0.01 } else { m.speed };
    s.move_relative_pub(m, speed);
    let v = m.vel;
    s.mob_move(m, v);
    m.vel = m.vel.map(|x| x * 0.9);
    // Nautiluses do not sink; fish and dolphin sink gently without a target.
    if !is_nautilus(m.kind) && m.target.is_none() {
        m.vel[1] -= 0.005;
    }
    true
}

/// Step in water: swimming; true if handled.
pub fn swim_step(s: &mut Server, m: &mut Mob) -> bool {
    if !handles(m.kind) || m.on_ground || !m.in_water {
        return false;
    }
    m.next_step = m.move_dist.trunc() + 1.0;
    // Squids only emit events, no sound.
    if is_squid(m.kind) {
        return true;
    }
    let v = m.vel;
    let speed = ((v[0] * v[0] * f64::from(0.2f32) + v[1] * v[1] + v[2] * v[2] * f64::from(0.2f32)).sqrt() as f32 * 0.35)
        .min(1.0);
    let pitch = 1.0 + (m.rng.next_float() - m.rng.next_float()) * 0.4;
    let snd = swim_sound(m);
    s.mob_sound(m, snd, speed, pitch);
    true
}

// ---------- Combat ----------

/// Reactions to damage dealt: ink, darkening, nautilus anger.
pub fn on_hurt(s: &mut Server, m: &mut Mob, by: Option<Ent>) {
    match m.kind {
        Kind::Squid | Kind::GlowSquid => {
            if m.last_hurt_by_mob.is_some() {
                squid::spawn_ink(s, m);
            }
            if m.kind == Kind::GlowSquid {
                m.sea.dark = 100;
            }
        }
        Kind::Nautilus | Kind::ZombieNautilus => {
            if let Some(b) = by {
                nautilus::set_anger_target(s, m, b);
            }
        }
        _ => {}
    }
    if is_nautilus(m.kind) {
        m.reset_love();
    }
}

/// Can attack: the baby dolphin cannot.
pub fn can_attack(m: &Mob) -> bool {
    !(m.kind == Kind::Dolphin && m.is_baby())
}

/// Experience: 1 to 3, without equipment bonus.
pub fn xp(m: &mut Mob) -> Option<i32> {
    handles(m.kind).then(|| 1 + m.rng.next_int_bound(3))
}

// ---------- Interactions ----------

/// Own right click; `Some(r)`: handled.
pub fn interact(s: &mut Server, m: &mut Mob, slot: usize, item: &str) -> Option<bool> {
    match m.kind {
        k if is_fish(k) => fish::bucket_pickup(s, m, slot, item),
        Kind::Dolphin => dolphin::interact(s, m, slot, item),
        Kind::Nautilus | Kind::ZombieNautilus => s.nautilus_interact(m, slot, item),
        _ => None,
    }
}

impl Server {
    /// Player swimming, the server's only rule (read by dolphins, steps and predicates),
    /// recomputed once per tick like the game: sprint in water, outside a vehicle; to
    /// start, eyes and feet in water; never in ability flight (the spectator always
    /// flies). State specific to each player, exchanged by `swap_player`.
    pub(crate) fn sealife_player_tick(&mut self) {
        let sprint = self.life.vit.sprinting;
        let riding = self.player_riding_any();
        let feet = super::mth::block_of(self.pos);
        let eye = super::mth::block_of([self.pos[0], self.player_eye_y(), self.pos[2]]);
        let in_water = self.player_in_water();
        let under = self.mob_fluid(eye) == Fluid::Water;
        let feet_water = self.mob_fluid(feet) == Fluid::Water;
        let no_swim = self.flying || self.game_mode == 3;
        let w = &mut self.mobs.sea;
        w.player_swimming = if no_swim {
            false
        } else if w.player_swimming {
            sprint && in_water && !riding
        } else {
            sprint && under && !riding && feet_water
        };
    }

    /// Air at −20 or less (breathing ignored).
    pub(crate) fn should_take_drowning_damage(&self, m: &Mob) -> bool {
        m.air <= -20
    }

    /// Relative acceleration exposed to the families.
    pub(crate) fn move_relative_pub(&self, m: &mut Mob, speed: f32) {
        let input = [f64::from(m.xxa), f64::from(m.yya), f64::from(m.zza)];
        let len = input[0] * input[0] + input[1] * input[1] + input[2] * input[2];
        if len < 1.0e-7 {
            return;
        }
        let v = if len > 1.0 { input.map(|x| x / len.sqrt()) } else { input };
        let v = v.map(|x| x * f64::from(speed));
        let a = m.yaw * super::mth::DEG;
        let (sn, cs) = (super::mth::sin(a), super::mth::cos(a));
        m.vel[0] += v[0] * f64::from(cs) - v[2] * f64::from(sn);
        m.vel[1] += v[1];
        m.vel[2] += v[2] * f64::from(cs) + v[0] * f64::from(sn);
    }
}
