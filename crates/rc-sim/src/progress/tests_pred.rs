//! G1-F07: entity and location predicates on a new flat world (ground: grass at -61, feet
//! at -60). Each test fails on the old behaviour (noted in its comment).

use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};

use super::Who;
use crate::blocks::probe::Probe;
use crate::commands::ents::E;
use crate::mobs::species::Kind;
use crate::newworld::{NewWorld, Preset, create_world};
use crate::player::Stack;

fn open(tag: &str) -> (Probe, PathBuf) {
    let data = Arc::new(rc_testdata::version_data().expect("26.3 version data missing"));
    assert!(crate::registries::client_jar(&data).is_some(), "26.3 client jar missing");
    let dir = std::env::temp_dir().join(format!("rc-pred-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut w = NewWorld::new(tag);
    w.seed = 1;
    w.preset = Preset::Flat;
    create_world(&dir, &data, &w).expect("world creation");
    let mut p = Probe::open(&dir, data);
    p.s.pos = [0.5, -60.0, 0.5];
    p.s.on_ground = true;
    p.s.loaded = true;
    (p, dir)
}

fn finish(p: Probe, dir: PathBuf) {
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

fn mob(p: &mut Probe, k: Kind, pos: [f64; 3]) -> i32 {
    let mut m = p.s.new_mob(k, pos, 0.0);
    m.persistent = true;
    m.old = pos;
    let id = m.id;
    p.s.add_mob(m);
    id
}

fn ent(p: &Probe, pred: Value, e: E) -> bool {
    let origin = p.s.pos;
    p.s.p_entity(&pred, Some(Who::E(e)), Some(origin))
}

/// `contains`: a single element satisfies both tests (old code: only one removed, false).
#[test]
fn contains_removes_every_satisfied_test() {
    let (p, dir) = open("contains");
    let items = [Stack { id: "minecraft:diamond".into(), count: 1, components: None }];
    let two = json!({"contains": [{"items": "minecraft:diamond"}, {"count": 1}]});
    assert!(p.s.p_collection(Some(&two), &items), "an element must remove all the tests it satisfies");
    let other = json!({"contains": [{"items": "minecraft:diamond"}, {"items": "minecraft:stone"}]});
    assert!(!p.s.p_collection(Some(&other), &items));
    finish(p, dir);
}

/// `smokey` follows the beehive rule: a slab (collision on the post) cuts the smoke of a
/// fire two cells lower (old code: true); a full block placed ON the fire lets it
/// through (old code: false).
#[test]
fn smokey_uses_the_hive_rule() {
    let (mut p, dir) = open("smokey");
    let q = json!({"smokey": true});
    p.run("setblock 0 -61 0 minecraft:campfire[lit=true]\nsetblock 0 -59 0 minecraft:oak_slab");
    let mut fails = Vec::new();
    if p.s.p_location(&q, 0.5, -58.0, 0.5) {
        fails.push("slab: smoke cut (fire not directly below)");
    }
    p.run("setblock 3 -61 0 minecraft:campfire[lit=true]\nsetblock 3 -60 0 minecraft:stone");
    if !p.s.p_location(&q, 3.5, -59.0, 0.5) {
        fails.push("full block placed on the fire: smoke");
    }
    finish(p, dir);
    assert!(fails.is_empty(), "{fails:?}");
}

/// `stepping_on` reads the supporting block at 1e-5 (carpet); old code: 0.2 below (grass).
/// `movement_affected_by` at 0.500001f (old: 0.5000001).
#[test]
fn stepping_on_and_movement_affected_by_use_the_support_rule() {
    let (mut p, dir) = open("stepping");
    p.run("setblock 0 -60 0 minecraft:white_carpet");
    p.s.pos = [0.5, -59.9375, 0.5];
    p.s.on_ground = true;
    let mut fails = Vec::new();
    let carpet = json!({"minecraft:stepping_on": {"block": {"blocks": "minecraft:white_carpet"}}});
    if !ent(&p, carpet, E::Player) {
        fails.push("stepping_on must see the supporting carpet");
    }
    // In the air, without a support: floor(-59.4999995 - 0.500001) = -61 (grass).
    p.run("setblock 0 -60 0 minecraft:air");
    p.s.on_ground = false;
    p.s.pos = [0.5, -59.499_999_5, 0.5];
    let grass = json!({"minecraft:movement_affected_by": {"block": {"blocks": "minecraft:grass_block"}}});
    if !ent(&p, grass, E::Player) {
        fails.push("movement_affected_by: offset 0.500001f");
    }
    finish(p, dir);
    assert!(fails.is_empty(), "{fails:?}");
}

/// `looking_at` requires line of sight: behind glass, false (old code: true).
#[test]
fn looking_at_needs_line_of_sight() {
    let (mut p, dir) = open("looking");
    let pig = mob(&mut p, Kind::Pig, [0.5, -60.0, 5.5]);
    p.s.yaw = 0.0;
    p.s.pitch = 13.0;
    let pred = json!({"minecraft:type_specific/player": {"looking_at": {"type": "minecraft:pig"}}});
    assert_eq!(p.s.adv_looking_at(), Some(E::Mob(pig)), "the ray must hit the pig");
    assert!(ent(&p, pred.clone(), E::Player), "clear view");
    p.run("fill 0 -60 3 0 -58 3 minecraft:glass");
    assert!(!ent(&p, pred, E::Player), "behind the glass: no line of sight");
    finish(p, dir);
}

/// `raider`: absent fields = false; current raid counted; ordinary white banner and
/// without a patrol leader = not captain (old: has_raid never true, white banner
/// = captain).
#[test]
fn raider_reads_raid_and_captain_state() {
    let (mut p, dir) = open("raider");
    let id = mob(&mut p, Kind::Pillager, [4.5, -60.0, 4.5]);
    let r = |has: Option<bool>, cap: Option<bool>| {
        let mut o = serde_json::Map::new();
        if let Some(h) = has {
            o.insert("has_raid".into(), Value::Bool(h));
        }
        if let Some(c) = cap {
            o.insert("is_captain".into(), Value::Bool(c));
        }
        json!({"minecraft:type_specific/raider": Value::Object(o)})
    };
    let set = |p: &mut Probe, f: &dyn Fn(&mut crate::mobs::mob::Mob)| {
        let m = p.s.mobs.list.iter_mut().flatten().find(|m| m.id == id).expect("pillager");
        f(m);
    };
    set(&mut p, &|m| m.equipment[5] = Some(Stack { id: "minecraft:white_banner".into(), count: 1, components: None }));
    let mut fails = Vec::new();
    if !ent(&p, r(None, None), E::Mob(id)) || ent(&p, r(None, Some(true)), E::Mob(id)) {
        fails.push("ordinary banner: not captain");
    }
    set(&mut p, &|m| {
        m.equipment[5] = Some(crate::mobs::illagers::ominous_banner());
        m.ill.patrol_leader = true;
        m.ill.raid = Some(7);
    });
    if !ent(&p, r(Some(true), Some(true)), E::Mob(id)) {
        fails.push("in a raid and captain");
    }
    if ent(&p, r(None, Some(true)), E::Mob(id)) {
        fails.push("has_raid absent = false");
    }
    finish(p, dir);
    assert!(fails.is_empty(), "{fails:?}");
}

/// Player flags: swimming and gliding read from the server state (old: always false).
#[test]
fn player_flags_swimming_and_fall_flying() {
    let (mut p, dir) = open("flags");
    p.s.mobs.sea.player_swimming = true;
    p.s.proj_player.fall_flying = true;
    let fails: Vec<&str> = ["is_swimming", "is_fall_flying", "is_flying"]
        .into_iter()
        .filter(|k| !ent(&p, json!({"minecraft:flags": {*k: true}}), E::Player))
        .collect();
    finish(p, dir);
    assert!(fails.is_empty(), "wrong flags: {fails:?}");
}

/// Two-player LAN: A swims, B walks on land. B's movement must not change the swimming seen for
/// A, nor the reverse (old: a shared field rewritten at the last player's movement, A seen on
/// land). Ability flight cuts swimming.
#[test]
fn player_swimming_is_per_player() {
    let (mut p, dir) = open("swim2");
    p.run("fill 9 -60 9 13 -58 13 minecraft:water\n");
    let mut b = p.s.new_player_ctx("B", 0xB, 8, 0, Some(0));
    // A (host): sprint, eyes and feet in water.
    p.s.pos = [11.5, -60.0, 11.5];
    p.s.life.vit.sprinting = true;
    p.s.sealife_player_tick();
    let swim = json!({"minecraft:flags": {"is_swimming": true}});
    let mut fails = Vec::new();
    if !ent(&p, swim.clone(), E::Player) {
        fails.push("A does not swim before B");
    }
    // B: on land, without sprint; their movement and tick go through their context.
    p.s.swap_player(&mut b);
    p.s.pos = [0.5, -60.0, 0.5];
    p.s.life.vit.sprinting = false;
    p.s.handle(crate::input::Input::Move { pos: Some([0.8, -60.0, 0.5]), rot: None, on_ground: true });
    p.s.sealife_player_tick();
    if ent(&p, swim.clone(), E::Player) {
        fails.push("B seen swimming");
    }
    p.s.swap_player(&mut b);
    if !ent(&p, swim.clone(), E::Player) {
        fails.push("A seen on land after B's movement");
    }
    // Ability flight: no more swimming.
    p.s.flying = true;
    p.s.sealife_player_tick();
    if ent(&p, swim, E::Player) {
        fails.push("swimming while flying");
    }
    finish(p, dir);
    assert!(fails.is_empty(), "{fails:?}");
}

/// NBT compared without `id` (old: `id` present); `predicates` evaluates the component value
/// (old: presence only); `movement` reads the speed (old: position delta, zero here);
/// `fishing_hook` without a field = true (old: false).
#[test]
fn nbt_partial_components_movement_and_hook() {
    let (mut p, dir) = open("misc");
    let pig = mob(&mut p, Kind::Pig, [2.5, -60.0, 2.5]);
    let mut fails = Vec::new();
    if ent(&p, json!({"minecraft:nbt": "{id:\"minecraft:pig\"}"}), E::Mob(pig)) {
        fails.push("nbt: `id` must be removed");
    }
    if !ent(&p, json!({"minecraft:type_specific/fishing_hook": {}}), E::Mob(pig))
        || ent(&p, json!({"minecraft:type_specific/fishing_hook": {"in_open_water": true}}), E::Mob(pig))
    {
        fails.push("hook without field = true, with field = false outside a hook");
    }
    p.s.mobs.list.iter_mut().flatten().find(|m| m.id == pig).expect("pig").vel = [0.1, 0.0, 0.0];
    if !ent(&p, json!({"minecraft:movement": {"x": {"min": 1.9, "max": 2.1}}}), E::Mob(pig)) {
        fails.push("movement: speed × 20");
    }
    let vil = mob(&mut p, Kind::Villager, [6.5, -60.0, 6.5]);
    let ty = |t: &str| json!({"minecraft:predicates": {"minecraft:villager/variant": t}});
    let have = p.s.w_component(Who::E(E::Mob(vil)), "minecraft:villager/variant").expect("variant");
    let have = have.as_str().expect("text").to_owned();
    let other = if have == "minecraft:desert" { "minecraft:snow" } else { "minecraft:desert" };
    if !ent(&p, ty(&have), E::Mob(vil)) || ent(&p, ty(other), E::Mob(vil)) {
        fails.push("predicates: variant value");
    }
    finish(p, dir);
    assert!(fails.is_empty(), "{fails:?}");
}
