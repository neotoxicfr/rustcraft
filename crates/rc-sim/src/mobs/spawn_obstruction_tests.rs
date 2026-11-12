//! The species' spawn obstruction tests (`Server::spawn_obstruction`), against the game's rules:
//! by default neither a liquid nor an entity that blocks building in the box; the fluid dwellers
//! look at the entities only, the ravager at the liquids only; the iron golem, the ocelot and the
//! warden add their own rule; a new chunk's region holds no entity. Flat world of `tests_g3f13`:
//! grass top at y = -60.

use super::spawn::SpawnRegion::{self, LiveWorld, NewChunk};
use super::species::Kind;
use crate::blocks::probe::Probe;
use crate::solid_entities_tests::{GROUND, cmd, open};

/// A cow standing at `(x, GROUND, z)`, in the mob list.
fn add_cow(p: &mut Probe, x: f64, z: f64) {
    let cow = p.s.new_mob(Kind::Cow, [x, GROUND, z], 0.0);
    p.s.add_mob(cow);
}

/// The spawn obstruction test of a new `kind` standing at `(x, GROUND, z)`, seen from `region`.
fn passes(p: &mut Probe, kind: Kind, x: f64, z: f64, region: SpawnRegion) -> bool {
    let m = p.s.new_mob(kind, [x, GROUND, z], 0.0);
    p.s.spawn_obstruction(&m, region)
}

/// The default test sees the entities: a zombie's box over a cow is refused (only the liquids were
/// read); the ravager's test reads the liquids only.
#[test]
fn the_default_spawn_obstruction_sees_the_entities() {
    let mut p = open("obstruction-default");
    add_cow(&mut p, 4.5, 4.5);
    assert!(!passes(&mut p, Kind::Zombie, 4.5, 4.5, LiveWorld), "over the cow");
    assert!(passes(&mut p, Kind::Zombie, 8.5, 4.5, LiveWorld), "beside it");
    assert!(passes(&mut p, Kind::Ravager, 4.5, 4.5, LiveWorld), "the ravager over the cow");
}

/// The fluid dwellers look at the entities only: a drowned under water over a cow is refused (it
/// looked at nothing), a tadpole under water is accepted (the water refused it).
#[test]
fn water_dwellers_look_at_the_entities_only() {
    let mut p = open("obstruction-water");
    cmd(&mut p, "fill 10 -60 10 12 -58 12 minecraft:water");
    add_cow(&mut p, 11.5, 11.5);
    assert!(!passes(&mut p, Kind::Drowned, 11.5, 11.5, LiveWorld), "a drowned over the cow");
    assert!(passes(&mut p, Kind::Tadpole, 10.5, 10.5, LiveWorld), "a tadpole under water");
}

/// A new chunk's region holds no entity: the spawns placed with it ignore the cows, whatever the
/// species' test (the axolotl's read the server's entities there).
#[test]
fn generation_spawns_ignore_the_entities() {
    let mut p = open("obstruction-generation");
    cmd(&mut p, "fill 14 -60 14 16 -58 16 minecraft:water");
    add_cow(&mut p, 15.5, 15.5);
    add_cow(&mut p, 20.5, 15.5);
    assert!(!passes(&mut p, Kind::Axolotl, 15.5, 15.5, LiveWorld), "an axolotl over a cow");
    assert!(passes(&mut p, Kind::Axolotl, 15.5, 15.5, NewChunk), "the same with a new chunk");
    assert!(!passes(&mut p, Kind::Sheep, 20.5, 15.5, LiveWorld), "a sheep over a cow");
    assert!(passes(&mut p, Kind::Sheep, 20.5, 15.5, NewChunk), "the same with a new chunk");
}

/// The warden's own rule, a free-room test of its box: a block in its box refuses it (only the
/// liquids and the entities were read).
#[test]
fn the_warden_needs_room_for_its_box() {
    let mut p = open("obstruction-warden");
    cmd(&mut p, "setblock 24 -58 4 minecraft:stone");
    assert!(!passes(&mut p, Kind::Warden, 24.5, 4.5, LiveWorld), "a stone in its box");
    assert!(passes(&mut p, Kind::Warden, 28.5, 4.5, LiveWorld), "in the open");
}

/// Every player of the world blocks a spawn, the LAN guests too (only the host was seen): a zombie
/// summoned by a spawner over a guest is refused, unless that guest is a spectator.
#[test]
fn a_lan_guest_blocks_a_spawn_over_it() {
    let mut p = open("obstruction-guest");
    let _link = super::tests_spawn_paths::add_guest(&mut p.s, "Guest", [40.5, GROUND, 4.5]);
    assert!(!passes(&mut p, Kind::Zombie, 40.5, 4.5, LiveWorld), "over the guest");
    assert!(passes(&mut p, Kind::Zombie, 44.5, 4.5, LiveWorld), "beside it");
    p.s.lan.as_mut().expect("LAN open").guests[0].ctx.as_mut().expect("guest state").game_mode = 3;
    assert!(passes(&mut p, Kind::Zombie, 40.5, 4.5, LiveWorld), "over a spectator guest");
}

/// A zombie-sized box standing at `(x, y, z)`.
fn zombie_box(x: f64, y: f64, z: f64) -> rc_physics::Aabb {
    rc_physics::Aabb { min: [x - 0.3, y, z - 0.3], max: [x + 0.3, y + 1.95, z + 0.3] }
}

/// Both ways of finding a mob in a spawn's way (`RUSTCRAFT_OPT_OBSTRUCTION_BOUND_FIRST`: species
/// bound read first, or the whole mob read) answer alike on a grid of boxes around a cow, a big
/// slime (a box that changes with its size), a removed pig and the tested sheep itself; a few
/// answers are fixed.
#[test]
fn both_ways_of_finding_a_mob_in_the_way_agree() {
    let mut p = open("obstruction-both-ways");
    add_cow(&mut p, 50.5, 4.5);
    let mut slime = p.s.new_mob(Kind::Slime, [53.5, GROUND, 4.5], 0.0);
    slime.set_size(3, true);
    p.s.add_mob(slime);
    let mut pig = p.s.new_mob(Kind::Pig, [56.5, GROUND, 4.5], 0.0);
    pig.removed = true;
    p.s.add_mob(pig);
    let sheep = p.s.new_mob(Kind::Sheep, [59.5, GROUND, 4.5], 0.0);
    let tested = sheep.id;
    p.s.add_mob(sheep);
    let in_the_way = |p: &Probe, x: f64, y: f64, z: f64| {
        let b = zombie_box(x, y, z);
        let (bound_first, whole_mob) = (p.s.mob_in_the_way(&b, tested, true), p.s.mob_in_the_way(&b, tested, false));
        assert_eq!(bound_first, whole_mob, "box at ({x}, {y}, {z})");
        bound_first
    };
    assert!(in_the_way(&p, 50.5, GROUND, 4.5), "over the cow");
    assert!(in_the_way(&p, 54.3, GROUND, 4.5), "under the big slime's edge");
    assert!(!in_the_way(&p, 56.5, GROUND, 4.5), "over the removed pig");
    assert!(!in_the_way(&p, 59.5, GROUND, 4.5), "over the tested sheep");
    assert!(!in_the_way(&p, 62.5, GROUND, 4.5), "in the open");
    let (mut blocked, mut free) = (0, 0);
    for step_x in 0..=64 {
        for step_z in 0..=24 {
            for y in [GROUND - 2.0, GROUND, GROUND + 1.5] {
                let (x, z) = (47.0 + f64::from(step_x) * 0.25, 1.5 + f64::from(step_z) * 0.25);
                if in_the_way(&p, x, y, z) { blocked += 1 } else { free += 1 }
            }
        }
    }
    assert!(blocked > 0 && free > 0, "{blocked} boxes blocked, {free} free");
}

/// The iron golem's own rule: a full top face under its feet (the default test took a slab).
#[test]
fn the_iron_golem_needs_a_full_floor() {
    let mut p = open("obstruction-golem");
    cmd(&mut p, "setblock 32 -61 4 minecraft:stone_slab[type=bottom]");
    assert!(!passes(&mut p, Kind::IronGolem, 32.5, 4.5, LiveWorld), "over a slab");
    assert!(passes(&mut p, Kind::IronGolem, 36.5, 4.5, LiveWorld), "on the grass");
}
