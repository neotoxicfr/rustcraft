//! Bees, foxes and pandas (family F11, `server-mobs.md` § 4.F11): info sheets, own
//! state, metadata, NBT, dispatch of the hooks called by the common engine (`mobs/`).
//! Bee: `bee.rs` (flight, pollination, anger, sting) and `hive.rs` (hives and nests: occupants
//! stored in the block entity); fox: `fox.rs`; panda: `panda.rs` (genes); goals: `goals.rs`.

pub(crate) mod bee;
pub(crate) mod fox;
pub(crate) mod goals;
pub(crate) mod hive;
pub(crate) mod panda;
pub(crate) mod rpos;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod world_tests;

use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::mob::{Ent, Mob};
use super::species::{Info, Kind, animal};
use crate::nbtio::{self, int};
use crate::server::Server;

// ---------- Info sheets ----------

static BEE: Info = {
    let mut i = animal("minecraft:bee", 0.55, 0.5, 0.3, 10.0, 0.3f32 as f64);
    i.attack = 2.0;
    i.tracking = 8;
    i.hurt = "minecraft:entity.bee.hurt";
    i.death = "minecraft:entity.bee.death";
    i.volume = 0.4;
    // The empty fall damage control: the fall never accumulates.
    i.fall_immune = true;
    i
};

static FOX: Info = {
    let mut i = animal("minecraft:fox", 0.6, 0.7, 0.4, 10.0, 0.3f32 as f64);
    i.attack = 2.0;
    i.follow = 32.0;
    i.tracking = 8;
    i.hurt = "minecraft:entity.fox.hurt";
    i.death = "minecraft:entity.fox.death";
    i
};

static PANDA: Info = {
    let mut i = animal("minecraft:panda", 1.3, 1.25, 1.25 * 0.85, 20.0, 0.15f32 as f64);
    i.attack = 6.0;
    i.hurt = "minecraft:entity.panda.hurt";
    i.death = "minecraft:entity.panda.death";
    i.step_sound = Some("minecraft:entity.panda.step");
    i
};

pub fn info(k: Kind) -> &'static Info {
    match k {
        Kind::Bee => &BEE,
        Kind::Fox => &FOX,
        _ => &PANDA,
    }
}

/// Species of the family.
pub fn handles(k: Kind) -> bool {
    matches!(k, Kind::Bee | Kind::Fox | Kind::Panda)
}

/// Dimensions of a baby: bee ½, fox 0.6 (eyes 0.34375), panda ½ (eyes 0.28125).
pub fn baby_dims(k: Kind) -> (f32, f32, f32) {
    match k {
        Kind::Bee => (0.55 * 0.5, 0.5 * 0.5, 0.3 * 0.5),
        Kind::Fox => (0.6 * 0.6, 0.7 * 0.6, 0.343_75),
        _ => (1.3 * 0.5, 1.25 * 0.5, 0.281_25),
    }
}

/// Food (breeding, tempting).
pub fn food_tag(k: Kind) -> Option<&'static str> {
    Some(match k {
        Kind::Bee => "minecraft:bee_food",
        Kind::Fox => "minecraft:fox_food",
        Kind::Panda => "minecraft:panda_food",
        _ => return None,
    })
}

// ---------- Own state ----------

/// State of the family (`wild` field of the mob).
#[derive(Debug, Clone, Default)]
pub struct Wild {
    /// Synchronized flags (bee: 2 roll, 4 has stung, 8 nectar; fox: 1 sitting,
    /// 4 crouching, 8 interested, 16 pouncing, 32 sleeping, 64 face planted, 128 defending; panda:
    /// 2 sneezing, 4 rolling, 8 sitting, 16 on its back).
    pub flags: u8,
    /// Fox: targeting goals already set (the instances are the same, the second add does nothing).
    pub fox_targets: bool,
    // Bee.
    pub hive: Option<[i32; 3]>,
    pub flower: Option<[i32; 3]>,
    pub no_nectar_ticks: i32,
    pub stay_out: i32,
    pub crops: i32,
    pub hive_cd: i32,
    pub flower_cd: i32,
    pub since_sting: i32,
    pub under_water: i32,
    /// Pollination goal in progress.
    pub pollinating: bool,
    /// Blacklisted hives (3 at most) and trips in progress (hive, known flower).
    pub blacklist: Vec<[i32; 3]>,
    /// Persistent anger target (UUID), written as is on save.
    pub angry_at: Option<u128>,
    // Fox.
    pub trusted: [Option<u128>; 2],
    pub since_eaten: i32,
    pub crouch: f32,
    // Panda.
    pub main_gene: u8,
    pub hidden_gene: u8,
    pub unhappy: i32,
    pub sneeze: i32,
    pub eat: i32,
    pub roll_counter: i32,
    pub roll_delta: [f64; 3],
    pub got_bamboo: bool,
    pub did_bite: bool,
    /// Player to look at imposed by the breeding goal:
    /// pending, player found or not.
    pub look_set: bool,
    pub look_at_player: bool,
    /// Item in hand already announced to the client.
    pub sent_main: Option<crate::player::Stack>,
}

impl Wild {
    pub fn flag(&self, f: u8) -> bool {
        self.flags & f != 0
    }

    pub fn set_flag(&mut self, f: u8, v: bool) {
        if v {
            self.flags |= f;
        } else {
            self.flags &= !f;
        }
    }
}

/// Construction: initial state specific to the species.
pub fn on_new(m: &mut Mob) {
    match m.kind {
        Kind::Fox => {
            m.attrs.safe_fall.base = 5.0;
            m.can_pickup = true;
        }
        // The adult panda picks up (the just-created baby has age 0: adult).
        Kind::Panda => m.can_pickup = !m.is_baby(),
        _ => {}
    }
}

/// Own navigation (construction): bee flight, fox's path of 32. The "float" goal,
/// built after the navigation, makes it floating for all three (the bee
/// included, despite the float setting of its flying navigation).
pub fn init_nav(m: &mut Mob) {
    if !handles(m.kind) {
        return;
    }
    m.nav.can_float = true;
    match m.kind {
        Kind::Bee => {
            m.nav.fly = true;
            m.nav.can_open_doors = false;
            m.nav.required_length = 48.0;
        }
        Kind::Fox => m.nav.required_length = 32.0,
        _ => {}
    }
}

/// Goals in the game's order (goal and target selectors).
pub fn register_goals(m: &mut Mob) {
    match m.kind {
        Kind::Bee => bee_register(m),
        Kind::Fox => fox::register(m),
        _ => panda::register(m),
    }
}

/// Bee: goals and targets; the two validations draw their delay at construction, then
/// the flower search delay (field initialized after the goals).
fn bee_register(m: &mut Mob) {
    use super::goals::{Food, Goal, Melee, TBase, Who};
    use goals::{Gate, WGoal};
    let wrap = |gate: Gate, inner: Goal| Goal::Wild(WGoal::Gate { gate, inner: Box::new(inner) });
    let hive_cd = m.rng.int_between(20, 40);
    let flower_cd = m.rng.int_between(20, 40);
    let melee = Goal::Melee {
        speed: f64::from(1.4f32),
        unseen: true,
        kind: Melee::Plain,
        path: None,
        pathed: [0.0; 3],
        recalc: 0,
        cd: 0,
        last: i64::MIN / 2,
    };
    let g = &mut m.goals;
    g.add(0, wrap(Gate::BeeAttack, melee));
    g.add(1, Goal::Wild(WGoal::EnterHive));
    g.add(2, Goal::Breed { speed: 1.0, partner: None, love_time: 0 });
    g.add(
        3,
        Goal::Tempt {
            speed: 1.25,
            food: Food::Tag("minecraft:bee_food"),
            scare: false,
            stop: 2.5,
            calm: 0,
            p: [0.0; 3],
            prot: [0.0; 2],
            on: false,
        },
    );
    g.add(3, Goal::Wild(WGoal::ValidateHive { cd: hive_cd, last: -1 }));
    g.add(3, Goal::Wild(WGoal::ValidateFlower { cd: flower_cd, last: -1 }));
    g.add(
        4,
        Goal::Wild(WGoal::Pollinate { success: 0, last_sound: 0, hover: None, ticks: 0, unreachable: Vec::new() }),
    );
    g.add(5, Goal::FollowParent { speed: 1.25, parent: None, recalc: 0 });
    g.add(5, Goal::Wild(WGoal::LocateHive));
    g.add(5, Goal::Wild(WGoal::GoToHive { travel: 0, stuck: 0, last: None }));
    g.add(6, Goal::Wild(WGoal::GoToFlower { travel: 0 }));
    g.add(7, Goal::Wild(WGoal::GrowCrop));
    g.add(8, Goal::Wild(WGoal::BeeWander));
    g.add(9, Goal::Float);
    let t = &mut m.targets;
    t.add(1, wrap(Gate::BeeHurtBy, Goal::HurtBy { base: TBase::new(true, false), alert: true, stamp: 0 }));
    t.add(
        2,
        wrap(
            Gate::BeeTarget,
            Goal::Nearest {
                base: TBase::new(true, false),
                who: Who::Player,
                interval: super::mth::reduced(10),
                target: None,
                angry_only: true,
                non_tame: false,
            },
        ),
    );
    t.add(3, Goal::ResetAnger { stamp: 0, alert: true });
    m.wild.flower_cd = m.rng.int_between(20, 60);
}

// ---------- Metadata ----------

pub fn define_data(m: &mut Mob) {
    m.define(16, Meta::Bool(false));
    m.define(17, Meta::Bool(false));
    match m.kind {
        Kind::Bee => {
            m.define(18, Meta::Byte(0));
            m.define(19, Meta::Long(-1));
        }
        Kind::Fox => {
            m.define(18, Meta::Int(0));
            m.define(19, Meta::Byte(0));
            m.define(20, Meta::Entity(None));
            m.define(21, Meta::Entity(None));
        }
        _ => {
            m.define(18, Meta::Int(0));
            m.define(19, Meta::Int(0));
            m.define(20, Meta::Int(0));
            m.define(21, Meta::Byte(0));
            m.define(22, Meta::Byte(0));
            m.define(23, Meta::Byte(0));
        }
    }
}

pub fn sync_data(m: &mut Mob) {
    m.set(5, Meta::Bool(m.surf.no_gravity));
    m.set(16, Meta::Bool(m.age < 0));
    m.set(17, Meta::Bool(m.mount.age_locked));
    let flags = m.wild.flags as i8;
    match m.kind {
        Kind::Bee => {
            m.set(18, Meta::Byte(flags));
            m.set(19, Meta::Long(m.anger_end));
        }
        Kind::Fox => {
            m.set(18, Meta::Int(m.variant));
            m.set(19, Meta::Byte(flags));
            m.set(20, Meta::Entity(m.wild.trusted[0]));
            m.set(21, Meta::Entity(m.wild.trusted[1]));
        }
        _ => {
            m.set(18, Meta::Int(m.wild.unhappy));
            m.set(19, Meta::Int(m.wild.sneeze));
            m.set(20, Meta::Int(m.wild.eat));
            m.set(21, Meta::Byte(m.wild.main_gene as i8));
            m.set(22, Meta::Byte(m.wild.hidden_gene as i8));
            m.set(23, Meta::Byte(flags));
        }
    }
}

// ---------- NBT ----------

fn pos_of(v: Option<&Nbt>) -> Option<[i32; 3]> {
    match v {
        Some(Nbt::IntArray(a)) if a.len() == 3 => Some([a[0], a[1], a[2]]),
        _ => None,
    }
}

fn pos_nbt(p: [i32; 3]) -> Nbt {
    Nbt::IntArray(vec![p[0], p[1], p[2]])
}

pub fn load(m: &mut Mob, e: &Nbt) {
    let flag = |k: &str| int(e, k).is_some_and(|v| v != 0);
    m.surf.no_gravity = flag("NoGravity");
    m.mount.age_locked = flag("AgeLocked");
    match m.kind {
        Kind::Bee => {
            let w = &mut m.wild;
            w.set_flag(bee::NECTAR, flag("HasNectar"));
            w.set_flag(bee::STUNG, flag("HasStung"));
            w.no_nectar_ticks = int(e, "TicksSincePollination").unwrap_or(0) as i32;
            w.stay_out = int(e, "CannotEnterHiveTicks").unwrap_or(0) as i32;
            w.crops = int(e, "CropsGrownSincePollination").unwrap_or(0) as i32;
            w.hive = pos_of(e.get("hive_pos"));
            w.flower = pos_of(e.get("flower_pos"));
            read_anger(m, e);
        }
        Kind::Fox => {
            m.wild.trusted = [None, None];
            if let Some(Nbt::List(l)) = e.get("Trusted") {
                for u in l.iter().filter_map(|t| nbtio::uuid(Some(t))) {
                    fox::add_trusted(m, u);
                }
            }
            m.wild.set_flag(fox::SLEEPING, flag("Sleeping"));
            m.variant = i32::from(e.get("Type").and_then(Nbt::as_str) == Some("snow"));
            m.wild.set_flag(fox::SITTING, flag("Sitting"));
            m.wild.set_flag(fox::CROUCHING, flag("Crouching"));
            // Targeting goals set on read-back (server), according to the variant.
            fox::set_target_goals(m);
        }
        _ => {
            let gene = |k: &str| {
                e.get(k).and_then(Nbt::as_str).and_then(|s| panda::GENES.iter().position(|g| *g == s)).unwrap_or(0)
                    as u8
            };
            m.wild.main_gene = gene("MainGene");
            m.wild.hidden_gene = gene("HiddenGene");
        }
    }
}

/// Anger end (or old `AngerTime`), target read back.
fn read_anger(m: &mut Mob, e: &Nbt) {
    m.anger_end = match e.get("anger_end_time").and_then(super::mob::f64_of) {
        Some(t) => t as i64,
        None => -1,
    };
    m.wild.angry_at = nbtio::uuid(e.get("angry_at"));
    // The target is only found again at the first tick (`bee::relink_anger`): the UUID is enough here.
    m.anger_target = None;
}

pub fn save(m: &Mob, e: &mut Nbt) {
    let set = |e: &mut Nbt, k: &str, v: Nbt| nbtio::set(e, k, v);
    if m.surf.no_gravity {
        set(e, "NoGravity", Nbt::Byte(1));
    } else {
        nbtio::remove(e, "NoGravity");
    }
    set(e, "AgeLocked", Nbt::Byte(i8::from(m.mount.age_locked)));
    match m.kind {
        Kind::Bee => {
            let w = &m.wild;
            match w.hive {
                Some(p) => set(e, "hive_pos", pos_nbt(p)),
                None => nbtio::remove(e, "hive_pos"),
            }
            match w.flower {
                Some(p) => set(e, "flower_pos", pos_nbt(p)),
                None => nbtio::remove(e, "flower_pos"),
            }
            set(e, "HasNectar", Nbt::Byte(i8::from(w.flag(bee::NECTAR))));
            set(e, "HasStung", Nbt::Byte(i8::from(w.flag(bee::STUNG))));
            set(e, "TicksSincePollination", Nbt::Int(w.no_nectar_ticks));
            set(e, "CannotEnterHiveTicks", Nbt::Int(w.stay_out));
            set(e, "CropsGrownSincePollination", Nbt::Int(w.crops));
            set(e, "anger_end_time", Nbt::Long(m.anger_end));
            match w.angry_at {
                Some(u) => set(e, "angry_at", nbtio::uuid_nbt(u)),
                None => nbtio::remove(e, "angry_at"),
            }
        }
        Kind::Fox => {
            let l = m.wild.trusted.iter().flatten().map(|u| nbtio::uuid_nbt(*u)).collect();
            set(e, "Trusted", Nbt::List(l));
            set(e, "Sleeping", Nbt::Byte(i8::from(m.wild.flag(fox::SLEEPING))));
            set(e, "Type", Nbt::String(if m.variant == 1 { "snow" } else { "red" }.into()));
            set(e, "Sitting", Nbt::Byte(i8::from(m.wild.flag(fox::SITTING))));
            set(e, "Crouching", Nbt::Byte(i8::from(m.wild.flag(fox::CROUCHING))));
        }
        _ => {
            let g = |v: u8| Nbt::String(panda::GENES[usize::from(v.min(6))].into());
            set(e, "MainGene", g(m.wild.main_gene));
            set(e, "HiddenGene", g(m.wild.hidden_gene));
        }
    }
}

// ---------- Engine hooks ----------

/// Ambient sound (bee: none; fox: sleep, nocturnal cry, ordinary; panda: by gene).
pub fn ambient_sound(s: &mut Server, m: &mut Mob) -> Option<String> {
    match m.kind {
        Kind::Bee => None,
        Kind::Fox => Some(fox::ambient_sound(s, m).to_owned()),
        _ => Some(panda::ambient_sound(m).to_owned()),
    }
}

/// Own volume of the ambient sound (fox's: cry at 2).
pub fn ambient_volume(m: &Mob, sound: &str) -> Option<f32> {
    (m.kind == Kind::Fox && sound == "minecraft:entity.fox.screech").then_some(2.0)
}

/// The animals stay.
pub fn remove_when_far_away(m: &Mob) -> Option<bool> {
    handles(m.kind).then_some(false)
}

/// The bee prefers air (10), otherwise 0; `None`: animal rule.
pub fn walk_target_value(s: &Server, m: &Mob, p: [i32; 3]) -> Option<f32> {
    (m.kind == Kind::Bee).then(|| if s.is_air(p) { 10.0 } else { 0.0 })
}

/// The stable-destination test of the bee's flying navigation: non-empty block below.
pub fn nav_stable(s: &Server, m: &Mob, p: [i32; 3]) -> Option<bool> {
    (m.kind == Kind::Bee).then(|| !s.is_air([p[0], p[1] - 1, p[2]]))
}

/// The navigation does not tick during pollination (overridden).
pub fn nav_frozen(m: &Mob) -> bool {
    m.kind == Kind::Bee && m.wild.pollinating
}

/// Own path maluses (bee: fire, water, cocoa, fences forbidden, water edge 16;
/// fox: hurting blocks without malus); `None`: common rule.
pub fn malus(k: Kind, t: super::nav::PathType) -> Option<f32> {
    use super::nav::PathType::*;
    match (k, t) {
        (Kind::Bee, Fire | Water | Cocoa | Fence) => Some(-1.0),
        (Kind::Bee, WaterBorder) => Some(16.0),
        (Kind::Fox, Damaging | DamagingInNeighbor) => Some(0.0),
        _ => None,
    }
}

/// Jump in a liquid: the bee only rises by 0.01.
pub fn jump_in_liquid(m: &mut Mob) -> bool {
    if m.kind != Kind::Bee {
        return false;
    }
    m.vel[1] += 0.01;
    true
}

/// The spawn placement of the fox and the panda is free: only the
/// species' rule and the free box count.
pub fn no_placement_restriction(k: Kind) -> bool {
    matches!(k, Kind::Fox | Kind::Panda)
}

/// Own spawn rule; `None`: common animal rule (panda).
pub fn spawn_rules(s: &mut Server, k: Kind, p: [i32; 3]) -> Option<bool> {
    (k == Kind::Fox).then(|| {
        let (a, b) = s.light(p);
        s.btag_id([p[0], p[1] - 1, p[2]], crate::tag_bits::TagId::FoxesSpawnableOn) && a.max(b) > 8
    })
}

/// Own finalization before the spawn finalization of mobs that grow up (fox variant and equipment,
/// panda genes, group data).
pub fn finalize_spawn(s: &mut Server, m: &mut Mob, group: &mut super::spawn::Group) {
    match m.kind {
        Kind::Fox => fox::finalize_spawn(s, m, group),
        Kind::Panda => panda::finalize_spawn(s, m, group),
        _ => {}
    }
}

/// Own right click; `Some(r)`: handled.
pub fn interact(s: &mut Server, m: &mut Mob, slot: usize, item: &str) -> Option<bool> {
    match m.kind {
        Kind::Bee => bee::interact(s, m, slot, item),
        Kind::Panda => panda::interact(s, m, slot, item),
        _ => None,
    }
}

/// Birth; true if handled.
pub fn breed(s: &mut Server, m: &mut Mob, partner: i32) -> bool {
    if !handles(m.kind) {
        return false;
    }
    let Some(pi) = s.mob_index(partner) else { return true };
    let Some(p) = s.mobs.list[pi].take() else { return true };
    let mut baby = s.new_mob(m.kind, m.pos, 0.0);
    match m.kind {
        Kind::Fox => baby.variant = if m.rng.next_bool() { m.variant } else { p.variant },
        Kind::Panda => {
            panda::genes_from_parents(&mut baby, m, Some(&p));
            panda::set_attributes(&mut baby);
        }
        _ => {}
    }
    if m.kind == Kind::Fox {
        // Fox mating: trust for the players who fed the parents.
        if m.love_cause || p.love_cause {
            fox::add_trusted(&mut baby, s.uuid);
        }
    }
    baby.age = -24000;
    baby.yaw = 0.0;
    baby.pitch = 0.0;
    s.mobs.list[pi] = Some(p);
    // Breeding statistic and trigger (before love is forgotten, which erases its cause).
    s.s9_bred(m, partner, Some(&baby));
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

/// `/summon` without NBT: the game first reads the `{id}` compound (the fox sets its targeting goals
/// with the default variant) then finalizes (second add without effect).
pub fn summon_loaded(m: &mut Mob) {
    if m.kind == Kind::Fox {
        fox::set_target_goals(m);
    }
}

impl Server {
    /// Own base tick, after the common tick.
    pub(crate) fn wild_post_tick(&mut self, m: &mut Mob) {
        match m.kind {
            Kind::Bee => bee::post_tick(self, m),
            Kind::Fox => fox::post_tick(self, m),
            Kind::Panda => panda::post_tick(self, m),
            _ => {}
        }
        // Item in mouth (fox) or paw (panda) sent back when it changes.
        if m.equipment[0] != m.wild.sent_main {
            if m.track.seen {
                self.send_mob_equipment(m, &[0]);
            }
            m.wild.sent_main = m.equipment[0].clone();
        }
    }

    /// Start of the AI step (fox: food in mouth, lost target, motionless sleep).
    pub(crate) fn wild_pre_ai_step(&mut self, m: &mut Mob) {
        if m.kind == Kind::Fox {
            fox::pre_ai_step(self, m);
        }
    }

    /// End of the AI step (bee: delays, roll, hive; fox: defense cry; pickup).
    pub(crate) fn wild_post_ai_step(&mut self, m: &mut Mob) {
        match m.kind {
            Kind::Bee => bee::post_ai_step(self, m),
            Kind::Fox => fox::post_ai_step(self, m),
            Kind::Panda => panda::pickup(self, m),
            _ => {}
        }
    }

    /// The species' own server AI step (bee: drowning, death after sting, anger).
    pub(crate) fn wild_custom_ai_step(&mut self, m: &mut Mob) {
        if m.kind == Kind::Bee {
            bee::custom_ai_step(self, m);
        }
    }

    /// Own movement control; true if it replaces the common control (bee: flight;
    /// motionless fox, busy panda: no tick).
    pub(crate) fn wild_move_control(&mut self, m: &mut Mob) -> bool {
        match m.kind {
            Kind::Bee => {
                bee::move_control(m);
                true
            }
            Kind::Fox => !fox::can_move(m),
            Kind::Panda => !panda::can_perform_action(self, m),
            _ => false,
        }
    }

    /// Own gaze control; true if it replaces the common control.
    pub(crate) fn wild_look_control(&mut self, m: &mut Mob) -> bool {
        match m.kind {
            Kind::Bee => bee::look_control(self, m),
            Kind::Fox => fox::look_control(m),
            _ => false,
        }
    }

    /// Before damage (pollination interrupted, panda standing).
    pub(crate) fn wild_before_hurt(&mut self, m: &mut Mob) {
        match m.kind {
            Kind::Bee => m.wild.pollinating = false,
            Kind::Panda => m.wild.set_flag(panda::SIT, false),
            _ => {}
        }
    }

    /// Hit by the mob; `Some(r)`: handled (bee sting).
    pub(crate) fn wild_attack(&mut self, m: &mut Mob, t: Ent) -> Option<bool> {
        match m.kind {
            Kind::Bee => Some(bee::sting(self, m, t)),
            Kind::Panda => {
                if !panda::is_aggressive(m) {
                    m.wild.did_bite = true;
                }
                None
            }
            _ => None,
        }
    }

    /// After a hit dealt by the mob (panda's).
    pub(crate) fn wild_after_attack(&mut self, m: &mut Mob, hurt: bool) {
        if hurt && m.kind == Kind::Panda {
            self.mob_sound(m, "minecraft:entity.panda.bite", 1.0, 1.0);
        }
    }

    /// Own death loot (fox's: item in mouth dropped first).
    pub(crate) fn wild_death_loot(&mut self, m: &mut Mob) {
        if m.kind == Kind::Fox
            && let Some(st) = m.equipment[0].take()
        {
            self.spawn_at_location(m, st, 0.0);
        }
    }

    /// Call for help of the retaliation: bee (sight on the aggressor), panda (only
    /// aggressive pandas); `None`: common rule.
    pub(crate) fn wild_alert_ok(&self, m: &Mob, o: &Mob, los: bool) -> Option<bool> {
        match m.kind {
            Kind::Bee => Some(o.kind == Kind::Bee && los),
            Kind::Panda => Some(o.kind == Kind::Panda && panda::is_aggressive(o)),
            _ => None,
        }
    }
}
