//! Pickups by the current player, like the game: the area is the player's own box (pose and
//! size) inflated by 1 / 0.5 / 1, joined to its vehicle's box when riding; a stack that does
//! not fit whole is only shrunk on the ground, with nothing shown or counted.

use std::path::PathBuf;
use std::sync::Arc;

use rc_client::{EntityEvent, Event};

use crate::blocks::probe::Probe;
use crate::mobs::species::Kind;
use crate::player::Stack;

/// First layer of air of the flat world (grass at y = −61).
const GROUND: f64 = -60.0;

fn open(tag: &str) -> (Probe, PathBuf) {
    let data = Arc::new(rc_testdata::version_data().expect("version data 26.3 missing"));
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-pickup-{tag}-{}", std::process::id())));
    let mut w = crate::newworld::NewWorld::new("pickup");
    w.preset = crate::newworld::Preset::Flat;
    w.seed = 0;
    w.generate_structures = false;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    let mut p = Probe::open(&dir, data);
    p.s.pos = [8.5, GROUND, 8.5];
    p.s.loaded = true;
    p.s.game_mode = 0;
    p.s.on_ground = true;
    p.s.life.vit.health = 20.0;
    p.s.life.vit.dead = false;
    for slot in p.s.slots.iter_mut() {
        *slot = None;
    }
    (p, dir)
}

fn stack(id: &str, count: i32) -> Stack {
    Stack { id: id.into(), count, components: None }
}

/// A stone stack lying at `pos`, ready to be picked up; its entity id.
fn lay(p: &mut Probe, pos: [f64; 3], count: i32) -> i32 {
    p.s.spawn_item(pos, [0.0; 3], stack("minecraft:stone", count), 0);
    p.s.life.drops.last().expect("item spawned").id
}

/// Stones the player holds, all slots together.
fn stones_held(p: &Probe) -> i32 {
    p.s.slots.iter().flatten().filter(|s| s.id == "minecraft:stone").map(|s| s.count).sum()
}

/// On a horse, the player reaches what lies on the ground beside its mount: the area is the
/// union of both boxes (the former area, hung from the player's feet, stopped 0.6 above it).
#[test]
fn rider_picks_up_items_beside_the_mount() {
    let (mut p, _dir) = open("rider");
    let horse = p.s.new_mob(Kind::Horse, [8.5, GROUND, 8.5], 0.0);
    let horse_id = horse.id;
    p.s.add_mob(horse);
    p.s.mobs.mounts.riding.vehicle = Some(horse_id);
    p.s.pos = [8.5, GROUND + 1.2, 8.5];
    lay(&mut p, [10.0, GROUND, 8.5], 3);
    p.s.pick_up_touched_drops();
    assert_eq!(stones_held(&p), 3, "the stones beside the horse are not picked up");
}

/// Swimming, the player's box is 0.6 high: an item 1.5 above its feet is out of reach (the
/// former area always took a standing box, 1.8 high).
#[test]
fn swimming_player_reaches_only_its_own_box() {
    let (mut p, _dir) = open("swimming");
    p.s.mobs.sea.player_swimming = true;
    assert_eq!(p.s.player_dims().1, 0.6, "swimming pose expected");
    lay(&mut p, [8.5, GROUND + 1.5, 8.5], 2);
    p.s.pick_up_touched_drops();
    assert_eq!(stones_held(&p), 0, "picked up above the swimming box");
    lay(&mut p, [8.5, GROUND + 0.8, 8.5], 2);
    p.s.pick_up_touched_drops();
    assert_eq!(stones_held(&p), 2, "the item inside the swimming area is not picked up");
}

/// Inventory full but a stone stack at 60: 4 of the 10 stones on the ground are stored, the 6
/// others stay, and nothing tells it (no pickup packet, no count sent, no statistic).
#[test]
fn partial_pickup_only_shrinks_the_ground_stack() {
    let (mut p, _dir) = open("partial");
    for slot in 9..45 {
        p.s.slots[slot] = Some(stack("minecraft:dirt", 64));
    }
    p.s.slots[36] = Some(stack("minecraft:stone", 60));
    let id = lay(&mut p, [8.5, GROUND, 8.5], 10);
    p.take_events();
    p.s.pick_up_touched_drops();
    assert_eq!(stones_held(&p), 64, "the 4 stones that fit are not stored");
    let ground = p.s.life.drops.iter().find(|d| d.id == id).expect("ground stack gone");
    assert_eq!(ground.stack.count, 6, "ground stack");
    let told = p.take_events().into_iter().any(|e| match e {
        Event::Misc(rc_client::misc::Misc::Take { item, .. }) => item == id,
        Event::Entity(EntityEvent::Data { id: entity, .. }) => entity == id,
        _ => false,
    });
    assert!(!told, "a partial pickup was shown to the client");
    assert_eq!(p.s.stat_value("minecraft:picked_up", "minecraft:stone"), 0, "partial pickup counted");
    // Room made: the 6 left are taken whole, and that pickup counts its 6.
    p.s.slots[9] = None;
    p.s.pick_up_touched_drops();
    assert_eq!(stones_held(&p), 70);
    assert_eq!(p.s.stat_value("minecraft:picked_up", "minecraft:stone"), 6, "whole pickup");
}
