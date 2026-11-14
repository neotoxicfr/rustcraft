//! What a use leaves behind and what a hit takes, like the game: after eating, after a block
//! placed, after a throw, the stack's own `use_remainder` and `use_cooldown`, read on the stack
//! as it was before the use; a negative damage amount does nothing, and the absorption reserve
//! never goes beyond its maximum.

use std::path::PathBuf;
use std::sync::Arc;

use rc_proto::nbt::Nbt;

use crate::blocks::Dir;
use crate::blocks::probe::Probe;
use crate::player::Stack;

fn open(tag: &str) -> (Probe, PathBuf) {
    let data = Arc::new(rc_testdata::version_data().expect("version data 26.3 missing"));
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-use-damage-{tag}-{}", std::process::id())));
    let mut w = crate::newworld::NewWorld::new("use-damage");
    w.preset = crate::newworld::Preset::Flat;
    w.seed = 0;
    w.generate_structures = false;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    let mut p = Probe::open(&dir, data);
    p.s.pos = [4.5, -60.0, 6.5];
    p.s.loaded = true;
    p.s.on_ground = true;
    p.s.game_mode = 0;
    p.s.life.vit.health = 20.0;
    p.s.life.vit.food = 10;
    p.s.life.vit.dead = false;
    p.s.life.vit.cooldown = 0;
    for slot in p.s.slots.iter_mut() {
        *slot = None;
    }
    (p, dir)
}

fn stack(id: &str, count: i32) -> Stack {
    Stack { id: id.into(), count, components: None }
}

fn compound(fields: Vec<(&str, Nbt)>) -> Nbt {
    Nbt::Compound(fields.into_iter().map(|(k, v)| (k.to_owned(), v)).collect())
}

fn main_hand(p: &Probe) -> usize {
    36 + p.s.held as usize
}

/// The stack in hand eaten to the end.
fn eat(p: &mut Probe, s: Stack) {
    let slot = main_hand(p);
    p.s.slots[slot] = Some(s);
    p.s.on_use_item(0);
    assert!(p.s.life.vit.using.is_some(), "use not started");
    p.s.finish_using();
}

/// Ticks left of the cooldown of `group`, if any.
fn cooldown_of(p: &Probe, group: &str) -> Option<i32> {
    let now = p.s.proj_player.ticks;
    p.s.proj_player.cooldowns.iter().find(|c| c.0 == group && c.2 > now).map(|c| c.2 - now)
}

/// Chorus fruit: its item's 1 s cooldown starts when it is eaten.
#[test]
fn eating_starts_the_cooldown_of_the_stack() {
    let (mut p, _dir) = open("chorus");
    eat(&mut p, stack("minecraft:chorus_fruit", 2));
    assert_eq!(cooldown_of(&p, "minecraft:chorus_fruit"), Some(20), "chorus fruit cooldown");
}

/// A patched `use_remainder` replaces the item's (a stew giving two diamonds, a named bowl);
/// in creative nothing is used up and no remainder appears.
#[test]
fn eating_leaves_the_remainder_of_the_stack() {
    let (mut p, _dir) = open("remainder");
    let mut stew = stack("minecraft:mushroom_stew", 1);
    crate::items::set_component(&mut stew, "minecraft:use_remainder", compound(vec![
        ("id", Nbt::String("minecraft:diamond".into())),
        ("count", Nbt::Int(2)),
    ]));
    eat(&mut p, stew);
    let left = p.s.slots[main_hand(&p)].clone().expect("no remainder");
    assert_eq!((left.id.as_str(), left.count), ("minecraft:diamond", 2), "patched remainder");
    let name = compound(vec![("minecraft:custom_name", Nbt::String("Bowl of the test".into()))]);
    let mut named = stack("minecraft:mushroom_stew", 1);
    crate::items::set_component(&mut named, "minecraft:use_remainder", compound(vec![
        ("id", Nbt::String("minecraft:bowl".into())),
        ("components", name),
    ]));
    eat(&mut p, named);
    let bowl = p.s.slots[main_hand(&p)].clone().expect("no bowl");
    assert_eq!(bowl.id, "minecraft:bowl");
    assert!(crate::items::component(&bowl, "minecraft:custom_name").is_some(), "the remainder lost its components");
    p.s.game_mode = 1;
    eat(&mut p, stack("minecraft:mushroom_stew", 1));
    let kept = p.s.slots[main_hand(&p)].clone().expect("stew gone in creative");
    assert_eq!(kept.id, "minecraft:mushroom_stew", "creative: remainder given");
}

/// A block placed from a stack carrying `use_cooldown` starts its cooldown (2 s here).
#[test]
fn placing_a_block_starts_the_cooldown_of_the_stack() {
    let (mut p, _dir) = open("place");
    let mut stone = stack("minecraft:stone", 4);
    crate::items::set_component(&mut stone, "minecraft:use_cooldown", compound(vec![("seconds", Nbt::Float(2.0))]));
    let slot = main_hand(&p);
    p.s.slots[slot] = Some(stone);
    p.run("setblock 4 -61 4 minecraft:dirt\n"); // loads the chunk clicked
    p.s.use_hand_on_block(0, [4, -61, 4], Dir::Up, [0.5, 1.0, 0.5]);
    assert_eq!(p.s.state_name(p.s.get([4, -60, 4])), "minecraft:stone", "block not placed");
    assert_eq!(cooldown_of(&p, "minecraft:stone"), Some(40), "cooldown after placing");
}

/// Instant damage of level 31 (6 << 30 wraps to a negative amount) does nothing: no health
/// lost, the absorption reserve neither created nor grown.
#[test]
fn negative_damage_does_nothing() {
    let (mut p, _dir) = open("negative");
    p.s.apply_instant("minecraft:instant_damage", 30, 1.0);
    assert_eq!(p.s.life.vit.health, 20.0, "health");
    assert_eq!(p.s.life.fx.absorption, 0.0, "absorption created by a negative hit");
    assert!(p.s.give_player_effect("minecraft:absorption", 600, 0), "absorption refused");
    assert_eq!(p.s.life.fx.absorption, 4.0, "absorption reserve");
    p.s.life.vit.cooldown = 0;
    p.s.apply_instant("minecraft:instant_damage", 30, 1.0);
    assert_eq!(p.s.life.fx.absorption, 4.0, "absorption grown by a negative hit");
    assert_eq!(p.s.life.vit.health, 20.0, "health");
}
