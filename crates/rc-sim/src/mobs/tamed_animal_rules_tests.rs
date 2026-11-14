//! Tamed animals and the wolf's anger, as the game has them: a lead keeps a pet from following
//! (and teleporting to) its owner; the order to sit is saved and kept apart from the sitting
//! pose; a wild wolf grows angry at whatever it targets, and its anger is saved with its target
//! and taken up again after a reload. World tests run on a copy of the test world
//! (`RUSTCRAFT_SOLO_WORLD`, area 200..231); without it they are skipped.

use rc_proto::nbt::Nbt;

use super::goals::Goal;
use super::leash::{Anchor, Holder, Leash};
use super::mob::{Ent, Mob};
use super::species::Kind;
use super::tests_spawn_paths::open;
use crate::blocks::probe::Probe;

fn spawn(p: &mut Probe, kind: Kind, pos: [f64; 3]) -> i32 {
    let mut m = p.s.new_mob(kind, pos, 0.0);
    let mut group = super::spawn::Group::default();
    p.s.finalize_spawn(&mut m, super::spawn::Reason::SpawnEgg, &mut group);
    m.set_pos(pos);
    let id = m.id;
    p.s.add_mob(m);
    id
}

fn take(p: &mut Probe, id: i32) -> (usize, Box<Mob>) {
    let i = p.s.mob_index(id).expect("mob");
    (i, p.s.mobs.list.take(i).expect("mob"))
}

fn with(p: &mut Probe, id: i32, f: impl FnOnce(&mut Mob)) {
    let i = p.s.mob_index(id).expect("mob");
    f(p.s.mobs.list.get_mut(i).expect("mob"));
}

fn mob(p: &Probe, id: i32) -> &Mob {
    p.s.mob_ref(id).expect("mob")
}

fn world(tag: &str) -> Option<Probe> {
    let (mut p, _dir) = open(tag)?;
    p.s.pos = [216.5, -59.0, 202.5];
    p.s.life.vit.health = 20.0;
    p.s.life.vit.dead = false;
    Some(p)
}

/// A pet of `kind` tamed by the player, on the ground.
fn pet(p: &mut Probe, kind: Kind, pos: [f64; 3]) -> i32 {
    let id = spawn(p, kind, pos);
    let owner = p.s.uuid;
    with(p, id, |m| {
        m.tame = true;
        m.owner = Some(owner);
        m.on_ground = true;
    });
    id
}

fn saved_sitting(m: &Mob) -> Option<i64> {
    crate::nbtio::int(&m.save(), "Sitting")
}

/// The owner 15 blocks away: a free pet starts following, one on a lead does not. Fails on the
/// code before (the lead was not among the reasons to stay).
#[test]
fn a_pet_on_a_lead_does_not_follow_its_owner() {
    let Some(mut p) = world("lead-keeps-pet") else { return };
    let wolf = pet(&mut p, Kind::Wolf, [216.5, -59.0, 217.5]);
    let mut follow = Goal::FollowOwner { speed: 1.0, start: 10.0, stop: 2.0, recalc: 0, old_water: 0.0 };
    let (i, mut w) = take(&mut p, wolf);
    assert!(p.s.goal_can_use(&mut follow, &mut w), "free pet does not follow");
    w.leash = Some(Box::new(Leash { holder: Some(Holder::Player), anchor: Anchor::Uuid(0), angular: 0.0, home: None }));
    assert!(!p.s.goal_can_use(&mut follow, &mut w), "pet on a lead follows its owner");
    // A lead read back whose holder is not found yet holds it too.
    w.leash.as_mut().expect("lead").holder = None;
    assert!(!p.s.goal_can_use(&mut follow, &mut w), "pet on a pending lead follows its owner");
    p.s.mobs.list.put(i, w);
}

/// A tamed wolf whose owner is away sits through its sit goal without being told to: the pose
/// shows, nothing is saved. Fails on the code before (the goal set the order, saved `Sitting:1b`).
#[test]
fn sitting_with_the_owner_away_is_a_pose_not_an_order() {
    let Some(mut p) = world("sit-owner-away") else { return };
    let wolf = spawn(&mut p, Kind::Wolf, [216.5, -59.0, 212.5]);
    with(&mut p, wolf, |w| {
        w.tame = true;
        w.owner = Some(42);
        w.on_ground = true;
    });
    let (i, mut w) = take(&mut p, wolf);
    let mut sit = Goal::SitOrdered;
    assert!(p.s.goal_can_use(&mut sit, &mut w), "no sit with the owner away");
    p.s.goal_start(&mut sit, &mut w);
    assert!(w.sitting, "no sitting pose");
    assert_eq!(saved_sitting(&w), Some(0), "the pose was saved as an order");
    p.s.mobs.list.put(i, w);
}

/// A wolf told to sit whose sit goal is interrupted (deep water: floating takes over) loses the
/// pose only: the order is still saved and it does not go after its owner. Fails on the code
/// before (the interruption erased the order: saved `Sitting:0b`, following again).
#[test]
fn an_interrupted_sit_keeps_the_order() {
    let Some(mut p) = world("sit-interrupted") else { return };
    let wolf = pet(&mut p, Kind::Wolf, [216.5, -59.0, 217.5]);
    let (i, mut w) = take(&mut p, wolf);
    w.ordered_to_sit = true;
    let mut sit = Goal::SitOrdered;
    p.s.goal_start(&mut sit, &mut w);
    p.s.goal_stop(&mut sit, &mut w);
    assert!(!w.sitting, "pose kept after the stop");
    assert_eq!(saved_sitting(&w), Some(1), "the order was erased");
    let mut follow = Goal::FollowOwner { speed: 1.0, start: 10.0, stop: 2.0, recalc: 0, old_water: 0.0 };
    assert!(!p.s.goal_can_use(&mut follow, &mut w), "told to sit, yet it follows its owner");
    p.s.mobs.list.put(i, w);
}

/// A hit makes a sitting wolf forget its order; a cat keeps its order. Fails on the code before
/// (the cat's order was erased too).
#[test]
fn a_hit_ends_a_wolfs_sit_order_but_not_a_cats() {
    let Some(mut p) = world("sit-order-hurt") else { return };
    for (kind, keeps) in [(Kind::Wolf, false), (Kind::Cat, true)] {
        let id = pet(&mut p, kind, [212.5, -59.0, 212.5]);
        let (i, mut m) = take(&mut p, id);
        m.ordered_to_sit = true;
        m.sitting = true;
        assert!(p.s.mob_hurt(&mut m, "minecraft:generic", 1.0, None, None), "{kind:?} not hurt");
        assert_eq!(saved_sitting(&m), Some(i64::from(keeps)), "{kind:?}: order after the hit");
        p.s.mobs.list.put(i, m);
    }
}

/// A wild wolf that targets a sheep grows angry at it for 20 to 39 s, after its AI step. Fails on
/// the code before (only a player's hit ever angered a wolf).
#[test]
fn a_wolf_grows_angry_at_its_target() {
    let Some(mut p) = world("wolf-anger-target") else { return };
    let wolf = spawn(&mut p, Kind::Wolf, [216.5, -59.0, 212.5]);
    let sheep = spawn(&mut p, Kind::Sheep, [216.5, -59.0, 214.5]);
    with(&mut p, wolf, |w| w.target = Some(Ent::Mob(sheep)));
    p.s.mobs_world_tick();
    let w = mob(&p, wolf);
    assert_eq!(w.anger_target, Some(Ent::Mob(sheep)), "anger target");
    assert_eq!(w.anger_uuid, Some(mob(&p, sheep).uuid), "anger reference");
    let left = w.anger_end - p.s.time;
    assert!((400..=780).contains(&left), "anger time {left}");
}

/// The anger of a wolf ends with the death of the mob it was angry at, and against a player in
/// creative mode.
#[test]
fn a_wolfs_anger_ends_with_its_target() {
    let Some(mut p) = world("wolf-anger-ends") else { return };
    let wolf = spawn(&mut p, Kind::Wolf, [216.5, -59.0, 212.5]);
    let sheep = spawn(&mut p, Kind::Sheep, [216.5, -59.0, 214.5]);
    let (i, mut w) = take(&mut p, wolf);
    w.target = Some(Ent::Mob(sheep));
    super::persistent_anger::update(&p.s, &mut w);
    assert!(w.is_angry(p.s.time), "not angry at its target");
    p.s.mobs.list.put(i, w);
    with(&mut p, sheep, |s| s.health = 0.0);
    let (i, mut w) = take(&mut p, wolf);
    super::persistent_anger::update(&p.s, &mut w);
    assert_eq!((w.anger_end, w.anger_target, w.target), (-1, None, None), "anger outlives its dead target");
    w.target = Some(Ent::Player);
    super::persistent_anger::update(&p.s, &mut w);
    assert!(w.is_angry(p.s.time), "not angry at the player");
    p.s.game_mode = 1;
    super::persistent_anger::update(&p.s, &mut w);
    assert_eq!(w.anger_end, -1, "anger kept against a creative player");
    p.s.mobs.list.put(i, w);
}

/// The wolf's anger is saved like the game's: the end always (-1 for none), the target's UUID
/// when there is one. Fails on the code before (no end written without anger, never a target).
#[test]
fn a_wolfs_anger_is_saved_with_its_target() {
    let Some(mut p) = world("wolf-anger-save") else { return };
    let wolf = spawn(&mut p, Kind::Wolf, [216.5, -59.0, 212.5]);
    let sheep = spawn(&mut p, Kind::Sheep, [216.5, -59.0, 214.5]);
    let calm = mob(&p, wolf).save();
    assert_eq!(crate::nbtio::int(&calm, "anger_end_time"), Some(-1), "calm wolf: end not written");
    assert!(calm.get("angry_at").is_none(), "calm wolf: a target written");
    with(&mut p, wolf, |w| w.target = Some(Ent::Mob(sheep)));
    p.s.mobs_world_tick();
    let angry = mob(&p, wolf).save();
    assert_eq!(crate::nbtio::uuid(angry.get("angry_at")), Some(mob(&p, sheep).uuid), "target not saved");
    assert!(crate::nbtio::int(&angry, "anger_end_time").is_some_and(|t| t > p.s.time), "end not saved");
}

/// A wolf read back angry at a sheep takes the sheep as its target again at its first tick.
/// Fails on the code before (the target was not read: the wolf went back to peace).
#[test]
fn a_reloaded_wolf_takes_up_its_anger_target() {
    let Some(mut p) = world("wolf-anger-reload") else { return };
    let wolf = spawn(&mut p, Kind::Wolf, [216.5, -59.0, 212.5]);
    let sheep = spawn(&mut p, Kind::Sheep, [216.5, -59.0, 214.5]);
    let sheep_uuid = mob(&p, sheep).uuid;
    let end = p.s.time + 600;
    with(&mut p, wolf, |w| {
        let mut e = w.save();
        crate::nbtio::set(&mut e, "angry_at", crate::nbtio::uuid_nbt(sheep_uuid));
        crate::nbtio::set(&mut e, "anger_end_time", Nbt::Long(end));
        w.load(&e);
    });
    assert_eq!(mob(&p, wolf).target, None, "target before the first tick");
    p.s.mobs_world_tick();
    assert_eq!(mob(&p, wolf).target, Some(Ent::Mob(sheep)), "anger target not taken up");
    assert_eq!(mob(&p, wolf).anger_target, Some(Ent::Mob(sheep)));
}
