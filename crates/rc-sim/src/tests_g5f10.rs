//! G5-F10: item uses on a fresh flat world (26.3 data required). Expected values
//! taken from the game's rules, not from our code.

use std::path::PathBuf;
use std::sync::Arc;

use rc_proto::nbt::Nbt;

use crate::blocks::Dir;
use crate::blocks::probe::Probe;
use crate::newworld::{NewWorld, Preset, create_world};
use crate::player::Stack;

fn open(tag: &str) -> (Probe, PathBuf) {
    let data = Arc::new(rc_testdata::version_data().expect("26.3 version data missing"));
    assert!(crate::registries::client_jar(&data).is_some(), "26.3 client jar missing");
    let dir = std::env::temp_dir().join(format!("rc-g5f10-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut w = NewWorld::new(tag);
    w.seed = 1;
    w.preset = Preset::Flat;
    create_world(&dir, &data, &w).expect("world creation");
    let mut p = Probe::open(&dir, data);
    p.s.pos = [0.5, -60.0, 0.5];
    let around: Vec<(i32, i32)> = (-1..=1).flat_map(|x| (-1..=1).map(move |z| (x, z))).collect();
    p.s.load_now(&around);
    p.s.loaded = true;
    (p, dir)
}

fn finish(p: Probe, dir: PathBuf) {
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

fn cmd(p: &mut Probe, c: &str) {
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, c);
    assert!(ok, "command refused: {c}");
}

fn stack(id: &str, count: i32) -> Stack {
    Stack { id: id.into(), count, components: None }
}

fn hold(p: &mut Probe, s: Stack) -> usize {
    let slot = 36 + p.s.held as usize;
    p.s.slots[slot] = Some(s);
    slot
}

fn logged(p: &Probe, ev: &str) -> usize {
    p.s.mobs.rec.vib.log.iter().filter(|e| e.0 == ev).count()
}

/// End of cooldown: the `cooldown` packet at 0 goes out at the tick when the cooldown expires.
#[test]
fn cooldown_end_is_sent() {
    let (mut p, dir) = open("cd");
    p.take_events();
    p.s.add_cooldown("minecraft:ender_pearl", 3);
    let zero = |evs: &[rc_client::Event]| {
        evs.iter()
            .filter(|e| matches!(e, rc_client::Event::Cooldown { group, ticks: 0 } if group == "minecraft:ender_pearl"))
            .count()
    };
    for t in 1..=3 {
        p.s.proj_player_tick();
        let evs = p.take_events();
        assert_eq!(zero(&evs), usize::from(t == 3), "tick {t}");
    }
    finish(p, dir);
}

/// An exploration map is a map item: clicked on a banner, the use is handled.
#[test]
fn exploration_map_is_a_map() {
    assert!(crate::maps::is_map_item("minecraft:buried_treasure_map"));
    assert!(crate::maps::is_map_item("minecraft:filled_map"));
    assert!(!crate::maps::is_map_item("minecraft:map"));
    let (mut p, dir) = open("map");
    cmd(&mut p, "setblock 2 -60 2 minecraft:white_banner");
    let mut m = stack("minecraft:buried_treasure_map", 1);
    crate::items::set_component(&mut m, "minecraft:map_id", Nbt::Int(0));
    hold(&mut p, m);
    assert!(p.s.proj_use_on(0, [2, -60, 2], 1, [0.5, 0.5, 0.5]), "exploration map on banner ignored");
    finish(p, dir);
}

/// Minecart placed on a rail by its item (survival: the item goes away), refused off a rail.
#[test]
fn minecart_item_places_on_rail() {
    let (mut p, dir) = open("cart");
    cmd(&mut p, "setblock 3 -60 0 minecraft:rail");
    let carts = |p: &Probe| p.s.all_ents().into_iter().filter(|e| p.s.ent_type(*e) == "minecraft:minecart").count();
    let slot = hold(&mut p, stack("minecraft:minecart", 2));
    assert!(!p.s.item_use_on(slot, [3, -61, 0], Dir::Up), "placed off rail");
    assert!(p.s.item_use_on(slot, [3, -60, 0], Dir::Up), "placement refused on the rail");
    assert_eq!(carts(&p), 1);
    assert_eq!(p.s.slots[slot].as_ref().map(|s| s.count), Some(1));
    finish(p, dir);
}

/// Goat horn: use of 7 s (140 ticks), cooldown of 140 ticks, `instrument_play` vibration.
#[test]
fn goat_horn_plays() {
    let (mut p, dir) = open("horn");
    let mut h = stack("minecraft:goat_horn", 1);
    crate::items::set_component(&mut h, "minecraft:instrument", Nbt::String("minecraft:ponder_goat_horn".into()));
    hold(&mut p, h.clone());
    assert!(p.s.w8c_use_item(0));
    assert_eq!(p.s.life.vit.using, Some((0, 140)));
    assert!(p.s.on_cooldown(&h), "cooldown missing");
    assert_eq!(logged(&p, "minecraft:instrument_play"), 1);
    assert_eq!(logged(&p, "minecraft:item_interact_start"), 1);
    finish(p, dir);
}

/// Knowledge book: recipes learned, book consumed; without a recipe, consumed anyway.
#[test]
fn knowledge_book_unlocks() {
    let (mut p, dir) = open("kb");
    let mut b = stack("minecraft:knowledge_book", 2);
    crate::items::set_component(
        &mut b,
        "minecraft:recipes",
        Nbt::List(vec![Nbt::String("minecraft:crafting_table".into())]),
    );
    let slot = hold(&mut p, b);
    p.s.life.known.remove("minecraft:crafting_table");
    assert!(p.s.w8c_use_item(0));
    assert!(p.s.life.known.contains("minecraft:crafting_table"));
    assert_eq!(p.s.slots[slot].as_ref().map(|s| s.count), Some(1));
    hold(&mut p, stack("minecraft:knowledge_book", 1));
    assert!(p.s.w8c_use_item(0));
    assert!(p.s.slots[slot].is_none(), "empty book not consumed");
    finish(p, dir);
}

/// Compass on lodestone: the compass alone changes in place; a stack keeps the rest and the
/// copy goes into the inventory; lodestone broken: target erased at the tick.
#[test]
fn lodestone_compass_binds_and_loses_target() {
    let (mut p, dir) = open("lode");
    cmd(&mut p, "setblock 4 -60 4 minecraft:lodestone");
    let target = |s: Option<&Stack>| {
        s.and_then(|s| crate::items::component(s, "minecraft:lodestone_tracker"))
            .and_then(|t| t.get("target"))
            .and_then(|t| t.get("pos"))
            .cloned()
    };
    let slot = hold(&mut p, stack("minecraft:compass", 1));
    assert!(p.s.item_use_on(slot, [4, -60, 4], Dir::Up));
    assert_eq!(target(p.s.slots[slot].as_ref()), Some(Nbt::IntArray(vec![4, -60, 4])));
    // Stack of two: a bound compass added elsewhere, the stack keeps one.
    let slot = hold(&mut p, stack("minecraft:compass", 2));
    assert!(p.s.item_use_on(slot, [4, -60, 4], Dir::Up));
    assert_eq!(p.s.slots[slot].as_ref().map(|s| s.count), Some(1));
    assert!(target(p.s.slots[slot].as_ref()).is_none());
    let bound = (0..46).filter(|&i| target(p.s.slots[i].as_ref()).is_some()).count();
    assert_eq!(bound, 1, "bound copy missing");
    // Lodestone removed: the target disappears (component kept, without target).
    let copy = (0..46).find(|&i| target(p.s.slots[i].as_ref()).is_some()).expect("copy");
    cmd(&mut p, "setblock 4 -60 4 minecraft:air");
    p.s.lodestone_compass_tick();
    assert!(target(p.s.slots[copy].as_ref()).is_none(), "target kept without lodestone");
    assert!(crate::items::component(p.s.slots[copy].as_ref().expect("compass"), "minecraft:lodestone_tracker").is_some());
    finish(p, dir);
}

/// Debug stick (creative + commands): use = next value of the first property
/// (sorted by name: `facing`); start of breaking = next property remembered, block intact.
#[test]
fn debug_stick_cycles_and_selects() {
    let (mut p, dir) = open("dbg");
    p.s.game_mode = 1;
    p.s.level.allow_commands = true;
    p.s.cmd.op = 4;
    cmd(&mut p, "setblock 2 -60 0 minecraft:oak_stairs[facing=north,half=bottom]");
    let slot = hold(&mut p, stack("minecraft:debug_stick", 1));
    let get = |p: &Probe, k: &str| p.s.v().get(p.s.get([2, -60, 0]), k).map(str::to_owned);
    assert!(p.s.item_use_on(slot, [2, -60, 0], Dir::Up));
    assert_eq!(get(&p, "facing").as_deref(), Some("south"));
    // Selection: none remembered → the first (`facing`), then the next (`half`).
    assert!(!p.s.player_destroy_block([2, -60, 0]));
    assert!(!p.s.player_destroy_block([2, -60, 0]));
    assert_eq!(get(&p, "facing").as_deref(), Some("south"), "block broken or changed");
    assert!(p.s.item_use_on(slot, [2, -60, 0], Dir::Up));
    assert_eq!(get(&p, "half").as_deref(), Some("top"));
    // Sneaking: previous value (before the first, the last).
    p.s.sneaking = true;
    assert!(p.s.item_use_on(slot, [2, -60, 0], Dir::Up));
    assert_eq!(get(&p, "half").as_deref(), Some("bottom"));
    // Survival: refused.
    p.s.game_mode = 0;
    p.s.sneaking = false;
    assert!(!p.s.item_use_on(slot, [2, -60, 0], Dir::Up));
    finish(p, dir);
}

/// Shears on a growing kelp head: age 25, wear 1; already at 25: nothing.
#[test]
fn shears_stop_kelp_growth() {
    let (mut p, dir) = open("kelp");
    cmd(&mut p, "fill 6 -60 6 6 -58 6 minecraft:water");
    cmd(&mut p, "setblock 6 -60 6 minecraft:kelp[age=3]");
    let slot = hold(&mut p, stack("minecraft:shears", 1));
    assert!(p.s.item_use_on(slot, [6, -60, 6], Dir::Up));
    assert_eq!(p.s.v().get_int(p.s.get([6, -60, 6]), "age"), 25);
    let dmg = |p: &Probe| {
        p.s.slots[slot].as_ref().and_then(|s| crate::items::component(s, "minecraft:damage")).and_then(Nbt::as_i64)
    };
    assert_eq!(dmg(&p), Some(1));
    assert!(!p.s.item_use_on(slot, [6, -60, 6], Dir::Up));
    assert_eq!(dmg(&p), Some(1));
    finish(p, dir);
}

/// Use vibrations: start and end of a prolonged use (shield raised then lowered); silent
/// item (`interact_vibrations` false): none; the last bone meal does not vibrate.
#[test]
fn use_vibrations() {
    let (mut p, dir) = open("vib");
    hold(&mut p, stack("minecraft:shield", 1));
    assert!(p.s.w8c_use_item(0));
    p.s.on_hand_action(5);
    assert_eq!(logged(&p, "minecraft:item_interact_start"), 1);
    assert_eq!(logged(&p, "minecraft:item_interact_finish"), 1);
    let mut quiet = stack("minecraft:shield", 1);
    crate::items::set_component(
        &mut quiet,
        "minecraft:use_effects",
        Nbt::Compound(vec![("interact_vibrations".into(), Nbt::Byte(0))]),
    );
    hold(&mut p, quiet);
    assert!(p.s.w8c_use_item(0));
    p.s.on_hand_action(5);
    assert_eq!(logged(&p, "minecraft:item_interact_start"), 1, "silent item that vibrates");
    assert_eq!(logged(&p, "minecraft:item_interact_finish"), 1);
    // Bone meal: vibration of the remaining stack only.
    cmd(&mut p, "setblock 1 -60 1 minecraft:wheat[age=0]");
    let slot = hold(&mut p, stack("minecraft:bone_meal", 1));
    assert!(p.s.item_use_on(slot, [1, -60, 1], Dir::Up));
    assert_eq!(logged(&p, "minecraft:item_interact_finish"), 1, "last bone meal that vibrates");
    cmd(&mut p, "setblock 1 -60 1 minecraft:wheat[age=0]");
    let slot = hold(&mut p, stack("minecraft:bone_meal", 2));
    assert!(p.s.item_use_on(slot, [1, -60, 1], Dir::Up));
    assert_eq!(logged(&p, "minecraft:item_interact_finish"), 2);
    finish(p, dir);
}

/// Eye of Ender in flight: first two ticks (zero velocity, then 0.03 / 0.015 towards a target
/// 12 blocks east and 8 higher), end after 80 ticks.
#[test]
fn eye_flight() {
    use crate::projectiles::{PKind, Proj};
    let (mut p, dir) = open("eye");
    let id = p.s.proj_new_id();
    let mut e = Proj::new(PKind::EyeOfEnder, id, [0.5, -50.0, 0.5], stack("minecraft:ender_eye", 1));
    e.eye_target = Some([12.5, -42.0, 0.5]);
    p.s.eye_tick(&mut e);
    assert_eq!(e.pos, [0.5, -50.0, 0.5]);
    p.s.eye_tick(&mut e);
    assert!((e.pos[0] - 0.53).abs() < 1e-12 && (e.pos[1] - (-50.0 + 0.015)).abs() < 1e-12, "{:?}", e.pos);
    for _ in 2..80 {
        p.s.eye_tick(&mut e);
        assert!(!e.removed);
    }
    p.s.eye_tick(&mut e);
    assert!(e.removed, "eye still in flight after 80 ticks");
    finish(p, dir);
}

/// Eye thrown with no locatable structure (flat world without stronghold): the use starts (never completed
/// alone), the eye stays in hand.
#[test]
fn eye_without_stronghold() {
    let (mut p, dir) = open("eye0");
    let slot = hold(&mut p, stack("minecraft:ender_eye", 1));
    assert!(p.s.proj_use_item(0));
    assert_eq!(p.s.life.vit.using, Some((0, 0)));
    assert_eq!(p.s.slots[slot].as_ref().map(|s| s.count), Some(1));
    assert!(p.s.proj.list.iter().all(|q| q.kind != crate::projectiles::PKind::EyeOfEnder));
    finish(p, dir);
}

/// Fox eating a chorus fruit: its consumption effects apply (it moves).
#[test]
fn fox_chorus_teleports() {
    let (mut p, dir) = open("fox");
    let mut m = p.s.new_mob(crate::mobs::species::Kind::Fox, [0.5, -60.0, 0.5], 0.0);
    let list = p
        .s
        .comp_json(&stack("minecraft:chorus_fruit", 1), "minecraft:consumable")
        .and_then(|c| c["on_consume_effects"].as_array().cloned())
        .expect("chorus effects");
    let before = m.pos;
    p.s.run_mob_consume_effects(&mut m, &list);
    assert_ne!(m.pos, before, "fox motionless");
    assert_eq!(logged(&p, "minecraft:teleport"), 1);
    // Through the fox finishing its meal (fruit held in the mouth): teleported, `eat` event.
    m.equipment[0] = Some(stack("minecraft:chorus_fruit", 1));
    let before = m.pos;
    crate::mobs::wild::fox::eat_mouth_item(&mut p.s, &mut m);
    assert_ne!(m.pos, before, "meal without consumption effect");
    assert_eq!(logged(&p, "minecraft:teleport"), 2);
    assert_eq!(logged(&p, "minecraft:eat"), 1);
    assert!(m.equipment[0].is_none());
    finish(p, dir);
}

/// Brushed suspicious sand: one hit every 10 ticks at most, `dusted` 1 (hits 1-2), 2 (3-5),
/// 3 (6-9); at the 10th the loot (fixed seed) drops above and the block becomes sand again.
/// With no hit for 40 ticks, the brushing recedes by 2.
#[test]
fn brush_suspicious_sand() {
    let (mut p, dir) = open("brush");
    let pos = [7, -61, 7];
    cmd(
        &mut p,
        "setblock 7 -61 7 minecraft:suspicious_sand{LootTable:\"minecraft:archaeology/desert_pyramid\",LootTableSeed:5L}",
    );
    let brush = stack("minecraft:brush", 1);
    let dusted = |p: &Probe| p.s.v().get_int(p.s.get(pos), "dusted");
    let mut seen = Vec::new();
    for i in 0..9 {
        assert!(!p.s.brush_block(pos, 1, Some(&brush)), "completed too early ({i})");
        // Second hit in the same window: ignored.
        assert!(!p.s.brush_block(pos, 1, Some(&brush)));
        seen.push(dusted(&p));
        p.s.time += 10;
    }
    assert_eq!(seen, [1, 1, 2, 2, 2, 3, 3, 3, 3]);
    assert!(p.s.be_get(pos).and_then(|b| b.get("LootTable")).is_none(), "loot not drawn on the first hit");
    assert!(p.s.brush_block(pos, 1, Some(&brush)));
    assert_eq!(p.s.state_name(p.s.get(pos)), "minecraft:sand");
    // Recede: three hits then 40 ticks with nothing.
    cmd(&mut p, "setblock 8 -61 8 minecraft:suspicious_gravel");
    let q = [8, -61, 8];
    for _ in 0..3 {
        p.s.brush_block(q, 1, Some(&brush));
        p.s.time += 10;
    }
    assert_eq!(p.s.v().get_int(p.s.get(q), "dusted"), 2);
    // Last hit at t+20: recede from t+60.
    p.s.time += 29;
    p.s.brush_check_reset(q);
    assert_eq!(p.s.v().get_int(p.s.get(q), "dusted"), 2, "recession too early");
    p.s.time += 1;
    p.s.brush_check_reset(q);
    assert_eq!(p.s.v().get_int(p.s.get(q), "dusted"), 1, "brushing not receded");
    finish(p, dir);
}

/// Micro-bench of the work added per tick (docs/perf.md, G5-F10).
#[test]
#[ignore]
fn bench_g5f10_ticks() {
    use std::time::Instant;
    let (mut p, dir) = open("bench");
    let n = 20_000u32;
    let per = |t: Instant, k: u32| t.elapsed().as_nanos() as f64 / f64::from(k);
    // Inventory full of ordinary items, no compass.
    for i in 9..45 {
        p.s.slots[i] = Some(stack("minecraft:stone", 64));
    }
    let t = Instant::now();
    for _ in 0..n {
        p.s.lodestone_compass_tick();
    }
    let none = per(t, n);
    // 36 compasses bound to an existing lodestone.
    cmd(&mut p, "setblock 4 -60 4 minecraft:lodestone");
    let slot = hold(&mut p, stack("minecraft:compass", 1));
    assert!(p.s.item_use_on(slot, [4, -60, 4], Dir::Up));
    let bound = p.s.slots[slot].clone();
    for i in 9..45 {
        p.s.slots[i] = bound.clone();
    }
    let t = Instant::now();
    for _ in 0..n {
        p.s.lodestone_compass_tick();
    }
    let tracked = per(t, n);
    cmd(&mut p, "setblock 5 -55 5 minecraft:end_gateway");
    let t = Instant::now();
    for _ in 0..n {
        p.s.gateway_be_tick([5, -55, 5]);
    }
    let gw = per(t, n);
    use crate::projectiles::{PKind, Proj};
    let id = p.s.proj_new_id();
    let t = Instant::now();
    let mut k = 0;
    while k < n {
        let mut e = Proj::new(PKind::EyeOfEnder, id, [0.5, -50.0, 0.5], stack("minecraft:ender_eye", 1));
        e.eye_target = Some([12.5, -42.0, 0.5]);
        for _ in 0..80 {
            p.s.eye_tick(&mut e);
        }
        k += 80;
    }
    let eye = per(t, k);
    eprintln!(
        "G5F10 compasses: {none:.0} ns/tick without compass, {tracked:.0} ns/tick with 36 tracked; \
         gateway {gw:.0} ns/tick; eye {eye:.0} ns/tick"
    );
    finish(p, dir);
}

/// End gateway: the age advances by one per tick (NBT `Age`), beam every 2400.
#[test]
fn gateway_ages() {
    let (mut p, dir) = open("gw");
    cmd(&mut p, "setblock 5 -55 5 minecraft:end_gateway{Age:2398L}");
    p.take_events();
    let age = |p: &Probe| match p.s.be_get([5, -55, 5]).and_then(|b| b.get("Age")) {
        Some(Nbt::Long(a)) => *a,
        o => panic!("Age missing: {o:?}"),
    };
    p.s.tick_block_entities();
    assert_eq!(age(&p), 2399);
    p.s.tick_block_entities();
    assert_eq!(age(&p), 2400);
    let beams = p.s.bs.events.iter().filter(|e| e.pos == [5, -55, 5] && e.a == 1).count();
    assert_eq!(beams, 1, "2400-tick beam missing");
    // End of cooldown: the chunk is marked to be saved (cooldown state change).
    assert!(p.s.dims.gateways.contains_key(&(p.s.dim.name.clone(), [5, -55, 5])), "cooldown missing");
    p.s.dims.gateways.insert((p.s.dim.name.clone(), [5, -55, 5]), 1);
    if let Some(Some(c)) = p.s.chunks.get_mut(&(0, 0)) {
        c.dirty = false;
    }
    p.s.gateways_tick();
    assert!(p.s.dims.gateways.is_empty());
    assert!(matches!(p.s.chunks.get(&(0, 0)), Some(Some(c)) if c.dirty), "end of cooldown not marked");
    finish(p, dir);
}

/// Use stack emptied by the use itself (only trident thrown, bow broken on shooting, last eye
/// thrown): an empty stack has no component, the end of the use does not vibrate. A hand emptied
/// by the use keeps the use, which only counts down.
#[test]
fn emptied_use_stack_does_not_vibrate() {
    use crate::projectiles::weapons::USE_DURATION;
    let (mut p, dir) = open("vibe");
    p.s.game_mode = 0;
    let finish_ev = "minecraft:item_interact_finish";
    let slot = hold(&mut p, stack("minecraft:trident", 1));
    assert!(p.s.proj_use_item(0));
    assert_eq!(logged(&p, "minecraft:item_interact_start"), 1);
    p.s.life.vit.using = Some((0, USE_DURATION - 20));
    assert!(p.s.proj_release());
    assert!(p.s.slots[slot].is_none(), "trident not thrown");
    p.s.on_hand_action(5);
    assert_eq!(logged(&p, finish_ev), 0, "thrown trident that vibrates");
    // Bow at one durability point: broken by the shot.
    let mut bow = stack("minecraft:bow", 1);
    crate::items::set_component(&mut bow, "minecraft:damage", Nbt::Int(383));
    hold(&mut p, bow);
    p.s.slots[9] = Some(stack("minecraft:arrow", 4));
    assert!(p.s.proj_use_item(0));
    p.s.life.vit.using = Some((0, USE_DURATION - 20));
    assert!(p.s.proj_release());
    assert!(p.s.slots[slot].is_none(), "bow not broken");
    assert_eq!(logged(&p, finish_ev), 0, "broken bow that vibrates");
    // Hand emptied by the use (last eye thrown): the use continues, no vibration on release.
    hold(&mut p, stack("minecraft:ender_eye", 1));
    p.s.start_using(0, 0);
    p.s.slots[slot] = None;
    p.s.use_stack_emptied(slot);
    p.s.vitals_tick();
    assert!(p.s.life.vit.using.is_some(), "use stopped on the hand emptied by the use");
    p.s.on_hand_action(5);
    assert_eq!(logged(&p, finish_ev), 0, "last eye that vibrates");
    // Stack moved out of the hand (not emptied by the use): end at the next tick, with vibration.
    hold(&mut p, stack("minecraft:shield", 1));
    assert!(p.s.w8c_use_item(0));
    p.s.slots[10] = p.s.slots[slot].take();
    p.s.vitals_tick();
    assert!(p.s.life.vit.using.is_none());
    assert_eq!(logged(&p, finish_ev), 1, "moved shield without vibration");
    finish(p, dir);
}

/// Brush broken by the last hit of a brushing: the end of the use does not vibrate.
#[test]
fn broken_brush_does_not_vibrate() {
    let (mut p, dir) = open("brushb");
    p.s.game_mode = 0;
    p.s.pitch = 90.0;
    let pos = [0, -61, 0];
    cmd(&mut p, "setblock 0 -61 0 minecraft:suspicious_sand{LootTable:\"minecraft:archaeology/desert_pyramid\"}");
    let mut brush = stack("minecraft:brush", 1);
    crate::items::set_component(&mut brush, "minecraft:damage", Nbt::Int(63));
    for _ in 0..9 {
        assert!(!p.s.brush_block(pos, 1, Some(&brush)));
        p.s.time += 10;
    }
    let slot = hold(&mut p, brush);
    p.s.start_using(0, 200);
    p.s.brush_use_tick(0, 196);
    assert_eq!(p.s.state_name(p.s.get(pos)), "minecraft:sand", "brushing not completed");
    assert!(p.s.slots[slot].is_none(), "brush not broken");
    p.s.on_hand_action(5);
    assert_eq!(logged(&p, "minecraft:item_interact_start"), 1);
    assert_eq!(logged(&p, "minecraft:item_interact_finish"), 0, "broken brush that vibrates");
    finish(p, dir);
}

/// Changing hotbar slot stops the main-hand use, vibration included; the
/// item choice (without direct stop) ends it at the next tick, the item in hand no longer being
/// that of the use. Nothing is eaten.
#[test]
fn hotbar_change_stops_main_hand_use() {
    let (mut p, dir) = open("held");
    p.s.game_mode = 0;
    p.s.held = 0;
    p.s.slots[36] = Some(stack("minecraft:apple", 1));
    p.s.slots[37] = Some(stack("minecraft:golden_apple", 1));
    p.s.life.vit.food = 10;
    p.s.on_use_item(0);
    assert!(p.s.life.vit.using.is_some());
    p.s.handle(crate::input::Input::HeldSlot(1));
    assert!(p.s.life.vit.using.is_none(), "use kept after the slot change");
    assert_eq!(logged(&p, "minecraft:item_interact_finish"), 1);
    // Item choice: back on the golden apple while eating the apple.
    p.s.held = 0;
    p.s.on_use_item(0);
    assert!(p.s.life.vit.using.is_some());
    p.s.try_pick_item(stack("minecraft:golden_apple", 1));
    assert_eq!(p.s.held, 1);
    for _ in 0..40 {
        p.s.vitals_tick();
    }
    assert!(p.s.life.vit.using.is_none(), "use kept after the item choice");
    assert_eq!(logged(&p, "minecraft:item_interact_finish"), 2);
    assert_eq!(p.s.slots[37].as_ref().map(|s| s.count), Some(1), "golden apple eaten");
    assert_eq!(p.s.slots[36].as_ref().map(|s| s.count), Some(1), "apple eaten");
    finish(p, dir);
}

/// `use_effects` component removed from the stack: the item no longer vibrates.
#[test]
fn removed_use_effects_is_silent() {
    let (mut p, dir) = open("vibr");
    let mut s = stack("minecraft:shield", 1);
    crate::items::set_component(&mut s, "!minecraft:use_effects", Nbt::Compound(Vec::new()));
    hold(&mut p, s);
    assert!(p.s.w8c_use_item(0));
    p.s.on_hand_action(5);
    assert_eq!(logged(&p, "minecraft:item_interact_start"), 0);
    assert_eq!(logged(&p, "minecraft:item_interact_finish"), 0);
    finish(p, dir);
}

/// Lodestone target placed on an item other than the compass: kept.
#[test]
fn lodestone_target_kept_on_other_items() {
    let (mut p, dir) = open("lode2");
    let target = Nbt::Compound(vec![(
        "target".into(),
        Nbt::Compound(vec![
            ("dimension".into(), Nbt::String(p.s.dim.name.clone())),
            ("pos".into(), Nbt::IntArray(vec![0, -60, 0])),
        ]),
    )]);
    let mut s = stack("minecraft:stick", 1);
    crate::items::set_component(&mut s, "minecraft:lodestone_tracker", target.clone());
    p.s.slots[9] = Some(s);
    p.s.lodestone_compass_tick();
    let kept = p.s.slots[9].as_ref().and_then(|s| crate::items::component(s, "minecraft:lodestone_tracker")).cloned();
    assert_eq!(kept, Some(target), "target erased on a stick");
    finish(p, dir);
}

/// Brush dust: no draw of the random on a block without terrain particles
/// (fence); draws on an ordinary block.
#[test]
fn brush_dust_skips_invisible_blocks() {
    let (mut p, dir) = open("dust");
    p.s.pitch = 90.0;
    cmd(&mut p, "setblock 0 -61 0 minecraft:stone");
    let before = p.s.life.rng;
    p.s.brush_use_tick(0, 196);
    assert_ne!(p.s.life.rng, before, "no draw on the stone (aim missed?)");
    cmd(&mut p, "setblock 0 -61 0 minecraft:barrier");
    assert_eq!(p.s.pov_hit(false).map(|h| h.0), Some([0, -61, 0]), "fence not aimed at");
    let before = p.s.life.rng;
    p.s.brush_use_tick(0, 196);
    assert_eq!(p.s.life.rng, before, "dust draws on a fence");
    finish(p, dir);
}
