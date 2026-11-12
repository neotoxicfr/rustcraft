//! Players of the local network and dimensions: what happens in a world reaches and touches only
//! the players standing in it (block cracks, explosions' reach, chunks held by a guest), and
//! players whose clients show different worlds neither see each other nor get the other world's
//! events, like the game. Fresh flat world: the 26.3 data cache and the client jar are required
//! (absent: FAILURE).

use crossbeam_channel::{Receiver, Sender};
use rc_client::{EntityEvent, Event};
use rc_proto::varint::read_varint;

use crate::blocks::probe::Probe;
use crate::dims::NETHER;
use crate::other_dimension_send_tests::{new_world, run};

/// Opens the world to the network and adds a joined guest at `pos` whose client holds the chunk it
/// stands in: what it is sent, and the sender that keeps its connection open.
fn add_guest(p: &mut Probe, name: &str, pos: [f64; 3]) -> (Receiver<Vec<u8>>, Sender<Vec<u8>>) {
    crate::lan::announce::LOCAL_ONLY.store(true, std::sync::atomic::Ordering::Relaxed);
    if p.s.lan.is_none() {
        let req = super::Publish { open: true, port: 0, guest_commands: false, force_mode: Some(0) };
        p.s.lan_publish(&req).expect("opening to the network");
    }
    let uuid = rc_client::offline_uuid(name);
    let mut ctx = p.s.new_player_ctx(name, uuid, 2, 0, Some(0));
    ctx.loaded = true;
    ctx.pos = pos;
    ctx.sent.insert(((pos[0].floor() as i32) >> 4, (pos[2].floor() as i32) >> 4));
    let (out, from_server) = crossbeam_channel::unbounded();
    let (keep, inp) = crossbeam_channel::unbounded();
    p.s.lan.as_mut().expect("LAN open").guests.push(super::Guest {
        name: name.into(),
        uuid,
        out,
        inp,
        ctx: Some(Box::new(ctx)),
        enc: Default::default(),
        gone: false,
        joined: true,
        net: Default::default(),
        req_view: 2,
        chunks: Default::default(),
        moves: Default::default(),
    });
    (from_server, keep)
}

fn packet_id(b: &[u8]) -> i32 {
    read_varint(&mut rc_proto::Reader::new(b)).unwrap_or(-1)
}

/// Entities a guest was told about since the last call: (appeared, gone).
fn added_and_removed(guest: &Receiver<Vec<u8>>, ids: &rc_client::ids::Ids) -> (Vec<i32>, Vec<i32>) {
    let (mut added, mut removed) = (Vec::new(), Vec::new());
    for b in guest.try_iter() {
        let mut r = rc_proto::Reader::new(&b);
        let id = read_varint(&mut r).unwrap_or(-1);
        if id == ids.add_entity {
            added.push(read_varint(&mut r).expect("entity id"));
        } else if id == ids.remove_entities {
            let n = read_varint(&mut r).expect("count");
            removed.extend((0..n).map(|_| read_varint(&mut r).expect("entity id")));
        }
    }
    (added, removed)
}

/// Entities the host was told were gone.
fn removed_for_host(events: &[Event]) -> Vec<i32> {
    events
        .iter()
        .filter_map(|e| match e {
            Event::Entity(EntityEvent::Remove(list)) => Some(list.clone()),
            _ => None,
        })
        .flatten()
        .collect()
}

/// A block crack (a door broken in a raid) in a world nobody stands in reaches no player: neither
/// a guest nor the host standing at the same coordinates in the overworld (before: chosen by
/// distance only). In the players' own world, both get it.
#[test]
fn a_block_crack_in_another_world_reaches_no_player() {
    let (mut p, _dir) = new_world("lan-crack");
    let (guest, _keep) = add_guest(&mut p, "Guest", [2.5, -60.0, 0.5]);
    let crack = p.s.peer.ids().block_destruction;
    let _ = p.take_events();
    p.s.in_world(NETHER, |s| s.door_break_progress(77, [2, -60, 0], 3));
    assert!(!guest.try_iter().any(|b| packet_id(&b) == crack), "crack in the Nether sent to the guest");
    let host = p.take_events();
    assert!(!host.iter().any(|e| matches!(e, Event::BlockDestruction { .. })), "crack in the Nether sent to the host");
    p.s.door_break_progress(77, [2, -60, 0], 3);
    assert!(guest.try_iter().any(|b| packet_id(&b) == crack), "crack in the guest's world not sent");
    assert!(p.take_events().iter().any(|e| matches!(e, Event::BlockDestruction { .. })), "crack not sent to the host");
    p.s.lan_close();
}

/// A world the server works in without the players (a parked one) has none of them in it: no
/// guest is a target there (explosions, wind bursts, sweeps), counts among its players, or holds
/// its chunks ticking. Before: every guest counted, at its overworld coordinates.
#[test]
fn guests_are_absent_from_a_world_they_do_not_stand_in() {
    let (mut p, _dir) = new_world("lan-presence");
    let (_guest, _keep) = add_guest(&mut p, "Guest", [480.5, -60.0, 0.5]);
    let guest_chunk = (30, 0);
    let here = (p.s.lan_peers().len(), p.s.world_players().len(), p.s.mob_chunk_ticking(guest_chunk));
    assert_eq!(here, (1, 2, true), "setup: the guest in the players' world");
    let there =
        p.s.in_world(NETHER, |s| (s.lan_peers().len(), s.world_players().len(), s.mob_chunk_ticking(guest_chunk)));
    assert_eq!(there, (0, 1, false), "guest present in the Nether");
    p.s.lan_close();
}

/// When the host goes to the Nether, the guest's client still shows the overworld: they stop
/// seeing each other (the guest is told the host's entity is gone) and what happens in the Nether
/// (a particle) no longer reaches the guest, like the game's per-world tracking; once the host is
/// back, they see each other again. Before: the guest kept the host in sight at its Nether
/// coordinates and got the Nether's events.
#[test]
fn players_whose_clients_show_different_worlds_stay_apart() {
    let (mut p, _dir) = new_world("lan-split");
    let (guest, _keep) = add_guest(&mut p, "Guest", [2.5, -60.0, 0.5]);
    let ids = *p.s.peer.ids();
    let host_id = p.s.player_id;
    p.s.sent.insert((0, 0));
    p.s.lan_broadcast_players();
    assert!(added_and_removed(&guest, &ids).0.contains(&host_id), "setup: the guest sees the host");
    run(&mut p, "execute in minecraft:the_nether run tp @a 0.5 80 0.5");
    assert_eq!(p.s.dim.name, NETHER, "host not in the Nether");
    let _ = (added_and_removed(&guest, &ids), p.take_events());
    p.s.lan_broadcast_players();
    assert!(added_and_removed(&guest, &ids).1.contains(&host_id), "the guest still sees the host");
    let _ = p.take_events();
    run(&mut p, "execute in minecraft:the_nether run particle minecraft:flame 0.5 80 0.5");
    assert!(!guest.try_iter().any(|b| packet_id(&b) == ids.level_particles), "a Nether particle sent to the guest");
    run(&mut p, "execute in minecraft:overworld run tp @a 0.5 -60 0.5");
    p.s.sent.insert((0, 0));
    let _ = added_and_removed(&guest, &ids);
    p.s.lan_broadcast_players();
    assert!(added_and_removed(&guest, &ids).0.contains(&host_id), "the guest no longer sees the host back");
    let guest_id = p.s.lan.as_ref().and_then(|l| l.guests[0].ctx.as_ref()).map(|c| c.player_id);
    assert!(!removed_for_host(&p.take_events()).iter().any(|id| Some(*id) == guest_id), "the host lost the guest");
    p.s.lan_close();
}
