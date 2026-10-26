//! Golems (family F06, `server-mobs.md` § 4.5): iron golem, snow golem, copper golem. Info
//! sheets, own state, metadata, NBT, goals, construction from blocks (`build.rs`) and dispatch
//! of the hooks called by the common engine (`mobs/`).

pub(crate) mod build;
pub(crate) mod copper;
pub(crate) mod iron;
pub(crate) mod snow;
pub(crate) mod snowball;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod world_tests;

use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::goals::{Goal, TBase, Who};
use super::mob::{Ent, Mob};
use super::species::{Category, Info, Kind, info as base_info};
use crate::nbtio::{self, int};
use crate::server::Server;

// ---------- Info sheets ----------

/// Golem: "misc" category, no ambient or default hurt sound, interval 120,
/// fall-immune (tag `fall_damage_immune`).
const fn golem(name: &'static str, width: f32, height: f32, eye: f32) -> Info {
    let mut i = base_info(name, Category::Misc, width, height, eye);
    i.ambient_interval = 120;
    i.fall_immune = true;
    i
}

static IRON_GOLEM: Info = {
    let mut i = golem("minecraft:iron_golem", 1.4, 2.7, 2.7 * 0.85);
    i.health = 100.0;
    i.speed = 0.25;
    i.attack = 15.0;
    i.step = 1.0;
    i.hurt = "minecraft:entity.iron_golem.hurt";
    i.death = "minecraft:entity.iron_golem.death";
    i
};

static SNOW_GOLEM: Info = {
    let mut i = golem("minecraft:snow_golem", 0.7, 1.9, 1.7);
    i.health = 4.0;
    i.speed = 0.2f32 as f64;
    i.tracking = 8;
    i.ambient = Some("minecraft:entity.snow_golem.ambient");
    i.hurt = "minecraft:entity.snow_golem.hurt";
    i.death = "minecraft:entity.snow_golem.death";
    // Wet (water, rain): 1 drowning point per tick.
    i.water_sensitive = true;
    i
};

static COPPER_GOLEM: Info = {
    let mut i = golem("minecraft:copper_golem", 0.49, 0.98, 0.8125);
    i.health = 12.0;
    i.speed = 0.2f32 as f64;
    i.step = 1.0;
    i.hurt = "minecraft:entity.copper_golem.hurt";
    i.death = "minecraft:entity.copper_golem.death";
    i
};

pub fn info(k: Kind) -> &'static Info {
    match k {
        Kind::IronGolem => &IRON_GOLEM,
        Kind::SnowGolem => &SNOW_GOLEM,
        _ => &COPPER_GOLEM,
    }
}

/// Species of the family.
pub fn handles(k: Kind) -> bool {
    matches!(k, Kind::IronGolem | Kind::SnowGolem | Kind::CopperGolem)
}

/// The game's "enemy" interface: monsters, slime, magma cube, phantom (not the sulfur cube).
pub fn is_enemy(k: Kind) -> bool {
    // F09: the zombie nautilus (monster category) is not an "enemy".
    k.is_monster() && k != Kind::SulfurCube && !super::sealife::handles(k)
}

// ---------- Own state ----------

/// Golem state (`golem` field of the mob).
#[derive(Debug, Clone, Default)]
pub struct Golem {
    /// Iron: built by a player (flag 1 of index 16).
    pub player_created: bool,
    /// Iron: hit animation (10 ticks) and held-out flower (400 ticks).
    pub attack_anim: i32,
    pub offer_flower: i32,
    /// Iron: persistent anger target (recorded UUID; reference, resolved to an entity at the tick).
    pub angry_at: Option<u128>,
    /// Iron: anger reference read back, to also set as target at the first tick.
    pub relink: bool,
    /// Snow: carried pumpkin (flag 16 of index 16).
    pub pumpkin: bool,
    /// Copper: oxidation (0 intact … 3 oxidized), interaction state (0 idle … 4).
    pub weather: i32,
    pub state: i32,
    /// Copper: next oxidation tick (−1 to draw, −2 waxed, 0 oxidized).
    pub next_weather: i64,
    /// Copper: last lightning received (a single effect per lightning).
    pub last_bolt: Option<u128>,
    /// Copper: chest open during the interaction.
    pub opened_chest: Option<[i32; 3]>,
    /// Copper: brain (memories, behaviors).
    pub brain: copper::Brain,
}

/// Construction: initial state specific to the species.
pub fn on_new(m: &mut Mob) {
    match m.kind {
        Kind::IronGolem => {
            m.attrs.knockback_resistance.base = 1.0;
        }
        Kind::SnowGolem => m.golem.pumpkin = true,
        Kind::CopperGolem => copper::on_new(m),
        _ => {}
    }
}

pub fn define_data(m: &mut Mob) {
    match m.kind {
        Kind::IronGolem => m.define(16, Meta::Byte(0)),
        Kind::SnowGolem => m.define(16, Meta::Byte(16)),
        _ => {
            m.define(16, Meta::Variant(0, 38));
            m.define(17, Meta::Variant(0, 37));
        }
    }
}

pub fn sync_data(m: &mut Mob) {
    match m.kind {
        Kind::IronGolem => m.set(16, Meta::Byte(i8::from(m.golem.player_created))),
        Kind::SnowGolem => m.set(16, Meta::Byte(if m.golem.pumpkin { 16 } else { 0 })),
        _ => {
            m.set(16, Meta::Variant(m.golem.weather, 38));
            m.set(17, Meta::Variant(m.golem.state, 37));
        }
    }
}

pub const WEATHER_NAMES: [&str; 4] = ["unaffected", "exposed", "weathered", "oxidized"];

pub fn load(m: &mut Mob, e: &Nbt) {
    let flag = |k: &str, d: bool| int(e, k).map_or(d, |v| v != 0);
    match m.kind {
        Kind::IronGolem => {
            m.golem.player_created = flag("PlayerCreated", false);
            // Persistent anger (`anger_end_time`, old `AngerTime`, target `angry_at`).
            m.anger_end = match e.get("anger_end_time").and_then(super::mob::f64_of) {
                Some(t) => t as i64,
                None => -1,
            };
            // The reference keeps the real UUID (player or mob): the entity is found again at the
            // first tick (`iron::relink_anger`), never assumed to be the player.
            m.golem.angry_at = nbtio::uuid(e.get("angry_at"));
            m.anger_target = None;
            m.golem.relink = m.golem.angry_at.is_some();
        }
        Kind::SnowGolem => m.golem.pumpkin = flag("Pumpkin", true),
        _ => {
            m.golem.next_weather = e.get("next_weather_age").and_then(super::mob::f64_of).map_or(-1, |v| v as i64);
            m.golem.weather = e
                .get("weather_state")
                .and_then(Nbt::as_str)
                .and_then(|s| WEATHER_NAMES.iter().position(|w| *w == s))
                .map_or(0, |i| i as i32);
            copper::load_brain(m, e);
        }
    }
}

pub fn save(m: &Mob, e: &mut Nbt) {
    let set = |e: &mut Nbt, k: &str, v: Nbt| nbtio::set(e, k, v);
    match m.kind {
        Kind::IronGolem => {
            set(e, "PlayerCreated", Nbt::Byte(i8::from(m.golem.player_created)));
            set(e, "anger_end_time", Nbt::Long(m.anger_end));
            // Reference written even if the entity is not loaded (it is erased with the anger).
            match m.golem.angry_at {
                Some(u) => set(e, "angry_at", nbtio::uuid_nbt(u)),
                None => nbtio::remove(e, "angry_at"),
            }
        }
        Kind::SnowGolem => set(e, "Pumpkin", Nbt::Byte(i8::from(m.golem.pumpkin))),
        _ => {
            set(e, "next_weather_age", Nbt::Long(m.golem.next_weather));
            set(e, "weather_state", Nbt::String(WEATHER_NAMES[m.golem.weather.clamp(0, 3) as usize].into()));
            copper::save_brain(m, e);
        }
    }
}

// ---------- Goals ----------

/// Own goals of the golems (`Goal::Golem`).
#[derive(Debug, Clone)]
pub enum GolemGoal {
    /// Iron: approach the target within 32 blocks at most (toward a random position).
    MoveTowardsTarget { wanted: [f64; 3], target: Option<Ent> },
    /// Iron: return to the village (wander with interval 10, without a village: never).
    MoveBackToVillage { wanted: [f64; 3] },
    /// Iron: wander in the village (interval 240).
    VillageStroll { wanted: [f64; 3] },
    /// Iron: hold out a poppy to a villager or a copper golem (400 ticks).
    OfferFlower { entity: Option<Ent>, tick: i32 },
    /// Iron: defend the village against a player with a bad reputation.
    DefendVillage { base: TBase, potential: Option<Ent> },
    /// Snow: ranged attack (speed 1.25, interval 20, radius 10).
    Ranged { target: Option<Ent>, attack_time: i32, see_time: i32 },
}

const MOVE: u8 = 1;
const LOOK: u8 = 2;
const TARGET: u8 = 8;

impl GolemGoal {
    pub fn flags(&self) -> u8 {
        match self {
            GolemGoal::MoveTowardsTarget { .. }
            | GolemGoal::MoveBackToVillage { .. }
            | GolemGoal::VillageStroll { .. } => MOVE,
            GolemGoal::OfferFlower { .. } | GolemGoal::Ranged { .. } => MOVE | LOOK,
            GolemGoal::DefendVillage { .. } => TARGET,
        }
    }

    pub fn every_tick(&self) -> bool {
        matches!(self, GolemGoal::Ranged { .. })
    }
}

fn nearest(who: Who, must_see: bool, interval: i32) -> Goal {
    Goal::Nearest {
        base: TBase::new(must_see, false),
        who,
        interval: super::mth::reduced(interval),
        target: None,
        angry_only: false,
        non_tame: false,
    }
}

/// Goals and targets in the game's order (the copper golem only has a brain).
pub fn register_goals(m: &mut Mob) {
    match m.kind {
        Kind::IronGolem => {
            let g = &mut m.goals;
            g.add(1, super::surface::melee(1.0, true, super::goals::Melee::Plain));
            g.add(2, Goal::Golem(GolemGoal::MoveTowardsTarget { wanted: [0.0; 3], target: None }));
            g.add(2, Goal::Golem(GolemGoal::MoveBackToVillage { wanted: [0.0; 3] }));
            g.add(4, Goal::Golem(GolemGoal::VillageStroll { wanted: [0.0; 3] }));
            g.add(5, Goal::Golem(GolemGoal::OfferFlower { entity: None, tick: 0 }));
            g.add(7, super::surface::look_player(6.0));
            g.add(8, super::surface::look_around());
            let t = &mut m.targets;
            t.add(1, Goal::Golem(GolemGoal::DefendVillage { base: TBase::new(false, true), potential: None }));
            t.add(2, Goal::HurtBy { base: TBase::new(true, false), alert: false, stamp: 0 });
            t.add(
                3,
                Goal::Nearest {
                    base: TBase::new(true, false),
                    who: Who::Player,
                    interval: super::mth::reduced(10),
                    target: None,
                    angry_only: true,
                    non_tame: false,
                },
            );
            t.add(3, nearest(Who::GolemPrey, false, 5));
            t.add(4, Goal::ResetAnger { stamp: 0, alert: false });
        }
        Kind::SnowGolem => {
            let g = &mut m.goals;
            g.add(1, Goal::Golem(GolemGoal::Ranged { target: None, attack_time: -1, see_time: 0 }));
            g.add(2, Goal::Stroll { speed: 1.0, interval: 120, prob: Some(1.000_000_1e-5), wanted: [0.0; 3] });
            g.add(3, super::surface::look_player(6.0));
            g.add(4, super::surface::look_around());
            m.targets.add(1, nearest(Who::Enemy, true, 10));
        }
        _ => {}
    }
}

/// Copper golem navigation: doors, length 48, fire malus.
pub fn init_nav(m: &mut Mob) {
    if m.kind == Kind::CopperGolem {
        m.nav.can_open_doors = true;
        m.nav.required_length = 48.0;
    }
}

// ---------- Engine hooks ----------

/// Ambient sound (snow golem only).
pub fn ambient_sound(m: &Mob) -> Option<String> {
    (m.kind == Kind::SnowGolem).then(|| "minecraft:entity.snow_golem.ambient".to_owned())
}

/// Hurt and death sounds (copper: by oxidation).
pub fn hurt_sound(m: &Mob, death: bool) -> Option<&'static str> {
    if m.kind != Kind::CopperGolem {
        return None;
    }
    Some(copper::sounds(m.golem.weather)[if death { 2 } else { 1 }])
}

/// Own step noise (iron, copper: volume 1); snow: block sound.
pub fn step_sound(m: &Mob) -> Option<(&'static str, f32)> {
    match m.kind {
        Kind::IronGolem => Some(("minecraft:entity.iron_golem.step", 1.0)),
        Kind::CopperGolem => Some((copper::sounds(m.golem.weather)[3], 1.0)),
        _ => None,
    }
}

/// The own possible-attack test: the iron golem built by a player never targets the player; no
/// iron golem targets a creeper.
pub fn can_attack(s: &Server, m: &Mob, target: Ent) -> bool {
    if m.kind != Kind::IronGolem {
        return true;
    }
    match target {
        Ent::Player => !m.golem.player_created,
        Ent::Mob(id) => s.mob_ref(id).is_none_or(|o| o.kind != Kind::Creeper),
    }
}

impl Server {
    pub(crate) fn golem_can_use(&mut self, g: &mut GolemGoal, m: &mut Mob) -> bool {
        match g {
            GolemGoal::Ranged { .. } => self.snow_can_use(g, m),
            _ => self.iron_can_use(g, m),
        }
    }

    pub(crate) fn golem_can_continue(&mut self, g: &mut GolemGoal, m: &mut Mob) -> bool {
        match g {
            GolemGoal::Ranged { .. } => self.snow_can_continue(g, m),
            GolemGoal::DefendVillage { base, .. } => self.target_continue(base, m),
            _ => self.iron_can_continue(g, m),
        }
    }

    pub(crate) fn golem_start(&mut self, g: &mut GolemGoal, m: &mut Mob) {
        match g {
            GolemGoal::Ranged { .. } => {}
            GolemGoal::DefendVillage { base, .. } => {
                let mut b = std::mem::take(base);
                self.iron_start(g, m);
                Self::target_start(&mut b);
                b.target_mob = m.target;
                if let GolemGoal::DefendVillage { base, .. } = g {
                    *base = b;
                }
            }
            _ => self.iron_start(g, m),
        }
    }

    pub(crate) fn golem_stop(&mut self, g: &mut GolemGoal, m: &mut Mob) {
        match g {
            GolemGoal::Ranged { .. } => self.snow_stop(g),
            GolemGoal::DefendVillage { base, .. } => {
                // Target forgotten.
                m.target = None;
                base.target_mob = None;
            }
            _ => self.iron_stop(g, m),
        }
    }

    pub(crate) fn golem_tick(&mut self, g: &mut GolemGoal, m: &mut Mob) {
        match g {
            GolemGoal::Ranged { .. } => self.snow_tick(g, m),
            _ => self.iron_tick(g, m),
        }
    }

    /// Start of the base tick: running particles of the iron golem (draws from the
    /// entity random, no server-side effect).
    pub(crate) fn golem_base_tick(&mut self, m: &mut Mob) {
        if m.kind != Kind::IronGolem {
            return;
        }
        iron::relink_anger(self, m);
        let h = m.vel[0] * m.vel[0] + m.vel[2] * m.vel[2];
        if h > f64::from(2.500_000_3e-7f32) && m.rng.next_int_bound(5) == 0 {
            let on = self.mob_below(m, 0.2);
            if !self.is_air(on) {
                let _ = m.rng.next_double();
                let _ = m.rng.next_double();
            }
        }
    }

    /// End of the AI step (after the mobs' common AI step).
    pub(crate) fn golem_post_ai_step(&mut self, m: &mut Mob) {
        match m.kind {
            Kind::IronGolem => iron::post_ai_step(self, m),
            Kind::SnowGolem => snow::post_ai_step(self, m),
            _ => {}
        }
    }

    /// The species' own server AI step (copper golem brain).
    pub(crate) fn golem_custom_ai_step(&mut self, m: &mut Mob) {
        if m.kind == Kind::CopperGolem {
            copper::brain_tick(self, m);
        }
    }

    /// End of `tick` (oxidation of the copper golem).
    pub(crate) fn golem_post_tick(&mut self, m: &mut Mob) {
        if m.kind == Kind::CopperGolem && !m.removed {
            copper::weathering_tick(self, m);
        }
    }

    /// Right click on a golem; `None`: common handling.
    pub(crate) fn golem_interact(&mut self, m: &mut Mob, slot: usize, item: &str) -> Option<bool> {
        match m.kind {
            Kind::IronGolem => iron::interact(self, m, slot, item),
            Kind::SnowGolem => snow::interact(self, m, slot, item),
            Kind::CopperGolem => copper::interact(self, m, slot, item),
            _ => None,
        }
    }

    /// Pushed by another mob: the iron golem sometimes targets the enemy.
    pub(crate) fn golem_pushed(&mut self, m: &mut Mob, other: &Mob) {
        if m.kind == Kind::IronGolem
            && is_enemy(other.kind)
            && other.kind != Kind::Creeper
            && m.rng.next_int_bound(20) == 0
        {
            m.target = Some(Ent::Mob(other.id));
        }
    }

    /// Cube pushed by an iron golem: contact damage to the golem if the
    /// cube can hurt (melee range, direct sight).
    pub(crate) fn cube_pushed_by_golem(&mut self, cube: &mut Mob, golem: &mut Mob) {
        if golem.kind != Kind::IronGolem || !cube.kind.is_cube() || !cube.can_deal_damage() || !cube.alive() {
            return;
        }
        let r = f64::from(2.04f32).sqrt() - f64::from(0.6f32);
        let b = cube.bbox();
        let reach = rc_physics::Aabb {
            min: [b.min[0] - r, b.min[1], b.min[2] - r],
            max: [b.max[0] + r, b.max[1], b.max[2] + r],
        };
        if !reach.intersects(&golem.bbox()) {
            return;
        }
        let from = [cube.pos[0], cube.eye_y(), cube.pos[2]];
        let to = [golem.pos[0], golem.eye_y(), golem.pos[2]];
        if super::mth::dist2(from, to) > 128.0 * 128.0 || self.clip_blocks(from, to) {
            return;
        }
        let dmg = cube.attack_damage();
        if self.mob_hurt(golem, "minecraft:mob_attack", dmg, Some(Ent::Mob(cube.id)), Some(cube.pos)) {
            let pitch = (cube.rng.next_float() - cube.rng.next_float()) * 0.2 + 1.0;
            self.cube_sound(cube, "minecraft:entity.slime.attack", 1.0, pitch);
        }
    }

    /// The iron and copper golem after damage dealt.
    pub(crate) fn golem_after_hurt(&mut self, m: &mut Mob, crack_before: Option<i32>) {
        if let Some(c) = crack_before
            && iron::crackiness(m) != c
        {
            self.mob_sound(m, "minecraft:entity.iron_golem.damage", 1.0, 1.0);
        }
    }
}

/// Poppy held out to a copper golem ("antenna" slot = saddle, guaranteed drop).
pub(crate) fn give_antenna_poppy(o: &mut Mob) {
    o.equipment[7] = Some(crate::player::Stack { id: "minecraft:poppy".into(), count: 1, components: None });
    o.drop_chance[7] = 2.0;
}
