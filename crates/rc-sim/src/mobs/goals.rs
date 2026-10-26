//! Goals and selectors (`server-mobs.md` § 3.2): cleanup, start by priority and
//! flags, tick of the running goals; common goals of the simulated species and targeting goals.

use super::mob::{Ent, Mob};
use super::mth::{dist2, reduced};
use super::nav::Path;
use super::species::Kind;
use crate::player::Stack;
use crate::server::Server;

pub const MOVE: u8 = 1;
pub const LOOK: u8 = 2;
pub const JUMP: u8 = 4;
pub const TARGET: u8 = 8;
const FLAGS: [u8; 4] = [MOVE, LOOK, JUMP, TARGET];

/// What a targeting or fleeing goal aims at.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Who {
    Player,
    Villager,
    Skeleton,
    /// Wolf prey (sheep, rabbit, fox).
    WolfPrey,
    Wolf,
    Cat,
    Endermite,
    // F02: monsters, chicken, baby turtle on dry land, fox.
    Monster,
    Chicken,
    BabyTurtleOnLand,
    Fox,
    /// Rabbit (cat prey).
    Rabbit,
    /// F04: untamed wolf (llama prey), zombies (trader's llama), illagers.
    WildWolf,
    Zombie,
    Illager,
    /// F05: "zombie" class (zombie, husk, zombie villager) fled by the trader.
    ZombieClass,
    /// Iron golem (F03: cubes; F06: zombies, skeletons, spiders).
    IronGolem,
    /// F06: "enemy" interface (snow golem).
    Enemy,
    /// F06: enemy except creeper (iron golem).
    GolemPrey,
    /// F08: axolotl (not simulated: the goal draws its interval without ever finding).
    Axolotl,
    /// F11: fox prey (chicken, rabbit), schooling fish, polar bear.
    FoxPrey,
    SchoolingFish,
    PolarBear,
    /// F12: piglins (piglin, brute; wither skeleton targets, simulated by F13).
    Piglin,
    /// F16: any living entity except the undead (`wither_friends`), player included.
    WitherPrey,
    /// A species (and its subspecies: the trader's llama is a llama).
    Exact(Kind),
}

/// Food that tempts: specific item or item tag.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Food {
    Item(&'static str),
    Tag(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Melee {
    Plain,
    Zombie {
        raise: i32,
    },
    Spider,
    /// Polar bear: rears up before striking, without an arm swing.
    PolarBear,
    /// F11: fox (without arm swing, audible bite).
    Fox,
}

/// Common state of the targeting goals.
#[derive(Debug, Clone, Default)]
pub struct TBase {
    pub must_see: bool,
    pub must_reach: bool,
    reach_cache: i32,
    reach_cache_time: i32,
    unseen: i32,
    pub target_mob: Option<Ent>,
    pub unseen_memory: i32,
}

impl TBase {
    pub fn new(must_see: bool, must_reach: bool) -> TBase {
        TBase { must_see, must_reach, unseen_memory: 60, ..Default::default() }
    }
}

#[derive(Debug, Clone)]
pub enum Goal {
    Float,
    Panic {
        speed: f64,
        pos: [f64; 3],
        tag: &'static str,
    },
    Breed {
        speed: f64,
        partner: Option<i32>,
        love_time: i32,
    },
    Tempt {
        speed: f64,
        food: Food,
        scare: bool,
        stop: f64,
        calm: i32,
        p: [f64; 3],
        prot: [f32; 2],
        on: bool,
    },
    FollowParent {
        speed: f64,
        parent: Option<i32>,
        recalc: i32,
    },
    Stroll {
        speed: f64,
        interval: i32,
        prob: Option<f32>,
        wanted: [f64; 3],
    },
    LookAtPlayer {
        dist: f32,
        prob: f32,
        at: Option<Ent>,
        time: i32,
    },
    LookAround {
        rel: [f64; 2],
        time: i32,
    },
    Melee {
        speed: f64,
        unseen: bool,
        kind: Melee,
        path: Option<Path>,
        pathed: [f64; 3],
        recalc: i32,
        cd: i32,
        last: i64,
    },
    Bow {
        speed: f64,
        min_interval: i32,
        radius2: f32,
        attack_time: i32,
        see: i32,
        cw: bool,
        back: bool,
        strafe: i32,
    },
    Swell {
        target: Option<Ent>,
    },
    Leap {
        yd: f32,
        target: Option<Ent>,
    },
    Avoid {
        who: Who,
        max: f32,
        walk: f64,
        sprint: f64,
        from: Option<Ent>,
        path: Option<Path>,
    },
    EatBlock {
        tick: i32,
    },
    RestrictSun,
    FleeSun {
        speed: f64,
        wanted: [f64; 3],
    },
    SitOrdered,
    FollowOwner {
        speed: f64,
        start: f32,
        stop: f32,
        recalc: i32,
        /// Water malus from before the departure (reset on stop).
        old_water: f32,
    },
    Beg {
        dist: f32,
        time: i32,
    },
    OcelotAttack {
        target: Option<Ent>,
        time: i32,
    },
    /// Enderman: place the carried block (one chance in 1000 per evaluation tick).
    EndermanLeave,
    /// Tamed cat: curl up against the sleeping owner (bed cell on the foot side), then morning
    /// gift.
    CatRelax {
        goal_pos: Option<[i32; 3]>,
        on_bed: i32,
    },
    /// Enderman: take a block from the "holdable" tag (one chance in 10).
    EndermanTake,
    EndermanFreeze {
        target: Option<Ent>,
    },
    // Targeting.
    HurtBy {
        base: TBase,
        alert: bool,
        stamp: i32,
    },
    Nearest {
        base: TBase,
        who: Who,
        interval: i32,
        target: Option<Ent>,
        angry_only: bool,
        non_tame: bool,
    },
    OwnerHurtBy {
        base: TBase,
        stamp: i32,
        by: Option<Ent>,
    },
    OwnerHurt {
        base: TBase,
        stamp: i32,
        hit: Option<Ent>,
    },
    ResetAnger {
        stamp: i32,
        alert: bool,
    },
    EndermanLook {
        base: TBase,
        pending: Option<Ent>,
        aggro: i32,
        teleport: i32,
    },
    /// Goals of the F01 family (`variants.rs`).
    Extra(super::variants::Xg),
    /// Goals of simple surface animals (F02).
    Surf(surface_goals::SGoal),
    /// F13: the zombified piglin's spear-use objective.
    Spear(super::piglins::spear::SpearGoal),
    /// F03: cube goals (`cubes.rs`).
    Cube(super::cubes::CubeGoal),
    /// F04: mount goals (`goals/mount_goals.rs`).
    Mount(mount_goals::MGoal),
    /// F05: wandering trader goals.
    Npc(super::villager::trader::TGoal),
    /// F06: golem goals (`golems/`).
    Golem(super::golems::GolemGoal),
    /// F07: goals of illagers, vex, ravager, witch (`illagers/goals.rs`).
    Ill(super::illagers::IGoal),
    /// F08: goals of the drowned and guardians (`aquatic/`).
    Aqua(super::aquatic::AquaGoal),
    /// F09: goals of aquatic fauna (`sealife/goals.rs`).
    Sea(super::sealife::goals::AGoal),
    /// F11: goals of the bee, fox, panda (`wild/goals.rs`).
    Wild(super::wild::goals::WGoal),
    /// F12: goals of the blaze, ghast and strider (`nether/goals.rs`).
    Neth(super::nether::NGoal),
    /// F14: goals of the adult happy ghast (`recent/happy_ghast.rs`).
    Rec(super::recent::happy_ghast::RGoal),
    /// F15: goals of the shulker (`end/goals.rs`).
    End(super::end::EGoal),
    /// F16: goals of the wither (`wither/`).
    Wither(super::wither::WGoal),
}

#[derive(Debug, Clone)]
pub struct Wrapped {
    pub prio: i32,
    pub goal: Goal,
    pub running: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Selector {
    pub goals: Vec<Wrapped>,
    locked: [Option<usize>; 4],
    pub disabled: u8,
}

impl Selector {
    pub fn add(&mut self, prio: i32, goal: Goal) {
        self.goals.push(Wrapped { prio, goal, running: false });
    }

    pub fn running(&self, f: impl Fn(&Goal) -> bool) -> bool {
        self.goals.iter().any(|w| w.running && f(&w.goal))
    }

    /// Locks recomputed after the removal of a goal (shifted indices).
    pub fn reset_locks(&mut self) {
        self.locked = [None; 4];
        for (i, w) in self.goals.iter().enumerate() {
            if w.running {
                for (k, bit) in FLAGS.iter().enumerate() {
                    if w.goal.flags() & bit != 0 {
                        self.locked[k] = Some(i);
                    }
                }
            }
        }
    }
}

impl Goal {
    pub fn flags(&self) -> u8 {
        match self {
            Goal::Float => JUMP,
            Goal::Panic { .. }
            | Goal::Stroll { .. }
            | Goal::Swell { .. }
            | Goal::Avoid { .. }
            | Goal::FleeSun { .. }
            | Goal::FollowParent { .. } => MOVE,
            Goal::Breed { .. } | Goal::Tempt { .. } | Goal::Melee { .. } | Goal::Bow { .. } => MOVE | LOOK,
            Goal::FollowOwner { .. } | Goal::OcelotAttack { .. } => MOVE | LOOK,
            Goal::LookAtPlayer { .. } | Goal::Beg { .. } => LOOK,
            Goal::LookAround { .. } => MOVE | LOOK,
            Goal::Leap { .. } => JUMP | MOVE,
            Goal::EatBlock { .. } => MOVE | LOOK | JUMP,
            Goal::SitOrdered => JUMP | MOVE,
            Goal::EndermanFreeze { .. } => JUMP | MOVE,
            Goal::RestrictSun
            | Goal::ResetAnger { .. }
            | Goal::EndermanLeave
            | Goal::EndermanTake
            | Goal::CatRelax { .. } => 0,
            Goal::Cube(c) => c.flags(),
            Goal::Mount(g) => g.flags(),
            Goal::Npc(t) => t.flags(),
            Goal::HurtBy { .. }
            | Goal::Nearest { .. }
            | Goal::OwnerHurtBy { .. }
            | Goal::OwnerHurt { .. }
            | Goal::EndermanLook { .. } => TARGET,
            Goal::Extra(x) => x.flags(),
            Goal::Surf(g) => g.flags(),
            Goal::Spear(_) => MOVE | LOOK,
            Goal::Golem(g) => g.flags(),
            Goal::Ill(g) => g.flags(),
            Goal::Aqua(a) => a.flags(),
            Goal::Sea(a) => a.flags(),
            Goal::Wild(w) => w.flags(),
            Goal::Neth(g) => g.flags(),
            Goal::Rec(g) => g.flags(),
            Goal::End(g) => g.flags(),
            Goal::Wither(g) => g.flags(),
        }
    }

    /// The possible-interruption test (F09: dolphin breathing, treasure and jump do not yield).
    fn interruptable(&self) -> bool {
        !matches!(self, Goal::Sea(a) if !a.interruptable()) && !matches!(self, Goal::Wild(w) if !w.interruptable())
    }

    pub(crate) fn every_tick(&self) -> bool {
        if let Goal::Aqua(a) = self {
            return a.every_tick();
        }
        if let Goal::End(g) = self {
            return g.every_tick();
        }
        if let Goal::Neth(g) = self {
            return g.every_tick();
        }
        if let Goal::Rec(g) = self {
            return g.every_tick();
        }
        if let Goal::Wither(g) = self {
            return g.every_tick();
        }
        if let Goal::Sea(a) = self {
            return a.every_tick();
        }
        if let Goal::Wild(w) = self {
            return w.every_tick();
        }
        if let Goal::Mount(g) = self {
            return g.every_tick();
        }
        if let Goal::Cube(c) = self {
            return c.every_tick();
        }
        if let Goal::Ill(g) = self {
            return g.every_tick();
        }
        if let Goal::Golem(g) = self {
            return g.every_tick();
        }
        matches!(
            self,
            Goal::Float
                | Goal::LookAround { .. }
                | Goal::Melee { .. }
                | Goal::Bow { .. }
                | Goal::Swell { .. }
                | Goal::OcelotAttack { .. }
        ) || matches!(self, Goal::Surf(g) if g.every_tick())
    }

    /// The adjusted tick delay.
    fn adjusted(&self, t: i32) -> i32 {
        if self.every_tick() { t } else { reduced(t) }
    }
}

impl Server {
    /// Full tick of a selector.
    pub(crate) fn selector_tick(&mut self, m: &mut Mob, targets: bool) {
        let mut sel = std::mem::take(if targets { &mut m.targets } else { &mut m.goals });
        for i in 0..sel.goals.len() {
            if sel.goals[i].running {
                let dis = sel.goals[i].goal.flags() & sel.disabled != 0;
                if dis || !self.goal_can_continue(&mut sel.goals[i].goal, m) {
                    sel.goals[i].running = false;
                    self.goal_stop(&mut sel.goals[i].goal, m);
                }
            }
        }
        for slot in 0..4 {
            if let Some(i) = sel.locked[slot]
                && !sel.goals[i].running
            {
                sel.locked[slot] = None;
            }
        }
        for i in 0..sel.goals.len() {
            if sel.goals[i].running {
                continue;
            }
            let flags = sel.goals[i].goal.flags();
            if flags & sel.disabled != 0 {
                continue;
            }
            let prio = sel.goals[i].prio;
            let replaceable = (0..4).all(|k| {
                if flags & FLAGS[k] == 0 {
                    return true;
                }
                match sel.locked[k] {
                    None => true,
                    Some(j) => sel.goals[j].prio > prio && sel.goals[j].goal.interruptable(),
                }
            });
            if !replaceable || !self.goal_can_use(&mut sel.goals[i].goal, m) {
                continue;
            }
            for (k, bit) in FLAGS.iter().enumerate() {
                if flags & bit == 0 {
                    continue;
                }
                if let Some(j) = sel.locked[k]
                    && sel.goals[j].running
                {
                    sel.goals[j].running = false;
                    self.goal_stop(&mut sel.goals[j].goal, m);
                }
                sel.locked[k] = Some(i);
            }
            sel.goals[i].running = true;
            self.goal_start(&mut sel.goals[i].goal, m);
        }
        for i in 0..sel.goals.len() {
            if sel.goals[i].running {
                self.goal_tick(&mut sel.goals[i].goal, m);
            }
        }
        *(if targets { &mut m.targets } else { &mut m.goals }) = sel;
    }

    /// Partial tick: only the running "every tick" goals.
    pub(crate) fn selector_tick_running(&mut self, m: &mut Mob, targets: bool) {
        let mut sel = std::mem::take(if targets { &mut m.targets } else { &mut m.goals });
        for i in 0..sel.goals.len() {
            if sel.goals[i].running && sel.goals[i].goal.every_tick() {
                self.goal_tick(&mut sel.goals[i].goal, m);
            }
        }
        *(if targets { &mut m.targets } else { &mut m.goals }) = sel;
    }

    // ---------- Entities seen by the goals ----------

    pub(crate) fn mob_index(&self, id: i32) -> Option<usize> {
        self.mobs.index_of(id)
    }

    /// Mob of the list, otherwise the most recent frozen copy (`ghost_push`).
    pub(crate) fn mob_ref(&self, id: i32) -> Option<&Mob> {
        self.mobs.get(id).or_else(|| self.mobs.ghosts.iter().rev().find(|o| o.id == id))
    }

    /// Attacker `m` (taken out of the list during its own tick) made visible to lookups
    /// by identifier until `ghost_pop`: frozen copy of its state at the time of the hit.
    pub(crate) fn ghost_push(&mut self, m: &Mob) {
        self.mobs.ghosts.push(m.clone());
    }

    pub(crate) fn ghost_pop(&mut self) {
        self.mobs.ghosts.pop();
    }

    pub(crate) fn ent_pos(&self, e: Ent) -> Option<[f64; 3]> {
        match e {
            Ent::Player => Some(self.pos),
            Ent::Mob(id) => self.mob_ref(id).map(|m| m.pos),
        }
    }

    pub(crate) fn ent_eye(&self, e: Ent) -> Option<f64> {
        match e {
            Ent::Player => Some(self.player_eye_y()),
            Ent::Mob(id) => self.mob_ref(id).map(Mob::eye_y),
        }
    }

    pub(crate) fn ent_alive(&self, e: Ent) -> bool {
        match e {
            Ent::Player => !self.life.vit.dead && self.life.vit.health > 0.0,
            Ent::Mob(id) => self.mob_ref(id).is_some_and(Mob::alive),
        }
    }

    pub(crate) fn ent_bbox(&self, e: Ent) -> Option<rc_physics::Aabb> {
        match e {
            Ent::Player => Some(self.player_aabb()),
            Ent::Mob(id) => self.mob_ref(id).map(Mob::bbox),
        }
    }

    /// Player attackable by a mob (the can-attack test + the "seen as an enemy" test).
    fn player_attackable(&self) -> bool {
        self.game_mode != 1 && self.game_mode != 3 && self.level.difficulty_id() != 0 && self.ent_alive(Ent::Player)
    }

    /// Visibility share of a target: sneaking 0.8; invisible
    /// 0.7 × share of the four worn armor pieces (at least 0.1); then each piece worn
    /// in its equipment slot whose visibility component targets the observer's species
    /// (zombie, skeleton, creeper, piglin heads); result clamped to [0, 10].
    pub(crate) fn visibility(&self, target: Ent, viewer: &Mob) -> f64 {
        // Equipment per slot (equipment slot index) and state of the target.
        let mut gear: [Option<&Stack>; 8] = [None; 8];
        let (discrete, invisible) = match target {
            Ent::Player => {
                let sel = 36 + self.held as usize;
                for (k, idx) in [(0, sel), (1, 45), (2, 8), (3, 7), (4, 6), (5, 5)] {
                    gear[k] = self.slots.get(idx).and_then(Option::as_ref);
                }
                (self.sneaking, self.life.fx.has("minecraft:invisibility"))
            }
            Ent::Mob(id) => {
                let Some(o) = self.mob_ref(id) else { return 1.0 };
                for (k, g) in gear.iter_mut().enumerate() {
                    *g = o.equipment[k].as_ref();
                }
                (false, o.invisible())
            }
        };
        let mut v: f64 = 1.0;
        if discrete {
            v *= 0.8;
        }
        if invisible {
            let worn = gear[2..6].iter().filter(|g| g.is_some()).count();
            let cover = (worn as f32 / 4.0).max(0.1);
            v *= 0.7 * f64::from(cover);
        }
        const SLOT_NAMES: [&str; 8] = ["mainhand", "offhand", "feet", "legs", "chest", "head", "body", "saddle"];
        let viewer_ty = viewer.kind.info().name;
        for (k, g) in gear.iter().enumerate() {
            let Some(st) = g else { continue };
            let Some(mv) = self.comp_json(st, "minecraft:mob_visibility") else { continue };
            let Some(eq) = self.comp_json(st, "minecraft:equippable") else { continue };
            if eq["slot"].as_str() != Some(SLOT_NAMES[k]) {
                continue;
            }
            let types = mv.get("targeting_entity_types");
            if types.is_some_and(|t| self.entity_set_contains(t, viewer_ty)) {
                v *= mv["visibility"].as_f64().map_or(1.0, |x| f64::from(x as f32));
            }
        }
        v.clamp(0.0, 10.0)
    }

    /// Is the player carried by this mob, directly or through a stack of passengers?
    pub(crate) fn carries_player(&self, m: &Mob) -> bool {
        m.mount.passengers.iter().any(|p| match *p {
            Ent::Player => true,
            Ent::Mob(id) => self.mob_ref(id).is_some_and(|o| o.id != m.id && self.carries_player(o)),
        })
    }

    /// Set of species (identifier, `#…` tag or list) containing `ty`.
    fn entity_set_contains(&self, set: &serde_json::Value, ty: &str) -> bool {
        match set {
            serde_json::Value::String(x) => match x.strip_prefix('#') {
                Some(tag) => self.entity_type_in(ty, tag),
                None => x == ty || x.strip_prefix("minecraft:").is_none() && format!("minecraft:{x}") == ty,
            },
            serde_json::Value::Array(a) => a.iter().any(|v| self.entity_set_contains(v, ty)),
            _ => false,
        }
    }

    /// The targeting conditions test.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn targeting_ok(
        &self,
        m: &mut Mob,
        target: Ent,
        combat: bool,
        range: f64,
        sight: bool,
        invis: bool,
    ) -> bool {
        if target == Ent::Mob(m.id) || !self.ent_alive(target) {
            return false;
        }
        if target == Ent::Player && self.game_mode == 3 {
            return false;
        }
        if combat && target == Ent::Player && !self.player_attackable() {
            return false;
        }
        // Never a ghast as a target.
        if combat && matches!(target, Ent::Mob(id) if self.mob_ref(id).is_some_and(|o| o.kind == Kind::Ghast)) {
            return false;
        }
        // F06: the iron golem's can-attack test (built: never the player; never a creeper).
        if combat && !super::golems::can_attack(self, m, target) {
            return false;
        }
        // F07: baby villagers spared by illagers; allies. F09: the baby dolphin does not attack.
        if combat && (!super::illagers::can_attack(self, m, target) || !super::sealife::can_attack(m)) {
            return false;
        }
        // F10: the axolotl playing dead is not seen as an enemy.
        if combat
            && let Ent::Mob(id) = target
            && self.mob_ref(id).is_some_and(super::brainy::hidden_from_enemies)
        {
            return false;
        }
        if range > 0.0 {
            let modi = if invis { self.visibility(target, m) } else { 1.0 };
            let vis = (range * modi).max(2.0);
            let Some(p) = self.ent_pos(target) else { return false };
            if dist2(m.pos, p) > vis * vis {
                return false;
            }
        }
        if sight && !self.line_of_sight(m, target) {
            return false;
        }
        true
    }

    /// Line of sight (cache per tick).
    pub(crate) fn line_of_sight(&self, m: &mut Mob, target: Ent) -> bool {
        if super::illagers::blind(m) {
            return false;
        }
        if let Some(&(_, v)) = m.sight.iter().find(|(e, _)| *e == target) {
            return v;
        }
        let v = match (self.ent_pos(target), self.ent_eye(target)) {
            (Some(p), Some(eye)) => {
                let from = [m.pos[0], m.eye_y(), m.pos[2]];
                let to = [p[0], eye, p[2]];
                dist2(from, to) <= 128.0 * 128.0 && !self.clip_blocks(from, to)
            }
            _ => false,
        };
        m.sight.push((target, v));
        v
    }

    /// Does the segment touch a block collision shape? (`clip`, collision shapes,
    /// fluids ignored.)
    pub(crate) fn clip_blocks(&self, from: [f64; 3], to: [f64; 3]) -> bool {
        self.clip_t(from, to).is_some()
    }
    /// Nearest entity of a kind in a box around the mob, distance measured
    /// from its eyes (targeting) or its feet (fleeing).
    fn nearest_of(
        &self,
        m: &mut Mob,
        who: Who,
        inflate: [f64; 3],
        from_eye: bool,
        test: &mut dyn FnMut(&Server, &mut Mob, Ent) -> bool,
    ) -> Option<Ent> {
        let bb = m.bbox();
        let area = rc_physics::Aabb {
            min: [bb.min[0] - inflate[0], bb.min[1] - inflate[1], bb.min[2] - inflate[2]],
            max: [bb.max[0] + inflate[0], bb.max[1] + inflate[1], bb.max[2] + inflate[2]],
        };
        let mut cands: Vec<Ent> = Vec::new();
        if who == Who::Player {
            cands.push(Ent::Player);
        } else {
            for o in self.mobs.list.iter().flatten() {
                let fits = match who {
                    Who::Villager => matches!(o.kind, Kind::Villager | Kind::WanderingTrader),
                    Who::ZombieClass => o.kind.zombie_like(),
                    Who::Skeleton => o.kind.skeleton_like(),
                    Who::Endermite => o.kind == Kind::Endermite,
                    Who::WolfPrey => o.kind == Kind::Sheep || o.kind == Kind::Rabbit,
                    Who::Wolf => o.kind == Kind::Wolf,
                    Who::Cat => o.kind == Kind::Cat,
                    Who::Exact(k) => {
                        o.kind == k
                            || (k == Kind::Llama && o.kind == Kind::TraderLlama)
                            || (k == Kind::Guardian && o.kind == Kind::ElderGuardian)
                    }
                    Who::Monster => {
                        o.kind.is_monster() && !super::mounts::handles(o.kind) && !super::sealife::handles(o.kind)
                    }
                    Who::Chicken => o.kind == Kind::Chicken,
                    Who::BabyTurtleOnLand => o.kind == Kind::Turtle && o.is_baby() && !o.in_water,
                    Who::Rabbit => o.kind == Kind::Rabbit,
                    Who::Fox => o.kind == Kind::Fox,
                    Who::FoxPrey => matches!(o.kind, Kind::Chicken | Kind::Rabbit),
                    Who::SchoolingFish => matches!(o.kind, Kind::Cod | Kind::Salmon | Kind::TropicalFish),
                    Who::PolarBear => o.kind == Kind::PolarBear,
                    Who::IronGolem => o.kind == Kind::IronGolem,
                    Who::WildWolf => o.kind == Kind::Wolf && !o.tame,
                    Who::Zombie => o.kind.zombie_like(),
                    Who::Illager => super::illagers::is_illager(o.kind),
                    Who::Enemy => super::golems::is_enemy(o.kind),
                    Who::GolemPrey => super::golems::is_enemy(o.kind) && o.kind != Kind::Creeper,
                    Who::Axolotl => o.kind == Kind::Axolotl,
                    Who::Piglin => matches!(o.kind, Kind::Piglin | Kind::PiglinBrute),
                    Who::WitherPrey => !o.kind.undead(),
                    Who::Player => false,
                };
                if fits && o.touches(&area) {
                    cands.push(Ent::Mob(o.id));
                }
            }
            // F16: the "living entity" class contains the player.
            if who == Who::WitherPrey && self.player_aabb().intersects(&area) {
                cands.push(Ent::Player);
            }
        }
        let from = [m.pos[0], if from_eye { m.eye_y() } else { m.pos[1] }, m.pos[2]];
        let mut best: Option<(f64, Ent)> = None;
        for e in cands {
            if !test(self, m, e) {
                continue;
            }
            let Some(p) = self.ent_pos(e) else { continue };
            let d = dist2(from, p);
            if best.is_none_or(|b| d < b.0) {
                best = Some((d, e));
            }
        }
        best.map(|b| b.1)
    }

    /// Items whose box touches that of the mob widened by
    /// 32/16/32, sorted by distance; first wanted one, within 32 blocks, seen from the
    /// mob's eyes towards the item's (0.2125 above its base).
    pub(crate) fn nearest_wanted_item(&self, m: &Mob, mut wants: impl FnMut(&Stack) -> bool) -> Option<i32> {
        let bb = m.bbox();
        let area = rc_physics::Aabb {
            min: [bb.min[0] - 32.0, bb.min[1] - 16.0, bb.min[2] - 32.0],
            max: [bb.max[0] + 32.0, bb.max[1] + 16.0, bb.max[2] + 32.0],
        };
        let mut list: Vec<(f64, usize)> = self
            .life
            .drops
            .iter()
            .enumerate()
            .filter(|(_, d)| {
                let ib = rc_physics::Aabb {
                    min: [d.pos[0] - 0.125, d.pos[1], d.pos[2] - 0.125],
                    max: [d.pos[0] + 0.125, d.pos[1] + 0.25, d.pos[2] + 0.125],
                };
                !d.is_removed() && ib.intersects(&area)
            })
            .map(|(i, d)| (dist2(m.pos, d.pos), i))
            .collect();
        list.sort_by(|a, b| a.0.total_cmp(&b.0));
        let eye = [m.pos[0], m.eye_y(), m.pos[2]];
        list.into_iter()
            .map(|(d, i)| (d, &self.life.drops[i]))
            .find(|(d, it)| {
                let p = it.pos;
                wants(&it.stack) && *d < 32.0 * 32.0 && !self.clip_blocks(eye, [p[0], p[1] + f64::from(0.2125f32), p[2]])
            })
            .map(|(_, it)| it.id)
    }

    fn holding(&self, f: Food) -> bool {
        let held = [&self.slots[36 + self.held as usize], &self.slots[45]];
        held.iter().any(|s| {
            s.as_ref().is_some_and(|s| match f {
                Food::Item(i) => s.id == i,
                Food::Tag(t) => self.item_tag(&s.id, t),
            })
        })
    }

    // ---------- Goal operations ----------

    pub(crate) fn goal_can_use(&mut self, g: &mut Goal, m: &mut Mob) -> bool {
        match g {
            Goal::Extra(x) => self.xg_can_use(x, m),
            Goal::Cube(c) => self.cube_goal_can_use(c, m),
            Goal::Mount(g) => self.mount_can_use(g, m),
            Goal::Npc(t) => self.tgoal_can_use(t, m),
            Goal::Golem(g) => self.golem_can_use(g, m),
            Goal::Ill(g) => self.ill_can_use(g, m),
            Goal::Aqua(a) => self.aqua_can_use(a, m),
            Goal::Sea(a) => self.sea_can_use(a, m),
            Goal::Wild(w) => self.wild_can_use(w, m),
            Goal::Neth(g) => self.neth_can_use(g, m),
            Goal::Rec(g) => self.rec_can_use(g, m),
            Goal::End(g) => self.end_can_use(g, m),
            Goal::Wither(g) => self.wither_can_use(g, m),
            Goal::Float => (m.in_water && m.water_height > self.fluid_jump_threshold(m)) || m.lava_height > 0.0,
            Goal::Panic { pos, tag, .. } => {
                if !self.should_panic(m, super::surface::panic_tag(m, tag)) {
                    return false;
                }
                if m.on_fire()
                    && let Some(w) = self.look_for_water(m, 5)
                {
                    *pos = [f64::from(w[0]), f64::from(w[1]), f64::from(w[2])];
                    return true;
                }
                match self.default_random_pos(m, 5, 4) {
                    Some(p) => {
                        *pos = p;
                        true
                    }
                    None => false,
                }
            }
            Goal::Breed { partner, .. } => {
                if !m.is_in_love() || !super::surface::can_breed(m) {
                    return false;
                }
                *partner = self.free_partner(m);
                partner.is_some()
            }
            Goal::Tempt { calm, food, .. } => {
                if *calm > 0 {
                    *calm -= 1;
                    return false;
                }
                let range = m.attrs.tempt_range.value();
                let f = *food;
                // The tamed cat no longer follows the fish.
                self.holding(f)
                    && self.targeting_ok(m, Ent::Player, false, range, false, true)
                    && !(m.kind == Kind::Cat && m.tame)
            }
            Goal::FollowParent { parent, .. } => {
                if m.age >= 0 {
                    return false;
                }
                let bb = m.bbox();
                let area = rc_physics::Aabb {
                    min: [bb.min[0] - 8.0, bb.min[1] - 4.0, bb.min[2] - 8.0],
                    max: [bb.max[0] + 8.0, bb.max[1] + 4.0, bb.max[2] + 8.0],
                };
                let mut best: Option<(f64, i32)> = None;
                // Same class or subclass: the baby llama also follows a trader's llama.
                let same = |k: Kind| k == m.kind || (m.kind == Kind::Llama && k == Kind::TraderLlama);
                for o in self.mobs.list.iter().flatten() {
                    if same(o.kind) && o.age >= 0 && o.touches(&area) {
                        let d = dist2(m.pos, o.pos);
                        if best.is_none_or(|b| d <= b.0) {
                            best = Some((d, o.id));
                        }
                    }
                }
                match best {
                    Some((d, id)) if d >= 9.0 => {
                        *parent = Some(id);
                        true
                    }
                    _ => false,
                }
            }
            Goal::Stroll { interval, prob, wanted, .. } => {
                if m.no_action_time >= 100 {
                    return false;
                }
                if m.rng.next_int_bound(reduced(*interval)) != 0 {
                    return false;
                }
                let p = match prob {
                    None => self.default_random_pos(m, 10, 7),
                    Some(pr) => {
                        if m.in_water {
                            self.land_random_pos(m, 15, 7).or_else(|| self.default_random_pos(m, 10, 7))
                        } else if m.rng.next_float() >= *pr {
                            self.land_random_pos(m, 10, 7)
                        } else {
                            self.default_random_pos(m, 10, 7)
                        }
                    }
                };
                match p {
                    Some(p) => {
                        *wanted = p;
                        true
                    }
                    None => false,
                }
            }
            Goal::LookAtPlayer { dist, prob, at, .. } => {
                if m.rng.next_float() >= *prob {
                    return false;
                }
                if let Some(t) = m.target {
                    *at = Some(t);
                }
                let d = f64::from(*dist);
                // The player riding this mob (even through an intermediate vehicle) is not
                // looked at.
                *at = (!self.carries_player(m) && self.targeting_ok(m, Ent::Player, false, d, true, true))
                    .then_some(Ent::Player);
                at.is_some()
            }
            Goal::LookAround { .. } => m.rng.next_float() < 0.02,
            Goal::Melee { path, last, kind, .. } => {
                let now = self.time;
                if now - *last < 20 {
                    return false;
                }
                *last = now;
                let Some(t) = m.target else { return false };
                if !self.ent_alive(t) {
                    return false;
                }
                let Some(tp) = self.ent_pos(t) else { return false };
                *path = self.create_path(m, super::mth::block_of(tp), 0);
                // The spider carrying a passenger (jockey) does not attack in melee.
                (path.is_some() || self.within_melee(m, t)) && !(*kind == Melee::Spider && !m.mount.passengers.is_empty())
            }
            Goal::Bow { .. } => m.target.is_some() && self.holds_bow(m),
            Goal::Swell { .. } => {
                m.swell_dir > 0
                    || m.target
                        .is_some_and(|t| self.ent_alive(t) && self.ent_pos(t).is_some_and(|p| dist2(m.pos, p) < 9.0))
            }
            Goal::Leap { target, .. } => {
                *target = m.target;
                let Some(t) = m.target else { return false };
                let Some(p) = self.ent_pos(t) else { return false };
                let d = dist2(m.pos, p);
                if !(4.0..=16.0).contains(&d) || !m.on_ground {
                    return false;
                }
                m.rng.next_int_bound(reduced(5)) == 0
            }
            Goal::Avoid { who, max, from, path, .. } => {
                // The tamed cat no longer flees the player.
                if !super::surface::avoid_allowed(m) || (m.kind == Kind::Cat && m.tame) {
                    return false;
                }
                let (w, mx) = (*who, f64::from(*max));
                // "Combat" conditions for any target, player included: in Peaceful, the
                // player is never fled.
                let f = self.nearest_of(m, w, [mx, 3.0, mx], false, &mut |s, m, e| {
                    (e != Ent::Player || (s.game_mode != 1 && s.game_mode != 3))
                        && !(w == Who::Exact(Kind::Armadillo)
                            && matches!(e, Ent::Mob(id) if s.mob_ref(id).is_some_and(super::brainy::armadillo::is_scared)))
                        && s.targeting_ok(m, e, true, mx, true, true)
                });
                *from = f;
                let Some(f) = f else { return false };
                let Some(fp) = self.ent_pos(f) else { return false };
                let Some(p) = self.default_random_pos_away(m, 16, 7, fp) else { return false };
                if dist2(fp, p) < dist2(fp, m.pos) {
                    return false;
                }
                *path = self.create_path(m, super::mth::block_of(p), 0);
                if path.is_none() {
                    return false;
                }
                // Wolf facing a llama: wild only, and according to the llama's strength (draw after
                // the flee search).
                if m.kind == Kind::Wolf && w == Who::Exact(Kind::Llama) {
                    let strength = match f {
                        Ent::Mob(id) => self.mob_ref(id).map_or(0, |l| l.mount.strength),
                        Ent::Player => 0,
                    };
                    return !m.tame && strength >= m.rng.next_int_bound(5);
                }
                true
            }
            Goal::EatBlock { .. } => {
                let n = g.adjusted(if m.is_baby() { 50 } else { 1000 });
                if m.rng.next_int_bound(n) != 0 {
                    return false;
                }
                let p = m.block_pos();
                self.btag_id(p, crate::tag_bits::TagId::EdibleForSheep)
                    || self.bname([p[0], p[1] - 1, p[2]]) == "minecraft:grass_block"
            }
            Goal::RestrictSun => self.bright_outside() && m.equipment[5].is_none(),
            Goal::FleeSun { wanted, .. } => {
                if m.target.is_some() || !self.bright_outside() || !m.on_fire() || !self.can_see_sky(m.block_pos()) {
                    return false;
                }
                if m.equipment[5].is_some() {
                    return false;
                }
                let b = m.block_pos();
                for _ in 0..10 {
                    let p = [
                        b[0] + m.rng.next_int_bound(20) - 10,
                        b[1] + m.rng.next_int_bound(6) - 3,
                        b[2] + m.rng.next_int_bound(20) - 10,
                    ];
                    if !self.can_see_sky(p) && self.walk_target_value(m, p) < 0.0 {
                        *wanted = [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5];
                        return true;
                    }
                }
                false
            }
            Goal::SitOrdered => {
                if !m.sitting && !m.tame {
                    return false;
                }
                if m.in_water || !m.on_ground {
                    return false;
                }
                if !self.owner_here(m) {
                    return true;
                }
                if dist2(m.pos, self.pos) < 144.0 && self.mobs.player_hurt_by.is_some() {
                    return false;
                }
                m.sitting
            }
            Goal::FollowOwner { start, .. } => {
                // Sitting, passenger or spectator owner: does not move towards them.
                if !self.owner_here(m) || m.sitting || m.mount.vehicle.is_some() || self.game_mode == 3 {
                    return false;
                }
                dist2(m.pos, self.pos) >= f64::from(*start * *start)
            }
            Goal::Beg { dist, .. } => {
                let d = f64::from(*dist);
                self.targeting_ok(m, Ent::Player, false, d, true, true)
                    && (self.holding(Food::Item("minecraft:bone")) || self.holding(Food::Tag("minecraft:wolf_food")))
            }
            Goal::OcelotAttack { target, .. } => {
                *target = m.target;
                m.target.is_some()
            }
            Goal::CatRelax { goal_pos, .. } => self.cat_relax_can_use(m, goal_pos),
            Goal::EndermanLeave => {
                m.carried.is_some()
                    && self.rule("minecraft:mob_griefing", true)
                    && m.rng.next_int_bound(reduced(2000)) == 0
            }
            Goal::EndermanTake => {
                m.carried.is_none()
                    && self.rule("minecraft:mob_griefing", true)
                    && m.rng.next_int_bound(reduced(20)) == 0
            }
            Goal::EndermanFreeze { target } => {
                *target = m.target;
                let Some(Ent::Player) = m.target else { return false };
                dist2(m.pos, self.pos) <= 256.0 && self.player_looking_at(m)
            }
            Goal::HurtBy { base, stamp, .. } => {
                let Some((by, ts)) = m.last_hurt_by_mob else { return false };
                if ts == *stamp {
                    return false;
                }
                if by == Ent::Player && self.rule("minecraft:universal_anger", false) {
                    return false;
                }
                if super::illagers::ignores_attacker(self, m, by) || super::end::shulker::ignores_attacker(self, m, by) {
                    return false;
                }
                self.target_can_attack(base, m, by, false, false)
            }
            Goal::Nearest { who, interval, target, angry_only, non_tame, .. } => {
                if *non_tame && m.tame {
                    return false;
                }
                if m.kind.base() == Kind::Spider && self.magic_light(m.block_pos()) >= 0.5 {
                    return false;
                }
                if *interval > 0 && m.rng.next_int_bound(*interval) != 0 {
                    return false;
                }
                let follow = m.follow_range();
                // The line of sight is always required when choosing the target; `must_see` only serves
                // for the chase.
                let (w, angry) = (*who, *angry_only);
                let creative_ok = |s: &Server, e: Ent| e != Ent::Player || s.player_attackable();
                *target = if w == Who::Player {
                    // F03: cubes only aim at a player at most 4 blocks in height away.
                    let ok = creative_ok(self, Ent::Player)
                        && (!m.kind.is_cube() || (self.pos[1] - m.pos[1]).abs() <= 4.0)
                        && super::nether::player_target_filter(self, m)
                        && (!angry
                            || m.anger_target == Some(Ent::Player)
                            || (m.anger_target.is_none()
                                && m.anger_end > 0
                                && m.anger_end > self.time
                                && self.rule("minecraft:universal_anger", false)))
                        && self.targeting_ok(m, Ent::Player, true, follow, true, true);
                    ok.then_some(Ent::Player)
                } else {
                    self.nearest_of(m, w, [follow, follow, follow], true, &mut |s, m, e| {
                        s.targeting_ok(m, e, true, follow, true, true)
                    })
                };
                target.is_some()
            }
            Goal::OwnerHurtBy { base, stamp, by } => {
                if !m.tame || m.sitting || !self.owner_here(m) {
                    return false;
                }
                // Last damage source of the owner less than 100 ticks old, outside exemption.
                let recent = self.life.vit.last_damage.as_ref().is_some_and(|(ty, _, t)| {
                    self.tick.saturating_sub(*t) <= 100 && !self.dmg_tag(ty, "minecraft:no_wolf_retaliation")
                });
                if !recent {
                    return false;
                }
                let Some((who, ts)) = self.mobs.player_hurt_by else { return false };
                *by = Some(who);
                ts != *stamp && self.target_can_attack(base, m, who, true, true) && self.wolf_wants_to_attack(who)
            }
            Goal::OwnerHurt { base, stamp, hit } => {
                if !m.tame || m.sitting || !self.owner_here(m) {
                    return false;
                }
                let Some((who, ts)) = self.mobs.player_hit else { return false };
                *hit = Some(who);
                ts != *stamp && self.target_can_attack(base, m, who, true, true) && self.wolf_wants_to_attack(who)
            }
            Goal::ResetAnger { stamp, .. } => {
                self.rule("minecraft:universal_anger", false)
                    && m.last_hurt_by_mob.is_some_and(|(e, ts)| e == Ent::Player && ts > *stamp)
            }
            Goal::Surf(sg) => self.surf_can_use(sg, m),
            Goal::Spear(_) => self.spear_goal_can_use(m),
            Goal::EndermanLook { pending, .. } => {
                let ok = self.player_attackable()
                    && (self.player_looking_at(m) || m.anger_target == Some(Ent::Player))
                    && self.targeting_ok(m, Ent::Player, true, m.follow_range(), false, true);
                *pending = ok.then_some(Ent::Player);
                pending.is_some()
            }
        }
    }

    pub(crate) fn goal_can_continue(&mut self, g: &mut Goal, m: &mut Mob) -> bool {
        match g {
            Goal::Extra(x) => self.xg_can_continue(x, m),
            Goal::Panic { .. } | Goal::Stroll { .. } | Goal::FleeSun { .. } => !m.nav.is_done(),
            Goal::Avoid { .. } => {
                !m.nav.is_done() && super::surface::avoid_allowed(m) && !(m.kind == Kind::Cat && m.tame)
            }
            Goal::Surf(sg) => self.surf_can_continue(sg, m),
            Goal::Spear(sg) => self.spear_goal_can_continue(sg, m),
            Goal::Cube(c) => self.cube_goal_can_continue(c, m),
            Goal::Mount(g) => self.mount_can_continue(g, m),
            Goal::Npc(t) => self.tgoal_can_continue(t, m),
            Goal::Golem(g) => self.golem_can_continue(g, m),
            Goal::Ill(g) => self.ill_can_continue(g, m),
            Goal::Aqua(a) => self.aqua_can_continue(a, m),
            Goal::Sea(a) => self.sea_can_continue(a, m),
            Goal::Wild(w) => self.wild_can_continue(w, m),
            Goal::Neth(g) => self.neth_can_continue(g, m),
            Goal::Rec(g) => self.rec_can_continue(g, m),
            Goal::End(g) => self.end_can_continue(g, m),
            Goal::Wither(g) => self.wither_can_continue(g, m),
            Goal::Breed { partner, love_time, .. } => {
                let Some(p) = partner.and_then(|id| self.mob_ref(id)) else { return false };
                p.alive() && p.is_in_love() && *love_time < 60 && !self.is_panicking(p)
            }
            Goal::Tempt { scare, p, prot, .. } => {
                if *scare && super::surface::can_scare(m) {
                    if dist2(m.pos, self.pos) < 36.0 {
                        if dist2(self.pos, *p) > 0.010_000_000_000_000_002 {
                            return false;
                        }
                        if (self.pitch - prot[0]).abs() > 5.0 || (self.yaw - prot[1]).abs() > 5.0 {
                            return false;
                        }
                    } else {
                        *p = self.pos;
                    }
                    *prot = [self.pitch, self.yaw];
                }
                self.goal_can_use(g, m)
            }
            Goal::FollowParent { parent, .. } => {
                if m.age >= 0 {
                    return false;
                }
                let Some(p) = parent.and_then(|id| self.mob_ref(id)) else { return false };
                if !p.alive() {
                    return false;
                }
                let d = dist2(m.pos, p.pos);
                (9.0..=256.0).contains(&d)
            }
            Goal::LookAtPlayer { dist, at, time, .. } => {
                let Some(e) = *at else { return false };
                if !self.ent_alive(e) {
                    return false;
                }
                let d = f64::from(*dist);
                self.ent_pos(e).is_some_and(|p| dist2(m.pos, p) <= d * d) && *time > 0
            }
            Goal::LookAround { time, .. } => *time >= 0,
            Goal::Melee { unseen, kind, .. } => {
                if *kind == Melee::Spider && self.magic_light(m.block_pos()) >= 0.5 && m.rng.next_int_bound(100) == 0 {
                    m.target = None;
                    return false;
                }
                let Some(t) = m.target else { return false };
                if !self.ent_alive(t) {
                    return false;
                }
                if !*unseen {
                    return !m.nav.is_done();
                }
                !(t == Ent::Player && (self.game_mode == 1 || self.game_mode == 3))
            }
            Goal::Bow { .. } => (m.target.is_some() || !m.nav.is_done()) && self.holds_bow(m),
            Goal::Leap { .. } => !m.on_ground,
            Goal::EatBlock { tick } => *tick > 0,
            Goal::SitOrdered => m.sitting,
            Goal::FollowOwner { stop, .. } => {
                if m.nav.is_done()
                    || m.sitting
                    || m.mount.vehicle.is_some()
                    || self.game_mode == 3
                    || !self.owner_here(m)
                {
                    return false;
                }
                dist2(m.pos, self.pos) > f64::from(*stop * *stop)
            }
            Goal::Beg { dist, time } => {
                let d = f64::from(*dist);
                self.ent_alive(Ent::Player)
                    && dist2(m.pos, self.pos) <= d * d
                    && *time > 0
                    && (self.holding(Food::Item("minecraft:bone")) || self.holding(Food::Tag("minecraft:wolf_food")))
            }
            Goal::OcelotAttack { target, .. } => {
                let Some(t) = *target else { return false };
                if !self.ent_alive(t) {
                    return false;
                }
                let Some(p) = self.ent_pos(t) else { return false };
                if dist2(m.pos, p) > 225.0 {
                    return false;
                }
                !m.nav.is_done() || self.goal_can_use(g, m)
            }
            Goal::EndermanFreeze { target } => {
                let Some(t) = *target else { return false };
                t == Ent::Player && dist2(m.pos, self.pos) <= 256.0 && self.player_looking_at(m)
            }
            Goal::Nearest { base, non_tame, target, .. } if *non_tame => {
                let Some(t) = *target else { return false };
                let _ = base;
                let follow = m.follow_range();
                self.targeting_ok(m, t, true, follow, false, true)
            }
            Goal::HurtBy { base, .. }
            | Goal::Nearest { base, .. }
            | Goal::OwnerHurtBy { base, .. }
            | Goal::OwnerHurt { base, .. }
            | Goal::EndermanLook { base, .. } => self.target_continue(base, m),
            Goal::CatRelax { goal_pos, .. } => self.cat_relax_can_continue(m, *goal_pos),
            _ => self.goal_can_use(g, m),
        }
    }

    pub(crate) fn goal_start(&mut self, g: &mut Goal, m: &mut Mob) {
        match g {
            Goal::Extra(x) => self.xg_start(x, m),
            Goal::Cube(c) => self.cube_goal_start(c, m),
            Goal::Mount(g) => self.mount_start(g, m),
            Goal::Npc(t) => self.tgoal_start(t, m),
            Goal::Golem(g) => self.golem_start(g, m),
            Goal::Ill(g) => self.ill_start(g, m),
            Goal::Aqua(a) => self.aqua_start(a, m),
            Goal::Sea(a) => self.sea_start(a, m),
            Goal::Wild(w) => self.wild_start(w, m),
            Goal::Neth(g) => self.neth_start(g, m),
            Goal::Rec(g) => self.rec_start(g, m),
            Goal::End(g) => self.end_start(g, m),
            Goal::Wither(g) => self.wither_start(g, m),
            Goal::Panic { speed, pos, .. } => {
                let (s, p) = (*speed, *pos);
                self.move_to_pos(m, p, s);
            }
            Goal::Tempt { p, on, .. } => {
                *p = self.pos;
                *on = true;
            }
            Goal::FollowParent { recalc, .. } => *recalc = 0,
            Goal::Stroll { speed, wanted, .. } => {
                let (s, w) = (*speed, *wanted);
                self.move_to_pos(m, w, s);
            }
            Goal::LookAtPlayer { time, .. } => *time = reduced(40 + m.rng.next_int_bound(40)),
            Goal::LookAround { rel, time } => {
                let r = std::f64::consts::TAU * m.rng.next_double();
                *rel = [r.cos(), r.sin()];
                *time = 20 + m.rng.next_int_bound(20);
            }
            Goal::Melee { speed, path, recalc, cd, kind, .. } => {
                let s = *speed;
                let p = path.take();
                self.move_to_path(m, p, s);
                m.aggressive = true;
                *recalc = 0;
                *cd = 0;
                if let Melee::Zombie { raise } = kind {
                    *raise = 0;
                }
            }
            Goal::Bow { .. } => m.aggressive = true,
            Goal::Swell { target } => {
                m.nav.stop();
                *target = m.target;
            }
            Goal::Leap { yd, target } => {
                let Some(t) = *target else { return };
                let Some(tp) = self.ent_pos(t) else { return };
                let mv = m.vel;
                let mut d = [tp[0] - m.pos[0], 0.0, tp[2] - m.pos[2]];
                let l = d[0] * d[0] + d[2] * d[2];
                if l > 1.0e-7 {
                    let n = l.sqrt();
                    d = [d[0] / n * 0.4 + mv[0] * 0.2, 0.0, d[2] / n * 0.4 + mv[2] * 0.2];
                }
                m.vel = [d[0], f64::from(*yd), d[2]];
                m.needs_sync = true;
            }
            Goal::Avoid { walk, path, who, .. } => {
                if m.kind == Kind::Wolf && *who == Who::Exact(Kind::Llama) {
                    m.target = None;
                }
                let s = *walk;
                let p = path.take();
                self.move_to_path(m, p, s);
            }
            Goal::EatBlock { tick } => {
                *tick = reduced(40);
                self.mob_event(m, 10);
                m.nav.stop();
            }
            Goal::RestrictSun => m.nav.avoid_sun = true,
            Goal::FleeSun { speed, wanted } => {
                let (s, w) = (*speed, *wanted);
                self.move_to_pos(m, w, s);
            }
            Goal::SitOrdered => {
                m.nav.stop();
                m.sitting = true;
            }
            Goal::FollowOwner { recalc, old_water, .. } => {
                *recalc = 0;
                *old_water = super::nav::mob_malus(m, super::nav::PathType::Water);
                super::nav::set_malus(m, super::nav::PathType::Water, 0.0);
            }
            Goal::Beg { time, .. } => {
                *time = reduced(40 + m.rng.next_int_bound(40));
                m.set(super::mob::WOLF_INTERESTED, rc_proto_v777::entity::Meta::Bool(true));
            }
            Goal::EndermanFreeze { .. } => m.nav.stop(),
            Goal::CatRelax { goal_pos: Some(p), .. } => {
                m.cat_sit_pose = false;
                let t = p.map(f64::from);
                self.move_to_pos(m, t, f64::from(1.1f32));
            }
            Goal::HurtBy { base, alert, stamp } => {
                super::piglins::zombified::before_set_target(m, m.last_hurt_by_mob.map(|h| h.0)); // F13
                m.target = m.last_hurt_by_mob.map(|h| h.0);
                base.target_mob = m.target;
                *stamp = m.last_hurt_by_mob.map_or(0, |h| h.1);
                base.unseen_memory = 300;
                if *alert {
                    self.alert_others(m);
                }
                Self::target_start(base);
            }
            Goal::Nearest { base, target, .. } => {
                super::piglins::zombified::before_set_target(m, *target); // F13
                m.target = *target;
                Self::target_start(base);
            }
            Goal::OwnerHurtBy { base, stamp, by } => {
                m.target = *by;
                *stamp = self.mobs.player_hurt_by.map_or(0, |h| h.1);
                Self::target_start(base);
            }
            Goal::OwnerHurt { base, stamp, hit } => {
                m.target = *hit;
                *stamp = self.mobs.player_hit.map_or(0, |h| h.1);
                Self::target_start(base);
            }
            Goal::ResetAnger { stamp, alert } => {
                *stamp = m.last_hurt_by_mob.map_or(0, |h| h.1);
                let time = self.time;
                forget_and_refresh_universal_anger(m, time);
                if *alert {
                    // Same-class kin (unit cube box + follow range, 10, follow range).
                    let follow = m.follow_range();
                    let p = m.pos;
                    let area = rc_physics::Aabb {
                        min: [p[0] - follow, p[1] - 10.0, p[2] - follow],
                        max: [p[0] + 1.0 + follow, p[1] + 1.0 + 10.0, p[2] + 1.0 + follow],
                    };
                    for o in self.mobs.list.iter_mut().flatten() {
                        if o.kind == m.kind && o.id != m.id && o.touches(&area) {
                            forget_and_refresh_universal_anger(o, time);
                        }
                    }
                }
            }
            Goal::Surf(sg) => self.surf_start(sg, m),
            Goal::Spear(sg) => self.spear_goal_start(sg, m),
            Goal::EndermanLook { base, aggro, teleport, .. } => {
                *aggro = reduced(5);
                *teleport = 0;
                m.stared_at = true;
                Self::target_start(base);
            }
            _ => {}
        }
    }

    pub(crate) fn goal_stop(&mut self, g: &mut Goal, m: &mut Mob) {
        match g {
            Goal::Extra(x) => self.xg_stop(x, m),
            Goal::CatRelax { on_bed, .. } => self.cat_relax_stop(m, on_bed),
            Goal::Cube(c) => self.cube_goal_stop(c, m),
            Goal::Mount(g) => self.mount_stop(g, m),
            Goal::Npc(t) => self.tgoal_stop(t, m),
            Goal::Golem(g) => self.golem_stop(g, m),
            Goal::Ill(g) => self.ill_stop(g, m),
            Goal::Aqua(a) => self.aqua_stop(a, m),
            Goal::Sea(a) => self.sea_stop(a, m),
            Goal::Wild(w) => self.wild_stop(w, m),
            Goal::Neth(g) => self.neth_stop(g, m),
            Goal::Rec(g) => self.rec_stop(g, m),
            Goal::End(g) => self.end_stop(g, m),
            Goal::Wither(g) => self.wither_stop(g, m),
            Goal::Stroll { .. } => m.nav.stop(),
            Goal::Breed { partner, love_time, .. } => {
                *partner = None;
                *love_time = 0;
            }
            Goal::Tempt { calm, on, .. } => {
                m.nav.stop();
                *calm = reduced(100);
                *on = false;
            }
            Goal::FollowParent { parent, .. } => *parent = None,
            Goal::LookAtPlayer { at, .. } => *at = None,
            Goal::Surf(sg) => self.surf_stop(sg, m),
            Goal::Spear(sg) => self.spear_goal_stop(sg, m),
            Goal::Melee { kind, .. } => {
                if *kind == Melee::PolarBear {
                    m.surf.standing = false;
                }
                if m.target == Some(Ent::Player) && (self.game_mode == 1 || self.game_mode == 3) {
                    m.target = None;
                }
                m.aggressive = false;
                m.nav.stop();
            }
            Goal::Bow { see, attack_time, .. } => {
                m.aggressive = false;
                *see = 0;
                *attack_time = -1;
                m.use_ticks = -1;
            }
            Goal::Swell { target } => *target = None,
            Goal::Avoid { from, .. } => *from = None,
            Goal::EatBlock { tick } => *tick = 0,
            Goal::RestrictSun => m.nav.avoid_sun = false,
            Goal::SitOrdered => m.sitting = false,
            Goal::FollowOwner { old_water, .. } => {
                m.nav.stop();
                super::nav::set_malus(m, super::nav::PathType::Water, *old_water);
            }
            Goal::Beg { .. } => m.set(super::mob::WOLF_INTERESTED, rc_proto_v777::entity::Meta::Bool(false)),
            Goal::OcelotAttack { target, .. } => {
                *target = None;
                m.nav.stop();
            }
            Goal::EndermanLook { base, .. } => {
                m.stared_at = false;
                m.target = None;
                base.target_mob = None;
            }
            Goal::HurtBy { base, .. }
            | Goal::Nearest { base, .. }
            | Goal::OwnerHurtBy { base, .. }
            | Goal::OwnerHurt { base, .. } => {
                m.target = None;
                base.target_mob = None;
            }
            _ => {}
        }
    }

    pub(crate) fn goal_tick(&mut self, g: &mut Goal, m: &mut Mob) {
        match g {
            Goal::Extra(x) => self.xg_tick(x, m),
            Goal::Cube(c) => self.cube_goal_tick(c, m),
            Goal::Mount(g) => self.mount_tick(g, m),
            Goal::Npc(t) => self.tgoal_tick(t, m),
            Goal::Golem(g) => self.golem_tick(g, m),
            Goal::Ill(g) => self.ill_tick(g, m),
            Goal::Aqua(a) => self.aqua_tick(a, m),
            Goal::Sea(a) => self.sea_tick(a, m),
            Goal::Wild(w) => self.wild_tick(w, m),
            Goal::Neth(g) => self.neth_tick(g, m),
            Goal::Rec(g) => self.rec_tick(g, m),
            Goal::End(g) => self.end_tick(g, m),
            Goal::Wither(g) => self.wither_goal_tick(g, m),
            Goal::Float => {
                if m.rng.next_float() < 0.8 {
                    m.jump_ctl = true;
                }
            }
            Goal::Breed { speed, partner, love_time } => {
                let Some(pid) = *partner else { return };
                let Some(p) = self.mob_ref(pid).map(|p| (p.pos, p.eye_y())) else { return };
                let x = max_head_x(m);
                self.look_at(m, [p.0[0], p.1, p.0[2]], 10.0, x);
                let s = *speed;
                self.move_to_entity(m, p.0, s);
                *love_time += 1;
                if *love_time >= reduced(60) && dist2(m.pos, p.0) < 9.0 {
                    self.breed(m, pid);
                }
            }
            Goal::Tempt { speed, stop, .. } => {
                let eye = self.player_eye_y();
                // Look: species' max head deviation + 20, species' max pitch.
                self.look_at(m, [self.pos[0], eye, self.pos[2]], max_head_y(m.kind) + 20.0, max_head_x(m));
                if dist2(m.pos, self.pos) < *stop * *stop {
                    m.nav.stop();
                } else {
                    let s = *speed;
                    let p = self.pos;
                    self.move_to_entity(m, p, s);
                }
            }
            Goal::FollowParent { speed, parent, recalc } => {
                *recalc -= 1;
                if *recalc <= 0 {
                    *recalc = reduced(10);
                    if let Some(p) = parent.and_then(|id| self.mob_ref(id)).map(|p| p.pos) {
                        let s = *speed;
                        self.move_to_entity(m, p, s);
                    }
                }
            }
            Goal::LookAtPlayer { at, time, .. } => {
                let Some(e) = *at else { return };
                if self.ent_alive(e)
                    && let (Some(p), Some(eye)) = (self.ent_pos(e), self.ent_eye(e))
                {
                    self.look_at(m, [p[0], eye, p[2]], 10.0, max_head_x(m));
                    *time -= 1;
                }
            }
            Goal::LookAround { rel, time } => {
                *time -= 1;
                let eye = m.eye_y();
                self.look_at(m, [m.pos[0] + rel[0], eye, m.pos[2] + rel[1]], 10.0, max_head_x(m));
            }
            Goal::Melee { .. } => self.melee_tick(g, m),
            Goal::Surf(sg) => self.surf_tick(sg, m),
            Goal::Spear(sg) => self.spear_goal_tick(sg, m),
            Goal::Panic { speed, .. } => {
                let s = *speed;
                self.surf_panic_tick(m, s);
            }
            Goal::Bow { .. } => self.bow_tick(g, m),
            Goal::Swell { target } => {
                m.swell_dir = match *target {
                    Some(t) if self.ent_alive(t) => {
                        let far = self.ent_pos(t).is_none_or(|p| dist2(m.pos, p) > 49.0);
                        if far || !self.line_of_sight(m, t) { -1 } else { 1 }
                    }
                    _ => -1,
                };
            }
            Goal::Avoid { from, walk, sprint, who, .. } => {
                if m.kind == Kind::Wolf && *who == Who::Exact(Kind::Llama) {
                    m.target = None;
                }
                let Some(f) = from.and_then(|f| self.ent_pos(f)) else { return };
                m.nav.speed_mod = if dist2(m.pos, f) < 49.0 { *sprint } else { *walk };
            }
            Goal::EatBlock { tick } => {
                *tick = (*tick - 1).max(0);
                if *tick == reduced(4) {
                    let p = m.block_pos();
                    let griefing = self.rule("minecraft:mob_griefing", true);
                    if self.btag_id(p, crate::tag_bits::TagId::EdibleForSheep) {
                        if griefing {
                            self.destroy_block(p, false, 512);
                        }
                        self.mob_ate(m);
                    } else {
                        let below = [p[0], p[1] - 1, p[2]];
                        if self.bname(below) == "minecraft:grass_block" {
                            if griefing {
                                let grass = self.st(below);
                                self.level_event(2001, below, grass as i32);
                                let dirt = self.default_state("minecraft:dirt");
                                self.set_block(below, dirt, 2);
                            }
                            self.mob_ate(m);
                        }
                    }
                }
            }
            Goal::FollowOwner { speed, recalc, .. } => {
                let far = dist2(m.pos, self.pos) >= 144.0;
                if !far {
                    let (eye, x) = (self.player_eye_y(), max_head_x(m));
                    self.look_at(m, [self.pos[0], eye, self.pos[2]], 10.0, x);
                }
                *recalc -= 1;
                if *recalc <= 0 {
                    *recalc = reduced(10);
                    if far {
                        self.teleport_to_owner(m);
                    } else {
                        let s = *speed;
                        let p = self.pos;
                        self.move_to_entity(m, p, s);
                    }
                }
            }
            Goal::Beg { time, .. } => {
                let (eye, x) = (self.player_eye_y(), max_head_x(m));
                self.look_at(m, [self.pos[0], eye, self.pos[2]], 10.0, x);
                *time -= 1;
            }
            Goal::OcelotAttack { target, time } => {
                let Some(t) = *target else { return };
                let (Some(p), Some(eye)) = (self.ent_pos(t), self.ent_eye(t)) else { return };
                self.look_at(m, [p[0], eye, p[2]], 30.0, 30.0);
                let w = f64::from(m.width() * 2.0);
                let r = w * w;
                let d = dist2(m.pos, p);
                let speed = if d > r && d < 16.0 {
                    1.33
                } else if d < 225.0 {
                    0.6
                } else {
                    0.8
                };
                self.move_to_entity(m, p, speed);
                *time = (*time - 1).max(0);
                if d <= r && *time <= 0 {
                    *time = 20;
                    self.mob_attack(m, t);
                }
            }
            Goal::EndermanLeave => self.enderman_leave_tick(m),
            Goal::CatRelax { goal_pos, on_bed } => self.cat_relax_tick(m, *goal_pos, on_bed),
            Goal::EndermanTake => self.enderman_take_tick(m),
            Goal::EndermanFreeze { .. } => {
                let eye = self.player_eye_y();
                self.look_at(m, [self.pos[0], eye, self.pos[2]], 10.0, 40.0);
            }
            Goal::EndermanLook { base, pending, aggro, teleport } => {
                if m.target.is_none() {
                    if pending.is_some() {
                        *aggro -= 1;
                        if *aggro <= 0 {
                            m.target = *pending;
                            *pending = None;
                            base.target_mob = m.target;
                        } else {
                            let eye = self.player_eye_y();
                            self.look_at(m, [self.pos[0], eye, self.pos[2]], 10.0, 40.0);
                        }
                    }
                } else if m.target == Some(Ent::Player) {
                    if self.player_looking_at(m) {
                        if dist2(m.pos, self.pos) < 16.0 {
                            self.enderman_teleport_random(m);
                        }
                        *teleport = 0;
                    } else if dist2(m.pos, self.pos) > 256.0 {
                        *teleport += 1;
                        if *teleport >= reduced(30) {
                            self.enderman_teleport_towards(m, self.pos);
                            *teleport = 0;
                        }
                    }
                }
            }
            _ => {}
        }
    }

    fn melee_tick(&mut self, g: &mut Goal, m: &mut Mob) {
        let Goal::Melee { speed, unseen, kind, pathed, recalc, cd, .. } = g else { return };
        let Some(t) = m.target else { return };
        let (Some(tp), Some(eye)) = (self.ent_pos(t), self.ent_eye(t)) else { return };
        self.look_at(m, [tp[0], eye, tp[2]], 30.0, 30.0);
        *recalc = (*recalc - 1).max(0);
        if (*unseen || self.line_of_sight(m, t))
            && *recalc <= 0
            && (*pathed == [0.0; 3] || dist2(tp, *pathed) >= 1.0 || m.rng.next_float() < 0.05)
        {
            *pathed = tp;
            *recalc = 4 + m.rng.next_int_bound(7);
            let d = dist2(m.pos, tp);
            if d > 1024.0 {
                *recalc += 10;
            } else if d > 256.0 {
                *recalc += 5;
            }
            let s = *speed;
            // Chasing an entity: without a new path, the old one stays followed.
            let path = self.create_path(m, super::mth::block_of(tp), 0);
            if !path.is_some_and(|p| self.move_to_path(m, Some(p), s)) {
                *recalc += 15;
            }
            // Tick every tick: delay unchanged.
        }
        *cd = (*cd - 1).max(0);
        if *kind == Melee::PolarBear {
            // Polar bear: strikes without a swing; rears up and growls when the blow approaches.
            if *cd <= 0 && self.within_melee(m, t) && self.line_of_sight(m, t) {
                *cd = 20;
                self.mob_attack(m, t);
                m.surf.standing = false;
            } else {
                let tw = self.ent_bbox(t).map_or(0.6, |b| b.max[0] - b.min[0]) as f32 + 3.0;
                if dist2(m.pos, tp) < f64::from(tw * tw) {
                    if *cd <= 0 {
                        m.surf.standing = false;
                        *cd = 20;
                    }
                    if *cd <= 10 {
                        m.surf.standing = true;
                        super::surface::polar::warning(self, m);
                    }
                } else {
                    *cd = 20;
                    m.surf.standing = false;
                }
            }
        } else if *kind == Melee::Fox {
            if *cd <= 0 && self.within_melee(m, t) && self.line_of_sight(m, t) {
                *cd = 20;
                self.mob_attack(m, t);
                self.mob_sound(m, "minecraft:entity.fox.bite", 1.0, 1.0);
            }
        } else if *cd <= 0 && self.within_melee(m, t) && self.line_of_sight(m, t) {
            *cd = 20;
            self.mob_swing(m);
            if m.kind != Kind::Creeper {
                self.mob_attack(m, t);
            }
        }
        if let Melee::Zombie { raise } = kind {
            *raise += 1;
            m.aggressive = *raise >= 5 && *cd < 10;
        }
    }

    fn bow_tick(&mut self, g: &mut Goal, m: &mut Mob) {
        let Goal::Bow { speed, min_interval, radius2, attack_time, see, cw, back, strafe } = g else { return };
        let Some(t) = m.target else { return };
        let (Some(tp), Some(eye)) = (self.ent_pos(t), self.ent_eye(t)) else { return };
        let d = dist2(m.pos, tp);
        let has = self.line_of_sight(m, t);
        let had = *see > 0;
        if has != had {
            *see = 0;
        }
        if has {
            *see += 1;
        } else {
            *see -= 1;
        }
        if d <= f64::from(*radius2) && *see >= 20 {
            m.nav.stop();
            *strafe += 1;
        } else {
            let s = *speed;
            self.move_to_entity(m, tp, s);
            *strafe = -1;
        }
        if *strafe >= 20 {
            if m.rng.next_float() < 0.3 {
                *cw = !*cw;
            }
            if m.rng.next_float() < 0.3 {
                *back = !*back;
            }
            *strafe = 0;
        }
        if *strafe > -1 {
            if d > f64::from(*radius2 * 0.75) {
                *back = false;
            } else if d < f64::from(*radius2 * 0.25) {
                *back = true;
            }
            m.move_ctl.op = super::mob::MoveOp::Strafe;
            m.move_ctl.strafe_fwd = if *back { -0.5 } else { 0.5 };
            m.move_ctl.strafe_right = if *cw { 0.5 } else { -0.5 };
            m.move_ctl.speed_mod = 0.25;
            mob_look_at_direct(m, tp, eye, 30.0, 30.0);
        } else {
            self.look_at(m, [tp[0], eye, tp[2]], 30.0, 30.0);
        }
        if m.use_ticks >= 0 {
            if !has && *see < -60 {
                m.use_ticks = -1;
            } else if has {
                let pull = m.use_ticks;
                if pull >= 20 {
                    m.use_ticks = -1;
                    self.shoot_arrow(m, t, bow_power(pull));
                    *attack_time = *min_interval;
                }
            }
        } else {
            *attack_time -= 1;
            if *attack_time <= 0 && *see >= -60 {
                m.use_ticks = 0;
            }
        }
    }

    // ---------- Targeting ----------

    pub(crate) fn target_start(base: &mut TBase) {
        base.reach_cache = 0;
        base.reach_cache_time = 0;
        base.unseen = 0;
    }

    /// The attack test of a targeting objective.
    fn target_can_attack(&self, base: &mut TBase, m: &mut Mob, t: Ent, sight: bool, invis: bool) -> bool {
        if !self.targeting_ok(m, t, true, -1.0, sight, invis) {
            return false;
        }
        if base.must_reach {
            base.reach_cache_time -= 1;
            if base.reach_cache_time <= 0 {
                base.reach_cache = 0;
            }
            if base.reach_cache == 0 {
                base.reach_cache = if self.can_reach(base, m, t) { 1 } else { 2 };
            }
            if base.reach_cache == 2 {
                return false;
            }
        }
        true
    }

    fn can_reach(&self, base: &mut TBase, m: &mut Mob, t: Ent) -> bool {
        base.reach_cache_time = reduced(10 + m.rng.next_int_bound(5));
        let Some(tp) = self.ent_pos(t) else { return false };
        let Some(path) = self.create_path(m, super::mth::block_of(tp), 0) else { return false };
        let Some(last) = path.end() else { return false };
        let xx = last[0] - tp[0].floor() as i32;
        let zz = last[2] - tp[2].floor() as i32;
        f64::from(xx * xx + zz * zz) <= 2.25
    }

    /// The chase test of a targeting objective.
    pub(crate) fn target_continue(&self, base: &mut TBase, m: &mut Mob) -> bool {
        let Some(t) = m.target.or(base.target_mob) else { return false };
        if !self.ent_alive(t) || (t == Ent::Player && !self.player_attackable()) {
            return false;
        }
        let follow = m.follow_range();
        let Some(p) = self.ent_pos(t) else { return false };
        if dist2(m.pos, p) > follow * follow {
            return false;
        }
        if base.must_see {
            if self.line_of_sight(m, t) {
                base.unseen = 0;
            } else {
                base.unseen += 1;
                if base.unseen > reduced(base.unseen_memory) {
                    return false;
                }
            }
        }
        m.target = Some(t);
        true
    }

    /// Call for help in retaliation (same species, box ±follow/±10 around a unit cube).
    fn alert_others(&mut self, m: &Mob) {
        let follow = m.follow_range();
        let p = m.pos;
        let area = rc_physics::Aabb {
            min: [p[0] - follow, p[1] - 10.0, p[2] - follow],
            max: [p[0] + 1.0 + follow, p[1] + 1.0 + 10.0, p[2] + 1.0 + follow],
        };
        let Some((by, _)) = m.last_hurt_by_mob else { return };
        // F11: the bee only alerts if it sees the aggressor (computed once).
        let los = match (self.ent_pos(by), self.ent_eye(by)) {
            (Some(p), Some(eye)) if m.kind == Kind::Bee => {
                let from = [m.pos[0], m.eye_y(), m.pos[2]];
                let to = [p[0], eye, p[2]];
                dist2(from, to) <= 128.0 * 128.0 && !self.clip_blocks(from, to)
            }
            _ => false,
        };
        let mut wild_ok = Vec::new();
        for o in self.mobs.list.iter().flatten() {
            if let Some(ok) = self.wild_alert_ok(m, o, los) {
                wild_ok.push((o.id, ok));
            }
        }
        for o in self.mobs.list.iter_mut().flatten() {
            if wild_ok.iter().any(|(id, ok)| *id == o.id && !ok) {
                continue;
            }
            // Search by class: a zombie also alerts its subspecies (husk, zombie villager).
            // F13: the zombie does not alert zombified piglins.
            if (o.kind == m.kind || (m.kind == Kind::Zombie && o.kind.zombie_like() && o.kind != Kind::ZombifiedPiglin))
                && o.id != m.id
                && o.target.is_none()
                && o.touches(&area)
                && (!m.tame || o.owner == m.owner)
            {
                super::piglins::zombified::before_set_target(o, Some(by)); // F13
                o.target = Some(by);
            }
        }
    }

    // ---------- Tools ----------

    pub(crate) fn look_at(&self, m: &mut Mob, p: [f64; 3], y_max: f32, x_max: f32) {
        m.look.wanted = p;
        m.look.y_max = y_max;
        m.look.x_max = x_max;
        m.look.cooldown = 2;
    }

    /// Ranges of the held item (`attack_range` component, factor
    /// for mobs) or √2.04 − 0.6; attack box widened to the mount; touched box of
    /// the target raised to the seat point if it is mounted.
    pub(crate) fn within_melee(&self, m: &Mob, t: Ent) -> bool {
        let Some(mut tb) = self.ent_bbox(t) else { return false };
        if let Some(seat_y) = self.riding_position_y(t) {
            tb.min[1] = tb.min[1].max(seat_y);
        }
        let range = m.equipment[0].as_ref().and_then(|s| self.comp_json(s, "minecraft:attack_range"));
        let (max_r, min_r) = match range {
            None => (f64::from(2.04f32).sqrt() - f64::from(0.6f32), 0.0),
            Some(r) => {
                let f = |k: &str, d: f32| r.get(k).and_then(serde_json::Value::as_f64).map_or(d, |v| v as f32);
                let factor = f("mob_factor", 1.0);
                (f64::from(f("max_reach", 3.0) * factor), f64::from(f("min_reach", 0.0) * factor))
            }
        };
        let hit = |r: f64| self.attack_box(m, r).intersects(&tb);
        hit(max_r) && (min_r <= 0.0 || !hit(min_r))
    }

    /// Own box extended horizontally to that of the mount,
    /// then widened by `r` (ravager: 0.05 less horizontally).
    fn attack_box(&self, m: &Mob, r: f64) -> rc_physics::Aabb {
        let mut b = m.bbox();
        if let Some(v) = m.mount.vehicle.and_then(|id| self.mob_ref(id)) {
            let vb = v.bbox();
            b.min[0] = b.min[0].min(vb.min[0]);
            b.min[2] = b.min[2].min(vb.min[2]);
            b.max[0] = b.max[0].max(vb.max[0]);
            b.max[2] = b.max[2].max(vb.max[2]);
        }
        let a = rc_physics::Aabb {
            min: [b.min[0] - r, b.min[1], b.min[2] - r],
            max: [b.max[0] + r, b.max[1], b.max[2] + r],
        };
        if m.kind == Kind::Ravager {
            rc_physics::Aabb {
                min: [a.min[0] + 0.05, a.min[1], a.min[2] + 0.05],
                max: [a.max[0] - 0.05, a.max[1], a.max[2] - 0.05],
            }
        } else {
            a
        }
    }

    /// Height of the seat point of a mounted entity.
    fn riding_position_y(&self, t: Ent) -> Option<f64> {
        let (v, rank, animal) = self.mobs.list.iter().flatten().find_map(|v| {
            let rank = v.mount.passengers.iter().position(|p| *p == t)?;
            let animal = matches!(t, Ent::Mob(id) if self.mob_ref(id).is_some_and(|o| o.kind.is_animal()));
            Some((v, rank, animal))
        })?;
        Some(v.pos[1] + self.passenger_point(v, rank, animal)[1])
    }

    fn holds_bow(&self, m: &Mob) -> bool {
        m.equipment[0].as_ref().is_some_and(|s| s.id == "minecraft:bow")
            || m.equipment[1].as_ref().is_some_and(|s| s.id == "minecraft:bow")
    }

    pub(crate) fn bright_outside(&self) -> bool {
        self.dim.name == "minecraft:overworld" && self.mob_sky_darken() < 4
    }

    fn owner_here(&self, m: &Mob) -> bool {
        m.tame && m.owner == Some(self.uuid) && self.ent_alive(Ent::Player)
    }

    /// Wolf filter on the target designated by its owner (the player): never a creeper or a
    /// ghast; a wolf only if it is wild or has another owner; neither a tamed equid, nor a tamed
    /// animal.
    pub(crate) fn wolf_wants_to_attack(&self, t: Ent) -> bool {
        let Ent::Mob(id) = t else { return true };
        let Some(o) = self.mob_ref(id) else { return true };
        match o.kind {
            Kind::Creeper | Kind::Ghast => false,
            Kind::Wolf => !o.tame || o.owner != Some(self.uuid),
            k if super::mounts::handles(k) => !o.mount.tamed(),
            _ => !o.tame,
        }
    }

    fn is_panicking(&self, m: &Mob) -> bool {
        m.goals.running(|g| matches!(g, Goal::Panic { .. }))
    }

    fn should_panic(&self, m: &Mob, tag: &str) -> bool {
        let Some(h) = &m.last_hit else { return false };
        if self.time - h.tick as i64 > 40 {
            return false;
        }
        self.dmg_tag(&h.ty, tag)
    }

    pub(crate) fn look_for_water(&self, m: &Mob, xz: i32) -> Option<[i32; 3]> {
        let b = m.block_pos();
        if self.has_collision(b) {
            return None;
        }
        // Box traversed by increasing Manhattan distance (vertical 1).
        super::brainy::manhattan_find(b, xz, 1, |p| self.mob_fluid(p) == super::env::Fluid::Water)
    }

    fn free_partner(&self, m: &mut Mob) -> Option<i32> {
        let bb = m.bbox();
        let area = rc_physics::Aabb { min: bb.min.map(|v| v - 8.0), max: bb.max.map(|v| v + 8.0) };
        let mut best: Option<(f64, i32)> = None;
        for o in self.mobs.list.iter().flatten() {
            if o.id == m.id || !o.touches(&area) {
                continue;
            }
            // F04: partner among equids (horse and donkey, llamas).
            match super::mounts::feed::can_mate(m, o) {
                Some(false) => continue,
                Some(true) => {}
                None => {
                    if o.kind != m.kind || !o.is_in_love() || o.age != 0 {
                        continue;
                    }
                    // Wolf and cat: both tamed; the wolf refuses a sitting partner.
                    let tame_only = matches!(m.kind, Kind::Wolf | Kind::Cat);
                    if tame_only && (!m.tame || !o.tame || (m.kind == Kind::Wolf && o.sitting)) {
                        continue;
                    }
                }
            }
            if self.is_panicking(o) {
                continue;
            }
            // Non-combat conditions, range 8 (reduced by invisibility), without sight.
            if !self.targeting_ok(m, Ent::Mob(o.id), false, 8.0, false, true) {
                continue;
            }
            let d = dist2(m.pos, o.pos);
            if best.is_none_or(|b| d < b.0) {
                best = Some((d, o.id));
            }
        }
        best.map(|b| b.1)
    }

    pub(crate) fn walk_target_value(&self, m: &Mob, p: [i32; 3]) -> f32 {
        if let Some(v) = super::surface::walk_target_value(self, m, p)
            .or_else(|| super::mounts::walk_target_value(self, m, p))
            .or_else(|| super::aquatic::walk_target_value(self, m, p))
            .or_else(|| super::wild::walk_target_value(self, m, p))
            .or_else(|| super::nether::walk_target_value(self, m, p))
            .or_else(|| super::piglins::walk_target_value(self, m, p))
            .or_else(|| super::recent::walk_target_value(self, m, p))
        {
            return v;
        }
        // F07: the pillager prefers no cell (= 0); F09: aquatic fauna.
        if m.kind == Kind::Enderman || m.kind.is_cube() || m.kind == Kind::Pillager
            || super::sealife::handles(m.kind)
            || m.kind == Kind::Axolotl
        {
            return 0.0;
        }
        // Silverfish: attracted to cells placed on a block it can infest.
        if m.kind == Kind::Silverfish && super::pests::is_host(self.bname([p[0], p[1] - 1, p[2]])) {
            return 10.0;
        }
        if m.kind.is_monster() {
            return -(self.magic_light(p) - 0.5);
        }
        if m.kind.is_animal() {
            if self.bname([p[0], p[1] - 1, p[2]]) == "minecraft:grass_block" {
                return 10.0;
            }
            return self.magic_light(p) - 0.5;
        }
        0.0
    }

    fn has_malus(&self, m: &Mob, p: [i32; 3]) -> bool {
        let view = self.mob_view(m);
        super::nav::mob_malus(m, static_type_of(self, &view, p)) != 0.0
    }

    fn random_dir(m: &mut Mob, h: i32, v: i32) -> [i32; 3] {
        let x = m.rng.next_int_bound(2 * h + 1) - h;
        let y = m.rng.next_int_bound(2 * v + 1) - v;
        let z = m.rng.next_int_bound(2 * h + 1) - h;
        [x, y, z]
    }

    /// Aimed point: drawn direction added to the position, first shifted towards the center of the
    /// residence zone if there is one and the range exceeds 1 (two draws per attempt).
    pub(crate) fn toward(m: &mut Mob, xz: f64, dir: [i32; 3]) -> [i32; 3] {
        let (mut xt, mut zt) = (f64::from(dir[0]), f64::from(dir[2]));
        if let Some((c, _)) = rpos_home(m)
            && xz > 1.0
        {
            if m.pos[0] > f64::from(c[0]) {
                xt -= m.rng.next_double() * xz / 2.0;
            } else {
                xt += m.rng.next_double() * xz / 2.0;
            }
            if m.pos[2] > f64::from(c[2]) {
                zt -= m.rng.next_double() * xz / 2.0;
            } else {
                zt += m.rng.next_double() * xz / 2.0;
            }
        }
        [(xt + m.pos[0]).floor() as i32, (f64::from(dir[1]) + m.pos[1]).floor() as i32, (zt + m.pos[2]).floor() as i32]
    }

    fn outside_limits(&self, p: [i32; 3]) -> bool {
        let h = self.dim.height;
        p[1] < h.min_y || p[1] >= h.min_y + h.sections as i32 * 16
    }

    /// 10 draws, best walking value.
    pub(crate) fn default_random_pos(&self, m: &mut Mob, h: i32, v: i32) -> Option<[f64; 3]> {
        let restrict = rpos_restricted(m, f64::from(h));
        let mut best: Option<(f32, [i32; 3])> = None;
        for _ in 0..10 {
            let dir = Self::random_dir(m, h, v);
            let p = Self::toward(m, f64::from(h), dir);
            if self.outside_limits(p)
                || (restrict && !within_home(m, p))
                || !self.nav_stable(m, p)
                || self.has_malus(m, p)
            {
                continue;
            }
            let w = self.walk_target_value(m, p);
            if w > best.map_or(f32::NEG_INFINITY, |b| b.0) {
                best = Some((w, p));
            }
        }
        best.map(|b| [f64::from(b.1[0]) + 0.5, f64::from(b.1[1]), f64::from(b.1[2]) + 0.5])
    }

    /// The land position draw.
    pub(crate) fn land_random_pos(&self, m: &mut Mob, h: i32, v: i32) -> Option<[f64; 3]> {
        let restrict = rpos_restricted(m, f64::from(h));
        let mut best: Option<(f32, [i32; 3])> = None;
        for _ in 0..10 {
            let dir = Self::random_dir(m, h, v);
            let p = Self::toward(m, f64::from(h), dir);
            if self.outside_limits(p) || (restrict && !within_home(m, p)) || !self.nav_stable(m, p) {
                continue;
            }
            let Some(p) = self.up_out_of_solid(m, p) else { continue };
            let w = self.walk_target_value(m, p);
            if w > best.map_or(f32::NEG_INFINITY, |b| b.0) {
                best = Some((w, p));
            }
        }
        best.map(|b| [f64::from(b.1[0]) + 0.5, f64::from(b.1[1]), f64::from(b.1[2]) + 0.5])
    }

    fn up_out_of_solid(&self, m: &Mob, mut p: [i32; 3]) -> Option<[i32; 3]> {
        let max = self.dim.height.min_y + self.dim.height.sections as i32 * 16 - 1;
        if self.is_solid(p) {
            p[1] += 1;
            while p[1] <= max && self.is_solid(p) {
                p[1] += 1;
            }
        }
        if self.mob_fluid(p) == super::env::Fluid::Water || self.has_malus(m, p) {
            return None;
        }
        Some(p)
    }

    /// The position draw opposite a point (cone of π/2 opposite).
    pub(crate) fn default_random_pos_away(&self, m: &mut Mob, h: i32, v: i32, from: [f64; 3]) -> Option<[f64; 3]> {
        let away = [m.pos[0] - from[0], m.pos[2] - from[2]];
        let restrict = rpos_restricted(m, f64::from(h));
        let mut best: Option<(f32, [i32; 3])> = None;
        for _ in 0..10 {
            let center = super::mth::atan2(away[1], away[0]) - f64::from(std::f32::consts::FRAC_PI_2);
            let yr = center + f64::from(2.0 * m.rng.next_float() - 1.0) * f64::from(std::f32::consts::FRAC_PI_2);
            let dist = super::mth::rpos_dist(m.rng.next_double().sqrt(), 0.0, f64::from(h));
            let xt = -dist * yr.sin();
            let zt = dist * yr.cos();
            if xt.abs() > f64::from(h) || zt.abs() > f64::from(h) {
                continue;
            }
            let yt = m.rng.next_int_bound(2 * v + 1) - v;
            let dir = [xt.floor() as i32, yt, zt.floor() as i32];
            let p = Self::toward(m, f64::from(h), dir);
            if self.outside_limits(p)
                || (restrict && !within_home(m, p))
                || !self.nav_stable(m, p)
                || self.has_malus(m, p)
            {
                continue;
            }
            let w = self.walk_target_value(m, p);
            if w > best.map_or(f32::NEG_INFINITY, |b| b.0) {
                best = Some((w, p));
            }
        }
        best.map(|b| [f64::from(b.1[0]) + 0.5, f64::from(b.1[1]), f64::from(b.1[2]) + 0.5])
    }

    fn teleport_to_owner(&mut self, m: &mut Mob) {
        let o = super::mth::block_of(self.pos);
        for _ in 0..10 {
            let dx = m.rng.int_between(-3, 3);
            let dz = m.rng.int_between(-3, 3);
            if dx.abs() < 2 && dz.abs() < 2 {
                continue;
            }
            let dy = m.rng.int_between(-1, 1);
            let p = [o[0] + dx, o[1] + dy, o[2] + dz];
            let view = self.mob_view(m);
            if static_type_of(self, &view, p) != super::nav::PathType::Walkable {
                continue;
            }
            if m.kind != Kind::Parrot && self.bname([p[0], p[1] - 1, p[2]]).ends_with("_leaves") {
                continue;
            }
            let bp = m.block_pos();
            let b = m.bbox().offset(f64::from(p[0] - bp[0]), f64::from(p[1] - bp[1]), f64::from(p[2] - bp[2]));
            if !rc_physics::is_free(&self.world(), &b) {
                continue;
            }
            m.pos = [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5];
            m.nav.stop();
            m.needs_sync = true;
            return;
        }
    }

    /// Does the player stare at the enderman (§ 4.1)?
    pub(crate) fn player_looking_at(&self, m: &Mob) -> bool {
        if self.slots[5].as_ref().is_some_and(|s| s.id == "minecraft:carved_pumpkin") {
            return false;
        }
        let (yr, pr) = (self.yaw.to_radians(), self.pitch.to_radians());
        let look = [-(yr.sin() * pr.cos()) as f64, -(pr.sin()) as f64, (yr.cos() * pr.cos()) as f64];
        let eye = self.player_eye_y();
        let to = [m.pos[0] - self.pos[0], m.eye_y() - eye, m.pos[2] - self.pos[2]];
        let dist = (to[0] * to[0] + to[1] * to[1] + to[2] * to[2]).sqrt();
        if dist < 1.0e-6 {
            return false;
        }
        let n = to.map(|v| v / dist);
        let dot = look[0] * n[0] + look[1] * n[1] + look[2] * n[2];
        dot > 1.0 - 0.025 / dist && !self.clip_blocks([self.pos[0], eye, self.pos[2]], [m.pos[0], m.eye_y(), m.pos[2]])
    }
}

/// Universal end of anger: the aggressor, the target and the anger (reference specific to each
/// species included) are forgotten, then the anger timer is restarted (20 to 39 s, random draw of
/// the entity): the mob stays angry at all players.
pub(crate) fn forget_and_refresh_universal_anger(m: &mut Mob, time: i64) {
    m.last_hurt_by_mob = None;
    m.anger_target = None;
    m.golem.angry_at = None;
    m.surf.angry_at = None;
    m.wild.angry_at = None;
    m.pig.anger_uuid = None;
    m.target = None;
    m.anger_end = time + i64::from(m.rng.int_between(400, 780));
}

/// Orientation of a mob towards a direct point (yaw and pitch of the body).
pub fn mob_look_at_direct(m: &mut Mob, tp: [f64; 3], eye: f64, y_max: f32, x_max: f32) {
    let xd = tp[0] - m.pos[0];
    let zd = tp[2] - m.pos[2];
    let yd = eye - m.eye_y();
    let sd = (xd * xd + zd * zd).sqrt();
    let yrot = super::mth::yaw_to(xd, zd);
    let xrot = super::mth::pitch_to(yd, sd);
    m.pitch = super::mth::rotlerp(m.pitch, xrot, x_max);
    m.yaw = super::mth::rotlerp(m.yaw, yrot, y_max);
}

/// Bow power according to the draw time.
pub fn bow_power(t: i32) -> f32 {
    let mut p = t as f32 / 20.0;
    p = (p * p + p * 2.0) / 3.0;
    p.min(1.0)
}

/// Static type of a cell for the mob (evaluator without cache).
pub fn static_type_of(s: &Server, view: &super::nav::MobView, p: [i32; 3]) -> super::nav::PathType {
    super::nav::static_path_type(s, view, p)
}

// Goals of simple surface animals (F02).
pub(crate) mod mount_goals;
pub(crate) mod surface_goals;

/// Head pitch bound: 40 degrees, 20 for the sitting wolf, 1 for the
/// dolphin, 180 for guardians (F08), F10's own values.
pub(crate) fn max_head_x(m: &Mob) -> f32 {
    // F15: shulker 180.
    if let Some((x, _)) = super::end::head_limits(m.kind) {
        return x;
    }
    match m.kind {
        Kind::Wolf if m.sitting => 20.0,
        Kind::Dolphin => 1.0,
        Kind::Guardian | Kind::ElderGuardian => 180.0,
        k => super::brainy::max_head_x(k).unwrap_or(40.0),
    }
}

/// Head/body deviation bound: 75 degrees, 1 for the dolphin, 45 for the
/// ravager (F07), F10's own values.
pub(crate) fn max_head_y(kind: super::species::Kind) -> f32 {
    if let Some((_, y)) = super::end::head_limits(kind) {
        return y;
    }
    use super::species::Kind;
    match kind {
        Kind::Dolphin => 1.0,
        Kind::Ravager => 45.0,
        // F14: breeze.
        Kind::Breeze => 30.0,
        k => super::brainy::max_head_y(k).unwrap_or(75.0),
    }
}

/// Residence zone of a mob read by the generic position draws (center, radius):
/// tamed nautilus, elder guardian, happy ghast.
pub(crate) fn rpos_home(m: &Mob) -> Option<([i32; 3], i32)> {
    m.sea.home.or(m.aqua.home).or(m.rec.restriction)
}

/// Residence set and mob within radius + range + 1 of its center: points outside the
/// zone are then refused.
pub(crate) fn rpos_restricted(m: &Mob, h: f64) -> bool {
    rpos_home(m).is_some_and(|(c, r)| {
        let lim = f64::from(r) + h + 1.0;
        let cc = [f64::from(c[0]) + 0.5, f64::from(c[1]) + 0.5, f64::from(c[2]) + 0.5];
        dist2(cc, m.pos) < lim * lim
    })
}

/// Cell in the residence zone (squared distance to the center strictly under radius²).
pub(crate) fn within_home(m: &Mob, p: [i32; 3]) -> bool {
    rpos_home(m).is_none_or(|(c, r)| {
        let d = [i64::from(p[0] - c[0]), i64::from(p[1] - c[1]), i64::from(p[2] - c[2])];
        d[0] * d[0] + d[1] * d[1] + d[2] * d[2] < i64::from(r) * i64::from(r)
    })
}

#[cfg(test)]
mod home_tests {
    use super::super::jrand::JRand;
    use super::super::mob::Mob;
    use super::super::species::Kind;
    use crate::server::Server;

    fn nautilus(pos: [f64; 3], home: Option<([i32; 3], i32)>) -> Mob {
        let mut m = Mob::new(Kind::Nautilus, 1, 1, pos, 0.0, JRand::new(7));
        m.sea.home = home;
        m
    }

    /// With a residence zone and a range > 1, the drawn point is shifted towards the center
    /// (two draws per attempt); without a zone, it consumes nothing.
    #[test]
    fn toward_shifts_to_home_center() {
        let mut m = nautilus([10.5, 64.0, -3.5], Some(([0, 64, 0], 32)));
        let mut r = m.rng;
        let (a, b) = (r.next_double(), r.next_double());
        let p = Server::toward(&mut m, 10.0, [2, 1, 3]);
        let want = [(2.0 - a * 5.0 + 10.5).floor() as i32, 65, (3.0 + b * 5.0 - 3.5).floor() as i32];
        assert_eq!(p, want);
        assert_eq!(m.rng.next_double(), r.next_double());

        let mut free = nautilus([10.5, 64.0, -3.5], None);
        let mut r = free.rng;
        assert_eq!(Server::toward(&mut free, 10.0, [2, 1, 3]), [12, 65, -1]);
        assert_eq!(free.rng.next_double(), r.next_double());
        // Range 1: no shift even with a zone.
        let mut near = nautilus([10.5, 64.0, -3.5], Some(([0, 64, 0], 32)));
        assert_eq!(Server::toward(&mut near, 1.0, [2, 1, 3]), [12, 65, -1]);
    }

    #[test]
    fn home_restriction_and_membership() {
        let m = nautilus([40.5, 64.0, 0.5], Some(([0, 64, 0], 32)));
        // 40 < 32 + 10 + 1: restricted; 40 ≥ 32 + 5 + 1: not.
        assert!(super::rpos_restricted(&m, 10.0));
        assert!(!super::rpos_restricted(&m, 5.0));
        assert!(super::within_home(&m, [31, 64, 0]));
        assert!(!super::within_home(&m, [32, 64, 0]));
        assert!(super::within_home(&nautilus([0.0; 3], None), [1000, 0, 0]));
    }
}
