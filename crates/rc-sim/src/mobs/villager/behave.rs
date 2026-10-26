//! Villager behaviors (the game's goal packages, F05): instant triggers and
//! behaviors with a duration, in the order of the core, work, play, rest, meet,
//! idle, panic and hide packages. The draws follow the game's: durations and world
//! randoms on the mobs' world random, positions and small draws on the own random.

use super::super::jrand::JRand;
use super::super::mob::{Ent, Mob};
use super::super::nav::Path as NavPath;
use super::super::species::Kind;
use super::brain::{Act, Brain, GPos, M, Node, St, Track, Walk, b, dist2, gate, idle, o, run_one};
use super::{PROF_FARMER, PROF_NITWIT, PROF_NONE};
use crate::player::Stack;
use crate::poi::{Occupancy, PoiType};
use crate::server::Server;
use rc_proto::nbt::Nbt;

/// POI (location) memory of a villager.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PoiMem {
    Home,
    JobSite,
    Potential,
    Meeting,
}

impl PoiMem {
    pub fn get(self, b: &Brain) -> Option<GPos> {
        match self {
            PoiMem::Home => b.mem.home,
            PoiMem::JobSite => b.mem.job_site,
            PoiMem::Potential => b.mem.potential_job_site,
            PoiMem::Meeting => b.mem.meeting_point,
        }
    }

    pub fn set(self, b: &mut Brain, v: Option<GPos>) {
        match self {
            PoiMem::Home => b.mem.home = v,
            PoiMem::JobSite => b.mem.job_site = v,
            PoiMem::Potential => b.mem.potential_job_site = v,
            PoiMem::Meeting => b.mem.meeting_point = v,
        }
    }
}

/// Behaviors with a duration.
#[derive(Debug, Clone)]
pub enum Beh {
    Swim,
    LookAtTarget,
    PanicTrigger,
    MoveToTarget { min: i32, max: i32, cooldown: i32, path: Option<Box<NavPath>>, last: Option<[i32; 3]>, speed: f32 },
    FollowTrader,
    GoToPotentialJobSite,
    WorkAtPoi { last_check: i64, composter: bool },
    Harvest { pos: Option<[i32; 3]>, next_ok: i64, worked: i32, valid: Vec<[i32; 3]> },
    Bonemeal { next_cycle: i64, last_session: i32, worked: i32, crop: Option<[i32; 3]> },
    ShowTrades { held: Option<Option<Stack>>, display: Vec<Stack>, cycle: i32, index: usize, look: i32 },
    GiveGift { until: i32, given: bool, since: i64 },
    JumpOnBed { speed: f32, bed: Option<[i32; 3]>, reach: i32, jumps: i32, cooldown: i32 },
    SleepInBed { next_ok: i64 },
    TradeWithVillager { trades: Vec<&'static str> },
    MakeLove { birth: i64 },
    // Celebration after the village's victory (sounds, fireworks).
    Celebrate { raid: Option<i32> },
}

/// New POI acquisition attempt (linear delay with jitter).
#[derive(Debug, Clone, Copy)]
pub struct Retry {
    prev: i64,
    next: i64,
    delay: i32,
}

impl Retry {
    fn new(rng: &mut JRand, t: i64) -> Retry {
        let mut r = Retry { prev: 0, next: 0, delay: 0 };
        r.mark(rng, t);
        r
    }

    fn mark(&mut self, rng: &mut JRand, t: i64) {
        self.prev = t;
        let d = self.delay + rng.next_int_bound(40) + 40;
        self.delay = d.min(400);
        self.next = t + i64::from(self.delay);
    }
}

/// Type of POI acquired.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Acq {
    Job,
    Home,
    Meeting,
}

/// Gaze target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LookKind {
    Cat,
    Villager,
    Player,
    Category(u8),
}

/// Instant triggers.
#[derive(Debug, Clone)]
pub enum Trig {
    Door { last: Option<[i32; 3]>, cooldown: i32 },
    WakeUp,
    ReactToBell,
    SetRaidStatus,
    Validate { mem: PoiMem, job: bool },
    CompetitorScan,
    GoToWantedItem,
    Acquire { kind: Acq, next: i64, cache: Vec<([i32; 3], Retry)> },
    YieldJobSite,
    AssignProfession,
    ResetProfession,
    StrollAroundPoi { mem: PoiMem, speed: f32, max: i32, next: i64 },
    StrollToPoi { mem: PoiMem, speed: f32, close: i32, max: i32, next: i64 },
    StrollToSecondary { next: i64 },
    SetLookAndInteract,
    WalkFromBlock { mem: PoiMem, speed: f32, close: i32, far: i32, too_long: i64 },
    UpdateSchedule,
    Look(LookKind),
    InteractWith { kind: LookKind, breed: bool },
    VillageStroll { speed: f32, h: i32, v: i32 },
    WalkFromLook { speed: f32, close: i32 },
    PlayTag,
    ClosestHome { cache: Vec<([i32; 3], i64)>, last: i64 },
    InsideBrownian,
    ClosestVillage,
    SocializeAtBell,
    CalmDown,
    AwayFrom { hostile: bool },
    Shuffled { kids: Vec<(Trig, i32, f64)>, rng: JRand },
    // Before the raid: ring the meeting-point bell (one time in 20, at 3 blocks).
    RingBell,
    // Raid finished, stopped or lost: back to idle and the schedule.
    ResetRaid,
    // Conditional sequence: raid won (`victory`) or raid in progress, then the trigger.
    IfRaid { victory: bool, then: Box<Trig> },
    // Go under the sky (10 tries at ±10/±3/±10).
    MoveToSky { speed: f32 },
    // Find a hiding place (house) and walk to it.
    LocateHiding { radius: i32, speed: f32, close: i32 },
    // Stay hidden `stay` ticks near the hiding place (300 ticks at most after the bell).
    SetHidden { stay: i32, close: i32, hidden: i32 },
}

impl Beh {
    pub fn entry(&self) -> Vec<(M, St)> {
        use M::*;
        use St::*;
        match self {
            Beh::LookAtTarget => vec![(Look, Present)],
            Beh::MoveToTarget { .. } => vec![(Path, Absent), (Walk, Present)],
            Beh::GoToPotentialJobSite => vec![(PotentialJobSite, Present)],
            Beh::WorkAtPoi { .. } => vec![(JobSite, Present)],
            Beh::Harvest { .. } => vec![(Look, Absent), (Walk, Absent), (Secondary, Present)],
            Beh::Bonemeal { .. } => vec![(Look, Absent), (Walk, Absent)],
            Beh::ShowTrades { .. } => vec![(Interaction, Present)],
            Beh::GiveGift { .. } => vec![(VisiblePlayer, Present)],
            Beh::JumpOnBed { .. } => vec![(NearestBed, Present), (Walk, Absent)],
            Beh::SleepInBed { .. } => vec![(Home, Present)],
            Beh::TradeWithVillager { .. } => vec![(Interaction, Present), (Visible, Present)],
            Beh::MakeLove { .. } => vec![(Breed, Present), (Visible, Present)],
            _ => Vec::new(),
        }
    }

    pub fn duration(&self) -> (i32, i32) {
        match self {
            Beh::LookAtTarget => (45, 90),
            Beh::MoveToTarget { min, max, .. } => (*min, *max),
            Beh::FollowTrader => (i32::MAX, i32::MAX),
            Beh::GoToPotentialJobSite => (1200, 1200),
            Beh::ShowTrades { .. } => (400, 1600),
            Beh::GiveGift { .. } => (100, 100),
            Beh::MakeLove { .. } => (350, 350),
            Beh::Celebrate { .. } => (600, 600),
            _ => (60, 60),
        }
    }

    /// False for behaviors without delay (always false).
    pub fn can_time_out(&self) -> bool {
        !matches!(self, Beh::FollowTrader | Beh::JumpOnBed { .. } | Beh::SleepInBed { .. })
    }
}

/// Activity packages of the villager (priority, activity, node), in the game's insertion order.
pub fn packages(baby: bool, prof: i32) -> Vec<(i32, Act, Node)> {
    let mut v: Vec<(i32, Act, Node)> = Vec::new();
    let sp = 0.5f32;
    let mut add = |a: Act, list: Vec<(i32, Node)>| {
        for (p, n) in list {
            v.push((p, a, n));
        }
    };
    let acquire = |k: Acq| o(Trig::Acquire { kind: k, next: 0, cache: Vec::new() });
    let minimal_look = || {
        (
            5,
            run_one(vec![
                (o(Trig::Look(LookKind::Villager)), 2),
                (o(Trig::Look(LookKind::Player)), 2),
                (idle(30, 60), 8),
            ]),
        )
    };
    let full_look = || {
        let mut kids = vec![
            (o(Trig::Look(LookKind::Cat)), 8),
            (o(Trig::Look(LookKind::Villager)), 2),
            (o(Trig::Look(LookKind::Player)), 2),
        ];
        for c in 0..6 {
            kids.push((o(Trig::Look(LookKind::Category(c))), 1));
        }
        kids.push((idle(30, 60), 2));
        (5, run_one(kids))
    };
    let move_to = |min, max| b(Beh::MoveToTarget { min, max, cooldown: 0, path: None, last: None, speed: 0.0 });
    // Work or play (added before the core).
    if baby {
        add(
            Act::Play,
            vec![
                (0, move_to(80, 120)),
                full_look(),
                (5, o(Trig::PlayTag)),
                (
                    5,
                    gate(
                        vec![(M::Babies, St::Absent)],
                        Vec::new(),
                        true,
                        vec![
                            (o(Trig::InteractWith { kind: LookKind::Villager, breed: false }), 2),
                            (o(Trig::InteractWith { kind: LookKind::Cat, breed: false }), 1),
                            (o(Trig::VillageStroll { speed: sp, h: 10, v: 7 }), 1),
                            (o(Trig::WalkFromLook { speed: sp, close: 2 }), 1),
                            (b(Beh::JumpOnBed { speed: sp, bed: None, reach: 0, jumps: 0, cooldown: 0 }), 2),
                            (idle(20, 40), 2),
                        ],
                    ),
                ),
                (99, o(Trig::UpdateSchedule)),
            ],
        );
    } else {
        let farmer = prof == PROF_FARMER;
        add(
            Act::Work,
            vec![
                minimal_look(),
                (
                    5,
                    run_one(vec![
                        (b(Beh::WorkAtPoi { last_check: 0, composter: farmer }), 7),
                        (o(Trig::StrollAroundPoi { mem: PoiMem::JobSite, speed: 0.4, max: 4, next: 0 }), 2),
                        (o(Trig::StrollToPoi { mem: PoiMem::JobSite, speed: 0.4, close: 1, max: 10, next: 0 }), 5),
                        (o(Trig::StrollToSecondary { next: 0 }), 5),
                        (
                            b(Beh::Harvest { pos: None, next_ok: 0, worked: 0, valid: Vec::new() }),
                            if farmer { 2 } else { 5 },
                        ),
                        (
                            b(Beh::Bonemeal { next_cycle: 0, last_session: 0, worked: 0, crop: None }),
                            if farmer { 4 } else { 7 },
                        ),
                    ]),
                ),
                (10, b(Beh::ShowTrades { held: None, display: Vec::new(), cycle: 0, index: 0, look: 0 })),
                (10, o(Trig::SetLookAndInteract)),
                (2, o(Trig::WalkFromBlock { mem: PoiMem::JobSite, speed: sp, close: 9, far: 100, too_long: 1200 })),
                (3, b(Beh::GiveGift { until: 600, given: false, since: 0 })),
                (99, o(Trig::UpdateSchedule)),
            ],
        );
    }
    // Core.
    add(
        Act::Core,
        vec![
            (0, b(Beh::Swim)),
            (0, o(Trig::Door { last: None, cooldown: 0 })),
            (0, b(Beh::LookAtTarget)),
            (0, b(Beh::PanicTrigger)),
            (0, o(Trig::WakeUp)),
            (0, o(Trig::ReactToBell)),
            (0, o(Trig::SetRaidStatus)),
            (0, o(Trig::Validate { mem: PoiMem::JobSite, job: true })),
            (0, o(Trig::Validate { mem: PoiMem::Potential, job: true })),
            (1, move_to(150, 250)),
            (2, o(Trig::CompetitorScan)),
            (3, b(Beh::FollowTrader)),
            (5, o(Trig::GoToWantedItem)),
            (6, acquire(Acq::Job)),
            (7, b(Beh::GoToPotentialJobSite)),
            (8, o(Trig::YieldJobSite)),
            (10, acquire(Acq::Home)),
            (10, acquire(Acq::Meeting)),
            (10, o(Trig::AssignProfession)),
            (10, o(Trig::ResetProfession)),
        ],
    );
    // Meeting.
    add(
        Act::Meet,
        vec![
            (
                2,
                o(Trig::Shuffled {
                    kids: vec![
                        (Trig::StrollAroundPoi { mem: PoiMem::Meeting, speed: 0.4, max: 40, next: 0 }, 2, 0.0),
                        (Trig::SocializeAtBell, 2, 0.0),
                    ],
                    rng: JRand::from_time(0x4D45_4554),
                }),
            ),
            (10, b(Beh::ShowTrades { held: None, display: Vec::new(), cycle: 0, index: 0, look: 0 })),
            (10, o(Trig::SetLookAndInteract)),
            (2, o(Trig::WalkFromBlock { mem: PoiMem::Meeting, speed: sp, close: 6, far: 100, too_long: 200 })),
            (3, b(Beh::GiveGift { until: 600, given: false, since: 0 })),
            (3, o(Trig::Validate { mem: PoiMem::Meeting, job: false })),
            (
                3,
                gate(
                    Vec::new(),
                    vec![M::Interaction],
                    false,
                    vec![(b(Beh::TradeWithVillager { trades: Vec::new() }), 1)],
                ),
            ),
            full_look(),
            (99, o(Trig::UpdateSchedule)),
        ],
    );
    // Rest.
    add(
        Act::Rest,
        vec![
            (2, o(Trig::WalkFromBlock { mem: PoiMem::Home, speed: sp, close: 1, far: 150, too_long: 1200 })),
            (3, o(Trig::Validate { mem: PoiMem::Home, job: false })),
            (3, b(Beh::SleepInBed { next_ok: 0 })),
            (
                5,
                gate(
                    vec![(M::Home, St::Absent)],
                    Vec::new(),
                    true,
                    vec![
                        (o(Trig::ClosestHome { cache: Vec::new(), last: 0 }), 1),
                        (o(Trig::InsideBrownian), 4),
                        (o(Trig::ClosestVillage), 2),
                        (idle(20, 40), 2),
                    ],
                ),
            ),
            minimal_look(),
            (99, o(Trig::UpdateSchedule)),
        ],
    );
    // Idle.
    add(
        Act::Idle,
        vec![
            (
                2,
                run_one(vec![
                    (o(Trig::InteractWith { kind: LookKind::Villager, breed: false }), 2),
                    (o(Trig::InteractWith { kind: LookKind::Villager, breed: true }), 1),
                    (o(Trig::InteractWith { kind: LookKind::Cat, breed: false }), 1),
                    (o(Trig::VillageStroll { speed: sp, h: 10, v: 7 }), 1),
                    (o(Trig::WalkFromLook { speed: sp, close: 2 }), 1),
                    (b(Beh::JumpOnBed { speed: sp, bed: None, reach: 0, jumps: 0, cooldown: 0 }), 1),
                    (idle(30, 60), 1),
                ]),
            ),
            (3, b(Beh::GiveGift { until: 600, given: false, since: 0 })),
            (3, o(Trig::SetLookAndInteract)),
            (3, b(Beh::ShowTrades { held: None, display: Vec::new(), cycle: 0, index: 0, look: 0 })),
            (
                3,
                gate(
                    Vec::new(),
                    vec![M::Interaction],
                    false,
                    vec![(b(Beh::TradeWithVillager { trades: Vec::new() }), 1)],
                ),
            ),
            (3, gate(Vec::new(), vec![M::Breed], false, vec![(b(Beh::MakeLove { birth: 0 }), 1)])),
            full_look(),
            (99, o(Trig::UpdateSchedule)),
        ],
    );
    // Panic.
    add(
        Act::Panic,
        vec![
            (0, o(Trig::CalmDown)),
            (1, o(Trig::AwayFrom { hostile: true })),
            (1, o(Trig::AwayFrom { hostile: false })),
            (3, o(Trig::VillageStroll { speed: sp * 1.5, h: 2, v: 2 })),
            minimal_look(),
        ],
    );
    // Before the raid: bell, run toward the meeting point or fast wandering.
    add(
        Act::PreRaid,
        vec![
            (0, o(Trig::RingBell)),
            (
                0,
                o(Trig::Shuffled {
                    kids: vec![
                        (
                            Trig::WalkFromBlock {
                                mem: PoiMem::Meeting,
                                speed: sp * 1.5,
                                close: 2,
                                far: 150,
                                too_long: 200,
                            },
                            6,
                            0.0,
                        ),
                        (Trig::VillageStroll { speed: sp * 1.5, h: 10, v: 7 }, 2, 0.0),
                    ],
                    rng: JRand::from_time(0x5052_4552),
                }),
            ),
            minimal_look(),
            (99, o(Trig::ResetRaid)),
        ],
    );
    // Raid: after the victory, under the sky or wandering; celebration; during the raid, hiding.
    add(
        Act::Raid,
        vec![
            (
                0,
                o(Trig::IfRaid {
                    victory: true,
                    then: Box::new(Trig::Shuffled {
                        kids: vec![
                            (Trig::MoveToSky { speed: sp }, 5, 0.0),
                            (Trig::VillageStroll { speed: sp * 1.1, h: 10, v: 7 }, 2, 0.0),
                        ],
                        rng: JRand::from_time(0x5241_4944),
                    }),
                }),
            ),
            (0, b(Beh::Celebrate { raid: None })),
            (
                2,
                o(Trig::IfRaid {
                    victory: false,
                    then: Box::new(Trig::LocateHiding { radius: 24, speed: sp * 1.4, close: 1 }),
                }),
            ),
            minimal_look(),
            (99, o(Trig::ResetRaid)),
        ],
    );
    // Hiding: hiding counter, search for a house.
    add(
        Act::Hide,
        vec![
            (0, o(Trig::SetHidden { stay: 15 * 20, close: 3, hidden: 0 })),
            (1, o(Trig::LocateHiding { radius: 32, speed: sp * 1.25, close: 2 })),
            minimal_look(),
        ],
    );
    // Sort: increasing priority, then activity, then insertion order (stable sort).
    v.sort_by_key(|x| (x.0, x.1));
    v
}

fn bp_center(p: [i32; 3]) -> [f64; 3] {
    [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5]
}

/// Distance to the block center strictly lower.
pub fn closer_center(p: [i32; 3], pos: [f64; 3], d: f64) -> bool {
    dist2(bp_center(p), pos) < d * d
}

fn manhattan(a: [i32; 3], b: [i32; 3]) -> i32 {
    (a[0] - b[0]).abs() + (a[1] - b[1]).abs() + (a[2] - b[2]).abs()
}

fn block_of(p: [f64; 3]) -> [i32; 3] {
    [p[0].floor() as i32, p[1].floor() as i32, p[2].floor() as i32]
}

/// The writing of the walk and look memories.
pub fn walk_and_look(b: &mut Brain, t: Track, speed: f32, close: i32) {
    b.mem.look = Some(t);
    b.mem.walk = Some(Walk { target: t, speed, close });
}

/// Profession → held job-site POI.
pub fn job_poi(prof: i32) -> Option<PoiType> {
    let name = super::super::variants::PROFESSIONS.get(prof as usize)?;
    match *name {
        "none" | "nitwit" => None,
        n => PoiType::from_name(n),
    }
}

/// Items requested by the profession (farmer only).
pub fn requested(prof: i32) -> &'static [&'static str] {
    if prof == PROF_FARMER {
        &["minecraft:wheat", "minecraft:wheat_seeds", "minecraft:beetroot_seeds", "minecraft:bone_meal"]
    } else {
        &[]
    }
}

impl Server {
    // ---------- Target tracking ----------

    pub(crate) fn track_pos(&self, t: Track) -> Option<[f64; 3]> {
        match t {
            Track::Block(p) => Some(bp_center(p)),
            Track::Ent(e, eye) => {
                let p = self.ent_pos(e)?;
                Some(if eye { [p[0], self.ent_eye(e)?, p[2]] } else { p })
            }
            Track::Item(id, eye) => {
                let p = self.life.drops.iter().find(|d| d.id == id)?.pos;
                Some(if eye { [p[0], p[1] + f64::from(0.2125f32), p[2]] } else { p })
            }
        }
    }

    pub(crate) fn track_block(&self, t: Track) -> Option<[i32; 3]> {
        match t {
            Track::Block(p) => Some(p),
            Track::Ent(e, _) => self.ent_pos(e).map(block_of),
            Track::Item(id, _) => self.life.drops.iter().find(|d| d.id == id).map(|d| block_of(d.pos)),
        }
    }

    fn track_visible(&self, m: &mut Mob, b: &mut Brain, t: Track) -> bool {
        match t {
            // An item (non-living entity) is always "visible".
            Track::Block(_) | Track::Item(..) => true,
            Track::Ent(e, _) => self.ent_alive(e) && self.visible(m, b, e),
        }
    }

    pub(crate) fn ent_is(&self, e: Ent, k: LookKind) -> bool {
        let Ent::Mob(id) = e else { return k == LookKind::Player };
        let Some(o) = self.mob_ref(id) else { return false };
        match k {
            LookKind::Cat => o.kind == Kind::Cat,
            LookKind::Villager => o.kind == Kind::Villager,
            LookKind::Player => false,
            LookKind::Category(c) => {
                // Full gaze order: creature, aquatic, axolotls, underground aquatic,
                // aquatic ambient, monster.
                use super::super::species::Category as C;
                const ORDER: [C; 6] =
                    [C::Creature, C::WaterCreature, C::Axolotls, C::UndergroundWaterCreature, C::WaterAmbient, C::Monster];
                ORDER.get(usize::from(c)).is_some_and(|&want| o.kind.info().category == want)
            }
        }
    }

    // ---------- Behaviors with a duration ----------

    pub(crate) fn beh_check(&mut self, m: &mut Mob, b: &mut Brain, beh: &mut Beh) -> bool {
        let t = self.time;
        match beh {
            Beh::Swim => self.swim_ok(m),
            Beh::LookAtTarget => true,
            Beh::PanicTrigger => true,
            Beh::MoveToTarget { cooldown, path, last, speed, .. } => {
                if *cooldown > 0 {
                    *cooldown -= 1;
                    return false;
                }
                let Some(w) = b.mem.walk else { return false };
                let reached = self.walk_reached(m, w);
                if !reached && self.compute_walk_path(m, b, w, t, path, speed) {
                    *last = self.track_block(w.target);
                    return true;
                }
                b.mem.walk = None;
                if reached {
                    b.mem.cant_reach_since = None;
                }
                false
            }
            Beh::FollowTrader => self.follow_trader_ok(m),
            Beh::GoToPotentialJobSite => matches!(b.active, Act::Idle | Act::Work | Act::Play),
            Beh::WorkAtPoi { last_check, .. } => {
                if t - *last_check < 300 {
                    return false;
                }
                if self.mobs.rng.next_int_bound(2) != 0 {
                    return false;
                }
                *last_check = t;
                self.here(b.mem.job_site).is_some_and(|p| closer_center(p, m.pos, 1.73))
            }
            Beh::Harvest { pos, valid, .. } => {
                if !self.rule("minecraft:mob_griefing", true) || m.villager[1] != PROF_FARMER {
                    return false;
                }
                valid.clear();
                for x in -1..=1 {
                    for y in -1..=1 {
                        for z in -1..=1 {
                            let p =
                                block_of([m.pos[0] + f64::from(x), m.pos[1] + f64::from(y), m.pos[2] + f64::from(z)]);
                            if self.harvest_valid(p) {
                                valid.push(p);
                            }
                        }
                    }
                }
                *pos = if valid.is_empty() {
                    None
                } else {
                    Some(valid[self.mobs.rng.next_int_bound(valid.len() as i32) as usize])
                };
                pos.is_some()
            }
            Beh::Bonemeal { last_session, crop, .. } => {
                if m.tick_count % 10 == 0 && (*last_session == 0 || *last_session + 160 <= m.tick_count) {
                    if self.npc_count(m, "minecraft:bone_meal") <= 0 {
                        return false;
                    }
                    *crop = self.bonemeal_target(m);
                    crop.is_some()
                } else {
                    false
                }
            }
            Beh::ShowTrades { .. } => self.show_trades_ok(m, b),
            Beh::GiveGift { until, .. } => {
                if !self.hero_visible(b) {
                    false
                } else if *until > 0 {
                    *until -= 1;
                    false
                } else {
                    true
                }
            }
            Beh::JumpOnBed { .. } => m.is_baby() && (self.on_or_over_bed(m) || b.mem.nearest_bed.is_some()),
            Beh::SleepInBed { .. } => {
                let Some(home) = self.here(b.mem.home) else { return false };
                if let Some(w) = b.mem.last_woken {
                    let since = t - w;
                    if since > 0 && since < 100 {
                        return false;
                    }
                }
                let s = self.st(home);
                closer_center(home, m.pos, 2.0)
                    && self.btag_id(home, crate::tag_bits::TagId::VillagersCanSleepOnBed)
                    && !self.v().get_bool(s, "occupied")
            }
            Beh::TradeWithVillager { .. } => self.target_valid(m, b, b.mem.interaction),
            Beh::MakeLove { .. } => self.breeding_possible(m, b),
            Beh::Celebrate { raid } => {
                let here = m.block_pos();
                *raid = self.raid_at(here);
                raid.and_then(|id| self.raid_ref(id)).is_some_and(|r| r.is_victory()) && self.no_blocks_above(m, here)
            }
        }
    }

    pub(crate) fn beh_start(&mut self, m: &mut Mob, b: &mut Brain, beh: &mut Beh, t: i64) {
        match beh {
            Beh::PanicTrigger => {
                if b.mem.hurt_by || b.mem.hostile.is_some() {
                    if !b.is_active(Act::Panic) {
                        b.mem.path = false;
                        m.nav.stop();
                        b.mem.walk = None;
                        b.mem.look = None;
                        b.mem.breed = None;
                        b.mem.interaction = None;
                    }
                    b.set_active_if_possible(Act::Panic);
                }
            }
            Beh::MoveToTarget { speed, path, .. } => {
                b.mem.path = path.is_some();
                let p = path.as_deref().cloned();
                self.move_to_path(m, p, f64::from(*speed));
            }
            Beh::FollowTrader => self.follow_trader(b),
            Beh::WorkAtPoi { composter, .. } => {
                b.mem.last_worked = Some(t);
                if let Some(g) = b.mem.job_site {
                    b.mem.look = Some(Track::Block(g.pos));
                }
                self.play_work_sound(m);
                if *composter
                    && let Some(p) = b.mem.job_site.map(|g| g.pos)
                    && self.bname(p) == "minecraft:composter"
                {
                    self.make_bread(m);
                    self.compost(m, p);
                }
                if self.should_restock(m) {
                    self.restock(m);
                }
            }
            Beh::Harvest { pos, next_ok, .. } => {
                if t > *next_ok
                    && let Some(p) = *pos
                {
                    b.mem.look = Some(Track::Block(p));
                    b.mem.walk = Some(Walk { target: Track::Block(p), speed: 0.5, close: 1 });
                }
            }
            Beh::Bonemeal { next_cycle, worked, crop, .. } => {
                if let Some(p) = *crop {
                    walk_and_look(b, Track::Block(p), 0.5, 1);
                }
                m.equipment[0] = Some(Stack { id: "minecraft:bone_meal".into(), count: 1, components: None });
                self.send_main_hand(m);
                *next_cycle = t;
                *worked = 0;
            }
            Beh::ShowTrades { held, display, cycle, index, look } => {
                if let Some(e) = b.mem.interaction {
                    b.mem.look = Some(Track::Ent(e, true));
                }
                *held = None;
                display.clear();
                *cycle = 0;
                *index = 0;
                *look = 40;
            }
            Beh::GiveGift { given, since, .. } => {
                *given = false;
                *since = t;
                b.mem.interaction = Some(Ent::Player);
                b.mem.look = Some(Track::Ent(Ent::Player, true));
            }
            Beh::JumpOnBed { speed, bed, reach, jumps, cooldown } => {
                if let Some(p) = b.mem.nearest_bed {
                    *bed = Some(p);
                    *reach = 100;
                    *jumps = 3 + self.mobs.rng.next_int_bound(4);
                    *cooldown = 0;
                    b.mem.walk = Some(Walk { target: Track::Block(p), speed: *speed, close: 0 });
                }
            }
            Beh::SleepInBed { next_ok } => {
                if t > *next_ok {
                    if b.mem.doors.is_some() {
                        self.close_doors(m, b, None, None);
                    }
                    if let Some(home) = b.mem.home.map(|g| g.pos)
                        && self.start_sleeping(m, home)
                    {
                        b.mem.last_slept = Some(t);
                    }
                    b.mem.walk = None;
                    b.mem.cant_reach_since = None;
                }
            }
            Beh::TradeWithVillager { trades } => {
                if let Some(Ent::Mob(id)) = b.mem.interaction {
                    self.lock_gaze(m, b, id, 0.5, 2);
                    let theirs = self.mob_ref(id).map_or(PROF_NONE, |o| o.villager[1]);
                    let mine = requested(m.villager[1]);
                    *trades = requested(theirs).iter().copied().filter(|i| !mine.contains(i)).collect();
                }
            }
            Beh::MakeLove { birth } => {
                if let Some(Ent::Mob(id)) = b.mem.breed {
                    self.lock_gaze(m, b, id, 0.5, 2);
                    if let Some(o) = self.mob_ref(id) {
                        self.mob_event(o, 18);
                    }
                    self.mob_event(m, 18);
                    let d = 275 + m.rng.next_int_bound(50);
                    *birth = t + i64::from(d);
                }
            }
            _ => {}
        }
    }

    pub(crate) fn beh_continue(&mut self, m: &mut Mob, b: &mut Brain, beh: &mut Beh, t: i64) -> bool {
        match beh {
            Beh::Swim => self.swim_ok(m),
            Beh::LookAtTarget => b.mem.look.is_some_and(|l| self.track_visible(m, b, l)),
            Beh::PanicTrigger => b.mem.hurt_by || b.mem.hostile.is_some(),
            Beh::MoveToTarget { path, last, .. } => {
                if path.is_none() || last.is_none() {
                    return false;
                }
                let Some(w) = b.mem.walk else { return false };
                let spectator = matches!(w.target, Track::Ent(Ent::Player, _)) && self.game_mode == 3;
                !m.nav.is_done() && !self.walk_reached(m, w) && !spectator
            }
            Beh::FollowTrader => self.follow_trader_ok(m),
            Beh::GoToPotentialJobSite => b.mem.potential_job_site.is_some(),
            Beh::WorkAtPoi { .. } => self.here(b.mem.job_site).is_some_and(|p| closer_center(p, m.pos, 1.73)),
            Beh::Harvest { worked, .. } => *worked < 200,
            Beh::Bonemeal { worked, crop, .. } => *worked < 80 && crop.is_some(),
            Beh::ShowTrades { look, .. } => self.show_trades_ok(m, b) && *look > 0 && b.mem.interaction.is_some(),
            Beh::GiveGift { given, .. } => self.hero_visible(b) && !*given,
            Beh::JumpOnBed { bed, reach, jumps, .. } => {
                let over = self.on_or_over_bed(m);
                m.is_baby()
                    && bed.is_some_and(|p| self.btag_id(p, crate::tag_bits::TagId::VillagerBabiesCanJumpOnBed))
                    && !(!over && *reach <= 0)
                    && !(over && *jumps <= 0)
            }
            Beh::SleepInBed { .. } => {
                let Some(home) = b.mem.home.map(|g| g.pos) else { return false };
                b.is_active(Act::Rest) && m.pos[1] > f64::from(home[1]) + 0.4 && closer_center(home, m.pos, 1.14)
            }
            Beh::TradeWithVillager { .. } => self.target_valid(m, b, b.mem.interaction),
            Beh::MakeLove { birth } => t <= *birth && self.breeding_possible(m, b),
            Beh::Celebrate { raid } => raid.and_then(|id| self.raid_ref(id)).is_some_and(|r| !r.is_stopped()),
        }
    }

    pub(crate) fn beh_tick(&mut self, m: &mut Mob, b: &mut Brain, beh: &mut Beh, t: i64) {
        match beh {
            Beh::Swim => {
                if m.rng.next_float() < 0.8 {
                    m.jump_ctl = true;
                }
            }
            Beh::LookAtTarget => {
                if let Some(p) = b.mem.look.and_then(|l| self.track_pos(l)) {
                    self.look_at(m, p, 10.0, 40.0);
                }
            }
            Beh::PanicTrigger => {
                if t % 100 == 0 {
                    self.spawn_golem_if_needed(m, b, t, 3);
                }
            }
            Beh::MoveToTarget { path, last, speed, .. } => {
                // The followed path is the navigation's; only its presence matters here (the
                // kept path is only read again after a new computation).
                let has = m.nav.path.is_some();
                b.mem.path = has;
                if !has {
                    *path = None;
                }
                if has
                    && let (Some(l), Some(w)) = (*last, b.mem.walk)
                    && let Some(cur) = self.track_block(w.target)
                {
                    let d = [cur[0] - l[0], cur[1] - l[1], cur[2] - l[2]];
                    let d2 = i64::from(d[0]).pow(2) + i64::from(d[1]).pow(2) + i64::from(d[2]).pow(2);
                    if d2 > 4 && self.compute_walk_path(m, b, w, t, path, speed) {
                        *last = Some(cur);
                        b.mem.path = path.is_some();
                        let p = path.as_deref().cloned();
                        self.move_to_path(m, p, f64::from(*speed));
                    }
                }
            }
            Beh::FollowTrader => self.follow_trader(b),
            Beh::GoToPotentialJobSite => {
                if let Some(g) = b.mem.potential_job_site {
                    walk_and_look(b, Track::Block(g.pos), 0.5, 1);
                }
            }
            Beh::Harvest { pos, next_ok, worked, valid } => self.harvest_tick(m, b, pos, next_ok, worked, valid, t),
            Beh::Bonemeal { next_cycle, worked, crop, .. } => {
                let Some(p) = *crop else { return };
                if t >= *next_cycle && closer_center(p, m.pos, 1.0) {
                    let has = self.npc_count(m, "minecraft:bone_meal") > 0;
                    if has && self.grow_crop(p) {
                        self.npc_remove(m, "minecraft:bone_meal", 1);
                        self.send_level_event(1505, p, 15);
                        *crop = self.bonemeal_target(m);
                        if let Some(c) = *crop {
                            walk_and_look(b, Track::Block(c), 0.5, 1);
                        }
                        *next_cycle = t + 40;
                    }
                    *worked += 1;
                }
            }
            Beh::ShowTrades { held, display, cycle, index, look } => {
                let Some(target) = b.mem.interaction else { return };
                b.mem.look = Some(Track::Ent(target, true));
                let item = self.held_main_hand();
                let changed = match held {
                    None => true,
                    Some(h) => h.as_ref().map(|s| s.id.as_str()) != item.as_ref().map(|s| s.id.as_str()),
                };
                if changed {
                    *held = Some(item.clone());
                    display.clear();
                    if let Some(it) = &item {
                        for off in self.npc_offers(m) {
                            if !off.out_of_stock()
                                && (off.cost_a.id == it.id || off.cost_b.as_ref().is_some_and(|c| c.id == it.id))
                            {
                                display.push(off.result.clone());
                            }
                        }
                        if !display.is_empty() {
                            *look = 900;
                            self.display_held(m, Some(display[0].clone()));
                        }
                    }
                }
                if !display.is_empty() {
                    if display.len() >= 2 {
                        *cycle += 1;
                        if *cycle >= 40 {
                            *index += 1;
                            *cycle = 0;
                            if *index > display.len() - 1 {
                                *index = 0;
                            }
                            self.display_held(m, Some(display[*index].clone()));
                        }
                    }
                } else {
                    self.display_held(m, None);
                    *look = (*look).min(40);
                }
                *look -= 1;
            }
            Beh::GiveGift { given, since, .. } => {
                b.mem.look = Some(Track::Ent(Ent::Player, true));
                let pb = block_of(self.pos);
                let vb = m.block_pos();
                let d = [pb[0] - vb[0], pb[1] - vb[1], pb[2] - vb[2]];
                if i64::from(d[0]).pow(2) + i64::from(d[1]).pow(2) + i64::from(d[2]).pow(2) < 25 {
                    if t - *since > 20 {
                        self.throw_gift(m);
                        *given = true;
                    }
                } else {
                    walk_and_look(b, Track::Ent(Ent::Player, true), 0.5, 5);
                }
            }
            Beh::JumpOnBed { reach, jumps, cooldown, .. } => {
                if !self.on_or_over_bed(m) {
                    *reach -= 1;
                } else if *cooldown > 0 {
                    *cooldown -= 1;
                } else if self.btag_id(m.block_pos(), crate::tag_bits::TagId::VillagerBabiesCanJumpOnBed) {
                    m.jump_ctl = true;
                    *jumps -= 1;
                    *cooldown = 5;
                }
            }
            Beh::TradeWithVillager { trades } => {
                let Some(Ent::Mob(id)) = b.mem.interaction else { return };
                let Some(tp) = self.mob_ref(id).map(|o| o.pos) else { return };
                if dist2(m.pos, tp) > 5.0 {
                    return;
                }
                self.lock_gaze(m, b, id, 0.5, 2);
                self.gossip_with(m, id, t);
                let farmer = m.villager[1] == PROF_FARMER;
                let wants_food = self.other_wants_food(id);
                if self.npc_food_points(m) >= 24 && (farmer || wants_food) {
                    self.throw_half_stack(m, tp, |s: &Server, st: &Stack| s.villager_food(st).is_some());
                }
                if farmer && self.npc_count(m, "minecraft:wheat") > 32 {
                    self.throw_half_stack(m, tp, |_, st: &Stack| st.id == "minecraft:wheat");
                }
                let list = trades.clone();
                if !list.is_empty() && list.iter().any(|i| self.npc_count(m, i) > 0) {
                    self.throw_half_stack(m, tp, move |_, st: &Stack| list.contains(&st.id.as_str()));
                }
            }
            Beh::MakeLove { birth } => {
                let Some(Ent::Mob(id)) = b.mem.breed else { return };
                let Some(tp) = self.mob_ref(id).map(|o| o.pos) else { return };
                if dist2(m.pos, tp) > 5.0 {
                    return;
                }
                self.lock_gaze(m, b, id, 0.5, 2);
                if t >= *birth {
                    self.eat_and_digest(m);
                    self.with_villager(id, |s, o, _| s.eat_and_digest(o));
                    self.try_give_birth(m, id);
                } else if m.rng.next_int_bound(35) == 0 {
                    if let Some(o) = self.mob_ref(id) {
                        self.mob_event(o, 12);
                    }
                    self.mob_event(m, 12);
                }
            }
            Beh::Celebrate { .. } => self.celebrate_tick(m),
            _ => {}
        }
    }

    pub(crate) fn beh_stop(&mut self, m: &mut Mob, b: &mut Brain, beh: &mut Beh, t: i64) {
        match beh {
            Beh::LookAtTarget => b.mem.look = None,
            Beh::MoveToTarget { cooldown, path, .. } => {
                if let Some(w) = b.mem.walk
                    && !self.walk_reached(m, w)
                    && m.nav.stuck
                {
                    *cooldown = self.mobs.rng.next_int_bound(40);
                }
                m.nav.stop();
                b.mem.walk = None;
                b.mem.path = false;
                *path = None;
            }
            Beh::FollowTrader => {
                b.mem.walk = None;
                b.mem.look = None;
            }
            Beh::GoToPotentialJobSite => {
                // Released in the location's dimension: another dimension is not reachable here.
                if let Some(p) = self.here(b.mem.potential_job_site.take())
                    && self.poi_type(p).is_some()
                {
                    self.poi_release(p);
                }
            }
            Beh::Harvest { worked, next_ok, .. } => {
                b.mem.look = None;
                b.mem.walk = None;
                *worked = 0;
                *next_ok = t + 40;
            }
            Beh::Bonemeal { last_session, .. } => {
                m.equipment[0] = None;
                self.send_main_hand(m);
                *last_session = m.tick_count;
            }
            Beh::ShowTrades { held, .. } => {
                b.mem.interaction = None;
                self.display_held(m, None);
                *held = None;
            }
            Beh::GiveGift { until, .. } => {
                *until = 600 + self.mobs.rng.next_int_bound(6001);
                b.mem.interaction = None;
                b.mem.walk = None;
                b.mem.look = None;
            }
            Beh::JumpOnBed { bed, reach, jumps, cooldown, .. } => {
                *bed = None;
                *reach = 0;
                *jumps = 0;
                *cooldown = 0;
            }
            Beh::SleepInBed { next_ok } => {
                if self.npc_sleeping(m).is_some() {
                    self.stop_sleeping(m, b);
                    *next_ok = t + 40;
                }
            }
            Beh::TradeWithVillager { .. } => b.mem.interaction = None,
            Beh::MakeLove { .. } => b.mem.breed = None,
            Beh::Celebrate { raid } => {
                *raid = None;
                let day = self.day_time();
                b.update_schedule(t, day);
            }
            _ => {}
        }
    }

    // ---------- Triggers ----------

    pub(crate) fn trig(&mut self, m: &mut Mob, b: &mut Brain, tr: &mut Trig, t: i64) -> bool {
        match tr {
            Trig::Door { last, cooldown } => self.door_trigger(m, b, last, cooldown),
            Trig::WakeUp => {
                if !b.is_active(Act::Rest) && self.npc_sleeping(m).is_some() {
                    self.stop_sleeping(m, b);
                    true
                } else {
                    false
                }
            }
            Trig::ReactToBell => {
                if b.mem.heard_bell.is_some() {
                    // Hiding place only outside a raid (the raid has its own activities).
                    if self.raid_at(m.block_pos()).is_none() {
                        b.set_active_if_possible(Act::Hide);
                    }
                    true
                } else {
                    false
                }
            }
            Trig::SetRaidStatus => {
                if self.mobs.rng.next_int_bound(20) != 0 {
                    return false;
                }
                if let Some(r) = self.raid_at(m.block_pos()).and_then(|id| self.raid_ref(id)) {
                    let a = if r.first_wave_spawned() && !r.between_waves() { Act::Raid } else { Act::PreRaid };
                    b.default = a;
                    b.set_active_if_possible(a);
                }
                true
            }
            Trig::RingBell => {
                let Some(meet) = b.mem.meeting_point else { return false };
                if self.mobs.rng.next_float() <= 0.95 {
                    return false;
                }
                let p = meet.pos;
                let me = m.block_pos();
                let d2: i32 = (0..3).map(|k| (p[k] - me[k]) * (p[k] - me[k])).sum();
                if meet.dim == self.dim_key() && d2 < 9 && self.bname(p) == "minecraft:bell" {
                    self.villager_ring_bell(m, p);
                    if closer_center(p, m.pos, 32.0) {
                        b.mem.heard_bell = Some(self.time);
                    }
                }
                true
            }
            Trig::ResetRaid => {
                if self.mobs.rng.next_int_bound(20) != 0 {
                    return false;
                }
                let r = self.raid_at(m.block_pos()).and_then(|id| self.raid_ref(id));
                if r.is_none_or(|r| r.is_stopped() || r.is_loss()) {
                    b.default = Act::Idle;
                    let day = self.day_time();
                    b.update_schedule(t, day);
                }
                true
            }
            Trig::IfRaid { victory, then } => {
                let r = self.raid_at(m.block_pos()).and_then(|id| self.raid_ref(id));
                let ok = if *victory {
                    r.is_some_and(|r| r.is_victory())
                } else {
                    r.is_some_and(|r| r.active && !r.is_victory() && !r.is_loss())
                };
                ok && self.trig(m, b, then, t)
            }
            Trig::MoveToSky { speed } => {
                if b.mem.walk.is_some() {
                    return false;
                }
                let here = m.block_pos();
                if self.can_see_sky(here) {
                    return false;
                }
                for _ in 0..10 {
                    let p = [
                        here[0] + m.rng.next_int_bound(20) - 10,
                        here[1] + m.rng.next_int_bound(6) - 3,
                        here[2] + m.rng.next_int_bound(20) - 10,
                    ];
                    if self.no_blocks_above(m, p) {
                        b.mem.walk = Some(Walk { target: Track::Block(p), speed: *speed, close: 0 });
                        break;
                    }
                }
                true
            }
            Trig::LocateHiding { radius, speed, close } => {
                if b.mem.walk.is_some() {
                    return false;
                }
                let (radius, speed, close) = (*radius, *speed, *close);
                let here = m.block_pos();
                let home = |t: PoiType| t == PoiType::Home;
                let near = self
                    .poi_in_range(home, here, close + 1, Occupancy::Any)
                    .first()
                    .map(|r| r.pos)
                    .filter(|&p| closer_center(p, m.pos, f64::from(close)));
                let found = near
                    .or_else(|| {
                        let mut list: Vec<[i32; 3]> =
                            self.poi_in_range(home, here, radius, Occupancy::Any).into_iter().map(|r| r.pos).collect();
                        m.rng.shuffle(&mut list);
                        list.first().copied()
                    })
                    .or_else(|| self.here(b.mem.home));
                if let Some(p) = found {
                    b.mem.path = false;
                    b.mem.look = None;
                    b.mem.breed = None;
                    b.mem.interaction = None;
                    b.mem.hiding_place = Some(p);
                    if !closer_center(p, m.pos, f64::from(close)) {
                        b.mem.walk = Some(Walk { target: Track::Block(p), speed, close });
                    }
                }
                true
            }
            Trig::SetHidden { stay, close, hidden } => {
                let (Some(hp), Some(heard)) = (b.mem.hiding_place, b.mem.heard_bell) else { return false };
                let timed_out = heard + 300 <= t;
                if *hidden <= *stay && !timed_out {
                    let me = m.block_pos();
                    let d2: i32 = (0..3).map(|k| (hp[k] - me[k]) * (hp[k] - me[k])).sum();
                    if d2 < *close * *close {
                        *hidden += 1;
                    }
                } else {
                    b.mem.heard_bell = None;
                    b.mem.hiding_place = None;
                    let day = self.day_time();
                    b.update_schedule(t, day);
                    *hidden = 0;
                }
                true
            }
            Trig::Validate { mem, job } => {
                let (mem, job) = (*mem, *job);
                let Some(p) = self.here(mem.get(b)) else { return false };
                if !closer_center(p, m.pos, 16.0) {
                    return false;
                }
                let ty = self.poi_type(p);
                let ok = match (mem, ty) {
                    (_, None) => false,
                    (PoiMem::Home, Some(t)) => t == PoiType::Home,
                    (PoiMem::Meeting, Some(t)) => t == PoiType::Meeting,
                    (PoiMem::JobSite, Some(t)) if job => job_poi(m.villager[1]) == Some(t),
                    (PoiMem::Potential, Some(t)) => {
                        if m.villager[1] == PROF_NONE {
                            t.is_job()
                        } else {
                            job_poi(m.villager[1]) == Some(t)
                        }
                    }
                    _ => false,
                };
                if !ok {
                    mem.set(b, None);
                } else if self.bed_occupied(p) && self.npc_sleeping(m).is_none() {
                    mem.set(b, None);
                    if !self.villager_sleeping_at(p) {
                        self.poi_release(p);
                    }
                }
                true
            }
            Trig::CompetitorScan => self.competitor_scan(m, b),
            Trig::GoToWantedItem => {
                if b.mem.walk.is_some() {
                    return false;
                }
                let Some(id) = b.mem.wanted_item else { return false };
                let Some(d) = self.life.drops.iter().find(|d| d.id == id) else { return false };
                let p = d.pos;
                // Followed item (gaze at its eyes), within 4 blocks and inside the world border.
                if dist2(p, m.pos) < 16.0 && self.may_interact(block_of(p)) && m.can_pickup {
                    b.mem.look = Some(Track::Item(id, true));
                    b.mem.walk = Some(Walk { target: Track::Item(id, false), speed: 0.5, close: 0 });
                    true
                } else {
                    false
                }
            }
            Trig::Acquire { kind, next, cache } => self.acquire_poi(m, b, *kind, next, cache, t),
            Trig::YieldJobSite => self.yield_job_site(m, b),
            Trig::AssignProfession => {
                let Some(g) = b.mem.potential_job_site else { return false };
                if !closer_center(g.pos, m.pos, 2.0) {
                    return false;
                }
                b.mem.potential_job_site = None;
                b.mem.job_site = Some(g);
                self.mob_event(m, 14);
                if m.villager[1] != PROF_NONE {
                    return true;
                }
                // Type read in the location's dimension (another dimension is not reachable here).
                if let Some(p) = self.here(Some(g))
                    && let Some(ty) = self.poi_type(p)
                {
                    let prof = (0..super::super::variants::PROFESSIONS.len() as i32).find(|&i| job_poi(i) == Some(ty));
                    if let Some(prof) = prof {
                        self.set_profession(m, prof);
                        b.refresh = true;
                    }
                }
                true
            }
            Trig::ResetProfession => {
                if b.mem.job_site.is_some() {
                    return false;
                }
                let prof = m.villager[1];
                let firable = prof != PROF_NONE && prof != PROF_NITWIT;
                if firable && m.x.villager_xp == 0 && m.villager[2] <= 1 {
                    self.set_profession(m, PROF_NONE);
                    b.refresh = true;
                    true
                } else {
                    false
                }
            }
            Trig::StrollAroundPoi { mem, speed, max, next } => {
                let Some(p) = self.here(mem.get(b)) else { return false };
                if !closer_center(p, m.pos, f64::from(*max)) {
                    return false;
                }
                if t <= *next {
                    return true;
                }
                let pos = self.rpos_land(m, None, 8, 6, &|_, _| 0.0);
                b.mem.walk = pos.map(|p| Walk { target: Track::Block(block_of(p)), speed: *speed, close: 1 });
                *next = t + 180;
                true
            }
            Trig::StrollToPoi { mem, speed, close, max, next } => {
                let Some(p) = self.here(mem.get(b)) else { return false };
                if !closer_center(p, m.pos, f64::from(*max)) {
                    return false;
                }
                if t <= *next {
                    return true;
                }
                b.mem.walk = Some(Walk { target: Track::Block(p), speed: *speed, close: *close });
                *next = t + 80;
                true
            }
            Trig::StrollToSecondary { next } => {
                let (Some(list), Some(job)) = (b.mem.secondary.clone(), b.mem.job_site) else { return false };
                if list.is_empty() {
                    return false;
                }
                let target = list[self.mobs.rng.next_int_bound(list.len() as i32) as usize];
                if closer_center(job.pos, m.pos, 6.0) {
                    if t > *next {
                        b.mem.walk = Some(Walk { target: Track::Block(target), speed: 0.5, close: 1 });
                        *next = t + 100;
                    }
                    true
                } else {
                    false
                }
            }
            Trig::SetLookAndInteract => {
                if b.mem.interaction.is_some() || b.mem.visible.is_none() {
                    return false;
                }
                let me = m.pos;
                let found = self
                    .find_closest(m, b, |s, e| e == Ent::Player && s.ent_pos(e).is_some_and(|p| dist2(p, me) <= 16.0));
                match found {
                    Some(e) => {
                        b.mem.interaction = Some(e);
                        b.mem.look = Some(Track::Ent(e, true));
                        true
                    }
                    None => false,
                }
            }
            Trig::WalkFromBlock { mem, speed, close, far, too_long } => {
                if b.mem.walk.is_some() {
                    return false;
                }
                let Some(g) = mem.get(b) else { return false };
                let unreachable = b.mem.cant_reach_since.is_some_and(|c| t - c > *too_long);
                // Location of another dimension: released and forgotten, like an unreachable location.
                let target = g.pos;
                if g.dim != self.dim_key() || unreachable {
                    self.release_poi_mem(m, b, *mem);
                    mem.set(b, None);
                    b.mem.cant_reach_since = Some(t);
                    return true;
                }
                let here = m.block_pos();
                if manhattan(target, here) > *far {
                    let mut toward: Option<[f64; 3]> = None;
                    let mut tries = 0;
                    while toward.is_none_or(|p| manhattan(block_of(p), here) > *far) {
                        let to = [f64::from(target[0]) + 0.5, f64::from(target[1]), f64::from(target[2]) + 0.5];
                        toward = self.rpos_default_towards(m, None, 15, 7, to, f64::from(std::f32::consts::FRAC_PI_2));
                        tries += 1;
                        if tries == 1000 {
                            self.release_poi_mem(m, b, *mem);
                            mem.set(b, None);
                            b.mem.cant_reach_since = Some(t);
                            return true;
                        }
                    }
                    if let Some(p) = toward {
                        b.mem.walk = Some(Walk { target: Track::Block(block_of(p)), speed: *speed, close: *close });
                    }
                } else if manhattan(target, here) > *close {
                    b.mem.walk = Some(Walk { target: Track::Block(target), speed: *speed, close: *close });
                }
                true
            }
            Trig::UpdateSchedule => {
                let day = self.day_time();
                b.update_schedule(t, day);
                true
            }
            Trig::Look(kind) => {
                if b.mem.look.is_some() || b.mem.visible.is_none() {
                    return false;
                }
                let me = m.pos;
                let k = *kind;
                let found = self
                    .find_closest(m, b, |s, e| s.ent_is(e, k) && s.ent_pos(e).is_some_and(|p| dist2(p, me) <= 64.0));
                match found {
                    Some(e) => {
                        b.mem.look = Some(Track::Ent(e, true));
                        true
                    }
                    None => false,
                }
            }
            Trig::InteractWith { kind, breed } => {
                if b.mem.walk.is_some() || b.mem.visible.is_none() {
                    return false;
                }
                let k = *kind;
                let breed = *breed;
                if breed && !self.can_breed(m) {
                    return false;
                }
                let valid = |s: &Server, e: Ent| s.ent_is(e, k) && (!breed || s.ent_can_breed(e));
                if self.find_closest(m, b, valid).is_none() {
                    return false;
                }
                let me = m.pos;
                let found =
                    self.find_closest(m, b, |s, e| s.ent_pos(e).is_some_and(|p| dist2(p, me) <= 64.0) && valid(s, e));
                if let Some(e) = found {
                    if breed {
                        b.mem.breed = Some(e);
                    } else {
                        b.mem.interaction = Some(e);
                    }
                    b.mem.look = Some(Track::Ent(e, true));
                    b.mem.walk = Some(Walk { target: Track::Ent(e, false), speed: 0.5, close: 2 });
                }
                true
            }
            Trig::VillageStroll { speed, h, v } => {
                if b.mem.walk.is_some() {
                    return false;
                }
                let here = m.block_pos();
                let pos = if self.is_village(here) {
                    self.rpos_land(m, None, *h, *v, &|_, _| 0.0)
                } else {
                    let sec = [here[0] >> 4, here[1] >> 4, here[2] >> 4];
                    let best = self.section_closest_to_village(sec, 2);
                    if best != sec {
                        let c = [
                            f64::from(best[0] * 16 + 8) + 0.5,
                            f64::from(best[1] * 16 + 8),
                            f64::from(best[2] * 16 + 8) + 0.5,
                        ];
                        self.rpos_default_towards(m, None, *h, *v, c, f64::from(std::f32::consts::FRAC_PI_2))
                    } else {
                        self.rpos_land(m, None, *h, *v, &|_, _| 0.0)
                    }
                };
                b.mem.walk = pos.map(|p| Walk { target: Track::Block(block_of(p)), speed: *speed, close: 0 });
                true
            }
            Trig::WalkFromLook { speed, close } => {
                if b.mem.walk.is_some() {
                    return false;
                }
                let Some(l) = b.mem.look else { return false };
                b.mem.walk = Some(Walk { target: l, speed: *speed, close: *close });
                true
            }
            Trig::PlayTag => self.play_tag(m, b),
            Trig::ClosestHome { cache, last } => self.closest_home(m, b, cache, last, t),
            Trig::InsideBrownian => {
                if b.mem.walk.is_some() {
                    return false;
                }
                let here = m.block_pos();
                if self.can_see_sky(here) {
                    return false;
                }
                let mut poses = Vec::new();
                for x in -1..=1 {
                    for y in -1..=1 {
                        for z in -1..=1 {
                            poses.push([here[0] + x, here[1] + y, here[2] + z]);
                        }
                    }
                }
                // List shuffle of the game: non-reproducible global random, clock random here.
                let mut r = JRand::from_time(t ^ i64::from(m.id));
                for i in (1..poses.len()).rev() {
                    let j = r.next_int_bound(i as i32 + 1) as usize;
                    poses.swap(i, j);
                }
                let found = poses.into_iter().find(|&p| !self.can_see_sky(p) && self.can_stand_on(m, p));
                if let Some(p) = found {
                    b.mem.walk = Some(Walk { target: Track::Block(p), speed: 0.5, close: 0 });
                }
                true
            }
            Trig::ClosestVillage => {
                if b.mem.walk.is_some() {
                    return false;
                }
                let here = m.block_pos();
                if self.is_village(here) {
                    return false;
                }
                let cur = self.sections_to_village(here);
                let mut target = None;
                for _ in 0..5 {
                    let pos = self.rpos_land(m, None, 15, 7, &|s, p| -f64::from(s.sections_to_village_ro(p)));
                    if let Some(p) = pos {
                        let d = self.sections_to_village(block_of(p));
                        if d < cur {
                            target = Some(p);
                            break;
                        }
                        if d == cur {
                            target = Some(p);
                        }
                    }
                }
                if let Some(p) = target {
                    b.mem.walk = Some(Walk { target: Track::Block(block_of(p)), speed: 0.5, close: 4 });
                }
                true
            }
            Trig::SocializeAtBell => {
                if b.mem.interaction.is_some() || b.mem.visible.is_none() {
                    return false;
                }
                let Some(meet) = b.mem.meeting_point else { return false };
                if self.mobs.rng.next_int_bound(100) != 0
                    || meet.dim != self.dim_key()
                    || !closer_center(meet.pos, m.pos, 4.0)
                {
                    return false;
                }
                if self.find_closest(m, b, |s, e| s.ent_is(e, LookKind::Villager)).is_none() {
                    return false;
                }
                let me = m.pos;
                let found = self.find_closest(m, b, |s, e| {
                    s.ent_is(e, LookKind::Villager) && s.ent_pos(e).is_some_and(|p| dist2(p, me) <= 32.0)
                });
                if let Some(e) = found {
                    b.mem.interaction = Some(e);
                    b.mem.look = Some(Track::Ent(e, true));
                    b.mem.walk = Some(Walk { target: Track::Ent(e, false), speed: 0.3, close: 1 });
                }
                true
            }
            Trig::CalmDown => {
                let near = b.mem.hurt_by_entity.and_then(|e| self.ent_pos(e)).is_some_and(|p| dist2(p, m.pos) <= 36.0);
                if !(b.mem.hurt_by || b.mem.hostile.is_some() || near) {
                    b.mem.hurt_by = false;
                    b.mem.hurt_by_entity = None;
                    let day = self.day_time();
                    b.update_schedule(t, day);
                }
                true
            }
            Trig::AwayFrom { hostile } => {
                let from = if *hostile { b.mem.hostile } else { b.mem.hurt_by_entity };
                let Some(e) = from else { return false };
                if b.mem.walk.is_some() {
                    return false;
                }
                let Some(avoid) = self.ent_pos(e) else { return false };
                if dist2(m.pos, avoid) >= 36.0 {
                    return false;
                }
                for _ in 0..10 {
                    if let Some(p) = self.rpos_land_away(m, None, 16, 7, avoid) {
                        b.mem.walk = Some(Walk { target: Track::Block(block_of(p)), speed: 0.75, close: 0 });
                        break;
                    }
                }
                true
            }
            Trig::Shuffled { kids, rng } => {
                for k in kids.iter_mut() {
                    let r = rng.next_float();
                    k.2 = -f64::from(r).powf(f64::from(1.0f32 / k.1 as f32));
                }
                kids.sort_by(|a, c| a.2.partial_cmp(&c.2).unwrap_or(std::cmp::Ordering::Equal));
                for kid in kids.iter_mut() {
                    let mut tr = std::mem::replace(&mut kid.0, Trig::WakeUp);
                    let ok = self.trig(m, b, &mut tr, t);
                    kid.0 = tr;
                    if ok {
                        break;
                    }
                }
                true
            }
        }
    }

    // ---------- Behavior tools ----------

    fn swim_ok(&self, m: &Mob) -> bool {
        let thr = if m.eye_height() < 0.4 { 0.0 } else { 0.4 };
        (m.in_water && m.water_height > thr) || m.lava_height > 0.0
    }

    fn walk_reached(&self, m: &Mob, w: Walk) -> bool {
        self.track_block(w.target).is_some_and(|p| manhattan(p, m.block_pos()) <= w.close)
    }

    /// The path is kept by the behavior; the
    /// navigation only receives one at start (or on recomputation while walking).
    fn compute_walk_path(
        &mut self,
        m: &mut Mob,
        b: &mut Brain,
        w: Walk,
        t: i64,
        kept: &mut Option<Box<NavPath>>,
        speed: &mut f32,
    ) -> bool {
        let Some(target) = self.track_block(w.target) else {
            *kept = None;
            return false;
        };
        *kept = self.create_path(m, target, 0).map(Box::new);
        *speed = w.speed;
        if self.walk_reached(m, w) {
            b.mem.cant_reach_since = None;
        } else {
            let can = kept.as_ref().is_some_and(|p| p.reached);
            if can {
                b.mem.cant_reach_since = None;
            } else if b.mem.cant_reach_since.is_none() {
                b.mem.cant_reach_since = Some(t);
            }
            if kept.is_some() {
                return true;
            }
            let to = [f64::from(target[0]) + 0.5, f64::from(target[1]), f64::from(target[2]) + 0.5];
            if let Some(step) = self.rpos_default_towards(m, None, 10, 7, to, f64::from(std::f32::consts::FRAC_PI_2)) {
                *kept = self.create_path(m, block_of(step), 0).map(Box::new);
                return kept.is_some();
            }
        }
        false
    }

    fn follow_trader_ok(&self, m: &Mob) -> bool {
        m.alive() && self.npc_trading(m) && !m.in_water && !self.hurt_recently(m) && dist2(m.pos, self.pos) <= 16.0
    }

    fn follow_trader(&self, b: &mut Brain) {
        b.mem.walk = Some(Walk { target: Track::Ent(Ent::Player, false), speed: 0.5, close: 2 });
        b.mem.look = Some(Track::Ent(Ent::Player, true));
    }

    fn harvest_valid(&self, p: [i32; 3]) -> bool {
        let s = self.st(p);
        let below = self.bname([p[0], p[1] - 1, p[2]]);
        self.crop_max_age(s).is_some_and(|(a, max)| a >= max) || (self.is_air(p) && below == "minecraft:farmland")
    }

    /// Crop (age, max age) for the game's crop blocks.
    pub(crate) fn crop_max_age(&self, s: u32) -> Option<(i32, i32)> {
        let n = self.state_name(s);
        let max = match n {
            "minecraft:wheat" | "minecraft:carrots" | "minecraft:potatoes" => 7,
            "minecraft:beetroots" => 3,
            // Age 2 = the flower itself: never "ripe" as a sprout.
            "minecraft:torchflower_crop" => 2,
            _ => return None,
        };
        Some((self.v().get_int(s, "age"), max))
    }

    #[allow(clippy::too_many_arguments)]
    fn harvest_tick(
        &mut self,
        m: &mut Mob,
        b: &mut Brain,
        pos: &mut Option<[i32; 3]>,
        next_ok: &mut i64,
        worked: &mut i32,
        valid: &mut Vec<[i32; 3]>,
        t: i64,
    ) {
        if !pos.is_none_or(|p| closer_center(p, m.pos, 1.0)) {
            return;
        }
        if let Some(p) = *pos
            && t > *next_ok
        {
            let s = self.st(p);
            let crop = self.crop_max_age(s);
            let below = self.bname([p[0], p[1] - 1, p[2]]) == "minecraft:farmland";
            if crop.is_some_and(|(a, max)| a >= max) {
                self.villager_destroy_block(m, p);
            }
            if self.v().is_air(s) && below && self.npc_has_plantable(m) {
                self.plant_seed(m, p);
            }
            if crop.is_some_and(|(a, max)| a < max) {
                valid.retain(|v| *v != p);
                *pos = if valid.is_empty() {
                    None
                } else {
                    Some(valid[self.mobs.rng.next_int_bound(valid.len() as i32) as usize])
                };
                if let Some(np) = *pos {
                    *next_ok = t + 20;
                    b.mem.walk = Some(Walk { target: Track::Block(np), speed: 0.5, close: 1 });
                    b.mem.look = Some(Track::Block(np));
                }
            }
        }
        *worked += 1;
    }

    /// Bone meal target: reservoir draw on the world random.
    fn bonemeal_target(&mut self, m: &Mob) -> Option<[i32; 3]> {
        let c = m.block_pos();
        let mut out = None;
        let mut count = 0;
        for x in -1..=1 {
            for y in -1..=1 {
                for z in -1..=1 {
                    let p = [c[0] + x, c[1] + y, c[2] + z];
                    let s = self.st(p);
                    if self.crop_max_age(s).is_some_and(|(a, max)| a < max) {
                        count += 1;
                        if self.mobs.rng.next_int_bound(count) == 0 {
                            out = Some(p);
                        }
                    }
                }
            }
        }
        out
    }

    fn show_trades_ok(&self, m: &Mob, b: &Brain) -> bool {
        let Some(t) = b.mem.interaction else { return false };
        t == Ent::Player && m.alive() && self.ent_alive(t) && !m.is_baby() && dist2(m.pos, self.pos) <= 17.0
    }

    fn hero_visible(&self, b: &Brain) -> bool {
        b.mem.visible_player.is_some() && self.player_has_effect("minecraft:hero_of_the_village")
    }

    fn on_or_over_bed(&self, m: &Mob) -> bool {
        let p = m.block_pos();
        self.btag_id(p, crate::tag_bits::TagId::VillagerBabiesCanJumpOnBed)
            || self.btag_id([p[0], p[1] - 1, p[2]], crate::tag_bits::TagId::VillagerBabiesCanJumpOnBed)
    }

    fn bed_occupied(&self, p: [i32; 3]) -> bool {
        let s = self.st(p);
        self.btag_id(p, crate::tag_bits::TagId::VillagersCanSleepOnBed) && self.v().get_bool(s, "occupied")
    }

    fn villager_sleeping_at(&self, p: [i32; 3]) -> bool {
        self.mobs.list.iter().flatten().any(|o| {
            o.kind == Kind::Villager
                && o.npc.as_ref().is_some_and(|n| n.sleeping.is_some())
                && o.touches(&rc_physics::Aabb {
                    min: [f64::from(p[0]), f64::from(p[1]), f64::from(p[2])],
                    max: [f64::from(p[0]) + 1.0, f64::from(p[1]) + 1.0, f64::from(p[2]) + 1.0],
                })
        })
    }

    /// Living, villager and visible.
    fn target_valid(&self, m: &mut Mob, b: &mut Brain, e: Option<Ent>) -> bool {
        let Some(e) = e else { return false };
        self.ent_is(e, LookKind::Villager) && self.ent_alive(e) && self.visible(m, b, e)
    }

    fn breeding_possible(&self, m: &mut Mob, b: &mut Brain) -> bool {
        let Some(e) = b.mem.breed else { return false };
        if !self.ent_is(e, LookKind::Villager) {
            return false;
        }
        self.target_valid(m, b, Some(e)) && self.can_breed(m) && self.ent_can_breed(e)
    }

    fn ent_can_breed(&self, e: Ent) -> bool {
        match e {
            Ent::Mob(id) => self.mob_ref(id).is_some_and(|o| self.can_breed(o)),
            Ent::Player => false,
        }
    }

    /// Mutual gaze with both walking toward each other.
    fn lock_gaze(&mut self, m: &mut Mob, b: &mut Brain, other: i32, speed: f32, close: i32) {
        b.mem.look = Some(Track::Ent(Ent::Mob(other), true));
        let me = m.id;
        self.with_villager(other, |_, _, ob| {
            if let Some(ob) = ob {
                ob.mem.look = Some(Track::Ent(Ent::Mob(me), true));
            }
        });
        walk_and_look(b, Track::Ent(Ent::Mob(other), true), speed, close);
        self.with_villager(other, |_, _, ob| {
            if let Some(ob) = ob {
                walk_and_look(ob, Track::Ent(Ent::Mob(me), true), speed, close);
            }
        });
    }

    fn other_wants_food(&self, id: i32) -> bool {
        self.mob_ref(id).is_some_and(|o| self.npc_food_points(o) < 12)
    }

    /// POI acquisition (batches of 5, retries with increasing delay).
    fn acquire_poi(
        &mut self,
        m: &mut Mob,
        b: &mut Brain,
        kind: Acq,
        next: &mut i64,
        cache: &mut Vec<([i32; 3], Retry)>,
        t: i64,
    ) -> bool {
        let present = match kind {
            Acq::Job => b.mem.job_site.is_some() || b.mem.potential_job_site.is_some(),
            Acq::Home => b.mem.home.is_some(),
            Acq::Meeting => b.mem.meeting_point.is_some(),
        };
        if present {
            return false;
        }
        let adult_only = kind != Acq::Home;
        if adult_only && m.is_baby() {
            return false;
        }
        if *next == 0 {
            *next = self.time + i64::from(self.mobs.rng.next_int_bound(20));
            return false;
        }
        if self.time < *next {
            return false;
        }
        *next = t + 20 + i64::from(self.mobs.rng.next_int_bound(20));
        cache.retain(|c| t - c.1.prev < 400);
        let prof = m.villager[1];
        let pred = move |ty: PoiType| match kind {
            Acq::Job => {
                if prof == PROF_NONE {
                    ty.is_job()
                } else {
                    job_poi(prof) == Some(ty)
                }
            }
            Acq::Home => ty == PoiType::Home,
            Acq::Meeting => ty == PoiType::Meeting,
        };
        let center = m.block_pos();
        // Retry filter evaluated on ALL POIs in the radius (the game's stream sort), then sort by
        // distance, first 5, then bed validity.
        let all = self.poi_in_range(pred, center, 48, Occupancy::HasSpace);
        let mut list = Vec::new();
        for r in all {
            let ok = match cache.iter_mut().find(|c| c.0 == r.pos) {
                None => true,
                Some(c) if t >= c.1.next => {
                    c.1.mark(&mut self.mobs.rng, t);
                    true
                }
                Some(_) => false,
            };
            if ok {
                list.push(r);
            }
        }
        list.sort_by_key(|r| crate::poi::dist2i(r.pos, center));
        list.truncate(5);
        let mut pois = Vec::new();
        for r in list {
            if kind == Acq::Home {
                let s = self.st(r.pos);
                if !(self.btag_id(r.pos, crate::tag_bits::TagId::VillagersCanSleepOnBed) && !self.v().get_bool(s, "occupied")) {
                    continue;
                }
            }
            pois.push((r.ty, r.pos));
        }
        let path = self.path_to_pois(m, &pois);
        if let Some(p) = path.filter(|p| p.reached) {
            let target = p.target;
            if self.poi_type(target).is_some() {
                self.poi_acquire(target);
                let g = Some(self.gpos_here(target));
                match kind {
                    Acq::Job => b.mem.potential_job_site = g,
                    Acq::Home => b.mem.home = g,
                    Acq::Meeting => b.mem.meeting_point = g,
                }
                if kind != Acq::Job {
                    self.mob_event(m, 14);
                }
                cache.clear();
            }
        } else {
            for (_, p) in pois {
                if !cache.iter().any(|c| c.0 == p) {
                    let r = Retry::new(&mut self.mobs.rng, t);
                    cache.push((p, r));
                }
            }
        }
        true
    }

    /// The path search toward points of interest.
    pub(crate) fn path_to_pois(&self, m: &mut Mob, pois: &[(PoiType, [i32; 3])]) -> Option<super::super::nav::Path> {
        if pois.is_empty() {
            return None;
        }
        let range = pois.iter().map(|p| p.0.valid_range()).max().unwrap_or(1).max(1);
        let targets: Vec<[i32; 3]> = pois.iter().map(|p| p.1).collect();
        self.create_path_multi(m, &targets, range)
    }

    fn competitor_scan(&mut self, m: &mut Mob, b: &mut Brain) -> bool {
        let (Some(job), Some(living)) = (b.mem.job_site, b.mem.living.clone()) else { return false };
        // Type read at this level whatever the dimension of the location (like the game); the
        // comparison of job sites, though, takes the dimension into account.
        let Some(ty) = self.poi_type(job.pos) else { return true };
        // The game's reduction: the winner (more experience, otherwise the second) faces the next.
        let mut winner: (i32, i32) = (m.id, m.x.villager_xp);
        for e in living {
            let Ent::Mob(id) = e else { continue };
            let Some(o) = self.mob_ref(id) else { continue };
            if o.kind != Kind::Villager || !o.alive() {
                continue;
            }
            let same = o.npc.as_ref().and_then(|n| n.brain.as_ref()).and_then(|ob| ob.mem.job_site) == Some(job);
            if !same || job_poi(o.villager[1]) != Some(ty) {
                continue;
            }
            let oxp = o.x.villager_xp;
            let (w, l) = if winner.1 > oxp { (winner, (id, oxp)) } else { ((id, oxp), winner) };
            if l.0 == m.id {
                b.mem.job_site = None;
            } else {
                self.with_villager(l.0, |_, _, ob| {
                    if let Some(ob) = ob {
                        ob.mem.job_site = None;
                    }
                });
            }
            winner = w;
        }
        true
    }

    fn yield_job_site(&mut self, m: &mut Mob, b: &mut Brain) -> bool {
        let (Some(pot), None, Some(living)) = (b.mem.potential_job_site, b.mem.job_site, b.mem.living.clone()) else {
            return false;
        };
        if m.is_baby() || m.villager[1] != PROF_NONE {
            return false;
        }
        let pot = pot.pos;
        let Some(ty) = self.poi_type(pot) else { return true };
        for e in living {
            let Ent::Mob(id) = e else { continue };
            let Some(o) = self.mob_ref(id) else { continue };
            if o.kind != Kind::Villager || !o.alive() {
                continue;
            }
            let ob = o.npc.as_ref().and_then(|n| n.brain.as_ref());
            if ob.is_some_and(|ob| ob.mem.potential_job_site.is_some()) {
                continue;
            }
            if job_poi(o.villager[1]) != Some(ty) {
                continue;
            }
            let their_job = ob.and_then(|ob| ob.mem.job_site);
            let wants = match their_job {
                Some(j) => j.pos == pot,
                None => {
                    let mut ok = false;
                    self.with_villager(id, |s, o, _| {
                        ok = s.create_path(o, pot, ty.valid_range()).is_some_and(|p| p.reached);
                    });
                    ok
                }
            };
            if !wants {
                continue;
            }
            b.mem.walk = None;
            b.mem.look = None;
            b.mem.potential_job_site = None;
            if their_job.is_none() {
                let g = self.gpos_here(pot);
                self.with_villager(id, |_, _, ob| {
                    if let Some(ob) = ob {
                        walk_and_look(ob, Track::Block(pot), 0.5, 1);
                        ob.mem.potential_job_site = Some(g);
                    }
                });
            }
            break;
        }
        true
    }

    /// Releases the POI of a memory if its type still matches.
    /// The registry of another dimension is not reachable from here: nothing is released there.
    pub(crate) fn release_poi_mem(&mut self, m: &Mob, b: &Brain, mem: PoiMem) {
        let Some(p) = self.here(mem.get(b)) else { return };
        let Some(ty) = self.poi_type(p) else { return };
        let ok = match mem {
            PoiMem::Home => ty == PoiType::Home,
            PoiMem::JobSite => job_poi(m.villager[1]) == Some(ty),
            PoiMem::Potential => ty.is_job(),
            PoiMem::Meeting => ty == PoiType::Meeting,
        };
        if ok {
            self.poi_release(p);
        }
    }

    /// Search for the village's closest section (cube of sections, first minimum).
    /// The game's cursor order: x fastest, then y, z slowest (ties: first).
    /// Shared with the iron golem (F06).
    pub(crate) fn section_closest_to_village(&mut self, c: [i32; 3], r: i32) -> [i32; 3] {
        super::super::golems::iron::closest_section(c, r, |s| self.section_to_village(s))
    }

    fn sections_to_village_ro(&self, p: [i32; 3]) -> i32 {
        self.mobs.poi.sections_to_village((p[0] >> 4, p[1] >> 4, p[2] >> 4))
    }

    fn can_stand_on(&self, m: &Mob, p: [i32; 3]) -> bool {
        // The "loaded and the entity can stand there" test (full top face) then the mob's no-collision test.
        self.chunks.get(&(p[0] >> 4, p[2] >> 4)).is_some_and(Option::is_some)
            && self.full_collision(p)
            && rc_physics::is_free(&self.world(), &m.bbox())
    }

    fn play_tag(&mut self, m: &mut Mob, b: &mut Brain) -> bool {
        let Some(babies) = b.mem.babies.clone() else { return false };
        if b.mem.walk.is_some() {
            return false;
        }
        if self.mobs.rng.next_int_bound(10) != 0 {
            return false;
        }
        let me = Ent::Mob(m.id);
        let chased_by = |s: &Server, e: Ent| -> Option<Ent> {
            let Ent::Mob(id) = e else { return None };
            s.mob_ref(id)?.npc.as_ref()?.brain.as_ref()?.mem.interaction
        };
        let chasing_me = babies.iter().any(|&f| chased_by(self, f) == Some(me));
        if !chasing_me {
            let mut counts: Vec<(Ent, i32)> = Vec::new();
            for &f in &babies {
                if let Some(t) = chased_by(self, f) {
                    match counts.iter_mut().find(|c| c.0 == t) {
                        Some(c) => c.1 += 1,
                        None => counts.push((t, 1)),
                    }
                }
            }
            counts.sort_by_key(|c| c.1);
            let target = counts.iter().find(|c| c.1 > 0 && c.1 <= 5).map(|c| c.0).or_else(|| babies.first().copied());
            if let Some(k) = target {
                b.mem.interaction = Some(k);
                b.mem.look = Some(Track::Ent(k, true));
                b.mem.walk = Some(Walk { target: Track::Ent(k, false), speed: 0.6, close: 1 });
            }
        } else {
            for _ in 0..10 {
                let p = self.rpos_land(m, None, 20, 8, &|_, _| 0.0);
                if let Some(p) = p
                    && self.is_village(block_of(p))
                {
                    b.mem.walk = Some(Walk { target: Track::Block(block_of(p)), speed: 0.6, close: 0 });
                    break;
                }
            }
        }
        true
    }

    fn closest_home(
        &mut self,
        m: &mut Mob,
        b: &mut Brain,
        cache: &mut Vec<([i32; 3], i64)>,
        last: &mut i64,
        t: i64,
    ) -> bool {
        if b.mem.walk.is_some() || b.mem.home.is_some() {
            return false;
        }
        if self.time - *last < 20 {
            return false;
        }
        let here = m.block_pos();
        let Some(closest) = self.poi_closest(|t| t == PoiType::Home, here, 48, Occupancy::Any) else { return false };
        if crate::poi::dist2i(closest, here) <= 4 {
            return false;
        }
        let mut tried = 0;
        *last = self.time + i64::from(self.mobs.rng.next_int_bound(20));
        let all = self.poi_in_range(|t| t == PoiType::Home, here, 48, Occupancy::Any);
        let mut pois = Vec::new();
        for r in all {
            if cache.iter().any(|c| c.0 == r.pos) {
                continue;
            }
            tried += 1;
            if tried >= 5 {
                continue;
            }
            cache.push((r.pos, *last + 40));
            pois.push((r.ty, r.pos));
        }
        let _ = t;
        let path = self.path_to_pois(m, &pois);
        if let Some(p) = path.filter(|p| p.reached) {
            if self.poi_type(p.target).is_some() {
                b.mem.walk = Some(Walk { target: Track::Block(p.target), speed: 0.5, close: 1 });
            }
        } else if tried < 5 {
            let l = *last;
            cache.retain(|c| c.1 >= l);
        }
        true
    }

    // ---------- Doors ----------

    pub(crate) fn is_mob_door(&self, p: [i32; 3]) -> bool {
        self.btag_id(p, crate::tag_bits::TagId::MobInteractableDoors) && self.bname(p).ends_with("_door")
    }

    pub(crate) fn set_door_open(&mut self, p: [i32; 3], open: bool, by: i32) {
        let s = self.st(p);
        if self.v().get_bool(s, "open") == open {
            return;
        }
        let ns = self.v().set_bool(s, "open", open);
        let name = self.state_name(s).to_owned();
        self.set_block(p, ns, 10);
        let other =
            if self.v().get(s, "half") == Some("lower") { [p[0], p[1] + 1, p[2]] } else { [p[0], p[1] - 1, p[2]] };
        let os = self.st(other);
        if self.state_name(os) == name {
            let ons = self.v().set_bool(os, "open", open);
            self.set_block(other, ons, 10);
        }
        // Sound for everyone and event attributed to the mob that opens.
        self.openable_sound(p, s, open, Some(Ent::Mob(by)));
    }

    fn door_trigger(&mut self, m: &mut Mob, b: &mut Brain, last: &mut Option<[i32; 3]>, cooldown: &mut i32) -> bool {
        if !b.mem.path {
            return false;
        }
        let Some(path) = m.nav.path.clone() else { return false };
        if path.next == 0 || path.is_done() {
            return false;
        }
        let next = path.nodes[path.next].0;
        if *last == Some(next) {
            *cooldown = 20;
        } else {
            *cooldown -= 1;
            if *cooldown > 0 {
                return false;
            }
        }
        *last = Some(next);
        let from = path.nodes[path.next - 1].0;
        if self.is_mob_door(from) {
            self.set_door_open(from, true, m.id);
            let d = b.mem.doors.get_or_insert_with(Vec::new);
            if !d.contains(&from) {
                d.push(from);
            }
        }
        if self.is_mob_door(next) && !self.v().get_bool(self.st(next), "open") {
            self.set_door_open(next, true, m.id);
            let d = b.mem.doors.get_or_insert_with(Vec::new);
            if !d.contains(&next) {
                d.push(next);
            }
        }
        if b.mem.doors.is_some() {
            self.close_doors(m, b, Some(from), Some(next));
        }
        true
    }

    /// Closing of the doors that were opened or passed through.
    fn close_doors(&mut self, m: &Mob, b: &mut Brain, from: Option<[i32; 3]>, to: Option<[i32; 3]>) {
        let Some(doors) = b.mem.doors.take() else { return };
        let mut keep = Vec::new();
        for d in doors {
            if from == Some(d) || to == Some(d) {
                keep.push(d);
                continue;
            }
            if !closer_center(d, m.pos, 3.0) || !self.is_mob_door(d) || !self.v().get_bool(self.st(d), "open") {
                continue;
            }
            let others = self.mobs.list.iter().flatten().any(|o| {
                o.kind == m.kind && o.id != m.id && closer_center(d, o.pos, 2.0) && {
                    o.nav.path.as_ref().is_some_and(|p| {
                        !p.is_done() && p.next > 0 && (p.nodes[p.next - 1].0 == d || p.nodes[p.next].0 == d)
                    })
                }
            });
            if !others {
                self.set_door_open(d, false, m.id);
            }
        }
        b.mem.doors = (!keep.is_empty()).then_some(keep);
    }
}

// ---------- Raid: bell, sky, celebration ----------

impl Server {
    /// Bell struck by a villager (without direction: the bell's).
    pub(crate) fn villager_ring_bell(&mut self, m: &Mob, p: [i32; 3]) {
        let s = self.st(p);
        let facing = self.data.state_properties(s).find(|(k, _)| *k == "facing").map(|(_, v)| v.to_owned());
        let dir = match facing.as_deref() {
            Some("north") => 2,
            Some("south") => 3,
            Some("west") => 4,
            _ => 5,
        };
        let blk = self.v().block(s);
        self.block_event(p, blk, 1, dir);
        let c = [f64::from(p[0]) + 0.5, f64::from(p[1]) + 0.5, f64::from(p[2]) + 0.5];
        self.play_sound("minecraft:block.bell.use", 4, c, 2.0, 1.0);
        self.game_event_block("minecraft:block_change", p, Some(Ent::Mob(m.id)), None);
    }

    /// Visible sky and nothing blocking movement above the
    /// mob's height.
    pub(crate) fn no_blocks_above(&self, m: &Mob, p: [i32; 3]) -> bool {
        self.can_see_sky(p) && f64::from(self.hm_motion_blocking(p[0], p[2])) <= m.pos[1]
    }

    /// Celebration: cheer (1 in 100), rocket of a random color (1 in 200, under the sky).
    pub(crate) fn celebrate_tick(&mut self, m: &mut Mob) {
        if m.rng.next_int_bound(100) == 0 {
            self.make_sound(m, "minecraft:entity.villager.celebrate");
        }
        if m.rng.next_int_bound(200) == 0 && self.no_blocks_above(m, m.block_pos()) {
            let dyes = super::super::combat::DYES;
            let color = dyes[m.rng.next_int_bound(dyes.len() as i32) as usize];
            let flight = m.rng.next_int_bound(3);
            let explosion = Nbt::Compound(vec![
                ("shape".into(), Nbt::String("burst".into())),
                ("colors".into(), Nbt::IntArray(vec![crate::special::firework_color(color)])),
                ("fade_colors".into(), Nbt::IntArray(Vec::new())),
                ("has_trail".into(), Nbt::Byte(0)),
                ("has_twinkle".into(), Nbt::Byte(0)),
            ]);
            let fw = Nbt::Compound(vec![
                ("flight_duration".into(), Nbt::Byte(flight as i8)),
                ("explosions".into(), Nbt::List(vec![explosion])),
            ]);
            let mut rocket = Stack { id: "minecraft:firework_rocket".into(), count: 1, components: None };
            crate::items::set_component(&mut rocket, "minecraft:fireworks", fw);
            let at = [m.pos[0], m.eye_y(), m.pos[2]];
            let p = self.new_firework(at, rocket.clone(), Some(m.uuid), false);
            self.proj_spawn(p, &rocket);
        }
    }
}
