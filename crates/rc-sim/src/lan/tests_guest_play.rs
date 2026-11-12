//! A LAN guest plays in the shared world like any player of the game: it picks up the items and
//! orbs it touches (and the orbs fly to it), it fishes with its own bobber, and the mobs around it
//! are tracked for it by its own range and chunks, whatever the host's position.

use rc_proto::Decode;
use rc_proto::varint::read_varint;

use super::tests_g3f05::{
    FLOOR, G, T, Told, feet, guest_ready, guest_told, host_ready, host_told, join, open, publish, server_tick,
};
use crate::player::Stack;

fn stack(id: &str, count: i32) -> Stack {
    Stack { id: id.into(), count, components: None }
}

/// The first guest's state.
fn guest(t: &T) -> &super::ctx::PlayerCtx {
    t.s.lan.as_ref().unwrap().guests[0].ctx.as_ref().unwrap()
}

fn guest_mut(t: &mut T) -> &mut super::ctx::PlayerCtx {
    t.s.lan.as_mut().unwrap().guests[0].ctx.as_mut().unwrap()
}

/// Items of `id` in a player's slots.
fn held_count(slots: &[Option<Stack>], id: &str) -> i32 {
    slots.iter().flatten().filter(|s| s.id == id).map(|s| s.count).sum()
}

/// A world open to one guest: the host at `host`, the guest at `guest_at`, both ready.
fn host_and_guest(tag: &str, host: [f64; 3], guest_at: [f64; 3]) -> (T, G) {
    let mut t = open(tag);
    publish(&mut t, false);
    let g = join(&mut t, "Guest", 2);
    host_ready(&mut t, host);
    guest_ready(&mut t, guest_at);
    (t, g)
}

// ---------- Pickup ----------

/// The guest takes the item lying at its feet, the host far away takes nothing, and the clients are
/// told the guest collected it. Fails on the code before (only the host picked up, after the
/// walk); sabotage verified (failure): the guest's pickup call removed from its tick.
#[test]
fn guest_picks_up_the_item_it_touches() {
    let at = feet([5, FLOOR + 1, 5]);
    let (mut t, _g) = host_and_guest("guest-item-pickup", feet([-5, FLOOR + 1, -5]), at);
    t.s.spawn_item(at, [0.0; 3], stack("minecraft:diamond", 3), 0);
    let item = t.s.life.drops.last().unwrap().id;
    let _ = super::tests_g3f05::drain(&t);
    server_tick(&mut t);
    assert_eq!(held_count(&guest(&t).slots, "minecraft:diamond"), 3, "the guest holds the diamonds");
    assert_eq!(held_count(&t.s.slots, "minecraft:diamond"), 0, "the host took nothing");
    assert!(t.s.life.drops.iter().all(|d| d.id != item), "the item left the world");
    let collector = guest(&t).player_id;
    let taken = super::tests_g3f05::drain(&t).into_iter().any(|e| {
        matches!(e, rc_client::Event::Misc(rc_client::misc::Misc::Take { item: i, collector: c, amount: 3 })
            if i == item && c == collector)
    });
    assert!(taken, "the host's client is told the guest collected the item");
}

/// An orb three blocks from the guest flies to it (the nearest player within 8 blocks, the host
/// being far) and gives it its experience. Fails on the code before (the orb only knew the host,
/// and only the host picked orbs up); sabotage verified (failure): the guests left out of the
/// orb's followers.
#[test]
fn an_orb_flies_to_the_nearest_guest_and_gives_it_experience() {
    let at = feet([5, FLOOR + 1, 5]);
    let (mut t, _g) = host_and_guest("guest-orb-pickup", feet([-5, FLOOR + 1, -5]), at);
    t.s.add_orb([at[0] - 3.0, at[1], at[2]], 7);
    let mut ticks = 0;
    while guest(&t).vit.xp_total == 0 && ticks < 200 {
        server_tick(&mut t);
        ticks += 1;
    }
    assert_eq!(guest(&t).vit.xp_total, 7, "the guest got the orb's experience ({ticks} ticks)");
    assert_eq!(t.s.life.vit.xp_total, 0, "the host got nothing");
    assert!(t.s.life.orbs.is_empty(), "the orb left the world");
}

// ---------- Fishing ----------

/// Host and guest each cast their own bobber; each bobber reads its own owner (a host without its
/// rod loses its bobber, the guest keeps its own), and each player reels in only its own. Fails on
/// the code before (one bobber per world: the guest's cast reeled the host's in); sabotage
/// verified (failure): the bobbers ticked with the host installed; the bobber found without
/// looking at its owner.
#[test]
fn host_and_guest_fish_with_their_own_bobbers() {
    let (mut t, _g) = host_and_guest("guest-fishing", feet([-3, FLOOR + 1, 0]), feet([3, FLOOR + 1, 0]));
    t.s.slots[36] = Some(stack("minecraft:fishing_rod", 1));
    t.s.held = 0;
    guest_mut(&mut t).slots[36] = Some(stack("minecraft:fishing_rod", 1));
    guest_mut(&mut t).held = 0;
    assert!(t.s.fishing_rod_use(0), "host casts");
    assert!(t.s.as_guest(0, |sv| sv.fishing_rod_use(0)), "guest casts");
    let owners = |t: &T| -> Vec<u128> { t.s.proj.hooks.iter().flatten().filter_map(|h| h.p.owner).collect() };
    let (host_uuid, guest_uuid) = (t.s.uuid, guest(&t).uuid);
    assert_eq!(owners(&t), vec![host_uuid, guest_uuid], "two bobbers, one per player");
    server_tick(&mut t);
    assert_eq!(owners(&t), vec![host_uuid, guest_uuid], "both bobbers tick with their owner");

    // The host puts its rod away: its bobber goes at its next tick, the guest's stays.
    t.s.slots[36] = None;
    server_tick(&mut t);
    assert_eq!(owners(&t), vec![guest_uuid], "only the host's bobber is gone");

    // The guest reels its own in; the host casts again.
    assert!(t.s.as_guest(0, |sv| sv.fishing_rod_use(0)), "guest reels in");
    assert!(t.s.as_guest(0, |sv| sv.own_hook().is_none()), "the guest's bobber is gone");
    t.s.slots[36] = Some(stack("minecraft:fishing_rod", 1));
    assert!(t.s.fishing_rod_use(0), "host casts again");
    server_tick(&mut t);
    assert_eq!(owners(&t), vec![host_uuid], "the host's new bobber alone");
}

// ---------- Mob tracking ----------

fn run(t: &mut T, c: &str) -> bool {
    let mut src = t.s.player_source();
    src.level = 4;
    t.s.perform(src, c).0
}

/// A cow standing still at `at` (no AI): its entity id.
fn still_cow(t: &mut T, at: [f64; 3]) -> i32 {
    assert!(run(t, &format!("summon minecraft:cow {} {} {} {{NoAI:1b}}", at[0], at[1], at[2])), "summon");
    t.s.mobs.list.iter().find(|m| m.kind == crate::mobs::species::Kind::Cow).map(|m| m.id).unwrap()
}

/// Where the guest's client holds `entity` after the packets read since the last call, starting
/// from `held`: the last absolute position (add, position sync) moved by the relative moves since.
fn guest_client_position(t: &T, g: &G, entity: i32, mut held: Option<[f64; 3]>) -> Option<[f64; 3]> {
    let ids = *t.s.peer.ids();
    for p in g.from.try_iter() {
        let mut r = rc_proto::Reader::new(&p);
        let packet = read_varint(&mut r).unwrap();
        if ![ids.add_entity, ids.move_entity_pos, ids.move_entity_pos_rot, ids.entity_position_sync].contains(&packet) {
            continue;
        }
        if read_varint(&mut r).unwrap() != entity {
            continue;
        }
        let vec3 = |r: &mut rc_proto::Reader<'_>| [0; 3].map(|_| f64::decode(r).unwrap());
        if packet == ids.add_entity {
            let _uuid = u128::decode(&mut r).unwrap();
            let _kind = read_varint(&mut r).unwrap();
            held = Some(vec3(&mut r));
        } else if packet == ids.entity_position_sync {
            assert_eq!(read_varint(&mut r).unwrap(), 0, "linear path");
            held = Some(vec3(&mut r));
        } else {
            assert_eq!(read_varint(&mut r).unwrap() >> 1, 0, "one linear step");
            let d = [0; 3].map(|_| f64::from(i16::decode(&mut r).unwrap()) / 4096.0);
            let at = held.expect("a move before the add");
            held = Some([at[0] + d[0], at[1] + d[1], at[2] + d[2]]);
        }
    }
    held
}

/// The guest is given the cow standing by it although the host is far away; the host is not. Then
/// the host comes near and the guest leaves: the host is given the cow, the guest told it is gone.
/// Fails on the code before (mobs paired around the host only, every client told the host's
/// pairings); sabotage verified (failure): the guests' pass skipped; the pairing sent to everyone.
#[test]
fn each_player_is_given_the_mobs_in_its_own_range() {
    let cow_at = feet([5, FLOOR + 1, 5]);
    let far = [-100.5, f64::from(FLOOR + 1), 0.5];
    let (mut t, g) = host_and_guest("tracked-per-player", far, feet([4, FLOOR + 1, 4]));
    guest_mut(&mut t).sent.insert((0, 0));
    let cow = still_cow(&mut t, cow_at);
    let _ = host_told(&t, cow);
    let _ = guest_told(&t, &g, cow);
    server_tick(&mut t);
    assert!(guest_told(&t, &g, cow).contains(&Told::Added), "the guest is given the cow by it");
    assert!(!host_told(&t, cow).contains(&Told::Added), "the host, 100 blocks away, is not");

    host_ready(&mut t, feet([0, FLOOR + 1, 0]));
    guest_mut(&mut t).pos = far;
    server_tick(&mut t);
    assert_eq!(host_told(&t, cow).first(), Some(&Told::Added), "the host came near: given the cow");
    assert!(guest_told(&t, &g, cow).contains(&Told::Removed), "the guest went away: told it is gone");
}

/// A guest paired while the host already holds the mob is given the state the host's client was
/// last sent, so that the moves coded for both land its client where the mob stands. Fails if the
/// guest's add carried the mob's current position (its client then ends half a block off);
/// sabotage verified (failure): the add at the current position.
#[test]
fn a_late_observer_ends_where_the_mob_stands() {
    let cow_at = feet([5, FLOOR + 1, 5]);
    let far = [-100.5, f64::from(FLOOR + 1), 0.5];
    let (mut t, g) = host_and_guest("late-observer", feet([0, FLOOR + 1, 0]), far);
    let cow = still_cow(&mut t, cow_at);
    server_tick(&mut t);
    assert_eq!(host_told(&t, cow).first(), Some(&Told::Added), "the host holds the cow");
    // Half a block east between two sends: the host's client still holds the old position.
    let slot = t.s.mobs.list.iter().position(|m| m.id == cow).unwrap();
    let mut m = t.s.mobs.list.take(slot).unwrap();
    let moved = [m.pos()[0] + 0.5, m.pos()[1], m.pos()[2]];
    m.set_pos(moved);
    t.s.mobs.list.put(slot, m);
    let _ = guest_told(&t, &g, cow);
    // The guest comes near and receives the chunk.
    guest_mut(&mut t).pos = feet([4, FLOOR + 1, 4]);
    guest_mut(&mut t).sent.insert((0, 0));
    let mut held = None;
    for _ in 0..4 {
        server_tick(&mut t);
        held = guest_client_position(&t, &g, cow, held);
    }
    let held = held.expect("the guest was given the cow");
    let at = t.s.mobs.list.iter().find(|m| m.id == cow).unwrap().pos();
    assert!((0..3).all(|i| (held[i] - at[i]).abs() < 1e-3), "guest's client at {held:?}, cow at {at:?}");
}

/// A guest leaving frees its seat: the guests above move down one seat in every mob's tracking.
#[test]
fn freed_seat_moves_the_seats_above_down() {
    assert_eq!(super::without_seat(0b1011, 1), 0b101);
    assert_eq!(super::without_seat(0b1011, 0), 0b101);
    assert_eq!(super::without_seat(0b1011, 3), 0b011);
    assert_eq!(super::without_seat(1 << 31, 31), 0);
    assert_eq!(super::without_seat(u32::MAX, 0), u32::MAX >> 1);
    // The tracker's byte: the host's bit stays, the guests of seats 0 and 2 become seats 0 and 1.
    let mut t = crate::mobs::track::Tracker::default();
    t.set_seen(true);
    t.set_guest(0, true);
    t.set_guest(2, true);
    t.set_guest(6, true);
    t.set_guest(6, false);
    t.guest_seat_freed(1);
    assert!(t.seen() && t.has_guest(0) && t.has_guest(1) && !t.has_guest(2), "{:b}", t.guests());
    assert_eq!(t.guests(), 0b11);
    assert!(t.forget_observers() && !t.paired());
}
