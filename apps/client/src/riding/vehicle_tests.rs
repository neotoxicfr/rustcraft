//! Proofs of the vehicle questions: over random event sequences, the rider list and the check
//! order answer like the walk of every entity (player's vehicle, root vehicles, seat roots,
//! piloting), a sabotaged check is caught; a passenger moved to another vehicle leaves the first
//! one; attributes do not outlive their entity across a world change; single precision of the
//! seat scale measured.

use super::*;
use crate::entities::Entities;
use rc_client::EntityEvent;
use rc_proto_v777::entity::AddEntity;

/// Local player (never in the entity table).
const LOCAL: i32 = 7_777;

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

fn add(ents: &mut Entities, id: i32) {
    let add = AddEntity { id, uuid: id as u128, kind: 0, pos: [0.0; 3], velocity: [0.0; 3], pitch: 0.0, yaw: 0.0, head_yaw: 0.0, data: 0 };
    ents.apply(EntityEvent::Add(Box::new(add)));
}

fn world() -> Entities {
    Entities { type_names: vec!["minecraft:pig".into()], local: LOCAL, ..Default::default() }
}

/// One random event of a server: spawn, removal, or passengers set (the local player, known or
/// unknown entities among them).
fn random_event(ents: &mut Entities, rng: &mut Rng) {
    let id = 1 + rng.below(60) as i32;
    match rng.below(10) {
        0 | 1 => add(ents, id),
        2 => ents.apply(EntityEvent::Remove(vec![id])),
        3 => ents.tick(),
        _ => {
            let passengers = (0..rng.below(3))
                .map(|_| match rng.below(6) {
                    0 => LOCAL,
                    1 => 100 + rng.below(5) as i32,
                    _ => 1 + rng.below(60) as i32,
                })
                .filter(|&p| p != id)
                .collect();
            ents.apply(EntityEvent::Passengers { vehicle: id, passengers });
        }
    }
}

/// Root vehicles of the client piloting: those whose id is a multiple of 3 when the local player
/// is their first passenger.
fn authority(ents: &Entities) -> impl Fn(i32) -> bool + '_ {
    move |id| id % 3 == 0 && ents.map.get(&id).is_some_and(|e| e.passengers.first() == Some(&LOCAL))
}

/// Events after which some answer of `paths` differs from the old walk.
fn differing_answers(paths: RidePaths, sabotage: bool) -> (usize, usize) {
    let mut ents = world();
    let mut rng = Rng(0x5eed_1234_abcd_0001);
    let (mut differ, mut piloted) = (0, 0);
    PILOT_CHECK_SABOTAGED.set(sabotage);
    for _ in 0..20_000 {
        random_event(&mut ents, &mut rng);
        let riders: Vec<i32> = ents.map.keys().copied().chain([LOCAL, 100]).collect();
        let mut same = riders.iter().all(|&r| {
            ents.vehicle_carrying(r, paths.rider_list) == ents.vehicle_carrying(r, false)
                && ents.root_vehicle_of(r, paths.rider_list) == ents.root_vehicle_of(r, false)
        });
        let sorted = |mut v: Vec<i32>| {
            v.sort_unstable();
            v
        };
        same &= sorted(ents.seat_roots(paths.rider_list)) == sorted(ents.seat_roots(false));
        for &id in ents.map.keys() {
            let new = piloted_by_player(&ents, id, LOCAL, authority(&ents), paths);
            PILOT_CHECK_SABOTAGED.set(false);
            let old = piloted_by_player(&ents, id, LOCAL, authority(&ents), RidePaths::OLD);
            PILOT_CHECK_SABOTAGED.set(sabotage);
            piloted += usize::from(old);
            same &= new == old;
        }
        differ += usize::from(!same);
    }
    PILOT_CHECK_SABOTAGED.set(false);
    (differ, piloted)
}

#[test]
fn vehicle_answers_match_the_walk_of_every_entity() {
    for paths in [
        RidePaths { rider_list: true, authority_first: false },
        RidePaths { rider_list: false, authority_first: true },
        RidePaths { rider_list: true, authority_first: true },
    ] {
        let (differ, piloted) = differing_answers(paths, false);
        assert_eq!(differ, 0, "{paths:?}");
        assert!(piloted > 100, "{paths:?}: the sequence pilots too rarely ({piloted})");
    }
}

/// Sabotage: piloted when either test passes (`||` for `&&`) is caught.
#[test]
fn sabotaged_pilot_check_is_caught() {
    let (differ, _) = differing_answers(RidePaths { rider_list: true, authority_first: true }, true);
    assert!(differ > 0);
}

/// Unknown passengers are skipped, like the game: an entity that spawns afterwards rides nothing
/// (before: still listed, so a later vehicle made it listed twice).
#[test]
fn unknown_passengers_are_skipped() {
    let mut ents = world();
    add(&mut ents, 1);
    ents.apply(EntityEvent::Passengers { vehicle: 1, passengers: vec![50, LOCAL] });
    assert_eq!(ents.map[&1].passengers, vec![LOCAL]);
    add(&mut ents, 50);
    assert_eq!(ents.vehicle_carrying(50, false), None);
}

/// A passenger announced on another vehicle leaves the first one at once, like the game (before:
/// listed by both until the first vehicle's own update); an unknown vehicle moves nobody.
#[test]
fn moved_passenger_leaves_its_previous_vehicle() {
    let mut ents = world();
    for id in [1, 2, 3] {
        add(&mut ents, id);
    }
    ents.apply(EntityEvent::Passengers { vehicle: 1, passengers: vec![3, LOCAL] });
    ents.apply(EntityEvent::Passengers { vehicle: 2, passengers: vec![3, LOCAL] });
    assert!(ents.map[&1].passengers.is_empty(), "{:?}", ents.map[&1].passengers);
    for from_list in [false, true] {
        assert_eq!(ents.vehicle_carrying(LOCAL, from_list), Some(2));
        assert_eq!(ents.vehicle_carrying(3, from_list), Some(2));
    }
    ents.apply(EntityEvent::Passengers { vehicle: 99, passengers: vec![3, LOCAL] });
    assert_eq!(ents.map[&2].passengers, vec![3, LOCAL]);
}

/// Two worlds in a row, same entity id: the scale received in the first world (3) no longer
/// applies to the entity of the second one, whose packet carries no scale (1, like the game; the
/// old table kept 3 by id). Within one world the values are those received.
#[test]
fn attributes_do_not_outlive_their_entity() {
    let names: Vec<String> = ["minecraft:scale", "minecraft:max_health"].map(String::from).to_vec();
    let attr = |id: i32, base: f64| rc_proto_v777::play::Attribute { id, base, modifiers: Vec::new() };
    let mut ents = world();
    add(&mut ents, 5);
    crate::app_events::store_attributes(ents.map.get_mut(&5).expect("spawned"), &[attr(0, 3.0), attr(1, 30.0)], &names);
    let e = &ents.map[&5];
    assert_eq!((e.attribute_scale(), e.synced_attribute(&names, "minecraft:max_health")), (3.0, Some(30.0)));
    // New world (join or respawn): table cleared, id 5 reused, only the health sent.
    ents.clear();
    add(&mut ents, 5);
    crate::app_events::store_attributes(ents.map.get_mut(&5).expect("spawned"), &[attr(1, 20.0)], &names);
    let e = &ents.map[&5];
    assert_eq!(e.attribute_scale(), 1.0);
    assert_eq!(e.synced_attribute(&names, "minecraft:scale"), None);
    assert_eq!(e.synced_attribute(&names, "minecraft:max_health"), Some(20.0));
}

/// Seats in single precision (scale × age scale, as the game multiplies them) against the old
/// double product: at most a few hundred-millionths of a block on a 2-block seat over every
/// scale the attribute allows, far below what can be seen.
#[test]
fn seat_scale_single_precision_rounding_is_measured() {
    let mut worst = 0.0f64;
    for step in 0..=1_000 {
        let scale = 0.0625 + f64::from(step) * (16.0 - 0.0625) / 1_000.0;
        for (age32, age64) in [(0.5f32, 0.5f64), (0.6, 0.6), (1.0, 1.0)] {
            let single = f64::from(scale as f32 * age32);
            worst = worst.max((single - scale * age64).abs() * 2.0);
        }
    }
    println!("seat height rounding, 2-block seat: {worst:e} blocks at most");
    assert!(worst < 1.0e-5, "{worst}");
}
