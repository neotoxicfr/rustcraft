//! A2-W1: locator bar waypoints sent by the integrated server: the host and a LAN guest see each
//! other (encoded packet read back by the client's decoder), crouching and leaving end the
//! waypoint, a mob given a transmit range by `/attribute` shows on the host's bar, and
//! `/waypoint list|modify` see every transmitter and reach the network. Before, no waypoint was
//! ever sent and `/attribute` refused the range of a mob.

use rc_client::Event;
use rc_proto::varint::read_varint;
use rc_proto_v777::waypoint::{Waypoint, WaypointId, WaypointOp, WaypointPos};

use super::tests_g3f05::{T, drain, join, open, publish};

const FLOOR: i32 = 199;

fn run(t: &mut T, c: &str) -> (bool, i32) {
    let mut src = t.s.player_source();
    src.level = 4;
    t.s.perform(src, c)
}

/// Waypoint packets a guest received, read back by the client's decoder.
fn guest_waypoints(t: &T, from: &crossbeam_channel::Receiver<Vec<u8>>) -> Vec<Waypoint> {
    let id = t.s.peer.ids().waypoint;
    from.try_iter()
        .filter_map(|p| {
            let mut r = rc_proto::Reader::new(&p);
            (read_varint(&mut r).ok()? == id).then(|| rc_proto_v777::waypoint::read_waypoint(&mut r).expect("waypoint read back"))
        })
        .collect()
}

/// Waypoint events the host received.
fn host_waypoints(t: &T) -> Vec<Waypoint> {
    drain(t).into_iter().filter_map(|e| if let Event::Waypoint(w) = e { Some(*w) } else { None }).collect()
}

fn about(list: &[Waypoint], uuid: u128) -> Vec<(WaypointOp, WaypointPos)> {
    list.iter().filter(|w| w.id == WaypointId::Uuid(uuid)).map(|w| (w.op, w.pos)).collect()
}

fn guest_uuid(t: &T) -> u128 {
    t.s.lan.as_ref().unwrap().guests[0].uuid
}

fn guest_loaded_at(t: &mut T, pos: [f64; 3]) {
    let c = t.s.lan.as_mut().unwrap().guests[0].ctx.as_mut().unwrap();
    c.loaded = true;
    c.teleport = None;
    c.pos = pos;
    c.last_good = pos;
}

/// Host and guest see each other's exact block; the host crouching ends its waypoint for the
/// guest; the guest leaving ends its own for the host. Sabotage verified (failure): crouch
/// modifier left out; the encoder skipping the waypoint event.
#[test]
fn host_and_guest_see_each_other() {
    let mut t = open("waypoint-pair");
    publish(&mut t, false);
    let g = join(&mut t, "Guest", 2);
    guest_loaded_at(&mut t, [0.5, f64::from(FLOOR + 1), 3.5]);
    let _ = guest_waypoints(&t, &g.from);
    let _ = drain(&t);
    t.s.waypoints_sync();
    let host = t.s.uuid;
    let block = |p: [f64; 3]| WaypointPos::Block(crate::mobs::mth::block_of(p));
    assert_eq!(about(&guest_waypoints(&t, &g.from), host), vec![(WaypointOp::Track, block(t.s.pos))], "guest sees the host");
    let guest_pos = t.s.lan.as_ref().unwrap().guests[0].ctx.as_ref().unwrap().pos;
    assert_eq!(about(&host_waypoints(&t), guest_uuid(&t)), vec![(WaypointOp::Track, block(guest_pos))], "host sees the guest");

    t.s.sneaking = true;
    assert_eq!(t.s.player_attribute(crate::waypoints::TRANSMIT_RANGE), 0.0, "crouching hides");
    t.s.waypoints_sync();
    assert_eq!(about(&guest_waypoints(&t, &g.from), host), vec![(WaypointOp::Untrack, WaypointPos::Empty)]);
    t.s.sneaking = false;
    t.s.waypoints_sync();
    assert_eq!(about(&guest_waypoints(&t, &g.from), host), vec![(WaypointOp::Track, block(t.s.pos))], "standing up shows again");

    let guest = guest_uuid(&t);
    t.s.lan_leave(0);
    let _ = drain(&t);
    t.s.waypoints_sync();
    assert_eq!(about(&host_waypoints(&t), guest), vec![(WaypointOp::Untrack, WaypointPos::Empty)], "guest left");
}

/// A mob given a transmit range by `/attribute` (refused before: the mob had no such attribute)
/// shows on the host's bar after its first tick, `/waypoint modify` sends it again in its new
/// colour and saves the icon in its data, and back to zero it is gone. Sabotage verified
/// (failure): the mob attribute not modeled; the icon not saved.
#[test]
fn mob_with_a_range_shows_on_the_host_bar() {
    let mut t = open("waypoint-mob");
    let (ok, _) = run(&mut t, &format!("summon minecraft:cow 6.5 {} 0.5 {{NoAI:1b}}", FLOOR + 1));
    assert!(ok, "summon");
    let (ok, _) = run(&mut t, "attribute @e[type=cow,limit=1] minecraft:waypoint_transmit_range base set 100");
    assert!(ok, "the mob holds the range attribute");
    let cow = t.s.mobs.list.iter().find(|m| m.kind == crate::mobs::species::Kind::Cow).map(|m| (m.id, m.uuid, m.pos())).unwrap();
    t.s.game_tick();
    t.s.tick += 1;
    assert!(t.s.mobs.get(cow.0).is_some_and(|m| m.tick_count > 0), "the cow ticked");
    let _ = drain(&t);
    // Its first tick done: the next pass connects it (a still mob connects when the host moves).
    t.s.waypoints.note_movement_packet(t.s.player_id);
    t.s.waypoints_sync();
    let pos = t.s.mobs.get(cow.0).unwrap().pos();
    let seen = about(&host_waypoints(&t), cow.1);
    assert_eq!(seen, vec![(WaypointOp::Track, WaypointPos::Block(crate::mobs::mth::block_of(pos)))]);

    let (ok, _) = run(&mut t, "waypoint modify @e[type=cow,limit=1] color red");
    assert!(ok, "modify");
    let sent: Vec<Waypoint> = host_waypoints(&t).into_iter().filter(|w| w.id == WaypointId::Uuid(cow.1)).collect();
    assert_eq!(sent.iter().map(|w| w.op).collect::<Vec<_>>(), vec![WaypointOp::Untrack, WaypointOp::Track]);
    assert_eq!(sent[1].color, Some(0xFF_5555), "tracked again in its new colour");
    let saved = t.s.mobs.get(cow.0).unwrap().save();
    let icon = saved.get("locator_bar_icon").expect("icon saved with the mob");
    assert_eq!(icon.get("color").and_then(|c| c.as_i64()), Some(0xFF_5555));

    let (ok, _) = run(&mut t, "attribute @e[type=cow,limit=1] minecraft:waypoint_transmit_range base set 0");
    assert!(ok);
    t.s.waypoints_sync();
    assert_eq!(about(&host_waypoints(&t), cow.1), vec![(WaypointOp::Untrack, WaypointPos::Empty)]);
}

/// A guest's view taken at the end of its own tick serves the pass of that tick only; a view of
/// an earlier tick is never used (the pass reads the guest again). Sabotage verified (failure):
/// tick of the view ignored.
#[test]
fn guest_view_serves_its_own_tick_only() {
    let mut t = open("waypoint-guest-view");
    publish(&mut t, false);
    let g = join(&mut t, "Guest", 2);
    guest_loaded_at(&mut t, [0.5, f64::from(FLOOR + 1), 3.5]);
    t.s.waypoints_sync();
    let guest = guest_uuid(&t);
    let at = |x: f64| [x, f64::from(FLOOR + 1), 3.5];
    let set_guest = |t: &mut T, x: f64| t.s.lan.as_mut().unwrap().guests[0].ctx.as_mut().unwrap().pos = at(x);
    let last_place = |t: &T| about(&host_waypoints(t), guest).last().map(|told| told.1);
    let _ = drain(&t);
    set_guest(&mut t, 1.5);
    t.s.as_guest(0, |sv| sv.note_guest_waypoint_view());
    t.s.waypoints_sync();
    assert_eq!(last_place(&t), Some(WaypointPos::Block([1, FLOOR + 1, 3])), "view of this tick");
    set_guest(&mut t, 2.5);
    t.s.as_guest(0, |sv| sv.note_guest_waypoint_view());
    set_guest(&mut t, 3.5);
    t.s.tick += 1;
    t.s.waypoints_sync();
    assert_eq!(last_place(&t), Some(WaypointPos::Block([3, FLOOR + 1, 3])), "an old view was used");
    let _ = guest_waypoints(&t, &g.from);
}

/// Micro-bench of the pass with LAN guests (`cargo test --release -p rc-sim --lib
/// bench_waypoint_pass_with_guests -- --ignored --nocapture`): 1, 4 and 8 guests walking around
/// the host, 7 alternated passes, median: each guest installed again by the pass, or its view
/// taken at the end of its own tick (taken outside the timing here: its tick installs it anyway);
/// and the two state swaps per guest alone (`as_guest` doing nothing).
#[test]
#[ignore = "micro-bench"]
fn bench_waypoint_pass_with_guests() {
    let median = |v: &mut Vec<f64>| {
        v.sort_by(f64::total_cmp);
        v[v.len() / 2] * 1e6
    };
    for guests in [1usize, 4, 8] {
        let mut t = open(&format!("waypoint-bench-guests-{guests}"));
        publish(&mut t, false);
        let receivers: Vec<_> = (0..guests).map(|k| join(&mut t, &format!("Guest{k}"), 2)).collect();
        for k in 0..guests {
            let c = t.s.lan.as_mut().unwrap().guests[k].ctx.as_mut().unwrap();
            c.loaded = true;
            c.teleport = None;
            c.pos = [k as f64 - 3.5, f64::from(FLOOR + 1), 3.5];
        }
        t.s.waypoints_sync();
        let rounds = 500;
        let walk = |t: &mut T, step: i32| {
            let dx = if step % 2 == 0 { 0.4 } else { -0.4 };
            for g in &mut t.s.lan.as_mut().unwrap().guests {
                g.ctx.as_mut().unwrap().pos[2] += dx;
            }
            t.s.pos[0] += dx;
        };
        let (mut installed, mut from_tick, mut swaps) = (Vec::new(), Vec::new(), Vec::new());
        for _ in 0..7 {
            let start = std::time::Instant::now();
            for step in 0..rounds {
                walk(&mut t, step);
                t.s.waypoints_sync();
            }
            installed.push(start.elapsed().as_secs_f64() / f64::from(rounds));
            let mut timed = std::time::Duration::ZERO;
            for step in 0..rounds {
                walk(&mut t, step);
                for k in 0..guests {
                    t.s.as_guest(k, |sv| sv.note_guest_waypoint_view());
                }
                let start = std::time::Instant::now();
                t.s.waypoints_sync();
                timed += start.elapsed();
            }
            from_tick.push(timed.as_secs_f64() / f64::from(rounds));
            let start = std::time::Instant::now();
            for _ in 0..rounds {
                for k in 0..guests {
                    t.s.as_guest(k, |sv| std::hint::black_box(sv.player_id));
                }
            }
            swaps.push(start.elapsed().as_secs_f64() / f64::from(rounds));
            receivers.iter().for_each(|g| while g.from.try_recv().is_ok() {});
            let _ = drain(&t);
        }
        println!(
            "[bench] waypoint pass with {guests} guests: installed again {:.2} µs, view from their tick {:.2} µs, swaps alone {:.2} µs",
            median(&mut installed),
            median(&mut from_tick),
            median(&mut swaps)
        );
    }
}

/// `/waypoint list` names every transmitter of the world (host, guest, mob), coloured by its own
/// icon; before, only the host. `/waypoint modify @s` saves the icon in the player's file.
/// Sabotage verified (failure): the list limited to the current player.
#[test]
fn waypoint_list_sees_every_transmitter() {
    let mut t = open("waypoint-list");
    publish(&mut t, false);
    let _g = join(&mut t, "Guest", 2);
    guest_loaded_at(&mut t, [0.5, f64::from(FLOOR + 1), 3.5]);
    assert!(run(&mut t, &format!("summon minecraft:pig 4.5 {} 0.5 {{NoAI:1b}}", FLOOR + 1)).0);
    assert!(run(&mut t, "attribute @e[type=pig,limit=1] minecraft:waypoint_transmit_range base set 50").0);
    let (ok, n) = run(&mut t, "waypoint list");
    assert!(ok);
    assert_eq!(n, 3, "host, guest and pig");
    assert!(run(&mut t, "waypoint modify @s color hex 00FF00").0);
    let file = t.s.player_nbt();
    let icon = file.get("locator_bar_icon").expect("icon saved in the player file");
    // The game's opaque value: 0xFF00FF00 as a signed int.
    assert_eq!(icon.get("color").and_then(|c| c.as_i64()), Some(-16_711_936));
    assert_eq!(icon.get("style").and_then(|s| s.as_str()), Some("minecraft:default"));
}

/// `/waypoint modify @s color hex F80`: each digit doubled (orange `FF8800`, not `000F80`),
/// saved opaque, and the guest told the 24 bits. Sabotage verified (failure): the digits read
/// as one number.
#[test]
fn short_hex_colour_doubles_each_digit() {
    let mut t = open("waypoint-short-hex");
    publish(&mut t, false);
    let g = join(&mut t, "Guest", 2);
    guest_loaded_at(&mut t, [0.5, f64::from(FLOOR + 1), 3.5]);
    t.s.waypoints_sync();
    let _ = guest_waypoints(&t, &g.from);
    assert!(run(&mut t, "waypoint modify @s color hex F80").0);
    let file = t.s.player_nbt();
    let saved = file.get("locator_bar_icon").and_then(|i| i.get("color")).and_then(|c| c.as_i64());
    assert_eq!(saved, Some(i64::from(0xFFFF_8800u32 as i32)));
    let host = t.s.uuid;
    let tracked: Vec<Option<u32>> =
        guest_waypoints(&t, &g.from).into_iter().filter(|w| w.id == WaypointId::Uuid(host) && w.op == WaypointOp::Track).map(|w| w.color).collect();
    assert_eq!(tracked, vec![Some(0xFF_8800)]);
}

/// Only the crouching pose hides: shift held while flying, or while spinning with a trident,
/// keeps the player on the bar (the game's pose order). Fails on the code before (the spin
/// attack hid the player); sabotage verified (failure): the crouch read from shift and flying
/// alone.
#[test]
fn only_the_crouching_pose_hides() {
    let mut t = open("waypoint-crouch-pose");
    let range = |t: &T| t.s.player_attribute(crate::waypoints::TRANSMIT_RANGE);
    t.s.sneaking = true;
    assert_eq!(range(&t), 0.0, "crouching");
    t.s.flying = true;
    assert!(range(&t) > 0.0, "shift held while flying");
    t.s.flying = false;
    t.s.proj_player.spin = 10;
    assert!(range(&t) > 0.0, "shift held while spinning");
}

/// `RUSTCRAFT_OPT_WAYPOINT_GUEST_VIEW_FROM_TICK`: over 30 real ticks (world tick then the
/// guests' ticks) with two guests walking, one of them crouching on and off, the snapshot built
/// from the views kept at the end of the guests' ticks equals the one read by installing each
/// guest again (the old path). Sabotage verified (failure): the view kept with the host's seat
/// instead of the guest's.
#[test]
fn guest_view_from_tick_gives_the_same_snapshot() {
    let mut t = open("waypoint-guest-view-equivalence");
    publish(&mut t, false);
    let _a = join(&mut t, "GuestA", 2);
    let _b = join(&mut t, "GuestB", 3);
    for k in 0..2 {
        let c = t.s.lan.as_mut().unwrap().guests[k].ctx.as_mut().unwrap();
        c.loaded = true;
        c.teleport = None;
        c.pos = [k as f64 * 4.0 - 2.0, f64::from(FLOOR + 1), 3.5];
        c.last_good = c.pos;
    }
    let snapshot = |t: &mut T| {
        let mut snap = crate::waypoints::Snapshot::default();
        t.s.fill_waypoint_snapshot(&mut snap);
        format!("{snap:?}")
    };
    for tick in 0..30 {
        for k in 0..2 {
            let c = t.s.lan.as_mut().unwrap().guests[k].ctx.as_mut().unwrap();
            c.pos[0] += if k == 0 { 0.3 } else { -0.2 };
            c.pos[2] += 0.1;
            c.sneaking = k == 1 && tick % 4 < 2;
        }
        t.s.pos[2] += 0.25;
        t.s.game_tick();
        t.s.lan_tick();
        let kept = t.s.waypoints.guest_views.clone();
        assert_eq!(kept.len(), 2, "both guests' views kept at tick {tick}");
        let from_ticks = snapshot(&mut t);
        assert!(t.s.waypoints.guest_views.is_empty(), "views used by the pass");
        let installed_again = snapshot(&mut t);
        assert_eq!(from_ticks, installed_again, "tick {tick}");
        t.s.waypoints.guest_views = kept;
        t.s.waypoints_sync();
        t.s.tick += 1;
    }
}
