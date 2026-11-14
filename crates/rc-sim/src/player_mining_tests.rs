//! Mining by the current player, read on the held stack like the game: in adventure, a tool
//! whose `can_break` accepts the block breaks it; the stack's own `tool` component (added,
//! changed or removed) gives the speed, the right tool for the loot, the wear and the right to
//! break in creative.

use std::path::PathBuf;
use std::sync::Arc;

use rc_proto::nbt::Nbt;

use crate::blocks::probe::Probe;
use crate::player::Stack;

/// The block mined, in the chunk the world opens with, 2.5 blocks from the player's eyes.
const STONE: [i32; 3] = [4, -60, 4];

fn open(tag: &str) -> (Probe, PathBuf) {
    let data = Arc::new(rc_testdata::version_data().expect("version data 26.3 missing"));
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-mining-{tag}-{}", std::process::id())));
    let mut w = crate::newworld::NewWorld::new("mining");
    w.preset = crate::newworld::Preset::Flat;
    w.seed = 0;
    w.generate_structures = false;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    let mut p = Probe::open(&dir, data);
    p.run(&format!("setblock {} {} {} minecraft:stone\n", STONE[0], STONE[1], STONE[2]));
    p.s.pos = [4.5, -60.0, 6.5];
    p.s.loaded = true;
    p.s.on_ground = true;
    p.s.game_mode = 0;
    for slot in p.s.slots.iter_mut() {
        *slot = None;
    }
    (p, dir)
}

fn stack(id: &str) -> Stack {
    Stack { id: id.into(), count: 1, components: None }
}

fn with(mut s: Stack, component: &str, value: Nbt) -> Stack {
    crate::items::set_component(&mut s, component, value);
    s
}

fn compound(fields: Vec<(&str, Nbt)>) -> Nbt {
    Nbt::Compound(fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

/// `can_break` accepting the blocks `blocks` (an id or a `#tag`).
fn can_break(blocks: &str) -> Nbt {
    compound(vec![("predicates", Nbt::List(vec![compound(vec![("blocks", Nbt::String(blocks.into()))])]))])
}

/// A `tool` component with one rule for `block` (speed, right for the loot), the rest given.
fn tool(rules: Vec<(&str, f32)>, damage_per_block: i32, creative: bool) -> Nbt {
    let rules = rules
        .into_iter()
        .map(|(block, speed)| {
            compound(vec![
                ("blocks", Nbt::String(block.into())),
                ("speed", Nbt::Float(speed)),
                ("correct_for_drops", Nbt::Byte(1)),
            ])
        })
        .collect();
    compound(vec![
        ("rules", Nbt::List(rules)),
        ("damage_per_block", Nbt::Int(damage_per_block)),
        ("can_destroy_blocks_in_creative", Nbt::Byte(i8::from(creative))),
    ])
}

fn hold(p: &mut Probe, s: Option<Stack>) {
    let slot = 36 + p.s.held as usize;
    p.s.slots[slot] = s;
}

fn stone_left(p: &Probe) -> bool {
    p.s.state_name(p.s.get(STONE)) == "minecraft:stone"
}

/// Adventure: a pickaxe whose `can_break` names stone starts mining it and breaks it; an empty
/// hand or a `can_break` for another block is refused, and a spectator never mines.
#[test]
fn adventure_mines_what_can_break_accepts() {
    let (mut p, _dir) = open("adventure");
    p.s.game_mode = 2;
    for (label, held) in [
        ("empty hand", None),
        ("can_break for dirt", Some(with(stack("minecraft:diamond_pickaxe"), "minecraft:can_break", can_break("minecraft:dirt")))),
    ] {
        hold(&mut p, held);
        p.s.on_action(0, STONE, 1, 0);
        assert!(!p.s.mining.is_some_and(|m| m.active), "{label}: mining started");
        assert!(!p.s.player_destroy_block(STONE), "{label}: stone broken");
    }
    hold(&mut p, Some(with(stack("minecraft:diamond_pickaxe"), "minecraft:can_break", can_break("minecraft:stone"))));
    p.s.on_action(0, STONE, 1, 1);
    assert!(p.s.mining.is_some_and(|m| m.active), "can_break for stone: mining refused");
    p.s.game_mode = 3;
    assert!(!p.s.player_destroy_block(STONE), "spectator broke the stone");
    p.s.game_mode = 2;
    assert!(p.s.player_destroy_block(STONE), "can_break for stone: break refused");
    assert!(!stone_left(&p), "stone still there");
}

/// The stack's own tool decides the mining speed and the right tool: a stick given a tool
/// mines stone fast and rightly; a pickaxe whose tool is taken away mines it by hand.
#[test]
fn mining_speed_and_right_tool_come_from_the_stack() {
    let (mut p, _dir) = open("speed");
    let stone = p.s.get(STONE);
    let by_hand = 1.0 / 1.5 / 100.0;
    hold(&mut p, Some(stack("minecraft:stick")));
    assert!((p.s.mine_gain(stone) - by_hand).abs() < 1e-6, "plain stick");
    let fast_stick = with(stack("minecraft:stick"), "minecraft:tool", tool(vec![("minecraft:stone", 20.0)], 1, true));
    hold(&mut p, Some(fast_stick.clone()));
    assert!((p.s.mine_gain(stone) - 20.0 / 1.5 / 30.0).abs() < 1e-6, "stick with a tool: {}", p.s.mine_gain(stone));
    assert!(p.s.tool_correct(Some(&fast_stick), stone), "stick with a tool: right for the loot");
    let mut bare = stack("minecraft:diamond_pickaxe");
    bare.components = Some(compound(vec![("!minecraft:tool", compound(vec![]))]));
    hold(&mut p, Some(bare.clone()));
    assert!((p.s.mine_gain(stone) - by_hand).abs() < 1e-6, "pickaxe without its tool");
    assert!(!p.s.tool_correct(Some(&bare), stone), "pickaxe without its tool: right for the loot");
}

/// Wear per block and creative breaking follow the stack's tool too: a sword patched to wear 5
/// per block takes 5 (its own tool says 2); patched to break in creative, it breaks there.
#[test]
fn wear_and_creative_breaking_come_from_the_stack() {
    let (mut p, _dir) = open("wear");
    let sword = with(stack("minecraft:diamond_sword"), "minecraft:tool", tool(vec![], 5, true));
    hold(&mut p, Some(sword.clone()));
    assert!(p.s.player_destroy_block(STONE), "survival break refused");
    let held = p.s.slots[36 + p.s.held as usize].clone().expect("sword gone");
    let damage = crate::items::component(&held, "minecraft:damage").and_then(Nbt::as_i64);
    assert_eq!(damage, Some(5), "wear of the patched sword");
    p.run(&format!("setblock {} {} {} minecraft:stone\n", STONE[0], STONE[1], STONE[2]));
    p.s.game_mode = 1;
    hold(&mut p, Some(stack("minecraft:diamond_sword")));
    assert!(!p.s.player_destroy_block(STONE), "a plain sword broke in creative");
    hold(&mut p, Some(sword));
    assert!(p.s.player_destroy_block(STONE), "the patched sword did not break in creative");
}
