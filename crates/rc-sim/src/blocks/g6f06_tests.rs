//! G6-F06: placement, survival and shapes of blocks (rules by family). Fresh flat world of
//! `g6f01_tests` (grass floor at y = −61): the 26.3 cache and the client jar are required (the test
//! fails without them). Each case fails on the old code (see the assertion message).

use rc_client::Event;
use rc_proto::nbt::Nbt;

use super::g6f01_tests::{W, Y, stack, world};
use super::*;
use crate::player::Stack;

const UP: [f32; 3] = [0.5, 1.0, 0.5];

/// Click on top of the ground (grass) at `(x, z)` with `item` in hand.
fn place_on_floor(w: &mut W, x: i32, z: i32, item: &str) {
    w.hold(stack(item, 1));
    w.click([x, Y - 1, z], Dir::Up, UP);
}

fn with_components(id: &str, comps: Vec<(&str, Nbt)>) -> Option<Stack> {
    let c = comps.into_iter().map(|(k, v)| (k.to_owned(), v)).collect();
    Some(Stack { id: id.to_owned(), count: 1, components: Some(Nbt::Compound(c)) })
}

fn level_events(evs: &[Event], kind: i32) -> usize {
    evs.iter().filter(|e| matches!(e, Event::LevelEvent { kind: k, .. } if *k == kind)).count()
}

/// Copper chests: two variants join (the less oxidized wins at placement), and
/// a half that ages drags the other along instead of detaching.
#[test]
fn copper_chests_join_across_variants_and_age_together() {
    let mut w = world("g6f06-chest");
    w.p.s.yaw = 0.0; // facing south: chest faces north
    w.set([3, Y, 3], "minecraft:copper_chest[facing=north]");
    place_on_floor(&mut w, 2, 3, "minecraft:exposed_copper_chest");
    let a = w.name([2, Y, 3]);
    assert!(a.starts_with("minecraft:copper_chest[facing=north,type=left"), "joined, less oxidized: {a}");
    assert!(w.name([3, Y, 3]).contains("type=right"), "neighbour joined: {}", w.name([3, Y, 3]));
    // The right half ages (direct placement, like oxidation): the left follows.
    let v = w.p.s.v();
    let ex = w.p.s.bs.t.block_id("minecraft:exposed_copper_chest");
    let st = v.set(v.set(v.default_state(ex), "facing", "north"), "type", "right");
    w.p.s.set_block([3, Y, 3], st, flag::ALL);
    let a = w.name([2, Y, 3]);
    assert!(a.starts_with("minecraft:exposed_copper_chest[facing=north,type=left"), "oxidized along with its neighbour: {a}");
    // Wooden chest against copper: never joined.
    w.set([6, Y, 3], "minecraft:chest[facing=north]");
    place_on_floor(&mut w, 5, 3, "minecraft:copper_chest");
    assert!(w.name([5, Y, 3]).contains("type=single"), "{}", w.name([5, Y, 3]));
}

/// Plants: kelp and seagrass refused on dry land; kelp placed under kelp → stem;
/// stacking up to 4; propagule; small dripleaf double; pitcher crop (carnivorous plant).
#[test]
fn plant_placement_rules() {
    let mut w = world("g6f06-plants");
    // Kelp, seagrass on dry land: nothing.
    place_on_floor(&mut w, 1, 1, "minecraft:kelp");
    assert_eq!(w.name([1, Y, 1]), "minecraft:air", "kelp on dry land refused");
    place_on_floor(&mut w, 1, 2, "minecraft:seagrass");
    assert_eq!(w.name([1, Y, 2]), "minecraft:air", "seagrass on dry land refused");
    // In water, under kelp: a piece of stem.
    w.set([1, Y, 3], "minecraft:water");
    w.set([1, Y + 1, 3], "minecraft:water");
    w.set([1, Y + 1, 3], "minecraft:kelp");
    place_on_floor(&mut w, 1, 3, "minecraft:kelp");
    assert_eq!(w.name([1, Y, 3]), "minecraft:kelp_plant", "kelp under kelp: stem");

    // Stacking: same item → +1 up to 4; sneaking: no stacking.
    for (at, block, prop) in [
        ([3, Y, 1], "minecraft:sea_pickle", "pickles"),
        ([3, Y, 2], "minecraft:pink_petals", "flower_amount"),
        ([3, Y, 3], "minecraft:wildflowers", "flower_amount"),
        ([3, Y, 4], "minecraft:leaf_litter", "segment_amount"),
        ([3, Y, 5], "minecraft:turtle_egg", "eggs"),
    ] {
        w.set(at, block);
        for n in 2..=5 {
            w.hold(stack(block, 1));
            w.click(at, Dir::Up, [0.5, 0.1, 0.5]);
            let s = w.p.s.get(at);
            assert_eq!(w.p.s.v().get_int(s, prop), n.min(4), "{block}: stacked ({})", w.name(at));
        }
    }
    w.set([3, Y, 6], "minecraft:sea_pickle");
    w.p.s.sneaking = true;
    w.hold(stack("minecraft:sea_pickle", 1));
    w.click([3, Y, 6], Dir::Up, [0.5, 0.1, 0.5]);
    w.p.s.sneaking = false;
    assert!(w.name([3, Y, 6]).contains("pickles=1"), "sneaking: no stacking");

    // Propagule: placed as an adult; hanging, holds on to the mangrove leaves above.
    place_on_floor(&mut w, 5, 1, "minecraft:mangrove_propagule");
    assert!(w.name([5, Y, 1]).contains("age=4"), "propagule placed as an adult: {}", w.name([5, Y, 1]));
    w.set([5, Y + 3, 3], "minecraft:mangrove_leaves[persistent=true]");
    w.set([5, Y + 2, 3], "minecraft:mangrove_propagule[hanging=true]");
    w.set([6, Y + 2, 3], "minecraft:stone"); // lateral shape update
    assert!(w.name([5, Y + 2, 3]).contains("hanging=true"), "hanging: holds on to the leaves");
    w.set([5, Y + 3, 3], "minecraft:air");
    assert_eq!(w.name([5, Y + 2, 3]), "minecraft:air", "hanging without leaves: falls");

    // Small dripleaf: two halves on clay, turned towards the player.
    w.p.s.yaw = 0.0;
    w.set([5, Y - 1, 5], "minecraft:clay");
    place_on_floor(&mut w, 5, 5, "minecraft:small_dripleaf");
    assert!(w.name([5, Y, 5]).contains("facing=north,half=lower"), "{}", w.name([5, Y, 5]));
    assert!(w.name([5, Y + 1, 5]).contains("half=upper"), "upper half: {}", w.name([5, Y + 1, 5]));
    place_on_floor(&mut w, 6, 5, "minecraft:small_dripleaf");
    assert_eq!(w.name([6, Y, 5]), "minecraft:air", "dry grass: refused");
    w.set([5, Y - 1, 5], "minecraft:stone");
    assert_eq!(w.name([5, Y, 5]), "minecraft:air", "support removed: broken");
    assert_eq!(w.name([5, Y + 1, 5]), "minecraft:air");

    // Pitcher crop: one half, on farmland only.
    w.set([7, Y - 1, 1], "minecraft:farmland");
    place_on_floor(&mut w, 7, 1, "minecraft:pitcher_pod");
    assert!(w.name([7, Y, 1]).starts_with("minecraft:pitcher_crop[age=0,half=lower"), "{}", w.name([7, Y, 1]));
    assert_eq!(w.name([7, Y + 1, 1]), "minecraft:air", "no upper half at placement");
    w.set([7, Y - 1, 1], "minecraft:stone");
    assert_eq!(w.name([7, Y, 1]), "minecraft:air", "without farmland: broken");
}

/// Hanging signs: ceiling (axis or free rotation, attached under an incomplete face),
/// wall (on one side of its axis), chaining under another without opening the editor.
#[test]
fn hanging_signs_place_like_the_game() {
    let mut w = world("g6f06-sign");
    w.p.s.yaw = 30.0;
    w.p.s.pitch = 0.0;
    w.set([3, Y + 2, 3], "minecraft:stone");
    w.hold(stack("minecraft:oak_hanging_sign", 1));
    w.click([3, Y + 2, 3], Dir::Down, [0.5, 0.0, 0.5]);
    let s = w.name([3, Y + 1, 3]);
    assert!(s.starts_with("minecraft:oak_hanging_sign[attached=false,rotation=8"), "look axis: {s}");
    // Under a fence (incomplete bottom face): attached in the middle, free rotation.
    w.set([5, Y + 2, 3], "minecraft:oak_fence");
    w.hold(stack("minecraft:oak_hanging_sign", 1));
    w.click([5, Y + 2, 3], Dir::Down, [0.5, 0.0, 0.5]);
    let s = w.name([5, Y + 1, 3]);
    assert!(s.starts_with("minecraft:oak_hanging_sign[attached=true,rotation=9"), "attached: {s}");
    // Chaining: click on the underside of the sign with a sign → placed below.
    let _ = w.events();
    w.hold(stack("minecraft:oak_hanging_sign", 1));
    w.click([3, Y + 1, 3], Dir::Down, [0.5, 0.0, 0.5]);
    assert!(w.name([3, Y, 3]).starts_with("minecraft:oak_hanging_sign"), "chained: {}", w.name([3, Y, 3]));
    // Wall: click on the west side of a stone → north-south wall sign.
    w.set([6, Y, 6], "minecraft:stone");
    w.hold(stack("minecraft:oak_hanging_sign", 1));
    w.click([6, Y, 6], Dir::West, [0.0, 0.5, 0.5]);
    let s = w.name([5, Y, 6]);
    assert!(s.starts_with("minecraft:oak_wall_hanging_sign[facing="), "wall sign placed: {s}");
    assert!(s.contains("facing=north") || s.contains("facing=south"), "{s}");
    // True to the game: the wall sign does not fall when its support goes.
    w.set([6, Y, 6], "minecraft:air");
    assert!(w.name([5, Y, 6]).starts_with("minecraft:oak_wall_hanging_sign"), "never detached");
}

/// Coral fan on the wall; scaffolding extended; powder snow bucket; End rod
/// head to tail; shelf fungus; leaves at a distance; campfire; chorus.
#[test]
fn wall_and_extension_placements() {
    let mut w = world("g6f06-wall");
    w.p.s.yaw = 30.0;
    w.p.s.pitch = 0.0;
    w.set([6, Y, 2], "minecraft:stone");
    w.hold(stack("minecraft:tube_coral_fan", 1));
    w.click([6, Y, 2], Dir::West, [0.0, 0.5, 0.5]);
    assert!(w.name([5, Y, 2]).starts_with("minecraft:tube_coral_wall_fan[facing=west"), "{}", w.name([5, Y, 2]));

    // Scaffolding: click from above → placed horizontally along the look direction; click from the side → above.
    w.p.s.yaw = 0.0;
    w.set([2, Y, 10], "minecraft:scaffolding[distance=0]");
    w.hold(stack("minecraft:scaffolding", 1));
    w.click([2, Y, 10], Dir::Up, [0.5, 1.0, 0.5]);
    assert!(w.name([2, Y, 11]).starts_with("minecraft:scaffolding[bottom=false,distance=0"), "{}", w.name([2, Y, 11]));
    w.hold(stack("minecraft:scaffolding", 1));
    w.click([2, Y, 10], Dir::North, [0.5, 0.5, 0.0]);
    assert!(w.name([2, Y + 1, 10]).starts_with("minecraft:scaffolding"), "above: {}", w.name([2, Y + 1, 10]));

    // Powder snow bucket: the block, then an empty bucket.
    place_on_floor(&mut w, 1, 6, "minecraft:powder_snow_bucket");
    assert_eq!(w.name([1, Y, 6]), "minecraft:powder_snow");
    assert_eq!(w.held().map(|s| s.id), Some("minecraft:bucket".to_owned()));

    // End rod against a rod turned towards the face: head to tail.
    w.set([1, Y, 12], "minecraft:end_rod[facing=up]");
    w.hold(stack("minecraft:end_rod", 1));
    w.click([1, Y, 12], Dir::Up, [0.5, 1.0, 0.5]);
    assert!(w.name([1, Y + 1, 12]).contains("facing=down"), "{}", w.name([1, Y + 1, 12]));

    // Shelf fungus: on the wall, falls without its support.
    w.set([6, Y, 12], "minecraft:oak_log");
    w.hold(stack("minecraft:shelf_mushroom", 1));
    w.click([6, Y, 12], Dir::West, [0.0, 0.5, 0.5]);
    assert!(w.name([5, Y, 12]).contains("facing=west"), "{}", w.name([5, Y, 12]));
    w.set([6, Y, 12], "minecraft:air");
    assert_eq!(w.name([5, Y, 12]), "minecraft:air", "without support: falls");

    // Leaves placed against a trunk: distance 1.
    w.set([11, Y, 2], "minecraft:oak_log");
    place_on_floor(&mut w, 11, 3, "minecraft:oak_leaves");
    assert!(w.name([11, Y, 3]).contains("distance=1"), "{}", w.name([11, Y, 3]));

    // Campfire: turned like the gaze (south), signal on a hay bale.
    w.set([12, Y - 1, 6], "minecraft:hay_block");
    place_on_floor(&mut w, 12, 6, "minecraft:campfire");
    let s = w.name([12, Y, 6]);
    assert!(s.contains("facing=south") && s.contains("signal_fire=true"), "{s}");

    // Chorus plant on End stone: connected below.
    w.set([12, Y - 1, 10], "minecraft:end_stone");
    place_on_floor(&mut w, 12, 10, "minecraft:chorus_plant");
    assert!(w.name([12, Y, 10]).contains("down=true"), "{}", w.name([12, Y, 10]));
}

/// Stack components: `block_state` applied; `block_entity_data` of another type ignored.
/// Consumable fallback; adventure and `can_place_on`; "used" statistic of the flint and steel.
#[test]
fn stack_rules_at_placement() {
    let mut w = world("g6f06-stack");
    let props = Nbt::Compound(vec![("honey_level".into(), Nbt::String("3".into()))]);
    w.hold(with_components("minecraft:beehive", vec![("minecraft:block_state", props)]));
    w.click([2, Y - 1, 2], Dir::Up, UP);
    assert!(w.name([2, Y, 2]).contains("honey_level=3"), "{}", w.name([2, Y, 2]));

    // Block entity data of another type: ignored.
    let data = Nbt::Compound(vec![
        ("id".into(), Nbt::String("minecraft:barrel".into())),
        ("CustomName".into(), Nbt::String("x".into())),
    ]);
    w.hold(with_components("minecraft:chest", vec![("minecraft:block_entity_data", data)]));
    w.click([4, Y - 1, 2], Dir::Up, UP);
    let be = w.p.s.block_entity([4, Y, 2]).cloned().expect("chest placed");
    assert!(be.get("CustomName").is_none(), "data of another type applied: {be:?}");

    // Carrot clicked on grass: placement impossible, it gets eaten.
    w.p.s.life.vit.food = 10;
    place_on_floor(&mut w, 6, 2, "minecraft:carrot");
    assert_eq!(w.name([6, Y, 2]), "minecraft:air");
    assert!(w.p.s.life.vit.using.is_some(), "consumable fallback: use started");
    w.p.s.stop_using();

    // Adventure: placement only on a block accepted by `can_place_on`.
    w.p.s.game_mode = 2;
    place_on_floor(&mut w, 2, 5, "minecraft:stone");
    assert_eq!(w.name([2, Y, 5]), "minecraft:air", "adventure without component: refused");
    let pred = Nbt::Compound(vec![("blocks".into(), Nbt::String("minecraft:grass_block".into()))]);
    w.hold(with_components("minecraft:stone", vec![("minecraft:can_place_on", Nbt::List(vec![pred]))]));
    w.click([2, Y - 1, 5], Dir::Up, UP);
    assert_eq!(w.name([2, Y, 5]), "minecraft:stone", "adventure: can_place_on accepts grass");
    w.p.s.game_mode = 0;

    // Flint and steel: "used" statistic.
    w.hold(stack("minecraft:flint_and_steel", 1));
    w.click([6, Y - 1, 6], Dir::Up, UP);
    assert!(w.name([6, Y, 6]).starts_with("minecraft:fire["), "{}", w.name([6, Y, 6]));
    assert_eq!(w.p.s.prog.stats.get("minecraft:used", "minecraft:flint_and_steel"), 1);
    // Fire put out by hand: extinguishing sound.
    let _ = w.events();
    w.p.s.player_destroy([6, Y, 6]);
    assert_eq!(level_events(&w.events(), 1009), 1, "1009 on extinguishing");

    // Jigsaw: refused unless creative operator; oriented otherwise.
    w.p.s.yaw = 0.0;
    place_on_floor(&mut w, 10, 2, "minecraft:jigsaw");
    assert_eq!(w.name([10, Y, 2]), "minecraft:air", "survival: refused");
    w.p.s.game_mode = 1;
    w.p.s.cmd.op = 4;
    place_on_floor(&mut w, 10, 2, "minecraft:jigsaw");
    assert!(w.name([10, Y, 2]).contains("orientation=up_north"), "{}", w.name([10, Y, 2]));

    // Bee nest with honey broken in creative: it drops.
    w.set([10, Y, 5], "minecraft:bee_nest[honey_level=5]");
    w.p.s.player_destroy([10, Y, 5]);
    let n = w.p.s.life.drops.iter().filter(|d| !d.is_removed() && d.stack.id == "minecraft:bee_nest").count();
    assert_eq!(n, 1, "bee nest kept in creative");
}

/// Light sensor: inversion recomputes the signal at once (night: inverted signal > 0).
#[test]
fn daylight_detector_inversion_updates_now() {
    let mut w = world("g6f06-daylight");
    w.cmd("time set midnight");
    w.set([3, Y, 3], "minecraft:daylight_detector[power=0]");
    w.hold(None);
    w.click([3, Y, 3], Dir::Up, [0.5, 0.375, 0.5]);
    let s = w.p.s.get([3, Y, 3]);
    assert!(w.p.s.v().get_bool(s, "inverted"));
    assert!(w.p.s.v().get_int(s, "power") > 0, "signal recomputed on click: {}", w.name([3, Y, 3]));
}

/// Own shapes: huge mushroom blocks, pale hanging moss, pale moss carpet.
#[test]
fn moss_and_mushroom_shapes() {
    let mut w = world("g6f06-moss");
    // Neighbour already off on that side (no shape cascade): the face comes from the placement.
    w.set([2, Y, 2], "minecraft:red_mushroom_block[east=false]");
    place_on_floor(&mut w, 3, 2, "minecraft:red_mushroom_block");
    assert!(w.name([3, Y, 2]).contains("west=false"), "face against the same block: {}", w.name([3, Y, 2]));
    assert!(w.name([3, Y, 2]).contains("north=true"));
    // Neighbour on: turned off by the shape update.
    w.set([2, Y, 4], "minecraft:red_mushroom_block");
    place_on_floor(&mut w, 3, 4, "minecraft:red_mushroom_block");
    assert!(w.name([2, Y, 4]).contains("east=false"), "neighbour off: {}", w.name([2, Y, 4]));

    // Hanging moss: tip recomputed, broken at the tick without support.
    w.set([5, Y + 3, 5], "minecraft:stone");
    w.set([5, Y + 2, 5], "minecraft:pale_hanging_moss");
    w.set([5, Y + 1, 5], "minecraft:pale_hanging_moss");
    assert!(w.name([5, Y + 2, 5]).contains("tip=false"), "{}", w.name([5, Y + 2, 5]));
    w.set([5, Y + 3, 5], "minecraft:air");
    let mut evs = Vec::new();
    w.ticks(2, &mut evs);
    assert_eq!(w.name([5, Y + 2, 5]), "minecraft:air", "without support: broken");

    // Carpet: side against a full wall (bottom, or top under the randomly drawn layer); no
    // side in open air; removed when the ground disappears.
    w.set([8, Y, 2], "minecraft:stone");
    place_on_floor(&mut w, 7, 2, "minecraft:pale_moss_carpet");
    let s = w.name([7, Y, 2]);
    assert!(s.starts_with("minecraft:pale_moss_carpet[bottom=true,east="), "{s}");
    assert!(!s.contains("east=none") && s.contains("west=none"), "{s}");
    w.set([7, Y - 1, 2], "minecraft:air");
    assert_eq!(w.name([7, Y, 2]), "minecraft:air", "without ground: removed");
}

/// Block entity data of a reserved type (sign): only for an operator in creative.
#[test]
fn op_only_block_entity_data() {
    let sign = || Nbt::Compound(vec![("id".into(), Nbt::String("minecraft:sign".into()))]);
    let data = Nbt::Compound(vec![("id".into(), Nbt::String("minecraft:sign".into())), ("is_waxed".into(), Nbt::Byte(1))]);
    let comps = Nbt::Compound(vec![("minecraft:block_entity_data".into(), data)]);
    let mut be = sign();
    crate::loot::be::apply(&mut be, &comps, false);
    assert!(be.get("is_waxed").is_none(), "non-operator: refused");
    let mut be = sign();
    crate::loot::be::apply(&mut be, &comps, true);
    assert_eq!(be.get("is_waxed").and_then(Nbt::as_i64), Some(1), "operator: applied");
}
