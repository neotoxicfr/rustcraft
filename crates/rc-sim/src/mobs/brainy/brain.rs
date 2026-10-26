//! Generic brain of family F10 (`server-mobs.md` § 3.3): memories with a lifetime, periodic
//! sensors, activities with conditions and behaviors ordered by priority. One tick: memories
//! aged, sensors (order of the species list), start of the stopped behaviors of the active
//! activities (priority, activity, insertion order), then tick or stop of all those running.
//! Durations are drawn from the world random at start.

use super::super::jrand::JRand;
use super::super::mob::{Ent, Mob};
use super::super::mth::{block_of, dist2};
use super::super::nav::Path;
use super::super::villager::brain::GPos;
use crate::server::Server;

/// "Never" duration of a memory.
pub const NEVER: i64 = i64::MAX;

/// Memory slot: value and remaining duration.
#[derive(Debug, Clone)]
pub struct Slot<T> {
    pub v: Option<T>,
    pub ttl: i64,
}

impl<T> Default for Slot<T> {
    fn default() -> Self {
        Slot { v: None, ttl: NEVER }
    }
}

impl<T> Slot<T> {
    /// Aging: erased once the duration reaches 0, otherwise counted down.
    pub fn tick(&mut self) {
        if self.v.is_some() && self.ttl != NEVER {
            if self.ttl <= 0 {
                self.erase();
            } else {
                self.ttl -= 1;
            }
        }
    }

    pub fn set(&mut self, v: T) {
        self.v = Some(v);
        self.ttl = NEVER;
    }

    pub fn set_exp(&mut self, v: T, ttl: i64) {
        self.v = Some(v);
        self.ttl = ttl;
    }

    pub fn set_opt(&mut self, v: Option<T>) {
        match v {
            Some(v) => self.set(v),
            None => self.erase(),
        }
    }

    pub fn erase(&mut self) {
        self.v = None;
        self.ttl = NEVER;
    }

    pub fn has(&self) -> bool {
        self.v.is_some()
    }
}

impl<T: Copy> Slot<T> {
    pub fn get(&self) -> Option<T> {
        self.v
    }
}

/// Followed target (following an entity / following a block position).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Track {
    /// Entity: gaze at eye height, targeted block at eye height.
    Ent { e: Ent, eye: bool, target_eye: bool },
    /// Block and followed point (block center, or exact point).
    Pos { b: [i32; 3], c: [f64; 3] },
    /// Ground item (id), at the height of its eyes or not.
    Item { id: i32, eye: bool },
}

impl Track {
    pub fn ent(e: Ent, eye: bool) -> Track {
        Track::Ent { e, eye, target_eye: false }
    }

    pub fn block(b: [i32; 3]) -> Track {
        Track::Pos { b, c: [f64::from(b[0]) + 0.5, f64::from(b[1]) + 0.5, f64::from(b[2]) + 0.5] }
    }

    pub fn vec(v: [f64; 3]) -> Track {
        Track::Pos { b: block_of(v), c: v }
    }
}

/// Walk target (`WalkTarget`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Walk {
    pub target: Track,
    pub speed: f32,
    pub close: i32,
}

impl Walk {
    /// Target given by a point: the block containing it (center followed).
    pub fn to(v: [f64; 3], speed: f32, close: i32) -> Walk {
        Walk { target: Track::block(block_of(v)), speed, close }
    }
}

/// Nearby living entities seen: sorted list and cache of visibility tests (evaluated on
/// demand).
#[derive(Debug, Clone, Default)]
pub struct Visible {
    pub list: Vec<Ent>,
    pub cache: Vec<Option<bool>>,
}

/// Memories of the family (all registered).
#[derive(Debug, Clone, Default)]
pub struct Mem {
    pub walk: Slot<Walk>,
    pub look: Slot<Track>,
    pub path: Slot<()>,
    pub cant_reach: Slot<i64>,
    pub attack: Slot<Ent>,
    pub attack_cooling: Slot<()>,
    pub living: Slot<Vec<Ent>>,
    pub visible: Slot<Visible>,
    pub players: Slot<Vec<Ent>>,
    pub visible_player: Slot<Ent>,
    pub attackable_player: Slot<Ent>,
    pub adult: Slot<i32>,
    pub hurt_by: Slot<String>,
    pub hurt_by_entity: Slot<Ent>,
    pub tempting: Slot<Ent>,
    pub tempt_cd: Slot<i32>,
    pub breed: Slot<i32>,
    pub panicking: Slot<bool>,
    pub lj_cd: Slot<i32>,
    pub lj_mid: Slot<bool>,
    pub ram_cd: Slot<i32>,
    pub ram_target: Slot<[f64; 3]>,
    /// Closest wanted item (id of the ground item).
    pub wanted_item: Slot<i32>,
    pub in_water: Slot<()>,
    pub pregnant: Slot<()>,
    pub attackable: Slot<Ent>,
    pub unreachable: Slot<Vec<u128>>,
    pub hunting_cd: Slot<bool>,
    pub play_dead: Slot<i32>,
    pub gaze_cd: Slot<i32>,
    pub danger: Slot<bool>,
    pub liked_player: Slot<u128>,
    pub liked_noteblock: Slot<GPos>,
    pub liked_noteblock_cd: Slot<i32>,
    pub pickup_cd: Slot<i32>,
    pub explored: Slot<Vec<GPos>>,
    pub sniff_target: Slot<[i32; 3]>,
    pub digging: Slot<bool>,
    pub happy: Slot<bool>,
    pub sniff_cd: Slot<()>,
    /// F13: memories of piglins and hoglins.
    pub pig: super::super::piglins::brain::PMem,
    /// F14: own memories (warden, breeze).
    pub x: super::super::recent::xbrain::XMem,
}

/// Memory keys (entry and activity conditions).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum K {
    Walk,
    Look,
    Path,
    CantReach,
    Attack,
    AttackCooling,
    Visible,
    Adult,
    HurtBy,
    HurtByEntity,
    Tempting,
    TemptCd,
    Breed,
    Panicking,
    LjCd,
    LjMid,
    RamCd,
    RamTarget,
    WantedItem,
    InWater,
    Pregnant,
    PlayDead,
    GazeCd,
    Danger,
    LikedNoteblockCd,
    PickupCd,
    SniffTarget,
    Digging,
    Happy,
    SniffCd,
    /// F13: own memories.
    P(super::super::piglins::brain::PK),
    /// F14: closest visible player, closest attackable entity, own memories.
    VisiblePlayer,
    Attackable,
    X(super::super::recent::xbrain::XK),
}

/// Expected state of a memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum St {
    Present,
    Absent,
    Registered,
}

impl Mem {
    pub fn tick(&mut self) {
        self.walk.tick();
        self.look.tick();
        self.path.tick();
        self.cant_reach.tick();
        self.attack.tick();
        self.attack_cooling.tick();
        self.living.tick();
        self.visible.tick();
        self.players.tick();
        self.visible_player.tick();
        self.attackable_player.tick();
        self.adult.tick();
        self.hurt_by.tick();
        self.hurt_by_entity.tick();
        self.tempting.tick();
        self.tempt_cd.tick();
        self.breed.tick();
        self.panicking.tick();
        self.lj_cd.tick();
        self.lj_mid.tick();
        self.ram_cd.tick();
        self.ram_target.tick();
        self.wanted_item.tick();
        self.in_water.tick();
        self.pregnant.tick();
        self.attackable.tick();
        self.unreachable.tick();
        self.hunting_cd.tick();
        self.play_dead.tick();
        self.gaze_cd.tick();
        self.danger.tick();
        self.liked_player.tick();
        self.liked_noteblock.tick();
        self.liked_noteblock_cd.tick();
        self.pickup_cd.tick();
        self.explored.tick();
        self.sniff_target.tick();
        self.digging.tick();
        self.happy.tick();
        self.sniff_cd.tick();
        self.pig.tick();
        self.x.tick();
    }

    pub fn has(&self, k: K) -> bool {
        match k {
            K::Walk => self.walk.has(),
            K::Look => self.look.has(),
            K::Path => self.path.has(),
            K::CantReach => self.cant_reach.has(),
            K::Attack => self.attack.has(),
            K::AttackCooling => self.attack_cooling.has(),
            K::Visible => self.visible.has(),
            K::Adult => self.adult.has(),
            K::HurtBy => self.hurt_by.has(),
            K::HurtByEntity => self.hurt_by_entity.has(),
            K::Tempting => self.tempting.has(),
            K::TemptCd => self.tempt_cd.has(),
            K::Breed => self.breed.has(),
            K::Panicking => self.panicking.has(),
            K::LjCd => self.lj_cd.has(),
            K::LjMid => self.lj_mid.has(),
            K::RamCd => self.ram_cd.has(),
            K::RamTarget => self.ram_target.has(),
            K::WantedItem => self.wanted_item.has(),
            K::InWater => self.in_water.has(),
            K::Pregnant => self.pregnant.has(),
            K::PlayDead => self.play_dead.has(),
            K::GazeCd => self.gaze_cd.has(),
            K::Danger => self.danger.has(),
            K::LikedNoteblockCd => self.liked_noteblock_cd.has(),
            K::PickupCd => self.pickup_cd.has(),
            K::SniffTarget => self.sniff_target.has(),
            K::Digging => self.digging.has(),
            K::Happy => self.happy.has(),
            K::SniffCd => self.sniff_cd.has(),
            K::P(k) => self.pig.has(k),
            K::VisiblePlayer => self.visible_player.has(),
            K::Attackable => self.attackable.has(),
            K::X(x) => self.x.has(x),
        }
    }

    pub fn erase(&mut self, k: K) {
        match k {
            K::Walk => self.walk.erase(),
            K::Look => self.look.erase(),
            K::Path => self.path.erase(),
            K::CantReach => self.cant_reach.erase(),
            K::Attack => self.attack.erase(),
            K::AttackCooling => self.attack_cooling.erase(),
            K::Visible => self.visible.erase(),
            K::Adult => self.adult.erase(),
            K::HurtBy => self.hurt_by.erase(),
            K::HurtByEntity => self.hurt_by_entity.erase(),
            K::Tempting => self.tempting.erase(),
            K::TemptCd => self.tempt_cd.erase(),
            K::Breed => self.breed.erase(),
            K::Panicking => self.panicking.erase(),
            K::LjCd => self.lj_cd.erase(),
            K::LjMid => self.lj_mid.erase(),
            K::RamCd => self.ram_cd.erase(),
            K::RamTarget => self.ram_target.erase(),
            K::WantedItem => self.wanted_item.erase(),
            K::InWater => self.in_water.erase(),
            K::Pregnant => self.pregnant.erase(),
            K::PlayDead => self.play_dead.erase(),
            K::GazeCd => self.gaze_cd.erase(),
            K::Danger => self.danger.erase(),
            K::LikedNoteblockCd => self.liked_noteblock_cd.erase(),
            K::PickupCd => self.pickup_cd.erase(),
            K::SniffTarget => self.sniff_target.erase(),
            K::Digging => self.digging.erase(),
            K::Happy => self.happy.erase(),
            K::SniffCd => self.sniff_cd.erase(),
            K::P(k) => self.pig.erase(k),
            K::VisiblePlayer => self.visible_player.erase(),
            K::Attackable => self.attackable.erase(),
            K::X(x) => self.x.erase(x),
        }
    }

    /// Countdown: integer value of the memory.
    fn count_slot(&mut self, k: K) -> Option<&mut Slot<i32>> {
        Some(match k {
            K::TemptCd => &mut self.tempt_cd,
            K::LjCd => &mut self.lj_cd,
            K::RamCd => &mut self.ram_cd,
            K::GazeCd => &mut self.gaze_cd,
            K::LikedNoteblockCd => &mut self.liked_noteblock_cd,
            K::PickupCd => &mut self.pickup_cd,
            K::PlayDead => &mut self.play_dead,
            _ => return None,
        })
    }

    pub fn check(&self, conds: &[(K, St)]) -> bool {
        conds.iter().all(|&(k, s)| match s {
            St::Present => self.has(k),
            St::Absent => !self.has(k),
            St::Registered => true,
        })
    }
}

/// Activities (order within the same priority: the game's hash table indexed by the name hash,
/// hence fixed: see `bucket`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Act {
    Core,
    Idle,
    Panic,
    Fight,
    PlayDead,
    LongJump,
    Ram,
    Tongue,
    Swim,
    LaySpawn,
    Sniff,
    Dig,
    /// F13: admiring, celebrating, fleeing, riding.
    AdmireItem,
    Celebrate,
    Avoid,
    Ride,
    /// F14: warden.
    Emerge,
    Roar,
    Investigate,
}

impl Act {
    /// Hash table bucket (16 buckets): `(h ^ h >>> 16) & 15`, `h` = Java hash of the name.
    pub fn bucket(self) -> i32 {
        match self {
            Act::Core => 1,
            Act::AdmireItem => 2,
            Act::Avoid => 11,
            Act::Ride => 13,
            Act::Dig => 3,
            Act::Idle | Act::Tongue | Act::Celebrate => 5,
            Act::Sniff => 6,
            Act::LaySpawn => 8,
            Act::Fight | Act::Swim => 14,
            Act::Panic | Act::PlayDead | Act::LongJump | Act::Ram => 15,
            Act::Emerge => 10,
            Act::Roar | Act::Investigate => 11,
        }
    }
}

/// Speed by state (axolotl: water / land).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Spd {
    Fixed(f32),
    /// In water / out of water.
    Water(f32, f32),
}

/// Stop distance of tempting (baby / adult).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Close {
    Fixed(f64),
    Age(f64, f64),
}

/// Condition of the simple triggers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cond {
    OnGround,
    InWater,
}

/// Attack target search (`StartAttacking`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Finder {
    /// Frog and axolotl: `nearest_attackable`, except while breeding.
    Attackable,
}

/// Long jump (and the variant preferring blocks).
#[derive(Debug, Clone)]
pub struct LongJump {
    pub lo: i32,
    pub hi: i32,
    pub height: i32,
    pub width: i32,
    pub mult: f32,
    /// Frog: preferred blocks (probability 0.5) and own landing.
    pub frog: bool,
    pub candidates: Vec<([i32; 3], i32)>,
    pub not_preferred: Vec<([i32; 3], i32)>,
    pub wants_preferred: bool,
    pub initial: Option<[f64; 3]>,
    pub chosen: Option<[f64; 3]>,
    pub tries: i32,
    pub prepare_start: i64,
    pub angles: JRand,
}

/// Ram preparation.
#[derive(Debug, Clone, Default)]
pub struct PrepRam {
    pub reached_at: Option<i64>,
    /// Start, target position at the time of choice, target.
    pub cand: Option<([i32; 3], [i32; 3], Ent)>,
}

/// Frog tongue.
#[derive(Debug, Clone, Copy, Default)]
pub struct Tongue {
    pub eat_timer: i32,
    pub path_counter: i32,
    /// 0 go to the target, 1 catch, 2 swallow, 3 done.
    pub state: u8,
}

/// Behaviors.
#[derive(Debug, Clone)]
pub enum Bk {
    Swim {
        chance: f32,
    },
    /// The animal panic behavior; armadillo: environmental causes and unroll at start; sniffer:
    /// sniffing cancelled at start.
    Panic {
        speed: f32,
        armadillo: bool,
        sniffer: bool,
    },
    LookSink {
        lo: i32,
        hi: i32,
    },
    MoveSink {
        lo: i32,
        hi: i32,
        cd: i32,
        path: Option<Box<Path>>,
        last: Option<[i32; 3]>,
        speed: f32,
        armadillo: bool,
    },
    CountDown(K),
    LookSometimes {
        range: f32,
        players: bool,
        ticker: i32,
        lo: i32,
        hi: i32,
    },
    SetLook {
        range: f32,
    },
    MakeLove {
        speed: f32,
        close: i32,
        at: i64,
        sniffer: bool,
    },
    Tempt {
        speed: Spd,
        close: Close,
        sniffer: bool,
    },
    BabyFollow {
        lo: i32,
        hi: i32,
        speed: Spd,
    },
    Stroll {
        speed: f32,
        from_water: bool,
    },
    StrollSwim {
        speed: f32,
    },
    StrollFly {
        speed: f32,
    },
    WalkFromLook {
        speed: Spd,
        close: i32,
        axolotl: bool,
    },
    DoNothing {
        lo: i32,
        hi: i32,
    },
    Trigger(Cond),
    Gate(Box<Gate>),
    StartAttacking(Finder),
    StopAttacking {
        axolotl: bool,
    },
    WalkToAttack(Spd),
    Melee {
        cd: i32,
    },
    EraseIfBreeding(K),
    FindLand {
        range: i32,
        speed: f32,
        next: i64,
    },
    FindLandNearLiquid {
        range: i32,
        speed: f32,
        next: i64,
    },
    FindLiquid {
        range: i32,
        speed: f32,
        next: i64,
    },
    LaySpawn,
    RandomLook {
        lo: i32,
        hi: i32,
        yaw: f32,
        min_pitch: f32,
        range: f32,
    },
    LongJumpMid {
        lo: i32,
        hi: i32,
        sound: &'static str,
    },
    LongJump(Box<LongJump>),
    PrepareRam(Box<PrepRam>),
    Ram {
        dir: [f64; 3],
    },
    Tongue(Tongue),
    Croak {
        counter: i32,
    },
    BallUp {
        peek: i32,
        was: bool,
    },
    RollOut,
    PlayDead,
    ValidatePlayDead,
    GoToItem,
    GiveItems,
    StayClose,
    Dig,
    FinishDig,
    Happy,
    Scent,
    Search,
    Sniffing,
    /// F13: behaviors of piglins and hoglins.
    Pig(Box<super::super::piglins::brain::PB>),
    /// F14: own behaviors (`recent/xbrain.rs`).
    X(super::super::recent::xbrain::XBk),
}

/// Gate: conditions, order (shuffled or not), policy (a single one or all), weighted children
/// and the list's own random.
#[derive(Debug, Clone)]
pub struct Gate {
    pub entry: Vec<(K, St)>,
    pub shuffle: bool,
    pub run_one: bool,
    pub rng: JRand,
    pub kids: Vec<Kid>,
}

#[derive(Debug, Clone)]
pub struct Kid {
    pub node: Node,
    pub weight: i32,
}

/// Placed behavior: priority, activity, execution state.
#[derive(Debug, Clone)]
pub struct Node {
    pub prio: i32,
    pub act: Act,
    pub k: Bk,
    pub running: bool,
    pub end: i64,
}

impl Node {
    pub fn new(prio: i32, act: Act, k: Bk) -> Node {
        Node { prio, act, k, running: false, end: 0 }
    }
}

/// Kind of a behavior: instant trigger, fixed duration, gate, or "do nothing".
pub(crate) enum Ctl {
    OneShot,
    Timed(i32, i32),
    Gate,
    Nothing,
}

impl Bk {
    pub(crate) fn ctl(&self) -> Ctl {
        match self {
            Bk::X(x) => x.ctl(),
            Bk::Swim { .. } | Bk::CountDown(_) | Bk::Tempt { .. } => Ctl::Timed(60, 60),
            Bk::Panic { .. } => Ctl::Timed(100, 120),
            Bk::LookSink { lo, hi } | Bk::MoveSink { lo, hi, .. } => Ctl::Timed(*lo, *hi),
            Bk::MakeLove { .. } => Ctl::Timed(110, 110),
            Bk::RandomLook { .. } => Ctl::Timed(60, 60),
            Bk::LongJumpMid { .. } => Ctl::Timed(100, 100),
            Bk::LongJump(_) | Bk::Ram { .. } => Ctl::Timed(200, 200),
            Bk::PrepareRam(_) => Ctl::Timed(160, 160),
            Bk::Tongue(_) | Bk::Croak { .. } => Ctl::Timed(100, 100),
            Bk::BallUp { .. } => Ctl::Timed(6000, 6000),
            Bk::PlayDead => Ctl::Timed(200, 200),
            Bk::GiveItems => Ctl::Timed(20, 20),
            Bk::Dig => Ctl::Timed(160, 180),
            Bk::FinishDig => Ctl::Timed(40, 40),
            Bk::Happy => Ctl::Timed(40, 100),
            Bk::Scent | Bk::Sniffing => Ctl::Timed(40, 80),
            Bk::Search => Ctl::Timed(600, 600),
            Bk::DoNothing { .. } => Ctl::Nothing,
            Bk::Gate(_) => Ctl::Gate,
            Bk::Pig(p) => match p.ctl() {
                super::super::piglins::brain::PCtl::OneShot => Ctl::OneShot,
                super::super::piglins::brain::PCtl::Timed(lo, hi) => Ctl::Timed(lo, hi),
            },
            _ => Ctl::OneShot,
        }
    }

    /// Memory conditions (entry condition or group of the behavior builder).
    pub(crate) fn entry(&self) -> &'static [(K, St)] {
        use K::*;
        use St::*;
        match self {
            Bk::X(x) => x.entry(),
            Bk::Panic { .. } => &[(Panicking, Registered), (HurtBy, Registered)],
            Bk::LookSink { .. } => &[(Look, Present)],
            Bk::MoveSink { .. } => &[(CantReach, Registered), (Path, Absent), (Walk, Present)],
            Bk::CountDown(TemptCd) => &[(TemptCd, Present)],
            Bk::CountDown(LjCd) => &[(LjCd, Present)],
            Bk::CountDown(RamCd) => &[(RamCd, Present)],
            Bk::CountDown(GazeCd) => &[(GazeCd, Present)],
            Bk::CountDown(LikedNoteblockCd) => &[(LikedNoteblockCd, Present)],
            Bk::CountDown(PickupCd) => &[(PickupCd, Present)],
            Bk::LookSometimes { .. } | Bk::SetLook { .. } => &[(Look, Absent), (Visible, Present)],
            Bk::MakeLove { .. } => {
                &[(Visible, Present), (Breed, Absent), (Walk, Registered), (Look, Registered), (Panicking, Absent)]
            }
            Bk::Tempt { .. } => &[
                (Look, Registered),
                (Walk, Registered),
                (TemptCd, Absent),
                (Tempting, Present),
                (Breed, Absent),
                (Panicking, Absent),
            ],
            Bk::BabyFollow { .. } => &[(Adult, Present), (Look, Registered), (Walk, Absent)],
            Bk::Stroll { .. } | Bk::StrollSwim { .. } | Bk::StrollFly { .. } => &[(Walk, Absent)],
            Bk::WalkFromLook { .. } => &[(Walk, Absent), (Look, Present)],
            Bk::StartAttacking(_) => &[(Attack, Absent), (CantReach, Registered)],
            Bk::StopAttacking { .. } => &[(Attack, Present), (CantReach, Registered)],
            Bk::WalkToAttack(_) => &[(Walk, Registered), (Look, Registered), (Attack, Present), (Visible, Registered)],
            Bk::Melee { .. } => &[(Look, Registered), (Attack, Present), (AttackCooling, Absent), (Visible, Present)],
            Bk::EraseIfBreeding(Attack) => &[(Attack, Present)],
            Bk::EraseIfBreeding(PlayDead) => &[(PlayDead, Present)],
            Bk::FindLand { .. } | Bk::FindLandNearLiquid { .. } | Bk::FindLiquid { .. } => {
                &[(Attack, Absent), (Walk, Absent), (Look, Registered)]
            }
            Bk::LaySpawn => &[(Attack, Absent), (Walk, Present), (Pregnant, Present)],
            Bk::RandomLook { .. } => &[(Look, Absent), (GazeCd, Absent)],
            Bk::LongJumpMid { .. } => &[(Look, Registered), (LjMid, Present)],
            Bk::LongJump(_) => &[(Look, Registered), (LjCd, Absent), (LjMid, Absent)],
            Bk::PrepareRam(_) => &[(Look, Registered), (RamCd, Absent), (Visible, Present), (RamTarget, Absent)],
            Bk::Ram { .. } => &[(RamCd, Absent), (RamTarget, Present)],
            Bk::Tongue(_) => &[(Walk, Absent), (Look, Registered), (Attack, Present), (Panicking, Absent)],
            Bk::Croak { .. } => &[(Walk, Absent)],
            Bk::RollOut => &[(Danger, Absent)],
            Bk::PlayDead => &[(PlayDead, Present), (HurtByEntity, Present)],
            Bk::ValidatePlayDead => &[(PlayDead, Present), (HurtByEntity, Registered)],
            Bk::GoToItem => &[(Look, Registered), (Walk, Registered), (WantedItem, Present), (PickupCd, Registered)],
            Bk::GiveItems => &[(Look, Registered), (Walk, Registered), (PickupCd, Registered)],
            Bk::StayClose => &[(Look, Registered), (Walk, Registered)],
            Bk::Dig => &[(Panicking, Absent), (Walk, Absent), (Digging, Present), (SniffCd, Absent)],
            Bk::FinishDig => &[(Panicking, Absent), (Walk, Absent), (Digging, Present), (SniffCd, Present)],
            Bk::Happy => &[(Happy, Present)],
            Bk::Scent => {
                &[(Panicking, Absent), (Digging, Absent), (SniffTarget, Absent), (Happy, Absent), (Breed, Absent)]
            }
            Bk::Search => &[(Walk, Present), (Panicking, Absent), (SniffTarget, Present)],
            Bk::Sniffing => &[(Walk, Absent), (SniffTarget, Absent), (SniffCd, Absent)],
            Bk::Pig(p) => p.entry(),
            _ => &[],
        }
    }
}

/// Sensors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sn {
    Living,
    Players,
    Items,
    Adult,
    HurtBy,
    FoodTempt,
    FrogTempt,
    FrogAttackables,
    AxolotlAttackables,
    InWater,
    ArmadilloScare,
    /// F13: own sensors.
    P(super::super::piglins::brain::PSn),
    X(super::super::recent::xbrain::XSn),
}

impl Sn {
    fn period(self) -> i32 {
        match self {
            Sn::ArmadilloScare => 5,
            Sn::P(p) => p.period(),
            Sn::X(x) => x.period(),
            _ => 20,
        }
    }
}

/// Definition of an activity: conditions and memories erased on exit.
#[derive(Debug, Clone)]
pub struct ActDef {
    pub act: Act,
    pub cond: Vec<(K, St)>,
    pub erase: Vec<K>,
}

/// Complete brain of a mob.
#[derive(Debug, Clone)]
pub struct Brain {
    pub mem: Mem,
    pub sensors: Vec<(Sn, i64)>,
    pub nodes: Vec<Node>,
    pub acts: Vec<ActDef>,
    /// Non-"core" activity currently running.
    pub act: Act,
}

impl Brain {
    /// New brain: sensor delays drawn from the entity's random (list order), behaviors sorted by
    /// priority, activity bucket, activity rank, insertion order.
    pub fn new(rng: &mut JRand, sensors: &[Sn], acts: Vec<(ActDef, Vec<Node>)>) -> Brain {
        let sensors = sensors.iter().map(|&s| (s, i64::from(rng.next_int_bound(s.period())))).collect();
        let mut keyed: Vec<((i32, i32, usize, usize), Node)> = Vec::new();
        let mut defs = Vec::new();
        for (ai, (def, nodes)) in acts.into_iter().enumerate() {
            for (ni, n) in nodes.into_iter().enumerate() {
                keyed.push(((n.prio, n.act.bucket(), ai, ni), n));
            }
            defs.push(def);
        }
        keyed.sort_by_key(|k| k.0);
        Brain {
            mem: Mem::default(),
            sensors,
            nodes: keyed.into_iter().map(|k| k.1).collect(),
            acts: defs,
            act: Act::Idle,
        }
    }

    fn requirements_met(&self, a: Act) -> bool {
        self.acts.iter().find(|d| d.act == a).is_some_and(|d| self.mem.check(&d.cond))
    }

    /// Erases the memories of the activities left.
    pub fn set_active(&mut self, a: Act) {
        if self.act == a {
            return;
        }
        let old = self.act;
        if let Some(d) = self.acts.iter().find(|d| d.act == old) {
            for k in d.erase.clone() {
                self.mem.erase(k);
            }
        }
        self.act = a;
    }

    /// Choosing the first valid activity.
    pub fn first_valid(&mut self, list: &[Act]) {
        for &a in list {
            if self.requirements_met(a) {
                self.set_active(a);
                break;
            }
        }
    }

    /// Return to the default activity (idle).
    pub fn use_default(&mut self) {
        self.set_active(Act::Idle);
    }
}

/// "Core" activity with no condition.
pub fn core() -> ActDef {
    ActDef { act: Act::Core, cond: Vec::new(), erase: Vec::new() }
}

pub fn act(a: Act, cond: &[(K, St)], erase: &[K]) -> ActDef {
    ActDef { act: a, cond: cond.to_vec(), erase: erase.to_vec() }
}

/// List of increasing priorities starting at `first`.
pub fn seq(first: i32, a: Act, list: Vec<Bk>) -> Vec<Node> {
    list.into_iter().enumerate().map(|(i, k)| Node::new(first + i as i32, a, k)).collect()
}

pub fn at(prio: i32, a: Act, k: Bk) -> Node {
    Node::new(prio, a, k)
}

/// The single-choice gate (shuffled, only one) or ordered gate that tries everything.
pub fn gate(entry: &[(K, St)], shuffle: bool, run_one: bool, salt: i64, kids: Vec<(Bk, i32)>) -> Bk {
    Bk::Gate(Box::new(Gate {
        entry: entry.to_vec(),
        shuffle,
        run_one,
        rng: JRand::from_time(salt),
        kids: kids.into_iter().map(|(k, w)| Kid { node: Node::new(0, Act::Core, k), weight: w }).collect(),
    }))
}

pub fn run_one(entry: &[(K, St)], salt: i64, kids: Vec<(Bk, i32)>) -> Bk {
    gate(entry, true, true, salt, kids)
}

pub fn try_all(entry: &[(K, St)], kids: Vec<(Bk, i32)>) -> Bk {
    gate(entry, false, false, 0, kids)
}

pub fn move_sink() -> Bk {
    Bk::MoveSink { lo: 150, hi: 250, cd: 0, path: None, last: None, speed: 0.0, armadillo: false }
}

pub fn look_sometimes(range: f32, players: bool) -> Bk {
    Bk::LookSometimes { range, players, ticker: 0, lo: 30, hi: 60 }
}

fn manhattan(a: [i32; 3], b: [i32; 3]) -> i32 {
    (a[0] - b[0]).abs() + (a[1] - b[1]).abs() + (a[2] - b[2]).abs()
}

fn dist_sqr_i(a: [i32; 3], b: [i32; 3]) -> i64 {
    let d = [i64::from(a[0] - b[0]), i64::from(a[1] - b[1]), i64::from(a[2] - b[2])];
    d[0] * d[0] + d[1] * d[1] + d[2] * d[2]
}

impl Server {
    // ---------- Followed targets ----------

    pub(crate) fn bz_track_pos(&self, t: Track) -> Option<[f64; 3]> {
        match t {
            Track::Ent { e, eye, .. } => {
                let p = self.ent_pos(e)?;
                Some(if eye { [p[0], self.ent_eye(e)?, p[2]] } else { p })
            }
            Track::Pos { c, .. } => Some(c),
            Track::Item { id, eye } => {
                let p = self.life.drops.iter().find(|d| d.id == id)?.pos;
                Some(if eye { [p[0], p[1] + f64::from(0.2125f32), p[2]] } else { p })
            }
        }
    }

    pub(crate) fn bz_track_block(&self, t: Track) -> Option<[i32; 3]> {
        match t {
            Track::Ent { e, target_eye, .. } => {
                let p = self.ent_pos(e)?;
                Some(if target_eye { block_of([p[0], self.ent_eye(e)?, p[2]]) } else { block_of(p) })
            }
            Track::Pos { b, .. } => Some(b),
            Track::Item { id, .. } => self.life.drops.iter().find(|d| d.id == id).map(|d| block_of(d.pos)),
        }
    }

    /// Block and item always, living entity present in the list of seen
    /// entities.
    fn bz_track_visible(&self, m: &mut Mob, b: &mut Brain, t: Track) -> bool {
        match t {
            Track::Ent { e, .. } => self.ent_alive(e) && self.bz_visible_contains(m, b, e),
            Track::Pos { .. } | Track::Item { .. } => true,
        }
    }

    // ---------- Visibility ----------

    /// Non-combat, shared sensor range (tracking of the last brain one of whose sensors ticked),
    /// line of sight (invisibility ignored for the attack target).
    pub(crate) fn bz_brain_targetable(&self, m: &mut Mob, b: &Brain, e: Ent) -> bool {
        let range = self.mobs.pig.sensor_range;
        let invis = b.mem.attack.get() != Some(e);
        self.targeting_ok(m, e, false, range, true, invis)
    }

    /// The attackable-target test (combat, shared range).
    pub(crate) fn bz_brain_attackable(&self, m: &mut Mob, b: &Brain, e: Ent) -> bool {
        let range = self.mobs.pig.sensor_range;
        let invis = b.mem.attack.get() != Some(e);
        self.targeting_ok(m, e, true, range, true, invis)
    }

    /// The search in the list of visible mobs.
    pub(crate) fn bz_visible_contains(&self, m: &mut Mob, b: &mut Brain, e: Ent) -> bool {
        let Some(i) = b.mem.visible.v.as_ref().and_then(|v| v.list.iter().position(|x| *x == e)) else {
            return false;
        };
        self.bz_visible_test(m, b, i)
    }

    fn bz_visible_test(&self, m: &mut Mob, b: &mut Brain, i: usize) -> bool {
        let Some(v) = b.mem.visible.v.as_ref() else { return false };
        if let Some(r) = v.cache[i] {
            return r;
        }
        let e = v.list[i];
        let r = self.bz_brain_targetable(m, b, e);
        if let Some(v) = b.mem.visible.v.as_mut() {
            v.cache[i] = Some(r);
        }
        r
    }

    /// First entity in the list that satisfies the filter, then visibility.
    pub(crate) fn bz_visible_closest(
        &self,
        m: &mut Mob,
        b: &mut Brain,
        f: &dyn Fn(&Server, Ent) -> bool,
    ) -> Option<Ent> {
        let n = b.mem.visible.v.as_ref()?.list.len();
        for i in 0..n {
            let e = b.mem.visible.v.as_ref()?.list[i];
            if f(self, e) && self.bz_visible_test(m, b, i) {
                return Some(e);
            }
        }
        None
    }

    // ---------- Tick ----------

    /// The brain tick.
    pub(crate) fn bz_brain_tick(&mut self, m: &mut Mob, b: &mut Brain) {
        b.mem.tick();
        self.bz_brain_sensors(m, b);
        let t = self.time;
        let mut nodes = std::mem::take(&mut b.nodes);
        for n in nodes.iter_mut() {
            if !n.running && (n.act == Act::Core || n.act == b.act) {
                self.bz_node_try_start(m, b, n, t);
            }
        }
        let running: Vec<usize> = (0..nodes.len()).filter(|&i| nodes[i].running).collect();
        for i in running {
            self.bz_node_tick_or_stop(m, b, &mut nodes[i], t);
        }
        b.nodes = nodes;
    }

    fn bz_node_try_start(&mut self, m: &mut Mob, b: &mut Brain, n: &mut Node, t: i64) -> bool {
        match n.k.ctl() {
            Ctl::Nothing => {
                let Bk::DoNothing { lo, hi } = n.k else { return false };
                n.running = true;
                n.end = t + i64::from(lo + self.mobs.rng.next_int_bound(hi + 1 - lo));
                true
            }
            Ctl::Gate => {
                let Bk::Gate(g) = &mut n.k else { return false };
                if !b.mem.check(&g.entry) {
                    return false;
                }
                n.running = true;
                if g.shuffle {
                    // Key −U^(1/weight), stable ascending sort.
                    let mut keyed: Vec<(f64, Kid)> = std::mem::take(&mut g.kids)
                        .into_iter()
                        .map(|k| {
                            let r = g.rng.next_float();
                            (-(f64::from(r).powf(1.0 / f64::from(k.weight))), k)
                        })
                        .collect();
                    keyed.sort_by(|a, c| a.0.partial_cmp(&c.0).unwrap_or(std::cmp::Ordering::Equal));
                    g.kids = keyed.into_iter().map(|k| k.1).collect();
                }
                let run_one = g.run_one;
                let mut kids = std::mem::take(&mut g.kids);
                for k in kids.iter_mut() {
                    if !k.node.running && self.bz_node_try_start(m, b, &mut k.node, t) && run_one {
                        break;
                    }
                }
                if let Bk::Gate(g) = &mut n.k {
                    g.kids = kids;
                }
                true
            }
            Ctl::OneShot => {
                if !b.mem.check(n.k.entry()) {
                    return false;
                }
                let ok = self.bz_trigger(m, b, &mut n.k, t);
                if ok {
                    n.running = true;
                }
                ok
            }
            Ctl::Timed(lo, hi) => {
                if !b.mem.check(n.k.entry()) || !self.bz_bh_check(m, b, &mut n.k, t) {
                    return false;
                }
                n.running = true;
                n.end = t + i64::from(lo + self.mobs.rng.next_int_bound(hi + 1 - lo));
                self.bz_bh_start(m, b, &mut n.k, t, n.end);
                true
            }
        }
    }

    fn bz_node_tick_or_stop(&mut self, m: &mut Mob, b: &mut Brain, n: &mut Node, t: i64) {
        match n.k.ctl() {
            Ctl::Nothing => {
                if t > n.end {
                    n.running = false;
                }
            }
            Ctl::OneShot => n.running = false,
            Ctl::Gate => {
                let Bk::Gate(g) = &mut n.k else { return };
                let mut kids = std::mem::take(&mut g.kids);
                for k in kids.iter_mut().filter(|k| k.node.running) {
                    self.bz_node_tick_or_stop(m, b, &mut k.node, t);
                }
                let any = kids.iter().any(|k| k.node.running);
                if let Bk::Gate(g) = &mut n.k {
                    g.kids = kids;
                }
                if !any {
                    self.bz_node_stop(m, b, n, t);
                }
            }
            Ctl::Timed(..) => {
                let timed_out = match n.k {
                    Bk::CountDown(_) | Bk::Tempt { .. } => false,
                    Bk::X(ref x) if x.never_times_out() => false,
                    _ => t > n.end,
                };
                if !timed_out && self.bz_bh_continue(m, b, &mut n.k, t) {
                    self.bz_bh_tick(m, b, &mut n.k, t, n.end);
                } else {
                    self.bz_node_stop(m, b, n, t);
                }
            }
        }
    }

    /// Stopping a running behavior (gates: children stopped).
    pub(crate) fn bz_node_stop(&mut self, m: &mut Mob, b: &mut Brain, n: &mut Node, t: i64) {
        n.running = false;
        match &mut n.k {
            Bk::Gate(g) => {
                let mut kids = std::mem::take(&mut g.kids);
                for k in kids.iter_mut().filter(|k| k.node.running) {
                    self.bz_node_stop(m, b, &mut k.node, t);
                }
                if let Bk::Gate(g) = &mut n.k {
                    g.kids = kids;
                }
            }
            k => {
                if matches!(k.ctl(), Ctl::Timed(..)) {
                    let end = n.end;
                    self.bz_bh_stop(m, b, &mut n.k, t, end);
                }
            }
        }
    }

    // ---------- Sensors ----------

    fn bz_brain_sensors(&mut self, m: &mut Mob, b: &mut Brain) {
        for i in 0..b.sensors.len() {
            b.sensors[i].1 -= 1;
            if b.sensors[i].1 <= 0 {
                let s = b.sensors[i].0;
                b.sensors[i].1 = i64::from(s.period());
                self.mobs.pig.sensor_range = m.follow_range(); // F13: shared ranges
                self.bz_sense(m, b, s);
            }
        }
    }

    pub(crate) fn bz_sense(&mut self, m: &mut Mob, b: &mut Brain, s: Sn) {
        match s {
            Sn::X(x) => self.x_sense(m, b, x),
            Sn::Living => {
                let r = m.follow_range();
                let bb = m.bbox();
                let area = rc_physics::Aabb { min: bb.min.map(|v| v - r), max: bb.max.map(|v| v + r) };
                let mut list: Vec<(f64, Ent)> = self
                    .mobs
                    .list
                    .iter()
                    .flatten()
                    .filter(|o| o.id != m.id && o.alive() && o.touches(&area))
                    .map(|o| (dist2(m.pos, o.pos), Ent::Mob(o.id)))
                    .collect();
                if self.ent_alive(Ent::Player) && self.player_aabb().intersects(&area) {
                    list.push((dist2(m.pos, self.pos), Ent::Player));
                }
                list.sort_by(|a, c| a.0.partial_cmp(&c.0).unwrap_or(std::cmp::Ordering::Equal));
                let ents: Vec<Ent> = list.into_iter().map(|x| x.1).collect();
                // Empty list = memory absent; the "seen" object always exists.
                let n = ents.len();
                b.mem.living.set_opt((!ents.is_empty()).then(|| ents.clone()));
                b.mem.visible.set(Visible { list: ents, cache: vec![None; n] });
            }
            Sn::Players => {
                let r = m.follow_range();
                let close = self.game_mode != 3 && self.ent_alive(Ent::Player) && dist2(m.pos, self.pos) < r * r;
                let players: Vec<Ent> = if close { vec![Ent::Player] } else { Vec::new() };
                b.mem.players.set_opt((!players.is_empty()).then(|| players.clone()));
                let visible: Vec<Ent> = players.into_iter().filter(|&e| self.bz_brain_targetable(m, b, e)).collect();
                b.mem.visible_player.set_opt(visible.first().copied());
                let att: Vec<Ent> = visible.into_iter().filter(|&e| self.bz_brain_attackable(m, b, e)).collect();
                b.mem.attackable_player.set_opt(att.first().copied());
            }
            Sn::HurtBy => {
                match &m.last_hit {
                    Some(h) if self.time - h.tick as i64 <= 40 => {
                        b.mem.hurt_by.set(h.ty.clone());
                        if let Some(e) = h.by {
                            b.mem.hurt_by_entity.set(e);
                        }
                    }
                    _ => b.mem.hurt_by.erase(),
                }
                if b.mem.hurt_by_entity.get().is_some_and(|e| !self.ent_alive(e)) {
                    b.mem.hurt_by_entity.erase();
                }
            }
            Sn::FoodTempt | Sn::FrogTempt => {
                let tag = if s == Sn::FrogTempt { Some("minecraft:frog_food") } else { super::food_tag(m.kind) };
                let range = m.attrs.tempt_range.value();
                let held = [&self.slots[36 + self.held as usize], &self.slots[45]];
                let holds = tag
                    .is_some_and(|tag| held.iter().any(|st| st.as_ref().is_some_and(|st| self.item_tag(&st.id, tag))));
                let ok = self.game_mode != 3
                    && holds
                    && !m.mount.passengers.contains(&Ent::Player)
                    && self.targeting_ok(m, Ent::Player, false, range, false, true);
                b.mem.tempting.set_opt(ok.then_some(Ent::Player));
            }
            Sn::Adult => {
                if b.mem.visible.has() {
                    let kind = m.kind;
                    let f = |s: &Server, e: Ent| match e {
                        Ent::Mob(id) => s.mob_ref(id).is_some_and(|o| o.kind == kind && !o.is_baby()),
                        Ent::Player => false,
                    };
                    let a = self
                        .bz_visible_closest(m, b, &f)
                        .and_then(|e| if let Ent::Mob(id) = e { Some(id) } else { None });
                    b.mem.adult.set_opt(a);
                }
            }
            Sn::Items => {
                let w = super::allay::wanted_item(self, m);
                b.mem.wanted_item.set_opt(w);
            }
            Sn::InWater => {
                if m.in_water {
                    b.mem.in_water.set(());
                } else {
                    b.mem.in_water.erase();
                }
            }
            Sn::FrogAttackables => {
                let unreachable = b.mem.unreachable.v.clone().unwrap_or_default();
                let me = m.pos;
                let cands: Vec<Ent> = b.mem.visible.v.as_ref().map(|v| v.list.clone()).unwrap_or_default();
                let mut found = None;
                for e in cands {
                    let ok = match e {
                        Ent::Mob(id) => self.mob_ref(id).is_some_and(|o| {
                            super::frog::can_eat(self, o) && !unreachable.contains(&o.uuid) && dist2(o.pos, me) < 100.0
                        }),
                        Ent::Player => false,
                    };
                    if ok && self.bz_brain_attackable(m, b, e) && self.bz_visible_contains(m, b, e) {
                        found = Some(e);
                        break;
                    }
                }
                b.mem.attackable.set_opt(found);
            }
            Sn::AxolotlAttackables => {
                let hunting = b.mem.hunting_cd.has();
                let me = m.pos;
                let cands: Vec<Ent> = b.mem.visible.v.as_ref().map(|v| v.list.clone()).unwrap_or_default();
                let mut found = None;
                for e in cands {
                    let close = self.ent_pos(e).is_some_and(|p| dist2(p, me) <= 64.0);
                    let ok = close
                        && match e {
                            Ent::Mob(id) => self.mob_ref(id).is_some_and(|o| {
                                o.in_water
                                    && (self.bz_etag(o.kind, "minecraft:axolotl_always_hostiles")
                                        || (!hunting && self.bz_etag(o.kind, "minecraft:axolotl_hunt_targets")))
                            }),
                            Ent::Player => false,
                        };
                    if ok && self.bz_brain_attackable(m, b, e) && self.bz_visible_contains(m, b, e) {
                        found = Some(e);
                        break;
                    }
                }
                b.mem.attackable.set_opt(found);
            }
            Sn::P(p) => self.pig_sense(m, b, p),
            Sn::ArmadilloScare => {
                if !super::armadillo::can_stay_rolled_up(self, m, b) {
                    b.mem.danger.erase();
                } else if let Some(list) = b.mem.living.v.clone()
                    && list.iter().any(|&e| super::armadillo::scared_by(self, m, e))
                {
                    b.mem.danger.set_exp(true, 80);
                }
            }
        }
    }

    // ---------- Behaviors: triggers ----------

    fn bz_speed_of(m: &Mob, s: Spd) -> f32 {
        match s {
            Spd::Fixed(v) => v,
            Spd::Water(w, l) => {
                if m.in_water {
                    w
                } else {
                    l
                }
            }
        }
    }

    /// Instant trigger (`OneShot`): true if it acted.
    pub(crate) fn bz_trigger(&mut self, m: &mut Mob, b: &mut Brain, k: &mut Bk, t: i64) -> bool {
        match k {
            Bk::X(x) => self.x_trigger(m, b, x, t),
            Bk::LookSometimes { range, players, ticker, lo, hi } => {
                let max = f64::from(*range * *range);
                let me = m.pos;
                let pl = *players;
                let f = move |s: &Server, e: Ent| {
                    (!pl || e == Ent::Player) && s.ent_pos(e).is_some_and(|p| dist2(p, me) <= max)
                };
                let Some(target) = self.bz_visible_closest(m, b, &f) else { return false };
                // Timer countdown with expiry test (world random).
                let fire = if *ticker == 0 {
                    *ticker = *lo + self.mobs.rng.next_int_bound(*hi - *lo + 1) - 1;
                    false
                } else {
                    *ticker -= 1;
                    *ticker == 0
                };
                if !fire {
                    return false;
                }
                b.mem.look.set(Track::ent(target, true));
                true
            }
            Bk::SetLook { range } => {
                let max = f64::from(*range * *range);
                let me = m.pos;
                let passengers = m.mount.passengers.clone();
                let f = move |s: &Server, e: Ent| {
                    e == Ent::Player && s.ent_pos(e).is_some_and(|p| dist2(p, me) <= max) && !passengers.contains(&e)
                };
                let Some(target) = self.bz_visible_closest(m, b, &f) else { return false };
                b.mem.look.set(Track::ent(target, true));
                true
            }
            Bk::BabyFollow { lo, hi, speed } => {
                if !m.is_baby() {
                    return false;
                }
                let Some(a) = b.mem.adult.get() else { return false };
                let Some(p) = self.mob_ref(a).map(|o| o.pos) else { return false };
                let d = dist2(m.pos, p);
                let close = *lo - 1;
                let (lo, hi) = (f64::from(*lo), f64::from(*hi + 1));
                if d < hi * hi && d >= lo * lo {
                    let s = Self::bz_speed_of(m, *speed);
                    b.mem.look.set(Track::ent(Ent::Mob(a), true));
                    b.mem.walk.set(Walk { target: Track::ent(Ent::Mob(a), false), speed: s, close });
                    return true;
                }
                false
            }
            Bk::Stroll { speed, from_water } => {
                if !*from_water && m.in_water {
                    return false;
                }
                let p = self.land_random_pos(m, 10, 7);
                b.mem.walk.set_opt(p.map(|p| Walk::to(p, *speed, 0)));
                true
            }
            Bk::StrollSwim { speed } => {
                if !m.in_water {
                    return false;
                }
                let p = self.bz_swim_target_pos(m);
                b.mem.walk.set_opt(p.map(|p| Walk::to(p, *speed, 0)));
                true
            }
            Bk::StrollFly { speed } => {
                let p = if m.kind == super::super::species::Kind::HappyGhast {
                    let home = m.rec.restriction;
                    super::allay::fly_target_pos_with(
                        self,
                        m,
                        home,
                        super::super::recent::happy_ghast::walk_target_value,
                    )
                } else {
                    super::allay::fly_target_pos(self, m)
                };
                b.mem.walk.set_opt(p.map(|p| Walk::to(p, *speed, 0)));
                true
            }
            Bk::WalkFromLook { speed, close, axolotl } => {
                let Some(l) = b.mem.look.get() else { return false };
                if *axolotl {
                    let Some(p) = self.bz_track_block(l) else { return false };
                    let water = self.mob_fluid(p) == super::super::env::Fluid::Water;
                    if water != m.in_water {
                        return false;
                    }
                }
                let s = Self::bz_speed_of(m, *speed);
                b.mem.walk.set(Walk { target: l, speed: s, close: *close });
                true
            }
            Bk::Trigger(c) => match c {
                Cond::OnGround => m.on_ground,
                Cond::InWater => m.in_water,
            },
            Bk::StartAttacking(_) => {
                if super::is_breeding(b) {
                    return false;
                }
                let Some(target) = b.mem.attackable.get() else { return false };
                if !self.bz_mob_can_attack(m, target) {
                    return false;
                }
                b.mem.attack.set(target);
                b.mem.cant_reach.erase();
                true
            }
            Bk::StopAttacking { axolotl } => {
                let Some(target) = b.mem.attack.get() else { return true };
                let tired = b.mem.cant_reach.get().is_some_and(|c| self.time - c > 200);
                if self.bz_mob_can_attack(m, target) && !tired && self.ent_alive(target) {
                    return true;
                }
                if *axolotl {
                    super::axolotl::on_stop_attacking(self, m, target);
                }
                b.mem.attack.erase();
                true
            }
            Bk::WalkToAttack(speed) => {
                let Some(target) = b.mem.attack.get() else { return false };
                let seen = b.mem.visible.has() && self.bz_visible_contains(m, b, target);
                if seen && self.within_melee(m, target) {
                    b.mem.walk.erase();
                } else {
                    let s = Self::bz_speed_of(m, *speed);
                    b.mem.look.set(Track::ent(target, true));
                    b.mem.walk.set(Walk { target: Track::ent(target, false), speed: s, close: 0 });
                }
                true
            }
            Bk::Melee { cd } => {
                let Some(target) = b.mem.attack.get() else { return false };
                if !self.within_melee(m, target) || !self.bz_visible_contains(m, b, target) {
                    return false;
                }
                b.mem.look.set(Track::ent(target, true));
                self.mob_swing(m);
                super::melee_hit(self, m, target);
                b.mem.attack_cooling.set_exp((), i64::from(*cd));
                true
            }
            Bk::EraseIfBreeding(key) => {
                if super::is_breeding(b) {
                    b.mem.erase(*key);
                    return true;
                }
                false
            }
            Bk::FindLand { range, speed, next } => self.bz_find_land(m, b, *range, *speed, next, t),
            Bk::FindLandNearLiquid { range, speed, next } => {
                self.bz_find_land_near_liquid(m, b, *range, *speed, next, t)
            }
            Bk::FindLiquid { range, speed, next } => self.bz_find_liquid(m, b, *range, *speed, next, t),
            Bk::LaySpawn => super::frog::lay_spawn(self, m, b),
            Bk::RollOut => {
                if super::armadillo::is_scared(m) {
                    super::armadillo::roll_out(self, m);
                    return true;
                }
                false
            }
            Bk::ValidatePlayDead => {
                let ticks = b.mem.play_dead.get().unwrap_or(0);
                if ticks <= 0 {
                    b.mem.play_dead.erase();
                    b.mem.hurt_by_entity.erase();
                    b.use_default();
                } else {
                    b.mem.play_dead.set(ticks - 1);
                }
                true
            }
            Bk::GoToItem => super::allay::go_to_item(self, m, b),
            Bk::StayClose => super::allay::stay_close(self, m, b),
            Bk::Pig(p) => self.pig_trigger(m, b, p, t),
            _ => false,
        }
    }

    // ---------- Behaviors with a duration ----------

    /// The additional start conditions.
    pub(crate) fn bz_bh_check(&mut self, m: &mut Mob, b: &mut Brain, k: &mut Bk, t: i64) -> bool {
        match k {
            Bk::X(x) => self.x_check(m, b, x, t),
            Bk::Swim { .. } => self.bz_swim_ok(m),
            Bk::Panic { armadillo, .. } => {
                let tag = if *armadillo { "minecraft:panic_environmental_causes" } else { "minecraft:panic_causes" };
                b.mem.hurt_by.v.as_ref().is_some_and(|ty| self.dmg_tag(ty, tag)) || b.mem.panicking.has()
            }
            Bk::MoveSink { cd, path, last, speed, armadillo, .. } => {
                if *armadillo && super::armadillo::is_scared(m) {
                    return false;
                }
                if *cd > 0 {
                    *cd -= 1;
                    return false;
                }
                let Some(w) = b.mem.walk.get() else { return false };
                let reached = self.bz_walk_reached(m, w);
                if !reached && self.bz_try_compute_path(m, b, w, t, path, speed) {
                    *last = self.bz_track_block(w.target);
                    return true;
                }
                b.mem.walk.erase();
                if reached {
                    b.mem.cant_reach.erase();
                }
                false
            }
            Bk::MakeLove { .. } => m.is_in_love() && self.bz_love_partner(m, b).is_some(),
            Bk::LongJump(j) => super::goat::long_jump_check(self, m, b, j),
            Bk::Tongue(_) => super::frog::tongue_check(self, m, b),
            Bk::Croak { .. } => super::pose(m) == super::POSE_STANDING,
            Bk::BallUp { .. } => m.on_ground && !m.in_water && m.lava_height <= 0.0,
            Bk::PlayDead => m.in_water,
            Bk::GiveItems => super::allay::can_throw(self, m, b),
            Bk::Dig | Bk::Search => super::sniffer::can_sniff(m, b),
            Bk::Scent => !b.mem.tempting.has(),
            Bk::Sniffing => !m.is_baby() && super::sniffer::can_sniff(m, b),
            Bk::Pig(p) => self.pig_check(m, b, p),
            _ => true,
        }
    }

    /// `start`.
    pub(crate) fn bz_bh_start(&mut self, m: &mut Mob, b: &mut Brain, k: &mut Bk, t: i64, end: i64) {
        let _ = end;
        match k {
            Bk::X(x) => self.x_start(m, b, x, t, end),
            Bk::Panic { armadillo, sniffer, .. } => {
                if *armadillo {
                    super::armadillo::roll_out(self, m);
                }
                if *sniffer {
                    super::sniffer::reset_sniffing(self, m, b);
                }
                b.mem.panicking.set(true);
                b.mem.walk.erase();
                m.nav.stop();
            }
            Bk::MoveSink { path, speed, .. } => {
                b.mem.path.set(());
                let p = path.as_ref().map(|p| (**p).clone());
                self.move_to_path(m, p, f64::from(*speed));
            }
            Bk::MakeLove { speed, close, at, sniffer } => {
                if *sniffer {
                    super::sniffer::reset_sniffing(self, m, b);
                }
                if let Some(pid) = self.bz_love_partner(m, b) {
                    b.mem.breed.set(pid);
                    let me = m.id;
                    self.bz_with_brain(pid, |_, ob| ob.mem.breed.set(me));
                    self.bz_lock_gaze(m, b, pid, *speed, *close);
                    *at = t + i64::from(60 + m.rng.next_int_bound(50));
                }
            }
            Bk::Tempt { sniffer, .. } => {
                if *sniffer {
                    super::sniffer::reset_sniffing(self, m, b);
                }
            }
            Bk::RandomLook { lo, hi, yaw, min_pitch, range } => {
                let pitch = (m.rng.next_float() * *range + *min_pitch).clamp(-90.0, 90.0);
                let rot = super::super::mth::wrap(m.yaw + 2.0 * m.rng.next_float() * *yaw - *yaw);
                let dir = direction(pitch, rot);
                let eye = [m.pos[0], m.eye_y(), m.pos[2]];
                b.mem.look.set(Track::vec([eye[0] + dir[0], eye[1] + dir[1], eye[2] + dir[2]]));
                let v = *lo + m.rng.next_int_bound(*hi - *lo + 1);
                b.mem.gaze_cd.set(v);
            }
            Bk::LongJumpMid { .. } => {
                super::set_discard_friction(m, true);
                super::set_pose(m, super::POSE_LONG_JUMPING);
            }
            Bk::LongJump(j) => super::goat::long_jump_start(self, m, b, j),
            Bk::PrepareRam(p) => super::goat::prepare_ram_start(self, m, b, p),
            Bk::Ram { dir } => super::goat::ram_start(self, m, b, dir),
            Bk::Tongue(tg) => super::frog::tongue_start(self, m, b, tg),
            Bk::Croak { counter } => {
                if !(m.in_water || m.lava_height > 0.0) {
                    super::set_pose(m, super::POSE_CROAKING);
                    *counter = 0;
                }
            }
            Bk::BallUp { .. } => super::armadillo::roll_up(self, m),
            Bk::PlayDead => {
                b.mem.walk.erase();
                b.mem.look.erase();
                self.give_mob_effect(m, "minecraft:regeneration", 200, 0);
            }
            Bk::GiveItems => super::allay::give_start(self, m, b),
            Bk::Dig => super::sniffer::transition(self, m, super::sniffer::DIGGING),
            Bk::FinishDig => super::sniffer::transition(self, m, super::sniffer::RISING),
            Bk::Happy => super::sniffer::transition(self, m, super::sniffer::FEELING_HAPPY),
            Bk::Scent => super::sniffer::transition(self, m, super::sniffer::SCENTING),
            Bk::Search => super::sniffer::transition(self, m, super::sniffer::SEARCHING),
            Bk::Sniffing => super::sniffer::transition(self, m, super::sniffer::SNIFFING),
            Bk::Pig(p) => self.pig_start(m, b, p),
            _ => {}
        }
    }

    /// The continuation test.
    pub(crate) fn bz_bh_continue(&mut self, m: &mut Mob, b: &mut Brain, k: &mut Bk, t: i64) -> bool {
        match k {
            Bk::X(x) => self.x_continue(m, b, x, t),
            Bk::Swim { .. } => self.bz_swim_ok(m),
            Bk::Panic { .. } | Bk::Happy | Bk::Scent => true,
            Bk::LookSink { .. } => match b.mem.look.get() {
                Some(l) => self.bz_track_visible(m, b, l),
                None => false,
            },
            Bk::MoveSink { path, last, .. } => {
                if path.is_none() || last.is_none() {
                    return false;
                }
                let Some(w) = b.mem.walk.get() else { return false };
                let spectator = matches!(w.target, Track::Ent { e: Ent::Player, .. }) && self.game_mode == 3;
                !m.nav.is_done() && !self.bz_walk_reached(m, w) && !spectator
            }
            Bk::CountDown(key) => b.mem.count_slot(*key).and_then(|s| s.get()).is_some_and(|v| v > 0),
            Bk::MakeLove { at, .. } => {
                let Some(pid) = b.mem.breed.get() else { return false };
                let ok_kind = self.mob_ref(pid).is_some_and(|p| p.kind == m.kind);
                if !ok_kind {
                    return false;
                }
                let alive = self.mob_ref(pid).is_some_and(Mob::alive);
                let can = self.mob_ref(pid).is_some_and(|p| super::can_mate(self, m, p));
                let partner_panics = self.mob_ref(pid).is_some_and(super::is_panicking);
                alive
                    && can
                    && self.bz_visible_contains(m, b, Ent::Mob(pid))
                    && t <= *at
                    && !b.mem.panicking.has()
                    && !partner_panics
            }
            Bk::Tempt { .. } => b.mem.tempting.has() && !b.mem.breed.has() && !b.mem.panicking.has(),
            Bk::LongJumpMid { .. } => !m.on_ground,
            Bk::LongJump(j) => super::goat::long_jump_continue(self, m, b, j),
            Bk::PrepareRam(p) => p.cand.is_some_and(|c| self.ent_alive(c.2)),
            Bk::Ram { .. } => b.mem.ram_target.has(),
            Bk::Tongue(tg) => b.mem.attack.has() && tg.state != 3 && !b.mem.panicking.has(),
            Bk::Croak { counter } => *counter < 60,
            Bk::BallUp { .. } => super::armadillo::state(m).threatened(),
            Bk::PlayDead => m.in_water && b.mem.play_dead.has(),
            Bk::GiveItems => super::allay::can_throw(self, m, b),
            Bk::Dig => b.mem.digging.has() && super::sniffer::can_dig(self, m, b) && !m.is_in_love(),
            Bk::FinishDig => b.mem.digging.has(),
            Bk::Search => super::sniffer::search_continue(self, m, b),
            Bk::Sniffing => super::sniffer::can_sniff(m, b),
            Bk::Pig(p) => self.pig_continue(m, b, p),
            _ => false,
        }
    }

    /// `tick`.
    pub(crate) fn bz_bh_tick(&mut self, m: &mut Mob, b: &mut Brain, k: &mut Bk, t: i64, end: i64) {
        let _ = end;
        match k {
            Bk::X(x) => self.x_tick(m, b, x, t, end),
            Bk::Swim { chance } => {
                if m.rng.next_float() < *chance {
                    m.jump_ctl = true;
                }
            }
            Bk::Panic { speed, .. } => {
                if m.nav.is_done()
                    && let Some(p) = self.bz_panic_pos(m)
                {
                    b.mem.walk.set(Walk::to(p, *speed, 0));
                }
            }
            Bk::LookSink { .. } => {
                if let Some(p) = b.mem.look.get().and_then(|l| self.bz_track_pos(l)) {
                    let (y, x) = (super::head_rot_speed(m.kind), super::super::goals::max_head_x(m));
                    self.look_at(m, p, y, x);
                }
            }
            Bk::MoveSink { path, last, speed, .. } => {
                let nav_path = m.nav.path.clone().map(Box::new);
                let changed = match (&*path, &nav_path) {
                    (Some(a), Some(c)) => a.nodes.len() != c.nodes.len() || a.target != c.target,
                    (None, None) => false,
                    _ => true,
                };
                if changed {
                    *path = nav_path.clone();
                    if nav_path.is_some() {
                        b.mem.path.set(());
                    } else {
                        b.mem.path.erase();
                    }
                }
                if nav_path.is_some()
                    && let Some(l) = *last
                    && let Some(w) = b.mem.walk.get()
                    && let Some(tb) = self.bz_track_block(w.target)
                    && dist_sqr_i(tb, l) > 4
                    && self.bz_try_compute_path(m, b, w, t, path, speed)
                {
                    *last = Some(tb);
                    b.mem.path.set(());
                    let p = path.as_ref().map(|p| (**p).clone());
                    self.move_to_path(m, p, f64::from(*speed));
                }
            }
            Bk::CountDown(key) => {
                if let Some(s) = b.mem.count_slot(*key)
                    && let Some(v) = s.get()
                {
                    s.set(v - 1);
                }
            }
            Bk::MakeLove { speed, close, at, .. } => {
                let Some(pid) = b.mem.breed.get() else { return };
                self.bz_lock_gaze(m, b, pid, *speed, *close);
                let pp = self.mob_ref(pid).map(|p| p.pos);
                if pp.is_some_and(|pp| dist2(m.pos, pp) < 9.0) && t >= *at {
                    super::spawn_child(self, m, pid);
                    b.mem.breed.erase();
                    self.bz_with_brain(pid, |_, ob| ob.mem.breed.erase());
                }
            }
            Bk::Tempt { speed, close, .. } => {
                let Some(player) = b.mem.tempting.get() else { return };
                b.mem.look.set(Track::ent(player, true));
                let c = match close {
                    Close::Fixed(v) => *v,
                    Close::Age(baby, adult) => {
                        if m.is_baby() {
                            *baby
                        } else {
                            *adult
                        }
                    }
                };
                let pp = self.ent_pos(player).unwrap_or(m.pos);
                if dist2(m.pos, pp) < c * c {
                    b.mem.walk.erase();
                } else {
                    let s = Self::bz_speed_of(m, *speed);
                    b.mem.walk.set(Walk { target: Track::ent(player, false), speed: s, close: 2 });
                }
            }
            Bk::LongJump(j) => super::goat::long_jump_tick(self, m, b, j, t),
            Bk::PrepareRam(p) => super::goat::prepare_ram_tick(self, m, b, p, t),
            Bk::Ram { dir } => super::goat::ram_tick(self, m, b, *dir),
            Bk::Tongue(tg) => super::frog::tongue_tick(self, m, b, tg),
            Bk::Croak { counter } => *counter += 1,
            Bk::BallUp { peek, was } => super::armadillo::ball_up_tick(self, m, b, peek, was),
            Bk::GiveItems => super::allay::give_tick(self, m, b),
            Bk::Pig(p) => self.pig_tick(m, b, p),
            _ => {}
        }
    }

    /// `stop`.
    pub(crate) fn bz_bh_stop(&mut self, m: &mut Mob, b: &mut Brain, k: &mut Bk, t: i64, end: i64) {
        let timed_out = t > end;
        match k {
            Bk::X(x) => self.x_stop(m, b, x, t, end),
            Bk::Panic { .. } => b.mem.panicking.erase(),
            Bk::LookSink { .. } => b.mem.look.erase(),
            Bk::MoveSink { cd, path, .. } => {
                if let Some(w) = b.mem.walk.get()
                    && !self.bz_walk_reached(m, w)
                    && m.nav.stuck
                {
                    *cd = self.mobs.rng.next_int_bound(40);
                }
                m.nav.stop();
                b.mem.walk.erase();
                b.mem.path.erase();
                *path = None;
            }
            Bk::CountDown(key) => b.mem.erase(*key),
            Bk::MakeLove { at, .. } => {
                b.mem.breed.erase();
                b.mem.walk.erase();
                b.mem.look.erase();
                *at = 0;
            }
            Bk::Tempt { .. } => {
                b.mem.tempt_cd.set(100);
                b.mem.walk.erase();
                b.mem.look.erase();
            }
            Bk::LongJumpMid { lo, hi, sound } => {
                if m.on_ground {
                    m.vel[0] *= f64::from(0.1f32);
                    m.vel[2] *= f64::from(0.1f32);
                    self.bz_mob_sound_src(m, sound, 6, 2.0, 1.0);
                }
                super::set_discard_friction(m, false);
                super::set_pose(m, super::POSE_STANDING);
                b.mem.lj_mid.erase();
                let v = *lo + self.mobs.rng.next_int_bound(*hi - *lo + 1);
                b.mem.lj_cd.set(v);
            }
            Bk::PrepareRam(_) => super::goat::prepare_ram_stop(self, m, b),
            Bk::Tongue(_) => {
                b.mem.attack.erase();
                super::frog::set_tongue_target(m, None);
                super::set_pose(m, super::POSE_STANDING);
            }
            Bk::Croak { .. } => super::set_pose(m, super::POSE_STANDING),
            Bk::BallUp { .. } => {
                if !super::armadillo::can_stay_rolled_up(self, m, b) {
                    super::armadillo::roll_out(self, m);
                }
            }
            Bk::Dig => {
                if timed_out {
                    b.mem.sniff_cd.set_exp((), 9600);
                } else {
                    super::sniffer::reset_sniffing(self, m, b);
                }
            }
            Bk::FinishDig => {
                super::sniffer::transition(self, m, super::sniffer::IDLING);
                if timed_out {
                    super::sniffer::store_explored(self, m, b);
                }
                b.mem.digging.erase();
                b.mem.happy.set(true);
            }
            Bk::Happy => {
                super::sniffer::transition(self, m, super::sniffer::IDLING);
                b.mem.happy.erase();
            }
            Bk::Scent => super::sniffer::transition(self, m, super::sniffer::IDLING),
            Bk::Search => {
                if super::sniffer::can_dig(self, m, b) && super::sniffer::can_sniff(m, b) {
                    b.mem.digging.set(true);
                }
                b.mem.walk.erase();
                b.mem.sniff_target.erase();
            }
            Bk::Sniffing => {
                super::sniffer::transition(self, m, super::sniffer::IDLING);
                if timed_out && let Some(p) = super::sniffer::dig_position(self, m, b) {
                    b.mem.sniff_target.set(p);
                    b.mem.walk.set(Walk { target: Track::block(p), speed: 1.25, close: 0 });
                }
            }
            Bk::Pig(p) => self.pig_stop(m, b, p),
            _ => {}
        }
        let _ = t;
    }

    // ---------- Tools ----------

    fn bz_swim_ok(&self, m: &Mob) -> bool {
        (m.in_water && m.water_height > self.fluid_jump_threshold(m)) || m.lava_height > 0.0
    }

    pub(crate) fn bz_walk_reached(&self, m: &Mob, w: Walk) -> bool {
        self.bz_track_block(w.target).is_some_and(|p| manhattan(p, m.block_pos()) <= w.close)
    }

    /// Path kept by the behavior; failing that, a partial step toward the target
    /// (10/7, π/2).
    fn bz_try_compute_path(
        &mut self,
        m: &mut Mob,
        b: &mut Brain,
        w: Walk,
        t: i64,
        kept: &mut Option<Box<Path>>,
        speed: &mut f32,
    ) -> bool {
        let Some(target) = self.bz_track_block(w.target) else {
            *kept = None;
            return false;
        };
        *kept = super::path_begin(self, m, |s, m| s.create_path(m, target, 0)).map(Box::new);
        *speed = w.speed;
        if self.bz_walk_reached(m, w) {
            b.mem.cant_reach.erase();
        } else {
            if kept.as_ref().is_some_and(|p| p.reached) {
                b.mem.cant_reach.erase();
            } else if !b.mem.cant_reach.has() {
                b.mem.cant_reach.set(t);
            }
            if kept.is_some() {
                return true;
            }
            let to = [f64::from(target[0]) + 0.5, f64::from(target[1]), f64::from(target[2]) + 0.5];
            if let Some(step) = self.pos_towards(m, 10, 7, to, std::f32::consts::FRAC_PI_2) {
                *kept = super::path_begin(self, m, |s, m| s.create_path(m, block_of(step), 0)).map(Box::new);
                return kept.is_some();
            }
        }
        false
    }

    /// Flee point: nearest water when on fire, otherwise a land point (5/4); the happy baby ghast
    /// flees in the air or in water, in a quarter-turn cone around its gaze.
    pub(crate) fn bz_panic_pos(&self, m: &mut Mob) -> Option<[f64; 3]> {
        if m.on_fire()
            && let Some(w) = self.bz_panic_water(m)
        {
            return Some([f64::from(w[0]) + 0.5, f64::from(w[1]), f64::from(w[2]) + 0.5]);
        }
        if m.kind == super::super::species::Kind::HappyGhast {
            let v = super::allay::view_vector(m);
            let rad = f64::from(std::f32::consts::FRAC_PI_2);
            return super::super::wild::rpos::air_and_water_pos(self, m, 5, 4, 0, v[0], v[2], rad);
        }
        self.land_random_pos(m, 5, 4)
    }

    /// The water search of a panicking animal (Manhattan 5/1/5; width 2: three neighbors in water).
    fn bz_panic_water(&self, m: &Mob) -> Option<[i32; 3]> {
        use super::super::env::Fluid;
        let o = m.block_pos();
        if self.has_collision(o) {
            return None;
        }
        let water = |p: [i32; 3]| self.mob_fluid(p) == Fluid::Water;
        let wide = m.width().ceil() as i32 == 2;
        super::manhattan_find(o, 5, 1, |p| {
            water(p)
                && (!wide
                    || (water([p[0], p[1], p[2] + 1])
                        && water([p[0] + 1, p[1], p[2]])
                        && water([p[0] + 1, p[1], p[2] + 1])))
        })
    }

    /// Tiers (1,1) to (10,7), pursuit in the direction found.
    fn bz_swim_target_pos(&self, m: &mut Mob) -> Option<[f64; 3]> {
        const TIERS: [(i32, i32); 6] = [(1, 1), (3, 3), (5, 5), (6, 5), (7, 7), (10, 7)];
        let mut fallback: Option<[f64; 3]> = None;
        let mut target: Option<[f64; 3]> = None;
        for (h, v) in TIERS {
            target = match fallback {
                None => self.random_swimmable_pos(m, h, v),
                Some(f) => {
                    let d = [f[0] - m.pos[0], f[1] - m.pos[1], f[2] - m.pos[2]];
                    let l = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
                    let n = if l < 1.0e-4 { [0.0; 3] } else { d.map(|x| x / l) };
                    Some([
                        m.pos[0] + n[0] * f64::from(h),
                        m.pos[1] + n[1] * f64::from(v),
                        m.pos[2] + n[2] * f64::from(h),
                    ])
                }
            };
            match target {
                Some(p) if self.mob_fluid(block_of(p)) != super::super::env::Fluid::None => fallback = Some(p),
                _ => return fallback,
            }
        }
        target
    }

    /// Closest visible partner.
    fn bz_love_partner(&self, m: &mut Mob, b: &mut Brain) -> Option<i32> {
        let me = m.clone();
        let f = move |s: &Server, e: Ent| match e {
            Ent::Mob(id) => s
                .mob_ref(id)
                .is_some_and(|o| o.kind == me.kind && super::can_mate(s, &me, o) && !super::is_panicking(o)),
            Ent::Player => false,
        };
        match self.bz_visible_closest(m, b, &f) {
            Some(Ent::Mob(id)) => Some(id),
            _ => None,
        }
    }

    /// Mutual gaze with both walking toward each other.
    fn bz_lock_gaze(&mut self, m: &mut Mob, b: &mut Brain, pid: i32, speed: f32, close: i32) {
        let me = m.id;
        let t = Track::ent(Ent::Mob(pid), true);
        b.mem.look.set(t);
        b.mem.walk.set(Walk { target: t, speed, close });
        let t2 = Track::ent(Ent::Mob(me), true);
        self.bz_with_brain(pid, |_, ob| {
            ob.mem.look.set(t2);
            ob.mem.walk.set(Walk { target: t2, speed, close });
        });
    }

    /// Access to the brain of another mob of the family.
    pub(crate) fn bz_with_brain(&mut self, id: i32, f: impl FnOnce(&mut Mob, &mut Brain)) {
        if let Some(i) = self.mob_index(id)
            && let Some(o) = self.mobs.list[i].as_mut()
            && let Some(mut ob) = o.bm.brain.take()
        {
            f(o, &mut ob);
            o.bm.brain = Some(ob);
        }
    }

    /// The dry-land search (water at the feet; search by Manhattan distance, dry land below).
    fn bz_find_land(&mut self, m: &mut Mob, b: &mut Brain, range: i32, speed: f32, next: &mut i64, t: i64) -> bool {
        use super::super::env::Fluid;
        let o = m.block_pos();
        if self.mob_fluid(o) != Fluid::Water {
            return false;
        }
        if t < *next {
            *next = t + 60;
            return true;
        }
        let found = super::manhattan_find(o, range, range, |p| {
            (p[0] != o[0] || p[2] != o[2])
                && self.mob_fluid(p) == Fluid::None
                && !self.has_collision(p)
                && self.bz_face_sturdy_up([p[0], p[1] - 1, p[2]])
        });
        if let Some(p) = found {
            b.mem.look.set(Track::block(p));
            b.mem.walk.set(Walk { target: Track::block(p), speed, close: 1 });
        }
        *next = t + 60;
        true
    }

    /// The dry-land search near a liquid (frog fluid tag: water).
    fn bz_find_land_near_liquid(
        &mut self,
        m: &mut Mob,
        b: &mut Brain,
        range: i32,
        speed: f32,
        next: &mut i64,
        t: i64,
    ) -> bool {
        let o = m.block_pos();
        if self.mob_source_water(o) {
            return false;
        }
        if t < *next {
            *next = t + 40;
            return true;
        }
        let found = super::manhattan_find(o, range, range, |p| {
            if p[0] == o[0] && p[2] == o[2] {
                return false;
            }
            if self.has_collision(p) || !self.has_collision([p[0], p[1] - 1, p[2]]) {
                return false;
            }
            [[0, 0, -1], [1, 0, 0], [0, 0, 1], [-1, 0, 0]].iter().any(|d| {
                let q = [p[0] + d[0], p[1], p[2] + d[2]];
                self.is_air(q) && self.mob_source_water([q[0], q[1] - 1, q[2]])
            })
        });
        if let Some(p) = found {
            b.mem.look.set(Track::block(p));
            b.mem.walk.set(Walk { target: Track::block(p), speed, close: 0 });
        }
        *next = t + 40;
        true
    }

    /// The liquid search (`axolotl_tries_to_find` tag: water): water block, preferably with air
    /// above, otherwise the first one more than 1.5 blocks away.
    fn bz_find_liquid(&mut self, m: &mut Mob, b: &mut Brain, range: i32, speed: f32, next: &mut i64, t: i64) -> bool {
        let o = m.block_pos();
        if self.mob_source_water(o) {
            return false;
        }
        if t < *next {
            *next = t + 22;
            return true;
        }
        let me = m.pos;
        let mut found: Option<[i32; 3]> = None;
        let _ = super::manhattan_find(o, range, range, |p| {
            if (p[0] == o[0] && p[2] == o[2]) || self.bname(p) != "minecraft:water" || !self.mob_source_water(p) {
                return false;
            }
            if self.is_air([p[0], p[1] + 1, p[2]]) {
                found = Some(p);
                return true;
            }
            let c = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
            if found.is_none() && dist2(c, me) >= 1.5 * 1.5 {
                found = Some(p);
            }
            false
        });
        if let Some(p) = found {
            b.mem.look.set(Track::block(p));
            b.mem.walk.set(Walk { target: Track::block(p), speed, close: 0 });
        }
        *next = t + 40;
        true
    }

    /// Sound attached to a mob in a given category.
    pub(crate) fn bz_mob_sound_src(&mut self, m: &Mob, name: &str, category: i32, volume: f32, pitch: f32) {
        self.entity_sound(m.id, name, category, volume, pitch);
    }
}

/// The direction vector for a pitch and a yaw.
pub fn direction(pitch: f32, yaw: f32) -> [f64; 3] {
    use super::super::mth::{DEG, cos, sin};
    let f = cos(-yaw * DEG - std::f32::consts::PI);
    let g = sin(-yaw * DEG - std::f32::consts::PI);
    let h = -cos(-pitch * DEG);
    let i = sin(-pitch * DEG);
    [f64::from(g * h), f64::from(i), f64::from(f * h)]
}
