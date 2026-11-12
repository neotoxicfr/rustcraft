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

/// The iron golem's own rule: a full top face under its feet (the default test took a slab).
#[test]
fn the_iron_golem_needs_a_full_floor() {
    let mut p = open("obstruction-golem");
    cmd(&mut p, "setblock 32 -61 4 minecraft:stone_slab[type=bottom]");
    assert!(!passes(&mut p, Kind::IronGolem, 32.5, 4.5, LiveWorld), "over a slab");
    assert!(passes(&mut p, Kind::IronGolem, 36.5, 4.5, LiveWorld), "on the grass");
}
