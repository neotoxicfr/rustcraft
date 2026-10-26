//! Brains of family F13 (`server-mobs.md` § 4.F13): piglin, brute, hoglin, zoglin.
//! The generic brain engine is that of F10 (`brainy/brain.rs`); this file adds the own
//! memories (`PMem`, keys `PK`), the sensors (`PSn`) and the behaviors (`PB`), then
//! builds the activities in the game's order.

use super::super::brainy::brain::{Act, Bk, Brain, K, Slot, Sn, St, act, core, move_sink, run_one, seq};
use super::super::jrand::JRand;
use super::super::mob::Ent;
use super::super::villager::brain::GPos;

// ---------- Memories ----------

/// Memories specific to piglins and hoglins (the common ones are in `Mem`).
#[derive(Debug, Clone, Default)]
pub struct PMem {
    /// "Angry at" memories (UUID) and "universal anger".
    pub angry_at: Slot<u128>,
    pub universal_anger: Slot<bool>,
    pub admiring_item: Slot<bool>,
    pub admiring_disabled: Slot<bool>,
    pub hunted_recently: Slot<bool>,
    /// Spear state (1 approach, 2 charge, 3 retreat); unregistered memory.
    pub spear_status: Slot<u8>,
    pub ate_recently: Slot<bool>,
    pub celebrate_location: Slot<[i32; 3]>,
    pub dancing: Slot<bool>,
    pub avoid_target: Slot<Ent>,
    pub ride_target: Slot<Ent>,
    pub interaction_target: Slot<Ent>,
    pub nearest_repellent: Slot<[i32; 3]>,
    pub nemesis: Slot<Ent>,
    pub zombified: Slot<Ent>,
    pub not_gold_player: Slot<Ent>,
    pub loved_item_player: Slot<Ent>,
    pub huntable_hoglin: Slot<Ent>,
    pub baby_hoglin: Slot<Ent>,
    /// Memory of the closest visible adult piglins (adult piglins and brutes seen).
    pub visible_adult_piglins: Slot<Vec<Ent>>,
    /// Memory of nearby adult piglins (adults among nearby entities, seen or not).
    pub nearby_adult_piglins: Slot<Vec<Ent>>,
    pub piglin_count: Slot<i32>,
    pub hoglin_count: Slot<i32>,
    pub time_reaching_item: Slot<i32>,
    pub disable_walk_to_item: Slot<bool>,
    /// Hoglin: "pacified" memory, closest adult piglin, adult hoglins seen.
    pub pacified: Slot<bool>,
    pub adult_piglin: Slot<Ent>,
    pub adult_hoglins: Slot<Vec<Ent>>,
    /// Brute: "home" memory (spawn place).
    pub home: Slot<GPos>,
    /// Memory of the doors to close again (doors opened along the way) and last path seen.
    pub doors_to_close: Slot<Vec<[i32; 3]>>,
}

/// Keys of the own memories.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PK {
    AngryAt,
    UniversalAnger,
    AdmiringItem,
    AdmiringDisabled,
    HuntedRecently,
    SpearStatus,
    AteRecently,
    CelebrateLocation,
    Dancing,
    AvoidTarget,
    RideTarget,
    InteractionTarget,
    NearestRepellent,
    Nemesis,
    Zombified,
    NotGoldPlayer,
    LovedItemPlayer,
    HuntableHoglin,
    BabyHoglin,
    VisibleAdultPiglins,
    TimeReachingItem,
    DisableWalkToItem,
    Pacified,
    AdultPiglin,
}

impl PMem {
    pub fn tick(&mut self) {
        self.angry_at.tick();
        self.universal_anger.tick();
        self.admiring_item.tick();
        self.admiring_disabled.tick();
        self.hunted_recently.tick();
        self.spear_status.tick();
        self.ate_recently.tick();
        self.celebrate_location.tick();
        self.dancing.tick();
        self.avoid_target.tick();
        self.ride_target.tick();
        self.interaction_target.tick();
        self.nearest_repellent.tick();
        self.nemesis.tick();
        self.zombified.tick();
        self.not_gold_player.tick();
        self.loved_item_player.tick();
        self.huntable_hoglin.tick();
        self.baby_hoglin.tick();
        self.visible_adult_piglins.tick();
        self.nearby_adult_piglins.tick();
        self.piglin_count.tick();
        self.hoglin_count.tick();
        self.time_reaching_item.tick();
        self.disable_walk_to_item.tick();
        self.pacified.tick();
        self.adult_piglin.tick();
        self.adult_hoglins.tick();
        self.home.tick();
        self.doors_to_close.tick();
    }

    pub fn has(&self, k: PK) -> bool {
        match k {
            PK::AngryAt => self.angry_at.has(),
            PK::UniversalAnger => self.universal_anger.has(),
            PK::AdmiringItem => self.admiring_item.has(),
            PK::AdmiringDisabled => self.admiring_disabled.has(),
            PK::HuntedRecently => self.hunted_recently.has(),
            PK::SpearStatus => self.spear_status.has(),
            PK::AteRecently => self.ate_recently.has(),
            PK::CelebrateLocation => self.celebrate_location.has(),
            PK::Dancing => self.dancing.has(),
            PK::AvoidTarget => self.avoid_target.has(),
            PK::RideTarget => self.ride_target.has(),
            PK::InteractionTarget => self.interaction_target.has(),
            PK::NearestRepellent => self.nearest_repellent.has(),
            PK::Nemesis => self.nemesis.has(),
            PK::Zombified => self.zombified.has(),
            PK::NotGoldPlayer => self.not_gold_player.has(),
            PK::LovedItemPlayer => self.loved_item_player.has(),
            PK::HuntableHoglin => self.huntable_hoglin.has(),
            PK::BabyHoglin => self.baby_hoglin.has(),
            PK::VisibleAdultPiglins => self.visible_adult_piglins.has(),
            PK::TimeReachingItem => self.time_reaching_item.has(),
            PK::DisableWalkToItem => self.disable_walk_to_item.has(),
            PK::Pacified => self.pacified.has(),
            PK::AdultPiglin => self.adult_piglin.has(),
        }
    }

    pub fn erase(&mut self, k: PK) {
        match k {
            PK::AngryAt => self.angry_at.erase(),
            PK::UniversalAnger => self.universal_anger.erase(),
            PK::AdmiringItem => self.admiring_item.erase(),
            PK::AdmiringDisabled => self.admiring_disabled.erase(),
            PK::HuntedRecently => self.hunted_recently.erase(),
            PK::SpearStatus => self.spear_status.erase(),
            PK::AteRecently => self.ate_recently.erase(),
            PK::CelebrateLocation => self.celebrate_location.erase(),
            PK::Dancing => self.dancing.erase(),
            PK::AvoidTarget => self.avoid_target.erase(),
            PK::RideTarget => self.ride_target.erase(),
            PK::InteractionTarget => self.interaction_target.erase(),
            PK::NearestRepellent => self.nearest_repellent.erase(),
            PK::Nemesis => self.nemesis.erase(),
            PK::Zombified => self.zombified.erase(),
            PK::NotGoldPlayer => self.not_gold_player.erase(),
            PK::LovedItemPlayer => self.loved_item_player.erase(),
            PK::HuntableHoglin => self.huntable_hoglin.erase(),
            PK::BabyHoglin => self.baby_hoglin.erase(),
            PK::VisibleAdultPiglins => self.visible_adult_piglins.erase(),
            PK::TimeReachingItem => self.time_reaching_item.erase(),
            PK::DisableWalkToItem => self.disable_walk_to_item.erase(),
            PK::Pacified => self.pacified.erase(),
            PK::AdultPiglin => self.adult_piglin.erase(),
        }
    }
}

// ---------- Sensors ----------

/// Own sensors (period 20, like all the game's sensors without override).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PSn {
    /// The piglin's own sensor.
    Piglin,
    /// The piglin brute's own sensor.
    Brute,
    /// The hoglin's own sensor.
    Hoglin,
    /// The closest-item sensor with the piglin's desire.
    Items,
}

impl PSn {
    pub fn period(self) -> i32 {
        20
    }
}

// ---------- Behaviors ----------

/// Who to look at or approach (`InteractWith`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Who {
    Any,
    Player,
    Piglin,
    Brute,
    /// Player holding a loved item.
    LovedItemPlayer,
}

/// Attack target (`StartAttacking`: species-specific search).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Finder {
    /// Adult piglin only.
    Piglin,
    Brute,
    Hoglin,
    Zoglin,
}

/// Own behaviors (kind: instant trigger or duration).
#[derive(Debug, Clone)]
pub enum PB {
    // Piglin core.
    /// Opens the doors of the path, closes those left behind.
    Doors {
        last_path: Option<Vec<[i32; 3]>>,
    },
    /// Baby → flee the nemesis for 5 to 7 s.
    BabyAvoidNemesis,
    /// Zombified within 6 → flee for 5 to 7 s.
    AvoidZombified,
    StopHoldingItem,
    StartAdmiring,
    StartCelebrating,
    StopAngryIfDead,
    // Idle.
    LookAt {
        who: Who,
        range: f32,
    },
    StartAttacking(Finder),
    StartHunting,
    /// The flee behavior from the closest repellent.
    AwayFromRepellent {
        speed: f32,
        dist: i32,
        interrupt: bool,
    },
    /// Target to flee (the "avoid target" memory or adult piglin).
    AwayFromEntity {
        from: PK,
        speed: f32,
        dist: i32,
        interrupt: bool,
        adult_only: bool,
    },
    /// The memory copy with expiry of the baby riding a baby hoglin (timer of 10 to 40 s).
    BabyRide {
        ticker: i32,
    },
    Stroll {
        speed: f32,
        h: i32,
        v: i32,
    },
    InteractWith {
        who: Who,
        speed: f32,
        stop: i32,
    },
    /// The walk target drawn from the look target, under the condition "no player with a loved item".
    WalkFromLook {
        speed: f32,
        close: i32,
        if_no_loved: bool,
    },
    LookAndInteract {
        range: i32,
    },
    // Combat.
    StopAttackingIfInvalid {
        finder: Option<Finder>,
    },
    BackUpIfTooClose {
        dist: i32,
        speed: f32,
    },
    WalkToAttack {
        speed: f32,
    },
    /// The melee attack; age condition (hoglin, zoglin).
    Melee {
        cd: i32,
        adult: Option<bool>,
    },
    Crossbow {
        state: u8,
        delay: i32,
    },
    /// `SpearApproach`, `SpearAttack`, `SpearRetreat` (spear: see `spear.rs`).
    SpearApproach,
    SpearAttack {
        engage: i32,
        charge: Option<[f64; 3]>,
    },
    SpearRetreat {
        time: i32,
        away: Option<[f64; 3]>,
    },
    RememberHoglinKill,
    EraseAttackIfNearZombified,
    // Celebration.
    GoToCelebrate {
        close: i32,
        speed: f32,
        dancing: bool,
    },
    // Admiring.
    GoToWantedItem,
    StopAdmiringTooFar,
    StopAdmiringTired,
    // Fleeing.
    EraseAvoidIfStop,
    // Riding.
    Mount {
        speed: f32,
    },
    /// Sequence "is a passenger" then one of the looks or nothing (shuffled).
    RideLook {
        rng: JRand,
        order: Vec<usize>,
    },
    Dismount,
    // Hoglin.
    BecomePassive,
    EraseAttackIfBreeding,
    // Brute.
    StrollToHome {
        next: i64,
    },
    StrollAroundHome {
        next: i64,
    },
}

/// Kind of an own behavior.
pub enum PCtl {
    OneShot,
    Timed(i32, i32),
}

impl PB {
    pub fn ctl(&self) -> PCtl {
        match self {
            PB::Crossbow { .. } => PCtl::Timed(1200, 1200),
            // Neither the approach nor the attack stops by timeout (always false).
            PB::SpearApproach | PB::SpearAttack { .. } => PCtl::Timed(1_000_000_000, 1_000_000_000),
            PB::SpearRetreat { .. } => PCtl::Timed(100, 100),
            _ => PCtl::OneShot,
        }
    }

    /// Memory conditions (assembling a behavior's conditions or the entry condition).
    pub fn entry(&self) -> &'static [(K, St)] {
        use K::*;
        use St::*;
        const ANGRY: K = P(PK::AngryAt);
        const ADMIRING: K = P(PK::AdmiringItem);
        match self {
            PB::Doors { .. } => &[(Path, Present), (P(PK::InteractionTarget), Registered)],
            PB::BabyAvoidNemesis => &[(P(PK::Nemesis), Present), (P(PK::AvoidTarget), Absent)],
            PB::AvoidZombified => &[(P(PK::Zombified), Present), (P(PK::AvoidTarget), Absent)],
            PB::StopHoldingItem => &[(ADMIRING, Absent)],
            PB::StartAdmiring => &[
                (WantedItem, Present),
                (ADMIRING, Absent),
                (P(PK::AdmiringDisabled), Absent),
                (P(PK::DisableWalkToItem), Absent),
            ],
            PB::StartCelebrating => &[
                (Attack, Present),
                (ANGRY, Registered),
                (P(PK::CelebrateLocation), Absent),
                (P(PK::Dancing), Registered),
            ],
            PB::StopAngryIfDead => &[(ANGRY, Present)],
            PB::LookAt { .. } => &[(Look, Absent), (Visible, Present)],
            PB::StartAttacking(_) => &[(Attack, Absent), (CantReach, Registered)],
            PB::StartHunting => &[
                (P(PK::HuntableHoglin), Present),
                (ANGRY, Absent),
                (P(PK::HuntedRecently), Absent),
                (P(PK::VisibleAdultPiglins), Registered),
            ],
            PB::AwayFromRepellent { .. } => &[(Walk, Registered), (P(PK::NearestRepellent), Present)],
            PB::AwayFromEntity { from: PK::AvoidTarget, .. } => &[(Walk, Registered), (P(PK::AvoidTarget), Present)],
            PB::AwayFromEntity { .. } => &[(Walk, Registered), (P(PK::AdultPiglin), Present)],
            PB::BabyRide { .. } => &[(P(PK::BabyHoglin), Present), (P(PK::RideTarget), Absent)],
            PB::Stroll { .. } => &[(Walk, Absent)],
            PB::InteractWith { .. } => {
                &[(P(PK::InteractionTarget), Registered), (Look, Registered), (Walk, Absent), (Visible, Present)]
            }
            PB::WalkFromLook { .. } => &[(Walk, Absent), (Look, Present)],
            PB::LookAndInteract { .. } => &[(Look, Registered), (P(PK::InteractionTarget), Absent), (Visible, Present)],
            PB::StopAttackingIfInvalid { .. } => &[(Attack, Present), (CantReach, Registered)],
            PB::BackUpIfTooClose { .. } => &[(Walk, Absent), (Look, Registered), (Attack, Present), (Visible, Present)],
            PB::WalkToAttack { .. } => {
                &[(Walk, Registered), (Look, Registered), (Attack, Present), (Visible, Registered)]
            }
            PB::Melee { .. } => &[(Look, Registered), (Attack, Present), (AttackCooling, Absent), (Visible, Present)],
            PB::Crossbow { .. } => &[(Look, Registered), (Attack, Present)],
            PB::SpearApproach => &[(P(PK::SpearStatus), Absent)],
            PB::SpearAttack { .. } | PB::SpearRetreat { .. } => &[(P(PK::SpearStatus), Present)],
            PB::RememberHoglinKill => &[(Attack, Present), (P(PK::HuntedRecently), Registered)],
            PB::EraseAttackIfNearZombified | PB::EraseAttackIfBreeding => &[(Attack, Present)],
            PB::BecomePassive => &[(Attack, Registered), (P(PK::Pacified), Absent), (P(PK::NearestRepellent), Present)],
            PB::GoToCelebrate { .. } => {
                &[(P(PK::CelebrateLocation), Present), (Attack, Absent), (Walk, Absent), (Look, Registered)]
            }
            PB::GoToWantedItem => {
                &[(Look, Registered), (Walk, Registered), (WantedItem, Present), (PickupCd, Registered)]
            }
            PB::StopAdmiringTooFar => &[(ADMIRING, Present), (WantedItem, Registered)],
            PB::StopAdmiringTired => &[
                (ADMIRING, Present),
                (WantedItem, Present),
                (P(PK::TimeReachingItem), Registered),
                (P(PK::DisableWalkToItem), Registered),
            ],
            PB::EraseAvoidIfStop => &[(P(PK::AvoidTarget), Present)],
            PB::Mount { .. } => &[(Look, Registered), (Walk, Absent), (P(PK::RideTarget), Present)],
            PB::RideLook { .. } => &[],
            PB::Dismount => &[(P(PK::RideTarget), Registered)],
            PB::StrollToHome { .. } | PB::StrollAroundHome { .. } => &[(Walk, Registered)],
        }
    }
}

fn pb(k: PB) -> Bk {
    Bk::Pig(Box::new(k))
}

fn psn(s: PSn) -> Sn {
    Sn::P(s)
}

/// The single-choice gate of idle looks: player, piglin, anyone (8 blocks), then nothing (30 to 60).
fn idle_look(salt: i64, brute: bool) -> Bk {
    let mut kids = vec![
        (pb(PB::LookAt { who: Who::Player, range: 8.0 }), 1),
        (pb(PB::LookAt { who: Who::Piglin, range: 8.0 }), 1),
    ];
    if brute {
        kids.push((pb(PB::LookAt { who: Who::Brute, range: 8.0 }), 1));
    }
    kids.push((pb(PB::LookAt { who: Who::Any, range: 8.0 }), 1));
    kids.push((Bk::DoNothing { lo: 30, hi: 60 }, 1));
    run_one(&[], salt, kids)
}

/// The single-choice gate of the piglin's idle movements.
fn idle_move(salt: i64) -> Bk {
    run_one(
        &[],
        salt,
        vec![
            (pb(PB::Stroll { speed: 0.6, h: 10, v: 7 }), 2),
            (pb(PB::InteractWith { who: Who::Piglin, speed: 0.6, stop: 2 }), 2),
            (pb(PB::WalkFromLook { speed: 0.6, close: 3, if_no_loved: true }), 2),
            (Bk::DoNothing { lo: 30, hi: 60 }, 1),
        ],
    )
}

fn repellent() -> Bk {
    pb(PB::AwayFromRepellent { speed: 1.0, dist: 8, interrupt: false })
}

/// Piglin brain.
pub fn piglin(rng: &mut JRand) -> Brain {
    let core_nodes = seq(
        0,
        Act::Core,
        vec![
            Bk::LookSink { lo: 45, hi: 90 },
            move_sink(),
            pb(PB::Doors { last_path: None }),
            pb(PB::BabyAvoidNemesis),
            pb(PB::AvoidZombified),
            pb(PB::StopHoldingItem),
            pb(PB::StartAdmiring),
            pb(PB::StartCelebrating),
            pb(PB::StopAngryIfDead),
        ],
    );
    let idle = seq(
        10,
        Act::Idle,
        vec![
            pb(PB::LookAt { who: Who::LovedItemPlayer, range: 14.0 }),
            pb(PB::StartAttacking(Finder::Piglin)),
            pb(PB::StartHunting),
            repellent(),
            pb(PB::BabyRide { ticker: 0 }),
            idle_look(0x5049_474c, false),
            idle_move(0x5049_474d),
            pb(PB::LookAndInteract { range: 4 }),
        ],
    );
    let admire =
        seq(10, Act::AdmireItem, vec![pb(PB::GoToWantedItem), pb(PB::StopAdmiringTooFar), pb(PB::StopAdmiringTired)]);
    let fight = seq(
        10,
        Act::Fight,
        vec![
            pb(PB::StopAttackingIfInvalid { finder: Some(Finder::Piglin) }),
            pb(PB::BackUpIfTooClose { dist: 5, speed: 0.75 }),
            pb(PB::WalkToAttack { speed: 1.0 }),
            pb(PB::SpearApproach),
            pb(PB::SpearAttack { engage: 0, charge: None }),
            pb(PB::SpearRetreat { time: 0, away: None }),
            pb(PB::Melee { cd: 20, adult: None }),
            pb(PB::Crossbow { state: 0, delay: 0 }),
            pb(PB::RememberHoglinKill),
            pb(PB::EraseAttackIfNearZombified),
        ],
    );
    let celebrate = seq(
        10,
        Act::Celebrate,
        vec![
            repellent(),
            pb(PB::LookAt { who: Who::LovedItemPlayer, range: 14.0 }),
            pb(PB::StartAttacking(Finder::Piglin)),
            pb(PB::GoToCelebrate { close: 2, speed: 1.0, dancing: false }),
            pb(PB::GoToCelebrate { close: 4, speed: 0.6, dancing: true }),
            run_one(
                &[],
                0x4345_4c45,
                vec![
                    (pb(PB::LookAt { who: Who::Piglin, range: 8.0 }), 1),
                    (pb(PB::Stroll { speed: 0.6, h: 2, v: 1 }), 1),
                    (Bk::DoNothing { lo: 10, hi: 20 }, 1),
                ],
            ),
        ],
    );
    let avoid = seq(
        10,
        Act::Avoid,
        vec![
            pb(PB::AwayFromEntity { from: PK::AvoidTarget, speed: 1.0, dist: 12, interrupt: true, adult_only: false }),
            idle_look(0x4156_4f49, false),
            idle_move(0x4156_4f4a),
            pb(PB::EraseAvoidIfStop),
        ],
    );
    let ride = seq(
        10,
        Act::Ride,
        vec![
            pb(PB::Mount { speed: 0.8 }),
            pb(PB::LookAt { who: Who::LovedItemPlayer, range: 8.0 }),
            pb(PB::RideLook { rng: JRand::from_time(0x5249_4445), order: Vec::new() }),
            pb(PB::Dismount),
        ],
    );
    use K::*;
    use St::*;
    Brain::new(
        rng,
        &[Sn::Living, Sn::Players, psn(PSn::Items), Sn::HurtBy, psn(PSn::Piglin)],
        vec![
            (core(), core_nodes),
            (act(Act::Idle, &[], &[]), idle),
            (act(Act::AdmireItem, &[(P(PK::AdmiringItem), Present)], &[P(PK::AdmiringItem)]), admire),
            (act(Act::Fight, &[(Attack, Present)], &[Attack]), fight),
            (act(Act::Celebrate, &[(P(PK::CelebrateLocation), Present)], &[P(PK::CelebrateLocation)]), celebrate),
            (act(Act::Avoid, &[(P(PK::AvoidTarget), Present)], &[P(PK::AvoidTarget)]), avoid),
            (act(Act::Ride, &[(P(PK::RideTarget), Present)], &[P(PK::RideTarget)]), ride),
        ],
    )
}

/// Brute brain.
pub fn brute(rng: &mut JRand) -> Brain {
    let core_nodes = seq(
        0,
        Act::Core,
        vec![Bk::LookSink { lo: 45, hi: 90 }, move_sink(), pb(PB::Doors { last_path: None }), pb(PB::StopAngryIfDead)],
    );
    let idle = seq(
        10,
        Act::Idle,
        vec![
            pb(PB::StartAttacking(Finder::Brute)),
            idle_look(0x4252_5554, true),
            run_one(
                &[],
                0x4252_5555,
                vec![
                    (pb(PB::Stroll { speed: 0.6, h: 10, v: 7 }), 2),
                    (pb(PB::InteractWith { who: Who::Piglin, speed: 0.6, stop: 2 }), 2),
                    (pb(PB::InteractWith { who: Who::Brute, speed: 0.6, stop: 2 }), 2),
                    (pb(PB::StrollToHome { next: 0 }), 2),
                    (pb(PB::StrollAroundHome { next: 0 }), 2),
                    (Bk::DoNothing { lo: 30, hi: 60 }, 1),
                ],
            ),
            pb(PB::LookAndInteract { range: 4 }),
        ],
    );
    let fight = seq(
        10,
        Act::Fight,
        vec![
            pb(PB::StopAttackingIfInvalid { finder: Some(Finder::Brute) }),
            pb(PB::WalkToAttack { speed: 1.0 }),
            pb(PB::Melee { cd: 20, adult: None }),
        ],
    );
    use K::*;
    use St::*;
    Brain::new(
        rng,
        &[Sn::Living, Sn::Players, psn(PSn::Items), Sn::HurtBy, psn(PSn::Brute)],
        vec![
            (core(), core_nodes),
            (act(Act::Idle, &[], &[]), idle),
            (act(Act::Fight, &[(Attack, Present)], &[Attack]), fight),
        ],
    )
}

/// The single-choice gate of the idle movements of the hoglin and the zoglin.
fn hog_move(salt: i64) -> Bk {
    run_one(
        &[],
        salt,
        vec![
            (pb(PB::Stroll { speed: 0.4, h: 10, v: 7 }), 2),
            (pb(PB::WalkFromLook { speed: 0.4, close: 3, if_no_loved: false }), 2),
            (Bk::DoNothing { lo: 30, hi: 60 }, 1),
        ],
    )
}

/// Hoglin brain.
pub fn hoglin(rng: &mut JRand) -> Brain {
    let core_nodes = seq(0, Act::Core, vec![Bk::LookSink { lo: 45, hi: 90 }, move_sink()]);
    let idle = seq(
        10,
        Act::Idle,
        vec![
            pb(PB::BecomePassive),
            Bk::MakeLove { speed: 0.6, close: 2, at: 0, sniffer: false },
            pb(PB::AwayFromRepellent { speed: 1.0, dist: 8, interrupt: true }),
            pb(PB::StartAttacking(Finder::Hoglin)),
            pb(PB::AwayFromEntity { from: PK::AdultPiglin, speed: 0.4, dist: 8, interrupt: false, adult_only: true }),
            Bk::LookSometimes { range: 8.0, players: false, ticker: 0, lo: 30, hi: 60 },
            Bk::BabyFollow { lo: 5, hi: 16, speed: super::super::brainy::brain::Spd::Fixed(0.6) },
            hog_move(0x484f_474c),
        ],
    );
    let fight = seq(
        10,
        Act::Fight,
        vec![
            pb(PB::BecomePassive),
            Bk::MakeLove { speed: 0.6, close: 2, at: 0, sniffer: false },
            pb(PB::WalkToAttack { speed: 1.0 }),
            pb(PB::Melee { cd: 40, adult: Some(true) }),
            pb(PB::Melee { cd: 15, adult: Some(false) }),
            pb(PB::StopAttackingIfInvalid { finder: None }),
            pb(PB::EraseAttackIfBreeding),
        ],
    );
    let avoid = seq(
        10,
        Act::Avoid,
        vec![
            pb(PB::AwayFromEntity { from: PK::AvoidTarget, speed: 1.3, dist: 15, interrupt: false, adult_only: false }),
            hog_move(0x484f_474d),
            Bk::LookSometimes { range: 8.0, players: false, ticker: 0, lo: 30, hi: 60 },
            pb(PB::EraseAvoidIfStop),
        ],
    );
    use K::*;
    use St::*;
    Brain::new(
        rng,
        &[Sn::Living, Sn::Players, Sn::Adult, psn(PSn::Hoglin)],
        vec![
            (core(), core_nodes),
            (act(Act::Idle, &[], &[]), idle),
            (act(Act::Fight, &[(Attack, Present)], &[Attack]), fight),
            (act(Act::Avoid, &[(P(PK::AvoidTarget), Present)], &[P(PK::AvoidTarget)]), avoid),
        ],
    )
}

/// Zoglin brain.
pub fn zoglin(rng: &mut JRand) -> Brain {
    let core_nodes = seq(0, Act::Core, vec![Bk::LookSink { lo: 45, hi: 90 }, move_sink()]);
    let idle = seq(
        10,
        Act::Idle,
        vec![
            pb(PB::StartAttacking(Finder::Zoglin)),
            Bk::LookSometimes { range: 8.0, players: false, ticker: 0, lo: 30, hi: 60 },
            hog_move(0x5a4f_474c),
        ],
    );
    let fight = seq(
        10,
        Act::Fight,
        vec![
            pb(PB::WalkToAttack { speed: 1.0 }),
            pb(PB::Melee { cd: 40, adult: Some(true) }),
            pb(PB::Melee { cd: 15, adult: Some(false) }),
            pb(PB::StopAttackingIfInvalid { finder: None }),
        ],
    );
    use K::*;
    use St::*;
    Brain::new(
        rng,
        &[Sn::Living, Sn::Players],
        vec![
            (core(), core_nodes),
            (act(Act::Idle, &[], &[]), idle),
            (act(Act::Fight, &[(Attack, Present)], &[Attack]), fight),
        ],
    )
}
