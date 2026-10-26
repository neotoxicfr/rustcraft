//! Aquatic hostiles and monument (family F08, `server-mobs.md` § 4): drowned, guardian,
//! elder guardian. Stat sheets, own state, metadata, NBT, goals, spawning (rules, placement
//! in water, replacement of lists by structures) and dispatch of the hooks called
//! by the common engine (`mobs/`).

pub(crate) mod drowned;
pub(crate) mod guardian;
pub(crate) mod spawn;
pub(crate) mod splash;
pub(crate) mod trident;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod world_tests;

use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::goals::{Goal, Melee, TBase, Who};
use super::mob::{Ent, Mob};
use super::species::{Info, Kind};
use crate::nbtio;
use crate::server::Server;

// ---------- Species info ----------

const fn monster(name: &'static str, width: f32, height: f32, eye: f32) -> Info {
    let mut i = super::species::info(name, super::species::Category::Monster, width, height, eye);
    i.xp = 5;
    i.tracking = 8;
    i.not_in_peaceful = true;
    i.hurt = "minecraft:entity.hostile.hurt";
    i.death = "minecraft:entity.hostile.death";
    i
}

/// Drowned: zombie attributes, step height 1, burns in sunlight.
static DROWNED: Info = {
    let mut i = monster("minecraft:drowned", 0.6, 1.95, 1.74);
    i.speed = 0.23;
    i.attack = 3.0;
    i.follow = 35.0;
    i.armor = 2.0;
    i.step = 1.0;
    i.ambient = Some("minecraft:entity.drowned.ambient");
    i.hurt = "minecraft:entity.drowned.hurt";
    i.death = "minecraft:entity.drowned.death";
    i.step_sound = Some("minecraft:entity.drowned.step");
    i.burns = true;
    i
};

/// Guardian: HP 30, speed 0.5, damage 6, experience 10, ambient sound every 160.
static GUARDIAN: Info = {
    let mut i = monster("minecraft:guardian", 0.85, 0.85, 0.425);
    i.health = 30.0;
    i.speed = 0.5;
    i.attack = 6.0;
    i.xp = 10;
    i.ambient_interval = 160;
    i.ambient = Some("minecraft:entity.guardian.ambient");
    i.hurt = "minecraft:entity.guardian.hurt";
    i.death = "minecraft:entity.guardian.death";
    i
};

/// Elder guardian: HP 80, speed 0.3 (float), damage 8, client-side tracking 10.
static ELDER_GUARDIAN: Info = {
    let mut i = monster("minecraft:elder_guardian", 1.9975, 1.9975, 0.998_75);
    i.health = 80.0;
    i.speed = 0.3f32 as f64;
    i.attack = 8.0;
    i.xp = 10;
    i.tracking = 10;
    i.ambient_interval = 160;
    i.ambient = Some("minecraft:entity.elder_guardian.ambient");
    i.hurt = "minecraft:entity.elder_guardian.hurt";
    i.death = "minecraft:entity.elder_guardian.death";
    i
};

pub fn info(k: Kind) -> &'static Info {
    match k {
        Kind::Drowned => &DROWNED,
        Kind::Guardian => &GUARDIAN,
        _ => &ELDER_GUARDIAN,
    }
}

/// Species of the family.
pub fn handles(k: Kind) -> bool {
    matches!(k, Kind::Drowned | Kind::Guardian | Kind::ElderGuardian)
}

/// Guardian or elder guardian (same class in the game).
pub fn is_guardian(k: Kind) -> bool {
    matches!(k, Kind::Guardian | Kind::ElderGuardian)
}

/// "In water" placement.
pub fn spawns_in_water(k: Kind) -> bool {
    handles(k)
}

/// Breathes underwater (`#can_breathe_under_water`: undead, guardians).
pub fn breathes_underwater(k: Kind) -> bool {
    is_guardian(k)
}

// ---------- Own state ----------

/// State of the family's species (`aqua` field of the mob).
#[derive(Debug, Clone, Default)]
pub struct Aqua {
    /// Drowned: "looks for land" (goal "surface"), swims (shared flag 0x10).
    pub searching_for_land: bool,
    pub swimming: bool,
    /// Guardian: moving (index 16), beam target (index 17: network id).
    pub moving: bool,
    pub beam: Option<Ent>,
    pub beam_id: i32,
    /// Guardian: wandering restarted by force, up to a position.
    pub stroll_trigger: bool,
    /// Elder guardian: residence zone (center, radius).
    pub home: Option<([i32; 3], i32)>,
}

/// Construction: own navigation and persistence.
pub fn on_new(m: &mut Mob) {
    match m.kind {
        Kind::Drowned => m.nav.amphibious = true,
        Kind::Guardian => m.nav.swim = true,
        Kind::ElderGuardian => {
            m.nav.swim = true;
            m.persistent = true;
        }
        _ => {}
    }
}

/// Own metadata of the guardians (the drowned takes over the zombie's).
pub fn define_data(m: &mut Mob) {
    m.define(16, Meta::Bool(false));
    m.define(17, Meta::Int(0));
}

/// Own values, called after the common values.
pub fn sync_data(m: &mut Mob) {
    match m.kind {
        Kind::Drowned => {
            // Swimming (shared flag 0x10) and raised trident (item in hand in use).
            if m.aqua.swimming
                && let Some(Meta::Byte(f)) = m.get(0).cloned()
            {
                m.set(0, Meta::Byte(f | 0x10));
            }
            m.set(8, Meta::Byte(i8::from(m.use_ticks >= 0)));
        }
        Kind::Guardian | Kind::ElderGuardian => {
            m.set(16, Meta::Bool(m.aqua.moving));
            let id = if m.aqua.beam.is_some() { m.aqua.beam_id } else { 0 };
            m.set(17, Meta::Int(id));
        }
        _ => {}
    }
}

/// Own NBT read: the elder guardian's residence zone (`home_pos`, `home_radius`).
pub fn load(m: &mut Mob, e: &Nbt) {
    if m.kind == Kind::ElderGuardian {
        let pos = match e.get("home_pos") {
            Some(Nbt::IntArray(v)) if v.len() == 3 => Some([v[0], v[1], v[2]]),
            _ => None,
        };
        let r = nbtio::int(e, "home_radius").map_or(-1, |v| v as i32);
        m.aqua.home = pos.filter(|_| r >= 0).map(|p| (p, r));
    }
}

pub fn save(m: &Mob, e: &mut Nbt) {
    if let Some((p, r)) = m.aqua.home {
        nbtio::set(e, "home_pos", Nbt::IntArray(vec![p[0], p[1], p[2]]));
        nbtio::set(e, "home_radius", Nbt::Int(r));
    }
}

// ---------- Goals ----------

/// Condition added to a common goal (game subclasses that filter the start condition).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    /// Drowned: the target is valid (night, or target in water) — use and pursuit.
    OkTarget,
    /// Drowned: the targeted player is valid (targeting filter, use only).
    OkTargetFind,
    /// Drowned: hurt by another drowned, no retaliation.
    NotByDrowned,
    /// Guardian: player more than 3 blocks away (targeting filter).
    BeyondThree,
}

/// Own goals (`Goal::Aqua`).
#[derive(Debug, Clone)]
pub enum AquaGoal {
    /// Filtered common goal.
    Gated { inner: Box<Goal>, gate: Gate },
    /// Drowned: go to the water (by day, out of water).
    GoToWater { wanted: [f64; 3] },
    /// Drowned: ranged trident (speed 1, interval 40, radius 10).
    Trident { target: Option<Ent>, attack_time: i32, see_time: i32 },
    /// Drowned: go to the beach (search 8 × 2).
    GoToBeach { next_start: i32, try_ticks: i32, max_stay: i32, pos: [i32; 3] },
    /// Drowned: rise toward the surface at night.
    SwimUp { stuck: bool },
    /// Guardian: beam.
    Beam { attack_time: i32 },
    /// Guardian: return toward the residence zone (elder guardian).
    TowardsHome { wanted: [f64; 3] },
    /// Guardian: wandering (interval 80, elder 400), restarted by force.
    Stroll { interval: i32, wanted: [f64; 3] },
    /// Guardian: look at another guardian (12 blocks, probability 0.01).
    LookAtGuardian { at: Option<i32>, time: i32 },
}

const MOVE: u8 = 1;
const LOOK: u8 = 2;
const JUMP: u8 = 4;

impl AquaGoal {
    pub fn flags(&self) -> u8 {
        match self {
            AquaGoal::Gated { inner, .. } => inner.flags(),
            AquaGoal::GoToWater { .. } => MOVE,
            AquaGoal::GoToBeach { .. } => MOVE | JUMP,
            AquaGoal::SwimUp { .. } => 0,
            AquaGoal::Trident { .. } | AquaGoal::Beam { .. } => MOVE | LOOK,
            AquaGoal::TowardsHome { .. } | AquaGoal::Stroll { .. } => MOVE | LOOK,
            AquaGoal::LookAtGuardian { .. } => LOOK,
        }
    }

    pub fn every_tick(&self) -> bool {
        match self {
            AquaGoal::Gated { inner, .. } => inner.every_tick(),
            AquaGoal::Trident { .. } | AquaGoal::Beam { .. } | AquaGoal::GoToBeach { .. } => true,
            _ => false,
        }
    }
}

fn gated(inner: Goal, gate: Gate) -> Goal {
    Goal::Aqua(AquaGoal::Gated { inner: Box::new(inner), gate })
}

fn nearest(who: Who, must_see: bool) -> Goal {
    Goal::Nearest {
        base: TBase::new(must_see, false),
        who,
        interval: super::mth::reduced(10),
        target: None,
        angry_only: false,
        non_tame: false,
    }
}

/// Goals and targets in the game's add order.
pub fn register_goals(m: &mut Mob) {
    let g = &mut m.goals;
    let t = &mut m.targets;
    match m.kind {
        Kind::Drowned => {
            // Zombie goals kept by the drowned (including egg trampling), then its own.
            g.add(
                4,
                Goal::Surf(super::goals::surface_goals::SGoal::move_to_block(
                    super::goals::surface_goals::Mtb::RemoveEgg,
                    1.0,
                    24,
                    3,
                    0,
                )),
            );
            g.add(8, super::surface::look_player(8.0));
            g.add(8, super::surface::look_around());
            g.add(1, Goal::Aqua(AquaGoal::GoToWater { wanted: [0.0; 3] }));
            g.add(2, Goal::Aqua(AquaGoal::Trident { target: None, attack_time: -1, see_time: 0 }));
            g.add(2, gated(super::surface::melee(1.0, false, Melee::Zombie { raise: 0 }), Gate::OkTarget));
            g.add(5, Goal::Aqua(AquaGoal::GoToBeach { next_start: 0, try_ticks: 0, max_stay: 0, pos: [0; 3] }));
            g.add(6, Goal::Aqua(AquaGoal::SwimUp { stuck: false }));
            g.add(7, Goal::Stroll { speed: 1.0, interval: 120, prob: None, wanted: [0.0; 3] });
            t.add(1, gated(Goal::HurtBy { base: TBase::new(true, false), alert: true, stamp: 0 }, Gate::NotByDrowned));
            t.add(2, gated(nearest(Who::Player, true), Gate::OkTargetFind));
            t.add(3, nearest(Who::Villager, false));
            t.add(3, nearest(Who::IronGolem, true));
            t.add(3, nearest(Who::Axolotl, true));
            t.add(5, nearest(Who::BabyTurtleOnLand, true));
        }
        _ => {
            let interval = if m.kind == Kind::ElderGuardian { 400 } else { 80 };
            g.add(4, Goal::Aqua(AquaGoal::Beam { attack_time: 0 }));
            g.add(5, Goal::Aqua(AquaGoal::TowardsHome { wanted: [0.0; 3] }));
            g.add(7, Goal::Aqua(AquaGoal::Stroll { interval, wanted: [0.0; 3] }));
            g.add(8, super::surface::look_player(8.0));
            g.add(8, Goal::Aqua(AquaGoal::LookAtGuardian { at: None, time: 0 }));
            g.add(9, super::surface::look_around());
            t.add(1, gated(nearest(Who::Player, true), Gate::BeyondThree));
        }
    }
}

// ---------- Engine hooks ----------

/// Ambient sound (aquatic or land variant).
pub fn ambient_sound(m: &Mob) -> Option<String> {
    let water = m.in_water;
    Some(
        match (m.kind, water) {
            (Kind::Drowned, true) => "minecraft:entity.drowned.ambient_water",
            (Kind::Drowned, false) => "minecraft:entity.drowned.ambient",
            (Kind::Guardian, true) => "minecraft:entity.guardian.ambient",
            (Kind::Guardian, false) => "minecraft:entity.guardian.ambient_land",
            (_, true) => "minecraft:entity.elder_guardian.ambient",
            (_, false) => "minecraft:entity.elder_guardian.ambient_land",
        }
        .to_owned(),
    )
}

/// Hurt and death sounds (aquatic or land variant).
pub fn hurt_sound(m: &Mob, death: bool) -> Option<&'static str> {
    if !handles(m.kind) {
        return None;
    }
    let w = m.in_water;
    Some(match (m.kind, death, w) {
        (Kind::Drowned, false, true) => "minecraft:entity.drowned.hurt_water",
        (Kind::Drowned, false, false) => "minecraft:entity.drowned.hurt",
        (Kind::Drowned, true, true) => "minecraft:entity.drowned.death_water",
        (Kind::Drowned, true, false) => "minecraft:entity.drowned.death",
        (Kind::Guardian, false, true) => "minecraft:entity.guardian.hurt",
        (Kind::Guardian, false, false) => "minecraft:entity.guardian.hurt_land",
        (Kind::Guardian, true, true) => "minecraft:entity.guardian.death",
        (Kind::Guardian, true, false) => "minecraft:entity.guardian.death_land",
        (_, false, true) => "minecraft:entity.elder_guardian.hurt",
        (_, false, false) => "minecraft:entity.elder_guardian.hurt_land",
        (_, true, true) => "minecraft:entity.elder_guardian.death",
        (_, true, false) => "minecraft:entity.elder_guardian.death_land",
    })
}

/// Pushed by the current: the drowned that swims is not.
pub fn pushed_by_fluid(m: &Mob) -> bool {
    !(m.kind == Kind::Drowned && m.aqua.swimming)
}

/// Walk value: guardians, water = 10 + light cost.
pub fn walk_target_value(s: &Server, m: &Mob, p: [i32; 3]) -> Option<f32> {
    if !is_guardian(m.kind) || s.mob_fluid(p) != super::env::Fluid::Water {
        return None;
    }
    Some(10.0 + (s.magic_light(p) - 0.5))
}

/// The obstruction test at spawn for the family's species: only no entity collision
/// (liquid is allowed).
pub fn spawn_obstruction_skips_fluid(k: Kind) -> bool {
    handles(k)
}

/// Swim update after the fluids.
pub fn update_swimming(s: &Server, m: &mut Mob) {
    if m.kind == Kind::Drowned {
        let under = m.was_eyes_in_water && m.in_water;
        m.aqua.swimming = !m.no_ai && under && drowned::wants_to_swim(s, m);
    }
}

/// Own movement in water; true if it replaces the common one.
pub fn travel_in_water(s: &mut Server, m: &mut Mob) -> bool {
    if !handles(m.kind) || !m.in_water {
        return false;
    }
    match m.kind {
        Kind::Drowned => {
            let under = m.was_eyes_in_water && m.in_water;
            if !(under && drowned::wants_to_swim(s, m)) {
                return false;
            }
            move_relative(m, 0.01);
            let v = m.vel;
            s.mob_move(m, v);
            m.vel = m.vel.map(|x| x * 0.9);
        }
        _ => {
            move_relative(m, 0.1);
            let v = m.vel;
            s.mob_move(m, v);
            m.vel = m.vel.map(|x| x * 0.9);
            if !m.aqua.moving && m.target.is_none() {
                m.vel[1] -= 0.005;
            }
        }
    }
    true
}

/// Relative movement (acceleration relative to the yaw).
pub(crate) fn move_relative(m: &mut Mob, speed: f32) {
    let input = [f64::from(m.xxa), f64::from(m.yya), f64::from(m.zza)];
    let len = input.iter().map(|x| x * x).sum::<f64>();
    if len < 1.0e-7 {
        return;
    }
    let v = if len > 1.0 { input.map(|x| x / len.sqrt()) } else { input };
    let v = v.map(|x| x * f64::from(speed));
    let (sn, c) = (super::mth::sin(m.yaw * super::mth::DEG), super::mth::cos(m.yaw * super::mth::DEG));
    m.vel[0] += v[0] * f64::from(c) - v[2] * f64::from(sn);
    m.vel[1] += v[1];
    m.vel[2] += v[2] * f64::from(c) + v[0] * f64::from(sn);
}

/// Own movement control; true if it replaces the common one.
pub fn move_control(s: &Server, m: &mut Mob) -> bool {
    match m.kind {
        Kind::Drowned => drowned::move_control(s, m),
        Kind::Guardian | Kind::ElderGuardian => {
            guardian::move_control(s, m);
            true
        }
        _ => false,
    }
}

/// Start of the AI step (guardians: air, jumps out of water, body turned toward the beam).
pub fn pre_ai_step(m: &mut Mob) {
    if is_guardian(m.kind) {
        guardian::pre_ai_step(m);
    }
}

/// The species-specific server AI step (elder guardian: mining fatigue, residence zone).
pub fn custom_ai_step(s: &mut Server, m: &mut Mob) {
    if m.kind == Kind::ElderGuardian {
        guardian::elder_custom_ai(s, m);
    }
}

/// Start of the server-side damage computation (guardians: thorns if motionless, restarted wandering).
pub fn before_hurt(s: &mut Server, m: &mut Mob, ty: &str, by: Option<Ent>) {
    if is_guardian(m.kind) {
        guardian::before_hurt(s, m, ty, by);
    }
}

/// Does the filter accept the found or aimed target?
fn gate_ok(s: &Server, m: &Mob, gate: Gate, inner: &Goal) -> bool {
    match gate {
        Gate::OkTarget => drowned::ok_target(s, m.target),
        Gate::OkTargetFind => match inner {
            Goal::Nearest { target, .. } => drowned::ok_target(s, *target),
            _ => true,
        },
        Gate::NotByDrowned => true,
        Gate::BeyondThree => match inner {
            Goal::Nearest { target: Some(t), .. } => s.ent_pos(*t).is_some_and(|p| super::mth::dist2(p, m.pos) > 9.0),
            _ => false,
        },
    }
}

/// Retaliation refused: the last attacker is a drowned.
fn hurt_by_drowned(s: &Server, m: &Mob) -> bool {
    match m.last_hurt_by_mob {
        Some((Ent::Mob(id), _)) => s.mob_ref(id).is_some_and(|o| o.kind == Kind::Drowned),
        _ => false,
    }
}

impl Server {
    pub(crate) fn aqua_can_use(&mut self, g: &mut AquaGoal, m: &mut Mob) -> bool {
        match g {
            AquaGoal::Gated { inner, gate } => {
                if *gate == Gate::NotByDrowned && hurt_by_drowned(self, m) {
                    return false;
                }
                let ok = self.goal_can_use(inner, m);
                if !ok {
                    return false;
                }
                let pass = gate_ok(self, m, *gate, inner);
                if !pass && let Goal::Nearest { target, .. } = &mut **inner {
                    *target = None;
                }
                pass
            }
            _ if m.kind == Kind::Drowned => self.drowned_can_use(g, m),
            _ => self.guardian_can_use(g, m),
        }
    }

    pub(crate) fn aqua_can_continue(&mut self, g: &mut AquaGoal, m: &mut Mob) -> bool {
        match g {
            AquaGoal::Gated { inner, gate } => {
                self.goal_can_continue(inner, m) && (*gate != Gate::OkTarget || drowned::ok_target(self, m.target))
            }
            _ if m.kind == Kind::Drowned => self.drowned_can_continue(g, m),
            _ => self.guardian_can_continue(g, m),
        }
    }

    pub(crate) fn aqua_start(&mut self, g: &mut AquaGoal, m: &mut Mob) {
        match g {
            AquaGoal::Gated { inner, .. } => self.goal_start(inner, m),
            _ if m.kind == Kind::Drowned => self.drowned_start(g, m),
            _ => self.guardian_start(g, m),
        }
    }

    pub(crate) fn aqua_stop(&mut self, g: &mut AquaGoal, m: &mut Mob) {
        match g {
            AquaGoal::Gated { inner, .. } => self.goal_stop(inner, m),
            _ if m.kind == Kind::Drowned => self.drowned_stop(g, m),
            _ => self.guardian_stop(g, m),
        }
    }

    pub(crate) fn aqua_tick(&mut self, g: &mut AquaGoal, m: &mut Mob) {
        match g {
            AquaGoal::Gated { inner, .. } => self.goal_tick(inner, m),
            _ if m.kind == Kind::Drowned => self.drowned_tick(g, m),
            _ => self.guardian_tick(g, m),
        }
    }
}

/// World state of the family: deferred thorns (thrown tridents are common projectiles).
#[derive(Debug, Clone, Default)]
pub struct AquaWorld {
    /// Thorns owed to an attacker taken out of the list (attacker, guardian, guardian's position).
    pub thorns: Vec<(i32, i32, [f64; 3])>,
}
