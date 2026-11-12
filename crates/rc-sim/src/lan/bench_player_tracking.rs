//! Players among themselves micro-bench (`docs/perf.md`, players among themselves), on the LAN
//! test world, the host and one paired guest:
//! - "pose-update-open-air": the end-of-tick pose of a player in the open (one fit test);
//! - "pose-update-under-ceiling": the same under a 1.5-block ceiling (three fit tests, a forced
//!   crouch);
//! - "pose-update-open-air-200-mobs": the open-air case with 200 ordinary mobs in the world (the
//!   walk looking for solid ones);
//! - "own-data-write": one synced value of the player's own entity written (its own client told at
//!   once, the value kept for the observers);
//! - "tracking-pass-three-writes": three values written then the tracking pass that sends them to
//!   the paired guest, with the packets each observer receives per tick (deterministic count: one
//!   data packet where each write used to make its own, sent to every player paired or not).
//!
//! `bench_mob_tracking_per_player`: the mobs' tracking pass with guests near or far, the orb
//! followers' list, the moves a far guest receives.
//!
//! Nanoseconds and allocations per call (the test allocator's count for this thread). The cases
//! take turns within each round, so that a loaded machine disturbs them alike. Run alone:
//! `cargo test --release -p rc-sim bench_player_tracking -- --ignored --nocapture --test-threads=1`.

use std::time::Instant;

use rc_client::{EntityEvent, Event};
use rc_proto_v777::entity::Meta;

use super::tests_g3f05::{T, drain, join, open, publish};
use crate::bench_kit::{median, report};
use crate::decoration::stand::attribute_may_differ_with;
use crate::test_alloc::allocations;

const BENCH: &str = "player-tracking";
const ROUNDS: usize = 9;
const FLOOR: i32 = 199;

/// One case: its name, its calls per round and what one call does (given the call's number).
type Case<'a> = (&'static str, u32, Box<dyn FnMut(&mut T, u32) + 'a>);

fn report_cases(t: &mut T, mut cases: Vec<Case<'_>>) {
    let mut ns = vec![Vec::new(); cases.len()];
    let mut allocs = vec![Vec::new(); cases.len()];
    for _ in 0..ROUNDS {
        for (k, (_, calls, call)) in cases.iter_mut().enumerate() {
            let _ = drain(t);
            let (start, before) = (Instant::now(), allocations());
            for n in 0..*calls {
                call(t, n);
                // The host's queue is emptied as it goes, never grown by the bench itself.
                if n % 256 == 0 {
                    let _ = drain(t);
                }
            }
            ns[k].push(start.elapsed().as_nanos() as f64 / f64::from(*calls));
            allocs[k].push((allocations() - before) as f64 / f64::from(*calls));
        }
    }
    for (k, (case, ..)) in cases.iter().enumerate() {
        report(BENCH, case, "ns_per_call", median(&ns[k]));
        report(BENCH, case, "ns_per_call_best", ns[k].iter().copied().fold(f64::INFINITY, f64::min));
        report(BENCH, case, "allocs_per_call", median(&allocs[k]));
    }
}

fn write_own_value(t: &T, index: u8, value: Meta) {
    let id = t.s.player_id;
    let _ = t.s.send(Event::Entity(EntityEvent::Data { id, values: vec![(index, value)] }));
}

#[test]
#[ignore]
fn bench_player_tracking() {
    let mut t = open("bench-player-tracking");
    publish(&mut t, false);
    let guest = join(&mut t, "Guest", 2);
    for _ in 0..3 {
        t.s.lan_tick();
    }
    // Packets the guest receives about the host in a tick of three writes.
    let _ = guest.from.try_iter().count();
    for (index, value) in [(1, Meta::Int(250)), (7, Meta::Int(30)), (12, Meta::Int(1))] {
        write_own_value(&t, index, value);
    }
    t.s.lan_tick();
    let data_id = t.s.peer.ids().set_entity_data;
    let host = t.s.player_id;
    let data_packets = guest
        .from
        .try_iter()
        .filter(|p| {
            let mut r = rc_proto::Reader::new(p);
            rc_proto::varint::read_varint(&mut r).ok() == Some(data_id)
                && rc_proto::varint::read_varint(&mut r).ok() == Some(host)
        })
        .count();
    report(BENCH, "tracking-pass-three-writes", "data_packets_per_observer_and_tick", data_packets as f64);

    report_cases(
        &mut t,
        vec![
            ("pose-update-open-air", 200_000, Box::new(|t: &mut T, _| t.s.update_player_pose())),
            (
                "own-data-write",
                400_000,
                Box::new(|t: &mut T, n| write_own_value(t, 1, Meta::Int(200 + (n % 2) as i32))),
            ),
            (
                "tracking-pass-three-writes",
                20_000,
                Box::new(|t: &mut T, n| {
                    let v = (n % 2) as i32;
                    write_own_value(t, 1, Meta::Int(250 + v));
                    write_own_value(t, 7, Meta::Int(30 + v));
                    write_own_value(t, 12, Meta::Int(1 + v));
                    t.s.lan_broadcast_players();
                    let _ = guest.from.try_iter().count();
                }),
            ),
        ],
    );
    let run = |t: &mut T, c: &str| {
        let mut src = t.s.player_source();
        src.level = 4;
        assert!(t.s.perform(src, c).0, "{c}");
    };
    run(&mut t, &format!("setblock 0 {} 0 minecraft:stone_slab[type=top]", FLOOR + 2));
    t.s.update_player_pose();
    assert_eq!(t.s.player_pose(), crate::mobs::PlayerPose::Crouching, "the ceiling forces a crouch");
    report_cases(&mut t, vec![("pose-update-under-ceiling", 200_000, Box::new(|t: &mut T, _| t.s.update_player_pose()))]);

    // The locator bar pass walks the living decorations: what 200 armor stands without any range
    // add to it (the difference between the two cases).
    report_cases(&mut t, vec![("waypoint-pass-no-armor-stand", 50_000, Box::new(|t: &mut T, _| t.s.waypoints_sync()))]);
    for k in 0..200 {
        run(&mut t, &format!("summon minecraft:armor_stand {} {} {} {{NoGravity:1b}}", k % 10, FLOOR + 1, k / 10 - 10));
    }
    report_cases(&mut t, vec![("waypoint-pass-200-armor-stands", 50_000, Box::new(|t: &mut T, _| t.s.waypoints_sync()))]);

    // What each stand costs that pass: the check of its compound, by one walk or two lookups.
    let stand = t.s.deco_living()[0].id;
    let compound = t.s.deco_nbt(stand).expect("the stand's compound").clone();
    let check = |one_walk: bool| {
        let e = std::hint::black_box(&compound);
        std::hint::black_box(attribute_may_differ_with(e, crate::waypoints::TRANSMIT_RANGE, one_walk));
    };
    report_cases(
        &mut t,
        vec![
            ("attribute-check-one-walk", 2_000_000, Box::new(|_: &mut T, _| check(true))),
            ("attribute-check-two-lookups", 2_000_000, Box::new(|_: &mut T, _| check(false))),
        ],
    );

    // The fit test also walks the mobs for solid ones: what 200 ordinary mobs add in the open.
    run(&mut t, &format!("setblock 0 {} 0 minecraft:air", FLOOR + 2));
    for k in 0..200 {
        let at = [f64::from(k % 20) * 2.0 + 4.5, f64::from(FLOOR + 1), f64::from(k / 20) * 2.0 + 4.5];
        let zombie = t.s.new_mob(crate::mobs::species::Kind::Zombie, at, 0.0);
        t.s.add_mob(zombie);
    }
    t.s.update_player_pose();
    assert_eq!(t.s.player_pose(), crate::mobs::PlayerPose::Standing, "open air again");
    report_cases(&mut t, vec![("pose-update-open-air-200-mobs", 200_000, Box::new(|t: &mut T, _| t.s.update_player_pose()))]);
}

/// Places every guest at `pos` with the four chunks of the test world received.
fn guests_at(t: &mut T, pos: [f64; 3]) {
    for g in &mut t.s.lan.as_mut().unwrap().guests {
        let c = g.ctx.as_mut().unwrap();
        c.pos = pos;
        c.sent.extend([(-1, -1), (-1, 0), (0, -1), (0, 0)]);
    }
}

/// Mob tracking per player (`docs/perf.md`, L14-LAN-JEU): one tracking pass over 200 still cows on
/// an open world with 0, 1 and 3 guests, near the cows (each one paired) or 100 blocks away (none);
/// the orb followers' list of 4 players; and the movement packets a far guest receives per pass (a
/// deterministic count: before, every guest was sent the host's pairings and moves).
#[test]
#[ignore]
fn bench_mob_tracking_per_player() {
    let mut t = open("bench-mob-tracking-per-player");
    publish(&mut t, false);
    let run = |t: &mut T, c: &str| {
        let mut src = t.s.player_source();
        src.level = 4;
        assert!(t.s.perform(src, c).0, "{c}");
    };
    for k in 0..200 {
        let (x, z) = (f64::from(k % 13) - 5.5, f64::from(k / 13 % 13) - 5.5);
        run(&mut t, &format!("summon minecraft:cow {x} {} {z} {{NoAI:1b}}", FLOOR + 1));
    }
    t.s.sent.extend([(-1, -1), (-1, 0), (0, -1), (0, 0)]);
    let pass = |t: &mut T, _: u32| t.s.track_mobs();
    report_cases(&mut t, vec![("mob-tracking-pass-200-mobs-no-guest", 2_000, Box::new(pass))]);
    let near = [0.5, f64::from(FLOOR + 1), 0.5];
    let far = [-100.5, f64::from(FLOOR + 1), 0.5];
    let mut guests = vec![join(&mut t, "Guest1", 2)];
    guests_at(&mut t, near);
    report_cases(&mut t, vec![("mob-tracking-pass-200-mobs-1-guest-near", 2_000, Box::new(pass))]);
    guests.push(join(&mut t, "Guest2", 2));
    guests.push(join(&mut t, "Guest3", 2));
    guests_at(&mut t, near);
    report_cases(&mut t, vec![("mob-tracking-pass-200-mobs-3-guests-near", 2_000, Box::new(pass))]);
    guests_at(&mut t, far);
    t.s.track_mobs();
    report_cases(&mut t, vec![("mob-tracking-pass-200-mobs-3-guests-far", 2_000, Box::new(pass))]);

    // Movement packets of the cows a far guest receives in a pass where they all move.
    let ids = *t.s.peer.ids();
    let _ = guests[0].from.try_iter().count();
    for m in t.s.mobs.list.iter_mut() {
        let p = m.pos();
        m.set_pos([p[0], p[1], p[2] + 0.25]);
        m.needs_sync = true;
    }
    t.s.track_mobs();
    let moves = guests[0]
        .from
        .try_iter()
        .filter(|p| {
            let id = rc_proto::varint::read_varint(&mut rc_proto::Reader::new(p)).ok();
            [Some(ids.move_entity_pos), Some(ids.move_entity_pos_rot), Some(ids.entity_position_sync)].contains(&id)
        })
        .count();
    report(BENCH, "mob-tracking-far-guest", "move_packets_per_pass", moves as f64);

    let mut followers = Vec::new();
    report_cases(
        &mut t,
        vec![("orb-followers-host-and-3-guests", 1_000_000, Box::new(move |t: &mut T, _| t.s.orb_followers(&mut followers)))],
    );
}
