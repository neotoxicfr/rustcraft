//! A mob whose state does not change sends no metadata. The common values (shared flags,
//! no gravity, pose) used to be written once by the base and once again by the family: the
//! value flipped twice per tick, stayed marked changed, and the tracker resent it every tick,
//! taking a position and rotation sync along with it (outside the tracking interval).

use rc_client::{EntityEvent, Event};
use rc_proto_v777::entity::Meta;

use super::mob::Mob;
use super::species::Kind;
use super::villager::Npc;
use crate::blocks::probe::{Probe, state_string};

const BED: [i32; 3] = [3, 64, 5];

fn bare_mob(kind: Kind) -> Mob {
    let mut m = Mob::new(kind, 7, 1, [0.5, 64.0, 0.5], 0.0, super::jrand::JRand::new(42));
    m.define_data();
    m
}

fn sleeping_npc() -> Option<Box<Npc>> {
    Some(Box::new(Npc { inv: vec![None; 8], sleeping: Some(BED), ..Npc::default() }))
}

/// A steady state of a family that used to rewrite a common value: the slot it touches and
/// the value the game shows for it.
struct SteadyCase {
    name: &'static str,
    kind: Kind,
    setup: fn(&mut Mob),
    slot: u8,
    expected: Meta,
}

fn case(name: &'static str, kind: Kind, setup: fn(&mut Mob), slot: u8, expected: Meta) -> SteadyCase {
    SteadyCase { name, kind, setup, slot, expected }
}

fn steady_cases() -> Vec<SteadyCase> {
    vec![
        case("swimming drowned", Kind::Drowned, |m| m.aqua.swimming = true, 0, Meta::Byte(0x10)),
        case("sleeping villager", Kind::Villager, |m| m.npc = sleeping_npc(), 6, Meta::Pose(2)),
        case("sleeping trader", Kind::WanderingTrader, |m| m.npc = sleeping_npc(), 6, Meta::Pose(2)),
        case("sitting camel", Kind::Camel, |m| m.mount.camel_sitting_pose = true, 6, Meta::Pose(10)),
        case("crouching rabbit", Kind::Rabbit, |m| m.surf.crouching = true, 6, Meta::Pose(5)),
        case("crouching ocelot", Kind::Ocelot, |m| m.surf.crouching = true, 6, Meta::Pose(5)),
        case("crouching cat", Kind::Cat, |m| m.surf.crouching = true, 6, Meta::Pose(5)),
        // State left by `vex_post_tick` at the end of its first tick.
        case("vex after its first tick", Kind::Vex, |m| m.surf.no_gravity = true, 5, Meta::Bool(true)),
    ]
}

#[test]
fn steady_state_leaves_no_value_marked_changed() {
    // Every case is checked before failing, so that the report names all the broken ones.
    let mut resent = Vec::new();
    for SteadyCase { name, kind, setup, slot, expected } in steady_cases() {
        let mut m = bare_mob(kind);
        setup(&mut m);
        m.sync_data();
        assert_eq!(m.get(slot), Some(&expected), "{name}: value of slot {slot}");
        let first = m.take_dirty();
        assert!(first.iter().any(|(i, _)| *i == slot), "{name}: first change of slot {slot} not sent");
        m.sync_data();
        let again = m.take_dirty();
        if !again.is_empty() {
            resent.push(format!("{name}: {again:?}"));
        }
        assert_eq!(m.get(slot), Some(&expected), "{name}: value of slot {slot} after the second tick");
    }
    assert!(resent.is_empty(), "values resent without a change: {resent:#?}");
}

#[test]
fn sleeping_villager_sends_its_bed_and_wakes_cleanly() {
    let mut m = bare_mob(Kind::Villager);
    m.npc = sleeping_npc();
    m.sync_data();
    assert_eq!(m.get(14), Some(&Meta::BlockPos(Some(BED))));
    m.take_dirty();
    m.npc.as_mut().unwrap().sleeping = None;
    m.sync_data();
    let woken = m.take_dirty();
    assert!(woken.contains(&(6, Meta::Pose(0))) && woken.contains(&(14, Meta::BlockPos(None))), "{woken:?}");
}

/// The game wakes a sleeper as it dies, then sets the dying pose.
#[test]
fn dying_pose_wins_over_sleeping() {
    let mut m = bare_mob(Kind::Villager);
    m.npc = sleeping_npc();
    m.dead = true;
    m.sync_data();
    assert_eq!(m.get(6), Some(&Meta::Pose(7)));
    m.take_dirty();
    m.sync_data();
    assert_eq!(m.take_dirty(), Vec::new());
}

/// Invisibility and swimming together: both bits, still nothing resent.
#[test]
fn invisible_swimming_drowned_keeps_both_bits() {
    let mut m = bare_mob(Kind::Drowned);
    m.aqua.swimming = true;
    m.x.effects.push(super::effects::Effect::new("minecraft:invisibility", 200, 0));
    m.sync_data();
    assert_eq!(m.get(0), Some(&Meta::Byte(0x30)));
    m.take_dirty();
    m.sync_data();
    assert_eq!(m.take_dirty(), Vec::new());
}

// ---------- Through the tracker, on the test world ----------

fn probe(tag: &str) -> Option<Probe> {
    let (world, data) = (crate::tests::test_world(), rc_testdata::version_data());
    let (Some(world), Some(data)) = (world.clone(), data) else {
        eprintln!("test world {world:?} or data missing: test skipped");
        return None;
    };
    let dir = super::tests::prep(&world, tag, 0, "normal", &[]);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run("fill 200 -60 200 231 -60 231 minecraft:stone\nfill 200 -59 200 231 -40 231 minecraft:air\nplayer 216.5 -59 202.5 0 0\n");
    if state_string(&p.s, p.s.get([216, -60, 216])) != "minecraft:stone" {
        eprintln!("zone 200..231 missing from the copy");
        return None;
    }
    p.s.view = 8;
    p.s.sent.insert((216 >> 4, 206 >> 4));
    Some(p)
}

/// Metadata and movement packets of one entity in a batch of events.
fn data_and_moves(events: &[Event], id: i32) -> (usize, usize) {
    let mut counts = (0, 0);
    for e in events {
        match e {
            Event::Entity(EntityEvent::Data { id: i, .. }) if *i == id => counts.0 += 1,
            Event::Entity(EntityEvent::Move(mv)) if mv.id == id => counts.1 += 1,
            _ => {}
        }
    }
    counts
}

/// A slowly moving mob in a steady state: its metadata goes once, then nothing; its position
/// goes only on the tracking interval ticks (every 3), never on the ticks in between.
#[test]
fn steady_mob_sends_moves_only_on_interval_ticks() {
    let Some(mut p) = probe("steady-synced-data") else { return };
    let mut extra_packets = Vec::new();
    for SteadyCase { name, kind, setup, .. } in steady_cases() {
        let mut m = p.s.new_mob(kind, [216.5, -59.0, 206.5], 0.0);
        m.define_data();
        let _ = p.take_events();
        p.s.send_changes(&mut m); // tick 0: pairing
        assert!(m.track.seen(), "{name}: not paired");
        let _ = p.take_events();
        setup(&mut m);
        p.s.send_changes(&mut m); // tick 1: the change itself
        let (data, _) = data_and_moves(&p.take_events(), m.id);
        assert_eq!(data, 1, "{name}: the change is sent once");
        for tick in 2..9 {
            let pos = m.pos();
            m.set_pos([pos[0] + 0.01, pos[1], pos[2]]);
            p.s.send_changes(&mut m);
            let (data, moves) = data_and_moves(&p.take_events(), m.id);
            let expected_moves = usize::from(tick % 3 == 0);
            if data != 0 || moves != expected_moves {
                extra_packets.push(format!("{name}, tick {tick}: {data} metadata, {moves} moves"));
            }
        }
    }
    assert!(extra_packets.is_empty(), "packets outside a change or the interval: {extra_packets:#?}");
}
