//! The solid entities in the server's free-room tests (`solid_entities.rs`): the rule of who is
//! solid to whom, and the sites that made their test on the blocks alone before (each test below
//! fails on that code: the solid mob was not seen). Flat world of `tests_g3f13`: grass top at
//! y = -60, player standing at (0.5, -60, 0.5).

use rc_physics::Aabb;

use crate::blocks::Dir;
use crate::blocks::probe::Probe;
use crate::decoration::Use;
use crate::mobs::mob::Mob;
use crate::mobs::species::Kind;
use crate::player::Stack;
use crate::solid_entities::RoomFor;

/// First air layer above the grass.
const GROUND: f64 = -60.0;

fn open(tag: &str) -> Probe {
    crate::tests_g3f13::open(tag).0
}

/// A shulker closed on the floor of the cell `(x, GROUND, z)` (box: that unit cube), without AI so
/// it neither peeks nor teleports; returns its id.
fn add_shulker(p: &mut Probe, x: i32, z: i32) -> i32 {
    let mut m = p.s.new_mob(Kind::Shulker, [f64::from(x) + 0.5, GROUND, f64::from(z) + 0.5], 0.0);
    m.no_ai = true;
    let id = m.id;
    p.s.add_mob(m);
    id
}

/// An adult happy ghast at `pos`, held still or not; returns its id.
fn add_happy_ghast(p: &mut Probe, pos: [f64; 3], still: bool) -> i32 {
    let mut m = p.s.new_mob(Kind::HappyGhast, pos, 0.0);
    m.no_ai = true;
    m.rec.stays_still = still;
    let id = m.id;
    p.s.add_mob(m);
    id
}

fn mob(p: &mut Probe, id: i32) -> &mut Mob {
    p.s.mobs.get_mut(id).expect("mob in the list")
}

fn unit_cell(x: i32, z: i32) -> Aabb {
    let (x, z) = (f64::from(x), f64::from(z));
    Aabb { min: [x, GROUND, z], max: [x + 1.0, GROUND + 1.0, z + 1.0] }
}

/// Who is solid to whom: a living shulker for every test, touching included (the search grows the
/// box by 1e-7), never to itself; a dead one never; a happy ghast only held still and adult, or
/// ridden for another happy ghast; nothing that shares the asker's root vehicle; the world border
/// only for a test made for an entity.
#[test]
fn solid_entity_rules_follow_the_game() {
    let mut p = open("solid-rules");
    let shulker = add_shulker(&mut p, 3, 0);
    let s = &p.s;
    let inside = unit_cell(3, 0).deflate(0.25);
    assert!(!s.room_free(RoomFor::NoEntity, &inside), "a shulker blocks a test made for no entity");
    let touching = Aabb { min: [2.0, GROUND, 0.0], max: [3.0, GROUND + 1.0, 1.0] };
    assert!(!s.room_free(RoomFor::NoEntity, &touching), "a simple contact counts");
    let apart = Aabb { min: [2.0, GROUND, 0.0], max: [3.0 - 1.0e-6, GROUND + 1.0, 1.0] };
    assert!(s.room_free(RoomFor::NoEntity, &apart), "1e-6 away it does not");
    let own = s.room_for_mob(s.mobs.get(shulker).expect("shulker"));
    assert!(s.room_free(own, &inside), "a shulker is not solid to itself");
    assert!(!s.room_free(s.room_for_player(), &inside), "it is solid to the player");
    mob(&mut p, shulker).health = 0.0;
    assert!(p.s.room_free(RoomFor::NoEntity, &inside), "a dead shulker is not solid");

    // A happy ghast in the air, its feet at (10.5, -50, 0.5); `in_ghast` is inside its box whether
    // adult or baby.
    let ghast = add_happy_ghast(&mut p, [10.5, -50.0, 0.5], false);
    let in_ghast = Aabb { min: [10.4, -49.9, 0.4], max: [10.6, -49.8, 0.6] };
    assert!(p.s.room_free(RoomFor::NoEntity, &in_ghast), "a moving happy ghast is not solid");
    mob(&mut p, ghast).rec.stays_still = true;
    assert!(!p.s.room_free(RoomFor::NoEntity, &in_ghast), "held still, it is");
    let g = mob(&mut p, ghast);
    (g.age, g.baby) = (-24000, true);
    assert!(g.bbox().intersects(&in_ghast), "the baby's box still holds the test box");
    assert!(p.s.room_free(RoomFor::NoEntity, &in_ghast), "a baby never is");
    let g = mob(&mut p, ghast);
    (g.age, g.baby) = (0, false);
    g.rec.stays_still = false;
    g.mount.passengers.push(crate::mobs::mob::Ent::Player);
    p.s.mobs.mounts.riding.vehicle = Some(ghast);
    assert!(p.s.room_free(RoomFor::NoEntity, &in_ghast), "ridden and moving: not solid to the others");
    let other_ghast = p.s.new_mob(Kind::HappyGhast, [30.5, -50.0, 0.5], 0.0);
    assert!(!p.s.room_free(p.s.room_for_mob(&other_ghast), &in_ghast), "but solid to another happy ghast");
    mob(&mut p, ghast).rec.stays_still = true;
    assert!(!p.s.room_free(RoomFor::NoEntity, &in_ghast), "held still and ridden: solid");
    assert!(p.s.room_free(p.s.room_for_player(), &in_ghast), "not to its own rider");

    // The world border: its edge at x = 1 across the box.
    p.s.cmd.border.center = [-9.0, 0.0];
    p.s.cmd.border.size = 20.0;
    let across = Aabb { min: [0.5, GROUND, 0.25], max: [1.5, GROUND + 1.0, 0.75] };
    assert!(p.s.room_free(RoomFor::NoEntity, &across), "a test made for no entity ignores the border");
    assert!(!p.s.room_free(p.s.room_for_player(), &across), "one made for an entity does not");
}

/// The player's move check counts the solid entities: a move whose box enters a shulker is a new
/// collision, refused and sent back (it was accepted: the check read the blocks alone).
#[test]
fn a_move_into_a_shulker_is_sent_back() {
    let mut p = open("solid-move");
    add_shulker(&mut p, 2, 0);
    let start = p.s.pos;
    let target = [1.9, GROUND, 0.5];
    p.s.on_move(Some(target), None, true);
    assert_eq!(p.s.pos, start, "the player stays where it was");
    assert!(p.s.teleport.is_some(), "and is sent back");
    let free = [0.5, GROUND, 3.5];
    p.s.teleport = None;
    p.s.on_move(Some(free), None, true);
    assert_eq!(p.s.pos, free, "a move away from it is accepted");
}

/// A shulker's room to open: a still happy ghast over it fills it (only shulkers counted before).
#[test]
fn a_still_happy_ghast_takes_a_shulkers_room() {
    let mut p = open("solid-shulker-room");
    let shulker = p.s.new_mob(Kind::Shulker, [5.5, GROUND, 0.5], 0.0);
    let cell = [5, GROUND as i32, 0];
    assert!(p.s.shulker_can_stay(&shulker, cell, 0), "room to open on the floor");
    // Its box from y = -58.5: the opened shulker would reach -58.
    let ghast = add_happy_ghast(&mut p, [5.5, GROUND + 1.5, 0.5], false);
    assert!(p.s.shulker_can_stay(&shulker, cell, 0), "a moving happy ghast lets it open");
    mob(&mut p, ghast).rec.stays_still = true;
    assert!(!p.s.shulker_can_stay(&shulker, cell, 0), "a still one does not");
}

/// A spawn egg's or an armor stand's height lands on a solid entity of the cell, as on a block.
#[test]
fn a_placed_entity_lands_on_a_shulker() {
    let mut p = open("solid-spawn-height");
    let cell = [7, GROUND as i32, 0];
    assert_eq!(p.s.box_y_offset(cell, false, 0.6, 1.8), 0.0, "on the grass");
    add_shulker(&mut p, 7, 0);
    assert_eq!(p.s.box_y_offset(cell, false, 0.6, 1.8), 1.0, "on the shulker");
}

/// Armor stand placement: a still happy ghast that only touches the stand's box refuses it (the
/// game's test grows the search by 1e-7, the entity lookup after it does not); an edge of the world
/// border across the box does not (the game's test is made for no entity).
#[test]
fn armor_stand_room_counts_touching_solids_but_not_the_border() {
    let mut p = open("solid-stand");
    let stand = Stack { id: "minecraft:armor_stand".into(), count: 1, components: None };
    let floor = [0, GROUND as i32 - 1, 3];
    // Stand box x from 0.25 to 0.75: the ghast's east face at x = 0.25.
    let ghast = add_happy_ghast(&mut p, [0.0, GROUND, 3.5], true);
    let half = {
        let b = mob(&mut p, ghast).bbox();
        (b.max[0] - b.min[0]) / 2.0
    };
    mob(&mut p, ghast).set_pos([0.25 - half, GROUND, 3.5]);
    p.s.slots[0] = Some(stand.clone());
    assert_eq!(p.s.stand_use_on(0, &stand, floor, Dir::Up), Some(Use::Fail), "touching a still happy ghast");
    mob(&mut p, ghast).rec.stays_still = false;
    assert_eq!(p.s.stand_use_on(0, &stand, floor, Dir::Up), Some(Use::Success), "a moving one is not solid");

    let mut p = open("solid-stand-border");
    p.s.cmd.border.center = [-9.5, 3.5];
    p.s.cmd.border.size = 20.0;
    p.s.slots[0] = Some(stand.clone());
    assert_eq!(p.s.stand_use_on(0, &stand, floor, Dir::Up), Some(Use::Success), "the border's edge at x = 0.5");
}

/// A random teleport (cat, chorus fruit) never lands in a shulker.
#[test]
fn a_random_teleport_does_not_land_in_a_shulker() {
    let mut p = open("solid-teleport");
    add_shulker(&mut p, 9, 0);
    let mut cat = p.s.new_mob(Kind::Cat, [0.5, GROUND, 6.5], 0.0);
    let into = [9.5, GROUND + 2.0, 0.5];
    assert!(!p.s.mob_random_teleport(&mut cat, into, "minecraft:cat_does_not_teleport_to"), "the shulker's cell");
    let beside = [9.5, GROUND + 2.0, 3.5];
    assert!(p.s.mob_random_teleport(&mut cat, beside, "minecraft:cat_does_not_teleport_to"), "a free cell");
    assert_eq!(cat.pos(), [9.5, GROUND, 3.5]);
}

/// A dropped item lands on a shulker like the client shows it (it fell through to the grass).
#[test]
fn a_dropped_item_lands_on_a_shulker() {
    let mut p = open("solid-item");
    add_shulker(&mut p, 4, 4);
    let stone = Stack { id: "minecraft:stone".into(), count: 1, components: None };
    p.s.spawn_item([4.5, GROUND + 3.0, 4.5], [0.0; 3], stone, 40);
    crate::bench_kit::run_ticks(&mut p, 40);
    let item = p.s.life.drops.first().expect("the item");
    assert!(item.on_ground, "the item rests");
    assert_eq!(item.pos[1], GROUND + 1.0, "on the shulker's top");
}
