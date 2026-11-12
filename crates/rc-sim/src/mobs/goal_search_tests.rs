//! Equivalence proofs of the mob goals' searches on a fresh flat world (26.3 version data
//! mandatory).
//!
//! - A crowded pen of animals, babies and a few monsters lives 400 ticks; what the searches decide
//!   (targets, parents, partners, fled mobs), where every mob stands and what its random source
//!   has drawn are reduced to checkpoint hashes COMPARED to the values written here, with the old
//!   paths, with the new ones and with the switches of the environment. An optimisation never
//!   changes them; a behaviour fix changes them and replaces the constants (the failure message
//!   gives the new values).
//! - Each new path is compared with the path before on the mobs of that pen: nearest mob of a
//!   class, parent followed, partner, call for help, melee reach, seat of a mounted target, food
//!   held by the player, "go to a block" searches.
//! - Every indexed species read of the ticks is checked against the walk of the list
//!   (`index::verify`), every kept goal flag against the computed one, every seat read from the
//!   vehicle link against the walk of the passengers.
//!
//! Micro-benches ignored (`--ignored --nocapture`, quiet machine).

use rc_physics::Aabb;

use crate::blocks::probe::Probe;
use crate::mobs::goals::goal_blocks::GoalBlock;
use crate::mobs::goals::surface_goals::{Mtb, SGoal};
use crate::mobs::goals::switches::{FORCED, GoalSwitches};
use crate::mobs::goals::{Food, Goal, Who, kin_species, partner_species, who_fits, who_species};
use crate::mobs::jrand::JRand;
use crate::mobs::mob::{Ent, Mob};
use crate::mobs::mth::dist2;
use crate::mobs::species::Kind;
use crate::player::Stack;
use crate::server::Server;
use crate::tests_g3f13::{finish, open};

/// Surface of the flat world (feet height of a land mob).
const LAND_Y: f64 = -60.0;

/// Deterministic draws (xorshift): positions never depend on the clock.
struct Draw(u64);

impl Draw {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn within(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * ((self.next() >> 11) as f64 / (1u64 << 53) as f64)
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn cmd(p: &mut Probe, c: &str) {
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, c);
    assert!(ok, "command refused: {c}");
}

fn game_ticks(p: &mut Probe, n: usize) {
    for _ in 0..n {
        p.s.game_tick();
        p.s.tick += 1;
    }
}

fn add(s: &mut Server, kind: Kind, pos: [f64; 3], setup: impl FnOnce(&mut Mob)) -> i32 {
    let mut m = s.new_mob(kind, pos, 0.0);
    m.persistent = true;
    setup(&mut m);
    let id = m.id;
    s.add_mob(m);
    id
}

fn held(id: &str) -> Option<Stack> {
    Some(Stack { id: id.into(), count: 1, components: None })
}

fn inflated(b: Aabb, by: [f64; 3]) -> Aabb {
    Aabb { min: [b.min[0] - by[0], b.min[1] - by[1], b.min[2] - by[2]], max: [b.max[0] + by[0], b.max[1] + by[1], b.max[2] + by[2]] }
}

fn slots_of(s: &Server, wanted: impl Fn(&Mob) -> bool) -> Vec<usize> {
    (0..s.mobs.list.len()).filter(|&i| s.mobs.list.get(i).is_some_and(&wanted)).collect()
}

/// Runs `f` with the goal switches forced on this thread, then gives the environment back.
fn with_switches<R>(set: GoalSwitches, f: impl FnOnce() -> R) -> R {
    FORCED.set(Some(set));
    let r = f();
    FORCED.set(None);
    r
}

/// Counts of an old/new comparison: (results found, results empty).
#[derive(Default, Debug)]
struct Seen(usize, usize);

impl Seen {
    fn note(&mut self, found: bool) {
        if found { self.0 += 1 } else { self.1 += 1 }
    }

    fn both(&self) -> bool {
        self.0 > 0 && self.1 > 0
    }
}

// ---------- The pen ----------

/// Mobs of the pen the scenario acts on by name.
struct Cast {
    /// The mob blamed for every hit of the scenario.
    aggressor: i32,
    /// One mob per alert rule: bee, panda, wild wolf, zombie, zombified piglin, polar bear cub, llama.
    victims: Vec<i32>,
}

/// Age and love of the `i`-th animal of a species: a third are babies, two adults in three are in
/// love.
fn family_state(i: usize, m: &mut Mob) {
    if i.is_multiple_of(3) {
        m.age = -24000;
    } else if i % 3 == 1 || i.is_multiple_of(2) {
        m.in_love = 600;
    }
}

/// The pen: 24 × 24 blocks east of the player, 235 mobs. Farm animals with babies and
/// lovers, tamed and wild pets, equines and llamas (partners across species), prey and hunters,
/// bees and pandas (their own alert rules), a few monsters, an iron golem and two jockeys.
fn populate_pen(s: &mut Server) -> Cast {
    let mut r = Draw(0x9E37_79B9_7F4A_7C15);
    let mut spot = move || [r.within(6.0, 30.0), LAND_Y, r.within(-12.0, 12.0)];
    let owner = s.uuid;
    for (kind, n) in [(Kind::Cow, 40), (Kind::Sheep, 40), (Kind::Pig, 30), (Kind::Chicken, 30)] {
        for i in 0..n {
            add(s, kind, spot(), |m| family_state(i, m));
        }
    }
    for (kind, n) in [(Kind::Horse, 8), (Kind::Donkey, 6), (Kind::Llama, 8), (Kind::TraderLlama, 3)] {
        for i in 0..n {
            add(s, kind, spot(), |m| {
                family_state(i, m);
                m.mount.flags |= crate::mobs::mounts::FLAG_TAME;
            });
        }
    }
    for i in 0..8 {
        add(s, Kind::Wolf, spot(), |m| {
            if i % 2 == 0 {
                (m.tame, m.owner, m.in_love) = (true, Some(owner), 600);
            }
        });
    }
    for _ in 0..6 {
        add(s, Kind::Cat, spot(), |m| (m.tame, m.owner, m.in_love) = (true, Some(owner), 600));
    }
    for (kind, n) in [(Kind::Rabbit, 6), (Kind::Fox, 4), (Kind::Bee, 6), (Kind::Panda, 6), (Kind::PolarBear, 4)] {
        for i in 0..n {
            add(s, kind, spot(), |m| {
                if kind == Kind::PolarBear && i % 2 == 0 {
                    m.age = -24000;
                }
                if kind == Kind::Panda && i % 2 == 0 {
                    (m.wild.main_gene, m.wild.hidden_gene) = (crate::mobs::wild::panda::AGGRESSIVE, crate::mobs::wild::panda::AGGRESSIVE);
                }
            });
        }
    }
    for (kind, n) in [
        (Kind::Zombie, 10),
        (Kind::Husk, 2),
        (Kind::Drowned, 2),
        (Kind::ZombifiedPiglin, 3),
        (Kind::Skeleton, 4),
        (Kind::Creeper, 2),
        (Kind::Spider, 2),
        (Kind::IronGolem, 1),
    ] {
        for _ in 0..n {
            add(s, kind, spot(), |_| {});
        }
    }
    // Two jockeys: a mounted target has its box raised to its seat.
    let mut spider = s.new_mob(Kind::Spider, spot(), 0.0);
    spider.persistent = true;
    s.spider_skeleton_jockey(&mut spider, crate::mobs::spawn::Reason::SpawnEgg);
    s.add_mob(spider);
    let mut chicken = s.new_mob(Kind::Chicken, spot(), 0.0);
    let mut rider = s.new_mob(Kind::Zombie, chicken.pos(), 0.0);
    // The rider stays adult-sized (a zombie's size is its own baby flag, not its age); the age is
    // part of the recorded scenario.
    (chicken.persistent, rider.persistent, rider.age) = (true, true, -24000);
    assert!(s.mob_start_riding(&mut rider, &mut chicken, false), "chicken jockey");
    s.add_mob(chicken);
    s.add_mob(rider);

    let first = |s: &Server, k: Kind, baby: bool| {
        s.mobs.list.iter().find(|m| m.kind == k && m.is_baby() == baby && !m.tame).map(|m| m.id).expect("cast member")
    };
    Cast {
        aggressor: first(s, Kind::Skeleton, false),
        victims: vec![
            first(s, Kind::Bee, false),
            first(s, Kind::Panda, false),
            first(s, Kind::Wolf, false),
            first(s, Kind::Zombie, false),
            first(s, Kind::ZombifiedPiglin, false),
            first(s, Kind::PolarBear, true),
            first(s, Kind::Llama, false),
        ],
    }
}

/// The aggressor "hits" every victim still there: each one retaliates and calls its kin for help.
fn blame_aggressor(s: &mut Server, cast: &Cast) {
    for &id in &cast.victims {
        if let Some(m) = s.mobs.get_mut(id) {
            m.last_hurt_by_mob = Some((Ent::Mob(cast.aggressor), m.tick_count));
        }
    }
}

/// Adults that can love again are put in love.
fn second_spring(s: &mut Server) {
    for m in s.mobs.list.iter_mut() {
        if m.kind.is_animal() && m.age == 0 && m.in_love == 0 {
            m.in_love = 600;
        }
    }
}

/// FNV-1a over what the scenario must keep: list order, places, random sources, and what each
/// search of the goals decided.
#[derive(Clone, Copy)]
struct Fnv(u64);

impl Fnv {
    fn new() -> Fnv {
        Fnv(0xCBF2_9CE4_8422_2325)
    }

    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 = (self.0 ^ u64::from(x)).wrapping_mul(0x0000_0100_0000_01B3);
        }
    }

    fn int(&mut self, v: i64) {
        self.bytes(&v.to_le_bytes());
    }

    fn float(&mut self, v: f64) {
        self.bytes(&v.to_bits().to_le_bytes());
    }

    fn ent(&mut self, e: Option<Ent>) {
        self.int(match e {
            None => -2,
            Some(Ent::Player) => -1,
            Some(Ent::Mob(id)) => i64::from(id),
        });
    }

    fn id(&mut self, id: Option<i32>) {
        self.int(id.map_or(-2, i64::from));
    }
}

fn hash_goal(h: &mut Fnv, running: bool, g: &Goal) {
    h.int(i64::from(running));
    match g {
        Goal::Breed { partner, love_time, .. } => {
            h.id(*partner);
            h.int(i64::from(*love_time));
        }
        Goal::FollowParent { parent, .. } => h.id(*parent),
        Goal::Avoid { from, .. } => h.ent(*from),
        Goal::Nearest { target, .. } => h.ent(*target),
        Goal::Tempt { on, calm, .. } => h.int(i64::from(*on) + 2 * i64::from(*calm)),
        Goal::Melee { cd, .. } => h.int(i64::from(*cd)),
        _ => {}
    }
}

fn hash_mob(h: &mut Fnv, m: &Mob) {
    h.int(i64::from(m.id));
    h.int(m.kind as i64);
    for v in m.pos().into_iter().chain(m.vel) {
        h.float(v);
    }
    h.float(f64::from(m.yaw));
    h.float(f64::from(m.health));
    h.int(i64::from(m.age));
    h.int(i64::from(m.in_love));
    h.int(i64::from(m.removed) + 2 * i64::from(m.dead));
    h.ent(m.target);
    h.ent(m.anger_target);
    h.ent(m.last_hurt_by_mob.map(|x| x.0));
    h.id(m.mount.vehicle);
    // Every draw moves the random source: one draw more or less shows here.
    h.bytes(format!("{:?}", m.rng).as_bytes());
    for w in m.goals.goals.iter().chain(&m.targets.goals) {
        hash_goal(h, w.running, &w.goal);
    }
}

fn pen_hash(s: &Server) -> u64 {
    let mut h = Fnv::new();
    for m in s.mobs.list.iter() {
        hash_mob(&mut h, m);
    }
    h.float(s.pos[0]);
    h.float(f64::from(s.life.vit.health));
    h.0
}

/// What a run of the pen saw.
#[derive(Debug, PartialEq)]
struct PenRun {
    /// Hash of the pen every 50 ticks.
    checkpoints: Vec<u64>,
    births: usize,
    /// Mobs with a target, a parent followed, a partner courted, a mob fled (summed over the
    /// checkpoints): the scenario must exercise each search.
    targets: usize,
    parents: usize,
    partners: usize,
    fled: usize,
}

fn count_decisions(s: &Server, run: &mut PenRun) {
    for m in s.mobs.list.iter() {
        run.targets += usize::from(m.target.is_some());
        for w in m.goals.goals.iter().filter(|w| w.running) {
            match &w.goal {
                Goal::FollowParent { parent: Some(_), .. } => run.parents += 1,
                Goal::Breed { partner: Some(_), .. } => run.partners += 1,
                Goal::Avoid { from: Some(_), .. } => run.fled += 1,
                _ => {}
            }
        }
    }
}

/// The pen lived 400 ticks: the player (creative, so never hunted) shows wheat then a carrot, the
/// aggressor is blamed twice, the adults are put in love a second time.
fn run_pen(tag: &str) -> PenRun {
    let (mut p, dir) = open(tag);
    p.s.game_mode = 1;
    let cast = populate_pen(&mut p.s);
    let last_placed = p.s.next_entity;
    let mut run = PenRun { checkpoints: Vec::new(), births: 0, targets: 0, parents: 0, partners: 0, fled: 0 };
    for step in 0..8 {
        let hand = 36 + p.s.held as usize;
        match step {
            1 | 5 => blame_aggressor(&mut p.s, &cast),
            2 => p.s.slots[hand] = held("minecraft:wheat"),
            4 => {
                p.s.slots[hand] = held("minecraft:carrot");
                second_spring(&mut p.s);
            }
            6 => p.s.slots[hand] = None,
            _ => {}
        }
        game_ticks(&mut p, 50);
        run.checkpoints.push(pen_hash(&p.s));
        count_decisions(&p.s, &mut run);
    }
    run.births = p.s.mobs.list.iter().filter(|m| m.id > last_placed && m.is_baby()).count();
    finish(p, dir);
    run
}

/// Checkpoint hashes of the pen. Recorded first on `main` 5782898d before any change of this work
/// (two runs, identical: 0x51ad_2403_23bd_d454 … 0x99b3_c367_5bbe_c40f). The navigation fixes of
/// G6-F20 (path start cell, paths searched for the mount) then move the animals of the pen, the
/// only cause of the change: the former values still hold just before that merge, with the spawn
/// and clock lots in, and the old and the new search paths give the values below alike.
const PEN_CHECKPOINTS: [u64; 8] = [
    0xa05b_324c_f97c_d0a5,
    0x74c4_3cf5_4d66_d9c0,
    0xeddd_735b_b814_7b92,
    0x6ae3_ebbd_9c4e_a995,
    0x1be6_6461_71b2_a708,
    0xf9eb_a235_e78d_0b2e,
    0x0860_6a60_e1bf_8627,
    0xdffa_670c_a095_c49c,
];

fn check_pen(run: &PenRun, what: &str) {
    eprintln!("pen ({what}): {run:?}");
    assert!(run.targets > 0 && run.parents > 0 && run.partners > 0 && run.fled > 0, "{what}: a search never ran");
    assert!(run.births > 0, "{what}: no birth, the partner search decided nothing");
    // The C library's trigonometry differs between systems: the recorded values are Windows'. Linux
    // gives other values since G6-F20 (the same in its three runs of the CI job, both paths alike).
    if cfg!(windows) {
        assert_eq!(run.checkpoints, PEN_CHECKPOINTS, "{what}: the pen changed, new checkpoints {:#018x?}", run.checkpoints);
    }
}

/// The pen with the switches of the environment (all on by default).
#[test]
fn pen_of_animals_keeps_its_fingerprint() {
    check_pen(&run_pen("pen-fingerprint"), "switches of the environment");
}

/// The same pen with every goal switch forced off, then on, each on its own thread.
#[test]
fn pen_of_animals_is_the_same_on_the_old_and_the_new_paths() {
    let run = |set: GoalSwitches, tag: &'static str| {
        std::thread::spawn(move || with_switches(set, || run_pen(tag))).join().expect("pen run")
    };
    let (old, new) = (run(GoalSwitches::OLD, "pen-old-paths"), run(GoalSwitches::ALL_NEW, "pen-new-paths"));
    check_pen(&old, "old paths");
    check_pen(&new, "new paths");
    assert_eq!(old, new);
}

/// The pen after `ticks` ticks, the player in survival (it counts among the living targets).
fn open_pen(tag: &str, ticks: usize) -> (Probe, std::path::PathBuf, Cast) {
    let (mut p, dir) = open(tag);
    p.s.game_mode = 1;
    let cast = populate_pen(&mut p.s);
    game_ticks(&mut p, ticks);
    p.s.game_mode = 0;
    (p, dir, cast)
}

// ---------- Classes and species ----------

fn of_species(kind: Kind) -> Mob {
    Mob::new(kind, 1, 1, [0.0; 3], 0.0, JRand::new(1))
}

/// A mob of `kind` in the state every class rule accepts: a baby on dry land, untamed.
fn willing_member(kind: Kind) -> Mob {
    let mut m = of_species(kind);
    (m.age, m.baby) = (-24000, true);
    assert!(m.is_baby() || kind == Kind::Frog, "{kind:?} cannot be a baby");
    m
}

/// Every class of `Who`; a new variant must be added here (`listed` fails to compile otherwise).
fn all_classes() -> Vec<Who> {
    let listed = |w: Who| match w {
        Who::Player | Who::Villager | Who::Skeleton | Who::WolfPrey | Who::Wolf | Who::Cat | Who::Endermite => (),
        Who::Monster | Who::Chicken | Who::BabyTurtleOnLand | Who::Fox | Who::Rabbit | Who::WildWolf | Who::Zombie => (),
        Who::Illager | Who::ZombieClass | Who::IronGolem | Who::Enemy | Who::GolemPrey | Who::Axolotl => (),
        Who::FoxPrey | Who::SchoolingFish | Who::PolarBear | Who::Piglin | Who::WitherPrey | Who::Exact(_) => (),
    };
    let mut classes = vec![
        Who::Player,
        Who::Villager,
        Who::Skeleton,
        Who::WolfPrey,
        Who::Wolf,
        Who::Cat,
        Who::Endermite,
        Who::Monster,
        Who::Chicken,
        Who::BabyTurtleOnLand,
        Who::Fox,
        Who::Rabbit,
        Who::WildWolf,
        Who::Zombie,
        Who::Illager,
        Who::ZombieClass,
        Who::IronGolem,
        Who::Enemy,
        Who::GolemPrey,
        Who::Axolotl,
        Who::FoxPrey,
        Who::SchoolingFish,
        Who::PolarBear,
        Who::Piglin,
        Who::WitherPrey,
    ];
    classes.extend(Kind::ALL.map(Who::Exact));
    classes.iter().copied().for_each(listed);
    classes
}

/// The species a closed class names are exactly those its rule accepts: the per-species walk of
/// the nearest-mob search misses no mob and walks no useless species.
#[test]
fn closed_classes_name_exactly_the_species_they_accept() {
    let mut closed = 0;
    for who in all_classes() {
        let accepted: Vec<Kind> = Kind::ALL.into_iter().filter(|&k| who_fits(who, &willing_member(k))).collect();
        match who_species(who) {
            Some(named) => {
                closed += 1;
                for kind in Kind::ALL {
                    assert_eq!(named.contains(&kind), accepted.contains(&kind), "{who:?} and {kind:?}: named or accepted, not both");
                }
                assert!(!accepted.is_empty(), "{who:?} accepts no species");
            }
            // An open class holds more species than a closed class can name.
            None if who != Who::Player => assert!(accepted.len() > named_capacity(), "{who:?} could be a closed class"),
            None => assert!(accepted.is_empty(), "the player is no mob of the list"),
        }
    }
    assert_eq!(closed, 20 + Kind::ALL.len());
}

fn named_capacity() -> usize {
    who_species(Who::Cat).expect("closed class").len()
}

/// A mob ready to mate: adult, in love, tamed, at full health.
fn willing_partner(kind: Kind, id: i32) -> Mob {
    let mut m = of_species(kind);
    (m.id, m.in_love, m.tame) = (id, 600, true);
    m.mount.flags |= crate::mobs::mounts::FLAG_TAME;
    m.health = m.max_health();
    m
}

/// Whatever the two species, a mob that passes the species tests of the partner search belongs to
/// the species the search walks.
#[test]
fn partner_species_name_every_possible_partner() {
    let (mut same, mut across) = (0, Vec::new());
    for a in Kind::ALL {
        for b in Kind::ALL {
            let (m, o) = (willing_partner(a, 1), willing_partner(b, 2));
            let may_mate = crate::mobs::mounts::feed::can_mate(&m, &o).unwrap_or(o.kind == m.kind);
            if !may_mate {
                continue;
            }
            assert!(partner_species(a).contains(&b), "{a:?} may mate with {b:?}, which its search does not walk");
            if a == b {
                same += 1;
            } else {
                across.push((a, b));
            }
        }
    }
    assert!(same > 60, "species mating with themselves: {same}");
    let pairs = [(Kind::Horse, Kind::Donkey), (Kind::Llama, Kind::TraderLlama)];
    assert_eq!(across.len(), 4, "pairs across species: {across:?}");
    assert!(pairs.iter().all(|&(a, b)| across.contains(&(a, b)) && across.contains(&(b, a))), "{across:?}");
}

// ---------- Nearest mob of a class ----------

type TargetTest<'a> = &'a mut dyn FnMut(&Server, &mut Mob, Ent) -> bool;

/// The search before this work: the whole list walked, the mobs in the box collected, then each
/// one tested and looked up again by identifier.
fn nearest_by_whole_list_walk(s: &Server, m: &mut Mob, who: Who, inflate: [f64; 3], from_eye: bool, test: TargetTest) -> Option<Ent> {
    let area = inflated(m.bbox(), inflate);
    let mut candidates: Vec<Ent> = Vec::new();
    if who == Who::Player {
        candidates.push(Ent::Player);
    } else {
        candidates.extend(s.mobs.list.iter().filter(|o| who_fits(who, o) && o.touches(&area)).map(|o| Ent::Mob(o.id)));
        if who == Who::WitherPrey && s.player_aabb().intersects(&area) {
            candidates.push(Ent::Player);
        }
    }
    let from = [m.pos()[0], if from_eye { m.eye_y() } else { m.pos()[1] }, m.pos()[2]];
    let mut best: Option<(f64, Ent)> = None;
    for e in candidates {
        if !test(s, m, e) {
            continue;
        }
        let Some(p) = s.ent_pos(e) else { continue };
        let d = dist2(from, p);
        if best.is_none_or(|b| d < b.0) {
            best = Some((d, e));
        }
    }
    best.map(|b| b.1)
}

/// The three searches (new, old path of the switch, whole-list walk before this work) on three
/// copies of the mob: same entity, same mob afterwards (sight cache included).
fn compare_nearest(s: &Server, m: &Mob, who: Who, inflate: [f64; 3], from_eye: bool) -> Option<Ent> {
    let range = inflate[0];
    let mut test = |s: &Server, m: &mut Mob, e: Ent| s.targeting_ok(m, e, true, range, true, true);
    let (mut a, mut b, mut c) = (m.clone(), m.clone(), m.clone());
    let new = s.nearest_of_with(&mut a, who, inflate, from_eye, &mut test, true);
    let old = s.nearest_of_with(&mut b, who, inflate, from_eye, &mut test, false);
    let before = nearest_by_whole_list_walk(s, &mut c, who, inflate, from_eye, &mut test);
    assert_eq!(new, before, "{:?} {} looking for {who:?}: other entity than before", m.kind, m.id);
    assert_eq!(old, before, "{:?} {} looking for {who:?}: old path differs from before", m.kind, m.id);
    assert_eq!(format!("{a:?}"), format!("{c:?}"), "{:?} {}: other mob state (new path)", m.kind, m.id);
    assert_eq!(format!("{b:?}"), format!("{c:?}"), "{:?} {}: other mob state (old path)", m.kind, m.id);
    new
}

#[test]
fn nearest_mob_of_a_class_matches_the_walk_of_the_whole_list() {
    let (mut p, dir, _) = open_pen("nearest-of-class", 60);
    let s = &mut p.s;
    let mut classes: Vec<Who> = all_classes().into_iter().filter(|w| !matches!(w, Who::Exact(_))).collect();
    classes.extend([Kind::Llama, Kind::Cow, Kind::Ocelot, Kind::Guardian, Kind::Armadillo].map(Who::Exact));
    let mut seen: Vec<(Who, Seen)> = classes.iter().map(|&w| (w, Seen::default())).collect();
    for i in slots_of(s, |_| true) {
        let m = s.mobs.list.take(i).expect("listed mob");
        for (who, count) in &mut seen {
            // Targeting (from the eyes, wide box) and fleeing (from the feet, small flat box).
            count.note(compare_nearest(s, &m, *who, [16.0, 16.0, 16.0], true).is_some());
            count.note(compare_nearest(s, &m, *who, [2.5, 2.0, 2.5], false).is_some());
        }
        s.mobs.list.put(i, m);
    }
    let found: Vec<_> = seen.iter().filter(|(_, n)| n.0 > 0).map(|(w, _)| *w).collect();
    let searches: usize = seen.iter().map(|(_, n)| n.0 + n.1).sum();
    eprintln!("{searches} nearest-mob searches compared; classes with a mob found: {found:?}");
    for who in [Who::WolfPrey, Who::Zombie, Who::Skeleton, Who::FoxPrey, Who::Monster, Who::WitherPrey, Who::Exact(Kind::Llama)] {
        let n = &seen.iter().find(|(w, _)| *w == who).expect("class").1;
        assert!(n.both(), "{who:?}: found and not found both needed, seen {n:?}");
    }
    finish(p, dir);
}

// ---------- Parents and partners ----------

#[test]
fn parents_and_partners_match_the_walk_of_the_whole_list() {
    let (mut p, dir, _) = open_pen("parents-partners", 30);
    let s = &mut p.s;
    second_spring(s);
    let (mut parents, mut partners) = (Seen::default(), Seen::default());
    for i in slots_of(s, |m| m.kind.is_animal()) {
        let mut m = s.mobs.list.take(i).expect("listed mob");
        let parent = s.closest_adult_kin_among(&m, s.mobs.of_kinds(kin_species(m.kind)));
        assert_eq!(parent, s.closest_adult_kin_among(&m, s.mobs.list.iter()), "{:?} {}: other parent", m.kind, m.id);
        parents.note(parent.is_some());
        // A lover of every species, babies included (the search itself does not ask the age).
        m.in_love = 600;
        let (mut a, mut b) = (m.clone(), m.clone());
        let partner = s.free_partner_among(&mut a, s.mobs.of_kinds(partner_species(m.kind)));
        assert_eq!(partner, s.free_partner_among(&mut b, s.mobs.list.iter()), "{:?} {}: other partner", m.kind, m.id);
        assert_eq!(format!("{a:?}"), format!("{b:?}"), "{:?} {}: other mob state", m.kind, m.id);
        partners.note(partner.is_some());
        s.mobs.list.put(i, m);
    }
    eprintln!("parents {parents:?}, partners {partners:?}");
    assert!(parents.both() && partners.both(), "found and not found both needed");
    // Partners across species were found: a horse courts a donkey, a llama a trader's llama.
    let mut across = |a: Kind, b: Kind| {
        slots_of(s, |m| m.kind == a).into_iter().any(|i| {
            let mut m = s.mobs.list.take(i).expect("listed mob");
            m.in_love = 600;
            let partner = s.free_partner_among(&mut m, s.mobs.of_kinds(partner_species(a)));
            let found = partner.and_then(|id| s.mob_ref(id)).is_some_and(|o| o.kind == b);
            s.mobs.list.put(i, m);
            found
        })
    };
    assert!(across(Kind::Horse, Kind::Donkey) || across(Kind::Donkey, Kind::Horse), "no horse and donkey pair");
    assert!(across(Kind::Llama, Kind::TraderLlama) || across(Kind::TraderLlama, Kind::Llama), "no llama pair");
    finish(p, dir);
}

/// Two adults at the same distance: the baby follows the LAST of the list, the lover courts the
/// FIRST (the two rules of the game's searches).
#[test]
fn ties_keep_the_last_parent_and_the_first_partner() {
    let (mut p, dir) = open("search-ties");
    let s = &mut p.s;
    let at = |dx: f64| [40.5 + dx, LAND_Y, 0.5];
    let west = add(s, Kind::Cow, at(-4.0), |m| m.in_love = 600);
    let east = add(s, Kind::Cow, at(4.0), |m| m.in_love = 600);
    let calf = s.new_mob(Kind::Cow, at(0.0), 0.0);
    assert_eq!(s.closest_adult_kin_among(&calf, s.mobs.of_kinds(kin_species(Kind::Cow))), Some((16.0, east)));
    let mut lover = s.new_mob(Kind::Cow, at(0.0), 0.0);
    lover.in_love = 600;
    assert_eq!(s.free_partner_among(&mut lover, s.mobs.of_kinds(partner_species(Kind::Cow))), Some(west));
    // The baby llama follows a trader's llama, the calf does not.
    let trader_llama = add(s, Kind::TraderLlama, at(3.0), |_| {});
    let cria = s.new_mob(Kind::Llama, at(0.0), 0.0);
    assert_eq!(s.closest_adult_kin_among(&cria, s.mobs.of_kinds(kin_species(Kind::Llama))), Some((9.0, trader_llama)));
    assert_eq!(s.closest_adult_kin_among(&calf, s.mobs.of_kinds(kin_species(Kind::Cow))), Some((16.0, east)));
    finish(p, dir);
}

// ---------- Calls for help ----------

fn list_state(s: &Server) -> Vec<String> {
    s.mobs.list.iter().map(|m| format!("{m:?}")).collect()
}

fn restore_list(s: &mut Server, saved: &[Mob]) {
    for (m, old) in s.mobs.list.iter_mut().zip(saved) {
        *m = old.clone();
    }
}

/// A call for help in one pass leaves every mob of the list as the two passes did.
#[test]
fn calls_for_help_match_the_two_passes() {
    let (mut p, dir, cast) = open_pen("calls-for-help", 40);
    let s = &mut p.s;
    // Only a mob without a target answers a call: half of the pen is given none.
    s.mobs.list.iter_mut().filter(|m| m.id % 2 == 0).for_each(|m| m.target = None);
    let mut called_by: Vec<(Kind, usize)> = Vec::new();
    let callers = [Kind::Bee, Kind::Panda, Kind::Wolf, Kind::Zombie, Kind::Husk, Kind::ZombifiedPiglin, Kind::Cow, Kind::Skeleton];
    for kind in callers {
        for i in slots_of(s, |m| m.kind == kind) {
            let mut m = s.mobs.list.take(i).expect("listed mob");
            m.last_hurt_by_mob = Some((Ent::Mob(cast.aggressor), m.tick_count));
            let saved: Vec<Mob> = s.mobs.list.iter().cloned().collect();
            let before = list_state(s);
            s.alert_others_with(&m, true);
            let one_pass = list_state(s);
            restore_list(s, &saved);
            s.alert_others_with(&m, false);
            assert!(one_pass == list_state(s), "{kind:?} {}: the call for help left other mobs", m.id);
            let called = one_pass.iter().zip(&before).filter(|(a, b)| a != b).count();
            called_by.push((kind, called));
            restore_list(s, &saved);
            s.mobs.list.put(i, m);
        }
    }
    eprintln!("{} calls for help compared; mobs called per caller: {called_by:?}", called_by.len());
    let calls = |k: Kind| called_by.iter().filter(|c| c.0 == k).map(|c| c.1).collect::<Vec<_>>();
    for kind in [Kind::Bee, Kind::Panda, Kind::Wolf, Kind::Zombie, Kind::ZombifiedPiglin] {
        assert!(calls(kind).iter().any(|&n| n > 0), "{kind:?}: no mob ever called");
    }
    // The panda's rule refuses the pandas that are not aggressive: never the whole species.
    let pandas = slots_of(s, |m| m.kind == Kind::Panda).len();
    assert!(calls(Kind::Panda).iter().all(|&n| n < pandas - 1), "pandas called: {:?} of {pandas}", calls(Kind::Panda));
    finish(p, dir);
}

// ---------- Melee reach and seats ----------

fn within_melee_both_ways(s: &Server, m: &Mob, t: Ent) -> bool {
    let early = s.within_melee_with(m, t, true);
    assert_eq!(early, s.within_melee_with(m, t, false), "{:?} {} against {t:?}: the early exit changed the answer", m.kind, m.id);
    early
}

fn seat_both_ways(s: &Server, t: Ent) -> Option<f64> {
    let seat = s.seat_height_by_vehicle_link(t);
    assert_eq!(seat, s.seat_height_by_passenger_walk(t), "seat of {t:?}: link and walk disagree");
    seat
}

#[test]
fn melee_reach_and_seats_match_the_old_path() {
    let (mut p, dir, _) = open_pen("melee-reach", 60);
    let s = &mut p.s;
    let targets: Vec<Ent> = s.mobs.list.iter().map(|m| Ent::Mob(m.id)).chain([Ent::Player]).collect();
    let seats = targets.iter().filter(|&&t| seat_both_ways(s, t).is_some()).count();
    assert!(seats >= 2, "mounted targets: {seats}");
    let mut reach = Seen::default();
    for i in slots_of(s, |_| true) {
        let mut m = s.mobs.list.take(i).expect("listed mob");
        for &t in &targets {
            reach.note(within_melee_both_ways(s, &m, t));
        }
        // The same attacker with a long weapon that has a minimum reach.
        let range = rc_proto::nbt::Nbt::Compound(vec![
            ("min_reach".into(), rc_proto::nbt::Nbt::Float(1.5)),
            ("max_reach".into(), rc_proto::nbt::Nbt::Float(4.0)),
        ]);
        let components = rc_proto::nbt::Nbt::Compound(vec![("minecraft:attack_range".into(), range)]);
        m.equipment[0] = Some(Stack { id: "minecraft:stick".into(), count: 1, components: Some(components) });
        for &t in &targets {
            reach.note(within_melee_both_ways(s, &m, t));
        }
        m.equipment[0] = None;
        s.mobs.list.put(i, m);
    }
    eprintln!("melee reach: {reach:?}, mounted targets {seats}");
    assert!(reach.both(), "in reach and out of reach both needed");
    finish(p, dir);
}

/// A mounted player has the bottom of its box raised to its seat: a baby zombie at its feet,
/// which hit it on foot, no longer reaches it. Early exit or not, same answers.
#[test]
fn seat_of_a_mounted_player_puts_it_out_of_a_small_attacker_reach() {
    let (mut p, dir) = open("melee-seat");
    let s = &mut p.s;
    s.game_mode = 0;
    let feet = s.pos;
    let mut small = s.new_mob(Kind::Zombie, [feet[0] + 0.7, feet[1], feet[2]], 0.0);
    small.baby = true;
    let head = small.bbox().max[1];
    assert!(head < feet[1] + 1.2, "a baby zombie is small: head at {head}");
    assert!(within_melee_both_ways(s, &small, Ent::Player), "on foot: in reach");
    let mut horse = s.new_mob(Kind::Horse, feet, 0.0);
    assert!(s.player_start_riding(&mut horse), "mounting");
    s.add_mob(horse);
    // The rider's own box still reaches down to the baby zombie: only its seat is above it.
    let seat = seat_both_ways(s, Ent::Player).expect("seat of the mounted player");
    assert!(s.player_aabb().min[1] < head && seat >= head, "box from {}, seat {seat}", s.player_aabb().min[1]);
    assert!(!within_melee_both_ways(s, &small, Ent::Player), "mounted: out of reach");
    finish(p, dir);
}

/// Faithful difference of the vehicle link: a mount that lists "the player" while the player
/// rides nothing — what a mounted LAN guest looks like from the host's side — gave the host that
/// seat on the old path; the link reads the host's own vehicle, like the game.
#[test]
fn seat_is_read_from_the_targets_own_vehicle() {
    let (mut p, dir) = open("melee-own-vehicle");
    let s = &mut p.s;
    let mut horse = s.new_mob(Kind::Horse, [20.5, LAND_Y + 30.0, 0.5], 0.0);
    horse.mount.passengers.push(Ent::Player);
    s.add_mob(horse);
    assert_eq!(s.mobs.mounts.riding.vehicle, None);
    assert!(s.seat_height_by_passenger_walk(Ent::Player).is_some(), "the walk takes the other rider's seat");
    assert_eq!(s.seat_height_by_vehicle_link(Ent::Player), None, "the player rides nothing");
    finish(p, dir);
}

// ---------- Goal selector ----------

fn kept_values_match(m: &Mob) -> usize {
    let goals = m.goals.goals.iter().chain(&m.targets.goals);
    goals.inspect(|w| assert_eq!(w.kept_values(), (w.goal.flags(), w.goal.every_tick()), "{:?}: {:?}", m.kind, w.goal)).count()
}

/// The flags kept beside each goal are those the goal computes: when it is added, for every
/// species, and after the goals have run.
#[test]
fn kept_goal_flags_are_those_the_goals_compute() {
    let (mut p, dir, _) = open_pen("goal-flags", 0);
    let mut fresh = 0;
    for kind in Kind::ALL {
        fresh += kept_values_match(&p.s.new_mob(kind, [40.5, LAND_Y, 0.5], 0.0));
    }
    assert!(fresh > 200, "goals of the {} species: {fresh}", Kind::ALL.len());
    p.s.game_mode = 1;
    game_ticks(&mut p, 120);
    let (mut running, mut all) = (0, 0);
    for m in p.s.mobs.list.iter() {
        all += kept_values_match(m);
        running += m.goals.goals.iter().chain(&m.targets.goals).filter(|w| w.running).count();
    }
    assert!(all > 1000 && running > 100, "goals of the pen: {all}, running {running}");
    // The kept values fit in the padding of the entry: it did not grow.
    assert_eq!(size_of::<crate::mobs::goals::Wrapped>(), size_of::<Goal>() + 8);
    finish(p, dir);
}

// ---------- Food held by the player ----------

#[test]
fn held_food_answers_match_with_and_without_the_memo() {
    let (mut p, dir) = open("held-food");
    let s = &mut p.s;
    let foods = [
        Food::Tag("minecraft:cow_food"),
        Food::Tag("minecraft:pig_food"),
        Food::Tag("minecraft:sheep_food"),
        Food::Tag("minecraft:chicken_food"),
        Food::Tag("minecraft:wolf_food"),
        Food::Tag("minecraft:cat_food"),
        Food::Tag("minecraft:not_a_tag"),
        Food::Item("minecraft:carrot_on_a_stick"),
        Food::Item("minecraft:bone"),
    ];
    let stacks = ["minecraft:wheat", "minecraft:carrot", "minecraft:bone", "minecraft:carrot_on_a_stick", "minecraft:cod", "minecraft:stone"];
    let hands: Vec<Option<Stack>> = std::iter::once(None).chain(stacks.iter().map(|id| held(id))).collect();
    let main_hand = 36 + s.held as usize;
    let mut answers = Seen::default();
    for tick in 0..3 {
        s.tick = 500 + tick;
        // The held stacks change inside one tick: the kept answers must follow them.
        for main in &hands {
            for off in &hands {
                (s.slots[main_hand], s.slots[45]) = (main.clone(), off.clone());
                for round in 0..2 {
                    for f in foods {
                        let kept = s.holding_with(f, true);
                        assert_eq!(kept, s.holding_with(f, false), "{f:?} with {main:?} and {off:?}, tick {tick}, round {round}");
                        answers.note(kept);
                    }
                }
            }
        }
    }
    assert!(answers.both(), "held and not held both needed: {answers:?}");
    // Wheat tempts the cow and the sheep, not the pig.
    (s.slots[main_hand], s.slots[45]) = (held("minecraft:wheat"), None);
    let tempted = [foods[0], foods[1], foods[2]].map(|f| s.holding_with(f, true));
    assert_eq!(tempted, [true, false, true]);
    finish(p, dir);
}

// ---------- Blocks ----------

#[test]
fn goal_blocks_are_the_blocks_of_their_names() {
    let (p, dir) = open("goal-blocks");
    for block in GoalBlock::ALL {
        let id = block.id(&p.s);
        assert_eq!(p.s.data.blocks.get(usize::from(id)).map(|b| b.name.as_str()), Some(block.name()), "{block:?}");
        let state = p.s.view().default_state(id);
        for other in GoalBlock::ALL {
            assert_eq!(p.s.state_is(state, other, true), other == block, "{block:?} is {other:?} by identifier");
            assert_eq!(p.s.state_is(state, other, false), other == block, "{block:?} is {other:?} by name");
        }
    }
    // A state outside the table is no block, by identifier as by name.
    assert!(GoalBlock::ALL.iter().all(|&b| !p.s.state_is(u32::MAX, b, true) && !p.s.state_is(u32::MAX, b, false)));
    finish(p, dir);
}

/// Targets of every "go to a block" goal, east of the player: turtle eggs (one covered), a pool,
/// a beach, a bed, a chest, a lit and a cold furnace, a carrot field (ripe and unripe).
fn build_block_targets(p: &mut Probe) {
    for c in [
        "setblock 50 -60 0 minecraft:turtle_egg",
        "setblock 44 -60 10 minecraft:turtle_egg",
        "setblock 60 -60 -5 minecraft:turtle_egg",
        "setblock 60 -59 -5 minecraft:stone",
        "fill 70 -61 -4 74 -61 4 minecraft:water",
        "fill 80 -61 -4 86 -61 4 minecraft:sand",
        "setblock 90 -60 0 minecraft:red_bed[part=foot,facing=north]",
        "setblock 90 -60 -1 minecraft:red_bed[part=head,facing=north]",
        "setblock 94 -60 2 minecraft:chest",
        "setblock 96 -60 2 minecraft:furnace[lit=true]",
        "setblock 98 -60 2 minecraft:furnace",
        "fill 100 -61 0 104 -61 4 minecraft:farmland",
        "fill 100 -60 0 104 -60 2 minecraft:carrots[age=7]",
        "fill 100 -60 3 104 -60 4 minecraft:carrots[age=3]",
    ] {
        cmd(p, c);
    }
}

/// A drawn search: the goal as its species registers it (or with drawn ranges), a mob around the
/// goal's targets one time in two and anywhere otherwise, at a drawn height, one time in four
/// with a residence zone.
fn drawn_search(s: &mut Server, r: &mut Draw) -> (SGoal, Mob) {
    // Kind, ranges of the species' goal, species, x of the kind's targets.
    let (kind, range, vrange, vstart, species, targets_x) = [
        (Mtb::RemoveEgg, 24, 3, 0, Kind::Zombie, 50.0),
        (Mtb::TurtleWater, 24, 1, -1, Kind::Turtle, 72.0),
        (Mtb::TurtleLayEgg, 16, 1, 0, Kind::Turtle, 83.0),
        (Mtb::CatLie, 8, 6, -2, Kind::Cat, 90.0),
        (Mtb::CatSit, 8, 1, 0, Kind::Cat, 96.0),
        (Mtb::RaidGarden, 16, 1, 0, Kind::Rabbit, 102.0),
    ][r.below(6)];
    let (range, vrange, vstart) = match r.below(3) {
        0 => (1 + r.below(24) as i32, r.below(7) as i32, r.below(5) as i32 - 3),
        _ => (range, vrange, vstart),
    };
    let mut goal = SGoal::move_to_block(kind, 1.0, range, vrange, vstart);
    if let SGoal::MoveToBlock { want_raid, .. } = &mut goal {
        *want_raid = true;
    }
    let reach = f64::from(range) * 1.2;
    let (x, z) = match r.below(2) {
        0 => (targets_x + r.within(-reach, reach), r.within(-reach, reach)),
        _ => (r.within(30.0, 120.0), r.within(-25.0, 25.0)),
    };
    let pos = [x, LAND_Y + [0.0, 0.0, 1.0, 2.0, -1.0, -2.0][r.below(6)], z];
    let mut m = s.new_mob(species, pos, 0.0);
    if r.below(4) == 0 {
        let center = [pos[0] as i32 + r.below(9) as i32 - 4, -60, pos[2] as i32 + r.below(9) as i32 - 4];
        m.sea.home = Some((center, 3 + r.below(20) as i32));
    }
    (goal, m)
}

/// 500 drawn searches: the box precheck and the identifiers give the block the plain walk by
/// names gave (same answer, same goal afterwards).
#[test]
fn block_searches_match_the_plain_walk() {
    let (mut p, dir) = open("block-searches");
    build_block_targets(&mut p);
    let s = &mut p.s;
    let mut r = Draw(0x1234_5678_9ABC_DEF1);
    let mut seen: [Seen; 6] = Default::default();
    for _ in 0..500 {
        let (goal, m) = drawn_search(s, &mut r);
        let search = |by_id: bool, precheck: bool| {
            let mut g = goal.clone();
            let set = GoalSwitches { block_ids: by_id, ..GoalSwitches::ALL_NEW };
            let found = with_switches(set, || s.mtb_find_with(&mut g, &m, precheck));
            (found, format!("{g:?}"))
        };
        let plain = search(false, false);
        for (by_id, precheck) in [(false, true), (true, false), (true, true)] {
            assert_eq!(search(by_id, precheck), plain, "identifiers {by_id}, precheck {precheck}: {goal:?} from {:?}", m.pos());
        }
        let SGoal::MoveToBlock { kind, .. } = goal else { unreachable!() };
        seen[kind as usize].note(plain.0);
    }
    eprintln!("searches per kind (found, not found): {seen:?}");
    assert!(seen.iter().all(Seen::both), "each kind of search must find and miss");
    finish(p, dir);
}

// ---------- Micro-benches (quiet machine; `--ignored --nocapture`) ----------

/// Fastest of seven rounds, in ns per item: `prepare` (not timed) builds the inputs of a round,
/// `run` (timed) uses them. The fastest round is the one the other work of the machine disturbed
/// the least.
fn fastest_round_ns<T>(items: usize, mut prepare: impl FnMut() -> T, mut run: impl FnMut(T) -> usize) -> f64 {
    let mut best = f64::INFINITY;
    let mut sink = 0;
    for _ in 0..7 {
        let input = prepare();
        let t = std::time::Instant::now();
        sink += run(input);
        best = best.min(t.elapsed().as_nanos() as f64 / items as f64);
    }
    std::hint::black_box(sink);
    best
}

/// `fastest_round_ns` of `reps` calls of `f` per round, in ns per call.
fn fastest_call_ns(reps: usize, mut f: impl FnMut() -> usize) -> f64 {
    fastest_round_ns(reps, || (), |()| (0..reps).map(|_| f()).sum())
}

/// Crowd at the density of the "crowd" scene: `animals` farm animals (a third babies, a third in
/// love) and `zombies` zombies, in the loaded part of the flat world.
fn populate_crowd(s: &mut Server, animals: usize, zombies: usize) {
    let mut r = Draw(0x5EED);
    let mut spot = move || [r.within(-28.0, 155.0), LAND_Y, r.within(-28.0, 44.0)];
    let kinds = [Kind::Cow, Kind::Sheep, Kind::Pig, Kind::Chicken];
    for i in 0..animals {
        add(s, kinds[i % 4], spot(), |m| family_state(i / 4, m));
    }
    for _ in 0..zombies {
        add(s, Kind::Zombie, spot(), |_| {});
    }
}

/// Parent, partner and target searches in a crowd of 1,600 animals and 100 zombies: per-species
/// walk against the walk of the whole list, one pass against candidates collected.
#[test]
#[ignore = "measurement bench"]
fn bench_neighbour_searches() {
    let (mut p, dir) = open("bench-neighbours");
    populate_crowd(&mut p.s, 1600, 100);
    crate::mobs::index::set_verify(false);
    let s = &p.s;
    let pick = |wanted: &dyn Fn(&Mob) -> bool| s.mobs.list.iter().filter(|m| wanted(m)).take(200).cloned().collect::<Vec<Mob>>();
    let (babies, lovers, zombies) = (pick(&|m| m.is_baby()), pick(&|m| m.in_love > 0), pick(&|m| m.kind == Kind::Zombie));
    // Each search works on its own copy of the mob (the copies are not timed): a search fills
    // the mob's sight cache, which would make the next one cheaper.
    let each = |mobs: &[Mob], search: &mut dyn FnMut(&mut Mob) -> bool| {
        fastest_round_ns(mobs.len(), || mobs.to_vec(), |mut copies| copies.iter_mut().map(|m| usize::from(search(m))).sum())
    };
    let parent_list = each(&babies, &mut |m| s.closest_adult_kin_among(m, s.mobs.list.iter()).is_some());
    let parent_species = each(&babies, &mut |m| s.closest_adult_kin_among(m, s.mobs.of_kinds(kin_species(m.kind))).is_some());
    eprintln!("bench parent search (N=1700): whole list {parent_list:.0} ns, per species {parent_species:.0} ns per baby");
    let partner_list = each(&lovers, &mut |m| s.free_partner_among(m, s.mobs.list.iter()).is_some());
    let partner_species_ns = each(&lovers, &mut |m| s.free_partner_among(m, s.mobs.of_kinds(partner_species(m.kind))).is_some());
    eprintln!("bench partner search (N=1700): whole list {partner_list:.0} ns, per species {partner_species_ns:.0} ns per lover");
    for (who, what) in [(Who::Villager, "absent species"), (Who::Zombie, "100 zombies"), (Who::Monster, "open class")] {
        let mut test = |s: &Server, m: &mut Mob, e: Ent| s.targeting_ok(m, e, true, 35.0, true, true);
        let far = [35.0; 3];
        let before = each(&zombies, &mut |m| nearest_by_whole_list_walk(s, m, who, far, true, &mut test).is_some());
        let collected = each(&zombies, &mut |m| s.nearest_of_with(m, who, far, true, &mut test, false).is_some());
        let one_pass = each(&zombies, &mut |m| s.nearest_of_with(m, who, far, true, &mut test, true).is_some());
        eprintln!("bench nearest {who:?} ({what}, N=1700): whole list {before:.0} ns, per species collected {collected:.0} ns, per species one pass {one_pass:.0} ns per search");
    }
    crate::mobs::index::set_verify(true);
    finish(p, dir);
}

/// Melee test of 100 zombies against a far iron golem among 1,000 animals: seat by passenger
/// walk or by vehicle link, with and without the early exit, bare hands and armed.
#[test]
#[ignore = "measurement bench"]
fn bench_melee_reach() {
    let (mut p, dir) = open("bench-melee");
    populate_crowd(&mut p.s, 1000, 100);
    let golem = add(&mut p.s, Kind::IronGolem, [60.5, LAND_Y, 0.5], |_| {});
    crate::mobs::index::set_verify(false);
    let s = &p.s;
    let zombies: Vec<Mob> = s.mobs.list.iter().filter(|m| m.kind == Kind::Zombie).cloned().collect();
    let mut armed = zombies.clone();
    armed.iter_mut().for_each(|m| m.equipment[0] = held("minecraft:iron_sword"));
    for (attackers, weapon) in [(&zombies, "bare hands"), (&armed, "iron sword")] {
        for (direct, early, what) in [
            (false, false, "passenger walk (before)"),
            (true, false, "vehicle link"),
            (false, true, "passenger walk + early exit"),
            (true, true, "vehicle link + early exit"),
        ] {
            let set = GoalSwitches { riding_seat_direct: direct, melee_early_exit: early, ..GoalSwitches::ALL_NEW };
            let in_reach = || attackers.iter().filter(|m| s.within_melee(m, Ent::Mob(golem))).count();
            let ns = with_switches(set, || fastest_call_ns(50, in_reach)) / attackers.len() as f64;
            eprintln!("bench melee reach (N=1101, far target, {weapon}): {what} {ns:.0} ns per test");
        }
    }
    crate::mobs::index::set_verify(true);
    finish(p, dir);
}

/// Control flags and "every tick" mark of every goal of 1,000 animals and 100 zombies: computed
/// against kept.
#[test]
#[ignore = "measurement bench"]
fn bench_goal_flags() {
    let (mut p, dir) = open("bench-goal-flags");
    populate_crowd(&mut p.s, 1000, 100);
    let s = &p.s;
    let goals = || s.mobs.list.iter().flat_map(|m| m.goals.goals.iter().chain(&m.targets.goals));
    let n = goals().count() as f64;
    let walk_only = fastest_call_ns(200, || goals().map(|w| w.prio as usize).sum()) / n;
    let computed = fastest_call_ns(200, || goals().map(|w| usize::from(w.goal.flags()) + usize::from(w.goal.every_tick())).sum()) / n;
    let kept = fastest_call_ns(200, || goals().map(|w| usize::from(w.kept_values().0) + usize::from(w.kept_values().1)).sum()) / n;
    eprintln!("bench goal flags ({n} goals): walk of the goals alone {walk_only:.2} ns, computed {computed:.2} ns, kept {kept:.2} ns per goal");
    finish(p, dir);
}

/// "Does the player hold this food?" asked by 1,000 animals of four species in one tick.
#[test]
#[ignore = "measurement bench"]
fn bench_held_food() {
    let (mut p, dir) = open("bench-held-food");
    let s = &mut p.s;
    let foods = ["minecraft:cow_food", "minecraft:sheep_food", "minecraft:pig_food", "minecraft:chicken_food"].map(Food::Tag);
    let main_hand = 36 + s.held as usize;
    for (what, main, off) in [
        ("empty hands", None, None),
        ("wheat", held("minecraft:wheat"), None),
        ("wheat and a shield", held("minecraft:wheat"), held("minecraft:shield")),
    ] {
        (s.slots[main_hand], s.slots[45]) = (main, off);
        let ask = |memo: bool| fastest_call_ns(200, || (0..1000).filter(|i| s.holding_with(foods[i % 4], memo)).count()) / 1000.0;
        let (computed, kept) = (ask(false), ask(true));
        eprintln!("bench held food ({what}): computed {computed:.1} ns, kept {kept:.1} ns per question");
    }
    finish(p, dir);
}

/// Turtle egg search of a zombie (range 24, 15,463 cells): plain walk by names, by identifiers,
/// with the box precheck; no egg in reach, then one egg 20 blocks away.
#[test]
#[ignore = "measurement bench"]
fn bench_block_goal_search() {
    let (mut p, dir) = open("bench-block-search");
    let zombie = p.s.new_mob(Kind::Zombie, [40.5, LAND_Y, 0.5], 0.0);
    let goal = SGoal::move_to_block(Mtb::RemoveEgg, 1.0, 24, 3, 0);
    for what in ["no egg", "an egg 20 blocks away"] {
        if what != "no egg" {
            cmd(&mut p, "setblock 60 -60 0 minecraft:turtle_egg");
        }
        let s = &p.s;
        for (by_id, precheck) in [(false, false), (true, false), (false, true), (true, true)] {
            let set = GoalSwitches { block_ids: by_id, ..GoalSwitches::ALL_NEW };
            let search = || usize::from(s.mtb_find_with(&mut goal.clone(), &zombie, precheck));
            let ns = with_switches(set, || fastest_call_ns(100, search));
            eprintln!("bench egg search ({what}): identifiers {by_id}, box precheck {precheck}: {:.1} µs per search", ns / 1000.0);
        }
    }
    finish(p, dir);
}
