//! Dispensers, droppers, hoppers, composter and containers end to end (test world
//! `RUSTCRAFT_SOLO_WORLD`). Each test fails on the code from before G5-F01.

use super::*;
use crate::blocks::probe::{Probe, state_string};

fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world()?, rc_testdata::version_data()?);
    let dir = crate::tests::fresh_copy(&world, tag);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run(BASE);
    assert_eq!(name_at(&p, [100, -60, 100]), "minecraft:stone", "test area missing");
    // Player far from the test cells (pressure plates, dispensers).
    p.run("player 100.5 -59 131.5 0 0\n");
    let _ = p.take_events();
    Some((p, dir))
}

const BASE: &str = "fill 100 -60 100 131 -60 131 minecraft:stone\nfill 100 -59 100 131 -40 131 minecraft:air\n";

fn name_at(p: &Probe, at: P) -> String {
    state_string(&p.s, p.s.get(at))
}

fn count(p: &Probe, at: P, id: &str) -> i32 {
    p.s.be_item_counts(at).iter().filter(|e| e.0 == id).map(|e| e.1).sum()
}

fn drops(p: &Probe, id: &str) -> i32 {
    p.s.life.drops.iter().filter(|d| !d.is_removed() && d.stack.id == id).map(|d| d.stack.count).sum()
}

/// Container slots at `at` rewritten (`(slot, item, count)`).
fn put(p: &mut Probe, at: P, items: &[(usize, &str, i32)]) {
    let mut slots = p.s.slots(at);
    for &(i, id, n) in items {
        slots[i] = Some(Stack { id: id.into(), count: n, components: None }.to_nbt(None));
    }
    p.s.store_slots(at, &slots);
}

fn level_events(evs: &[rc_client::Event]) -> Vec<(i32, i32)> {
    evs.iter()
        .filter_map(|e| match e {
            rc_client::Event::LevelEvent { kind, data, .. } => Some((*kind, *data)),
            _ => None,
        })
        .collect()
}

fn dispense(p: &mut Probe, at: P) -> Vec<rc_client::Event> {
    let _ = p.take_events();
    let s = p.s.get(at);
    p.s.dispense_from(s, at);
    p.take_events()
}

fn done(p: Probe, dir: std::path::PathBuf) {
    drop(p);
    let _ = std::fs::remove_dir_all(dir);
}

/// Hoppers and furnace: the ingredient enters from the top, the fuel from the side (stone is
/// refused), and the furnace ticks without having been opened (its simulated state is the source).
#[test]
fn hoppers_feed_a_live_furnace_by_face() {
    let Some((mut p, dir)) = probe("g5f01-furnace") else { return };
    p.run(
        "setblock 110 -59 110 minecraft:furnace[facing=north]\nsetblock 110 -58 110 minecraft:hopper[facing=down]\n\
         setblock 111 -59 110 minecraft:hopper[facing=west]\n",
    );
    put(&mut p, [110, -58, 110], &[(0, "minecraft:raw_iron", 1)]);
    put(&mut p, [111, -59, 110], &[(0, "minecraft:stone", 1), (1, "minecraft:coal", 1)]);
    assert!(p.s.hopper_try_move([110, -58, 110], None));
    assert!(p.s.hopper_try_move([111, -59, 110], None));
    assert_eq!(count(&p, [111, -59, 110], "minecraft:stone"), 1, "stone refused by the fuel slot");
    assert_eq!(count(&p, [110, -59, 110], "minecraft:raw_iron"), 1);
    assert_eq!(count(&p, [110, -59, 110], "minecraft:coal"), 1);
    p.s.furnaces_tick();
    assert_eq!(name_at(&p, [110, -59, 110]), "minecraft:furnace[facing=north,lit=true]", "furnace lit");
    // Empty furnace: from the side only the fuel slot is offered; the ore stays.
    p.run("setblock 114 -59 110 minecraft:furnace[facing=north]\nsetblock 115 -59 110 minecraft:hopper[facing=west]\n");
    put(&mut p, [115, -59, 110], &[(0, "minecraft:raw_iron", 1)]);
    assert!(!p.s.hopper_try_move([115, -59, 110], None));
    assert_eq!(count(&p, [114, -59, 110], "minecraft:raw_iron"), 0, "ore entered from the side");
    done(p, dir);
}

/// Hopper under a furnace: takes the result, leaves the fuel, takes an empty bucket.
#[test]
fn hopper_under_furnace_takes_output_not_fuel() {
    let Some((mut p, dir)) = probe("g5f01-furnace-down") else { return };
    p.run("setblock 112 -58 110 minecraft:furnace[facing=north]\nsetblock 112 -59 110 minecraft:hopper[facing=down]\n");
    let (furnace, hopper) = ([112, -58, 110], [112, -59, 110]);
    put(&mut p, furnace, &[(1, "minecraft:coal", 4), (2, "minecraft:iron_ingot", 1)]);
    assert!(p.s.hopper_try_move(hopper, None));
    assert_eq!(count(&p, hopper, "minecraft:iron_ingot"), 1, "result first");
    p.s.bs.hoppers.get_mut(&hopper).expect("hopper").1 = 0;
    assert!(!p.s.hopper_try_move(hopper, None));
    assert_eq!(count(&p, hopper, "minecraft:coal"), 0, "fuel kept");
    put(&mut p, furnace, &[(1, "minecraft:bucket", 1)]);
    assert!(p.s.hopper_try_move(hopper, None));
    assert_eq!(count(&p, hopper, "minecraft:bucket"), 1, "empty bucket taken from below");
    done(p, dir);
}

/// Chest open while a hopper fills it: the next click starts from the real contents, nothing
/// is lost.
#[test]
fn open_chest_menu_keeps_hopper_items() {
    let Some((mut p, dir)) = probe("g5f01-menu") else { return };
    p.run(
        "setblock 112 -59 112 minecraft:chest[facing=north]\nsetblock 112 -58 112 minecraft:hopper[facing=down]\n\
         player 112.5 -59 110.5 0 0\n",
    );
    p.s.game_mode = 0;
    assert!(p.s.use_block([112, -59, 112]));
    assert!(p.s.life.menu.is_some(), "chest menu");
    put(&mut p, [112, -58, 112], &[(0, "minecraft:diamond", 1)]);
    assert!(p.s.hopper_try_move([112, -58, 112], None));
    p.s.slots[9] = Some(Stack { id: "minecraft:stick".into(), count: 1, components: None });
    let (window, state) = p.s.life.menu.as_ref().map(|m| (m.window, m.state)).expect("menu");
    // Shift-click the stick (first inventory slot) into the chest.
    p.s.on_click(window, state, 27, 0, 1, Vec::new(), None);
    assert_eq!(count(&p, [112, -59, 112], "minecraft:stick"), 1);
    assert_eq!(count(&p, [112, -59, 112], "minecraft:diamond"), 1, "diamond from the hopper lost");
    done(p, dir);
}

/// Ordinary item: thrown in front, 0.7 block from the center (lowered by 0.15625), click 1000 and
/// smoke 2000 towards the east.
#[test]
fn dispenser_ejects_plain_items() {
    let Some((mut p, dir)) = probe("g5f01-eject") else { return };
    p.run("setblock 114 -59 114 minecraft:dispenser[facing=east]\n");
    put(&mut p, [114, -59, 114], &[(0, "minecraft:stone", 2)]);
    let evs = dispense(&mut p, [114, -59, 114]);
    assert_eq!(count(&p, [114, -59, 114], "minecraft:stone"), 1);
    let d = p.s.life.drops.iter().find(|d| d.stack.id == "minecraft:stone").expect("stone thrown");
    assert!((d.pos[0] - 115.2).abs() < 1.0e-9 && (d.pos[1] - (-58.656_25)).abs() < 1.0e-9, "{:?}", d.pos);
    assert_eq!(level_events(&evs), vec![(1000, 0), (2000, 5)]);
    done(p, dir);
}

/// Dropper into a chest: the item goes through with no sound at all; empty: failure click only.
#[test]
fn dropper_into_chest_is_silent() {
    let Some((mut p, dir)) = probe("g5f01-dropper") else { return };
    p.run("setblock 116 -59 114 minecraft:dropper[facing=east]\nsetblock 117 -59 114 minecraft:chest[facing=north]\n");
    put(&mut p, [116, -59, 114], &[(0, "minecraft:stone", 1)]);
    let evs = dispense(&mut p, [116, -59, 114]);
    assert_eq!(count(&p, [117, -59, 114], "minecraft:stone"), 1);
    assert!(level_events(&evs).is_empty(), "{:?}", level_events(&evs));
    let evs = dispense(&mut p, [116, -59, 114]);
    assert_eq!(level_events(&evs), vec![(1001, 0)]);
    done(p, dir);
}

/// Water bucket: placed in front, empty bucket returned, click and smoke; empty bucket on the
/// source: water bucket; flint and steel facing TNT: primed.
#[test]
fn dispenser_buckets_and_flint() {
    let Some((mut p, dir)) = probe("g5f01-bucket") else { return };
    p.run("setblock 118 -59 114 minecraft:dispenser[facing=east]\n");
    put(&mut p, [118, -59, 114], &[(0, "minecraft:water_bucket", 1)]);
    let evs = dispense(&mut p, [118, -59, 114]);
    assert_eq!(name_at(&p, [119, -59, 114]), "minecraft:water[level=0]");
    assert_eq!(count(&p, [118, -59, 114], "minecraft:bucket"), 1);
    assert!(level_events(&evs).contains(&(2000, 5)), "smoke");
    let evs = dispense(&mut p, [118, -59, 114]);
    assert_eq!(count(&p, [118, -59, 114], "minecraft:water_bucket"), 1, "water taken back");
    assert_eq!(level_events(&evs), vec![(1000, 0), (2000, 5)]);
    p.run("setblock 119 -59 114 minecraft:tnt\n");
    put(&mut p, [118, -59, 114], &[(0, "minecraft:flint_and_steel", 1)]);
    dispense(&mut p, [118, -59, 114]);
    assert_eq!(name_at(&p, [119, -59, 114]), "minecraft:air", "TNT primed");
    assert!(p.s.bs.entities.list.iter().any(|e| matches!(e.kind, super::super::entities::EntKind::Tnt(_))));
    done(p, dir);
}

/// Bone meal on wheat, Shulker box placed in front (facing up on a floor).
#[test]
fn dispenser_bone_meal_and_shulker_box() {
    let Some((mut p, dir)) = probe("g5f01-bonemeal") else { return };
    p.run(
        "setblock 120 -59 114 minecraft:dispenser[facing=east]\nsetblock 121 -60 114 minecraft:farmland\n\
         setblock 121 -59 114 minecraft:wheat[age=0]\n",
    );
    put(&mut p, [120, -59, 114], &[(0, "minecraft:bone_meal", 1)]);
    dispense(&mut p, [120, -59, 114]);
    assert_ne!(name_at(&p, [121, -59, 114]), "minecraft:wheat[age=0]", "wheat grown");
    p.run("setblock 121 -59 114 minecraft:air\n");
    put(&mut p, [120, -59, 114], &[(0, "minecraft:red_shulker_box", 1)]);
    dispense(&mut p, [120, -59, 114]);
    assert_eq!(name_at(&p, [121, -59, 114]), "minecraft:red_shulker_box[facing=up]");
    done(p, dir);
}

/// Composter: under a hopper at level 8, bone meal goes down and it empties; a pie
/// poured from above adds a layer; placed at level 7, it will go to level 8.
#[test]
fn composter_works_with_hoppers() {
    let Some((mut p, dir)) = probe("g5f01-composter") else { return };
    p.run("setblock 120 -59 120 minecraft:hopper[facing=down]\nsetblock 120 -58 120 minecraft:composter[level=8]\n");
    assert!(p.s.hopper_try_move([120, -59, 120], None));
    assert_eq!(count(&p, [120, -59, 120], "minecraft:bone_meal"), 1);
    assert_eq!(name_at(&p, [120, -58, 120]), "minecraft:composter[level=0]");
    p.run("setblock 120 -57 120 minecraft:hopper[facing=down]\n");
    put(&mut p, [120, -57, 120], &[(0, "minecraft:pumpkin_pie", 1)]);
    assert!(p.s.hopper_try_move([120, -57, 120], None));
    assert_eq!(name_at(&p, [120, -58, 120]), "minecraft:composter[level=1]");
    p.run("setblock 122 -59 120 minecraft:composter[level=7]\ntick 21\n");
    assert_eq!(name_at(&p, [122, -59, 120]), "minecraft:composter[level=8]");
    done(p, dir);
}

/// Copper chest that oxidizes (or is waxed): its contents stay.
#[test]
fn copper_chest_keeps_items_across_states() {
    let Some((mut p, dir)) = probe("g5f01-copper") else { return };
    p.run("setblock 122 -59 122 minecraft:copper_chest[facing=north]\n");
    put(&mut p, [122, -59, 122], &[(0, "minecraft:diamond", 3)]);
    p.run("setblock 122 -59 122 minecraft:exposed_copper_chest[facing=north]\n");
    assert_eq!(count(&p, [122, -59, 122], "minecraft:diamond"), 3, "contents lost on oxidation");
    done(p, dir);
}

/// Rail deprived of support: drops as an item.
#[test]
fn unsupported_rail_drops_item() {
    let Some((mut p, dir)) = probe("g5f01-rail") else { return };
    p.run("setblock 124 -59 124 minecraft:rail\n");
    let air = p.s.air();
    p.s.set_block([124, -60, 124], air, flag::ALL);
    assert_eq!(name_at(&p, [124, -59, 124]), "minecraft:air");
    assert_eq!(drops(&p, "minecraft:rail"), 1);
    done(p, dir);
}

/// Wooden plate: an item on the ground counts; stone plate (living entities only): it does not.
#[test]
fn plates_count_item_entities() {
    let Some((mut p, dir)) = probe("g5f01-plate") else { return };
    p.s.spawn_item([126.5, -59.0, 126.5], [0.0; 3], Stack { id: "minecraft:stone".into(), count: 1, components: None }, 0);
    let b = rc_physics::Aabb { min: [126.0625, -59.0, 126.0625], max: [126.9375, -58.75, 126.9375] };
    assert_eq!(p.s.count_entities_in(&b, false), 1);
    assert_eq!(p.s.count_entities_in(&b, true), 0);
    done(p, dir);
}

/// Locked chest: refused empty-handed (message), opened with the key; Ender chest
/// removed: its menu is no longer valid.
#[test]
fn locked_chest_and_ender_chest_menu() {
    let Some((mut p, dir)) = probe("g5f01-lock") else { return };
    p.run(
        "setblock 126 -59 120 minecraft:chest[facing=north]\nsetblock 128 -59 120 minecraft:ender_chest\n\
         player 127.5 -59 118.5 0 0\n",
    );
    p.s.game_mode = 0;
    let lock = Nbt::Compound(vec![("items".into(), Nbt::String("minecraft:diamond".into()))]);
    crate::nbtio::set(p.s.be_get_mut([126, -59, 120]).expect("chest"), "lock", lock);
    let held = 36 + p.s.held as usize;
    p.s.slots[held] = None;
    assert!(p.s.use_block([126, -59, 120]));
    assert!(p.s.life.menu.is_none(), "locked chest opened");
    assert!(p.take_events().iter().any(|e| matches!(e, rc_client::Event::ActionBar(_))));
    p.s.slots[held] = Some(Stack { id: "minecraft:diamond".into(), count: 1, components: None });
    assert!(p.s.use_block([126, -59, 120]));
    assert!(p.s.life.menu.is_some(), "key refused");
    p.s.close_menu(true);
    assert!(p.s.use_block([128, -59, 120]));
    assert!(p.s.menu_valid());
    let air = p.s.air();
    p.s.set_block([128, -59, 120], air, flag::ALL);
    assert!(!p.s.menu_valid(), "Ender menu kept without a chest");
    done(p, dir);
}

/// Placement: frosted ice (scheduled melt), lit target (turned off), wet sponge in the Nether
/// (dried); covered dirt path: dirt; shelf: block entity.
#[test]
fn placement_side_effects() {
    let Some((mut p, dir)) = probe("g5f01-place") else { return };
    p.run("setblock 104 -59 104 minecraft:frosted_ice\nsetblock 106 -59 104 minecraft:target[power=15]\n");
    let ice = p.s.v().block(p.s.get([104, -59, 104]));
    assert!(p.s.has_scheduled_tick([104, -59, 104], ice), "frosted ice without melt");
    assert_eq!(name_at(&p, [106, -59, 104]), "minecraft:target[power=0]");
    p.run("setblock 108 -59 104 minecraft:dirt_path\n");
    let stone = p.s.default_state("minecraft:stone");
    p.s.set_block([108, -58, 104], stone, flag::ALL);
    p.run("tick 2\n");
    assert_eq!(name_at(&p, [108, -59, 104]), "minecraft:dirt");
    p.run("setblock 110 -59 104 minecraft:oak_shelf[facing=north]\n");
    assert_eq!(
        p.s.be_get([110, -59, 104]).and_then(|b| b.get("id")).and_then(Nbt::as_str),
        Some("minecraft:shelf")
    );
    p.s.dim.name = "minecraft:the_nether".into();
    p.run("setblock 112 -59 104 minecraft:wet_sponge\n");
    assert_eq!(name_at(&p, [112, -59, 104]), "minecraft:sponge");
    p.s.dim.name = "minecraft:overworld".into();
    done(p, dir);
}

/// Open Shulker box: the observer looking at it lights up (neighbouring shapes notified
/// when the lid leaves).
#[test]
fn shulker_lid_updates_observer() {
    let Some((mut p, dir)) = probe("g5f01-shulker") else { return };
    p.run("setblock 104 -59 108 minecraft:shulker_box[facing=up]\nsetblock 103 -59 108 minecraft:observer[facing=east]\n");
    p.s.container_openers([104, -59, 108], crate::mobs::mob::Ent::Player, 1);
    // Lid that leaves then reaches the stop: two shape updates, two pulses.
    let (mut edges, mut was) = (0, false);
    for _ in 0..20 {
        p.tick();
        let on = name_at(&p, [103, -59, 108]).contains("powered=true");
        edges += i32::from(on && !was);
        was = on;
    }
    assert_eq!(edges, 2, "observer pulses");
    done(p, dir);
}

/// Dispenser projectiles: the wind charge is one, the trident is not (thrown).
#[test]
fn dispensable_projectiles() {
    assert!(crate::projectiles::dispense::dispensable("minecraft:wind_charge"));
    assert!(!crate::projectiles::dispense::dispensable("minecraft:trident"));
}

/// Mob that lands on a honey block: event 54 (jump particles).
#[test]
fn mob_landing_on_honey_sends_event() {
    let Some((mut p, dir)) = probe("g5f01-honey") else { return };
    p.run("setblock 104 -59 112 minecraft:honey_block\n");
    let mut m = p.s.new_mob(crate::mobs::species::Kind::Pig, [104.5, -57.5, 112.5], 0.0);
    m.fall_distance = 3.0;
    let _ = p.take_events();
    p.s.mob_move(&mut m, [0.0, -1.0, 0.0]);
    let id = m.id;
    let hit = p.take_events().iter().any(|e| {
        matches!(e, rc_client::Event::Entity(rc_client::EntityEvent::Event { id: i, event: 54 }) if *i == id)
    });
    assert!(hit, "honey slide missing");
    done(p, dir);
}

/// Furnace and brewing stand placed then loaded with data (`setblock …{Items:…}`, components of
/// the placed item): the simulated state, created empty with the block, is read back; nothing is lost.
#[test]
fn furnace_and_brewer_load_given_data() {
    let Some((mut p, dir)) = probe("g5f01-loaddata") else { return };
    p.run("setblock 102 -59 108 minecraft:furnace[facing=north]\nsetblock 104 -59 108 minecraft:brewing_stand\n");
    let items = |slot: i8, id: &str, n: i32| {
        Nbt::Compound(vec![(
            "Items".into(),
            Nbt::List(vec![Nbt::Compound(vec![
                ("Slot".into(), Nbt::Byte(slot)),
                ("id".into(), Nbt::String(id.into())),
                ("count".into(), Nbt::Int(n)),
            ])]),
        )])
    };
    p.s.load_block_data([102, -59, 108], items(0, "minecraft:raw_iron", 5));
    p.s.load_block_data([104, -59, 108], items(4, "minecraft:blaze_powder", 3));
    assert_eq!(count(&p, [102, -59, 108], "minecraft:raw_iron"), 5, "furnace emptied");
    assert_eq!(count(&p, [104, -59, 108], "minecraft:blaze_powder"), 3, "brewing stand emptied");
    p.run("tick 3\n");
    assert_eq!(count(&p, [102, -59, 108], "minecraft:raw_iron"), 5, "furnace overwritten at tick");
    assert_eq!(count(&p, [104, -59, 108], "minecraft:blaze_powder"), 3, "brewing stand overwritten at tick");
    done(p, dir);
}

/// Full composter above a hopper that refuses bone meal (five different items): emptied anyway,
/// bone meal lost (game quirk).
#[test]
fn full_composter_over_cluttered_hopper_empties() {
    let Some((mut p, dir)) = probe("g5f01-composter-lost") else { return };
    p.run("setblock 102 -59 102 minecraft:hopper[facing=down]\nsetblock 102 -58 102 minecraft:composter[level=8]\n");
    let ids = ["minecraft:stone", "minecraft:dirt", "minecraft:sand", "minecraft:gravel", "minecraft:cobblestone"];
    let fill: Vec<(usize, &str, i32)> = ids.iter().enumerate().map(|(i, id)| (i, *id, 1)).collect();
    put(&mut p, [102, -59, 102], &fill);
    assert!(!p.s.hopper_try_move([102, -59, 102], None));
    assert_eq!(name_at(&p, [102, -58, 102]), "minecraft:composter[level=0]", "composter not emptied");
    assert_eq!(count(&p, [102, -59, 102], "minecraft:bone_meal"), 0);
    done(p, dir);
}

/// Dropper pouring into an empty hopper: transfer delay of 8 set on the hopper.
#[test]
fn dropper_into_empty_hopper_sets_cooldown() {
    let Some((mut p, dir)) = probe("g5f01-dropper-hopper") else { return };
    p.run("setblock 102 -59 106 minecraft:dropper[facing=east]\nsetblock 103 -59 106 minecraft:hopper[facing=down]\n");
    put(&mut p, [102, -59, 106], &[(0, "minecraft:stone", 2)]);
    let _ = dispense(&mut p, [102, -59, 106]);
    assert_eq!(count(&p, [103, -59, 106], "minecraft:stone"), 1);
    assert_eq!(p.s.bs.hoppers.get(&[103, -59, 106]).map(|h| h.1), Some(8), "delay missing");
    done(p, dir);
}

/// Player falling 23.5 blocks onto honey: damage × 0.2 (4 and not 20).
#[test]
fn player_fall_on_honey_is_reduced() {
    let Some((mut p, dir)) = probe("g5f01-honey-player") else { return };
    p.run("setblock 108 -59 128 minecraft:honey_block\n");
    p.s.game_mode = 0;
    p.s.teleport = None;
    p.s.loaded = true;
    p.s.life.vit.health = 20.0;
    p.s.pos = [108.5, -58.0625, 128.5];
    p.s.on_ground = true;
    p.s.life.vit.fall = 23.0;
    p.s.after_move([108.5, -57.5625, 128.5], false);
    assert_eq!(p.s.life.vit.health, 16.0, "fall damage on honey");
    done(p, dir);
}

/// Double copper chest: it opens (double menu) and the lock of one half blocks opening
/// through the other.
#[test]
fn copper_chest_double_menu_and_lock() {
    let Some((mut p, dir)) = probe("g5f01-copper-lock") else { return };
    p.run(
        "setblock 126 -59 124 minecraft:copper_chest[facing=north,type=left]\n\
         setblock 127 -59 124 minecraft:copper_chest[facing=north,type=right]\nplayer 126.5 -59 122.5 0 0\n",
    );
    p.s.game_mode = 0;
    let held = 36 + p.s.held as usize;
    p.s.slots[held] = None;
    assert!(p.s.use_block([126, -59, 124]));
    assert!(p.s.life.menu.as_ref().is_some_and(|m| m.block2.is_some()), "no double menu");
    p.s.close_menu(true);
    let lock = Nbt::Compound(vec![("items".into(), Nbt::String("minecraft:diamond".into()))]);
    crate::nbtio::set(p.s.be_get_mut([127, -59, 124]).expect("chest"), "lock", lock);
    assert!(p.s.use_block([126, -59, 124]));
    assert!(p.s.life.menu.is_none(), "locked half opened through the other");
    done(p, dir);
}

/// Bench (ignored by default): cost of re-reading the menu (full double chest) and of a
/// hopper transfer to a simulated furnace. `cargo test --release -p rc-sim bench_g5f01 --
/// --ignored --nocapture`.
#[test]
#[ignore = "bench"]
fn bench_g5f01_pull_and_hopper() {
    let Some((mut p, dir)) = probe("g5f01-bench") else { return };
    p.run(
        "setblock 112 -59 112 minecraft:chest[facing=north,type=left]\nsetblock 111 -59 112 minecraft:chest[facing=north,type=right]\n\
         player 112 -59 110.5 0 0\nsetblock 110 -59 110 minecraft:furnace[facing=north]\nsetblock 110 -58 110 minecraft:hopper[facing=down]\n",
    );
    p.s.game_mode = 0;
    for at in [[112, -59, 112], [111, -59, 112]] {
        let full: Vec<(usize, &str, i32)> = (0..27).map(|i| (i, "minecraft:cobblestone", 64)).collect();
        put(&mut p, at, &full);
    }
    assert!(p.s.use_block([112, -59, 112]));
    let n = 20_000;
    let t = std::time::Instant::now();
    for _ in 0..n {
        if let Some(m) = p.s.life.menu.as_mut() {
            m.own[0] = None;
        }
        p.s.pull_menu();
    }
    let pull = t.elapsed().as_secs_f64() / f64::from(n) * 1.0e6;
    let t = std::time::Instant::now();
    for _ in 0..n {
        put(&mut p, [110, -58, 110], &[(0, "minecraft:raw_iron", 1)]);
        p.s.bs.hoppers.get_mut(&[110, -58, 110]).expect("hopper").1 = 0;
        p.s.hopper_try_move([110, -58, 110], None);
        let mut slots = p.s.slots([110, -59, 110]);
        slots[0] = None;
        p.s.store_slots([110, -59, 110], &slots);
    }
    let hop = t.elapsed().as_secs_f64() / f64::from(n) * 1.0e6;
    eprintln!("bench G5-F01: menu re-read (full double chest) {pull:.2} µs; hopper → furnace {hop:.2} µs");
    done(p, dir);
}