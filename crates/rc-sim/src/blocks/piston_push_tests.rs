//! Pistons push every entity whose piston reaction is "push", projectiles included: a snowball
//! in the head's way leaves with it, an arrow left stuck in mid-air starts falling once pushed,
//! and a mob carried into a sweet berry bush is hurt by the push itself, like the game applies
//! the effects of the blocks a push crosses. Before: projectiles stayed in place and the pushed
//! mob only met the bush at its own next move (standing still there, it was never hurt).
//! Fresh flat world (26.3 version data mandatory).

use crate::blocks::probe::Probe;
use crate::player::Stack;
use crate::projectiles::{PKind, Proj};
use crate::tests_g3f13::{finish, open};

fn cmd(p: &mut Probe, c: &str) {
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, c);
    assert!(ok, "command refused: {c}");
}

fn game_ticks(p: &mut Probe, n: i32) {
    for _ in 0..n {
        p.s.game_tick();
        p.s.tick += 1;
    }
}

/// A piston facing east at (60, -60, z), powered: its head sweeps the cell (61, -60, z).
fn powered_piston(p: &mut Probe, z: i32) {
    cmd(p, &format!("setblock 60 -60 {z} minecraft:piston[facing=east]"));
    cmd(p, &format!("setblock 59 -60 {z} minecraft:redstone_block"));
}

/// A weightless, motionless projectile of `kind` added at `pos`; its identifier.
fn add_projectile(p: &mut Probe, kind: PKind, item: &str, pos: [f64; 3], stuck: bool) -> i32 {
    let id = p.s.proj_new_id();
    let mut pr = Proj::new(kind, id, pos, Stack { id: item.into(), count: 1, components: None });
    pr.no_gravity = true;
    // Stuck in the air: its own tick sees no change of the block at its tip, it would never fall.
    pr.in_ground = stuck;
    pr.last_state = stuck.then(|| p.s.air());
    p.s.proj_add(pr);
    id
}

fn projectile(p: &Probe, id: i32) -> &Proj {
    p.s.proj.list.iter().find(|q| q.id == id).expect("projectile")
}

fn close(got: f64, want: f64, what: &str) {
    assert!((got - want).abs() < 1.0e-6, "{what}: {got} instead of {want}");
}

#[test]
fn pistons_push_projectiles_and_free_a_stuck_arrow() {
    let (mut p, dir) = open("piston-projectiles");
    let snowball = add_projectile(&mut p, PKind::Snowball, "minecraft:snowball", [61.5, -59.9, 0.5], false);
    let arrow = add_projectile(&mut p, PKind::Arrow, "minecraft:arrow", [61.5, -59.9, 2.5], true);
    for z in [0, 2] {
        powered_piston(&mut p, z);
    }
    game_ticks(&mut p, 4);
    // Out of the extended head (box from x 62.01): half widths 0.125 and 0.25.
    close(projectile(&p, snowball).pos[0], 62.135, "snowball");
    close(projectile(&p, arrow).pos[0], 62.26, "arrow");
    assert!(!projectile(&p, arrow).in_ground, "the pushed arrow, free around its tip, should fall");
    finish(p, dir);
}

/// Each projectile has its own cap, 0.51 block per axis and per game tick.
#[test]
fn piston_cap_holds_for_projectiles() {
    let (mut p, dir) = open("piston-projectile-cap");
    let id = add_projectile(&mut p, PKind::Snowball, "minecraft:snowball", [20.5, -59.5, 0.5], false);
    let i = p.s.proj.list.iter().position(|q| q.id == id).expect("snowball");
    let mut pr = p.s.proj.list[i].clone();
    for _ in 0..2 {
        p.s.proj_piston_move(&mut pr, [0.4, 0.0, 0.0]);
    }
    close(pr.pos[0], 21.01, "capped within the tick");
    p.s.time += 1;
    p.s.proj_piston_move(&mut pr, [0.4, 0.0, 0.0]);
    close(pr.pos[0], 21.41, "at the next tick");
    finish(p, dir);
}

#[test]
fn pushed_mob_is_hurt_by_the_berry_bush_it_is_pushed_into() {
    let (mut p, dir) = open("piston-berries");
    cmd(&mut p, "setblock 62 -60 0 minecraft:sweet_berry_bush[age=1]");
    cmd(&mut p, "summon minecraft:pig 61.5 -60 0.5 {NoAI:1b,PersistenceRequired:1b}");
    let pig = p.s.mobs.list.iter().find(|m| m.kind == crate::mobs::species::Kind::Pig).expect("pig").id;
    let health = p.s.mobs.get(pig).expect("pig").health;
    powered_piston(&mut p, 0);
    game_ticks(&mut p, 4);
    let m = p.s.mobs.get(pig).expect("pig");
    assert!(m.pos()[0] > 62.0, "pig not pushed into the bush: {:?}", m.pos());
    assert_eq!(m.health, health - 1.0, "the push through the bush should hurt the pig once");
    finish(p, dir);
}
