//! Players sharing a vehicle (the host and a guest on one camel, in one boat): each keeps its own
//! entry among the passengers, so the clients are told two distinct riders, `execute on passengers`
//! finds each, the one who gets off leaves alone, a two-seat boat stays full, each rider is seated
//! once per tick in its own state, and only the pilot paddles. Before: every player was "the
//! player" in the list. Fresh flat world: the 26.3 data cache and the client jar are required
//! (absent: FAILURE).

use super::tests_commands_reach_guests::{guest, host_and_guest, host_runs, tagged_mob};
use crate::blocks::probe::Probe;
use crate::mobs::mob::Passenger;
use crate::mobs::species::Kind;
use crate::server::Server;

/// The host mounts a camel by `ride`, then the guest: `ride Guest dismount` leaves the host on it
/// (its own state still names the camel). Before: the guest's dismount removed every player entry.
#[test]
fn guest_dismount_leaves_the_host_on_the_camel() {
    let (mut p, _dir, _packets, _keep) = host_and_guest("lan-shared-camel");
    let camel = tagged_mob(&mut p, Kind::Camel, "summon minecraft:camel 0.5 -60 0.5 {NoAI:1b,Tags:[\"mount\"]}");
    assert!(host_runs(&mut p, "ride @s mount @e[tag=mount,limit=1]").0, "the host mounts");
    assert!(host_runs(&mut p, "ride Guest mount @e[tag=mount,limit=1]").0, "the guest mounts");
    let (host, guest_id) = (p.s.player_id, guest(&p, 0).player_id);
    assert_eq!(passenger_ids(&p, camel), [host, guest_id], "two distinct riders in the passengers packet");
    let guest_rides = "execute as @e[tag=mount] on passengers if entity @s[name=Guest]";
    assert_eq!(host_runs(&mut p, guest_rides), (true, 1), "the guest among the camel's passengers");
    assert!(host_runs(&mut p, "ride Guest dismount").0, "the guest gets off");
    assert_eq!(passenger_ids(&p, camel), [host], "the host still rides");
    assert_eq!(
        (p.s.mobs.mounts.riding.vehicle, guest(&p, 0).ride.vehicle),
        (Some(camel), None),
        "vehicles (host, guest)"
    );
    p.s.lan_close();
}

/// The camel's tick seats each of its two riders once, in its own state, on its own seat. Before:
/// the host was seated twice (on the front seat) and the guest never.
#[test]
fn each_rider_of_a_shared_camel_is_seated_on_its_own_seat() {
    let (mut p, _dir, _packets, _keep) = host_and_guest("lan-camel-seats");
    let camel = tagged_mob(&mut p, Kind::Camel, "summon minecraft:camel 0.5 -60 0.5 {NoAI:1b,Tags:[\"mount\"]}");
    assert!(host_runs(&mut p, "ride @s mount @e[tag=mount,limit=1]").0);
    assert!(host_runs(&mut p, "ride Guest mount @e[tag=mount,limit=1]").0);
    settle_teleports(&mut p);
    p.s.mobs.get_mut(camel).expect("camel").set_pos([3.5, -60.0, 0.5]);
    p.s.tick_passengers(camel);
    let v = p.s.mob_ref(camel).expect("camel");
    let (front, back) = (p.s.seat_of(v, 0, None), p.s.seat_of(v, 1, None));
    assert_ne!(front, back, "two seats");
    assert_eq!(p.s.pos, front, "the host on the front seat");
    assert_eq!(guest(&p, 0).pos, back, "the guest on the back seat");
    p.s.lan_close();
}

/// A boat taken by the host then the guest: two distinct riders, no room left, each seated on its
/// own seat by the boat's tick, only the host (its pilot) paddles, and the guest gets off alone,
/// freeing one seat. Before: the second player's entry was the first one's, so the guest's
/// dismount emptied the boat (which then took two more passengers) and the tick seated the host
/// twice.
#[test]
fn host_and_guest_share_a_boat() {
    let (mut p, _dir, _packets, _keep) = host_and_guest("lan-shared-boat");
    assert!(host_runs(&mut p, "summon minecraft:oak_boat 0.5 -60 0.5").0);
    let boat = p.s.carts.list.iter().flatten().last().map(|c| c.id).expect("boat");
    assert!(board(&mut p, None, boat), "the host boards");
    assert!(board(&mut p, Some(0), boat), "the guest boards");
    let (host, guest_id) = (p.s.player_id, guest(&p, 0).player_id);
    let ids = |p: &Probe| p.s.cart_ref(boat).map(|c| p.s.cart_passenger_ids(c)).unwrap_or_default();
    assert_eq!(ids(&p), [host, guest_id], "two distinct riders, the pilot first");
    assert!(!room_in(&p, boat), "both seats taken");
    settle_teleports(&mut p);
    if let Some(c) = p.s.cart_mut(boat) {
        c.pos[0] += 2.0;
    }
    p.s.cart_tick_passengers(boat);
    let c = p.s.cart_ref(boat).expect("boat");
    let (front, back) =
        (Server::cart_seat(c, Passenger::Player(host), None), Server::cart_seat(c, Passenger::Player(guest_id), None));
    assert_ne!(front, back, "two seats");
    assert_eq!(p.s.pos, front, "the host on the front seat");
    assert_eq!(guest(&p, 0).pos, back, "the guest on the back seat");
    p.s.as_player(Some(0), |sv| sv.boat_paddles_from_pilot(true, true));
    assert!(!paddling(&p, boat), "the guest is no pilot: its paddles do nothing");
    p.s.boat_paddles_from_pilot(true, false);
    assert!(paddling(&p, boat), "the host pilots");
    p.s.as_player(Some(0), Server::player_stop_riding_cart);
    assert_eq!(ids(&p), [host], "the host still aboard");
    assert_eq!(p.s.pcart.vehicle, Some(boat));
    assert!(room_in(&p, boat), "one seat free again");
    p.s.lan_close();
}

/// `ride` puts a player in front of a mob already riding the mount, like any mount does, so that
/// it steers. Before: the forced mount put the player last.
#[test]
fn ride_puts_the_player_in_front_of_a_mob_rider() {
    let (mut p, _dir) = crate::other_dimension_send_tests::new_world("ride-in-front");
    let summon =
        "summon minecraft:horse 0.5 -60 0.5 {NoAI:1b,Tags:[\"mount\"],Passengers:[{id:\"minecraft:zombie\",NoAI:1b}]}";
    let horse = tagged_mob(&mut p, Kind::Horse, summon);
    assert!(host_runs(&mut p, "ride @s mount @e[tag=mount,limit=1]").0);
    let first = p.s.mob_ref(horse).and_then(|m| m.mount.passengers.first().copied());
    assert_eq!(first, Some(p.s.player_passenger()), "the player rides in front of the zombie");
    assert_eq!(p.s.mob_ref(horse).map(|m| m.mount.passengers.len()), Some(2));
}

/// A saddled horse steered by the guest turns to the guest's gaze in its tick, run with the host
/// installed (the mob walk). Before: it took the host's gaze.
#[test]
fn a_mount_steered_by_the_guest_turns_to_the_guests_gaze() {
    let (mut p, _dir, _packets, _keep) = host_and_guest("lan-guest-pilot");
    let horse = tagged_mob(&mut p, Kind::Horse, "summon minecraft:horse 6.5 -60 3.5 {Tags:[\"mount\"]}");
    let saddle = crate::player::Stack { id: "minecraft:saddle".into(), count: 1, components: None };
    p.s.mobs.get_mut(horse).expect("horse").equipment[crate::mobs::mounts::SADDLE] = Some(saddle);
    assert!(host_runs(&mut p, "ride Guest mount @e[tag=mount,limit=1]").0);
    p.s.yaw = 0.0;
    if let Some(c) = p.s.lan.as_mut().and_then(|l| l.guests[0].ctx.as_mut()) {
        c.yaw = 90.0;
    }
    let i = p.s.mob_index(horse).expect("horse");
    let mut m = p.s.mobs.list.take(i).expect("horse");
    p.s.tick_mob(&mut m);
    p.s.mobs.list.put(i, m);
    assert_eq!(p.s.mob_ref(horse).map(|m| m.yaw), Some(90.0), "the horse turned to the host's gaze");
    p.s.lan_close();
}

fn passenger_ids(p: &Probe, vehicle: i32) -> Vec<i32> {
    p.s.mob_ref(vehicle).map(|m| p.s.passenger_ids(m)).unwrap_or_default()
}

/// The player of seat `seat` (`None`: the host) boards the boat by the right click's path.
fn board(p: &mut Probe, seat: Option<usize>, boat: i32) -> bool {
    p.s.as_player(seat, |sv| {
        let Some((i, mut c)) = sv.cart_take(boat) else { return false };
        let ok = sv.player_start_riding_cart(&mut c);
        sv.cart_put(i, c);
        ok
    })
}

fn room_in(p: &Probe, boat: i32) -> bool {
    p.s.cart_ref(boat).is_some_and(|c| p.s.vehicle_can_add_passenger(c))
}

fn paddling(p: &Probe, boat: i32) -> bool {
    p.s.cart_ref(boat).is_some_and(|c| c.paddle_data(0))
}

/// The teleports of the mounts acknowledged by both players: their vehicle's tick seats them.
fn settle_teleports(p: &mut Probe) {
    p.s.teleport = None;
    if let Some(c) = p.s.lan.as_mut().and_then(|l| l.guests[0].ctx.as_mut()) {
        c.teleport = None;
    }
}
