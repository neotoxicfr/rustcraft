//! Nether hostiles (family F12, `server-mobs.md` § 4.F12): blaze, ghast, wither
//! skeleton, strider. Stat sheets, own state, metadata, NBT, goals and hooks called by the
//! common engine (`mobs/`). The goals are in `goals.rs`, the ghast's flight and shot in
//! `ghast.rs`, the strider (lava, cold, saddle, stick) in `strider.rs`; the fireballs are
//! in `mobs/fireballs.rs`. The wither skeleton inherits the skeleton's goals and shot (F01).

pub(crate) mod blaze;
pub(crate) mod ghast;
pub(crate) mod goals;
pub(crate) mod strider;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod world_tests;

use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::goals::{Goal, TBase, Who};
use super::mob::{Ent, Mob};
use super::nav::PathType;
use super::species::{Category, Info, Kind, info as base_info};
use crate::nbtio::{self, int};
use crate::server::Server;

pub use goals::NGoal;

// ---------- Species info ----------

/// Game monster: 5 experience points, tracking 8 chunks.
const fn monster(name: &'static str, width: f32, height: f32, eye: f32) -> Info {
    let mut i = base_info(name, Category::Monster, width, height, eye);
    i.xp = 5;
    i.tracking = 8;
    i.not_in_peaceful = true;
    i.hurt = "minecraft:entity.hostile.hurt";
    i.death = "minecraft:entity.hostile.death";
    i
}

/// Blaze: 0.6 × 1.8 (eyes at 0.85), damage 6, speed 0.23, tracking 48, 10 experience.
static BLAZE: Info = {
    let mut i = monster("minecraft:blaze", 0.6, 1.8, 1.8 * 0.85);
    i.attack = 6.0;
    i.speed = 0.23f32 as f64;
    i.follow = 48.0;
    i.xp = 10;
    i.ambient = Some("minecraft:entity.blaze.ambient");
    i.hurt = "minecraft:entity.blaze.hurt";
    i.death = "minecraft:entity.blaze.death";
    i.fire_immune = true;
    i.water_sensitive = true;
    // `#fall_damage_immune`.
    i.fall_immune = true;
    i
};

/// Ghast: 4 × 4 (eyes at 2.6), HP 10, tracking 100, no fall, volume 5, client tracking 10.
static GHAST: Info = {
    let mut i = monster("minecraft:ghast", 4.0, 4.0, 2.6);
    i.health = 10.0;
    i.follow = 100.0;
    i.tracking = 10;
    i.volume = 5.0;
    i.ambient = Some("minecraft:entity.ghast.ambient");
    i.hurt = "minecraft:entity.ghast.hurt";
    i.death = "minecraft:entity.ghast.death";
    i.fire_immune = true;
    i.fall_immune = true;
    i
};

/// Wither skeleton: 0.7 × 2.4 (eyes at 2.1), skeleton base (speed 0.25), burns in sunlight
/// but immune to fire.
static WITHER_SKELETON: Info = {
    let mut i = monster("minecraft:wither_skeleton", 0.7, 2.4, 2.1);
    i.speed = 0.25;
    i.ambient = Some("minecraft:entity.wither_skeleton.ambient");
    i.hurt = "minecraft:entity.wither_skeleton.hurt";
    i.death = "minecraft:entity.wither_skeleton.death";
    i.step_sound = Some("minecraft:entity.wither_skeleton.step");
    i.burns = true;
    i.fire_immune = true;
    i
};

/// Strider: animal 0.9 × 1.7, HP 20, speed 0.175, sensitive to water, immune to fire.
static STRIDER: Info = {
    let mut i = super::species::animal("minecraft:strider", 0.9, 1.7, 1.7 * 0.85, 20.0, 0.175f32 as f64);
    i.ambient = Some("minecraft:entity.strider.ambient");
    i.hurt = "minecraft:entity.strider.hurt";
    i.death = "minecraft:entity.strider.death";
    i.step_sound = Some("minecraft:entity.strider.step");
    i.fire_immune = true;
    i.water_sensitive = true;
    i
};

pub fn info(k: Kind) -> &'static Info {
    match k {
        Kind::Blaze => &BLAZE,
        Kind::Ghast => &GHAST,
        Kind::WitherSkeleton => &WITHER_SKELETON,
        _ => &STRIDER,
    }
}

/// Species of the family.
pub fn handles(k: Kind) -> bool {
    matches!(k, Kind::Blaze | Kind::Ghast | Kind::WitherSkeleton | Kind::Strider)
}

/// Dimensions of the baby strider (0.45 × 0.85, eyes at 0.4375).
pub fn baby_dims(k: Kind) -> Option<(f32, f32, f32)> {
    (k == Kind::Strider).then_some((0.45, 0.85, 0.4375))
}

/// The ghast is not a game "monster" (simple mob, outside the "enemy" category): neither light wear,
/// nor monster walk value.
pub fn is_ghast(k: Kind) -> bool {
    k == Kind::Ghast
}

/// "Magic" light value always 1 (blaze): inaction always wears off.
pub fn always_idle_wear(k: Kind) -> bool {
    k == Kind::Blaze
}

/// Game animal (strider: breeding, food, experience 1 to 3).
pub fn is_animal(k: Kind) -> bool {
    k == Kind::Strider
}

/// Food (tag `strider_food`).
pub fn food_tag(k: Kind) -> Option<&'static str> {
    (k == Kind::Strider).then_some("minecraft:strider_food")
}

// ---------- Own state ----------

/// State of the family (`neth` field of the mob).
#[derive(Debug, Clone)]
pub struct Neth {
    /// Blaze: allowed height offset, next draw, charged (flag 1 of index 16).
    pub height_offset: f32,
    pub next_height_change: i32,
    pub charged: bool,
    /// Ghast: shot charge (index 16), explosion power, floating duration of the control.
    pub charging: bool,
    pub explosion_power: i32,
    pub float_duration: i32,
    /// Strider: total boost duration (index 18), boost in progress, boost tick, cold
    /// (index 19), player steering with the stick (updated every tick).
    pub boost_total: i32,
    pub boosting: bool,
    pub boost_time: i32,
    pub suffocating: bool,
    pub steered: bool,
}

impl Default for Neth {
    fn default() -> Neth {
        Neth {
            height_offset: 0.5,
            next_height_change: 0,
            charged: false,
            charging: false,
            explosion_power: 1,
            float_duration: 0,
            boost_total: 0,
            boosting: false,
            boost_time: 0,
            suffocating: false,
            steered: false,
        }
    }
}

pub fn define_data(m: &mut Mob) {
    match m.kind {
        // Blaze flags data.
        Kind::Blaze => m.define(16, Meta::Byte(0)),
        // Ghast "charging" data.
        Kind::Ghast => m.define(16, Meta::Bool(false)),
        // The wither skeleton has nothing of its own (no conversion or shearing).
        Kind::WitherSkeleton => {}
        _ => {
            // The "growing mob" layer (baby, locked age), then boost and cold.
            m.define(16, Meta::Bool(false));
            m.define(17, Meta::Bool(false));
            m.define(18, Meta::Int(0));
            m.define(19, Meta::Bool(false));
        }
    }
}

pub fn sync_data(m: &mut Mob) {
    match m.kind {
        Kind::Blaze => m.set(16, Meta::Byte(i8::from(m.neth.charged))),
        Kind::Ghast => m.set(16, Meta::Bool(m.neth.charging)),
        Kind::WitherSkeleton => {}
        _ => {
            m.set(16, Meta::Bool(m.age < 0));
            m.set(17, Meta::Bool(m.mount.age_locked));
            m.set(18, Meta::Int(m.neth.boost_total));
            m.set(19, Meta::Bool(m.neth.suffocating));
        }
    }
}

pub fn load(m: &mut Mob, e: &Nbt) {
    match m.kind {
        Kind::Ghast => m.neth.explosion_power = int(e, "ExplosionPower").map_or(1, |v| i32::from(v as i8)),
        Kind::Strider => m.mount.age_locked = int(e, "AgeLocked").is_some_and(|v| v != 0),
        _ => {}
    }
}

pub fn save(m: &Mob, e: &mut Nbt) {
    match m.kind {
        Kind::Ghast => nbtio::set(e, "ExplosionPower", Nbt::Byte(m.neth.explosion_power as i8)),
        Kind::Strider => nbtio::set(e, "AgeLocked", Nbt::Byte(i8::from(m.mount.age_locked))),
        // Base damage 4 set at spawn: saved with the attributes.
        Kind::WitherSkeleton => {
            if let Some(Nbt::List(list)) = nbtio::get_mut(e, "attributes") {
                list.retain(|a| a.get("id").and_then(Nbt::as_str) != Some("minecraft:attack_damage"));
                list.push(Nbt::Compound(vec![
                    ("id".into(), Nbt::String("minecraft:attack_damage".into())),
                    ("base".into(), Nbt::Double(m.attrs.attack.base)),
                ]));
            }
        }
        _ => {}
    }
}

// ---------- Goals ----------

fn hurt_by(alert: bool) -> Goal {
    Goal::HurtBy { base: TBase::new(true, false), alert, stamp: 0 }
}

fn look_player(dist: f32) -> Goal {
    Goal::LookAtPlayer { dist, prob: 0.02, at: None, time: 0 }
}

/// Goals and targets in the game's add order (the wither skeleton goes through the skeleton base).
pub fn register_goals(m: &mut Mob) {
    let g = &mut m.goals;
    let t = &mut m.targets;
    match m.kind {
        Kind::Blaze => {
            g.add(4, NGoal::BlazeAttack { step: 0, time: 0, last_seen: 0 }.wrap());
            // Without a restriction zone (never set here), inactive.
            g.add(7, Goal::Stroll { speed: 1.0, interval: 120, prob: Some(0.0), wanted: [0.0; 3] });
            g.add(8, look_player(8.0));
            g.add(8, super::surface::look_around());
            t.add(1, hurt_by(true));
            t.add(2, super::surface::nearest(Who::Player, true, 10));
        }
        Kind::Ghast => {
            g.add(5, NGoal::GhastFloat.wrap());
            g.add(7, NGoal::GhastLook.wrap());
            g.add(7, NGoal::GhastShoot { charge: 0 }.wrap());
            // Player at 4 blocks of height at most (own filter, interval 10, sight required).
            t.add(1, super::surface::nearest(Who::Player, true, 10));
        }
        Kind::Strider => strider::register_goals(m),
        _ => {}
    }
}

/// Targeting of the player by a ghast: vertical gap of 4 blocks at most.
pub fn player_target_filter(s: &Server, m: &Mob) -> bool {
    m.kind != Kind::Ghast || (s.pos[1] - m.pos[1]).abs() <= 4.0
}

// ---------- Engine hooks ----------

/// Own path maluses (constructors of the blaze, the wither skeleton, the strider).
pub fn malus(k: Kind, t: PathType) -> Option<f32> {
    use PathType::*;
    Some(match (k, t) {
        (Kind::Blaze, Water) => -1.0,
        (Kind::Blaze, Lava) => 8.0,
        (Kind::Blaze, FireInNeighbor | Fire) => 0.0,
        (Kind::WitherSkeleton, Lava) => 8.0,
        (Kind::Strider, Water) => -1.0,
        (Kind::Strider, Lava | FireInNeighbor | Fire) => 0.0,
        _ => return None,
    })
}

/// Own walk value; `None`: common rule.
pub fn walk_target_value(s: &Server, m: &Mob, p: [i32; 3]) -> Option<f32> {
    match m.kind {
        // The ghast is only a simple mob: zero value everywhere.
        Kind::Ghast => Some(0.0),
        Kind::Strider => Some(strider::walk_target_value(s, m, p)),
        _ => None,
    }
}

/// Own stable destination: lava for the strider.
pub fn stable_destination(s: &Server, m: &Mob, p: [i32; 3]) -> bool {
    m.kind == Kind::Strider && s.bname(p) == "minecraft:lava"
}

/// Own ambient sound; `None`: the sheet's sound.
pub fn ambient_sound(m: &Mob) -> Option<String> {
    match m.kind {
        Kind::Strider => strider::ambient_sound(m).map(Into::into),
        k => info(k).ambient.map(Into::into),
    }
}

/// Own step noise (volume, sound); `None`: common rule.
pub fn step_sound(m: &Mob) -> Option<(&'static str, f32)> {
    match m.kind {
        Kind::Strider => Some((
            if m.lava_height > 0.0 { "minecraft:entity.strider.step_lava" } else { "minecraft:entity.strider.step" },
            1.0,
        )),
        _ => None,
    }
}

/// Next step: strider every 0.6 of distance traveled.
pub fn next_step(m: &Mob) -> Option<f32> {
    (m.kind == Kind::Strider).then_some(m.move_dist + 0.6)
}

/// The wither skeleton refuses the wither effect.
pub fn refuses_effect(k: Kind, id: &str) -> bool {
    k == Kind::WitherSkeleton && id == "minecraft:wither"
}

/// Maximum size of a spawn group (ghast: 1).
pub fn max_cluster(k: Kind) -> Option<i32> {
    (k == Kind::Ghast).then_some(1)
}

/// Despawn far from players: the strider is an animal (never).
pub fn remove_when_far_away(m: &Mob) -> Option<bool> {
    (m.kind == Kind::Strider).then_some(false)
}

/// "In lava" placement of the strider; `None`: ground.
pub fn placement_ok(s: &Server, k: Kind, p: [i32; 3]) -> Option<bool> {
    (k == Kind::Strider).then(|| s.mob_fluid(p) == super::env::Fluid::Lava)
}

/// Obstruction: the strider only looks at entities.
pub fn obstruction_ok(s: &Server, m: &Mob) -> Option<bool> {
    if m.kind != Kind::Strider {
        return None;
    }
    Some(s.unobstructed(&m.bbox(), m.id))
}

/// Own spawn rule (world random source); `None`: common rule.
pub fn spawn_rules(s: &mut Server, k: Kind, p: [i32; 3]) -> Option<bool> {
    let below = [p[0], p[1] - 1, p[2]];
    match k {
        // Difficulty, support block except spawner (light irrelevant).
        Kind::Blaze => Some(s.level.difficulty_id() != 0 && (s.rule_is_spawner() || s.valid_spawn_on(below, k))),
        // Difficulty, 1 chance in 20, then the mobs rule.
        Kind::Ghast => {
            Some(
                s.level.difficulty_id() != 0
                    && s.mobs.rng.next_int_bound(20) == 0
                    && (s.rule_is_spawner() || s.valid_spawn_on(below, k)),
            )
        }
        Kind::Strider => Some(strider::spawn_rule(s, p)),
        _ => None,
    }
}

/// Construction: nothing drawn.
pub fn on_new(m: &mut Mob) {
    if m.kind == Kind::Ghast {
        m.neth.explosion_power = 1;
    }
}

/// Before the common tick (a strider's tick before `super.tick`): sounds, cold.
pub fn pre_tick(s: &mut Server, m: &mut Mob) {
    if m.kind == Kind::Strider {
        strider::pre_tick(s, m);
    }
}

/// End of tick (the strider's floating after `super.tick`).
pub fn post_tick(s: &mut Server, m: &mut Mob) {
    if m.kind == Kind::Strider {
        strider::float(s, m);
    }
}

/// Start of the AI step: slowed fall of the blaze.
pub fn pre_ai_step(m: &mut Mob) {
    if m.kind == Kind::Blaze && !m.on_ground && m.vel[1] < 0.0 {
        m.vel[1] *= 0.6;
    }
}

/// Wanted height of the blaze.
pub fn custom_ai_step(s: &mut Server, m: &mut Mob) {
    if m.kind == Kind::Blaze {
        blaze::custom_ai_step(s, m);
    }
}

/// Own movement control; true if it replaces the common one.
pub fn move_control(s: &Server, m: &mut Mob) -> bool {
    if m.kind == Kind::Ghast {
        ghast::move_control(s, m);
        return true;
    }
    false
}

/// Own movement (`travel`); true if it replaces the common one.
pub fn travel(s: &mut Server, m: &mut Mob) -> bool {
    if m.kind == Kind::Ghast {
        ghast::travel(s, m);
        return true;
    }
    false
}

/// After a successful melee hit: wither effect (10 s) of the wither skeleton.
pub fn on_hit(s: &mut Server, m: &Mob, t: Ent) {
    if m.kind != Kind::WitherSkeleton {
        return;
    }
    match t {
        Ent::Player => {
            s.give_player_effect_from("minecraft:wither", 200, 0, Some(m));
        }
        Ent::Mob(id) => {
            if let Some(i) = s.mob_index(id)
                && let Some(mut o) = s.mobs.list[i].take()
            {
                s.give_mob_effect(&mut o, "minecraft:wither", 200, 0);
                s.mobs.list[i] = Some(o);
            }
        }
    }
}

/// Arrow set on fire for 100 s (wither skeleton holding a bow).
pub fn flaming_arrows(k: Kind) -> bool {
    k == Kind::WitherSkeleton
}

/// The ghast takes 1,000 damage from a big fireball returned by a player, despite its
/// fire immunity (overridden).
/// True when the direct source is a big fireball (direct hit or explosion) and
/// the author the player, whatever the damage type.
pub fn reflected_fireball(m: &Mob, direct_large: bool, by: Option<Ent>) -> bool {
    m.kind == Kind::Ghast && direct_large && by == Some(Ent::Player)
}

/// Finalization before the mob's (strider: mounts and own group).
pub fn finalize_pre(s: &mut Server, m: &mut Mob, group: &mut super::spawn::Group) {
    if m.kind == Kind::Strider {
        strider::finalize(s, m, group);
    }
}

/// Finalization after the common spawn finalization (wither skeleton: sword, pickup, damage 4).
pub fn finalize_post(s: &mut Server, m: &mut Mob, special: f32) {
    if m.kind != Kind::WitherSkeleton {
        return;
    }
    // Own default equipment: stone sword only (neither armor nor draw);
    // no equipment enchantment.
    m.equipment[0] = Some(crate::player::Stack { id: "minecraft:stone_sword".into(), count: 1, components: None });
    super::brains::reassess_weapon(s, m);
    m.can_pickup = s.mobs.rng.next_float() < 0.55 * special;
    s.halloween_head(m);
    m.attrs.attack.base = 4.0;
    super::brains::reassess_weapon(s, m);
}

impl Server {
    /// Right click on a mob of the family; `None`: common handling.
    pub(crate) fn nether_interact(&mut self, m: &mut Mob, slot: usize, item: &str) -> Option<bool> {
        (m.kind == Kind::Strider).then(|| strider::interact(self, m, slot, item))
    }
}
