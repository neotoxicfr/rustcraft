//! The solid entities in the server's free-room tests (`solid_entities.rs`): the rule of who is
//! solid to whom, and the sites that made their test on the blocks alone before (each test below
//! fails on that code: the solid mob was not seen). Flat world of `tests_g3f13`: grass top at
//! y = -60, player standing at (0.5, -60, 0.5).

use rc_physics::Aabb;

use crate::blocks::Dir;
use crate::blocks::probe::Probe;
use crate::decoration::Use;
use crate::mobs::jrand::JRand;
use crate::mobs::mob::{Ent, Mob};
use crate::mobs::species::Kind;
use crate::player::Stack;
use crate::projectiles::{PKind, Proj};
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

fn cmd(p: &mut Probe, c: &str) {
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, c);
    assert!(ok, "command refused: {c}");
}

/// The world border's east edge at the whole `x`: a square of 200 blocks that holds every loaded
/// chunk. (The border's collision shape follows the blocks: an edge between two whole numbers
/// blocks from the next one.)
fn border_east_edge_at(p: &mut Probe, x: i32) {
    p.s.cmd.border.center = [f64::from(x) - 100.0, 0.0];
    p.s.cmd.border.size = 200.0;
}

/// The world border's west edge at the whole `x`, likewise.
fn border_west_edge_at(p: &mut Probe, x: i32) {
    p.s.cmd.border.center = [f64::from(x) + 100.0, 0.0];
    p.s.cmd.border.size = 200.0;
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

/// The move check's new-collision part counts a solid entity its search finds within 1e-7 of the
/// shrunk new box, even without touching it (blocks are kept only where they touch it).
#[test]
fn a_move_stopping_just_short_of_a_shulker_is_sent_back() {
    let mut p = open("solid-move-margin");
    add_shulker(&mut p, 2, 0);
    let half = p.s.player_pose().bbox_at([0.0; 3]).max[0];
    let margin = f64::from(1.0e-5f32);
    // The box enters the shulker (x = 2) by `depth`; shrunk by the margin, it stops short of it.
    let move_to_depth = |p: &mut Probe, depth: f64| {
        let start = p.s.pos;
        let target = [2.0 + depth - half, GROUND, 0.5];
        p.s.on_move(Some(target), None, true);
        let accepted = p.s.pos == target;
        p.s.teleport = None;
        p.s.on_move(Some(start), None, true);
        assert_eq!(p.s.pos, start, "back at the start");
        accepted
    };
    assert!(!move_to_depth(&mut p, margin - 5.0e-8), "5e-8 short of the shulker: within the search");
    assert!(move_to_depth(&mut p, margin - 2.0e-7), "2e-7 short of it: outside the search");
}

/// A monster spawner never puts its entity in a shulker, and its test, made for no entity, lets a
/// box across the world border through.
#[test]
fn a_spawner_spawns_around_a_shulker_and_across_the_border() {
    let mut p = open("solid-spawner");
    add_shulker(&mut p, 6, 0);
    let tnt_count = |p: &Probe| p.s.bs.entities.list.iter().filter(|e| !e.removed).count();
    let spawner_at = |p: &mut Probe, block: [i32; 3], pos: [f64; 3]| {
        cmd(
            p,
            &format!(
                "setblock {} {} {} minecraft:spawner{{Delay:0s,SpawnCount:1s,\
                 SpawnData:{{entity:{{id:\"minecraft:tnt\",fuse:80s,Pos:[{:?}d,{:?}d,{:?}d]}}}}}}",
                block[0], block[1], block[2], pos[0], pos[1], pos[2]
            ),
        );
        let players = p.s.alive_players();
        p.s.spawner_tick(block, &players);
    };
    spawner_at(&mut p, [3, GROUND as i32, 3], [6.5, GROUND, 0.5]);
    assert_eq!(tnt_count(&p), 0, "nothing spawned in the shulker");
    border_east_edge_at(&mut p, 9);
    spawner_at(&mut p, [3, GROUND as i32, 5], [8.9, GROUND, 0.5]);
    assert_eq!(tnt_count(&p), 1, "the primed TNT spawned across the border's edge");
}

/// The mobs spawned with a new chunk: animals of a flat world's generation spawns.
fn open_with_generation_spawns(tag: &str) -> Probe {
    let mut p = open(tag);
    // A flat world places none; the rule itself is the same for every generator.
    p.s.generator.as_mut().expect("the flat world's generator").mob_gen = true;
    cmd(&mut p, "gamerule minecraft:spawn_mobs true");
    p
}

/// Kind and position of the mobs that the generation spawns of chunk `c` add, in spawn order.
fn generation_spawns(p: &mut Probe, c: (i32, i32)) -> Vec<(Kind, [f64; 3])> {
    let before: Vec<i32> = p.s.mobs.list.iter().map(|m| m.id).collect();
    p.s.spawn_original_mobs(c);
    let mut added: Vec<&Mob> = p.s.mobs.list.iter().filter(|m| !before.contains(&m.id)).collect();
    added.sort_by_key(|m| m.id);
    added.iter().map(|m| (m.kind, m.pos())).collect()
}

/// The generation spawns test their room for no entity in a region that holds none: neither a
/// solid entity within the search's 1e-7 nor the world border across the box stops them.
#[test]
fn generation_spawns_see_neither_solid_entities_nor_the_border() {
    let mut p = open_with_generation_spawns("solid-generation");
    // The first spawn of the first chunk that has one clear of the chunk's sides (an x of its own,
    // not brought back inside the chunk, so its block is the one under its position).
    let clear_of_sides = |c: (i32, i32), kind: Kind, pos: [f64; 3]| {
        let (x0, w) = (f64::from(c.0 * 16), f64::from(kind.info().width));
        pos[0].fract() == 0.0 && pos[0] > x0 + w && pos[0] < x0 + 16.0 - w
    };
    let (chunk, (kind, first)) = (-2..=9)
        .flat_map(|x| (-2..=2).map(move |z| (x, z)))
        .find_map(|c| generation_spawns(&mut p, c).first().copied().filter(|&(k, pos)| clear_of_sides(c, k, pos)).map(|s| (c, s)))
        .expect("a generation spawn on the flat world");
    drop(p);

    // The same chunk in a fresh copy of the world, the border and a shulker against that spawn.
    let mut p = open_with_generation_spawns("solid-generation-again");
    let spawn_box = Mob::bbox_at(first, kind.info().width, kind.info().height);
    // The west edge through the spawn's position: its block inside, its box across.
    border_west_edge_at(&mut p, first[0] as i32);
    // A shulker against the box's east face: the free-room search would find it, but the box does
    // not overlap it, so no species' entity obstruction test (strict) refuses the spot either.
    let mut shulker = p.s.new_mob(Kind::Shulker, [spawn_box.max[0] + 0.5 + 1.0e-9, first[1], first[2]], 0.0);
    shulker.no_ai = true;
    let gap = shulker.bbox().min[0] - spawn_box.max[0];
    assert!(gap > 0.0 && gap < 1.0e-7, "the shulker is {gap} away");
    p.s.add_mob(shulker);
    let spawns = generation_spawns(&mut p, chunk);
    assert_eq!(spawns.first(), Some(&(kind, first)), "the same first spawn in chunk {chunk:?}");
}

/// An enderman's teleport, made for no entity: a shulker refuses it, the border does not.
#[test]
fn an_enderman_teleport_avoids_a_shulker_but_not_the_border() {
    let mut p = open("solid-enderman");
    add_shulker(&mut p, 9, 0);
    let mut enderman = p.s.new_mob(Kind::Enderman, [0.5, GROUND, 8.5], 0.0);
    assert!(!p.s.enderman_teleport(&mut enderman, [9.5, GROUND + 2.0, 0.5]), "into the shulker");
    border_east_edge_at(&mut p, 13);
    assert!(p.s.enderman_teleport(&mut enderman, [12.9, GROUND + 2.0, 0.5]), "across the border's edge");
    assert_eq!(enderman.pos(), [12.9, GROUND, 0.5]);
}

/// The respawn search, made for no entity: a shulker on the spawn point lifts it above the shulker
/// (whose top touches a box standing on it: a touching solid counts), the border does not (a spawn
/// box just beyond the edge, which the border's collision shape holds).
#[test]
fn the_respawn_search_avoids_a_shulker_but_not_the_border() {
    let mut p = open("solid-respawn");
    cmd(&mut p, "gamerule minecraft:respawn_radius 0");
    p.s.level.spawn_pos = [14, GROUND as i32, 0];
    assert_eq!(p.s.world_spawn_point().0, [14.5, GROUND, 0.5], "on the grass");
    add_shulker(&mut p, 14, 0);
    assert_eq!(p.s.world_spawn_point().0, [14.5, GROUND + 2.0, 0.5], "clear of the shulker");
    // An adventure world goes straight to the height fix, which a border radius does not change.
    p.s.level.game_type = 2;
    p.s.level.spawn_pos = [16, GROUND as i32, 0];
    border_east_edge_at(&mut p, 16);
    assert_eq!(p.s.world_spawn_point().0, [16.5, GROUND, 0.5], "just beyond the border's edge");
}

/// A stuck arrow falls when nothing holds its tip any more: a shulker holds it, the border does not.
#[test]
fn an_arrow_is_held_by_a_shulker_but_not_by_the_border() {
    let mut p = open("solid-arrow");
    add_shulker(&mut p, 3, 0);
    let arrow = |pos: [f64; 3]| {
        let stack = Stack { id: "minecraft:arrow".into(), count: 1, components: None };
        Proj::new(PKind::Arrow, 1_000_000, pos, stack)
    };
    assert!(!p.s.arrow_should_fall(&arrow([2.97, GROUND + 0.5, 0.5])), "its tip against the shulker");
    assert!(p.s.arrow_should_fall(&arrow([1.5, GROUND + 0.5, 0.5])), "in the open");
    border_east_edge_at(&mut p, 6);
    assert!(p.s.arrow_should_fall(&arrow([6.0, GROUND + 0.5, 0.5])), "across the border's edge");
}

/// The game's vehicle move check sends a move back when it failed and the old box was free, or when
/// the new box meets a collision the old one did not have. Ours lacks the second part (gap A3), so
/// the target is beyond the wall, clear of it: only the first part decides, the same in both. A
/// shulker in the old box counts (the horse was stuck, the move stands).
#[test]
fn a_vehicle_stuck_in_a_shulker_keeps_a_refused_move() {
    let mut p = open("solid-vehicle");
    cmd(&mut p, "fill 7 -60 4 7 -58 9 minecraft:stone");
    let start = [5.5, GROUND, 6.5];
    let mut horse = p.s.new_mob(Kind::Horse, start, 0.0);
    horse.no_ai = true;
    horse.equipment[crate::mobs::mounts::SADDLE] = Some(Stack { id: "minecraft:saddle".into(), count: 1, components: None });
    horse.mount.passengers.push(Ent::Player);
    let id = horse.id;
    p.s.add_mob(horse);
    p.s.mobs.mounts.riding.vehicle = Some(id);
    // Through the stone wall: the replayed move stops against it, more than 2.5 blocks short of a
    // target whose box (x from 8.8 to 10.2) touches neither the stone nor the shulker.
    let beyond_the_wall = [9.5, GROUND, 6.5];
    let ride_through_the_wall = |p: &mut Probe| {
        p.s.mobs.mounts.riding.last_good = start;
        p.s.move_vehicle(beyond_the_wall, 0.0, 0.0, true);
        mob(p, id).pos()
    };
    assert_eq!(ride_through_the_wall(&mut p), start, "a free horse is sent back");
    add_shulker(&mut p, 5, 6);
    assert_eq!(ride_through_the_wall(&mut p), beyond_the_wall, "a horse stuck in a shulker keeps the move");
}

/// A pet's teleport to its owner, made for the pet: never into a shulker, nor across the world
/// border (which a test made for no entity would let through).
#[test]
fn a_pet_teleports_neither_into_a_shulker_nor_across_the_border() {
    let mut p = open("solid-pet");
    let wolf_far_away = |p: &mut Probe| {
        let mut wolf = p.s.new_mob(Kind::Wolf, [0.5, GROUND, 12.5], 0.0);
        wolf.rng = JRand::new(7);
        wolf
    };
    // Control: in the open, the wolf lands near the player (at x, z = 0.5) within its tries.
    let mut wolf = wolf_far_away(&mut p);
    let landed = (0..20).any(|_| {
        p.s.teleport_to_owner_pub(&mut wolf);
        wolf.pos()[2] < 4.0
    });
    assert!(landed, "the wolf teleports to its owner");
    // Every cell the teleport can choose (two to three blocks from the owner) holds a shulker, but
    // for the column at x = 2, which the wolf then finds.
    for dx in -3..=3 {
        for dz in -3..=3 {
            let in_ring = i32::max(dx, dz) >= 2 || i32::min(dx, dz) <= -2;
            if in_ring && dx != 2 {
                add_shulker(&mut p, dx, dz);
            }
        }
    }
    let mut wolf = wolf_far_away(&mut p);
    let landed = (0..20).any(|_| {
        p.s.teleport_to_owner_pub(&mut wolf);
        wolf.pos()[2] < 4.0
    });
    assert!(landed, "the wolf teleports to its owner");
    assert_eq!(wolf.pos()[0], 2.5, "in the one column without a shulker");
    // The border's east edge at x = 2: that column is beyond it. The game counts the border when the
    // pet, where it stands, is inside and within two blocks of an edge: 1.5 blocks here.
    border_east_edge_at(&mut p, 2);
    let mut wolf = wolf_far_away(&mut p);
    for _ in 0..20 {
        p.s.teleport_to_owner_pub(&mut wolf);
    }
    assert_eq!(wolf.pos(), [0.5, GROUND, 12.5], "the wolf stays where it was");
}
