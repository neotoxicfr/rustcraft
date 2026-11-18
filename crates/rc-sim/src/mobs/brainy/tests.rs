//! Tests of family F10: brain mechanics (memories, behavior order, activity choice), Manhattan
//! traversal, saving of memories; then end-to-end scenarios on a copy of the test world
//! (`world_tests`).

use super::super::jrand::JRand;
use super::super::species::Kind;
use super::brain::*;

#[test]
fn memory_ttl_like_the_game() {
    // A memory set for 2 ticks lives 3 agings (2, 1, 0 then erased).
    let mut s: Slot<i32> = Slot::default();
    s.set_exp(7, 2);
    s.tick();
    assert_eq!((s.get(), s.ttl), (Some(7), 1));
    s.tick();
    assert_eq!((s.get(), s.ttl), (Some(7), 0));
    s.tick();
    assert_eq!(s.get(), None);
    assert_eq!(s.ttl, NEVER);
    // Without duration: never erased.
    s.set(3);
    for _ in 0..10 {
        s.tick();
    }
    assert_eq!(s.get(), Some(3));
}

#[test]
fn activity_buckets_follow_java_hash_order() {
    // Core (bucket 1) before idle (5), tongue (5), laying (8), swimming (14), long jump (15).
    assert!(Act::Core.bucket() < Act::Idle.bucket());
    assert!(Act::Idle.bucket() == Act::Tongue.bucket());
    assert!(Act::Dig.bucket() < Act::Sniff.bucket());
    assert!(Act::LaySpawn.bucket() < Act::Swim.bucket());
    assert_eq!(Act::LongJump.bucket(), Act::Ram.bucket());
}

#[test]
fn goat_nodes_sorted_by_priority_then_activity() {
    let mut r = JRand::new(1);
    let b = super::goat::brain(&mut r);
    // At priority 0: swimming (core), then gaze and love (idle), jump in progress, ram.
    let p0: Vec<Act> = b.nodes.iter().filter(|n| n.prio == 0).map(|n| n.act).collect();
    assert_eq!(p0, vec![Act::Core, Act::Idle, Act::Idle, Act::LongJump, Act::Ram]);
    // Six sensors, delays drawn in [0, 20).
    assert_eq!(b.sensors.len(), 6);
    assert!(b.sensors.iter().all(|s| (0..20).contains(&s.1)));
}

#[test]
fn first_valid_activity_and_memory_erase() {
    let mut r = JRand::new(5);
    let mut b = super::axolotl::brain(&mut r);
    assert_eq!(b.act, Act::Idle);
    b.mem.attack.set(super::super::mob::Ent::Player);
    super::axolotl::update_activity(&mut b);
    assert_eq!(b.act, Act::Fight);
    // Target lost: back to idle, hunt cooldown of 2,400 ticks.
    b.mem.attack.erase();
    super::axolotl::update_activity(&mut b);
    assert_eq!(b.act, Act::Idle);
    assert_eq!(b.mem.hunting_cd.ttl, 2400);
    // Play dead: never left through this choice; the memory is erased on leaving it.
    b.mem.play_dead.set(200);
    super::axolotl::update_activity(&mut b);
    assert_eq!(b.act, Act::PlayDead);
    b.mem.play_dead.set(0);
    super::axolotl::update_activity(&mut b);
    assert_eq!(b.act, Act::PlayDead);
    b.use_default();
    assert_eq!(b.act, Act::Idle);
    assert!(!b.mem.play_dead.has());
}

#[test]
fn manhattan_order() {
    let mut seen = Vec::new();
    let _ = super::manhattan_find([0, 0, 0], 1, 1, |p| {
        seen.push(p);
        false
    });
    assert_eq!(&seen[..7], &[[0, 0, 0], [-1, 0, 0], [0, -1, 0], [0, 0, 1], [0, 0, -1], [0, 1, 0], [1, 0, 0]]);
    assert_eq!(seen.len(), 27);
}

#[test]
fn memories_round_trip() {
    let mut m = Mem::default();
    m.lj_cd.set(321);
    m.ram_cd.set(4000);
    m.panicking.set(true);
    let n = super::nbt::save_memories(Kind::Goat, &m);
    let mut back = Mem::default();
    super::nbt::load_memories(&mut back, Some(&n));
    assert_eq!(back.lj_cd.get(), Some(321));
    assert_eq!(back.ram_cd.get(), Some(4000));
    assert_eq!(back.panicking.get(), Some(true));
    // Memory with a duration: value and remaining duration.
    let mut a = Mem::default();
    a.danger.set_exp(true, 55);
    let n = super::nbt::save_memories(Kind::Armadillo, &a);
    let mut back = Mem::default();
    super::nbt::load_memories(&mut back, Some(&n));
    assert_eq!((back.danger.get(), back.danger.ttl), (Some(true), 55));
}

#[test]
fn horn_choice_is_stable_per_goat() {
    let u = 0x1234_5678_9abc_def0_0fed_cba9_8765_4321u128;
    let a = super::goat::horn_pick(u, 4);
    assert_eq!(a, super::goat::horn_pick(u, 4));
    assert!(a < 4);
}

#[test]
fn armadillo_states() {
    use super::armadillo::State;
    for (i, n) in ["idle", "rolling", "scared", "unrolling"].iter().enumerate() {
        let s = State::of(i as u8);
        assert_eq!(s.name(), *n);
        assert_eq!(s.id(), i as u8);
    }
    assert!(!State::Idle.threatened() && State::Scared.threatened());
}

#[path = "world_tests.rs"]
mod world_tests;
