//! The server's movements against the solid entities, one test per family of movement sites
//! (mobs and vehicles, the player's replay, primed TNT, falling blocks, armor stands, projectiles,
//! minecarts), and the experience orb's own rules. Each test fails when its site goes back to the
//! plain world (blocks and border only) or, for the orb, to the code before (its stuck tests
//! counting the border, no way out of a block at spawn). Flat world of `tests_g3f13`: grass top at
//! y = -60, player standing at (0.5, -60, 0.5).

use crate::blocks::probe::Probe;
use crate::mobs::species::Kind;
use crate::player::Stack;
use crate::projectiles::{PKind, Proj};
use crate::solid_entities_tests::{GROUND, add_shulker, border_east_edge_at, cmd, open};

/// The position of the last entity of type `ty` (a summoned one).
fn last_position_of(p: &Probe, ty: &str) -> [f64; 3] {
    let e = p.s.all_ents().into_iter().rfind(|e| p.s.ent_type(*e) == ty).expect("the summoned entity");
    p.s.e_pos(e)
}

/// A mob's movement stops against a shulker's face (it walked into it).
#[test]
fn a_mob_walks_into_a_shulker_and_stops() {
    let mut p = open("move-mob");
    add_shulker(&mut p, 3, 0);
    let mut zombie = p.s.new_mob(Kind::Zombie, [1.5, GROUND, 0.5], 0.0);
    p.s.mob_move(&mut zombie, [2.0, 0.0, 0.0]);
    assert!((zombie.bbox().max[0] - 3.0).abs() < 1.0e-9, "against the shulker's west face: {:?}", zombie.pos());
    assert!(zombie.horizontal_collision);
}

/// A strider walking on a lava pool stops at the world border's edge: its lava world forwards the
/// border (it walked across it).
#[test]
fn a_strider_on_lava_stops_at_the_border() {
    let mut p = open("move-strider-border");
    cmd(&mut p, "fill 24 -61 -1 34 -61 1 minecraft:lava");
    border_east_edge_at(&mut p, 30);
    let mut strider = p.s.new_mob(Kind::Strider, [27.5, GROUND - 0.5, 0.5], 0.0);
    p.s.mob_move(&mut strider, [3.0, 0.0, 0.0]);
    assert!((strider.bbox().max[0] - 30.0).abs() < 1.0e-9, "at the border's edge: {:?}", strider.pos());
    assert_eq!(strider.pos()[1], GROUND - 0.5, "still on the lava");
}

/// A strider stops against a shulker's face: its lava world forwards the solid entities.
#[test]
fn a_strider_walks_into_a_shulker_and_stops() {
    let mut p = open("move-strider-shulker");
    add_shulker(&mut p, 3, 4);
    let mut strider = p.s.new_mob(Kind::Strider, [1.5, GROUND, 4.5], 0.0);
    p.s.mob_move(&mut strider, [2.0, 0.0, 0.0]);
    assert!((strider.bbox().max[0] - 3.0).abs() < 1.0e-9, "against the shulker's west face: {:?}", strider.pos());
}

/// A zombie falling more than 2.5 blocks moves through the powder snow world: it lands on a
/// shulker's top (it fell through to the grass).
#[test]
fn a_zombie_falling_from_high_lands_on_a_shulker() {
    let mut p = open("move-fall-shulker");
    add_shulker(&mut p, 3, 8);
    let mut zombie = p.s.new_mob(Kind::Zombie, [3.5, GROUND + 4.0, 8.5], 0.0);
    zombie.fall_distance = 4.0;
    p.s.mob_move(&mut zombie, [0.0, -4.0, 0.0]);
    assert_eq!(zombie.pos()[1], GROUND + 1.0, "on the shulker's top");
}

/// A fox walks on powder snow, so it always moves through the powder snow world: it stops against
/// a shulker's face (it walked into it).
#[test]
fn a_fox_walks_into_a_shulker_and_stops() {
    let mut p = open("move-fox-shulker");
    add_shulker(&mut p, 3, 12);
    let mut fox = p.s.new_mob(Kind::Fox, [1.5, GROUND, 12.5], 0.0);
    p.s.mob_move(&mut fox, [2.0, 0.0, 0.0]);
    assert!((fox.bbox().max[0] - 3.0).abs() < 1.0e-9, "against the shulker's west face: {:?}", fox.pos());
}

/// The move check tests the old box with the player where its replay left it: a player whose box
/// crosses the border's edge, whose replay is stopped by a wall 2.7 blocks inside, is sent back
/// (judged from where it started, the border counted, the old box collided and the move passed).
#[test]
fn the_move_check_counts_the_border_from_where_the_replay_ends() {
    let mut p = open("move-replay-border");
    p.s.game_mode = 0;
    cmd(&mut p, "fill 26 -60 20 26 -59 20 minecraft:stone");
    border_east_edge_at(&mut p, 30);
    let start = [29.9, GROUND, 20.5];
    p.s.pos = start;
    p.s.last_good = start;
    p.s.on_move(Some([25.0, GROUND, 20.5]), None, true);
    assert_eq!(p.s.pos, start, "the move beyond the wall is sent back");
}

/// The player's replayed move stops at a shulker, so a move past it (the box at the target clear
/// of it) fails and is sent back (the replay went through it and accepted the move).
#[test]
fn the_players_replay_stops_at_a_shulker() {
    let mut p = open("move-replay");
    p.s.game_mode = 0;
    add_shulker(&mut p, 2, 0);
    let start = p.s.pos;
    p.s.on_move(Some([3.9, GROUND, 0.5]), None, true);
    assert_eq!(p.s.pos, start, "the move past the shulker is sent back");
    p.s.teleport = None;
    let elsewhere = [0.5, GROUND, 3.9];
    p.s.on_move(Some(elsewhere), None, true);
    assert_eq!(p.s.pos, elsewhere, "the same distance in the open is accepted");
}

/// A primed TNT falling onto a shulker rests on its top (it fell through to the grass).
#[test]
fn a_primed_tnt_lands_on_a_shulker() {
    let mut p = open("move-tnt");
    add_shulker(&mut p, 4, 2);
    cmd(&mut p, "summon minecraft:tnt 4.5 -57 2.5 {fuse:80s}");
    crate::bench_kit::run_ticks(&mut p, 40);
    let y = p.s.bs.entities.list.iter().find(|e| !e.removed).expect("the primed TNT").pos[1];
    assert!((GROUND + 1.0..GROUND + 1.1).contains(&y), "on the shulker's top: y = {y}");
}

/// Falling sand lands on a shulker, over an empty cell, so it drops as an item like the game's (it
/// fell through and took the shulker's cell as a block).
#[test]
fn falling_sand_lands_on_a_shulker() {
    let mut p = open("move-sand");
    add_shulker(&mut p, 6, 2);
    cmd(&mut p, "summon minecraft:falling_block 6.5 -56 2.5 {BlockState:{Name:\"minecraft:sand\"}}");
    crate::bench_kit::run_ticks(&mut p, 40);
    assert_ne!(p.s.bname([6, GROUND as i32, 2]), "minecraft:sand", "no block in the shulker's cell");
    assert!(p.s.life.drops.iter().any(|d| d.stack.id == "minecraft:sand"), "dropped as an item");
}

/// An armor stand falling onto a shulker stands on its top (it fell through to the grass).
#[test]
fn an_armor_stand_lands_on_a_shulker() {
    let mut p = open("move-stand");
    add_shulker(&mut p, 8, 2);
    cmd(&mut p, "summon minecraft:armor_stand 8.5 -57 2.5");
    crate::bench_kit::run_ticks(&mut p, 40);
    assert_eq!(last_position_of(&p, "minecraft:armor_stand")[1], GROUND + 1.0, "on the shulker's top");
}

/// A projectile moved with its collisions (a rocket, a fishing hook) stops against a shulker.
#[test]
fn a_projectiles_movement_stops_against_a_shulker() {
    let mut p = open("move-projectile");
    add_shulker(&mut p, 12, 0);
    let rocket = Stack { id: "minecraft:firework_rocket".into(), count: 1, components: None };
    let mut proj = Proj::new(PKind::Firework, 1_000_000, [10.5, GROUND + 0.5, 0.5], rocket);
    p.s.proj_move_collide(&mut proj, [3.0, 0.0, 0.0]);
    assert!((proj.bbox().max[0] - 12.0).abs() < 1.0e-9, "against the shulker's west face: {:?}", proj.pos);
    assert!(proj.horizontal_collision);
}

/// A minecart off its rails stops against a shulker (it rolled into it).
#[test]
fn a_minecart_stops_against_a_shulker() {
    let mut p = open("move-minecart");
    add_shulker(&mut p, 16, 0);
    cmd(&mut p, "summon minecraft:minecart 14.5 -60 0.5");
    let slot = p.s.carts.list.iter().rposition(Option::is_some).expect("the minecart");
    let mut cart = p.s.carts.list[slot].take().expect("the minecart");
    p.s.cart_move(&mut cart, [2.0, 0.0, 0.0]);
    let front = cart.bbox().max[0];
    p.s.carts.list[slot] = Some(cart);
    assert!((front - 16.0).abs() < 1.0e-9, "against the shulker's west face: front at x = {front}");
}

/// A new orb stuck in the ground goes to the closest free spot (the game's free position search on
/// spawn): its box 0.2 inside the grass comes out on top of it (it stayed in the grass).
#[test]
fn a_new_orb_stuck_in_the_ground_comes_out_on_top() {
    let mut p = open("orb-out-of-the-ground");
    p.s.add_orb([20.5, GROUND - 0.2, 3.5], 1);
    let orb = p.s.life.orbs.last().expect("the orb");
    assert!((orb.pos[1] - GROUND).abs() < 1.0e-9, "on the grass: {:?}", orb.pos);
    assert_eq!([orb.pos[0], orb.pos[2]], [20.5, 3.5], "straight up");
}

/// An orb's stuck tests are made for no entity: a still orb in the air across the world border's
/// edge falls straight down (counted, the border held it up without gravity and pushed it west,
/// out of the border's shape).
#[test]
fn an_orb_across_the_border_falls_straight_down() {
    let mut p = open("orb-border");
    border_east_edge_at(&mut p, 30);
    p.s.add_orb([30.0, GROUND + 5.0, 0.5], 1);
    p.s.life.orbs.last_mut().expect("the orb").vel = [0.0; 3];
    crate::bench_kit::run_ticks(&mut p, 80);
    let orb = p.s.life.orbs.last().expect("the orb");
    assert_eq!(orb.pos[0], 30.0, "never pushed sideways");
    assert!(orb.pos[1] < GROUND + 0.5, "fallen to the grass: {:?}", orb.pos);
}
