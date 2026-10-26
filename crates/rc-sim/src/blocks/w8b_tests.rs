//! W8b: loot of blocks broken without a player, noisy pistons, rain on blocks, bone
//! meal, trees, tools (axe, hoe, shovel), wax (test world `RUSTCRAFT_SOLO_WORLD`).

use super::*;
use crate::blocks::probe::{Probe, state_string};
use crate::player::Stack;

const BASE: &str = "fill 100 -60 100 131 -60 131 minecraft:stone\nfill 100 -59 100 131 -40 131 minecraft:air\n";

fn probe(tag: &str) -> Option<(Probe, std::path::PathBuf)> {
    let (world, data) = (crate::tests::test_world()?, rc_testdata::version_data()?);
    let dir = crate::tests::fresh_copy(&world, tag);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run(BASE);
    assert_eq!(name_at(&p, [100, -60, 100]), "minecraft:stone", "test area missing");
    p.s.pos = [110.5, -59.0, 110.5];
    Some((p, dir))
}

fn done(p: Probe, dir: std::path::PathBuf) {
    drop(p);
    let _ = std::fs::remove_dir_all(dir);
}

fn name_at(p: &Probe, at: P) -> String {
    state_string(&p.s, p.s.get(at))
}

fn items_named(p: &Probe, id: &str) -> i32 {
    p.s.life.drops.iter().filter(|d| !d.is_removed() && d.stack.id == id).map(|d| d.stack.count).sum()
}

fn hold(p: &mut Probe, id: &str, count: i32) {
    p.s.game_mode = 0;
    let slot = 36 + p.s.held as usize;
    p.s.slots[slot] = Some(Stack { id: id.to_owned(), count, components: None });
}

fn held(p: &Probe) -> Option<Stack> {
    p.s.slots[36 + p.s.held as usize].clone()
}

fn sounds(p: &Probe, name: &str) -> usize {
    let Some(id) = p.s.sound_id(name) else { return 0 };
    p.take_events()
        .iter()
        .filter(|e| {
            matches!(e, rc_client::Event::Sound(s) if matches!(s.sound, rc_proto_v777::play::SoundRef::Id(i) if i == id))
        })
        .count()
}

/// Block ticks without draining the events; `name` sounds counted over the ticks.
fn ticks_counting(p: &mut Probe, n: i32, name: &str) -> usize {
    let mut c = 0;
    for _ in 0..n {
        p.s.time += 1;
        p.s.tick_world_blocks();
        p.s.flush_block_changes();
        c += sounds(p, name);
    }
    c
}

/// Torch deprived of its support: it falls as an item (block loot, empty tool).
#[test]
fn torch_without_support_drops_item() {
    let Some((mut p, dir)) = probe("w8b-torch") else { return };
    p.run("setblock 105 -59 105 minecraft:torch\nsetblock 105 -60 105 minecraft:air\ntick 2\n");
    assert_eq!(name_at(&p, [105, -59, 105]), "minecraft:air");
    assert_eq!(items_named(&p, "minecraft:torch"), 1, "torch dropped");
    done(p, dir);
}

/// Water that carries a torch away: loot dropped.
#[test]
fn water_washes_torch_with_drop() {
    let Some((mut p, dir)) = probe("w8b-wash") else { return };
    p.run("setblock 110 -59 110 minecraft:torch\nsetblock 111 -59 110 minecraft:water\ntick 20\n");
    assert!(name_at(&p, [110, -59, 110]).starts_with("minecraft:water"), "{}", name_at(&p, [110, -59, 110]));
    assert_eq!(items_named(&p, "minecraft:torch"), 1, "washed-away torch dropped");
    done(p, dir);
}

/// Piston that crushes a torch: item dropped; extend and retract sounds.
#[test]
fn piston_crushes_torch_and_sounds() {
    let Some((mut p, dir)) = probe("w8b-piston") else { return };
    p.run("setblock 101 -59 115 minecraft:piston[facing=east]\nsetblock 102 -59 115 minecraft:torch\n");
    let _ = p.take_events();
    p.run("setblock 100 -59 115 minecraft:redstone_block\n");
    let ext = ticks_counting(&mut p, 4, "minecraft:block.piston.extend");
    assert_eq!(items_named(&p, "minecraft:torch"), 1, "crushed torch dropped");
    assert_eq!(ext, 1, "extend sound");
    p.run("setblock 100 -59 115 minecraft:air\n");
    assert_eq!(ticks_counting(&mut p, 4, "minecraft:block.piston.contract"), 1, "retract sound");
    done(p, dir);
}

/// Bone meal on wheat: age + 2..5, one bone meal consumed; age-1 torchflower crop → flower.
#[test]
fn bone_meal_grows_crops() {
    let Some((mut p, dir)) = probe("w8b-crop") else { return };
    p.run("setblock 104 -60 104 minecraft:farmland\nsetblock 104 -59 104 minecraft:wheat\n");
    p.run("setblock 106 -60 104 minecraft:farmland\nsetblock 106 -59 104 minecraft:torchflower_crop[age=1]\n");
    hold(&mut p, "minecraft:bone_meal", 5);
    p.s.use_item_on_block([104, -59, 104], Dir::Up, [0.5, 0.5, 0.5]);
    let age: i32 = p.s.v().get_int(p.s.get([104, -59, 104]), "age");
    assert!((2..=5).contains(&age), "age {age}");
    assert_eq!(held(&p).map(|s| s.count), Some(4));
    p.s.use_item_on_block([106, -59, 104], Dir::Up, [0.5, 0.5, 0.5]);
    assert_eq!(name_at(&p, [106, -59, 104]), "minecraft:torchflower");
    // Short grass → tall grass (two halves).
    p.run("setblock 108 -60 104 minecraft:grass_block\nsetblock 108 -59 104 minecraft:short_grass\n");
    p.s.use_item_on_block([108, -59, 104], Dir::Up, [0.5, 0.5, 0.5]);
    assert_eq!(name_at(&p, [108, -59, 104]), "minecraft:tall_grass[half=lower]");
    assert_eq!(name_at(&p, [108, -58, 104]), "minecraft:tall_grass[half=upper]");
    done(p, dir);
}

/// Torchflower crop: random ticks lead it up to the flower (maximum age 2).
#[test]
fn torchflower_crop_blooms_by_random_ticks() {
    let Some((mut p, dir)) = probe("w8b-torchflower") else { return };
    p.run(
        "setblock 104 -60 108 minecraft:farmland[moisture=7]\nsetblock 104 -59 108 minecraft:torchflower_crop[age=1]\n",
    );
    for _ in 0..4000 {
        let s = p.s.get([104, -59, 108]);
        if p.s.state_name(s) != "minecraft:torchflower_crop" {
            break;
        }
        p.s.nature_random_tick(s, [104, -59, 108]);
    }
    assert_eq!(name_at(&p, [104, -59, 108]), "minecraft:torchflower");
    done(p, dir);
}

/// Oak sapling under bone meal: stage 1 then a tree (log in place of the sapling).
#[test]
fn bone_meal_grows_oak_tree() {
    let Some((mut p, dir)) = probe("w8b-tree") else { return };
    p.run("fill 110 -60 120 112 -60 122 minecraft:dirt\nsetblock 111 -59 121 minecraft:oak_sapling\n");
    hold(&mut p, "minecraft:bone_meal", 64);
    for _ in 0..60 {
        if p.s.state_name(p.s.get([111, -59, 121])) != "minecraft:oak_sapling" {
            break;
        }
        p.s.use_item_on_block([111, -59, 121], Dir::Up, [0.5, 0.5, 0.5]);
    }
    let base = name_at(&p, [111, -59, 121]);
    assert!(base.starts_with("minecraft:oak_log"), "log expected: {base}");
    assert!((-58..-50).any(|y| name_at(&p, [111, y, 121]).starts_with("minecraft:oak_log")), "trunk");
    done(p, dir);
}

/// Decaying leaves: their loot goes through the table (empty tool).
#[test]
fn decaying_leaves_drop_loot() {
    let Some((mut p, dir)) = probe("w8b-leaves") else { return };
    // 200 isolated leaves (distance 7): at least one sapling, stick or apple.
    p.run("fill 100 -59 125 119 -50 125 minecraft:oak_leaves[distance=7,persistent=false]\n");
    for y in -59..-49 {
        for x in 100..120 {
            let s = p.s.get([x, y, 125]);
            p.s.nature_random_tick(s, [x, y, 125]);
        }
    }
    assert_eq!(name_at(&p, [110, -55, 125]), "minecraft:air");
    let n = items_named(&p, "minecraft:oak_sapling")
        + items_named(&p, "minecraft:stick")
        + items_named(&p, "minecraft:apple");
    assert!(n > 0, "leaf loot");
    done(p, dir);
}

/// Axe: stripped log (axis kept), wear 1; hoe: farmland (not from below);
/// shovel: path (not under a block); honeycomb: waxed copper, honeycomb consumed.
#[test]
fn tools_transform_blocks() {
    let Some((mut p, dir)) = probe("w8b-tools") else { return };
    p.run("setblock 104 -59 112 minecraft:oak_log[axis=x]\n");
    hold(&mut p, "minecraft:iron_axe", 1);
    p.s.use_item_on_block([104, -59, 112], Dir::Up, [0.5, 1.0, 0.5]);
    assert_eq!(name_at(&p, [104, -59, 112]), "minecraft:stripped_oak_log[axis=x]");
    assert_eq!(p.s.life.items.damage(&held(&p).expect("axe")), 1, "wear");

    p.run("setblock 106 -60 112 minecraft:dirt\n");
    hold(&mut p, "minecraft:stone_hoe", 1);
    p.s.use_item_on_block([106, -60, 112], Dir::Down, [0.5, 0.0, 0.5]);
    assert_eq!(name_at(&p, [106, -60, 112]), "minecraft:dirt", "not from below");
    p.s.use_item_on_block([106, -60, 112], Dir::Up, [0.5, 1.0, 0.5]);
    assert_eq!(name_at(&p, [106, -60, 112]), "minecraft:farmland[moisture=0]");

    p.run("setblock 108 -60 112 minecraft:grass_block\nsetblock 108 -59 112 minecraft:stone\n");
    hold(&mut p, "minecraft:wooden_shovel", 1);
    p.s.use_item_on_block([108, -60, 112], Dir::North, [0.5, 0.5, 0.0]);
    assert!(name_at(&p, [108, -60, 112]).starts_with("minecraft:grass_block"), "block above");
    p.run("setblock 108 -59 112 minecraft:air\n");
    p.s.use_item_on_block([108, -60, 112], Dir::North, [0.5, 0.5, 0.0]);
    assert_eq!(name_at(&p, [108, -60, 112]), "minecraft:dirt_path");

    p.run("setblock 110 -59 112 minecraft:exposed_copper\n");
    hold(&mut p, "minecraft:honeycomb", 3);
    p.s.use_item_on_block([110, -59, 112], Dir::Up, [0.5, 1.0, 0.5]);
    assert_eq!(name_at(&p, [110, -59, 112]), "minecraft:waxed_exposed_copper");
    assert_eq!(held(&p).map(|s| s.count), Some(2));
    // Axe on waxed copper: unwaxing.
    hold(&mut p, "minecraft:iron_axe", 1);
    p.s.use_item_on_block([110, -59, 112], Dir::Up, [0.5, 1.0, 0.5]);
    assert_eq!(name_at(&p, [110, -59, 112]), "minecraft:exposed_copper");
    p.s.use_item_on_block([110, -59, 112], Dir::Up, [0.5, 1.0, 0.5]);
    assert_eq!(name_at(&p, [110, -59, 112]), "minecraft:copper_block", "scraping");
    done(p, dir);
}

/// Rain: an empty cauldron under the open sky ends up filled with water; without rain, never.
#[test]
fn rain_fills_cauldron() {
    let Some((mut p, dir)) = probe("w8b-rain") else { return };
    p.run("setblock 120 -59 120 minecraft:cauldron\n");
    for _ in 0..400 {
        p.s.tick_precipitation([120, -59, 120]);
    }
    assert_eq!(name_at(&p, [120, -59, 120]), "minecraft:cauldron", "clear weather");
    p.s.life.weather.rain_level = 1.0;
    assert!(p.s.bs_raining());
    assert!(p.s.bs_raining_at([120, -58, 120]), "rain above the cauldron");
    for _ in 0..400 {
        p.s.tick_precipitation([120, -59, 120]);
    }
    assert!(name_at(&p, [120, -59, 120]).starts_with("minecraft:water_cauldron"), "{}", name_at(&p, [120, -59, 120]));
    done(p, dir);
}

/// Right click: cake eaten (hunger), candle placed then candle cake eaten (candle dropped),
/// flower pot filled then emptied, composter, cauldron (bucket, bottle), TNT with a flint and steel, dragon
/// egg moved, candle put out by hand.
#[test]
fn right_click_block_interactions() {
    let Some((mut p, dir)) = probe("w8b-use") else { return };
    let up = [0.5f32, 1.0, 0.5];
    p.run(
        "setblock 102 -59 102 minecraft:cake
",
    );
    p.s.slots[36 + p.s.held as usize] = None;
    p.s.game_mode = 0;
    p.s.life.vit.food = 10;
    p.s.use_item_on_block([102, -59, 102], Dir::Up, up);
    assert_eq!(name_at(&p, [102, -59, 102]), "minecraft:cake[bites=1]");
    assert_eq!(p.s.life.vit.food, 12);

    p.run(
        "setblock 103 -59 102 minecraft:cake
",
    );
    hold(&mut p, "minecraft:red_candle", 2);
    p.s.use_item_on_block([103, -59, 102], Dir::Up, up);
    assert_eq!(name_at(&p, [103, -59, 102]), "minecraft:red_candle_cake[lit=false]");
    assert_eq!(held(&p).map(|s| s.count), Some(1));
    p.s.slots[36 + p.s.held as usize] = None;
    p.s.use_item_on_block([103, -59, 102], Dir::Up, up);
    assert_eq!(name_at(&p, [103, -59, 102]), "minecraft:cake[bites=1]");
    assert_eq!(items_named(&p, "minecraft:red_candle"), 1, "candle dropped");

    p.run(
        "setblock 104 -59 102 minecraft:flower_pot
",
    );
    hold(&mut p, "minecraft:poppy", 1);
    p.s.use_item_on_block([104, -59, 102], Dir::Up, up);
    assert_eq!(name_at(&p, [104, -59, 102]), "minecraft:potted_poppy");
    assert!(held(&p).is_none());
    p.s.use_item_on_block([104, -59, 102], Dir::Up, up);
    assert_eq!(name_at(&p, [104, -59, 102]), "minecraft:flower_pot");
    assert_eq!(held(&p).map(|s| s.id), Some("minecraft:poppy".to_owned()), "poppy given back");

    p.run(
        "setblock 105 -59 102 minecraft:composter[level=8]
",
    );
    p.s.slots[36 + p.s.held as usize] = None;
    p.s.use_item_on_block([105, -59, 102], Dir::Up, up);
    assert_eq!(name_at(&p, [105, -59, 102]), "minecraft:composter[level=0]");
    assert_eq!(items_named(&p, "minecraft:bone_meal"), 1);
    hold(&mut p, "minecraft:pumpkin_pie", 64);
    p.s.use_item_on_block([105, -59, 102], Dir::Up, up);
    assert_eq!(name_at(&p, [105, -59, 102]), "minecraft:composter[level=1]", "pie: always one layer");
    assert_eq!(held(&p).map(|s| s.count), Some(63));

    p.run(
        "setblock 106 -59 102 minecraft:cauldron
",
    );
    hold(&mut p, "minecraft:water_bucket", 1);
    p.s.use_item_on_block([106, -59, 102], Dir::Up, up);
    assert_eq!(name_at(&p, [106, -59, 102]), "minecraft:water_cauldron[level=3]");
    assert_eq!(held(&p).map(|s| s.id), Some("minecraft:bucket".to_owned()));
    hold(&mut p, "minecraft:glass_bottle", 1);
    p.s.use_item_on_block([106, -59, 102], Dir::Up, up);
    assert_eq!(name_at(&p, [106, -59, 102]), "minecraft:water_cauldron[level=2]");
    assert_eq!(held(&p).map(|s| s.id), Some("minecraft:potion".to_owned()));
    hold(&mut p, "minecraft:bucket", 1);
    p.s.use_item_on_block([106, -59, 102], Dir::Up, up);
    assert_eq!(name_at(&p, [106, -59, 102]), "minecraft:water_cauldron[level=2]", "bucket: full cauldron only");

    p.run(
        "setblock 107 -59 102 minecraft:tnt
",
    );
    hold(&mut p, "minecraft:flint_and_steel", 1);
    let before = p.s.bs.entities.list.len();
    p.s.use_item_on_block([107, -59, 102], Dir::Up, up);
    assert_eq!(name_at(&p, [107, -59, 102]), "minecraft:air");
    assert_eq!(p.s.bs.entities.list.len(), before + 1, "TNT primed");
    assert_eq!(name_at(&p, [107, -58, 102]), "minecraft:air", "no fire placed");

    p.run(
        "setblock 108 -59 102 minecraft:dragon_egg
",
    );
    p.s.slots[36 + p.s.held as usize] = None;
    p.s.use_item_on_block([108, -59, 102], Dir::Up, up);
    assert_eq!(name_at(&p, [108, -59, 102]), "minecraft:air", "egg moved");

    p.run(
        "setblock 109 -59 102 minecraft:candle[lit=true]
",
    );
    p.s.use_item_on_block([109, -59, 102], Dir::Up, up);
    assert!(name_at(&p, [109, -59, 102]).contains("lit=false"), "{}", name_at(&p, [109, -59, 102]));
    done(p, dir);
}

/// Sponge placed near water: water removed, sponge soaked.
#[test]
fn sponge_absorbs_water() {
    let Some((mut p, dir)) = probe("w8b-sponge") else { return };
    p.run("fill 112 -59 106 116 -59 108 minecraft:water\n");
    p.run("setblock 111 -59 107 minecraft:sponge\n");
    assert_eq!(name_at(&p, [111, -59, 107]), "minecraft:wet_sponge");
    assert_eq!(name_at(&p, [114, -59, 107]), "minecraft:air");
    done(p, dir);
}

/// Grass: snowy under a placed snow layer, no longer under air.
#[test]
fn grass_becomes_snowy() {
    let Some((mut p, dir)) = probe("w8b-snowy") else { return };
    p.run("setblock 118 -60 106 minecraft:grass_block\n");
    p.run("setblock 118 -59 106 minecraft:snow\n");
    assert_eq!(name_at(&p, [118, -60, 106]), "minecraft:grass_block[snowy=true]");
    p.run("setblock 118 -59 106 minecraft:air\n");
    assert_eq!(name_at(&p, [118, -60, 106]), "minecraft:grass_block[snowy=false]");
    done(p, dir);
}

/// Heads: standing, turned like the gaze (without a half turn); against a wall, wall variant.
/// Banner against a wall: wall variant.
#[test]
fn heads_and_banners_placement() {
    let Some((mut p, dir)) = probe("w8b-heads") else { return };
    // Looking south (yaw 0) and downward: standing head, rotation 0.
    p.s.yaw = 0.0;
    p.s.pitch = 60.0;
    hold(&mut p, "minecraft:skeleton_skull", 1);
    p.s.place_item("minecraft:skeleton_skull", [120, -60, 110], Dir::Up, [120.5, -59.0, 110.5]);
    assert_eq!(name_at(&p, [120, -59, 110]), "minecraft:skeleton_skull[powered=false,rotation=0]");
    // North face of a wall aimed at horizontally while looking south: wall variant facing north.
    p.s.pitch = 0.0;
    p.run("setblock 122 -59 112 minecraft:stone\n");
    p.s.place_item("minecraft:skeleton_skull", [122, -59, 112], Dir::North, [122.5, -58.5, 112.0]);
    assert_eq!(name_at(&p, [122, -59, 111]), "minecraft:skeleton_wall_skull[facing=north,powered=false]");
    p.run("setblock 124 -59 112 minecraft:stone\n");
    p.s.place_item("minecraft:white_banner", [124, -59, 112], Dir::North, [124.5, -58.5, 112.0]);
    assert_eq!(name_at(&p, [124, -59, 111]), "minecraft:white_wall_banner[facing=north]");
    done(p, dir);
}

/// Iron door broken by hand (upper half): in 26.3 no door requires a tool, the door drops once.
#[test]
fn iron_door_by_hand_drops_once() {
    let Some((mut p, dir)) = probe("w8c-door") else { return };
    p.run(
        "setblock 105 -59 105 minecraft:iron_door[half=lower]\nsetblock 105 -58 105 minecraft:iron_door[half=upper]\n",
    );
    p.s.game_mode = 0;
    p.s.slots[36 + p.s.held as usize] = None;
    assert_eq!(items_named(&p, "minecraft:iron_door"), 0, "before");
    p.s.try_break([105, -58, 105]);
    p.run("tick 2\n");
    assert_eq!(name_at(&p, [105, -59, 105]), "minecraft:air");
    assert_eq!(items_named(&p, "minecraft:iron_door"), 1, "one door dropped");
    done(p, dir);
}

/// Lone dark oak sapling: bone meal has no effect, nothing is consumed.
#[test]
fn lone_dark_oak_sapling_ignores_bone_meal() {
    let Some((mut p, dir)) = probe("w8c-dark") else { return };
    p.run("fill 110 -60 120 112 -60 122 minecraft:dirt\nsetblock 111 -59 121 minecraft:dark_oak_sapling\n");
    hold(&mut p, "minecraft:bone_meal", 5);
    for _ in 0..10 {
        p.s.use_item_on_block([111, -59, 121], Dir::Up, [0.5, 0.5, 0.5]);
    }
    assert_eq!(name_at(&p, [111, -59, 121]), "minecraft:dark_oak_sapling[stage=0]");
    assert_eq!(held(&p).map(|s| s.count), Some(5), "bone meal not consumed");
    done(p, dir);
}

/// TNT lit with a flint and steel in adventure mode without `can_break`: nothing is primed.
#[test]
fn adventure_flint_does_not_prime_tnt() {
    let Some((mut p, dir)) = probe("w8c-tnt") else { return };
    p.run("setblock 105 -59 105 minecraft:tnt\n");
    hold(&mut p, "minecraft:flint_and_steel", 1);
    p.s.game_mode = 2;
    p.s.use_item_on_block([105, -59, 105], Dir::Up, [0.5, 0.5, 0.5]);
    assert_eq!(name_at(&p, [105, -59, 105]), "minecraft:tnt[unstable=false]");
    assert!(p.s.bs.entities.list.is_empty());
    p.s.game_mode = 0;
    p.s.use_item_on_block([105, -59, 105], Dir::Up, [0.5, 0.5, 0.5]);
    assert_eq!(p.s.bs.entities.list.last().and_then(|e| e.owner), Some(crate::mobs::mob::Ent::Player));
    done(p, dir);
}

/// Piston probes (G5-F14), ignored by default and failing explicitly without a test world.
/// Launch:
/// `RUSTCRAFT_SOLO_WORLD=%APPDATA%\.minecraft\saves\Test cargo test --release -p rc-sim -- --ignored blocks::growth::w8b_tests`
#[allow(dead_code)]
const G5F14_PROBES: () = ();

fn probe_req(tag: &str) -> (Probe, std::path::PathBuf) {
    probe(tag).expect("test world or data missing: RUSTCRAFT_SOLO_WORLD=<copy of the Test world> (G5F14_PROBES)")
}

/// Full server ticks (game order: blocks, entities, then block entities).
fn game_ticks(p: &mut Probe, n: i32) {
    for _ in 0..n {
        p.s.game_tick();
        p.s.tick += 1;
    }
}

fn tick_blocks(p: &mut Probe, n: i32) {
    for _ in 0..n {
        p.s.time += 1;
        p.s.tick_world_blocks();
        p.s.flush_block_changes();
    }
}

/// G5-F14: at the start of the retraction, the base turned into a moving block updates the shape of
/// its neighbours (the fence attached to the base's solid back detaches from it), then reattaches to it.
#[test]
#[ignore = "test world required: see G5F14_PROBES"]
fn piston_retraction_updates_neighbour_shapes() {
    let (mut p, dir) = probe_req("g5f14-shape");
    p.run("setblock 101 -59 115 minecraft:piston[facing=east]\nsetblock 100 -59 115 minecraft:oak_fence\n");
    p.run("setblock 101 -59 114 minecraft:redstone_block\n");
    tick_blocks(&mut p, 4);
    assert!(name_at(&p, [100, -59, 115]).contains("east=true"), "{}", name_at(&p, [100, -59, 115]));
    p.run("setblock 101 -59 114 minecraft:air\n");
    tick_blocks(&mut p, 1);
    assert!(name_at(&p, [101, -59, 115]).starts_with("minecraft:moving_piston"), "{}", name_at(&p, [101, -59, 115]));
    assert!(name_at(&p, [100, -59, 115]).contains("east=false"), "{}", name_at(&p, [100, -59, 115]));
    tick_blocks(&mut p, 4);
    assert_eq!(name_at(&p, [101, -59, 115]), "minecraft:piston[extended=false,facing=east]");
    assert!(name_at(&p, [100, -59, 115]).contains("east=true"), "{}", name_at(&p, [100, -59, 115]));
    done(p, dir);
}

/// G5-F14: a wire crushed by the head notifies the diagonal wires (indirect shape): the wire
/// placed higher no longer connects downward.
#[test]
#[ignore = "test world required: see G5F14_PROBES"]
fn crushed_wire_updates_diagonal_wire() {
    let (mut p, dir) = probe_req("g5f14-wire");
    p.run("setblock 101 -59 115 minecraft:piston[facing=east]\nsetblock 102 -59 115 minecraft:redstone_wire\n");
    // Wire connected to the south and, going down, to the west (otherwise an isolated wire is a cross).
    p.run("setblock 103 -59 115 minecraft:stone\nsetblock 103 -59 116 minecraft:stone\n");
    p.run("setblock 103 -58 116 minecraft:redstone_wire\nsetblock 103 -58 115 minecraft:redstone_wire\n");
    tick_blocks(&mut p, 2);
    let w = name_at(&p, [103, -58, 115]);
    assert!(w.contains("west=side") && w.contains("east=none"), "{w}");
    p.run("setblock 101 -59 114 minecraft:redstone_block\n");
    tick_blocks(&mut p, 4);
    assert!(name_at(&p, [102, -59, 115]).starts_with("minecraft:piston_head"), "{}", name_at(&p, [102, -59, 115]));
    assert!(name_at(&p, [103, -58, 115]).contains("west=none"), "{}", name_at(&p, [103, -58, 115]));
    done(p, dir);
}

fn add_pig(p: &mut Probe, id: i32, pos: [f64; 3]) {
    let mut m = crate::mobs::mob::Mob::new(
        crate::mobs::species::Kind::Pig,
        id,
        id as u128,
        pos,
        0.0,
        crate::mobs::jrand::JRand::new(1),
    );
    m.on_ground = true;
    p.s.add_mob(m);
}

fn pig(p: &Probe, id: i32) -> &crate::mobs::mob::Mob {
    p.s.mobs.list.iter().flatten().find(|m| m.id == id).expect("pig")
}

/// G5-F14: the head coming out pushes the mob and the item in front of it out of the cell where it
/// arrives (the head ends at x = 103, the entity beyond).
#[test]
#[ignore = "test world required: see G5F14_PROBES"]
fn piston_head_pushes_mob_and_item() {
    let (mut p, dir) = probe_req("g5f14-push");
    p.run("setblock 101 -59 115 minecraft:piston[facing=east]\nsetblock 101 -59 117 minecraft:piston[facing=east]\n");
    add_pig(&mut p, 900_001, [102.5, -59.0, 115.5]);
    let stack = Stack { id: "minecraft:stone".to_owned(), count: 1, components: None };
    p.s.spawn_item([102.5, -59.0, 117.5], [0.0; 3], stack, 10);
    p.run("setblock 101 -59 116 minecraft:redstone_block\n");
    tick_blocks(&mut p, 4);
    assert!(name_at(&p, [102, -59, 115]).starts_with("minecraft:piston_head"), "{}", name_at(&p, [102, -59, 115]));
    let x = pig(&p, 900_001).pos[0];
    assert!(x - 0.45 >= 103.0, "pig at x = {x}");
    let item = p.s.life.drops.iter().find(|d| d.stack.id == "minecraft:stone").expect("item");
    assert!(item.pos[0] - 0.125 >= 103.0, "item at x = {}", item.pos[0]);
    done(p, dir);
}

/// G5-F14: a slime block pushed upward launches the mob standing on it (vertical
/// velocity 1) and lifts it along.
#[test]
#[ignore = "test world required: see G5F14_PROBES"]
fn slime_block_launches_mob() {
    let (mut p, dir) = probe_req("g5f14-slime");
    p.run("setblock 106 -59 110 minecraft:piston[facing=up]\nsetblock 106 -58 110 minecraft:slime_block\n");
    add_pig(&mut p, 900_002, [106.5, -57.0, 110.5]);
    p.run("setblock 107 -59 110 minecraft:redstone_block\n");
    tick_blocks(&mut p, 2);
    let m = pig(&p, 900_002);
    assert_eq!(m.vel[1], 1.0, "{:?}", m.vel);
    assert!(m.pos[1] > -57.0, "{:?}", m.pos);
    done(p, dir);
}

/// G5-F14: the mob ticks before the moving block (game order); pushed east by
/// the head, it is no longer on the ground at the end of the tick (the server is authoritative: ground recomputed
/// even without vertical movement).
#[test]
#[ignore = "test world required: see G5F14_PROBES"]
fn horizontal_push_ungrounds_mob() {
    let (mut p, dir) = probe_req("g5f14-ground");
    p.run("setblock 101 -59 115 minecraft:piston[facing=east]\n");
    add_pig(&mut p, 900_003, [102.5, -59.0, 115.5]);
    game_ticks(&mut p, 2);
    assert!(pig(&p, 900_003).on_ground, "pig standing");
    let x0 = pig(&p, 900_003).pos[0];
    p.run("setblock 101 -59 116 minecraft:redstone_block\n");
    game_ticks(&mut p, 1);
    let m = pig(&p, 900_003);
    assert!(m.pos[0] > x0 + 0.2, "pig not pushed: x {} -> {}", x0, m.pos[0]);
    assert!(!m.on_ground, "pushed pig still on the ground");
    done(p, dir);
}

/// G5-F14: launched by the slime during the block-entity phase, the pig keeps its vertical
/// velocity 1 until its own next tick (in the game, the momentum is not consumed the same
/// tick), then it rises by almost one block.
#[test]
#[ignore = "test world required: see G5F14_PROBES"]
fn slime_launch_waits_for_next_mob_tick() {
    let (mut p, dir) = probe_req("g5f14-slime2");
    p.run("setblock 106 -59 110 minecraft:piston[facing=up]\nsetblock 106 -58 110 minecraft:slime_block\n");
    add_pig(&mut p, 900_004, [106.5, -57.0, 110.5]);
    game_ticks(&mut p, 2);
    let y0 = pig(&p, 900_004).pos[1];
    p.run("setblock 107 -59 110 minecraft:redstone_block\n");
    game_ticks(&mut p, 1);
    let m = pig(&p, 900_004);
    assert!(m.pos[1] > y0, "pig not lifted: {:?}", m.pos);
    assert_eq!(m.vel[1], 1.0, "momentum consumed on the launch tick: {:?}", m.vel);
    let y1 = m.pos[1];
    game_ticks(&mut p, 1);
    let m = pig(&p, 900_004);
    assert!(m.vel[1] < 1.0 && m.pos[1] > y1 + 0.5, "the pig did not tick: {:?} {:?}", m.pos, m.vel);
    done(p, dir);
}

/// G5-F14: primed TNT is pushed by the head like any "pushable" entity.
#[test]
#[ignore = "test world required: see G5F14_PROBES"]
fn piston_head_pushes_primed_tnt() {
    let (mut p, dir) = probe_req("g5f14-tnt");
    p.run("setblock 101 -59 115 minecraft:piston[facing=east]\nsetblock 102 -59 115 minecraft:tnt\n");
    hold(&mut p, "minecraft:flint_and_steel", 1);
    p.s.use_item_on_block([102, -59, 115], Dir::Up, [0.5, 0.5, 0.5]);
    assert_eq!(p.s.bs.entities.list.len(), 1, "TNT primed");
    p.run("setblock 101 -59 116 minecraft:redstone_block\n");
    tick_blocks(&mut p, 4);
    assert!(name_at(&p, [102, -59, 115]).starts_with("minecraft:piston_head"), "{}", name_at(&p, [102, -59, 115]));
    let x = p.s.bs.entities.list[0].pos[0];
    assert!(x - 0.49 >= 103.0, "TNT at x = {x}");
    done(p, dir);
}

/// Block tick noted by a feature (closed eyeblossom of the pale garden's simple block, delay 1): the
/// copy into the live world schedules it with its delay; placing the block alone schedules
/// none. Independent of the test world's biomes (the pale garden feature is not always there).
#[test]
fn placed_eyeblossom_schedules_its_tick() {
    let Some((mut p, dir)) = probe("g6-eyeblossom") else { return };
    let at = [110, -58, 110];
    p.run("setblock 110 -59 110 minecraft:grass_block\nsetblock 110 -58 110 minecraft:closed_eyeblossom\n");
    let id = p.s.bs.t.block_id("minecraft:closed_eyeblossom");
    let ch = (at[0] >> 4, at[2] >> 4);
    assert!(!p.s.bs.block_ticks.chunk_ticks(ch).iter().any(|t| t.pos == at), "placing alone scheduled a tick");
    let (wd, dg) = p.s.place_data().expect("generation data");
    let closed = wd.blocks.block_id("closed_eyeblossom").expect("closed eyeblossom");
    let mut dc = p.s.deco_chunk(ch.0, ch.1, &dg, &wd);
    dc.block_ticks.push((at, closed, 1));
    assert_eq!(p.s.apply_deco(&dc, &wd.blocks), 0, "no block changed");
    let ticks = p.s.bs.block_ticks.chunk_ticks(ch);
    assert!(
        ticks.iter().any(|t| t.pos == at && t.kind == u32::from(id) && t.at == p.s.time + 1),
        "eyeblossom tick missing: {ticks:?}"
    );
    done(p, dir);
}

/// Fluid tick noted by a feature without a block change (neighbour of a geode crack):
/// only the copy into the live world schedules it, at that very moment.
#[test]
fn deco_fluid_tick_reaches_live_world() {
    let Some((mut p, dir)) = probe("g6-fluidtick") else { return };
    let w = [120, -58, 120];
    p.run("fill 119 -59 119 121 -57 121 minecraft:stone\nsetblock 120 -58 120 minecraft:water\ntick 20\n");
    let ch = (w[0] >> 4, w[2] >> 4);
    assert!(!p.s.bs.fluid_ticks.chunk_ticks(ch).iter().any(|t| t.pos == w), "closed water still pending");
    let (wd, dg) = p.s.place_data().expect("generation data");
    let mut dc = p.s.deco_chunk(ch.0, ch.1, &dg, &wd);
    dc.fluid_ticks.push((w, 0));
    assert_eq!(p.s.apply_deco(&dc, &wd.blocks), 0, "no block changed");
    let ticks = p.s.bs.fluid_ticks.chunk_ticks(ch);
    assert!(ticks.iter().any(|t| t.pos == w && t.kind == 1 && t.at == p.s.time), "water tick missing: {ticks:?}");
    done(p, dir);
}

/// Shovel on grass with an item that parries in the off hand: the intent is to parry, not
/// to make a path. The test reads the stack's `blocks_attacks` component (added to a stick,
/// removed from a shield), not only the item's default one.
#[test]
fn shovel_respects_offhand_parry_component() {
    use rc_proto::nbt::Nbt;
    let Some((mut p, dir)) = probe("w8b-parry") else { return };
    let with = |id: &str, key: &str| Stack {
        id: id.to_owned(),
        count: 1,
        components: Some(Nbt::Compound(vec![(key.to_owned(), Nbt::Compound(Vec::new()))])),
    };
    p.s.sneaking = false;
    p.run("setblock 108 -60 114 minecraft:grass_block\n");
    hold(&mut p, "minecraft:wooden_shovel", 1);
    p.s.slots[45] = Some(with("minecraft:stick", "minecraft:blocks_attacks"));
    p.s.use_item_on_block([108, -60, 114], Dir::Up, [0.5, 1.0, 0.5]);
    assert!(name_at(&p, [108, -60, 114]).starts_with("minecraft:grass_block"), "parrying stick ignored");
    p.s.slots[45] = Some(with("minecraft:shield", "!minecraft:blocks_attacks"));
    p.s.use_item_on_block([108, -60, 114], Dir::Up, [0.5, 1.0, 0.5]);
    assert_eq!(name_at(&p, [108, -60, 114]), "minecraft:dirt_path", "shield without parry counted as parrying");
    done(p, dir);
}
