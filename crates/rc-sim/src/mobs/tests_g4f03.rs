//! G4-F03: goal and sensor AI (wolf, breeding, panic, cat, villager). "World" tests
//! on the copy of the test world (`RUSTCRAFT_SOLO_WORLD`, zone 200..231); absent: tests skipped.

use rc_client::Event;
use rc_proto::nbt::Nbt;
use rc_proto_v777::entity::Meta;

use super::effects::Effect;
use super::goals::surface_goals::SGoal;
use super::goals::{Food, Goal, Melee, TBase, Who};
use super::jrand::JRand;
use super::mob::{Ent, Mob, MoveOp};
use super::species::Kind;
use super::villager::behave::{LookKind, Trig};
use super::villager::brain::Track;
use crate::blocks::probe::{Probe, state_string};
use crate::player::Stack;

fn probe(tag: &str) -> Option<Probe> {
    let (world, data) = (crate::tests::test_world(), rc_testdata::version_data());
    let (Some(world), Some(data)) = (world.clone(), data) else {
        eprintln!("test world {world:?} or data missing: test skipped");
        return None;
    };
    let dir = super::tests::prep(&world, tag, 0, "normal", &[]);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run("fill 200 -60 200 231 -60 231 minecraft:stone\nfill 200 -59 200 231 -40 231 minecraft:air\nplayer 216.5 -59 202.5 0 0\n");
    if state_string(&p.s, p.s.get([216, -60, 216])) != "minecraft:stone" {
        eprintln!("zone 200..231 missing from the copy");
        return None;
    }
    p.s.game_mode = 0;
    p.s.loaded = true;
    p.s.life.vit.health = 20.0;
    p.s.life.vit.dead = false;
    Some(p)
}

fn spawn(p: &mut Probe, k: Kind, pos: [f64; 3]) -> i32 {
    let mut m = p.s.new_mob(k, pos, 0.0);
    let mut g = super::spawn::Group::default();
    p.s.finalize_spawn(&mut m, super::spawn::Reason::SpawnEgg, &mut g);
    m.pos = pos;
    let id = m.id;
    p.s.add_mob(m);
    id
}

/// Mob taken out of the list (as during its tick).
fn take(p: &mut Probe, id: i32) -> (usize, Box<Mob>) {
    let i = p.s.mob_index(id).expect("mob");
    (i, p.s.mobs.list[i].take().expect("mob"))
}

fn with(p: &mut Probe, id: i32, f: impl FnOnce(&mut Mob)) {
    let i = p.s.mob_index(id).expect("mob");
    f(p.s.mobs.list[i].as_mut().expect("mob"));
}

// ---------- Without a world ----------

/// Sitting wolf: head tilted at most 20 degrees; standing, 40.
#[test]
fn sitting_wolf_head() {
    let mut w = Mob::new(Kind::Wolf, 1, 1, [0.0; 3], 0.0, JRand::new(1));
    assert_eq!(super::goals::max_head_x(&w), 40.0);
    w.sitting = true;
    assert_eq!(super::goals::max_head_x(&w), 20.0);
    let mut c = Mob::new(Kind::Cat, 2, 1, [0.0; 3], 0.0, JRand::new(1));
    c.sitting = true;
    assert_eq!(super::goals::max_head_x(&c), 40.0, "only the wolf lowers its head when sitting");
}

// ---------- World ----------

/// Target designated by the owner: never a creeper, nor a tamed equine, nor a tamed animal,
/// nor a wolf of the same owner; retaliation requires a recent hit received outside the exemption.
#[test]
fn wolf_filters_the_owners_target() {
    let Some(mut p) = probe("g4f03-wolf") else { return };
    let uuid = p.s.uuid;
    let w = spawn(&mut p, Kind::Wolf, [216.5, -59.0, 206.5]);
    let creeper = spawn(&mut p, Kind::Creeper, [219.5, -59.0, 206.5]);
    let zombie = spawn(&mut p, Kind::Zombie, [213.5, -59.0, 206.5]);
    let horse = spawn(&mut p, Kind::Horse, [216.5, -59.0, 210.5]);
    let cat = spawn(&mut p, Kind::Cat, [210.5, -59.0, 210.5]);
    let other = spawn(&mut p, Kind::Wolf, [222.5, -59.0, 210.5]);
    // Filter.
    assert!(!p.s.wolf_wants_to_attack(Ent::Mob(creeper)));
    assert!(p.s.wolf_wants_to_attack(Ent::Mob(zombie)));
    assert!(p.s.wolf_wants_to_attack(Ent::Mob(horse)), "wild horse spared");
    with(&mut p, horse, |h| h.mount.set_flag(super::mounts::FLAG_TAME, true));
    assert!(!p.s.wolf_wants_to_attack(Ent::Mob(horse)), "tamed horse targeted");
    assert!(p.s.wolf_wants_to_attack(Ent::Mob(cat)));
    with(&mut p, cat, |c| c.tame = true);
    assert!(!p.s.wolf_wants_to_attack(Ent::Mob(cat)), "tamed cat targeted");
    assert!(p.s.wolf_wants_to_attack(Ent::Mob(other)), "wild wolf spared");
    with(&mut p, other, |o| {
        o.tame = true;
        o.owner = Some(uuid);
    });
    assert!(!p.s.wolf_wants_to_attack(Ent::Mob(other)), "wolf of the same owner targeted");
    with(&mut p, other, |o| o.owner = Some(uuid ^ 1));
    assert!(p.s.wolf_wants_to_attack(Ent::Mob(other)), "wolf of another owner spared");

    let (i, mut wm) = take(&mut p, w);
    wm.tame = true;
    wm.owner = Some(uuid);
    // The owner hits a creeper: the wolf does not charge it; a zombie: yes.
    let mut g = Goal::OwnerHurt { base: TBase::new(false, false), stamp: 0, hit: None };
    p.s.mobs.player_hit = Some((Ent::Mob(creeper), 5));
    assert!(!p.s.goal_can_use(&mut g, &mut wm), "creeper charged");
    p.s.mobs.player_hit = Some((Ent::Mob(zombie), 6));
    assert!(p.s.goal_can_use(&mut g, &mut wm), "zombie ignored");
    // Retaliation: nothing without a recent hit received; ordinary hit yes; burning sulfur cube no.
    let mut g = Goal::OwnerHurtBy { base: TBase::new(false, false), stamp: 0, by: None };
    p.s.mobs.player_hurt_by = Some((Ent::Mob(zombie), 7));
    p.s.life.vit.last_damage = None;
    assert!(!p.s.goal_can_use(&mut g, &mut wm), "retaliation without a hit received");
    p.s.tick = 1000;
    let now = p.s.tick;
    p.s.life.vit.last_damage = Some(("minecraft:mob_attack".into(), 0.0, now));
    assert!(p.s.goal_can_use(&mut g, &mut wm), "retaliation refused");
    p.s.life.vit.last_damage = Some(("minecraft:sulfur_cube_hot".into(), 0.0, now));
    assert!(!p.s.goal_can_use(&mut g, &mut wm), "retaliation despite the exemption");
    // 100-tick bound, on both sides.
    p.s.life.vit.last_damage = Some(("minecraft:mob_attack".into(), 0.0, now - 100));
    assert!(p.s.goal_can_use(&mut g, &mut wm), "100-tick hit forgotten");
    p.s.life.vit.last_damage = Some(("minecraft:mob_attack".into(), 0.0, now - 101));
    assert!(!p.s.goal_can_use(&mut g, &mut wm), "hit of more than 100 ticks retained");

    // Zombie hit fully absorbed (golden apple): it still counts, the wolf retaliates.
    p.s.life.vit.last_damage = None;
    p.s.life.vit.cooldown = 0;
    p.s.life.fx.absorption = 8.0;
    p.s.mobs.player_ticks = 50; // fresh timestamp for the goal
    let zm = p.s.mobs.list[p.s.mob_index(zombie).expect("zombie")].as_deref().expect("zombie").clone();
    assert!(p.s.player_hurt_by_mob(&zm, "minecraft:mob_attack", 3.0, None), "absorbed hit refused");
    assert_eq!(p.s.life.vit.health, 20.0, "hit not absorbed");
    assert!(p.s.goal_can_use(&mut g, &mut wm), "no retaliation after an absorbed hit");
    p.s.mobs.list[i] = Some(wm);
}

/// Wolves and cats: mating only when tamed; the wolf refuses a sitting partner.
#[test]
fn wild_wolves_and_cats_do_not_breed() {
    let Some(mut p) = probe("g4f03-breeding") else { return };
    for k in [Kind::Wolf, Kind::Cat] {
        let a = spawn(&mut p, k, [210.5, -59.0, 220.5]);
        let b = spawn(&mut p, k, [211.5, -59.0, 220.5]);
        with(&mut p, b, |o| o.in_love = 600);
        let (i, mut am) = take(&mut p, a);
        am.in_love = 600;
        let mut g = Goal::Breed { speed: 1.0, partner: None, love_time: 0 };
        assert!(!p.s.goal_can_use(&mut g, &mut am), "{k:?} wild ones mated");
        am.tame = true;
        assert!(!p.s.goal_can_use(&mut g, &mut am), "{k:?}: wild partner accepted");
        p.s.mobs.list[i] = Some(am);
        with(&mut p, b, |o| o.tame = true);
        let (i, mut am) = take(&mut p, a);
        assert!(p.s.goal_can_use(&mut g, &mut am), "{k:?} tamed ones refused");
        p.s.mobs.list[i] = Some(am);
        with(&mut p, b, |o| o.sitting = true);
        let (i, mut am) = take(&mut p, a);
        let ok = p.s.goal_can_use(&mut g, &mut am);
        assert_eq!(ok, k == Kind::Cat, "{k:?}: sitting partner");
        p.s.mobs.list[i] = Some(am);
        for id in [a, b] {
            with(&mut p, id, |o| o.removed = true);
        }
    }
}

/// Partner searched at 8 blocks (distance² 64): diagonally at 7.7 it is out of range.
#[test]
fn partner_at_eight_blocks() {
    let Some(mut p) = probe("g4f03-range") else { return };
    let a = spawn(&mut p, Kind::Cow, [210.5, -59.0, 210.5]);
    let b = spawn(&mut p, Kind::Cow, [217.5, -59.0, 217.5]);
    with(&mut p, b, |o| o.in_love = 600);
    let (i, mut am) = take(&mut p, a);
    am.in_love = 600;
    let mut g = Goal::Breed { speed: 1.0, partner: None, love_time: 0 };
    assert!(!p.s.goal_can_use(&mut g, &mut am), "partner at 9.9 blocks accepted");
    p.s.mobs.list[i] = Some(am);
    with(&mut p, b, |o| o.pos = [215.5, -59.0, 215.5]);
    let (_, mut am) = take(&mut p, a);
    assert!(p.s.goal_can_use(&mut g, &mut am), "partner at 7.1 blocks refused");
}

/// Panic while on fire: water searched in increasing Manhattan distance over the whole 5/1/5 box
/// (corner at 10 included), positive z before its mirror.
#[test]
fn panic_looks_for_water_like_the_game() {
    let Some(mut p) = probe("g4f03-water") else { return };
    let c = spawn(&mut p, Kind::Cow, [216.5, -59.0, 216.5]);
    let (_, cm) = take(&mut p, c);
    p.run("setblock 221 -59 221 minecraft:water\n");
    assert_eq!(p.s.look_for_water(&cm, 5), Some([221, -59, 221]), "corner of the box ignored");
    p.run("setblock 216 -59 217 minecraft:water\nsetblock 216 -59 215 minecraft:water\n");
    assert_eq!(p.s.look_for_water(&cm, 5), Some([216, -59, 217]), "z order");
    p.run("setblock 215 -59 216 minecraft:water\n");
    assert_eq!(p.s.look_for_water(&cm, 5), Some([215, -59, 216]), "increasing x first");
}

/// Universal anger: aggressor and target forgotten, timer restarted (20 to 39 s), fellows too.
#[test]
fn universal_anger_end() {
    let Some(mut p) = probe("g4f03-anger") else { return };
    crate::nbtio::set(&mut p.s.rules, "minecraft:universal_anger", Nbt::Byte(1));
    let a = spawn(&mut p, Kind::Wolf, [210.5, -59.0, 210.5]);
    let b = spawn(&mut p, Kind::Wolf, [213.5, -59.0, 210.5]);
    with(&mut p, b, |o| {
        o.target = Some(Ent::Player);
        o.last_hurt_by_mob = Some((Ent::Player, 3));
    });
    let (i, mut am) = take(&mut p, a);
    am.last_hurt_by_mob = Some((Ent::Player, 5));
    am.target = Some(Ent::Player);
    am.anger_target = Some(Ent::Player);
    let mut g = Goal::ResetAnger { stamp: 0, alert: true };
    assert!(p.s.goal_can_use(&mut g, &mut am));
    p.s.goal_start(&mut g, &mut am);
    let t = p.s.time;
    assert!(am.last_hurt_by_mob.is_none() && am.target.is_none() && am.anger_target.is_none());
    assert!((t + 400..=t + 780).contains(&am.anger_end), "timer {} (time {t})", am.anger_end);
    assert!(!p.s.goal_can_use(&mut g, &mut am), "restarted without a new hit");
    p.s.mobs.list[i] = Some(am);
    let bi = p.s.mob_index(b).unwrap();
    let bm = p.s.mobs.list[bi].as_ref().unwrap();
    assert!(bm.target.is_none() && bm.last_hurt_by_mob.is_none(), "fellow not warned");
    assert!((t + 400..=t + 780).contains(&bm.anger_end));
}

/// Flee: the nearest is measured from the feet (targeting, however, from the eyes).
#[test]
fn flee_measured_from_the_feet() {
    let Some(mut p) = probe("g4f03-flee") else { return };
    let cr = spawn(&mut p, Kind::Creeper, [216.5, -59.0, 216.5]);
    // A: 2 blocks on the ground (feet 4, eyes 4 + 1.445²); B: 2.3 blocks at eye height (feet
    // 5.29 + 1.445², eyes 5.29).
    let a = spawn(&mut p, Kind::Cat, [218.5, -59.0, 216.5]);
    let b = spawn(&mut p, Kind::Cat, [216.5, -59.0, 218.8]);
    with(&mut p, b, |o| o.pos[1] = -59.0 + f64::from(1.7f32 * 0.85f32));
    let (_, mut cm) = take(&mut p, cr);
    let mut g = Goal::Avoid { who: Who::Cat, max: 6.0, walk: 1.0, sprint: 1.2, from: None, path: None };
    let _ = p.s.goal_can_use(&mut g, &mut cm);
    let Goal::Avoid { from, .. } = g else { unreachable!() };
    assert_eq!(from, Some(Ent::Mob(a)), "nearest measured from the eyes");
}

/// Cat: tempting only applies untamed; crouching posture at 0.6, sprint at 1.33.
#[test]
fn cat_tempt_and_posture() {
    let Some(mut p) = probe("g4f03-cat") else { return };
    let held = 36 + p.s.held as usize;
    p.s.slots[held] = Some(Stack { id: "minecraft:cod".into(), count: 1, components: None });
    let c = spawn(&mut p, Kind::Cat, [216.5, -59.0, 206.5]);
    let (i, mut cm) = take(&mut p, c);
    let tempt = || Goal::Tempt {
        speed: 0.6,
        food: Food::Tag("minecraft:cat_food"),
        scare: true,
        stop: 2.5,
        calm: 0,
        p: [0.0; 3],
        prot: [0.0; 2],
        on: false,
    };
    assert!(p.s.goal_can_use(&mut tempt(), &mut cm), "wild cat not tempted");
    cm.tame = true;
    assert!(!p.s.goal_can_use(&mut tempt(), &mut cm), "tamed cat tempted");
    // Posture.
    cm.move_ctl.op = MoveOp::MoveTo;
    cm.move_ctl.speed_mod = 0.6;
    super::surface::custom_ai_step(&mut p.s, &mut cm);
    cm.sync_data();
    assert!(cm.surf.crouching && !cm.surf.sprinting);
    assert_eq!(cm.get(6), Some(&Meta::Pose(5)));
    cm.move_ctl.speed_mod = 1.33;
    super::surface::custom_ai_step(&mut p.s, &mut cm);
    cm.sync_data();
    assert!(!cm.surf.crouching && cm.surf.sprinting);
    assert_eq!(cm.get(6), Some(&Meta::Pose(0)));
    assert!(matches!(cm.get(0), Some(Meta::Byte(f)) if f & 8 != 0), "sprint flag missing");
    let base = cm.attrs.speed.base;
    assert!((cm.attrs.speed.value() - base * (1.0 + f64::from(0.3f32))).abs() < 1e-9);
    cm.move_ctl.op = MoveOp::Wait;
    super::surface::custom_ai_step(&mut p.s, &mut cm);
    assert!(!cm.surf.crouching && !cm.surf.sprinting);
    p.s.mobs.list[i] = Some(cm);
}

/// Trampled turtle egg: three egg crumbs per jump, twenty puffs on breaking.
#[test]
fn trampled_egg_with_particles() {
    let Some(mut p) = probe("g4f03-egg") else { return };
    p.run("setblock 216 -59 216 minecraft:turtle_egg\n");
    let z = spawn(&mut p, Kind::Zombie, [216.5, -59.0, 216.5]);
    let (_, mut zm) = take(&mut p, z);
    let reg = |p: &Probe, r: &str, n: &str| {
        p.s.data.registry(r).and_then(|x| x.entries.iter().position(|e| e == n)).expect("registry") as i32
    };
    let (item_kind, egg, poof) =
        (reg(&p, "minecraft:particle_type", "minecraft:item"), reg(&p, "minecraft:item", "minecraft:egg"), reg(&p, "minecraft:particle_type", "minecraft:poof"));
    p.take_events();
    p.s.egg_eat(&mut zm, [216, -59, 216], true, 1);
    let ev = p.take_events();
    let crumbs: Vec<_> = ev
        .iter()
        .filter_map(|e| match e {
            Event::Particles(lp) if lp.particle.kind == item_kind && lp.particle.item == egg => Some(lp.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(crumbs.len(), 1, "egg crumbs missing");
    assert_eq!(crumbs[0].count, 3);
    assert_eq!(crumbs[0].pos, [216.5, -58.3, 216.5]);
    assert!(crumbs[0].delta.iter().all(|d| d.abs() <= 0.04) && crumbs[0].speed[0] == 0.15);
    p.s.egg_eat(&mut zm, [216, -59, 216], true, 61);
    let ev = p.take_events();
    let puffs = ev.iter().filter(|e| matches!(e, Event::Particles(lp) if lp.particle.kind == poof && lp.count == 1)).count();
    assert_eq!(puffs, 20, "puffs on breaking");
    assert_eq!(state_string(&p.s, p.s.get([216, -59, 216])), "minecraft:air");
}

/// Begging: the sitting wolf looks with a max tilt of 20.
#[test]
fn sitting_wolf_begging() {
    let Some(mut p) = probe("g4f03-begging") else { return };
    let w = spawn(&mut p, Kind::Wolf, [216.5, -59.0, 206.5]);
    let (_, mut wm) = take(&mut p, w);
    wm.sitting = true;
    let mut g = Goal::Beg { dist: 8.0, time: 10 };
    p.s.goal_tick(&mut g, &mut wm);
    assert_eq!(wm.look.x_max, 20.0);
}

/// Spider carrying a passenger: no melee attack.
#[test]
fn jockey_spider_no_melee() {
    let Some(mut p) = probe("g4f03-spider") else { return };
    // In contact with the player: within melee range even without a path.
    let s = spawn(&mut p, Kind::Spider, [216.5, -59.0, 204.0]);
    let z = spawn(&mut p, Kind::Skeleton, [216.5, -58.0, 204.0]);
    let (i, mut sm) = take(&mut p, s);
    sm.target = Some(Ent::Player);
    let t = p.s.time;
    let melee =
        || Goal::Melee { speed: 1.0, unseen: true, kind: Melee::Spider, path: None, pathed: [0.0; 3], recalc: 0, cd: 0, last: t - 100 };
    sm.mount.passengers.clear();
    assert!(p.s.goal_can_use(&mut melee(), &mut sm), "lone spider without melee");
    sm.mount.passengers.push(Ent::Mob(z));
    assert!(!p.s.goal_can_use(&mut melee(), &mut sm), "jockey in melee");
    p.s.mobs.list[i] = Some(sm);
}

/// Parrot: does not follow an invisible mob.
#[test]
fn parrot_ignores_invisible_mobs() {
    let Some(mut p) = probe("g4f03-parrot") else { return };
    let pr = spawn(&mut p, Kind::Parrot, [216.5, -59.0, 216.5]);
    let z = spawn(&mut p, Kind::Zombie, [219.5, -59.0, 216.5]);
    with(&mut p, z, |o| o.x.effects.push(Effect::new("minecraft:invisibility", 200, 0)));
    let (i, mut pm) = take(&mut p, pr);
    let mut g = Goal::Surf(SGoal::FollowMob { following: None, recalc: 0, old_water: 0.0 });
    assert!(!p.s.goal_can_use(&mut g, &mut pm), "invisible mob followed");
    p.s.mobs.list[i] = Some(pm);
    with(&mut p, z, |o| o.x.effects.clear());
    let (_, mut pm) = take(&mut p, pr);
    assert!(p.s.goal_can_use(&mut g, &mut pm), "visible mob ignored");
}

/// Baby happy ghast: panic aims at the air or water within the quarter turn of the look.
#[test]
fn baby_ghast_panics_in_the_air() {
    let Some(mut p) = probe("g4f03-ghast") else { return };
    let g = spawn(&mut p, Kind::HappyGhast, [216.5, -52.0, 216.5]);
    let (_, mut gm) = take(&mut p, g);
    gm.age = -24000;
    gm.rec.restriction = None;
    gm.pitch_o = 0.0;
    // Look towards +z (yaw 0) then towards −z (yaw 180): the point follows the look.
    for (yaw, ahead) in [(0.0f32, 1.0f64), (180.0, -1.0)] {
        gm.head_yaw_o = yaw;
        let mut found = 0;
        for seed in 0..30i64 {
            gm.rng = JRand::new(seed.wrapping_mul(0x5DEE_CE66_D1F3) ^ 0x2545_F491);
            if let Some(q) = p.s.bz_panic_pos(&mut gm) {
                found += 1;
                assert!((q[2] - 216.5) * ahead >= 0.0, "point outside the look {yaw}: {q:?}");
                assert!((q[1] - -52.0).abs() <= 5.0, "height outside ±4: {q:?}");
            }
        }
        assert!(found >= 25, "too few flee points: {found}");
    }
    // Home radius 3 around it: every point stays in the zone (shared helper).
    gm.rec.restriction = Some(([216, -52, 216], 3));
    let mut found = 0;
    for seed in 0..60i64 {
        gm.rng = JRand::new(seed.wrapping_mul(0x5DEE_CE66_D1F3) ^ 0x2545_F491);
        if let Some(q) = p.s.bz_panic_pos(&mut gm) {
            found += 1;
            let b = [q[0].floor() as i32, q[1].floor() as i32, q[2].floor() as i32];
            assert!(super::goals::within_home(&gm, b), "point outside the home: {q:?}");
        }
    }
    assert!(found >= 1, "no point in the home");
}

/// Sensors: the range of the conditions is that of the last sensor that ticked (shared setting).
#[test]
fn sensors_shared_range() {
    let Some(mut p) = probe("g4f03-sensors") else { return };
    let g = spawn(&mut p, Kind::Goat, [216.5, -59.0, 212.5]);
    let (_, mut gm) = take(&mut p, g);
    let b = gm.bm.brain.take().expect("brain");
    p.s.mobs.pig.sensor_range = 4.0;
    assert!(!p.s.bz_brain_targetable(&mut gm, &b, Ent::Player), "own range used");
    gm.sight.clear();
    p.s.mobs.pig.sensor_range = 16.0;
    assert!(p.s.bz_brain_targetable(&mut gm, &b, Ent::Player));
}

/// Villager: full look at aquatic categories and monsters.
#[test]
fn villager_looks_at_aquatic_mobs() {
    let Some(mut p) = probe("g4f03-look") else { return };
    let cases = [
        (Kind::Cow, 0u8),
        (Kind::Dolphin, 1),
        (Kind::Axolotl, 2),
        (Kind::GlowSquid, 3),
        (Kind::Cod, 4),
        (Kind::Zombie, 5),
    ];
    for (k, c) in cases {
        let id = spawn(&mut p, k, [210.5, -59.0, 220.5]);
        for other in 0..6u8 {
            assert_eq!(p.s.ent_is(Ent::Mob(id), LookKind::Category(other)), other == c, "{k:?} / category {other}");
        }
    }
}

/// Villager: goes to the item itself (look at its eyes), not out of the world border.
#[test]
fn villager_goes_to_the_item() {
    let Some(mut p) = probe("g4f03-item") else { return };
    let v = spawn(&mut p, Kind::Villager, [216.5, -59.0, 212.5]);
    p.s.spawn_item([217.5, -59.0, 212.5], [0.0; 3], Stack { id: "minecraft:bread".into(), count: 1, components: None }, 10);
    let id = p.s.life.drops.last().expect("item").id;
    let (i, mut vm) = take(&mut p, v);
    vm.can_pickup = true;
    let mut b = vm.npc.as_mut().and_then(|n| n.brain.take()).expect("brain");
    b.mem.wanted_item = Some(id);
    b.mem.walk = None;
    let t = p.s.time;
    assert!(p.s.trig(&mut vm, &mut b, &mut Trig::GoToWantedItem, t));
    assert_eq!(b.mem.walk.map(|w| w.target), Some(Track::Item(id, false)));
    assert_eq!(b.mem.look, Some(Track::Item(id, true)));
    let lp = p.s.track_pos(Track::Item(id, true)).unwrap();
    assert_eq!(lp[1], -59.0 + f64::from(0.2125f32));
    // Border reduced around the origin: the item is outside.
    b.mem.walk = None;
    p.s.cmd.border.center = [0.0, 0.0];
    p.s.cmd.border.size = 10.0;
    assert!(!p.s.trig(&mut vm, &mut b, &mut Trig::GoToWantedItem, t), "item outside the border targeted");
    if let Some(n) = vm.npc.as_mut() {
        n.brain = Some(b);
    }
    p.s.mobs.list[i] = Some(vm);
}

/// Item sensor: 32/16/32 box against the item's box (a bread 17.5 blocks above is seen).
#[test]
fn item_sensor_game_box() {
    let Some(mut p) = probe("g4f03-item-sensor") else { return };
    let v = spawn(&mut p, Kind::Villager, [216.5, -59.0, 212.5]);
    p.s.spawn_item([216.5, -41.5, 212.5], [0.0; 3], Stack { id: "minecraft:bread".into(), count: 1, components: None }, 10);
    let id = p.s.life.drops.last().expect("item").id;
    let (_, vm) = take(&mut p, v);
    assert_eq!(p.s.nearest_wanted_item(&vm, |st| p.s.villager_wants(&vm, st)), Some(id));
    // 18.1 blocks above: outside the box.
    let d = p.s.life.drops.last_mut().unwrap();
    d.pos[1] = -59.0 + 18.1;
    assert_eq!(p.s.nearest_wanted_item(&vm, |st| p.s.villager_wants(&vm, st)), None);
}

/// Talked to a villager: counted for an adult (even without an offer) and the trader, not a baby.
#[test]
fn talked_to_a_villager() {
    let Some(mut p) = probe("g4f03-talked") else { return };
    let key = "minecraft:talked_to_villager";
    let v = spawn(&mut p, Kind::Villager, [216.5, -59.0, 205.5]);
    let (i, mut vm) = take(&mut p, v);
    assert_eq!(p.s.npc_interact(&mut vm, ""), Some(true));
    assert_eq!(p.s.stat_value("minecraft:custom", key), 1);
    vm.age = -24000;
    assert_eq!(p.s.npc_interact(&mut vm, ""), Some(true));
    assert_eq!(p.s.stat_value("minecraft:custom", key), 1, "baby counted");
    p.s.mobs.list[i] = Some(vm);
    let t = spawn(&mut p, Kind::WanderingTrader, [218.5, -59.0, 205.5]);
    let (i, mut tm) = take(&mut p, t);
    p.s.npc_interact(&mut tm, "");
    assert_eq!(p.s.stat_value("minecraft:custom", key), 2, "trader not counted");
    p.s.mobs.list[i] = Some(tm);
}

/// Breeding: "animals bred" statistic for the fox, the goat and the nautilus; cause forgotten.
#[test]
fn breeding_counts_everywhere() {
    let Some(mut p) = probe("g4f03-stat-breeding") else { return };
    let key = "minecraft:animals_bred";
    let mut n = 0;
    for k in [Kind::Fox, Kind::Goat, Kind::Nautilus] {
        let a = spawn(&mut p, k, [210.5, -59.0, 214.5 + f64::from(n) * 4.0]);
        let b = spawn(&mut p, k, [211.5, -59.0, 214.5 + f64::from(n) * 4.0]);
        with(&mut p, b, |o| o.in_love = 600);
        let (i, mut am) = take(&mut p, a);
        am.in_love = 600;
        am.love_cause = true;
        match k {
            Kind::Goat => super::brainy::spawn_child(&mut p.s, &mut am, b),
            Kind::Nautilus => p.s.nautilus_breed(&mut am, b),
            _ => p.s.breed(&mut am, b),
        }
        n += 1;
        assert_eq!(p.s.stat_value("minecraft:custom", key), n, "{k:?} not counted");
        assert!(!am.love_cause && am.in_love == 0, "{k:?}: love not forgotten");
        p.s.mobs.list[i] = Some(am);
    }
}
