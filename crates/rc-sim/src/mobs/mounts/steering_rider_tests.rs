//! A rider that steers its mount by a brain (a baby piglin on a baby hoglin) in a fresh flat
//! world: the path its brain asks for goes to the navigation of the mount, which walks it. The
//! 26.3 data cache and the client jar are required: the tests fail without them.

use crate::bench_kit::cmd;
use crate::blocks::probe::Probe;
use crate::mobs::brainy::brain::Walk;
use crate::mobs::species::Kind;

/// First air layer of the flat world.
const FLOOR_Y: i32 = -60;
/// Where the pair stands, and the cell the rider's brain wants to walk to (6 blocks east).
const PAIR_AT: [f64; 3] = [8.5, -60.0, 0.5];
const WALK_TO: [i32; 3] = [14, FLOOR_Y, 0];

fn mob_of_kind(p: &Probe, kind: Kind) -> i32 {
    p.s.mobs.list.iter().find(|m| m.kind == kind).map(|m| m.id).unwrap_or_else(|| panic!("no {kind:?}"))
}

/// A baby piglin riding a baby hoglin, the hoglin on the ground; returns (piglin, hoglin).
fn baby_piglin_on_baby_hoglin(p: &mut Probe) -> (i32, i32) {
    let [x, y, z] = PAIR_AT;
    cmd(p, &format!("summon minecraft:hoglin {x} {y} {z} {{Age:-24000,PersistenceRequired:1b}}"));
    cmd(p, &format!("summon minecraft:piglin {x} {y} {z} {{IsBaby:1b,PersistenceRequired:1b}}"));
    let (piglin, hoglin) = (mob_of_kind(p, Kind::Piglin), mob_of_kind(p, Kind::Hoglin));
    let (piglin_slot, hoglin_slot) = (p.s.mob_index(piglin).expect("piglin"), p.s.mob_index(hoglin).expect("hoglin"));
    let mut rider = p.s.mobs.list.take(piglin_slot).expect("piglin");
    let mut mount = p.s.mobs.list.take(hoglin_slot).expect("hoglin");
    assert!(rider.is_baby() && mount.is_baby(), "babies");
    assert!(p.s.mob_start_riding(&mut rider, &mut mount, true), "piglin not mounted");
    mount.on_ground = true;
    p.s.mobs.list.put(hoglin_slot, mount);
    p.s.mobs.list.put(piglin_slot, rider);
    (piglin, hoglin)
}

/// One tick of the rider, taken out of the mob list as the server does, its brain told to walk
/// to [`WALK_TO`] just before.
fn tick_rider_walking_to_the_east(p: &mut Probe, rider: i32) {
    let i = p.s.mob_index(rider).expect("rider in the list");
    let mut m = p.s.mobs.list.take(i).expect("rider");
    let brain = m.bm.brain.as_mut().expect("piglin brain");
    let to = WALK_TO.map(f64::from);
    brain.mem.walk.set(Walk::to([to[0] + 0.5, to[1], to[2] + 0.5], 1.0, 0));
    p.s.tick_mob(&mut m);
    p.s.mobs.list.put(i, m);
}

#[test]
fn path_asked_by_the_brain_of_a_steering_rider_goes_to_its_mount() {
    let (mut p, _world_folder) = crate::tests_g3f13::open("piglin-steers-hoglin");
    let (piglin, hoglin) = baby_piglin_on_baby_hoglin(&mut p);
    assert_eq!(p.s.steered_mount(p.s.mob_ref(piglin).expect("piglin")).map(|v| v.id), Some(hoglin), "not steered");

    tick_rider_walking_to_the_east(&mut p, piglin);

    let rider = p.s.mob_ref(piglin).expect("piglin");
    assert!(rider.nav.path.is_none(), "the path stayed in the rider's own navigation: {:?}", rider.nav.path);
    let mount = p.s.mob_ref(hoglin).expect("hoglin");
    let path = mount.nav.path.as_ref().expect("no path in the mount's navigation");
    assert_eq!(path.end(), Some(WALK_TO), "path of the mount: {path:?}");
    assert_eq!(path.nodes[0].0, mount.block_pos(), "the path starts where the mount stands");
}
