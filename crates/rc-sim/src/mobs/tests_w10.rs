//! Singleplayer AI (wave W10): attribute bounds, visibility, pursuit without a path, zombie goals,
//! flee, cats, enderman, villager raids, jockeys, melee range.
//! The "world" tests require the test world (`RUSTCRAFT_SOLO_WORLD`, zone 200..231).

use super::goals::surface_goals::{Mtb, SGoal};
use super::goals::{Goal, Who};
use super::illagers::goals::IGoal;
use super::jrand::JRand;
use super::mob::{Ent, Mob};
use super::nav::PathType;
use super::species::Kind;
use crate::blocks::probe::{Probe, state_string};
use crate::player::Stack;
use rc_proto::nbt::Nbt;

fn probe(tag: &str, difficulty: &str) -> Option<Probe> {
    let (world, data) = (crate::tests::test_world(), rc_testdata::version_data());
    let (Some(world), Some(data)) = (world.clone(), data) else {
        eprintln!("test world {world:?} or data missing: test skipped");
        return None;
    };
    let dir = super::tests::prep(&world, tag, 0, difficulty, &[]);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run("fill 200 -60 200 231 -60 231 minecraft:stone\nfill 200 -59 200 231 -45 231 minecraft:air\nplayer 216.5 -59 202.5 0 0\n");
    if state_string(&p.s, p.s.get([216, -60, 216])) != "minecraft:stone" {
        eprintln!("zone 200..231 missing from the copy");
        return None;
    }
    p.s.game_mode = 0;
    p.s.loaded = true;
    Some(p)
}

fn spawn(p: &mut Probe, k: Kind, pos: [f64; 3]) -> i32 {
    let mut m = p.s.new_mob(k, pos, 0.0);
    let mut g = super::spawn::Group::default();
    p.s.finalize_spawn(&mut m, super::spawn::Reason::SpawnEgg, &mut g);
    let id = m.id;
    p.s.add_mob(m);
    id
}

/// Mob taken out of the list (as during its tick).
fn take(p: &mut Probe, id: i32) -> (usize, Box<Mob>) {
    let i = p.s.mob_index(id).expect("mob");
    (i, p.s.mobs.list[i].take().expect("mob"))
}

// ---------- Without a world ----------

/// Value brought back within the registry bounds; NaN gives the minimum; default of the jump in
/// single precision.
#[test]
fn attribute_bounds() {
    let mut m = Mob::new(Kind::Zombie, 1, 1, [0.0; 3], 0.0, JRand::new(1));
    // Very strong slowness: −150 % of the total → speed 0, not negative.
    m.attrs.speed.set_mod("minecraft:effect.slowness", -1.5, 2);
    assert_eq!(m.attrs.speed.value(), 0.0);
    m.attrs.max_health.base = 0.0;
    assert_eq!(m.attrs.max_health.value(), 1.0);
    m.attrs.follow.base = 5000.0;
    assert_eq!(m.attrs.follow.value(), 2048.0);
    m.attrs.knockback_resistance.base = f64::NAN;
    assert_eq!(m.attrs.knockback_resistance.value(), -2.0);
    assert_eq!(m.attrs.jump.base, 0.41999998688697815);
    assert_eq!(m.attrs.gravity.value(), 0.08);
}

/// Zombie goals in the game's order: spear (2), melee (3), village crossing (6), and last
/// door breaking (1), active only with the ability; read from the NBT.
#[test]
fn zombie_goals_and_doors() {
    let mut z = Mob::new(Kind::Zombie, 1, 1, [0.0; 3], 0.0, JRand::new(1));
    let nbt = Nbt::Compound(vec![("CanBreakDoors".into(), Nbt::Byte(1))]);
    super::variants::load_extra(&mut z, &nbt);
    assert!(z.can_break_doors && z.nav.can_open_doors);
    let mut out = Nbt::Compound(Vec::new());
    super::variants::save_extra(&z, &mut out);
    assert_eq!(crate::nbtio::int(&out, "CanBreakDoors"), Some(1));
    super::brains::set_can_break_doors(&mut z, false);
    assert!(!z.can_break_doors && !z.nav.can_open_doors);
}

/// Villager packages: pre-raid (bell, meeting, return), raid (sky, celebration, hiding),
/// hiding (counter, search).
#[test]
fn villager_raid_packages() {
    use super::villager::behave::{Beh, Trig, packages};
    use super::villager::brain::{Act, Node};
    let v = packages(false, 0);
    let has = |a: Act, f: &dyn Fn(&Node) -> bool| v.iter().any(|(_, act, n)| *act == a && f(n));
    assert!(has(Act::PreRaid, &|n| matches!(n, Node::O { t: Trig::RingBell, .. })));
    assert!(has(Act::PreRaid, &|n| matches!(n, Node::O { t: Trig::ResetRaid, .. })));
    assert!(has(Act::Raid, &|n| matches!(n, Node::B { b: Beh::Celebrate { .. }, .. })));
    assert!(has(Act::Raid, &|n| matches!(n, Node::O { t: Trig::IfRaid { victory: false, .. }, .. })));
    assert!(has(Act::Hide, &|n| matches!(n, Node::O { t: Trig::SetHidden { stay: 300, close: 3, .. }, .. })));
    assert!(has(Act::Hide, &|n| matches!(n, Node::O { t: Trig::LocateHiding { radius: 32, close: 2, .. }, .. })));
}

// ---------- With a world ----------

/// Invisible player without armour: range × 0.07 (spotted at 2 blocks instead of 35); piglin
/// head: ×0.5 for the piglin, no effect on the zombie.
#[test]
fn visibility_invisible_and_heads() {
    let Some(mut p) = probe("w10vis", "normal") else { return };
    let z = spawn(&mut p, Kind::Zombie, [216.5, -59.0, 212.5]);
    let (i, mut zm) = take(&mut p, z);
    assert!(p.s.targeting_ok(&mut zm, Ent::Player, true, 35.0, false, true), "spotted without effect");
    p.s.give_player_effect("minecraft:invisibility", 600, 0);
    let v = p.s.visibility(Ent::Player, &zm);
    assert!((v - 0.7 * f64::from(0.1f32)).abs() < 1e-12, "visibility {v}");
    assert!(!p.s.targeting_ok(&mut zm, Ent::Player, true, 35.0, false, true), "invisible spotted at 10 blocks");
    // Four armour pieces: 0.7.
    for (slot, id) in [
        (5, "minecraft:iron_helmet"),
        (6, "minecraft:iron_chestplate"),
        (7, "minecraft:iron_leggings"),
        (8, "minecraft:iron_boots"),
    ] {
        p.s.slots[slot] = Some(Stack { id: id.into(), count: 1, components: None });
    }
    assert!((p.s.visibility(Ent::Player, &zm) - 0.7).abs() < 1e-9);
    p.s.mobs.list[i] = Some(zm);
    // Piglin head worn, without invisibility.
    p.s.life.fx = Default::default();
    for slot in 5..9 {
        p.s.slots[slot] = None;
    }
    p.s.slots[5] = Some(Stack { id: "minecraft:piglin_head".into(), count: 1, components: None });
    let pig = Mob::new(Kind::Piglin, 99, 99, [216.5, -59.0, 206.5], 0.0, JRand::new(3));
    let zom = Mob::new(Kind::Zombie, 98, 98, [216.5, -59.0, 206.5], 0.0, JRand::new(3));
    assert_eq!(p.s.visibility(Ent::Player, &pig), 0.5);
    assert_eq!(p.s.visibility(Ent::Player, &zom), 1.0);
}

/// Pursuit of an entity without a path: the old path stays followed.
#[test]
fn pursuit_without_path_keeps_the_trail() {
    let Some(mut p) = probe("w10nav", "normal") else { return };
    let z = spawn(&mut p, Kind::Zombie, [205.5, -59.0, 205.5]);
    let (i, mut zm) = take(&mut p, z);
    zm.on_ground = true;
    assert!(p.s.move_to_pos(&mut zm, [215.5, -59.0, 205.5], 1.0));
    let before = zm.nav.path.clone().expect("path");
    // Target in a missing chunk: no path possible.
    assert!(!p.s.move_to_entity(&mut zm, [100_000.5, -59.0, 0.5], 1.0));
    let after = zm.nav.path.clone().expect("path cleared");
    assert_eq!(format!("{after:?}"), format!("{before:?}"));
    p.s.mobs.list[i] = Some(zm);
}

/// Sulfur spike: caution like the stalactite (speleothem tag).
#[test]
fn sulfur_spike_is_cautious() {
    let Some(mut p) = probe("w10spike", "normal") else { return };
    p.run("setblock 210 -59 210 minecraft:sulfur_spike\nsetblock 212 -59 210 minecraft:pointed_dripstone\n");
    let z = spawn(&mut p, Kind::Zombie, [205.5, -59.0, 205.5]);
    let (i, zm) = take(&mut p, z);
    let view = p.s.mob_view(&zm);
    for q in [[210, -59, 210], [212, -59, 210]] {
        let t = super::goals::static_type_of(&p.s, &view, q);
        assert_eq!(t, PathType::DamageCautious, "{q:?}");
    }
    assert!(!p.s.pathfindable_land([210, -59, 210]));
    p.s.mobs.list[i] = Some(zm);
}

/// In Peaceful, the wild cat does not flee the player (combat conditions).
#[test]
fn wild_cat_does_not_flee_in_peaceful() {
    let Some(mut p) = probe("w10peace", "peaceful") else { return };
    let c = spawn(&mut p, Kind::Cat, [216.5, -59.0, 206.5]);
    let (i, mut cm) = take(&mut p, c);
    cm.on_ground = true;
    let mut g = Goal::Avoid { who: Who::Player, max: 16.0, walk: 0.8, sprint: 1.33, from: None, path: None };
    for _ in 0..20 {
        assert!(!p.s.goal_can_use(&mut g, &mut cm), "flee in Peaceful");
    }
    p.s.mobs.list[i] = Some(cm);
    // Control: in Normal, the same flee starts.
    let Some(mut p) = probe("w10peace2", "normal") else { return };
    let c = spawn(&mut p, Kind::Cat, [216.5, -59.0, 206.5]);
    let (_, mut cm) = take(&mut p, c);
    cm.on_ground = true;
    let mut g = Goal::Avoid { who: Who::Player, max: 16.0, walk: 0.8, sprint: 1.33, from: None, path: None };
    assert!((0..50).any(|_| p.s.goal_can_use(&mut g, &mut cm)), "no flee in Normal");
}

/// Following the owner: water costs nothing during the goal, then its penalty returns.
#[test]
fn owner_following_swims() {
    let Some(mut p) = probe("w10follow", "normal") else { return };
    let w = spawn(&mut p, Kind::Wolf, [216.5, -59.0, 220.5]);
    let (i, mut wm) = take(&mut p, w);
    let before = super::nav::mob_malus(&wm, PathType::Water);
    assert_eq!(before, 8.0);
    let mut g = Goal::FollowOwner { speed: 1.0, start: 10.0, stop: 2.0, recalc: 0, old_water: 0.0 };
    p.s.goal_start(&mut g, &mut wm);
    assert_eq!(super::nav::mob_malus(&wm, PathType::Water), 0.0);
    p.s.goal_stop(&mut g, &mut wm);
    assert_eq!(super::nav::mob_malus(&wm, PathType::Water), 8.0);
    p.s.mobs.list[i] = Some(wm);
}

/// Tamed cat: sits on a lit furnace (not an unlit one).
#[test]
fn cat_sits_on_lit_furnace() {
    let Some(mut p) = probe("w10cat", "normal") else { return };
    p.run("setblock 212 -59 210 minecraft:furnace[lit=false]\n");
    let c = spawn(&mut p, Kind::Cat, [210.5, -59.0, 210.5]);
    let (i, mut cm) = take(&mut p, c);
    cm.tame = true;
    let mut g = Goal::Surf(SGoal::move_to_block(Mtb::CatSit, 0.8, 8, 1, 0));
    assert!(!p.s.goal_can_use(&mut g, &mut cm), "unlit furnace accepted");
    p.s.mobs.list[i] = Some(cm);
    p.run("setblock 212 -59 210 minecraft:furnace[lit=true]\n");
    let (i, mut cm) = take(&mut p, c);
    let mut g = Goal::Surf(SGoal::move_to_block(Mtb::CatSit, 0.8, 8, 1, 0));
    assert!(p.s.goal_can_use(&mut g, &mut cm), "lit furnace refused");
    let Goal::Surf(SGoal::MoveToBlock { pos, .. }) = g else { unreachable!() };
    assert_eq!(pos, [212, -59, 210]);
    p.s.mobs.list[i] = Some(cm);
}

/// Enderman: takes a holdable block (default state carried), puts it down later; the carried block
/// is written and read back.
#[test]
fn enderman_takes_and_places() {
    let Some(mut p) = probe("w10ender", "normal") else { return };
    p.run("fill 210 -60 210 214 -60 214 minecraft:grass_block\n");
    let e = spawn(&mut p, Kind::Enderman, [212.5, -59.0, 212.5]);
    let (i, mut em) = take(&mut p, e);
    for _ in 0..400 {
        if em.carried.is_some() {
            break;
        }
        // Height drawn 0..3: the grass blocks are under the feet, the enderman goes down one block.
        em.pos[1] = -60.0;
        p.s.enderman_take_tick(&mut em);
    }
    let grass = p.s.default_state("minecraft:grass_block");
    assert_eq!(em.carried, Some(grass));
    let mut out = Nbt::Compound(Vec::new());
    p.s.enderman_save_carried(&em, &mut out);
    let mut back = em.clone();
    back.carried = None;
    p.s.enderman_load_carried(&mut back, &out);
    assert_eq!(back.carried, Some(grass));
    em.pos[1] = -59.0;
    for _ in 0..400 {
        if em.carried.is_none() {
            break;
        }
        p.s.enderman_leave_tick(&mut em);
    }
    assert!(em.carried.is_none(), "block never placed");
    p.s.mobs.list[i] = Some(em);
}

/// Jockeys: skeleton on spider; new chicken under a baby zombie.
#[test]
fn jockeys() {
    let Some(mut p) = probe("w10jockey", "normal") else { return };
    let mut sp = p.s.new_mob(Kind::Spider, [210.5, -59.0, 210.5], 0.0);
    p.s.spider_skeleton_jockey(&mut sp, super::spawn::Reason::SpawnEgg);
    let rider = sp.mount.passengers.first().copied();
    let Some(Ent::Mob(sk)) = rider else { panic!("no rider: {rider:?}") };
    let skm = p.s.mob_ref(sk).expect("skeleton added");
    assert_eq!((skm.kind, skm.mount.vehicle), (Kind::Skeleton, Some(sp.id)));
    let mut made = false;
    for _ in 0..3000 {
        let mut z = p.s.new_mob(Kind::Zombie, [220.5, -59.0, 220.5], 0.0);
        let mut g = super::spawn::Group { zombie_baby: Some(true), ..Default::default() };
        p.s.finalize_spawn(&mut z, super::spawn::Reason::SpawnEgg, &mut g);
        if let Some(v) = z.mount.vehicle {
            let c = p.s.mob_ref(v).expect("chicken");
            assert!(c.kind == Kind::Chicken && c.chicken_jockey);
            made = true;
            break;
        }
    }
    assert!(made, "no chicken jockey in 3000 attempts");
}

/// Melee range of the held item (`attack_range`): 2..4 blocks for the mob.
#[test]
fn item_melee_range() {
    let Some(mut p) = probe("w10reach", "normal") else { return };
    let z = spawn(&mut p, Kind::Zombie, [216.5, -59.0, 205.5]);
    let (i, mut zm) = take(&mut p, z);
    let comps = Nbt::Compound(vec![(
        "minecraft:attack_range".into(),
        Nbt::Compound(vec![("min_reach".into(), Nbt::Float(2.0)), ("max_reach".into(), Nbt::Float(4.0))]),
    )]);
    zm.equipment[0] = Some(Stack { id: "minecraft:stick".into(), count: 1, components: Some(comps) });
    // Player at 3 blocks (boxes 2.4 apart): within range; right against: under the minimum range.
    p.run("player 216.5 -59 202.5 0 0\n");
    assert!(p.s.within_melee(&zm, Ent::Player));
    p.run("player 216.5 -59 205.0 0 0\n");
    assert!(!p.s.within_melee(&zm, Ent::Player));
    zm.equipment[0] = None;
    assert!(p.s.within_melee(&zm, Ent::Player));
    p.s.mobs.list[i] = Some(zm);
}

/// Goals set by the server: zombie (spear 2, crossing 6, doors 1 last), wolf (llama 3),
/// trader (7 flees), creeper (ocelot then cat).
#[test]
fn goal_lists() {
    let Some(mut p) = probe("w10goals", "normal") else { return };
    let z = spawn(&mut p, Kind::Zombie, [210.5, -59.0, 210.5]);
    let zm = p.s.mob_ref(z).unwrap();
    let g: Vec<(i32, &Goal)> = zm.goals.goals.iter().map(|w| (w.prio, &w.goal)).collect();
    assert!(g.iter().any(|(pr, x)| *pr == 2 && matches!(x, Goal::Spear(_))));
    assert!(g.iter().any(|(pr, x)| *pr == 6 && matches!(x, Goal::Ill(IGoal::VillageWalk { .. }))));
    assert!(matches!(g.last(), Some((1, Goal::Ill(IGoal::BreakDoor { zombie: true, .. })))));
    let w = spawn(&mut p, Kind::Wolf, [212.5, -59.0, 210.5]);
    let wm = p.s.mob_ref(w).unwrap();
    assert!(
        wm.goals
            .goals
            .iter()
            .any(|x| x.prio == 3 && matches!(x.goal, Goal::Avoid { who: Who::Exact(Kind::Llama), .. }))
    );
    let t = spawn(&mut p, Kind::WanderingTrader, [214.5, -59.0, 210.5]);
    let tm = p.s.mob_ref(t).unwrap();
    assert_eq!(tm.goals.goals.iter().filter(|x| matches!(x.goal, Goal::Avoid { .. })).count(), 7);
    let c = spawn(&mut p, Kind::Creeper, [216.5, -59.0, 210.5]);
    let cm = p.s.mob_ref(c).unwrap();
    let av: Vec<Who> = cm
        .goals
        .goals
        .iter()
        .filter_map(|x| if let Goal::Avoid { who, .. } = x.goal { Some(who) } else { None })
        .collect();
    assert_eq!(av, vec![Who::Exact(Kind::Ocelot), Who::Cat]);
    let e = spawn(&mut p, Kind::Enderman, [218.5, -59.0, 210.5]);
    let em = p.s.mob_ref(e).unwrap();
    assert!(em.goals.goals.iter().any(|x| x.prio == 10 && matches!(x.goal, Goal::EndermanLeave)));
    assert!(em.goals.goals.iter().any(|x| x.prio == 11 && matches!(x.goal, Goal::EndermanTake)));
}

/// The drowned (amphibious navigation, therefore "on the ground" for the game) keeps the ability to break
/// doors: read from the NBT, rewritten, kept by `set_can_break_doors`.
#[test]
fn drowned_breaks_doors() {
    let mut d = Mob::new(Kind::Drowned, 1, 1, [0.0; 3], 0.0, JRand::new(1));
    assert!(d.nav.amphibious);
    let nbt = Nbt::Compound(vec![("CanBreakDoors".into(), Nbt::Byte(1))]);
    super::variants::load_extra(&mut d, &nbt);
    assert!(d.can_break_doors && d.nav.can_open_doors);
    let mut out = Nbt::Compound(Vec::new());
    super::variants::save_extra(&d, &mut out);
    assert_eq!(crate::nbtio::int(&out, "CanBreakDoors"), Some(1));
}
