//! Cube family (mission F03, `server-mobs.md` § 4.13): slime, magma cube and sulfur
//! cube, simulated like the official server 26.3. Variable size that sets the box, HP,
//! speed and damage; movement by jumps; squish on landing; contact damage; split on
//! death; spawning in slime chunks and in swamps depending on the moon.
//! Sulfur cube: small ones, absorbed item (pack archetypes), shearing, bucket, fuse, own knockback.

use hashbrown::HashSet;
use std::sync::{Mutex, OnceLock};

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;
use serde_json::Value;

use super::goals::{Goal, JUMP, LOOK, MOVE};
use super::mob::{Ent, Mob, MoveOp};
use super::mth::{dist2, reduced, rotlerp_360};
use super::spawn::Group;
use super::species::Kind;
use crate::nbtio::{self, int};
use crate::player::Stack;
use crate::server::Server;

/// Moon phase → brightness; the surface slime chance
/// is half of it.
const MOON: [f32; 8] = [1.0, 0.75, 0.5, 0.25, 0.0, 0.25, 0.5, 0.75];
const SLIME_SALT: i64 = 987_234_911;
/// "Body" equipment slot (item absorbed by the sulfur cube).
const BODY: usize = 6;

/// Sulfur cube archetype (`sulfur_cube_archetype/*.json`).
#[derive(Debug, Clone)]
pub struct Archetype {
    pub id: String,
    /// Item tag (`#…`) or ids.
    pub items: Vec<String>,
    /// (attribute, id, amount, operation 0/1/2).
    pub mods: Vec<(String, &'static str, f64, i32)>,
    pub buoyant: bool,
    /// (power, fire, fuse).
    pub explosion: Option<(i32, bool, i32)>,
    /// (damage type, amount, attributed to the source).
    pub contact: Option<(String, Value, bool)>,
    pub kb: (f32, f32),
    pub sound: Sounds,
}

#[derive(Debug, Clone)]
pub struct Sounds {
    pub hit: String,
    pub push: String,
    pub push_threshold: f32,
    pub push_cooldown: f32,
}

impl Default for Sounds {
    fn default() -> Sounds {
        Sounds {
            hit: "minecraft:entity.sulfur_cube.regular.hit".into(),
            push: "minecraft:entity.sulfur_cube.regular.push".into(),
            push_threshold: 0.2,
            push_cooldown: 0.5,
        }
    }
}

/// Own state of a cube.
#[derive(Debug, Clone)]
pub struct CubeState {
    pub size: i32,
    pub was_on_ground: bool,
    pub squish: f32,
    pub o_squish: f32,
    pub target_squish: f32,
    /// Movement control: wanted direction, jump delay, aggressive.
    pub dir: f32,
    pub jump_delay: i32,
    pub aggressive: bool,
    // Sulfur cube.
    pub age_locked: bool,
    pub forced_age: i32,
    pub forced_age_timer: i32,
    pub pickup_timer: i32,
    pub push_sound_cd: i32,
    pub from_bucket: bool,
    pub fuse: i32,
    pub max_fuse: i32,
    pub invulnerable: bool,
    pub floats: bool,
    pub explosion: Option<(i32, bool, i32)>,
    pub contacts: Vec<(String, Value, bool)>,
    pub kb: (f32, f32),
    pub sounds: Sounds,
    /// Archetypes applied (to remove their modifiers).
    pub applied: Vec<String>,
    /// Body item seen at the last tick.
    pub last_body: Option<Stack>,
}

impl Default for CubeState {
    fn default() -> CubeState {
        CubeState {
            size: 1,
            was_on_ground: false,
            squish: 0.0,
            o_squish: 0.0,
            target_squish: 0.0,
            // `180 × yaw / π` at construction, yaw still zero: 0 (game default, kept).
            dir: 0.0,
            jump_delay: 0,
            aggressive: false,
            age_locked: false,
            forced_age: 0,
            forced_age_timer: 0,
            pickup_timer: 0,
            push_sound_cd: 0,
            from_bucket: false,
            fuse: -1,
            max_fuse: -1,
            invulnerable: false,
            floats: false,
            explosion: None,
            contacts: Vec::new(),
            kb: (0.33, 0.06),
            sounds: Sounds::default(),
            applied: Vec::new(),
            last_body: None,
        }
    }
}

/// World data for the cubes (seed, known player speed, archetypes).
#[derive(Debug, Default)]
pub struct CubesWorld {
    pub seed: Option<i64>,
    pub last_player: Option<[f64; 3]>,
    pub player_speed: [f64; 3],
    pub archetypes: Option<Vec<Archetype>>,
}

/// Cube goals (and sulfur cube).
#[derive(Debug, Clone)]
pub enum CubeGoal {
    Float,
    Attack {
        tired: i32,
    },
    RandomDir {
        chosen: f32,
        next: i32,
    },
    KeepJumping,
    /// Attraction (without path): small ones by the slimeball, adults by an absorbable block.
    Tempt {
        calm: i32,
    },
    SearchItems {
        target: Option<i32>,
    },
}

impl CubeGoal {
    pub fn flags(&self) -> u8 {
        match self {
            CubeGoal::Float | CubeGoal::KeepJumping => JUMP | MOVE,
            CubeGoal::Attack { .. } | CubeGoal::RandomDir { .. } | CubeGoal::Tempt { .. } => LOOK,
            CubeGoal::SearchItems { .. } => LOOK,
        }
    }

    pub fn every_tick(&self) -> bool {
        matches!(self, CubeGoal::Float | CubeGoal::Attack { .. })
    }
}

/// Modifier ids coming from the data, kept for the lifetime of the process.
fn intern(s: &str) -> &'static str {
    static SET: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut set = SET.get_or_init(|| Mutex::new(HashSet::new())).lock().unwrap_or_else(|e| e.into_inner());
    if let Some(v) = set.get(s) {
        return v;
    }
    let v: &'static str = Box::leak(s.to_owned().into_boxed_str());
    set.insert(v);
    v
}

/// Seed of the slime chunk: products in `int` that overflow, like the game.
pub fn is_slime_chunk(seed: i64, x: i32, z: i32) -> bool {
    let a = i64::from(x.wrapping_mul(x).wrapping_mul(4_987_142));
    let b = i64::from(x.wrapping_mul(5_947_611));
    let c = i64::from(z.wrapping_mul(z)).wrapping_mul(4_392_871);
    let d = i64::from(z.wrapping_mul(389_711));
    let s = seed.wrapping_add(a).wrapping_add(b).wrapping_add(c).wrapping_add(d) ^ SLIME_SALT;
    super::jrand::JRand::new(s).next_int_bound(10) == 0
}

/// Goals in the game's order (constructor: float, direction, jump, then behaviors).
pub fn register_goals(m: &mut Mob) {
    m.nav.can_float = true;
    let g = &mut m.goals;
    g.add(1, Goal::Cube(CubeGoal::Float));
    g.add(4, Goal::Cube(CubeGoal::RandomDir { chosen: 0.0, next: 0 }));
    g.add(5, Goal::Cube(CubeGoal::KeepJumping));
    match m.kind {
        Kind::SulfurCube => {
            g.add(2, Goal::Cube(CubeGoal::Tempt { calm: 0 }));
            g.add(3, Goal::Cube(CubeGoal::SearchItems { target: None }));
        }
        _ => {
            g.add(2, Goal::Cube(CubeGoal::Attack { tired: 0 }));
            let t = &mut m.targets;
            t.add(1, cube_nearest(super::goals::Who::Player, true));
            t.add(3, cube_nearest(super::goals::Who::IronGolem, true));
        }
    }
}

fn cube_nearest(who: super::goals::Who, must_see: bool) -> Goal {
    Goal::Nearest {
        base: super::goals::TBase::new(must_see, false),
        who,
        interval: reduced(10),
        target: None,
        angry_only: false,
        non_tame: false,
    }
}

// ---------- Entity's own state ----------

impl Mob {
    /// Dimensions by size (`type × size`, without age scale).
    pub fn cube_dims(&self) -> (f32, f32, f32) {
        let i = self.kind.info();
        let s = self.cube.size as f32;
        (i.width * s, i.height * s, i.eye * s)
    }

    pub fn is_tiny(&self) -> bool {
        self.cube.size <= 1
    }

    pub fn has_body_item(&self) -> bool {
        self.equipment[BODY].is_some()
    }

    /// Species-specific persistence (outside the `persistent` flag): a sulfur cube carrying
    /// a block or coming out of a bucket does not despawn and does not count toward the spawn cap.
    /// (Leash and mount are not modeled yet.)
    pub fn requires_custom_persistence(&self) -> bool {
        // F04: a passenger does not despawn.
        self.mount.vehicle.is_some()
            || self.kind == Kind::SulfurCube && (self.has_body_item() || self.cube.from_bucket)
            // F07: raid raider.
            || super::illagers::custom_persistence(self)
            // F09: fish out of a bucket, tamed nautilus.
            || super::sealife::requires_persistence(self)
            // F10: bucket axolotl, tadpole.
            || super::brainy::requires_persistence(self)
    }

    /// Breathing underwater: the sulfur cube loaded with a block does not drown.
    pub fn can_breathe_underwater(&self) -> bool {
        // Tag `#can_breathe_under_water` (turtle included).
        self.kind.breathes_underwater()
            || (self.kind == Kind::SulfurCube && self.has_body_item())
            // Water breathing effect (witch's potion).
            || self.x.effects.iter().any(|e| e.id == "minecraft:water_breathing")
    }

    /// Capped size, HP, speed, damage, armor, experience.
    pub fn set_size(&mut self, size: i32, update_health: bool) {
        let actual = size.clamp(1, 127);
        self.cube.size = actual;
        let health = if self.kind == Kind::SulfurCube { 4 * actual } else { actual * actual };
        self.attrs.max_health.base = f64::from(health);
        self.attrs.speed.base = f64::from(0.2f32 + 0.1f32 * actual as f32);
        if update_health {
            self.health = self.max_health();
        }
        match self.kind {
            Kind::Slime => self.attrs.attack.base = f64::from(actual),
            Kind::MagmaCube => {
                self.attrs.attack.base = f64::from(actual);
                // The uncapped parameter, like the game.
                self.attrs.armor.base = f64::from(size.wrapping_mul(3));
            }
            Kind::SulfurCube if update_health && size == 1 && !self.is_baby() => self.set_baby(true),
            _ => {}
        }
    }

    /// The age setting of a sulfur cube mob (boundary: adult size).
    pub fn cube_set_age(&mut self, age: i32) {
        let old = self.age;
        self.age = age;
        if (old < 0 && age >= 0) || (old >= 0 && age < 0) {
            // Grown up again, size 2.
            if age >= 0 {
                self.set_size(2, true);
            }
        }
    }

    pub fn set_baby(&mut self, baby: bool) {
        if self.kind == Kind::SulfurCube {
            self.cube_set_age(if baby { -24000 } else { 0 });
        }
    }

    pub fn hurt_sound(&self) -> &'static str {
        let tiny = self.is_tiny();
        match self.kind {
            Kind::Slime => {
                if tiny {
                    "minecraft:entity.slime.hurt_small"
                } else {
                    "minecraft:entity.slime.hurt"
                }
            }
            Kind::MagmaCube => {
                if tiny {
                    "minecraft:entity.magma_cube.hurt_small"
                } else {
                    "minecraft:entity.magma_cube.hurt"
                }
            }
            Kind::SulfurCube => {
                if tiny {
                    "minecraft:entity.small_sulfur_cube.hurt"
                } else {
                    "minecraft:entity.sulfur_cube.hurt"
                }
            }
            _ => self.kind.info().hurt,
        }
    }

    pub fn death_sound(&self) -> &'static str {
        let tiny = self.is_tiny();
        match self.kind {
            Kind::Slime => {
                if tiny {
                    "minecraft:entity.slime.death_small"
                } else {
                    "minecraft:entity.slime.death"
                }
            }
            Kind::MagmaCube => {
                if tiny {
                    "minecraft:entity.magma_cube.death_small"
                } else {
                    "minecraft:entity.magma_cube.death"
                }
            }
            Kind::SulfurCube => {
                if tiny {
                    "minecraft:entity.small_sulfur_cube.death"
                } else {
                    "minecraft:entity.sulfur_cube.death"
                }
            }
            _ => self.kind.info().death,
        }
    }

    fn squish_sound(&self) -> &'static str {
        let tiny = self.is_tiny();
        match self.kind {
            Kind::Slime if tiny => "minecraft:entity.slime.squish_small",
            Kind::Slime => "minecraft:entity.slime.squish",
            Kind::MagmaCube if tiny => "minecraft:entity.magma_cube.squish_small",
            Kind::MagmaCube => "minecraft:entity.magma_cube.squish",
            _ if tiny => "minecraft:entity.small_sulfur_cube.squish",
            _ if self.has_body_item() => "minecraft:entity.sulfur_cube.bounce",
            _ => "minecraft:entity.sulfur_cube.squish",
        }
    }

    fn jump_sound(&self) -> &'static str {
        let tiny = self.is_tiny();
        match self.kind {
            Kind::Slime if tiny => "minecraft:entity.slime.jump_small",
            Kind::Slime => "minecraft:entity.slime.jump",
            Kind::MagmaCube => "minecraft:entity.magma_cube.jump",
            _ if tiny => "minecraft:entity.small_sulfur_cube.jump",
            _ => "minecraft:entity.sulfur_cube.jump",
        }
    }

    /// Volume of the species' sounds.
    pub fn sound_volume(&self) -> f32 {
        if self.kind.is_cube() {
            0.4 * self.cube.size as f32
        } else {
            // F14: happy ghast (small 1, adult 4).
            super::recent::sound_volume(self).unwrap_or_else(|| self.kind.info().volume)
        }
    }

    /// Sound source (5 hostile, 6 neutral): the sulfur cube is neutral.
    pub fn sound_category(&self) -> i32 {
        if self.kind.is_monster() && self.kind != Kind::SulfurCube { 5 } else { 6 }
    }

    pub(crate) fn cube_sync_data(&mut self) {
        self.set(18, Meta::Int(self.cube.size));
        if self.kind == Kind::SulfurCube {
            self.set(17, Meta::Bool(self.cube.age_locked));
            self.set(19, Meta::Bool(self.cube.from_bucket));
            self.set(20, Meta::Int(self.cube.max_fuse));
        }
    }

    /// Size read before the attributes.
    pub(crate) fn cube_load_size(&mut self, e: &Nbt) {
        let size = int(e, "Size").unwrap_or(0) as i32;
        self.set_size(size.wrapping_add(1), false);
    }

    pub(crate) fn cube_load(&mut self, e: &Nbt) {
        let flag = |k: &str| int(e, k).is_some_and(|v| v != 0);
        self.cube.was_on_ground = flag("wasOnGround");
        if self.kind == Kind::SulfurCube {
            self.cube.pickup_timer = int(e, "pickup_timer").unwrap_or(0) as i32;
            self.cube.from_bucket = flag("from_bucket");
            self.cube.fuse = int(e, "fuse").map_or(-1, |v| v as i32);
            self.cube.max_fuse = self.cube.fuse;
            self.cube.forced_age = int(e, "ForcedAge").unwrap_or(0) as i32;
            self.cube.age_locked = flag("AgeLocked");
        } else {
            // Slime and magma: never small, age not read.
            self.age = 0;
        }
    }

    pub(crate) fn cube_save(&self, e: &mut Nbt) {
        nbtio::set(e, "Size", Nbt::Int(self.cube.size - 1));
        nbtio::set(e, "wasOnGround", Nbt::Byte(i8::from(self.cube.was_on_ground)));
        if self.kind == Kind::SulfurCube {
            nbtio::set(e, "ForcedAge", Nbt::Int(self.cube.forced_age));
            nbtio::set(e, "AgeLocked", Nbt::Byte(i8::from(self.cube.age_locked)));
            nbtio::remove(e, "InLove");
            nbtio::set(e, "pickup_timer", Nbt::Int(self.cube.pickup_timer));
            nbtio::set(e, "from_bucket", Nbt::Byte(i8::from(self.cube.from_bucket)));
            nbtio::set(e, "fuse", Nbt::Int(self.cube.fuse));
        }
    }
}

/// Value of an attribute capped to its range.
fn ranged(v: f64, lo: f64, hi: f64) -> f64 {
    if v.is_nan() { lo } else { v.clamp(lo, hi) }
}

impl Mob {
    pub fn bounciness(&self) -> f64 {
        ranged(self.attrs.bounciness.value(), 0.0, 1.0)
    }

    pub fn friction_modifier(&self) -> f32 {
        ranged(self.attrs.friction_mod.value(), 0.0, 2048.0) as f32
    }

    pub fn air_drag_modifier(&self) -> f32 {
        ranged(self.attrs.air_drag_mod.value(), 0.0, 2048.0) as f32
    }

    pub fn knockback_resistance(&self) -> f64 {
        ranged(self.attrs.knockback_resistance.value(), -2.0, 1.0)
    }

    /// Vertical air friction of the "omnidirectional" kind (full sulfur cube).
    pub fn omnidirectional_air(&self) -> bool {
        self.kind == Kind::SulfurCube && self.has_body_item()
    }

    pub(crate) fn can_deal_damage(&self) -> bool {
        match self.kind {
            Kind::Slime => !self.is_tiny() && !self.no_ai,
            Kind::MagmaCube => !self.no_ai,
            _ => false,
        }
    }

    pub(crate) fn attack_damage(&self) -> f32 {
        let a = self.attrs.attack.value() as f32;
        if self.kind == Kind::MagmaCube { a + 2.0 } else { a }
    }

    fn set_direction(&mut self, yaw: f32, aggressive: bool) {
        self.cube.dir = yaw;
        self.cube.aggressive = aggressive;
    }

    fn set_wanted_movement(&mut self, speed: f64) {
        self.move_ctl.speed_mod = speed;
        self.move_ctl.op = MoveOp::MoveTo;
    }

    fn is_primed(&self) -> bool {
        self.cube.fuse >= 0
    }

    fn can_explode(&self) -> bool {
        self.cube.explosion.is_some() && self.alive() && !self.is_primed()
    }
}

// ---------- Simulation ----------

impl Server {
    pub(crate) fn cube_sound(&mut self, m: &Mob, name: &str, volume: f32, pitch: f32) {
        self.mob_sound(m, name, volume, pitch);
    }

    /// Player targeted by a cube (neither creative, nor spectator, nor peaceful).
    fn cube_can_attack(&self, t: Ent) -> bool {
        match t {
            Ent::Player => {
                self.game_mode != 1 && self.game_mode != 3 && self.level.difficulty_id() != 0 && self.ent_alive(t)
            }
            Ent::Mob(_) => self.ent_alive(t),
        }
    }

    // ----- Goals -----

    pub(crate) fn cube_goal_can_use(&mut self, g: &mut CubeGoal, m: &mut Mob) -> bool {
        match g {
            CubeGoal::Float => m.in_water || m.lava_height > 0.0,
            CubeGoal::Attack { .. } => m.target.is_some_and(|t| self.cube_can_attack(t)),
            CubeGoal::RandomDir { .. } => m.target.is_none() && (m.on_ground || m.in_water || m.lava_height > 0.0),
            CubeGoal::KeepJumping => true,
            CubeGoal::Tempt { calm } => {
                if *calm > 0 {
                    *calm -= 1;
                    return false;
                }
                let range = m.attrs.tempt_range.value();
                self.sulfur_tempted(m) && self.targeting_ok(m, Ent::Player, false, range, false, true)
            }
            CubeGoal::SearchItems { target } => {
                if m.is_baby() || m.cube.pickup_timer > 0 {
                    return false;
                }
                *target = self.nearest_swallowable(m);
                target.is_some()
            }
        }
    }

    pub(crate) fn cube_goal_can_continue(&mut self, g: &mut CubeGoal, m: &mut Mob) -> bool {
        match g {
            CubeGoal::Attack { tired } => {
                let Some(t) = m.target else { return false };
                if !self.cube_can_attack(t) {
                    return false;
                }
                *tired -= 1;
                *tired > 0
            }
            _ => self.cube_goal_can_use(g, m),
        }
    }

    pub(crate) fn cube_goal_start(&mut self, g: &mut CubeGoal, _m: &mut Mob) {
        if let CubeGoal::Attack { tired } = g {
            *tired = reduced(300);
        }
    }

    pub(crate) fn cube_goal_stop(&mut self, g: &mut CubeGoal, m: &mut Mob) {
        if let CubeGoal::Tempt { calm } = g {
            m.set_wanted_movement(0.0);
            *calm = reduced(100);
        }
    }

    pub(crate) fn cube_goal_tick(&mut self, g: &mut CubeGoal, m: &mut Mob) {
        match g {
            CubeGoal::Float => {
                if m.rng.next_float() < 0.8 {
                    m.jump_ctl = true;
                }
                m.set_wanted_movement(1.2);
            }
            CubeGoal::Attack { .. } => {
                if let Some(t) = m.target
                    && let (Some(p), Some(eye)) = (self.ent_pos(t), self.ent_eye(t))
                {
                    super::goals::mob_look_at_direct(m, p, eye, 10.0, 10.0);
                }
                let (y, a) = (m.yaw, m.can_deal_damage());
                m.set_direction(y, a);
            }
            CubeGoal::RandomDir { chosen, next } => {
                *next -= 1;
                if *next <= 0 {
                    *next = reduced(40 + m.rng.next_int_bound(60));
                    *chosen = m.rng.next_int_bound(360) as f32;
                }
                m.set_direction(*chosen, false);
            }
            CubeGoal::KeepJumping => m.set_wanted_movement(1.0),
            CubeGoal::Tempt { .. } => {
                let eye = self.player_eye_y();
                self.look_at(m, [self.pos[0], eye, self.pos[2]], 75.0 + 20.0, 0.0);
                if dist2(m.pos, self.pos) < 1.0 {
                    m.set_wanted_movement(0.0);
                } else {
                    super::goals::mob_look_at_direct(m, self.pos, eye, 10.0, 10.0);
                    let y = m.yaw;
                    m.set_direction(y, true);
                }
            }
            CubeGoal::SearchItems { target } => {
                let it = target.and_then(|id| self.life.drops.iter().find(|d| d.id == id)).map(|d| d.pos);
                if let Some(p) = it {
                    // Item: vertical center of its box (0.25).
                    super::goals::mob_look_at_direct(m, p, p[1] + 0.125, 10.0, 10.0);
                }
                let y = m.yaw;
                m.set_direction(y, true);
            }
        }
    }

    /// Does the player hold something to attract the cube (small: food; adult: absorbable block)?
    fn sulfur_tempted(&self, m: &Mob) -> bool {
        let tag = if m.is_baby() { "minecraft:sulfur_cube_food" } else { "minecraft:sulfur_cube_swallowable" };
        [&self.slots[36 + self.held as usize], &self.slots[45]]
            .iter()
            .any(|s| s.as_ref().is_some_and(|s| self.item_tag(&s.id, tag)))
    }

    /// Nearest absorbable item on the ground in the box inflated by 8.
    fn nearest_swallowable(&self, m: &Mob) -> Option<i32> {
        let b = m.bbox();
        let mut best: Option<(f64, i32)> = None;
        for d in &self.life.drops {
            if d.is_removed() || d.pickup_delay > 0 || d.stack.count <= 0 {
                continue;
            }
            if !self.item_tag(&d.stack.id, "minecraft:sulfur_cube_swallowable") {
                continue;
            }
            let ib = rc_physics::Aabb {
                min: [d.pos[0] - 0.125, d.pos[1], d.pos[2] - 0.125],
                max: [d.pos[0] + 0.125, d.pos[1] + 0.25, d.pos[2] + 0.125],
            };
            let area = rc_physics::Aabb {
                min: [b.min[0] - 8.0, b.min[1] - 8.0, b.min[2] - 8.0],
                max: [b.max[0] + 8.0, b.max[1] + 8.0, b.max[2] + 8.0],
            };
            if !ib.intersects(&area) {
                continue;
            }
            let dd = dist2(m.pos, d.pos);
            if best.is_none_or(|x| dd < x.0) {
                best = Some((dd, d.id));
            }
        }
        best.map(|b| b.1)
    }

    // ----- Controls -----

    /// Movement control by jumps (tick of the cubes' movement control).
    pub(crate) fn cube_move_control(&mut self, m: &mut Mob) {
        if m.kind == Kind::SulfurCube && m.has_body_item() {
            return;
        }
        m.yaw = rotlerp_360(m.yaw, m.cube.dir, 90.0);
        m.head_yaw = m.yaw;
        m.body_yaw = m.yaw;
        if m.move_ctl.op != MoveOp::MoveTo {
            m.zza = 0.0;
            return;
        }
        m.move_ctl.op = MoveOp::Wait;
        let sp = (m.move_ctl.speed_mod * m.movement_speed()) as f32;
        if m.on_ground {
            m.speed = sp;
            m.zza = sp;
            let d = m.cube.jump_delay;
            m.cube.jump_delay -= 1;
            if d <= 0 {
                let mut delay = m.rng.next_int_bound(20) + 10;
                if m.kind == Kind::MagmaCube {
                    delay *= 4;
                }
                if m.cube.aggressive {
                    delay /= 3;
                }
                m.cube.jump_delay = delay;
                m.jump_ctl = true;
                let adj = if m.is_tiny() { 1.4 } else { 0.8 };
                let pitch = ((m.rng.next_float() - m.rng.next_float()) * 0.2 + 1.0) * adj;
                let (s, v) = (m.jump_sound(), m.sound_volume());
                self.cube_sound(m, s, v, pitch);
            } else {
                m.xxa = 0.0;
                m.zza = 0.0;
                m.speed = 0.0;
            }
        } else {
            m.speed = sp;
            m.zza = sp;
        }
    }

    /// Gaze of the full sulfur cube: yaw brought back to the nearest quarter turn; true if
    /// the normal control is replaced.
    pub(crate) fn sulfur_look_control(&self, m: &mut Mob) -> bool {
        if m.kind != Kind::SulfurCube || !m.has_body_item() {
            return false;
        }
        let close = wrap_degrees_90(m.yaw);
        m.yaw -= close;
        m.head_yaw = m.yaw;
        true
    }

    // ----- Tick -----

    /// Before the common tick: squish, fuse, redstone signal. True if the cube exploded.
    pub(crate) fn cube_pre_tick(&mut self, m: &mut Mob) -> bool {
        if m.kind == Kind::SulfurCube {
            if m.cube.fuse > 0 {
                m.cube.fuse -= 1;
            }
            if let Some((power, _fire, _)) = m.cube.explosion
                && m.cube.fuse == 0
            {
                m.dead = true;
                if self.rule("minecraft:tnt_explodes", true) {
                    let c = [m.pos[0], m.pos[1] + f64::from(m.height()) * 0.0625, m.pos[2]];
                    self.mob_explode(m, c, power as f32);
                }
                self.mob_removed_effects(m); // "killed" removal effects, explosion or not
                m.removed = true;
                return true;
            }
            if m.can_explode() && self.cube_powered(m.block_pos()) {
                self.sulfur_prime(m, false);
            }
        }
        m.cube.o_squish = m.cube.squish;
        m.cube.squish += (m.cube.target_squish - m.cube.squish) * 0.5;
        false
    }

    /// Signal received (neighbors, or the block itself if it emits: plate, lever…).
    fn cube_powered(&self, p: [i32; 3]) -> bool {
        if self.best_neighbor_signal(p) != 0 {
            return true;
        }
        !self.full_collision(p) && crate::blocks::Dir::ALL.iter().any(|&d| self.signal(p, d) > 0)
    }

    /// After the common tick: landing (particles, squish sound), decay.
    pub(crate) fn cube_post_tick(&mut self, m: &mut Mob) {
        if m.on_ground && !m.cube.was_on_ground {
            let size = m.width() * 2.0;
            let mut i = 0;
            // Particles added server-side have no effect; the draws, however, count.
            while (i as f32) < size * 16.0 {
                let _ = (m.rng.next_float(), m.rng.next_float());
                i += 1;
            }
            let pitch = ((m.rng.next_float() - m.rng.next_float()) * 0.2 + 1.0) / 0.8;
            let (s, v) = (m.squish_sound(), m.sound_volume());
            self.cube_sound(m, s, v, pitch);
            m.cube.target_squish = -0.5;
        } else if !m.on_ground && m.cube.was_on_ground {
            m.cube.target_squish = 1.0;
        }
        m.cube.was_on_ground = m.on_ground;
        m.cube.target_squish *= if m.kind == Kind::MagmaCube { 0.9 } else { 0.6 };
    }

    /// The server AI step specific to the sulfur cube species.
    pub(crate) fn cube_custom_ai(&mut self, m: &mut Mob) {
        if m.cube.pickup_timer > 0 {
            m.cube.pickup_timer -= 1;
        }
        if m.cube.push_sound_cd > 0 {
            m.cube.push_sound_cd -= 1;
        }
    }

    /// Split on removal after death (`remove`).
    pub(crate) fn cube_split(&mut self, m: &mut Mob) {
        let size = m.cube.size;
        if size <= 1 {
            return;
        }
        let off = m.width() / 2.0;
        let half = size / 2;
        let count = match m.kind {
            Kind::SulfurCube => {
                if m.is_primed() {
                    0
                } else {
                    2
                }
            }
            _ => 2 + m.rng.next_int_bound(3),
        };
        for i in 0..count {
            let xd = ((i % 2) as f32 - 0.5) * off;
            let zd = ((i / 2) as f32 - 0.5) * off;
            let mut c = self.new_mob(m.kind, m.pos, 0.0);
            c.head_yaw = 0.0;
            c.body_yaw = 0.0;
            // The common conversion.
            if m.is_baby() {
                c.set_baby(true);
            }
            if m.kind == Kind::SulfurCube {
                c.cube_set_age(m.age);
                c.cube.forced_age = m.cube.forced_age;
                c.cube.forced_age_timer = m.cube.forced_age_timer;
            }
            c.left_handed = m.left_handed;
            c.no_ai = m.no_ai;
            c.persistent |= m.persistent;
            c.custom_name = m.custom_name.clone();
            // Setting up the split cube.
            c.set_size(half, true);
            c.pos = [m.pos[0] + f64::from(xd), m.pos[1] + 0.5, m.pos[2] + f64::from(zd)];
            c.old = c.pos;
            c.yaw = m.rng.next_float() * 360.0;
            c.pitch = 0.0;
            if m.kind == Kind::SulfurCube {
                c.set_baby(true);
            }
            self.add_mob(c);
        }
    }

    // ----- Spawning -----

    /// The choice of the spawn size (world random source) after the common spawn finalization.
    pub(crate) fn cube_spawn_size(&mut self, m: &mut Mob, special: f32) {
        if m.kind == Kind::SulfurCube {
            let s = if m.is_baby() { 1 } else { 2 };
            m.set_size(s, true);
            return;
        }
        let mut scale = self.mobs.rng.next_int_bound(3);
        if scale < 2 && self.mobs.rng.next_float() < 0.5 * special {
            scale += 1;
        }
        m.set_size(1 << scale, true);
    }

    /// Own spawn rule (the support block is checked afterwards by the caller).
    pub(crate) fn cube_spawn_rule(&mut self, kind: Kind, p: [i32; 3]) -> bool {
        match kind {
            Kind::Slime => {
                if self.level.difficulty_id() == 0 {
                    return false;
                }
                if self.biome_tagged(p, "minecraft:allows_surface_slime_spawns") && p[1] > 50 && p[1] < 70 {
                    let chance = self.surface_slime_chance();
                    if self.mobs.rng.next_float() < chance && self.mob_brightness(p) <= self.mobs.rng.next_int_bound(8)
                    {
                        return true;
                    }
                }
                let seed = match self.mobs.cubes.seed {
                    Some(s) => s,
                    None => {
                        let s = self.seed();
                        self.mobs.cubes.seed = Some(s);
                        s
                    }
                };
                let chunk = is_slime_chunk(seed, p[0] >> 4, p[2] >> 4);
                self.mobs.rng.next_int_bound(10) == 0 && chunk && p[1] < 40
            }
            Kind::MagmaCube => self.level.difficulty_id() != 0,
            _ => true,
        }
    }

    /// Surface slime chance: moon track (world clock, 8-day period).
    fn surface_slime_chance(&self) -> f32 {
        if self.dim.name != "minecraft:overworld" {
            return 0.0;
        }
        let t = self.clocks.iter().find(|(n, _)| n == "minecraft:overworld").map_or(self.time, |c| c.1);
        MOON[t.div_euclid(24000).rem_euclid(8) as usize] * 0.5
    }

    fn biome_tagged(&self, p: [i32; 3], tag: &str) -> bool {
        let Some(id) = self.biome_at(p) else { return false };
        self.regs
            .tags
            .iter()
            .find(|(r, _)| r == "minecraft:worldgen/biome")
            .and_then(|(_, t)| t.iter().find(|(n, _)| n == tag))
            .is_some_and(|(_, ids)| ids.contains(&(id as i32)))
    }

    /// Base experience (slime and magma: the size; sulfur: 1–2, small 0).
    pub(crate) fn cube_xp(&self, m: &mut Mob) -> Option<i32> {
        match m.kind {
            Kind::SulfurCube => Some(if m.is_baby() { 0 } else { 1 + m.rng.next_int_bound(2) }),
            _ => None,
        }
    }

    // ----- Contact with the player -----

    /// The contact with a player of the cubes the player touches (box inflated by 1; 0.5; 1).
    pub(crate) fn cubes_player_touch(&mut self) {
        let now = self.pos;
        let last = self.mobs.cubes.last_player.replace(now).unwrap_or(now);
        self.mobs.cubes.player_speed = [now[0] - last[0], now[1] - last[1], now[2] - last[2]];
        if self.game_mode == 3 || self.life.vit.dead || self.life.vit.health <= 0.0 {
            return;
        }
        let pb = self.player_aabb();
        let area = rc_physics::Aabb {
            min: [pb.min[0] - 1.0, pb.min[1] - 0.5, pb.min[2] - 1.0],
            max: [pb.max[0] + 1.0, pb.max[1] + 0.5, pb.max[2] + 1.0],
        };
        let ids: Vec<i32> = self
            .mobs
            .list
            .iter()
            .flatten()
            .filter(|o| o.kind.is_cube() && !o.removed && o.touches(&area))
            .map(|o| o.id)
            .collect();
        for id in ids {
            let Some(i) = self.mob_index(id) else { continue };
            let Some(mut m) = self.mobs.list[i].take() else { continue };
            if m.can_deal_damage() {
                self.cube_deal_damage(&mut m);
            }
            if m.kind == Kind::SulfurCube {
                self.sulfur_player_push(&mut m);
            }
            self.mobs.list[i] = Some(m);
        }
    }

    /// Alive, melee range, direct line of sight.
    fn cube_deal_damage(&mut self, m: &mut Mob) {
        if !m.alive() || m.dead {
            return;
        }
        if !self.within_melee(m, Ent::Player) {
            return;
        }
        let from = [m.pos[0], m.eye_y(), m.pos[2]];
        let to = [self.pos[0], self.player_eye_y(), self.pos[2]];
        if dist2(from, to) > 128.0 * 128.0 || self.clip_blocks(from, to) {
            return;
        }
        let dmg = m.attack_damage();
        let kb = (m.pos[0] - self.pos[0], m.pos[2] - self.pos[2]);
        if self.player_hurt_by_mob(m, "minecraft:mob_attack", dmg, Some(kb)) {
            let pitch = (m.rng.next_float() - m.rng.next_float()) * 0.2 + 1.0;
            self.cube_sound(m, "minecraft:entity.slime.attack", 1.0, pitch);
        }
    }

    /// Push of a full sulfur cube by the player.
    fn sulfur_player_push(&mut self, m: &mut Mob) {
        if !m.has_body_item() {
            return;
        }
        let c2p = [m.pos[0] - self.pos[0], m.pos[1] - self.pos[1], m.pos[2] - self.pos[2]];
        let hd = (c2p[0] * c2p[0] + c2p[2] * c2p[2]).sqrt();
        let ph = self.player_aabb().max[1] - self.pos[1];
        let (feet, bottom) = (self.pos[1], m.pos[1]);
        let top = bottom + f64::from(m.height());
        if !(hd < f64::from(1.3f32) && feet <= top && feet + ph > bottom) {
            return;
        }
        let kb = (1.0 - m.knockback_resistance()).max(0.0);
        let dir = normalize([c2p[0], 0.0, c2p[2]]).map(|v| v * kb);
        let sp = self.mobs.cubes.player_speed;
        let speed = ((sp[0] * sp[0] + sp[1] * sp[1] + sp[2] * sp[2]).sqrt() * 2.0 * f64::from(0.3f32)).clamp(0.0, 0.5);
        let up = if m.on_ground { kb * f64::from(0.3f32) } else { 0.0 };
        let pv = [dir[0] * speed, up * speed, dir[2] * speed];
        m.needs_sync = true;
        let thr = f64::from(m.cube.sounds.push_threshold);
        if pv[0] * pv[0] + pv[1] * pv[1] + pv[2] * pv[2] > thr * thr && m.cube.push_sound_cd <= 0 {
            m.cube.push_sound_cd = (m.cube.sounds.push_cooldown * 20.0) as i32;
            let s = m.cube.sounds.push.clone();
            self.cube_sound(m, &s, 1.0, 1.0);
        }
        for (v, p) in m.vel.iter_mut().zip(pv) {
            *v += p;
        }
        self.sulfur_contact_damage(m);
    }

    /// Contact damage of the archetype (burning cube) on the player.
    fn sulfur_contact_damage(&mut self, m: &mut Mob) {
        for (ty, amount, _attr) in m.cube.contacts.clone() {
            let a = amount.as_f64().unwrap_or(0.0) as f32;
            let kb = (m.pos[0] - self.pos[0], m.pos[2] - self.pos[2]);
            self.player_hurt_by_mob(m, &ty, a, Some(kb));
        }
    }

    // ----- Sulfur cube damage and knockback -----

    /// The server-side damage computation of the sulfur cube: priming, immunity with an absorbed block (knockback only).
    /// `Some(r)` if handled here.
    pub(crate) fn sulfur_hurt(
        &mut self,
        m: &mut Mob,
        ty: &str,
        amount: f32,
        by: Option<Ent>,
        src: Option<[f64; 3]>,
    ) -> Option<bool> {
        // The "absorbed block" branch goes BEFORE the invulnerability test: a primed cube
        // (invulnerable) is still thrown by a hit or an explosion, and the hit counts as landed.
        if m.has_body_item() {
            if m.can_explode() && !m.is_primed() {
                if self.dmg_tag(ty, "minecraft:is_fire") {
                    self.sulfur_prime(m, false);
                } else if self.dmg_tag(ty, "minecraft:is_explosion") {
                    self.sulfur_prime(m, true);
                }
            }
            if self.dmg_tag(ty, "minecraft:sulfur_cube_with_block_immune_to") {
                if !self.dmg_tag(ty, "minecraft:no_knockback") {
                    let (xd, zd) = match src {
                        Some(s) => (s[0] - m.pos[0], s[2] - m.pos[2]),
                        None => (0.0, 0.0),
                    };
                    self.mob_knockback(m, 0.4, xd, zd, by, amount, false);
                }
                return Some(true);
            }
        }
        // Generic path: the fuse's invulnerability is only tested here.
        if m.cube.invulnerable && !self.dmg_tag(ty, "minecraft:bypasses_invulnerability") {
            return Some(false);
        }
        None
    }

    /// Fuse (short and random if imminent), invulnerable, priming sound.
    fn sulfur_prime(&mut self, m: &mut Mob, imminent: bool) -> bool {
        let Some((_, _, fuse)) = m.cube.explosion else { return false };
        if !m.alive() || !self.rule("minecraft:tnt_explodes", true) || m.is_primed() {
            return false;
        }
        let t = if imminent { m.rng.next_int_bound((fuse / 4).max(1)) + fuse / 8 } else { fuse };
        m.cube.invulnerable = true;
        m.cube.fuse = t;
        m.cube.max_fuse = t;
        self.make_sound(m, "minecraft:entity.tnt.primed");
        true
    }

    /// Knockback of a mob; that of the full sulfur cube depends on the attacker's aim.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn mob_knockback(
        &mut self,
        m: &mut Mob,
        power: f64,
        xd: f64,
        zd: f64,
        by: Option<Ent>,
        damage: f32,
        from_effect: bool,
    ) {
        // F14: frozen creaking, no knockback.
        // F15: seated dragon without knockback; hit on a part: the weapon's knockback goes to the part.
        if !super::recent::accepts_knockback(m)
            || super::end::no_knockback(m)
            || (from_effect && self.mobs.end.part_attack)
        {
            return;
        }
        let src = by.and_then(|e| self.attacker_view(e));
        match src {
            Some((eye, feet, look)) if m.kind == Kind::SulfurCube && m.has_body_item() => {
                self.sulfur_knockback(m, power, xd, zd, (eye, feet, look), damage, from_effect);
            }
            _ => Server::knockback(m, power, xd, zd),
        }
    }

    /// Eye, feet and gaze direction of an attacker.
    fn attacker_view(&self, e: Ent) -> Option<([f64; 3], [f64; 3], [f64; 3])> {
        let (feet, eye_y, yaw, pitch) = match e {
            Ent::Player => (self.pos, self.player_eye_y(), self.yaw, self.pitch),
            Ent::Mob(id) => {
                let o = self.mob_ref(id)?;
                (o.pos, o.eye_y(), o.yaw, o.pitch)
            }
        };
        Some(([feet[0], eye_y, feet[2]], feet, view_vector(pitch, yaw)))
    }

    #[allow(clippy::too_many_arguments)]
    fn sulfur_knockback(
        &mut self,
        m: &mut Mob,
        power: f64,
        xd: f64,
        zd: f64,
        src: ([f64; 3], [f64; 3], [f64; 3]),
        damage: f32,
        from_effect: bool,
    ) {
        let k = Kb {
            eye: src.0,
            feet: src.1,
            look: src.2,
            bbox: m.bbox(),
            height: m.height(),
            pos: m.pos,
            powers: m.cube.kb,
            resistance: m.knockback_resistance(),
            dir: (xd, zd),
            damage,
            effect: from_effect.then_some(power),
        };
        m.vel = sulfur_kb_velocity(m.vel, &k);
        m.needs_sync = true;
        let s = m.cube.sounds.hit.clone();
        self.cube_sound(m, &s, 1.0, 1.0);
    }

    // ----- Absorbed item, archetypes -----

    fn archetypes(&mut self) -> Vec<Archetype> {
        if let Some(a) = &self.mobs.cubes.archetypes {
            return a.clone();
        }
        let mut list: Vec<(String, Value)> = self.life.pack.all("sulfur_cube_archetype");
        // Order of the synced registry if known, otherwise of the ids.
        let order = self.regs.entries("minecraft:sulfur_cube_archetype").to_vec();
        if !order.is_empty() {
            list.sort_by_key(|(id, _)| order.iter().position(|o| o == id).unwrap_or(usize::MAX));
        }
        let out: Vec<Archetype> = list.iter().map(|(id, v)| parse_archetype(id, v)).collect();
        self.mobs.cubes.archetypes = Some(out.clone());
        out
    }

    fn matching_archetypes(&mut self, s: &Stack) -> Vec<Archetype> {
        let all = self.archetypes();
        all.into_iter()
            .filter(|a| {
                a.items.iter().any(|i| match i.strip_prefix('#') {
                    Some(tag) => self.item_tag(&s.id, tag),
                    None => *i == s.id,
                })
            })
            .collect()
    }

    /// Change of the body item: goals, archetypes, send.
    pub(crate) fn sulfur_equipment_check(&mut self, m: &mut Mob) {
        let current = m.equipment[BODY].clone();
        if current == m.cube.last_body {
            return;
        }
        let previous = std::mem::replace(&mut m.cube.last_body, current.clone());
        self.sulfur_body_changed(m, previous.as_ref());
        if m.track.seen {
            let net = self.net(current.as_ref());
            let slots: rc_proto_v777::entity::Equipment = vec![(BODY as u8, net)];
            let _ = self.send(Event::Entity(EntityEvent::Equipment { id: m.id, slots }));
        }
    }

    fn sulfur_body_changed(&mut self, m: &mut Mob, previous: Option<&Stack>) {
        if m.has_body_item() {
            m.goals = Default::default();
            m.speed = 0.0;
            m.zza = 0.0;
        } else {
            register_goals(m);
        }
        if let Some(p) = previous {
            for a in self.matching_archetypes(p) {
                for (attr, id, _, _) in &a.mods {
                    if let Some(at) = attr_slot(m, attr) {
                        at.remove_mod(id);
                    }
                }
            }
        }
        m.cube.floats = false;
        m.cube.explosion = None;
        m.cube.contacts.clear();
        m.cube.kb = (0.33, 0.06);
        m.cube.sounds = Sounds::default();
        m.cube.applied.clear();
        if let Some(c) = m.equipment[BODY].clone() {
            for a in self.matching_archetypes(&c) {
                if a.buoyant {
                    m.cube.floats = true;
                }
                if a.explosion.is_some() {
                    m.cube.explosion = a.explosion;
                }
                if let Some(cd) = &a.contact {
                    m.cube.contacts.push(cd.clone());
                }
                m.cube.kb = a.kb;
                m.cube.sounds = a.sound.clone();
                for (attr, id, amount, op) in &a.mods {
                    if let Some(at) = attr_slot(m, attr) {
                        at.set_mod(id, *amount, *op);
                    }
                }
                m.cube.applied.push(a.id.clone());
            }
        }
    }

    /// Pickup of an absorbable block on the ground (range 1; 0; 1).
    pub(crate) fn sulfur_pickup(&mut self, m: &mut Mob) {
        if m.has_body_item() || !m.alive() || m.dead || !self.rule("minecraft:mob_griefing", true) {
            return;
        }
        let b = m.bbox();
        let area = rc_physics::Aabb {
            min: [b.min[0] - 1.0, b.min[1], b.min[2] - 1.0],
            max: [b.max[0] + 1.0, b.max[1], b.max[2] + 1.0],
        };
        let cands: Vec<(i32, String)> = self
            .life
            .drops
            .iter()
            .filter(|d| !d.is_removed() && d.stack.count > 0 && d.pickup_delay == 0)
            .filter(|d| {
                let ib = rc_physics::Aabb {
                    min: [d.pos[0] - 0.125, d.pos[1], d.pos[2] - 0.125],
                    max: [d.pos[0] + 0.125, d.pos[1] + 0.25, d.pos[2] + 0.125],
                };
                ib.intersects(&area)
            })
            .map(|d| (d.id, d.stack.id.clone()))
            .collect();
        for (id, item) in cands {
            let holdable =
                !m.has_body_item() && !m.is_baby() && self.item_tag(&item, "minecraft:sulfur_cube_swallowable");
            if !holdable || m.cube.pickup_timer > 0 {
                continue;
            }
            self.thrown_pickup_by_mob(id, m); // W8c: the reaction to an item pickup
            if let Some(one) = self.mob_take_one(id, m.id) {
                m.equipment[BODY] = Some(one);
                m.drop_chance[BODY] = 2.0;
                self.cube_sound(m, "minecraft:entity.sulfur_cube.absorb", 1.0, 1.0);
            }
        }
    }

    /// Drops the body item at the passengers' attachment point.
    fn sulfur_eject(&mut self, m: &Mob, s: Stack) {
        let pos = [m.pos[0], m.pos[1] + f64::from(m.height()), m.pos[2]];
        let r = &mut self.mobs.rng;
        let v = [r.next_double() * 0.2 - 0.1, 0.2, r.next_double() * 0.2 - 0.1];
        self.spawn_item(pos, v, s, 10);
    }

    /// Sulfur cube age: forced growth particles.
    pub(crate) fn sulfur_age_tick(&mut self, m: &mut Mob) {
        if m.cube.forced_age_timer > 0 {
            if m.cube.forced_age_timer % 4 == 0 {
                let w = f64::from(m.width());
                let x = m.pos[0] + w * (2.0 * m.rng.next_double() - 1.0);
                let y = m.pos[1] + f64::from(m.height()) * m.rng.next_double() + 0.5;
                let z = m.pos[2] + w * (2.0 * m.rng.next_double() - 1.0);
                self.particles("minecraft:happy_villager", [x, y, z], [0.0; 3], 0.0, 1);
            }
            m.cube.forced_age_timer -= 1;
        }
        if m.alive() {
            let age = m.age;
            if m.is_baby() && !m.cube.age_locked {
                m.cube_set_age(age + 1);
            } else if age > 0 {
                m.cube_set_age(age - 1);
            }
        }
    }

    /// Growth.
    fn sulfur_age_up(m: &mut Mob, seconds: i32) {
        let old = m.age;
        let age = (old + seconds * 20).min(0);
        let delta = age - old;
        m.cube_set_age(age);
        m.cube.forced_age += delta;
        if m.cube.forced_age_timer == 0 {
            m.cube.forced_age_timer = 40;
        }
        if m.age == 0 {
            let f = m.cube.forced_age;
            m.cube_set_age(f);
        }
    }

    /// Right click on a sulfur cube; `None`: nothing of its own.
    pub(crate) fn sulfur_interact(&mut self, m: &mut Mob, slot: usize) -> Option<bool> {
        let held = self.slots[slot].clone();
        let item = held.as_ref().map(|s| s.id.clone()).unwrap_or_default();
        if m.is_baby() {
            if self.item_tag(&item, "minecraft:sulfur_cube_food") && !m.cube.age_locked {
                let age = m.age;
                self.sulfur_consume(slot);
                // `(int)(ticks / 20 * 0.1F)`: integer division then float product.
                Self::sulfur_age_up(m, ((-age / 20) as f32 * 0.1f32) as i32);
                self.make_sound(m, "minecraft:entity.small_sulfur_cube.eat");
                return Some(true);
            }
            return None;
        }
        if m.is_primed() {
            return Some(false);
        }
        if m.can_explode() && (item == "minecraft:flint_and_steel" || item == "minecraft:fire_charge") {
            if !self.rule("minecraft:tnt_explodes", true) {
                return Some(false);
            }
            self.sulfur_prime(m, false);
            if item == "minecraft:flint_and_steel" {
                self.hurt_item(slot, 1);
            } else {
                self.sulfur_consume(slot);
            }
            return Some(true);
        }
        if item == "minecraft:shears" && m.has_body_item() {
            let s = m.equipment[BODY].take().expect("body item");
            self.sulfur_eject(m, s);
            self.cube_sound(m, "minecraft:entity.sulfur_cube.eject", 1.0, 1.0);
            m.cube.pickup_timer = 100;
            self.hurt_item(slot, 1);
            return Some(true);
        }
        if let Some(h) = held.as_ref()
            && self.item_tag(&h.id, "minecraft:sulfur_cube_swallowable")
        {
            if let Some(cur) = m.equipment[BODY].clone() {
                if cur.id == h.id {
                    return Some(false);
                }
                self.sulfur_eject(m, cur);
            }
            m.equipment[BODY] = Some(Stack { count: 1, ..h.clone() });
            m.drop_chance[BODY] = 2.0;
            self.cube_sound(m, "minecraft:entity.sulfur_cube.absorb", 1.0, 1.0);
            self.sulfur_consume(slot);
            return Some(true);
        }
        if item == "minecraft:bucket" && m.alive() {
            self.cube_sound(m, "minecraft:item.bucket.fill_sulfur_cube", 1.0, 1.0);
            let bucket = self.sulfur_bucket_of(m);
            self.fill_held(slot, bucket);
            m.removed = true;
            return Some(true);
        }
        None
    }

    fn sulfur_consume(&mut self, slot: usize) {
        if self.game_mode == 1 {
            return;
        }
        if let Some(s) = &mut self.slots[slot] {
            s.count -= 1;
            if s.count <= 0 {
                self.slots[slot] = None;
            }
        }
    }

    /// The swap of an empty item for its filled result without keeping the original item in creative.
    fn fill_held(&mut self, slot: usize, filled: Stack) {
        if self.game_mode == 1 {
            let has = self.slots.iter().flatten().any(|s| s.id == filled.id && s.components == filled.components);
            if !has {
                self.drop_or_store(filled);
            }
            return;
        }
        self.sulfur_consume(slot);
        if self.slots[slot].is_none() {
            self.slots[slot] = Some(filled);
        } else {
            self.drop_or_store(filled);
        }
    }

    fn drop_or_store(&mut self, s: Stack) {
        // Game inventory order: hotbar (36–44) then storage (9–35).
        if let Some(i) = (36..45).chain(9..36).find(|&i| self.slots[i].is_none()) {
            self.slots[i] = Some(s);
        } else {
            self.drop_from_player(s, false);
        }
    }

    /// Sulfur cube bucket: name, entity data (HP, age), absorbed contents.
    fn sulfur_bucket_of(&self, m: &Mob) -> Stack {
        let mut b = Stack { id: "minecraft:sulfur_cube_bucket".into(), count: 1, components: None };
        if let Some(n) = &m.custom_name {
            crate::items::set_component(&mut b, "minecraft:custom_name", n.clone());
        }
        let mut data = super::interact::bucket_default_data(m);
        nbtio::set(&mut data, "age", Nbt::Int(m.age));
        nbtio::set(&mut data, "age_locked", Nbt::Byte(i8::from(m.cube.age_locked)));
        crate::items::set_component(&mut b, "minecraft:bucket_entity_data", data);
        if let Some(c) = &m.equipment[BODY] {
            let content = Nbt::Compound(vec![("id".into(), Nbt::String(c.id.clone()))]);
            crate::items::set_component(&mut b, "minecraft:sulfur_cube_content", content);
        }
        b
    }

    /// Sulfur cube bucket emptied against a block: the cube is born on the face aimed at.
    pub(crate) fn sulfur_bucket_use(&mut self, slot: usize, clicked: [i32; 3], face: i32) -> bool {
        let Some(bucket) = self.slots[slot].clone() else { return false };
        let dirs = [[0, -1, 0], [0, 1, 0], [0, 0, -1], [0, 0, 1], [-1, 0, 0], [1, 0, 0]];
        let d = dirs[face.clamp(0, 5) as usize];
        let at = [clicked[0] + d[0], clicked[1] + d[1], clicked[2] + d[2]];
        let y_off = self.spawn_y_offset(at, false, Kind::SulfurCube);
        let pos = [f64::from(at[0]) + 0.5, f64::from(at[1]) + y_off, f64::from(at[2]) + 0.5];
        let yaw = super::mth::wrap(self.mobs.rng.next_float() * 360.0);
        let mut m = self.new_mob(Kind::SulfurCube, pos, yaw);
        m.head_yaw = m.yaw;
        m.body_yaw = m.yaw;
        let comp = |k: &str| crate::items::component(&bucket, k).cloned();
        if let Some(n) = comp("minecraft:custom_name") {
            m.custom_name = Some(n);
        }
        if let Some(c) = comp("minecraft:sulfur_cube_content")
            && let Some(id) = c.get("id").and_then(Nbt::as_str)
        {
            m.equipment[BODY] = Some(Stack { id: id.to_owned(), count: 1, components: None });
            m.drop_chance[BODY] = 2.0;
        }
        let mut g = Group::default();
        self.finalize_spawn(&mut m, super::spawn::Reason::Bucket, &mut g);
        if let Some(data) = comp("minecraft:bucket_entity_data") {
            super::interact::load_bucket_default_data(&mut m, &data);
            let age = int(&data, "age").unwrap_or(0) as i32;
            m.cube_set_age(age);
            m.cube.age_locked = int(&data, "age_locked").is_some_and(|v| v != 0);
        }
        m.cube.from_bucket = true;
        self.add_mob(m);
        self.world_sound("minecraft:item.bucket.empty_sulfur_cube", 6, pos, 1.0, 1.0);
        if self.game_mode != 1 {
            self.slots[slot] = Some(Stack { id: "minecraft:bucket".into(), count: 1, components: None });
        }
        true
    }

    /// Floating of the full, floating sulfur cube.
    pub(crate) fn sulfur_buoyancy(&self, m: &mut Mob) {
        if !(m.has_body_item() && m.cube.floats) {
            return;
        }
        let vibe = 0.2 * super::mth::sin(m.tick_count as f32 * 0.4);
        let h = if m.in_water { m.water_height } else { m.lava_height };
        let imm = h - self.fluid_jump_threshold(m) + f64::from(vibe);
        if imm > 0.0 {
            m.vel[1] += imm.min(1.0) * f64::from(0.04f32);
        }
    }
}

/// Named attribute of a mob (archetype modifiers).
fn attr_slot<'a>(m: &'a mut Mob, name: &str) -> Option<&'a mut super::mob::Attr> {
    Some(match name {
        "minecraft:knockback_resistance" => &mut m.attrs.knockback_resistance,
        "minecraft:explosion_knockback_resistance" => &mut m.attrs.explosion_kb_resistance,
        "minecraft:bounciness" => &mut m.attrs.bounciness,
        "minecraft:friction_modifier" => &mut m.attrs.friction_mod,
        "minecraft:air_drag_modifier" => &mut m.attrs.air_drag_mod,
        "minecraft:movement_speed" => &mut m.attrs.speed,
        "minecraft:max_health" => &mut m.attrs.max_health,
        "minecraft:gravity" => &mut m.attrs.gravity,
        "minecraft:jump_strength" => &mut m.attrs.jump,
        "minecraft:step_height" => &mut m.attrs.step,
        "minecraft:armor" => &mut m.attrs.armor,
        _ => return None,
    })
}

fn parse_archetype(id: &str, v: &Value) -> Archetype {
    let items = match &v["items"] {
        Value::String(s) => vec![s.clone()],
        Value::Array(a) => a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect(),
        _ => Vec::new(),
    };
    let mods = v["attribute_modifiers"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|m| {
                    let op = match m["operation"].as_str() {
                        Some("add_multiplied_base") => 1,
                        Some("add_multiplied_total") => 2,
                        _ => 0,
                    };
                    (
                        m["attribute"].as_str().unwrap_or("").to_owned(),
                        intern(m["id"].as_str().unwrap_or("")),
                        m["amount"].as_f64().unwrap_or(0.0),
                        op,
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let e = &v["explosion"];
    let explosion = e.is_object().then(|| {
        (
            e["power"].as_i64().unwrap_or(0) as i32,
            e["causes_fire"].as_bool().unwrap_or(false),
            e["fuse"].as_i64().unwrap_or(1) as i32,
        )
    });
    let c = &v["contact_damage"];
    let contact = c.is_object().then(|| {
        (
            c["damage_type"].as_str().unwrap_or("").to_owned(),
            c["amount"].clone(),
            c["attribute_to_source"].as_bool().unwrap_or(false),
        )
    });
    let k = &v["knockback_modifiers"];
    let s = &v["sound_settings"];
    let d = Sounds::default();
    Archetype {
        id: id.to_owned(),
        items,
        mods,
        buoyant: v["buoyant"].as_bool().unwrap_or(false),
        explosion,
        contact,
        kb: (
            k["horizontal_power"].as_f64().unwrap_or(0.33) as f32,
            k["vertical_power"].as_f64().unwrap_or(0.06) as f32,
        ),
        sound: Sounds {
            hit: s["hit_sound"].as_str().map_or(d.hit, str::to_owned),
            push: s["push_sound"].as_str().map_or(d.push, str::to_owned),
            push_threshold: s["push_sound_impulse_threshold"].as_f64().map_or(d.push_threshold, |x| x as f32),
            push_cooldown: s["push_sound_cooldown"].as_f64().map_or(d.push_cooldown, |x| x as f32),
        },
    }
}

/// Inputs of the full sulfur cube's knockback.
pub struct Kb {
    pub eye: [f64; 3],
    pub feet: [f64; 3],
    /// Attacker's gaze.
    pub look: [f64; 3],
    pub bbox: rc_physics::Aabb,
    pub height: f32,
    pub pos: [f64; 3],
    /// Horizontal and vertical powers of the archetype.
    pub powers: (f32, f32),
    pub resistance: f64,
    /// Direction received (source − target).
    pub dir: (f64, f64),
    pub damage: f32,
    /// Knockback from an effect (charged hit): power.
    pub effect: Option<f64>,
}

/// Velocity after the knockback: horizontal hit angle (×1.6), vertical transfer by the
/// aimed height (×0.5), rotation by the attacker's position (×0.8), square root of the damage.
pub fn sulfur_kb_velocity(vel: [f64; 3], k: &Kb) -> [f64; 3] {
    let look = normalize(k.look);
    let (mut hp, mut vp) = k.powers;
    let (oh, ov) = (hp, vp);
    let b = &k.bbox;
    let center = [(b.min[0] + b.max[0]) / 2.0, (b.min[1] + b.max[1]) / 2.0, (b.min[2] + b.max[2]) / 2.0];
    let a2t = normalize(sub(center, k.eye));
    let ang = (look[0] * a2t[2] - look[2] * a2t[0]).atan2(look[0] * a2t[0] + look[2] * a2t[2]) as f32;
    let (nx, nz) = rotate2(k.dir.0 as f32, k.dir.1 as f32, ang * 1.6);
    let half = 0.5 * k.height;
    let top = normalize(sub([center[0], center[1] + f64::from(half), center[2]], k.eye));
    let bot = normalize(sub([center[0], center[1] - f64::from(half), center[2]], k.eye));
    let f = clamped_map(look[1], top[1], bot[1], -1.0, 1.0) as f32;
    let mut ratio = (f * 0.5).abs();
    if f < 0.0 {
        ratio = -ratio;
    }
    hp *= 1.0 - ratio;
    vp *= 1.0 + ratio;
    let d = sub(k.pos, k.feet);
    let vang = (-d[1]).atan2((d[0] * d[0] + d[2] * d[2]).sqrt()) as f32;
    let (mut rx, mut ry) = rotate2(hp, vp, -vang * 0.8);
    let hr = if oh > 0.0 { rx.abs() / oh } else { 0.0 };
    let vr = if ov > 0.0 { ry.abs() / ov } else { 0.0 };
    let mr = hr.max(vr);
    if mr > 1.0 {
        rx *= 1.0 / mr;
        ry *= 1.0 / mr;
    }
    hp = rx;
    vp = ry;
    let mult = k.damage.sqrt() * k.effect.map_or(1.0, |p| p as f32 * 0.25);
    hp *= mult;
    vp *= mult;
    hp *= (1.0 - k.resistance) as f32;
    vp *= (1.0 - k.resistance) as f32;
    hp *= 0.4;
    hp = hp.clamp(-128.0, 128.0);
    vp = vp.clamp(-128.0, 128.0);
    let hk = normalize([f64::from(nx), 0.0, f64::from(nz)]).map(|v| v * f64::from(hp));
    [vel[0] - hk[0], vel[1] + f64::from(vp) * 1.2, vel[2] - hk[2]]
}

/// Bringing an angle back into [−90, 90).
pub fn wrap_degrees_90(a: f32) -> f32 {
    let mut n = a % 90.0;
    if n >= 45.0 {
        n -= 90.0;
    }
    if n < -45.0 {
        n += 90.0;
    }
    n
}

fn sub(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// Normalization of a vector (zero below 1e-5).
fn normalize(v: [f64; 3]) -> [f64; 3] {
    let d = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if d < f64::from(1.0e-5f32) { [0.0; 3] } else { [v[0] / d, v[1] / d, v[2] / d] }
}

/// Rotation of a plane vector (tabulated sine).
fn rotate2(x: f32, y: f32, a: f32) -> (f32, f32) {
    let (c, s) = (super::mth::cos(a), super::mth::sin(a));
    (x * c - y * s, y * c + x * s)
}

/// The bounded linear mapping.
fn clamped_map(v: f64, from_min: f64, from_max: f64, to_min: f64, to_max: f64) -> f64 {
    let f = (v - from_min) / (from_max - from_min);
    if f < 0.0 {
        to_min
    } else if f > 1.0 {
        to_max
    } else {
        to_min + f * (to_max - to_min)
    }
}

/// The aim vector computation (pitch, yaw).
pub(crate) fn view_vector(pitch: f32, yaw: f32) -> [f64; 3] {
    let rx = pitch * (std::f32::consts::PI / 180.0);
    let ry = -yaw * (std::f32::consts::PI / 180.0);
    let (yc, ys) = (super::mth::cos(ry), super::mth::sin(ry));
    let (xc, xs) = (super::mth::cos(rx), super::mth::sin(rx));
    [f64::from(ys * xc), f64::from(-xs), f64::from(yc * xc)]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slime_chunk_overflows_like_the_game() {
        // Seed 0: values computed by the game's formula (overflowing int products).
        let n =
            (-50..50).flat_map(|x| (-50..50).map(move |z| (x, z))).filter(|&(x, z)| is_slime_chunk(0, x, z)).count();
        // About one chunk in ten.
        assert!((800..1200).contains(&n), "{n}");
        // Huge coordinates: no panic (intended overflows).
        let _ = is_slime_chunk(i64::MAX, i32::MAX, i32::MIN);
    }

    #[test]
    fn fixed_size_hp_speed_damage() {
        let rng = super::super::jrand::JRand::new(1);
        let mut m = Mob::new(Kind::Slime, 1, 0, [0.0; 3], 0.0, rng);
        m.set_size(4, true);
        assert_eq!(m.max_health(), 16.0);
        assert_eq!(m.health, 16.0);
        assert_eq!(m.attrs.speed.base, f64::from(0.2f32 + 0.1f32 * 4.0));
        assert_eq!(m.attrs.attack.base, 4.0);
        assert_eq!(m.width(), 0.52 * 4.0);
        assert_eq!(m.eye_height(), f64::from(0.325f32 * 4.0));
        let mut g = Mob::new(Kind::MagmaCube, 2, 0, [0.0; 3], 0.0, rng);
        g.set_size(2, true);
        assert_eq!(g.attrs.armor.base, 6.0);
        assert_eq!(g.attack_damage(), 4.0);
        m.set_size(500, false);
        assert_eq!(m.cube.size, 127);
    }

    #[test]
    fn sulfur_size_one_becomes_small() {
        let rng = super::super::jrand::JRand::new(2);
        let mut m = Mob::new(Kind::SulfurCube, 1, 0, [0.0; 3], 0.0, rng);
        m.set_size(2, true);
        assert_eq!(m.max_health(), 8.0);
        assert!(!m.is_baby());
        m.set_size(1, true);
        assert!(m.is_baby());
        assert_eq!(m.age, -24000);
        // Back to adult age: size 2.
        m.cube_set_age(0);
        assert_eq!(m.cube.size, 2);
        assert_eq!(m.width(), 0.49 * 2.0);
    }

    #[test]
    fn loaded_sulfur_persistent_and_breathes() {
        let rng = super::super::jrand::JRand::new(4);
        let mut m = Mob::new(Kind::SulfurCube, 1, 0, [0.0; 3], 0.0, rng);
        assert!(!m.requires_custom_persistence());
        assert!(!m.can_breathe_underwater());
        m.equipment[BODY] = Some(Stack { id: "minecraft:stone".into(), count: 1, components: None });
        assert!(m.requires_custom_persistence());
        assert!(m.can_breathe_underwater());
        m.equipment[BODY] = None;
        m.cube.from_bucket = true;
        assert!(m.requires_custom_persistence());
        assert!(!m.can_breathe_underwater());
        // The other cubes do not have this persistence.
        let rng = super::super::jrand::JRand::new(5);
        let mut s = Mob::new(Kind::Slime, 1, 0, [0.0; 3], 0.0, rng);
        s.cube.from_bucket = true;
        assert!(!s.requires_custom_persistence());
    }

    /// Official server 26.3 (25570, 12/10): sulfur cube size 2 with TNT at (605.5, −55,
    /// 605.5), without gravity; AI-less pig at (603.5, −55, 605.5) facing east;
    /// `damage … 4 mob_attack by <pig>` → Motion [0.2640000283718109, 0.08282230496406555, 0].
    /// (Without a connected player the cube had not ticked: archetype not applied, default settings.)
    #[test]
    fn sulfur_knockback_like_official_server() {
        let pos = [605.5, -55.0, 605.5];
        let feet = [603.5, -55.0, 605.5];
        let k = Kb {
            eye: [feet[0], feet[1] + f64::from(0.765f32), feet[2]],
            feet,
            look: view_vector(0.0, -90.0),
            bbox: Mob::bbox_at(pos, 0.98, 0.98),
            height: 0.98,
            pos,
            powers: (0.33, 0.06),
            resistance: 0.0,
            dir: (feet[0] - pos[0], feet[2] - pos[2]),
            damage: 4.0,
            effect: None,
        };
        let v = sulfur_kb_velocity([0.0; 3], &k);
        assert_eq!(v[0], 0.264_000_028_371_810_9);
        assert_eq!(v[1], 0.082_822_304_964_065_55);
        assert!(v[2].abs() < 1.0e-9, "{v:?}");
        // Same scene, client connected (the cube ticks): "explosive" archetype applied
        // (0.4125 / 0.09, knockback resistance −1) → [0.6600000262260437, 0.24846693277359008, 0].
        let k = Kb { powers: (0.4125, 0.09), resistance: -1.0, ..k };
        let v = sulfur_kb_velocity([0.0; 3], &k);
        assert_eq!(v[0], 0.660_000_026_226_043_7);
        assert_eq!(v[1], 0.248_466_932_773_590_08);
    }

    #[test]
    fn quarter_turn() {
        assert_eq!(wrap_degrees_90(100.0), 10.0);
        assert_eq!(wrap_degrees_90(50.0), -40.0);
        assert_eq!(wrap_degrees_90(-50.0), 40.0);
    }

    #[test]
    fn nbt_size_read_before_attributes() {
        let rng = super::super::jrand::JRand::new(3);
        let mut m = Mob::new(Kind::Slime, 1, 0, [0.0; 3], 0.0, rng);
        m.define_data();
        m.set_size(2, true);
        let e = m.save();
        assert_eq!(int(&e, "Size"), Some(1));
        let mut n = Mob::new(Kind::Slime, 2, 0, [0.0; 3], 0.0, rng);
        n.define_data();
        n.load(&e);
        assert_eq!(n.cube.size, 2);
        assert_eq!(n.max_health(), 4.0);
        n.sync_data();
        assert_eq!(n.get(18), Some(&Meta::Int(2)));
    }
}
