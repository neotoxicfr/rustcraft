//! Rules of the ground path search on scenes built by hand in a fresh flat world (grass at
//! y = -61, first air layer at y = -60): the camel walks over fences, the strider starts its
//! paths from the top of the lava it stands in. The 26.3 data cache and the client jar are
//! required: the tests fail without them.

use super::search_tests::path_line;
use super::{MemoKind, Path, PathType, SearchOptions, TypeRules, find_path, find_path_with};
use crate::bench_kit::{cmd, run_ticks};
use crate::blocks::probe::Probe;
use crate::mobs::mob::Mob;
use crate::mobs::species::Kind;

/// First air layer of the flat world.
const FLOOR_Y: i32 = -60;
/// Fence ring around the camel: x = 4 and x = 14, z = -6 and z = 6.
const RING_X: (i32, i32) = (4, 14);
const RING_Z: (i32, i32) = (-6, 6);
/// A cell east of the ring, 11 blocks from the camel (within its 16 block search).
const OUTSIDE_THE_RING: [i32; 3] = [20, FLOOR_Y, 0];

fn build_fence_ring(p: &mut Probe) {
    let ((x0, x1), (z0, z1), y) = (RING_X, RING_Z, FLOOR_Y);
    for (from, to) in [([x0, z0], [x1, z0]), ([x0, z1], [x1, z1]), ([x0, z0], [x0, z1]), ([x1, z0], [x1, z1])] {
        cmd(p, &format!("fill {} {y} {} {} {y} {} minecraft:oak_fence", from[0], from[1], to[0], to[1]));
    }
}

fn inside_the_ring(cell: [i32; 3]) -> bool {
    RING_X.0 < cell[0] && cell[0] < RING_X.1 && RING_Z.0 < cell[2] && cell[2] < RING_Z.1
}

/// A camel standing in the middle of the ring.
fn camel_in_the_ring(p: &mut Probe, kind: Kind) -> Mob {
    let mut camel = p.s.new_mob(kind, [9.0, f64::from(FLOOR_Y), 0.0], 0.0);
    camel.on_ground = true;
    camel
}

fn manhattan(a: [i32; 3], b: [i32; 3]) -> i32 {
    (0..3).map(|i| (a[i] - b[i]).abs()).sum()
}

fn path_out_of_the_ring(p: &Probe, mob: &mut Mob) -> Path {
    p.s.create_path(mob, OUTSIDE_THE_RING, 1).expect("a path, at least towards the nearest cell")
}

#[test]
fn camel_walks_over_the_fence_ring_that_holds_other_mounts() {
    let (mut p, _world_folder) = crate::tests_g3f13::open("camel-fence-ring");
    build_fence_ring(&mut p);
    for kind in [Kind::Camel, Kind::CamelHusk] {
        let mut camel = camel_in_the_ring(&mut p, kind);
        // The step of a camel (1.5) is what lets it onto a fence (1.5 high).
        assert_eq!(camel.step_height(), 1.5, "{kind:?} step");
        let path = path_out_of_the_ring(&p, &mut camel);
        let line = path_line(&Some(path.clone()));
        assert!(path.reached, "{kind:?} stays in the ring: {line}");
        assert!(manhattan(path.end().expect("last cell"), OUTSIDE_THE_RING) <= 1, "{kind:?} end: {line}");
        // It crosses one cell up, its feet level with the cell above the fence.
        let over_the_fence = |cell: &([i32; 3], PathType)| cell.0[1] == FLOOR_Y + 1 && cell.1 == PathType::Walkable;
        assert!(path.nodes.iter().any(over_the_fence), "{kind:?} never steps over the fence: {line}");

        // The same body without the rule is held by the ring, like a horse.
        let mut view = p.s.mob_view(&camel);
        assert!(view.crosses_fences, "{kind:?} crosses fences");
        view.crosses_fences = false;
        let held = find_path(&p.s, &view, OUTSIDE_THE_RING, 16.0, 1, 256).expect("a path");
        let line = path_line(&Some(held.clone()));
        assert!(!held.reached, "{kind:?} without the rule leaves the ring: {line}");
        assert!(held.nodes.iter().all(|cell| inside_the_ring(cell.0) && cell.0[1] == FLOOR_Y), "{line}");
    }
    let mut horse = camel_in_the_ring(&mut p, Kind::Horse);
    assert!(!p.s.mob_view(&horse).crosses_fences, "a horse does not cross fences");
    let held = path_out_of_the_ring(&p, &mut horse);
    assert!(!held.reached && held.nodes.iter().all(|cell| inside_the_ring(cell.0)), "horse: {held:?}");
}

/// A husk riding the camel husk standing in the middle of the ring (the pair a desert spawn
/// makes); the camel is in the mob list, the husk is handed back like a mob taken for its tick.
fn husk_steering_a_camel_in_the_ring(p: &mut Probe) -> (Mob, i32) {
    let mut camel = camel_in_the_ring(p, Kind::CamelHusk);
    let mut husk = p.s.new_mob(Kind::Husk, camel.pos(), 0.0);
    assert!(p.s.mob_start_riding(&mut husk, &mut camel, true), "husk not mounted");
    let camel_id = camel.id;
    p.s.add_mob(camel);
    (husk, camel_id)
}

/// The path a goal of the rider gets: asked between the loan and the return of the mount's
/// navigation, as the AI step of the rider does around its goals.
fn path_asked_by_the_rider(p: &mut Probe, rider: &mut Mob, target: [i32; 3]) -> Path {
    let lender = p.s.borrow_vehicle_nav(rider);
    assert!(lender.is_some(), "the rider does not steer its mount");
    let path = p.s.create_path(rider, target, 1).expect("a path, at least towards the nearest cell");
    p.s.return_vehicle_nav(rider, lender);
    path
}

/// The goals of a steering rider plan with the navigation of the mount, which searches for the
/// mount: its cell, its body and step, its fence rule, its follow range.
#[test]
fn rider_steering_a_camel_gets_the_path_of_the_camel() {
    let (mut p, _world_folder) = crate::tests_g3f13::open("husk-on-camel-fence-ring");
    build_fence_ring(&mut p);
    let (mut husk, camel_id) = husk_steering_a_camel_in_the_ring(&mut p);
    let camel = p.s.mob_ref(camel_id).expect("camel").clone();
    // The rider sees further than the camel.
    assert_eq!((husk.follow_range(), camel.follow_range()), (35.0, 16.0), "follow ranges");

    let path = path_asked_by_the_rider(&mut p, &mut husk, OUTSIDE_THE_RING);
    let line = path_line(&Some(path.clone()));
    assert!(path.reached, "the ridden camel stays in the ring: {line}");
    assert_eq!(path.nodes[0].0, camel.block_pos(), "the path starts where the camel stands: {line}");
    let over_the_fence = |cell: &([i32; 3], PathType)| cell.0[1] == FLOOR_Y + 1 && cell.1 == PathType::Walkable;
    assert!(path.nodes.iter().any(over_the_fence), "the ridden camel never steps over the fence: {line}");
    // The same path as the camel alone.
    let mut alone = camel_in_the_ring(&mut p, Kind::CamelHusk);
    assert_eq!(line, path_line(&Some(path_out_of_the_ring(&p, &mut alone))), "ridden and alone");

    // A target 25 cells east: within the sight of the husk, beyond the 16 cells a camel searches.
    let far_east = [camel.block_pos()[0] + 25, FLOOR_Y, 0];
    let far = path_asked_by_the_rider(&mut p, &mut husk, far_east);
    let line = path_line(&Some(far.clone()));
    let end = far.end().expect("last cell");
    assert!(!far.reached && end[0] - camel.block_pos()[0] <= 16, "searched further than the camel does: {line}");
    assert!(!inside_the_ring(end), "{line}");

    // On foot, the husk is held by the ring.
    let mut on_foot = p.s.new_mob(Kind::Husk, camel.pos(), 0.0);
    on_foot.on_ground = true;
    let held = path_out_of_the_ring(&p, &mut on_foot);
    assert!(!held.reached && held.nodes.iter().all(|cell| inside_the_ring(cell.0)), "husk on foot: {held:?}");
}

/// In play: the husk hunts the player standing west of the ring, and the camel it steers carries
/// it over the fences.
#[test]
fn husk_on_a_camel_rides_out_of_the_fence_ring_after_the_player() {
    let (mut p, _world_folder) = crate::tests_g3f13::open("husk-on-camel-hunt");
    build_fence_ring(&mut p);
    let (husk, camel_id) = husk_steering_a_camel_in_the_ring(&mut p);
    assert!(!inside_the_ring(crate::mobs::mth::block_of(p.s.pos)), "the player stands outside the ring");
    p.s.add_mob(husk);
    let mut highest_feet = f64::from(FLOOR_Y);
    for tick in 0..300 {
        run_ticks(&mut p, 1);
        let camel = p.s.mob_ref(camel_id).expect("camel");
        highest_feet = highest_feet.max(camel.pos()[1]);
        if !inside_the_ring(camel.block_pos()) && camel.block_pos()[0] < RING_X.0 {
            // It went over the fence (1.5 high), not through it.
            assert!(highest_feet >= f64::from(FLOOR_Y) + 1.5, "left at tick {tick}, feet never above {highest_feet}");
            return;
        }
    }
    let camel = p.s.mob_ref(camel_id).expect("camel");
    panic!("after 300 ticks the ridden camel is still penned at {:?}, feet never above {highest_feet}", camel.pos());
}

/// The search over the fence explores cells no other mob reaches (above the fences): every
/// optimisation lead of the search, alone and together, gives the path found with all of them off.
#[test]
fn fence_crossing_path_is_the_same_with_every_search_lead() {
    let (mut p, _world_folder) = crate::tests_g3f13::open("camel-fence-leads");
    build_fence_ring(&mut p);
    let camel = camel_in_the_ring(&mut p, Kind::Camel);
    let view = p.s.mob_view(&camel);
    let search = |opts: SearchOptions| {
        // Twice: the second search reuses the buffers the first one kept.
        let first = path_line(&find_path_with(&p.s, &view, &[OUTSIDE_THE_RING], 16.0, 1, 256, opts));
        assert_eq!(path_line(&find_path_with(&p.s, &view, &[OUTSIDE_THE_RING], 16.0, 1, 256, opts)), first);
        first
    };
    let old = SearchOptions::OLD;
    let want = search(old);
    assert!(want.contains(" true "), "with every lead off the camel stays in the ring: {want}");
    for opts in [
        SearchOptions { type_rules: TypeRules::StateOnce, ..old },
        SearchOptions { type_rules: TypeRules::Table, ..old },
        SearchOptions { memo: MemoKind::HashMap, ..old },
        SearchOptions { memo: MemoKind::DirectMapped, ..old },
        SearchOptions { scratch: true, ..old },
        SearchOptions { region: true, ..old },
        SearchOptions { palette: true, ..old },
        SearchOptions::ALL_NEW,
    ] {
        assert_eq!(search(opts), want, "{opts:?}");
    }
}

/// Lava pool `depth` cells deep (x 20..=26, z -3..=3) whose top layer takes the place of the grass.
fn dig_lava_pool(p: &mut Probe, depth: i32) {
    cmd(p, &format!("fill 20 {} -3 26 {} 3 minecraft:lava", FLOOR_Y - depth, FLOOR_Y - 1));
}

/// First cell of the path of a mob of `kind` whose feet are at height `feet_y` in the middle of the pool.
fn first_cell_from_the_pool(p: &mut Probe, kind: Kind, feet_y: f64, on_ground: bool) -> ([i32; 3], PathType) {
    let mut mob = p.s.new_mob(kind, [23.5, feet_y, 0.5], 0.0);
    mob.on_ground = on_ground;
    mob.lava_height = 0.5;
    let path = p.s.create_path(&mut mob, [25, FLOOR_Y, 0], 1).expect("a path");
    path.nodes[0]
}

#[test]
fn strider_path_starts_in_the_top_lava_cell() {
    let top_lava_cell = ([23, FLOOR_Y - 1, 0], PathType::Lava);
    let (mut p, _world_folder) = crate::tests_g3f13::open("strider-lava-start");
    dig_lava_pool(&mut p, 1);
    // Standing on the surface: its feet are half a cell up in the lava cell.
    let standing = first_cell_from_the_pool(&mut p, Kind::Strider, f64::from(FLOOR_Y) - 0.5, true);
    assert_eq!(standing, top_lava_cell, "strider standing on the lava");

    dig_lava_pool(&mut p, 3);
    let pool_bottom = f64::from(FLOOR_Y - 3);
    // Sunk to the bottom of a deep pool: still the top cell of the lava column.
    for on_ground in [false, true] {
        let sunk = first_cell_from_the_pool(&mut p, Kind::Strider, pool_bottom, on_ground);
        assert_eq!(sunk, top_lava_cell, "strider at the bottom of the pool, on ground {on_ground}");
    }
    // A mob that does not stand on lava starts where its feet are.
    let zombie = first_cell_from_the_pool(&mut p, Kind::Zombie, pool_bottom, true);
    assert_eq!(zombie, ([23, FLOOR_Y - 3, 0], PathType::Lava), "zombie at the bottom of the pool");
}
