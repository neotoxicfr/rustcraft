//! Simple surface animals (family F02, `server-mobs.md` § 4.1 and § 4.3): rabbit,
//! mooshroom, bat, ocelot, polar bear, turtle, parrot. Info sheets, own state,
//! metadata, NBT and dispatch of the hooks called by the common engine (`mobs/`).

pub(crate) mod bat;
pub(crate) mod mooshroom;
pub(crate) mod ocelot;
pub(crate) mod parrot;
pub(crate) mod polar;
pub(crate) mod rabbit;
pub(crate) mod turtle;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod world_tests;

use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::goals::{Food, Goal, Melee, TBase, Who};
use super::mob::Mob;
use super::species::{Category, Info, Kind, animal, info as base_info};
use crate::nbtio::{self, int};
use crate::server::Server;

// ---------- Info sheets ----------

static RABBIT: Info = {
    let mut i = animal("minecraft:rabbit", 0.49, 0.6, 0.59, 3.0, 0.3f32 as f64);
    i.attack = 3.0;
    i.tracking = 8;
    i.ambient = Some("minecraft:entity.rabbit.ambient");
    i.hurt = "minecraft:entity.rabbit.hurt";
    i.death = "minecraft:entity.rabbit.death";
    i
};

static MOOSHROOM: Info = {
    let mut i = animal("minecraft:mooshroom", 0.9, 1.4, 1.3, 10.0, 0.2f32 as f64);
    i.ambient = Some("minecraft:entity.cow.ambient");
    i.hurt = "minecraft:entity.cow.hurt";
    i.death = "minecraft:entity.cow.death";
    i.step_sound = Some("minecraft:entity.cow.step");
    i.volume = 0.4;
    i
};

static BAT: Info = {
    let mut i = base_info("minecraft:bat", Category::Ambient, 0.5, 0.9, 0.45);
    i.health = 6.0;
    i.tracking = 5;
    i.ambient = Some("minecraft:entity.bat.ambient");
    i.hurt = "minecraft:entity.bat.hurt";
    i.death = "minecraft:entity.bat.death";
    i.volume = 0.1;
    // The fall damage control neutralized: never any fall damage.
    i.fall_immune = true;
    i
};

static OCELOT: Info = {
    let mut i = animal("minecraft:ocelot", 0.6, 0.7, 0.7 * 0.85, 10.0, 0.3f32 as f64);
    i.attack = 3.0;
    i.ambient = Some("minecraft:entity.ocelot.ambient");
    i.hurt = "minecraft:entity.ocelot.hurt";
    i.death = "minecraft:entity.ocelot.death";
    i.ambient_interval = 900;
    i.fall_immune = true;
    i
};

static POLAR_BEAR: Info = {
    let mut i = animal("minecraft:polar_bear", 1.4, 1.4, 1.4 * 0.85, 30.0, 0.25);
    i.attack = 6.0;
    i.follow = 20.0;
    i.ambient = Some("minecraft:entity.polar_bear.ambient");
    i.hurt = "minecraft:entity.polar_bear.hurt";
    i.death = "minecraft:entity.polar_bear.death";
    i.step_sound = Some("minecraft:entity.polar_bear.step");
    i
};

static TURTLE: Info = {
    let mut i = animal("minecraft:turtle", 1.2, 0.4, 0.4 * 0.85, 30.0, 0.25);
    i.step = 1.0;
    i.ambient = Some("minecraft:entity.turtle.ambient_land");
    i.hurt = "minecraft:entity.turtle.hurt";
    i.death = "minecraft:entity.turtle.death";
    i.step_sound = Some("minecraft:entity.turtle.shamble");
    i.ambient_interval = 200;
    i
};

static PARROT: Info = {
    let mut i = animal("minecraft:parrot", 0.5, 0.9, 0.54, 6.0, 0.2f32 as f64);
    i.attack = 3.0;
    i.tracking = 8;
    i.ambient = Some("minecraft:entity.parrot.ambient");
    i.hurt = "minecraft:entity.parrot.hurt";
    i.death = "minecraft:entity.parrot.death";
    i.step_sound = Some("minecraft:entity.parrot.step");
    i.fall_immune = true;
    i
};

pub fn info(k: Kind) -> &'static Info {
    match k {
        Kind::Rabbit => &RABBIT,
        Kind::Mooshroom => &MOOSHROOM,
        Kind::Bat => &BAT,
        Kind::Ocelot => &OCELOT,
        Kind::PolarBear => &POLAR_BEAR,
        Kind::Turtle => &TURTLE,
        _ => &PARROT,
    }
}

/// Species of the family.
pub fn handles(k: Kind) -> bool {
    matches!(
        k,
        Kind::Rabbit | Kind::Mooshroom | Kind::Bat | Kind::Ocelot | Kind::PolarBear | Kind::Turtle | Kind::Parrot
    )
}

pub fn is_animal(k: Kind) -> bool {
    handles(k) && k != Kind::Bat
}

/// Breeding food: the bear and the parrot have none.
pub fn food_tag(k: Kind) -> Option<&'static str> {
    Some(match k {
        Kind::Rabbit => "minecraft:rabbit_food",
        Kind::Mooshroom => "minecraft:cow_food",
        Kind::Ocelot => "minecraft:ocelot_food",
        Kind::Turtle => "minecraft:turtle_food",
        _ => return None,
    })
}

/// Baby dimensions (width, height, eyes).
pub fn baby_dims(k: Kind) -> Option<(f32, f32, f32)> {
    Some(match k {
        Kind::Rabbit => (0.24, 0.4, 0.39),
        Kind::Mooshroom => (0.45, 0.7, 0.69),
        Kind::Ocelot => (0.3, 0.35, 0.343_75),
        Kind::PolarBear => (0.7, 0.7, 0.343_75),
        Kind::Turtle => (1.2 * 0.3, 0.4 * 0.3, 0.4 * 0.85 * 0.3),
        _ => return None,
    })
}

/// Sea level of the dimension (flat world: −63).
pub(crate) fn sea_level(s: &Server) -> i32 {
    match s.dim.name.as_str() {
        "minecraft:the_nether" => 32,
        "minecraft:the_end" => 0,
        _ if s.flat() => -63,
        _ => 63,
    }
}

// ---------- Own state ----------

/// State of the family's species (fields unused for the others).
#[derive(Debug, Clone, Default)]
pub struct Surf {
    // Rabbit.
    pub jump_ticks: i32,
    pub jump_duration: i32,
    pub was_on_ground: bool,
    pub jump_delay: i32,
    pub more_carrot: i32,
    pub can_jump: bool,
    pub next_jump_speed: f64,
    // Bat.
    pub bat_flags: i8,
    pub bat_target: Option<[i32; 3]>,
    // Mooshroom.
    pub stew: Option<Nbt>,
    pub last_bolt: Option<u128>,
    // Ocelot (and generic posture/sprint).
    pub trusting: bool,
    pub crouching: bool,
    pub sprinting: bool,
    // Polar bear.
    pub standing: bool,
    pub warning: i32,
    /// UUID of the persistent anger target (written as is on save).
    pub angry_at: Option<u128>,
    // Turtle.
    pub home: [i32; 3],
    pub travel: Option<[i32; 3]>,
    pub going_home: bool,
    pub has_egg: bool,
    pub laying: bool,
    pub lay_counter: i32,
    // Parrot.
    pub ride_cd: i32,
    pub flapping: f32,
    pub flap_speed: f32,
    pub next_flap: f32,
    pub fly_dist: f32,
    pub no_gravity: bool,
    /// Baby at the previous tick (turtle growing up).
    pub prev_baby: Option<bool>,
}

/// Constructor (`new`): fields initialized, entity random draws.
pub fn on_new(m: &mut Mob) {
    match m.kind {
        Kind::Rabbit => {
            // Rest animation delay (field initialized at construction: one draw).
            let _ = m.rng.next_int_bound(40) + 180;
            rabbit::set_speed_modifier(m, 0.0);
        }
        Kind::Bat => m.surf.bat_flags = 1,
        Kind::Parrot => {
            m.surf.flapping = 1.0;
            m.surf.next_flap = 1.0;
        }
        _ => {}
    }
}

// ---------- Metadata ----------

/// Own values (after those of the living entity and the mob, indices 0 to 15).
pub fn define_data(m: &mut Mob) {
    if m.kind == Kind::Bat {
        m.define(16, Meta::Byte(0));
        return;
    }
    m.define(16, Meta::Bool(false));
    m.define(17, Meta::Bool(false));
    match m.kind {
        Kind::Rabbit | Kind::Mooshroom => m.define(18, Meta::Int(0)),
        Kind::Ocelot | Kind::PolarBear => m.define(18, Meta::Bool(false)),
        Kind::Turtle => {
            m.define(18, Meta::Bool(false));
            m.define(19, Meta::Bool(false));
        }
        Kind::Parrot => {
            m.define(18, Meta::Byte(0));
            m.define(19, Meta::Entity(None));
            m.define(20, Meta::Int(0));
        }
        _ => {}
    }
}

pub fn sync_data(m: &mut Mob) {
    // Shared flags (ocelot sprint included) and no gravity: common tick. Crouching posture.
    if !m.dead {
        m.set(6, Meta::Pose(if m.surf.crouching { 5 } else { 0 }));
    }
    if m.kind == Kind::Bat {
        m.set(16, Meta::Byte(m.surf.bat_flags));
        return;
    }
    m.set(16, Meta::Bool(m.age < 0));
    match m.kind {
        Kind::Rabbit | Kind::Mooshroom => m.set(18, Meta::Int(m.variant)),
        Kind::Ocelot => m.set(18, Meta::Bool(m.surf.trusting)),
        Kind::PolarBear => m.set(18, Meta::Bool(m.surf.standing)),
        Kind::Turtle => {
            m.set(18, Meta::Bool(m.surf.has_egg));
            m.set(19, Meta::Bool(m.surf.laying));
        }
        Kind::Parrot => {
            let f = i8::from(m.sitting) | if m.tame { 4 } else { 0 };
            m.set(18, Meta::Byte(f));
            m.set(19, Meta::Entity(m.owner));
            m.set(20, Meta::Int(m.variant));
        }
        _ => {}
    }
}

// ---------- NBT ----------

pub fn load(m: &mut Mob, e: &Nbt) {
    let flag = |k: &str| int(e, k).is_some_and(|v| v != 0);
    match m.kind {
        Kind::Rabbit => {
            // Goes through the variant pose: the killer gets armor 8 and the temporary attack bonus
            // back (never saved).
            rabbit::set_variant(m, int(e, "RabbitType").map_or(0, |v| rabbit::by_id(v as i32)));
            m.surf.more_carrot = int(e, "MoreCarrotTicks").unwrap_or(0) as i32;
        }
        Kind::Mooshroom => {
            m.variant = i32::from(e.get("Type").and_then(Nbt::as_str) == Some("brown"));
            m.surf.stew = e.get("stew_effects").cloned();
        }
        Kind::Bat => m.surf.bat_flags = int(e, "BatFlags").unwrap_or(0) as i8,
        Kind::Ocelot => m.surf.trusting = flag("Trusting"),
        Kind::PolarBear => {
            m.anger_end = e.get("anger_end_time").and_then(super::mob::f64_of).map_or(-1, |t| t as i64);
            m.surf.angry_at = nbtio::uuid(e.get("angry_at"));
            m.anger_target = m.surf.angry_at.map(|_| super::mob::Ent::Player);
        }
        Kind::Turtle => {
            let p = m.block_pos();
            m.surf.home = match e.get("home_pos") {
                Some(Nbt::IntArray(a)) if a.len() == 3 => [a[0], a[1], a[2]],
                _ => p,
            };
            m.surf.has_egg = flag("has_egg");
        }
        Kind::Parrot => {
            m.variant = int(e, "Variant").map_or(0, |v| (v as i32).clamp(0, 4));
            m.sitting = flag("Sitting");
            m.owner = nbtio::uuid(e.get("Owner"));
            m.tame = m.owner.is_some();
        }
        _ => {}
    }
}

pub fn save(m: &Mob, e: &mut Nbt) {
    let set = |e: &mut Nbt, k: &str, v: Nbt| nbtio::set(e, k, v);
    match m.kind {
        Kind::Rabbit => {
            set(e, "RabbitType", Nbt::Int(m.variant));
            set(e, "MoreCarrotTicks", Nbt::Int(m.surf.more_carrot));
        }
        Kind::Mooshroom => {
            set(e, "Type", Nbt::String(if m.variant == 1 { "brown" } else { "red" }.into()));
            match &m.surf.stew {
                Some(s) => set(e, "stew_effects", s.clone()),
                None => nbtio::remove(e, "stew_effects"),
            }
        }
        Kind::Bat => set(e, "BatFlags", Nbt::Byte(m.surf.bat_flags)),
        Kind::Ocelot => set(e, "Trusting", Nbt::Byte(i8::from(m.surf.trusting))),
        Kind::PolarBear => {
            set(e, "anger_end_time", Nbt::Long(m.anger_end));
            // Absent when there is no target (the starting compound is the copy of the NBT read).
            match m.surf.angry_at.filter(|_| m.anger_target.is_some()) {
                Some(u) => set(e, "angry_at", nbtio::uuid_nbt(u)),
                None => nbtio::remove(e, "angry_at"),
            }
        }
        Kind::Turtle => {
            let h = m.surf.home;
            set(e, "home_pos", Nbt::IntArray(vec![h[0], h[1], h[2]]));
            set(e, "has_egg", Nbt::Byte(i8::from(m.surf.has_egg)));
        }
        Kind::Parrot => {
            set(e, "Variant", Nbt::Int(m.variant));
            set(e, "Sitting", Nbt::Byte(i8::from(m.sitting)));
            match m.owner {
                Some(o) => set(e, "Owner", nbtio::uuid_nbt(o)),
                None => nbtio::remove(e, "Owner"),
            }
        }
        _ => {}
    }
}

// ---------- Goals ----------

pub(crate) fn stroll(speed: f64) -> Goal {
    Goal::Stroll { speed, interval: 120, prob: Some(0.001), wanted: [0.0; 3] }
}

pub(crate) fn look_player(dist: f32) -> Goal {
    Goal::LookAtPlayer { dist, prob: 0.02, at: None, time: 0 }
}

pub(crate) fn look_around() -> Goal {
    Goal::LookAround { rel: [0.0; 2], time: 0 }
}

pub(crate) fn panic(speed: f64, tag: &'static str) -> Goal {
    Goal::Panic { speed, pos: [0.0; 3], tag }
}

pub(crate) fn breed_goal(speed: f64) -> Goal {
    Goal::Breed { speed, partner: None, love_time: 0 }
}

pub(crate) fn tempt(speed: f64, food: &'static str, scare: bool) -> Goal {
    Goal::Tempt { speed, food: Food::Tag(food), scare, stop: 2.5, calm: 0, p: [0.0; 3], prot: [0.0; 2], on: false }
}

pub(crate) fn follow_parent(speed: f64) -> Goal {
    Goal::FollowParent { speed, parent: None, recalc: 0 }
}

pub(crate) fn melee(speed: f64, unseen: bool, kind: Melee) -> Goal {
    Goal::Melee { speed, unseen, kind, path: None, pathed: [0.0; 3], recalc: 0, cd: 0, last: i64::MIN / 2 }
}

pub(crate) fn avoid(who: Who, max: f32, walk: f64, sprint: f64) -> Goal {
    Goal::Avoid { who, max, walk, sprint, from: None, path: None }
}

pub(crate) fn nearest(who: Who, must_see: bool, interval: i32) -> Goal {
    Goal::Nearest {
        base: TBase::new(must_see, false),
        who,
        interval: super::mth::reduced(interval),
        target: None,
        angry_only: false,
        non_tame: false,
    }
}

/// Goals of each species, in the game's insertion order.
pub fn register_goals(s: &Server, m: &mut Mob) {
    match m.kind {
        Kind::Rabbit => rabbit::register(m),
        Kind::Mooshroom => mooshroom::register(m),
        Kind::Ocelot => ocelot::register(m),
        Kind::PolarBear => polar::register(m),
        Kind::Turtle => turtle::register(m),
        Kind::Parrot => parrot::register(m),
        _ => {}
    }
    let _ = s;
}

/// Own navigation (construction).
pub fn init_nav(m: &mut Mob) {
    match m.kind {
        Kind::Parrot => {
            m.nav.fly = true;
            m.nav.can_float = true;
            m.nav.can_open_doors = false;
        }
        Kind::Turtle => m.nav.amphibious = true,
        _ => {}
    }
}

// ---------- Sounds ----------

/// Ambient sound; `None`: silence.
pub fn ambient_sound(s: &mut Server, m: &mut Mob) -> Option<String> {
    match m.kind {
        Kind::Bat => (!(m.surf.bat_flags & 1 != 0 && m.rng.next_int_bound(4) != 0))
            .then(|| "minecraft:entity.bat.ambient".into()),
        Kind::PolarBear => Some(
            if m.is_baby() {
                "minecraft:entity.polar_bear.ambient_baby"
            } else {
                "minecraft:entity.polar_bear.ambient"
            }
            .into(),
        ),
        Kind::Turtle => {
            (!m.in_water && m.on_ground && !m.is_baby()).then(|| "minecraft:entity.turtle.ambient_land".into())
        }
        Kind::Parrot => Some(parrot::ambient(s)),
        k => info(k).ambient.map(Into::into),
    }
}

/// Own hurt / death (baby turtle).
pub fn hurt_sound(m: &Mob, death: bool) -> Option<&'static str> {
    (m.kind == Kind::Turtle && m.is_baby()).then_some(if death {
        "minecraft:entity.turtle.death_baby"
    } else {
        "minecraft:entity.turtle.hurt_baby"
    })
}

/// Own step noise (baby turtle).
pub fn step_sound(m: &Mob) -> Option<&'static str> {
    (m.kind == Kind::Turtle && m.is_baby()).then_some("minecraft:entity.turtle.shamble_baby")
}

/// Voice pitch after the common draw.
pub fn voice_pitch(m: &Mob, p: f32) -> f32 {
    match m.kind {
        Kind::Bat => p * 0.95,
        _ => p,
    }
}

/// Sound category: killer rabbit hostile, bat neutral (default of mobs).
pub fn sound_source(m: &Mob) -> Option<i32> {
    (m.kind == Kind::Rabbit && m.variant == rabbit::EVIL).then_some(5)
}

// ---------- Ticks ----------

/// Before the common tick (`tick` overridden before `super.tick`).
pub fn pre_tick(m: &mut Mob) {
    if m.kind == Kind::Parrot {
        m.surf.ride_cd += 1;
    }
}

/// After the entity's full tick.
pub fn post_tick(s: &mut Server, m: &mut Mob) {
    match m.kind {
        Kind::Bat => bat::post_tick(m),
        Kind::PolarBear => polar::post_tick(s, m),
        Kind::Turtle => {
            let b = m.is_baby();
            if m.surf.prev_baby == Some(true) && !b && m.alive() {
                turtle::grew_up(s, m);
            }
            m.surf.prev_baby = Some(b);
        }
        _ => {}
    }
}

/// Start of the AI step.
pub fn pre_ai_step(s: &mut Server, m: &mut Mob) {
    if m.kind == Kind::Parrot {
        parrot::pre_ai_step(s, m);
    }
}

/// End of the AI step.
pub fn post_ai_step(s: &mut Server, m: &mut Mob) {
    match m.kind {
        Kind::Rabbit => rabbit::post_ai_step(m),
        Kind::Turtle => turtle::post_ai_step(s, m),
        Kind::Parrot => parrot::post_ai_step(m),
        Kind::Chicken => parrot::chicken_flaps(m),
        _ => {}
    }
}

/// The species' own server AI step.
pub fn custom_ai_step(s: &mut Server, m: &mut Mob) {
    match m.kind {
        Kind::Rabbit => rabbit::custom_ai_step(s, m),
        Kind::Bat => bat::custom_ai_step(s, m),
        // The cat shares the ocelot's rule (crouching at 0.6, sprint at 1.33).
        Kind::Ocelot | Kind::Cat => ocelot::custom_ai_step(m),
        _ => {}
    }
}

/// Own movement control; true if it replaces the common control.
pub fn move_control(s: &Server, m: &mut Mob) -> bool {
    match m.kind {
        Kind::Rabbit => {
            rabbit::move_control_pre(m);
            false
        }
        Kind::Parrot => {
            parrot::move_control(m);
            true
        }
        Kind::Turtle => {
            turtle::move_control(s, m);
            true
        }
        _ => false,
    }
}

/// Own jump control; true if it replaces the common control.
pub fn jump_control(s: &mut Server, m: &mut Mob) -> bool {
    if m.kind != Kind::Rabbit {
        return false;
    }
    if m.jump_ctl {
        rabbit::start_jumping(s, m);
        m.jump_ctl = false;
    }
    true
}

/// Jump multiplier, 1 by default.
pub fn jump_mult(s: &Server, m: &Mob) -> f32 {
    if m.kind == Kind::Rabbit { rabbit::jump_power(s, m) / 0.42 } else { 1.0 }
}

/// End of the jump from the ground: sprint momentum, then the rabbit's momentum.
pub fn after_jump(s: &mut Server, m: &mut Mob) {
    if m.surf.sprinting {
        let a = m.yaw * super::mth::DEG;
        m.vel[0] += f64::from(-super::mth::sin(a) * 0.2);
        m.vel[2] += f64::from(super::mth::cos(a) * 0.2);
    }
    if m.kind == Kind::Rabbit {
        rabbit::after_jump(s, m);
    }
}

/// Effective gravity.
pub fn gravity(m: &Mob, g: f64) -> f64 {
    if m.surf.no_gravity { 0.0 } else { g }
}

/// Vertical friction in the air: 0.91 for omnidirectional flyers.
pub fn air_vertical_friction(m: &Mob) -> Option<f32> {
    // F11: the bee too.
    (m.kind == Kind::Parrot || m.kind == Kind::Bee).then_some(0.91)
}

/// Slowdown in water (0.9 when sprinting).
pub fn water_slow_down(m: &Mob) -> f32 {
    if m.surf.sprinting {
        0.9
    } else if m.kind == Kind::PolarBear {
        0.98
    } else {
        0.8
    }
}

/// Own movement in water (turtle); true if it replaces the common one.
pub fn travel_in_water(s: &mut Server, m: &mut Mob) -> bool {
    if m.kind != Kind::Turtle || !m.in_water {
        return false;
    }
    turtle::travel_in_water(s, m);
    true
}

/// Next step.
pub fn next_step(m: &Mob) -> Option<f32> {
    (m.kind == Kind::Turtle).then_some(m.move_dist + 0.15)
}

/// Push: the bat is neither pushed nor pushes; the parrot does not push the player.
pub fn pushable(m: &Mob) -> bool {
    m.kind != Kind::Bat
}

pub fn pushes(m: &Mob) -> bool {
    m.kind != Kind::Bat
}

pub fn pushes_player(m: &Mob) -> bool {
    m.kind != Kind::Parrot
}

/// The turtle is not pushed by currents.
pub fn pushed_by_fluid(k: Kind) -> bool {
    k != Kind::Turtle
}

// ---------- Combat ----------

/// Reactions to damage (after it landed).
pub fn on_hurt(s: &mut Server, m: &mut Mob, by: Option<super::mob::Ent>) {
    match m.kind {
        Kind::Bat => m.surf.bat_flags &= !1,
        Kind::Parrot => m.sitting = false,
        _ => {}
    }
    if is_animal(m.kind) {
        m.reset_love();
    }
    let _ = (s, by);
}

/// Before damage (waking up, standing up).
pub fn before_hurt(m: &mut Mob) {
    match m.kind {
        Kind::Bat => m.surf.bat_flags &= !1,
        Kind::Parrot => m.sitting = false,
        _ => {}
    }
}

// ---------- Spawning ----------

/// Own finalization before the spawn finalization of mobs that grow up (variants, group data).
pub fn finalize_spawn(s: &mut Server, m: &mut Mob, group: &mut super::spawn::Group) {
    let p = m.block_pos();
    match m.kind {
        Kind::Rabbit => {
            let mut v = rabbit::random_variant(s, p);
            match group.surf_variant {
                Some(g) => v = g,
                None => {
                    group.surf_variant = Some(v);
                    group.baby_chance = Some(1.0);
                }
            }
            rabbit::set_variant(m, v);
        }
        Kind::Ocelot | Kind::PolarBear => {
            if !group.ageable {
                group.baby_chance = Some(1.0);
            }
        }
        Kind::Turtle => m.surf.home = p,
        Kind::Parrot => {
            m.variant = s.mobs.rng.next_int_bound(5);
            if !group.ageable {
                group.no_baby = true;
            }
        }
        _ => {}
    }
}

/// Own spawn rule; `None`: common animal rule.
pub fn spawn_rules(s: &mut Server, k: Kind, p: [i32; 3]) -> Option<bool> {
    let below = [p[0], p[1] - 1, p[2]];
    let bright = |s: &Server| {
        let (a, b) = s.light(p);
        a.max(b) > 8
    };
    Some(match k {
        Kind::Rabbit => s.btag_id(below, crate::tag_bits::TagId::RabbitsSpawnableOn) && bright(s),
        Kind::Mooshroom => s.btag_id(below, crate::tag_bits::TagId::MooshroomsSpawnableOn) && bright(s),
        Kind::Parrot => s.btag_id(below, crate::tag_bits::TagId::ParrotsSpawnableOn) && bright(s),
        Kind::Ocelot => s.mobs.rng.next_int_bound(3) != 0,
        Kind::PolarBear => {
            if s.biome_tag(p, "minecraft:polar_bears_spawn_on_alternate_blocks") {
                bright(s) && s.btag_id(below, crate::tag_bits::TagId::PolarBearsSpawnableOnAlternate)
            } else {
                return None;
            }
        }
        Kind::Turtle => p[1] < sea_level(s) + 4 && turtle::on_sand(s, p) && bright(s),
        Kind::Bat => bat::spawn_rules(s, p),
        _ => return None,
    })
}

/// Own obstruction: the ocelot requires grass or leaves.
pub fn spawn_obstruction_ok(s: &Server, m: &Mob) -> bool {
    if m.kind != Kind::Ocelot {
        return true;
    }
    let p = m.block_pos();
    if p[1] < sea_level(s) {
        return false;
    }
    let below = [p[0], p[1] - 1, p[2]];
    s.bname(below) == "minecraft:grass_block" || s.btag_id(below, crate::tag_bits::TagId::Leaves)
}

/// Despawning far from players; `None`: common rule.
pub fn remove_when_far_away(m: &Mob) -> Option<bool> {
    Some(match m.kind {
        Kind::Ocelot => !m.surf.trusting && m.tick_count > 2400,
        Kind::Bat => m.custom_name.is_none(),
        k if handles(k) => false,
        _ => return None,
    })
}

/// Own walk value.
pub fn walk_target_value(s: &Server, m: &Mob, p: [i32; 3]) -> Option<f32> {
    match m.kind {
        Kind::Mooshroom => {
            Some(if s.bname([p[0], p[1] - 1, p[2]]) == "minecraft:mycelium" { 10.0 } else { s.magic_light(p) - 0.5 })
        }
        Kind::Turtle => {
            Some(if (!m.surf.going_home && s.mob_fluid(p) == super::env::Fluid::Water) || turtle::on_sand(s, p) {
                10.0
            } else {
                s.magic_light(p) - 0.5
            })
        }
        _ => None,
    }
}

// ---------- Interactions and breeding ----------

/// Own right click; `Some(r)`: handled.
pub fn interact(s: &mut Server, m: &mut Mob, slot: usize, item: &str) -> Option<bool> {
    match m.kind {
        Kind::Mooshroom => mooshroom::interact(s, m, slot, item),
        Kind::Ocelot => ocelot::interact(s, m, slot, item),
        Kind::Parrot => parrot::interact(s, m, slot, item),
        _ => None,
    }
}

/// Own breeding; true if handled.
pub fn breed(s: &mut Server, m: &mut Mob, partner: i32) -> bool {
    match m.kind {
        Kind::Turtle => {
            turtle::breed(s, m, partner);
            true
        }
        Kind::Rabbit | Kind::Mooshroom | Kind::Ocelot | Kind::PolarBear => {
            let Some(pi) = s.mob_index(partner) else { return true };
            let pv = s.mobs.list[pi].as_ref().map_or(0, |p| p.variant);
            let mut baby = s.new_mob(m.kind, m.pos, 0.0);
            baby.age = -24000;
            match m.kind {
                Kind::Rabbit => {
                    let mut v = rabbit::random_variant(s, m.block_pos());
                    if m.rng.next_int_bound(20) != 0 {
                        v = if m.rng.next_bool() { pv } else { m.variant };
                    }
                    rabbit::set_variant(&mut baby, v);
                }
                Kind::Mooshroom => {
                    baby.variant = if m.variant == pv && m.rng.next_int_bound(1024) == 0 {
                        1 - m.variant
                    } else if m.rng.next_bool() {
                        m.variant
                    } else {
                        pv
                    };
                }
                _ => {}
            }
            baby.yaw = 0.0;
            s.s9_bred(m, partner, Some(&baby)); // S9 (before love is forgotten, which erases its cause)
            m.age = 6000;
            m.reset_love();
            if let Some(p) = s.mobs.list[pi].as_mut() {
                p.age = 6000;
                p.reset_love();
            }
            s.mob_event(m, 18);
            if s.rule("minecraft:mob_drops", true) {
                let xp = m.rng.next_int_bound(7) + 1;
                s.spawn_orbs(m.pos, xp);
            }
            s.add_mob(baby);
            true
        }
        _ => false,
    }
}

/// The "can fall in love" test / usable breeding goal.
pub fn can_breed(m: &Mob) -> bool {
    !(m.kind == Kind::Turtle && m.surf.has_egg)
}

/// Avoidance goal allowed (killer rabbit, trusting ocelot: no).
pub fn avoid_allowed(m: &Mob) -> bool {
    match m.kind {
        Kind::Rabbit => m.variant != rabbit::EVIL,
        Kind::Ocelot => !m.surf.trusting,
        _ => true,
    }
}

/// Can tempting scare?
pub fn can_scare(m: &Mob) -> bool {
    !(m.kind == Kind::Ocelot && m.surf.trusting)
}

/// Panic tag: the cub panics at everything, the adult at environmental causes.
pub fn panic_tag(m: &Mob, tag: &'static str) -> &'static str {
    if m.kind == Kind::PolarBear {
        if m.is_baby() { "minecraft:panic_causes" } else { "minecraft:panic_environmental_causes" }
    } else {
        tag
    }
}

/// Panic in progress (tick): rabbit speed, parrot teleportation to its owner.
pub fn panic_tick(s: &mut Server, m: &mut Mob, speed: f64) {
    match m.kind {
        Kind::Rabbit => rabbit::set_speed_modifier(m, speed),
        Kind::Parrot => parrot::panic_tick(s, m),
        _ => {}
    }
}

/// Loot table without a killed-entity context (shearing, growth), the game's random sequence.
pub(crate) fn loot(s: &mut Server, table: &str, ctx: &crate::loot::Ctx<'_>) -> Vec<crate::player::Stack> {
    let tags = &s.block_tags;
    let data = &s.data;
    let tag_fn = |tag: &str, n: &str| {
        let id = data.blocks.iter().position(|b| b.name == n).map(|i| i as u32);
        id.is_some_and(|id| tags.get(tag).is_some_and(|v| v.binary_search(&id).is_ok()))
    };
    let l = crate::loot::Loot { pack: &s.life.pack, items: &s.life.items, block_tag: &tag_fn };
    let seq = s.life.pack.get("loot_table", table).and_then(|t| t["random_sequence"].as_str().map(str::to_owned));
    match seq {
        Some(q) => {
            let rng = s.life.seqs.get(&q);
            l.drops(table, ctx, rng)
        }
        None => {
            let mut rng = s.life.rng;
            let d = l.drops(table, ctx, &mut rng);
            s.life.rng = rng;
            d
        }
    }
}

impl Server {
    /// Player in water (feet or eyes cell).
    pub(crate) fn player_in_water(&self) -> bool {
        let feet = super::mth::block_of(self.pos);
        let eye = super::mth::block_of([self.pos[0], self.player_eye_y(), self.pos[2]]);
        self.mob_fluid(feet) == super::env::Fluid::Water || self.mob_fluid(eye) == super::env::Fluid::Water
    }

    /// Player in powder snow.
    pub(crate) fn player_in_powder_snow(&self) -> bool {
        self.bname(super::mth::block_of(self.pos)) == "minecraft:powder_snow"
    }
}

/// Lightning (called by each life tick of a real lightning): common strike
/// (fire, 5 damage) except species overrides.
pub fn thunder_hit(s: &mut Server, m: &mut Mob, bolt: u128) {
    match m.kind {
        Kind::Mooshroom => mooshroom::thunder_hit(s, m, bolt),
        Kind::Turtle => {
            s.mob_hurt(m, "minecraft:lightning_bolt", f32::MAX, None, None);
        }
        // F06: copper golem (common strike, then one oxidation step removed).
        Kind::CopperGolem => {
            s.mob_thunder_hit_default(m);
            super::golems::copper::thunder_hit(m, bolt);
        }
        // F07: struck villager → witch (outside peaceful), otherwise common strike.
        Kind::Villager => {
            if !s.villager_struck(m) {
                s.mob_thunder_hit_default(m);
            }
        }
        // F13: struck pig → zombified piglin (outside peaceful), otherwise common strike.
        Kind::Pig => {
            if !s.pig_struck(m) {
                s.mob_thunder_hit_default(m);
            }
        }
        // Charged creeper.
        Kind::Creeper => {
            s.mob_thunder_hit_default(m);
            m.powered = true;
        }
        _ => s.mob_thunder_hit_default(m),
    }
}
