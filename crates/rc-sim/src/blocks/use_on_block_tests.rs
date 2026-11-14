//! Using an item on a block and placing blocks: the off hand's item (block, torch, bone meal,
//! flint and steel), the entities that keep a block from being placed, concrete powder placed in
//! or against water, candles stacked and their support, and the block entity data sent after
//! its block. Fresh flat world from `g6f01_tests`: the 26.3 cache and the client jar are required
//! (the test fails without them).

use rc_client::Event;
use rc_proto::nbt::Nbt;

use super::block_entity_updates::UpdateTag;
use super::g6f01_tests::{W, Y, stack, world};
use super::*;
use crate::player::OFF_HAND_SLOT;

/// Face index of the top face in the `use_item_on` packet.
const TOP: i32 = 1;

/// The player ready to use items (loaded, no pending teleport), survival, standing at the centre
/// of chunk (0, 0), which its client holds (the broadcast of changed blocks reaches it).
fn ready(tag: &str) -> W {
    let mut w = world(tag);
    w.p.s.loaded = true;
    w.p.s.teleport = None;
    w.p.s.sent.insert((0, 0));
    w
}

fn block(w: &W, at: P) -> String {
    w.name(at).split('[').next().unwrap_or("").to_owned()
}

/// A click of hand `hand` on the top face of `floor`, as the client sends it.
fn click_top(w: &mut W, hand: i32, floor: P) {
    w.p.s.on_use_item_on(hand, floor, TOP, [0.5, 1.0, 0.5], false, 1);
}

/// Block item `item` placed by the main hand on the top of `floor`; true if placed.
fn place_on_top(w: &mut W, item: &str, floor: P) -> bool {
    let at = [f64::from(floor[0]) + 0.5, f64::from(floor[1] + 1), f64::from(floor[2]) + 0.5];
    w.p.s.place_item(item, floor, Dir::Up, at)
}

// --- Off hand. ---

/// Main hand empty, a torch in the off hand: the client's click of the off hand places it, one
/// torch fewer. Before, the server ignored every off hand use on a block (the torch vanished).
#[test]
fn a_torch_in_the_off_hand_is_placed() {
    let mut w = ready("offhandtorch");
    let floor = [8, Y - 1, 10];
    w.p.s.slots[OFF_HAND_SLOT] = stack("minecraft:torch", 3);
    click_top(&mut w, 1, floor);
    assert_eq!(block(&w, above(floor)), "minecraft:torch", "torch placed by the off hand");
    assert_eq!(w.p.s.slots[OFF_HAND_SLOT].as_ref().map(|s| s.count), Some(2), "one torch used");
}

/// Off hand items used on a block: a stone block placed, bone meal on wheat, flint and steel
/// lighting the floor (its wear taken from the off hand).
#[test]
fn off_hand_items_act_on_blocks() {
    let mut w = ready("offhanditems");
    let floor = [10, Y - 1, 10];
    w.p.s.slots[OFF_HAND_SLOT] = stack("minecraft:stone", 2);
    click_top(&mut w, 1, floor);
    assert_eq!(block(&w, above(floor)), "minecraft:stone", "block placed by the off hand");
    let wheat = [6, Y, 10];
    w.set(below(wheat), "minecraft:farmland");
    w.set(wheat, "minecraft:wheat[age=0]");
    w.p.s.slots[OFF_HAND_SLOT] = stack("minecraft:bone_meal", 1);
    click_top(&mut w, 1, wheat);
    assert!(w.p.s.view().get_int(w.p.s.get(wheat), rc_data::PropKey::Age) >= 2, "wheat grown by the off hand's bone meal");
    assert!(w.p.s.slots[OFF_HAND_SLOT].is_none(), "the bone meal was used");
    let stone_floor = [6, Y - 1, 6];
    w.set(stone_floor, "minecraft:stone");
    w.p.s.slots[OFF_HAND_SLOT] = stack("minecraft:flint_and_steel", 1);
    click_top(&mut w, 1, stone_floor);
    assert_eq!(block(&w, above(stone_floor)), "minecraft:fire", "fire lit by the off hand");
}

/// The block's use without an item belongs to the main hand: the off hand's click leaves a lever
/// alone. Sneaking with a torch in the off hand, the main hand's click does too (either hand busy
/// stops the block's use); before, only the main hand counted and the lever was pulled.
#[test]
fn the_off_hand_never_uses_a_block_and_sneaking_with_it_busy_leaves_the_block_alone() {
    let mut w = ready("offhandlever");
    let lever = [8, Y, 6];
    w.set(lever, "minecraft:lever[face=floor,facing=north,powered=false]");
    click_top(&mut w, 1, lever);
    assert_eq!(w.p.s.view().get_named(w.p.s.get(lever), "powered"), Some("false"), "off hand: lever untouched");
    w.p.s.slots[OFF_HAND_SLOT] = stack("minecraft:torch", 1);
    w.p.s.sneaking = true;
    click_top(&mut w, 0, lever);
    assert_eq!(w.p.s.view().get_named(w.p.s.get(lever), "powered"), Some("false"), "sneaking, off hand busy: lever untouched");
    w.p.s.sneaking = false;
    click_top(&mut w, 0, lever);
    assert_eq!(w.p.s.view().get_named(w.p.s.get(lever), "powered"), Some("true"), "empty main hand: lever pulled");
}

/// A respawn anchor with one charge in the overworld, a stone in the main hand and glowstone in
/// the off hand: the main hand's click lets the stone be placed on it (no explosion), the off
/// hand's click charges it. Before, the main hand's click blew the anchor up.
#[test]
fn glowstone_waiting_in_the_off_hand_keeps_the_anchor_from_exploding() {
    let mut w = ready("offhandanchor");
    let anchor = [10, Y, 6];
    w.set(anchor, "minecraft:respawn_anchor[charges=1]");
    w.hold(stack("minecraft:stone", 1));
    w.p.s.slots[OFF_HAND_SLOT] = stack("minecraft:glowstone", 1);
    click_top(&mut w, 0, anchor);
    assert_eq!(block(&w, anchor), "minecraft:respawn_anchor", "the anchor did not explode");
    assert_eq!(block(&w, above(anchor)), "minecraft:stone", "the main hand's stone placed on it");
    click_top(&mut w, 1, anchor);
    assert_eq!(w.p.s.view().get_named(w.p.s.get(anchor), "charges"), Some("2"), "charged by the off hand");
}

// --- Entities in the way. ---

/// An entity that blocks building stands in the cell: a cow, an armor stand, a minecart, an End
/// crystal, a falling block, a boat. The stone is refused; next to a marker armor stand it goes.
/// Before, only the placing player was tested.
#[test]
fn a_block_is_not_placed_where_an_entity_stands() {
    let mut w = ready("entitiesintheway");
    let cases = [
        ([2, 4], "minecraft:cow", "{NoAI:1b}"),
        ([4, 4], "minecraft:armor_stand", ""),
        ([6, 4], "minecraft:minecart", "{NoGravity:1b}"),
        ([11, 4], "minecraft:end_crystal", "{ShowBottom:0b}"),
        ([14, 4], "minecraft:falling_block", "{BlockState:{Name:\"minecraft:sand\"},NoGravity:1b,Time:1}"),
        ([2, 12], "minecraft:oak_boat", "{NoGravity:1b}"),
    ];
    for ([x, z], kind, data) in cases {
        w.cmd(&format!("summon {kind} {x}.5 {Y} {z}.5 {data}"));
        assert!(!place_on_top(&mut w, "minecraft:stone", [x, Y - 1, z]), "{kind}: stone placed in it");
        assert_eq!(block(&w, [x, Y, z]), "minecraft:air", "{kind}: the cell stays free");
    }
    w.cmd(&format!("summon minecraft:armor_stand 6.5 {Y} 12.5 {{Marker:1b}}"));
    assert!(place_on_top(&mut w, "minecraft:stone", [6, Y - 1, 12]), "a marker armor stand blocks nothing");
}

/// The entity's own box is held against the placed block: a cow whose box reaches into the cell
/// from the side keeps the stone out, one fully outside it does not.
#[test]
fn the_placed_shape_decides_against_the_entity_box() {
    let mut w = ready("entityshape");
    // A cow half a block to the side of the cell: its box (0.9 wide) still reaches into it.
    w.cmd(&format!("summon minecraft:cow 3.2 {Y} 8.5 {{NoAI:1b}}"));
    assert!(!place_on_top(&mut w, "minecraft:stone", [2, Y - 1, 8]), "cow reaching into the cell");
    // A cow fully out of the cell: the stone goes.
    w.cmd(&format!("summon minecraft:cow 13.5 {Y} 2.5 {{NoAI:1b}}"));
    assert!(place_on_top(&mut w, "minecraft:stone", [12, Y - 1, 1]), "cow out of the cell");
}

/// The spawn obstruction test sees the same entities: a boat keeps a mob from spawning in its box.
/// Before, only mobs, players, TNT and falling blocks counted.
#[test]
fn a_boat_keeps_a_mob_from_spawning_in_its_box() {
    let mut w = ready("spawnboat");
    let zombie_box = rc_physics::Aabb { min: [4.2, f64::from(Y), 12.2], max: [4.8, f64::from(Y) + 1.95, 12.8] };
    assert!(w.p.s.unobstructed(&zombie_box, crate::building_blockers::NO_ENTITY), "nothing there yet");
    w.cmd(&format!("summon minecraft:oak_boat 4.5 {Y} 12.5 {{NoGravity:1b}}"));
    assert!(!w.p.s.unobstructed(&zombie_box, crate::building_blockers::NO_ENTITY), "the boat is in the way");
}

// --- Concrete powder. ---

/// Concrete powder placed in a water source, or against water, is concrete at once; out of
/// water it stays powder. Quirk of the game kept: water under the cell only counts when the cell
/// itself holds water. Before, the powder stayed powder in every case.
#[test]
fn concrete_powder_placed_in_or_against_water_hardens() {
    let mut w = ready("concretewater");
    w.set([4, Y, 4], "minecraft:water");
    assert!(place_on_top(&mut w, "minecraft:red_concrete_powder", [4, Y - 1, 4]), "placed into the water");
    assert_eq!(block(&w, [4, Y, 4]), "minecraft:red_concrete", "in water: concrete");
    w.set([6, Y, 2], "minecraft:water");
    assert!(place_on_top(&mut w, "minecraft:red_concrete_powder", [6, Y - 1, 3]), "placed beside the water");
    assert_eq!(block(&w, [6, Y, 3]), "minecraft:red_concrete", "against water: concrete");
    assert!(place_on_top(&mut w, "minecraft:red_concrete_powder", [12, Y - 1, 12]), "placed out of water");
    assert_eq!(block(&w, [12, Y, 12]), "minecraft:red_concrete_powder", "dry: powder");
    // Over water, clicked from the side of a block: the water below does not count.
    w.set([10, Y, 2], "minecraft:water");
    w.set([11, Y + 1, 2], "minecraft:stone");
    let side = [11.0, f64::from(Y + 1) + 0.5, 2.5];
    assert!(w.p.s.place_item("minecraft:red_concrete_powder", [11, Y + 1, 2], Dir::West, side), "placed over the water");
    assert_eq!(block(&w, [10, Y + 1, 2]), "minecraft:red_concrete_powder", "water below only: powder (quirk)");
}

// --- Candles. ---

/// A candle clicked with a candle (not sneaking) takes one more, up to four. Sneaking, it does
/// not. Before, a new candle went into the next cell instead.
#[test]
fn candles_stack_up_to_four() {
    let mut w = ready("candlestack");
    let floor = [6, Y - 1, 6];
    assert!(place_on_top(&mut w, "minecraft:candle", floor), "first candle");
    let candle = above(floor);
    for n in 2..=4 {
        let at = [f64::from(candle[0]) + 0.5, f64::from(candle[1]) + 0.3, f64::from(candle[2]) + 0.5];
        assert!(w.p.s.place_item("minecraft:candle", candle, Dir::Up, at), "candle {n}");
        assert_eq!(w.p.s.view().get_named(w.p.s.get(candle), "candles"), Some(n.to_string().as_str()), "{n} candles");
        assert_eq!(block(&w, above(candle)), "minecraft:air", "candle {n}: nothing went above");
    }
    let lone = [10, Y, 6];
    assert!(place_on_top(&mut w, "minecraft:candle", below(lone)), "second spot");
    w.p.s.sneaking = true;
    let at = [10.5, f64::from(Y) + 0.3, 6.5];
    let _ = w.p.s.place_item("minecraft:candle", lone, Dir::Up, at);
    assert_eq!(w.p.s.view().get_named(w.p.s.get(lone), "candles"), Some("1"), "sneaking: not stacked");
}

/// A candle needs the centre of the top face below at placement: refused on a bottom slab. A
/// candle whose support is then taken away stays (quirk of the game kept).
#[test]
fn a_candle_needs_a_centre_support_when_placed_only() {
    let mut w = ready("candlesupport");
    w.set([4, Y, 10], "minecraft:stone_slab[type=bottom]");
    assert!(!place_on_top(&mut w, "minecraft:candle", [4, Y, 10]), "refused on a bottom slab");
    assert_eq!(block(&w, [4, Y + 1, 10]), "minecraft:air");
    w.set([6, Y, 10], "minecraft:stone");
    assert!(place_on_top(&mut w, "minecraft:candle", [6, Y, 10]), "on stone");
    w.set([6, Y, 10], "minecraft:air");
    assert_eq!(block(&w, [6, Y + 1, 10]), "minecraft:candle", "left without support, it stays");
}

// --- Block entity data after its block. ---

/// Index of the first block state sent for `p` (single or section update), and of the first
/// block entity data sent for it.
fn first_block_and_data(evs: &[Event], p: P) -> (Option<usize>, Option<usize>) {
    let local = (((p[1] & 15) << 8) | ((p[2] & 15) << 4) | (p[0] & 15)) as u16;
    let section = [p[0] >> 4, p[1] >> 4, p[2] >> 4];
    let block = evs.iter().position(|e| match e {
        Event::BlockUpdate { pos, .. } => [pos.x, pos.y, pos.z] == p,
        Event::SectionUpdate { section: s, changes } => *s == section && changes.iter().any(|c| c.0 == local),
        _ => false,
    });
    let data = evs.iter().position(|e| matches!(e, Event::BlockEntityData { pos, .. } if *pos == p));
    (block, data)
}

/// The block entity data sent for `p`, the last one.
fn data_sent(evs: &[Event], p: P) -> Option<Nbt> {
    evs.iter().rev().find_map(|e| match e {
        Event::BlockEntityData { pos, nbt, .. } if *pos == p => nbt.clone(),
        _ => None,
    })
}

fn assert_block_then_data(evs: &[Event], p: P, what: &str) {
    let (block, data) = first_block_and_data(evs, p);
    let (Some(block), Some(data)) = (block, data) else { panic!("{what}: block {block:?}, data {data:?}") };
    assert!(block < data, "{what}: data sent before the block ({data} < {block})");
}

/// `setblock` of a sign with text: the block goes first, then its text. Before, the text left at
/// once, ahead of the block, and a client dropped it (blank sign).
#[test]
fn a_sign_set_with_text_sends_the_text_after_the_block() {
    let mut w = ready("signdata");
    let _ = w.events();
    let sign = [4, Y, 4];
    w.cmd(&format!(
        "setblock {} {} {} minecraft:oak_sign{{front_text:{{messages:[\"Hello\",\"\",\"\",\"\"]}}}}",
        sign[0], sign[1], sign[2]
    ));
    let evs = w.events();
    assert_block_then_data(&evs, sign, "setblock");
    assert!(data_sent(&evs, sign).is_some_and(|n| n.get("front_text").is_some()), "the text is sent");
    // Cloned: the copy's text follows the copy.
    let copy = [4, Y, 8];
    w.cmd(&format!("clone {0} {1} {2} {0} {1} {2} {3} {4} {5}", sign[0], sign[1], sign[2], copy[0], copy[1], copy[2]));
    assert_block_then_data(&w.events(), copy, "clone");
}

/// A banner with patterns placed by the player: the other clients get the block, then its
/// patterns. Before, the patterns left during the placement, before any block was sent.
#[test]
fn a_banner_placed_with_patterns_sends_them_after_the_block() {
    let mut w = ready("bannerdata");
    let patterns = Nbt::List(vec![Nbt::Compound(vec![
        ("pattern".into(), Nbt::String("minecraft:stripe_bottom".into())),
        ("color".into(), Nbt::String("red".into())),
    ])]);
    let mut banner = crate::player::Stack { id: "minecraft:white_banner".into(), count: 1, components: None };
    crate::items::set_component(&mut banner, "minecraft:banner_patterns", patterns);
    w.hold(Some(banner));
    let _ = w.events();
    let floor = [10, Y - 1, 10];
    click_top(&mut w, 0, floor);
    let at = above(floor);
    assert_eq!(block(&w, at), "minecraft:white_banner", "banner placed");
    let evs = w.events();
    assert_block_then_data(&evs, at, "player placement");
    let sent = data_sent(&evs, at).expect("banner data");
    assert!(!crate::nbtio::list(sent.get("patterns")).is_empty(), "the patterns are sent: {sent:?}");
}

/// The update tag of each kind, as the game writes it: a chest sends none, a spawner leaves its
/// spawn candidates out, a copper golem statue sends an empty compound, a head its profile.
/// Before, only campfires, shelves, pots and conduits had one.
#[test]
fn each_block_entity_kind_sends_the_game_s_update_tag() {
    let mut w = ready("updatetags");
    let tag_of = |w: &W, p: P| w.p.s.be_update_tag(p, w.p.s.get(p));
    w.set([2, Y, 2], "minecraft:chest");
    assert_eq!(tag_of(&w, [2, Y, 2]), None, "chest: nothing");
    w.cmd(&format!(
        "setblock 4 {Y} 2 minecraft:spawner{{SpawnPotentials:[{{weight:1,data:{{entity:{{id:\"minecraft:pig\"}}}}}}],Delay:5s}}"
    ));
    let spawner = tag_of(&w, [4, Y, 2]).expect("spawner tag");
    assert!(spawner.get("SpawnPotentials").is_none() && spawner.get("Delay").is_some(), "spawner: {spawner:?}");
    w.set([6, Y, 2], "minecraft:copper_golem_statue");
    assert_eq!(tag_of(&w, [6, Y, 2]), Some(Nbt::Compound(Vec::new())), "statue: empty");
    w.cmd(&format!("setblock 8 {Y} 2 minecraft:player_head{{profile:{{name:\"Steve\"}}}}"));
    assert!(tag_of(&w, [8, Y, 2]).is_some_and(|t| t.get("profile").is_some()), "head: its profile");
    let table = &w.p.s.bs.t;
    let of = |name: &str| table.update_tag[table.block_id(name) as usize];
    assert_eq!(of("minecraft:suspicious_sand"), UpdateTag::Brushable);
    assert_eq!(of("minecraft:trial_spawner"), UpdateTag::TrialSpawner);
    assert_eq!(of("minecraft:vault"), UpdateTag::Vault);
    assert_eq!(of("minecraft:oak_wall_hanging_sign"), UpdateTag::Fields);
    assert_eq!(of("minecraft:end_gateway"), UpdateTag::Fields);
    assert_eq!(of("minecraft:hopper"), UpdateTag::None);
    // One per block in the table: kept to a byte.
    assert_eq!(std::mem::size_of::<UpdateTag>(), 1);
}
