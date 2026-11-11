//! G3-F05: LAN guests (section fluids, enabled features, permissions, rules, skin and arms,
//! spectators, bounded view, departure, connection checks, lightning) and hits between
//! players. New world created by the generator: only the version data is required (absent:
//! explicit failure). The guest is plugged in through its channels, without TCP.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crossbeam_channel::{Receiver, Sender};
use rc_client::Event;
use rc_proto::varint::{read_varint, write_varint};

use crate::server::Server;

pub(super) struct T {
    pub(super) s: Server,
    pub(super) session: rc_client::Session,
    dir: std::path::PathBuf,
}

impl Drop for T {
    fn drop(&mut self) {
        self.s.lan_close();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const FLOOR: i32 = 199;

pub(super) fn open(tag: &str) -> T {
    let data = Arc::new(rc_testdata::version_data().expect("version data missing: G3-F05 tests impossible"));
    crate::registries::client_jar(&data).expect("client jar missing: G3-F05 tests impossible");
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-g3f05-{tag}-{}", std::process::id())));
    let mut w = crate::newworld::NewWorld::new(tag);
    w.seed = 7;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    crate::tests::solo_slot();
    let uuid = rc_client::offline_uuid("G3F05");
    let (session, peer) = rc_client::memory::session(data.clone(), uuid).expect("session");
    let opts = crate::Options {
        world: dir.clone(),
        name: "G3F05".into(),
        uuid,
        info: rc_proto_v777::play::ClientInformation { view_distance: 2, ..Default::default() },
        sim_distance: 12,
    };
    let mut s = match Server::open(opts, uuid, data, peer, Arc::new(AtomicBool::new(false))) {
        Ok(s) => s,
        Err((_, e)) => panic!("opening: {e}"),
    };
    s.loaded = true;
    s.teleport = None;
    s.game_mode = 0;
    s.on_ground = true;
    s.load_now(&[(-1, -1), (-1, 0), (0, -1), (0, 0)]);
    let stone = s.default_state("minecraft:stone");
    let air = s.default_state("minecraft:air");
    for x in -6..=6 {
        for z in -6..=6 {
            s.set_block([x, FLOOR, z], stone, 2);
            for y in FLOOR + 1..=FLOOR + 6 {
                s.set_block([x, y, z], air, 2);
            }
        }
    }
    s.pos = [0.5, f64::from(FLOOR + 1), 0.5];
    s.last_good = s.pos;
    s.sent.insert((0, 0));
    // The guests arrive two blocks from the host.
    s.level.spawn_pos = [0, FLOOR + 1, 2];
    crate::lan::announce::LOCAL_ONLY.store(true, std::sync::atomic::Ordering::Relaxed);
    let t = T { s, session, dir };
    drain(&t);
    t
}

pub(super) fn drain(t: &T) -> Vec<Event> {
    let mut out = Vec::new();
    while let Ok(e) = t.session.events.try_recv() {
        out.push(e);
    }
    out
}

pub(super) fn publish(t: &mut T, guest_commands: bool) {
    let req = super::Publish { open: true, port: 0, guest_commands, force_mode: None };
    t.s.lan_publish(&req).expect("opening");
}

/// Plugged-in guest: packets received, sending packets to the server.
pub(super) struct G {
    pub(super) from: Receiver<Vec<u8>>,
    to: Sender<Vec<u8>>,
    id: i32,
}

pub(super) fn join(t: &mut T, name: &str, view: u8) -> G {
    let (out, from) = crossbeam_channel::unbounded();
    let (to, inp) = crossbeam_channel::unbounded();
    let info = rc_proto_v777::play::ClientInformation { view_distance: view, ..Default::default() };
    let j = super::conn::Joined { name: name.into(), uuid: rc_client::offline_uuid(name), properties: Vec::new(), info, out, inp };
    t.s.lan_join(j);
    let l = t.s.lan.as_ref().unwrap();
    let id = l.guests.last().and_then(|g| g.ctx.as_ref()).map(|c| c.player_id).unwrap();
    G { from, to, id }
}

fn guest_packets(g: &G) -> Vec<Vec<u8>> {
    g.from.try_iter().collect()
}

fn id_of(p: &[u8]) -> i32 {
    read_varint(&mut rc_proto::Reader::new(p)).unwrap_or(-1)
}

/// Body of a packet (after the identifier).
fn body(p: &[u8]) -> &[u8] {
    let mut r = rc_proto::Reader::new(p);
    let _ = read_varint(&mut r);
    &p[p.len() - r.remaining()..]
}

fn ids(t: &T) -> rc_client::ids::Ids {
    *t.s.peer.ids()
}

/// Entity events (identifier on 4 bytes, then the event) received by the guest.
fn entity_events(t: &T, ps: &[Vec<u8>]) -> Vec<(i32, i8)> {
    let id = ids(t).entity_event;
    ps.iter()
        .filter(|p| id_of(p) == id)
        .map(|p| {
            let b = body(p);
            (i32::from_be_bytes([b[0], b[1], b[2], b[3]]), b[4] as i8)
        })
        .collect()
}

/// The sections announce their number of fluid states: a section with water no longer announces
/// zero (the game client would not look for swimming or a current there).
#[test]
fn fluids_announced_in_sections() {
    let mut t = open("fluids");
    let water = t.s.default_state("minecraft:water");
    let seagrass = t.s.default_state("minecraft:seagrass");
    for x in 0..3 {
        t.s.set_block([x, FLOOR + 1, 0], water, 2);
    }
    t.s.set_block([4, FLOOR + 1, 0], seagrass, 2);
    let c = t.s.chunks.get(&(0, 0)).and_then(|c| c.as_ref()).expect("chunk");
    let mut packet = t.s.chunk_packet(c);
    // Only the water section, first: its header follows the heightmaps.
    let target = ((FLOOR + 1 - t.s.dim.height.min_y) / 16) as usize;
    packet.sections = vec![packet.sections[target].clone()];
    packet.heightmaps.clear();
    let mut out = Vec::new();
    t.s.encode_event(&mut super::encode::EncState::default(), &Event::Chunk(Box::new(packet)), &mut out);
    let chunk = out.iter().find(|p| id_of(p) == ids(&t).chunk).expect("chunk packet");
    // x, z, no heightmap, size of the sections, then the section header.
    let mut r = rc_proto::Reader::new(body(chunk));
    let _ = r.take(8).unwrap();
    assert_eq!(read_varint(&mut r).unwrap(), 0);
    let _ = read_varint(&mut r).unwrap();
    let non_empty = <i16 as rc_proto::Decode>::decode(&mut r).unwrap();
    let fluids = <i16 as rc_proto::Decode>::decode(&mut r).unwrap();
    // Three water sources and a seagrass (submerged); the ground stone is not one.
    assert_eq!(fluids, 4, "section: {non_empty} non-empty, {fluids} fluids");
}

/// Enabled features of the world (experiments) sent to guests at configuration.
#[test]
fn world_features_at_configuration() {
    let mut t = open("features");
    t.s.features = vec!["minecraft:vanilla".into(), "minecraft:trade_rebalance".into()];
    publish(&mut t, false);
    let sh = &t.s.lan.as_ref().unwrap().shared;
    assert_eq!(sh.features, t.s.features);
    let mut w = Vec::new();
    super::conn::write_features(&mut w, &sh.features);
    let mut r = rc_proto::Reader::new(&w);
    assert_eq!(read_varint(&mut r).unwrap(), 2);
    assert_eq!(rc_proto::read_str(&mut r, 32767).unwrap(), "minecraft:vanilla");
    assert_eq!(rc_proto::read_str(&mut r, 32767).unwrap(), "minecraft:trade_rebalance");
}

/// Guest rights: game master (26) if the world allows commands and the opening grants them;
/// an access change is sent back (event and command tree).
#[test]
fn guest_rights() {
    assert_eq!(super::guest_level(true, true), 2);
    assert_eq!(super::guest_level(false, true), 0);
    assert_eq!(super::guest_level(true, false), 0);
    let mut t = open("rights");
    t.s.level.allow_commands = true;
    publish(&mut t, true);
    let g = join(&mut t, "Guest", 2);
    let ps = guest_packets(&g);
    assert!(entity_events(&t, &ps).contains(&(g.id, 26)), "level 2 announced: {:?}", entity_events(&t, &ps));
    // Access removed by the world options: sent back to the guest.
    *t.s.publish_req.lock().unwrap() = Some(super::Publish { open: true, port: 0, guest_commands: false, force_mode: None });
    t.s.lan_requests();
    let ps = guest_packets(&g);
    assert!(entity_events(&t, &ps).contains(&(g.id, 24)), "level 0 sent back");
    assert!(ps.iter().any(|p| id_of(p) == ids(&t).commands), "command tree sent back");
    assert_eq!(t.s.lan.as_ref().unwrap().guests[0].ctx.as_ref().unwrap().op, 0);
}

/// World open to commands without commands for guests: the guest's level 0 refuses the
/// mode change requested by packet, and, even in creative, the debug stick and the breaking
/// of a command block. The host (level 4) changes mode.
#[test]
fn guest_without_rights_refused() {
    let mut t = open("norights");
    t.s.level.allow_commands = true;
    t.s.cmd.op = 4;
    publish(&mut t, false);
    let _g = join(&mut t, "Guest", 2);
    let gm = ids(&t).change_game_mode;
    let mut pkt = Vec::new();
    write_varint(&mut pkt, gm);
    write_varint(&mut pkt, 1);
    let cb = t.s.default_state("minecraft:command_block");
    let at = [2, FLOOR + 1, 0];
    t.s.set_block(at, cb, 2);
    let (mode, stick, broke) = t.s.as_guest(0, |sv| {
        assert_eq!(sv.cmd.op, 0);
        sv.life_packet(&pkt);
        let mode = sv.game_mode;
        sv.game_mode = 1;
        let slot = 36 + sv.held as usize;
        sv.slots[slot] = Some(crate::player::Stack { id: "minecraft:debug_stick".into(), count: 1, components: None });
        let stick = sv.debug_stick_use(slot, at, true);
        sv.slots[slot] = None;
        let broke = sv.player_destroy_block(at);
        (mode, stick, broke)
    });
    assert_eq!(mode, 0, "mode change refused");
    assert!(!stick, "debug stick refused");
    assert!(!broke, "command block intact");
    assert_eq!(t.s.get(at), cb);
    t.s.life_packet(&pkt);
    assert_eq!(t.s.game_mode, 1, "level 4 host: accepted");
}

/// Rules changed: each player receives the reduced debug event on their own entity;
/// immediate respawn and limited crafting for everyone.
#[test]
fn rules_sent_to_guests() {
    let mut t = open("rules");
    publish(&mut t, false);
    let g = join(&mut t, "Guest", 2);
    let _ = guest_packets(&g);
    drain(&t);
    crate::nbtio::set(&mut t.s.rules, "minecraft:reduced_debug_info", rc_proto::nbt::Nbt::Byte(1));
    t.s.after_rule_change("minecraft:reduced_debug_info");
    crate::nbtio::set(&mut t.s.rules, "minecraft:immediate_respawn", rc_proto::nbt::Nbt::Byte(1));
    t.s.after_rule_change("minecraft:immediate_respawn");
    let ps = guest_packets(&g);
    assert!(entity_events(&t, &ps).contains(&(g.id, 22)), "reduced debug on the guest");
    let ge = ids(&t).game_event;
    assert!(ps.iter().any(|p| id_of(p) == ge && body(p)[0] == 11), "immediate respawn on the guest");
    let host = drain(&t);
    let hid = t.s.player_id;
    assert!(host.iter().any(|e| matches!(e, Event::Entity(rc_client::EntityEvent::Event { id, event: 22 }) if *id == hid)));
}

/// Player pairing: the guest sees the host with their skin parts and main arm;
/// a host turned spectator disappears from the guest's side.
#[test]
fn skin_arm_and_spectator() {
    let mut t = open("skin");
    publish(&mut t, false);
    t.s.model = (0x3f, 0);
    let g = join(&mut t, "Guest", 2);
    // The guest's chunks, then player tracking.
    for _ in 0..3 {
        t.s.lan_tick();
    }
    let ps = guest_packets(&g);
    let hid = t.s.player_id;
    let add = ids(&t).add_entity;
    assert!(ps.iter().any(|p| id_of(p) == add && varint_at(body(p)) == hid), "host added on the guest");
    let data = ids(&t).set_entity_data;
    let meta: Vec<&Vec<u8>> = ps.iter().filter(|p| id_of(p) == data && varint_at(body(p)) == hid).collect();
    assert!(meta.iter().any(|p| p.ends_with(&[16, 0, 0x3f, 0xff])), "host skin parts: {meta:?}");
    // The host sees the guest.
    let host = drain(&t);
    assert!(host.iter().any(|e| matches!(e, Event::Entity(rc_client::EntityEvent::Add(a)) if a.id == g.id)), "guest on the host");
    // Spectator: removed on the guest's side (non-spectator).
    t.s.game_mode = 3;
    t.s.lan_tick();
    let ps = guest_packets(&g);
    let rm = ids(&t).remove_entities;
    assert!(ps.iter().any(|p| id_of(p) == rm && body(p)[1..].starts_with(&varint(hid))), "spectator hidden");
    // Back to survival: added again.
    t.s.game_mode = 0;
    t.s.lan_tick();
    assert!(guest_packets(&g).iter().any(|p| id_of(p) == add && varint_at(body(p)) == hid));
}

/// A spectator carries the "invisible" shared flag, and the pairing data carries the shared flags:
/// a guest turned spectator after the host did is shown the host with that flag (translucent, like
/// the game), although it was not paired when the flag went out. Sabotage verified (failure):
/// shared flags left out of the pairing data; spectator mode ignored by the flag.
#[test]
fn spectator_paired_later_sees_the_invisible_flag() {
    let mut t = open("pair-flags");
    publish(&mut t, false);
    let g = join(&mut t, "Guest", 2);
    for _ in 0..3 {
        t.s.lan_tick();
    }
    // The host turns spectator: its flags go out, and the guest (survival) stops seeing it.
    t.s.set_game_mode(3);
    t.s.vitals_tick();
    t.s.lan_tick();
    let hid = t.s.player_id;
    let removed = ids(&t).remove_entities;
    assert!(guest_packets(&g).iter().any(|p| id_of(p) == removed && body(p)[1..].starts_with(&varint(hid))));
    // The guest turns spectator in turn: the host is paired again, flags first in its data.
    t.s.lan.as_mut().unwrap().guests[0].ctx.as_mut().unwrap().game_mode = 3;
    t.s.lan_tick();
    let ps = guest_packets(&g);
    let (add, data) = (ids(&t).add_entity, ids(&t).set_entity_data);
    assert!(ps.iter().any(|p| id_of(p) == add && varint_at(body(p)) == hid), "host shown to the spectator guest");
    let host_data: Vec<&[u8]> = ps
        .iter()
        .filter(|p| id_of(p) == data && varint_at(body(p)) == hid)
        .map(|p| &body(p)[varint(hid).len()..])
        .collect();
    // Data 0, serializer 0 (byte), invisible alone.
    assert!(host_data.iter().any(|values| values.starts_with(&[0, 0, 0x20])), "invisible flag at pairing: {host_data:?}");
}

fn varint(v: i32) -> Vec<u8> {
    let mut w = Vec::new();
    write_varint(&mut w, v);
    w
}

fn varint_at(b: &[u8]) -> i32 {
    read_varint(&mut rc_proto::Reader::new(b)).unwrap_or(-1)
}

/// Guest view bounded by the host's (the server's); a host that changes view announces it
/// to the guests and recomputes theirs.
#[test]
fn view_bounded_by_host() {
    let mut t = open("view");
    publish(&mut t, false);
    let g = join(&mut t, "Guest", 10);
    assert_eq!(t.s.lan.as_ref().unwrap().guests[0].ctx.as_ref().unwrap().view, 2);
    let _ = guest_packets(&g);
    t.s.handle(crate::input::Input::ClientInfo { view: 4, skin_parts: 0, main_hand: 1 });
    let ps = guest_packets(&g);
    let radius = ids(&t).chunk_radius;
    assert!(ps.iter().any(|p| id_of(p) == radius && varint_at(body(p)) == 4), "server view announced");
    assert_eq!(t.s.lan.as_ref().unwrap().guests[0].ctx.as_ref().unwrap().view, 4);
}

/// Guest departure: their vehicle and their ender pearls leave the world with them.
#[test]
fn departure_vehicle_and_pearls() {
    let mut t = open("departure");
    publish(&mut t, false);
    let g = join(&mut t, "Guest", 2);
    let uuid = rc_client::offline_uuid("Guest");
    let horse_id = t.s.as_guest(0, |sv| {
        let mut h = sv.new_mob(crate::mobs::species::Kind::Horse, sv.pos, 0.0);
        assert!(sv.player_start_riding(&mut h), "mounting");
        let id = h.id;
        sv.add_mob(h);
        id
    });
    let pearl = rc_proto::nbt::Nbt::Compound(vec![
        ("id".into(), rc_proto::nbt::Nbt::String("minecraft:ender_pearl".into())),
        ("Pos".into(), rc_proto::nbt::Nbt::List(vec![rc_proto::nbt::Nbt::Double(3.0), rc_proto::nbt::Nbt::Double(f64::from(FLOOR + 3)), rc_proto::nbt::Nbt::Double(3.0)])),
        ("Owner".into(), crate::nbtio::uuid_nbt(uuid)),
    ]);
    let p = t.s.proj_from_nbt(&pearl).expect("pearl");
    t.s.proj_add(p);
    drop(g);
    t.s.lan_leave(0);
    assert!(t.s.mobs.get(horse_id).is_none_or(|m| m.removed), "vehicle removed");
    assert!(
        t.s.proj.list.iter().filter(|p| p.owner == Some(uuid)).all(|p| p.removed),
        "pearls removed"
    );
}

/// L9-TICK: a guest's ender pearl carries its ticket like the host's (placed where it enters a
/// chunk), and vanishes when its guest is dead (rule `ender_pearls_vanish_on_death`). Before:
/// a parked world only stayed active for the host's pearls, and a dead guest's pearl flew on.
#[test]
fn guest_pearl_ticket_and_vanishing() {
    let mut t = open("guest-pearl");
    publish(&mut t, false);
    let _g = join(&mut t, "Guest", 2);
    let uuid = rc_client::offline_uuid("Guest");
    let pearl = |x: f64| {
        use rc_proto::nbt::Nbt;
        let list = |v: [f64; 3]| Nbt::List(v.map(Nbt::Double).to_vec());
        Nbt::Compound(vec![
            ("id".into(), Nbt::String("minecraft:ender_pearl".into())),
            ("Pos".into(), list([x, f64::from(FLOOR + 3), 3.5])),
            ("Motion".into(), list([1.0, 0.0, 0.0])),
            ("NoGravity".into(), Nbt::Byte(1)),
            ("Owner".into(), crate::nbtio::uuid_nbt(uuid)),
        ])
    };
    let p = t.s.proj_from_nbt(&pearl(-0.5)).expect("pearl");
    t.s.proj_add(p);
    t.s.game_tick();
    t.s.tick += 1;
    assert_eq!(t.s.proj.tickets.iter().map(|k| k.chunk).collect::<Vec<_>>(), vec![(0, 0)], "guest pearl ticket");
    // Its guest dead: the pearl vanishes at its next tick.
    if let Some(c) = t.s.lan.as_mut().and_then(|l| l.guests[0].ctx.as_mut()) {
        c.vit.dead = true;
    }
    t.s.game_tick();
    assert!(t.s.proj.list.iter().filter(|p| p.owner == Some(uuid)).all(|p| p.removed), "pearl of a dead guest");
}

/// Shared vehicle (host and guest on the same camel): the guest's file does not carry
/// the vehicle (`RootVehicle` absent) and the camel stays in the world at their departure.
#[test]
fn departure_shared_vehicle() {
    let mut t = open("shared");
    publish(&mut t, false);
    let g = join(&mut t, "Guest", 2);
    let mut c = t.s.new_mob(crate::mobs::species::Kind::Camel, t.s.pos, 0.0);
    assert!(t.s.player_start_riding(&mut c), "host mounted");
    let cid = c.id;
    t.s.add_mob(c);
    let saved = t.s.as_guest(0, |sv| {
        let i = sv.mob_index(cid).expect("camel");
        let mut m = sv.mobs.list.take(i).expect("camel");
        let ok = sv.player_start_riding(&mut m);
        sv.mobs.list.put(i, m);
        assert!(ok, "guest mounted");
        sv.player_nbt()
    });
    assert!(saved.get("RootVehicle").is_none(), "shared vehicle: not in the file");
    drop(g);
    t.s.lan_leave(0);
    let camel = t.s.mobs.get(cid).expect("camel");
    assert!(!camel.removed, "camel stayed with the host");
    assert_eq!(camel.mount.passengers.len(), 1, "only the guest gets off");
    // Only player on it: the host's file carries it.
    assert!(t.s.player_nbt().get("RootVehicle").is_some(), "only player: vehicle written");
}

/// Speed check: a guest who jumps more than 10 blocks in one packet is reframed;
/// the world owner is not.
#[test]
fn guest_speed_checked() {
    let mut t = open("speed");
    publish(&mut t, false);
    let _g = join(&mut t, "Guest", 2);
    let (start, after) = t.s.as_guest(0, |sv| {
        sv.loaded = true;
        sv.teleport = None;
        sv.on_ground = true;
        let start = sv.pos;
        sv.on_move(Some([start[0] + 8.0, start[1], start[2] + 8.0]), None, true);
        (start, sv.pos)
    });
    assert_eq!(after, start, "guest reframed");
    let start = t.s.pos;
    t.s.on_move(Some([start[0] + 3.0, start[1], start[2] + 3.0]), None, true);
    assert_ne!(t.s.pos, start, "host never checked");
}

/// Guest passenger moved far by their vehicle (teleported): their rotation packets are neither
/// counted nor checked, they are not reframed.
#[test]
fn passenger_speed_not_checked() {
    let mut t = open("passenger");
    publish(&mut t, false);
    let _g = join(&mut t, "Guest", 2);
    let (teleport, counted) = t.s.as_guest(0, |sv| {
        sv.loaded = true;
        sv.teleport = None;
        let mut h = sv.new_mob(crate::mobs::species::Kind::Horse, sv.pos, 0.0);
        assert!(sv.player_start_riding(&mut h), "mounting");
        sv.add_mob(h);
        sv.teleport = None;
        sv.pos[0] += 50.0;
        sv.on_move(None, Some((10.0, 0.0)), true);
        (sv.teleport, sv.lan.as_ref().unwrap().guests[0].moves.1)
    });
    assert!(teleport.is_none(), "passenger never reframed");
    assert_eq!(counted, 0, "passenger packet not counted");
}

/// Minecart ridden by a guest: its tick seats the guest, never the host (before: the host's
/// position followed the seat).
#[test]
fn guest_minecart_ticks_guest() {
    let mut t = open("minecart");
    publish(&mut t, false);
    let _g = join(&mut t, "Guest", 2);
    t.s.next_entity += 1;
    let mut c = crate::minecart::Cart::new(crate::minecart::CKind::Rideable, t.s.next_entity, [3.5, f64::from(FLOOR + 1), 3.5]);
    let cid = c.id;
    t.s.as_guest(0, |sv| {
        sv.loaded = true;
        sv.teleport = None;
        assert!(sv.player_start_riding_cart(&mut c), "guest mounted");
    });
    t.s.cart_add(c);
    let host = t.s.pos;
    t.s.minecarts_tick();
    assert_eq!(t.s.pos, host, "host immobile");
    let (guest_pos, guest_cart) = {
        let g = t.s.lan.as_ref().unwrap().guests[0].ctx.as_ref().unwrap();
        (g.pos, g.pcart.vehicle)
    };
    assert_eq!(guest_cart, Some(cid));
    let seat = t.s.cart_ref(cid).map(|c| crate::server::Server::cart_seat(c, None)).unwrap();
    assert_eq!(guest_pos, seat, "guest seated");
}

/// L10-ORDER: a guest's respawn renews its own rank in the world's entity list, not the host's
/// (the rank is part of each player's state).
#[test]
fn guest_respawn_keeps_the_host_rank() {
    let mut t = open("respawn-rank");
    publish(&mut t, false);
    let _g = join(&mut t, "Guest", 2);
    let host = t.s.walk.player;
    let before = t.s.lan.as_ref().unwrap().guests[0].ctx.as_ref().unwrap().walk_rank;
    t.s.as_guest(0, |sv| {
        sv.life.vit.dead = true;
        sv.perform_respawn();
    });
    assert_eq!(t.s.walk.player, host, "host rank unchanged");
    let after = t.s.lan.as_ref().unwrap().guests[0].ctx.as_ref().unwrap().walk_rank;
    assert!(after > before, "guest rank renewed: {before} -> {after}");
}

/// Echo (pong) answered to the guest; chunks sent in batches bounded by its acknowledgements.
#[test]
fn echo_and_chunk_throughput() {
    let mut t = open("throughput");
    // Chunks of the guest's view already there: none stays in generation.
    let near: Vec<(i32, i32)> = (-3..=3).flat_map(|x| (-3..=3).map(move |z| (x, z))).collect();
    t.s.load_now(&near);
    publish(&mut t, false);
    let g = join(&mut t, "Guest", 2);
    let ids = ids(&t);
    let mut ping = varint(ids.ping_request);
    ping.extend_from_slice(&1234i64.to_be_bytes());
    g.to.send(ping).unwrap();
    t.s.lan_poll();
    let ps = guest_packets(&g);
    let pong = ps.iter().find(|p| id_of(p) == ids.pong_response).expect("echo");
    assert_eq!(body(pong), 1234i64.to_be_bytes());
    let batch = |ps: &[Vec<u8>]| -> Vec<i32> {
        ps.iter().filter(|p| id_of(p) == ids.chunk_batch_finished).map(|p| varint_at(body(p))).collect()
    };
    t.s.lan_tick();
    let first = batch(&guest_packets(&g));
    assert_eq!(first.len(), 1, "one batch");
    assert!(first[0] > 1 && first[0] <= 9, "starting batch: {first:?}");
    t.s.lan_tick();
    assert!(batch(&guest_packets(&g)).is_empty(), "no second batch without acknowledgement");
    let mut ack = varint(ids.chunk_batch_received);
    ack.extend_from_slice(&2.0f32.to_be_bytes());
    g.to.send(ack).unwrap();
    t.s.lan_poll();
    t.s.lan_tick();
    assert_eq!(batch(&guest_packets(&g)), [2], "acknowledged throughput");
}

/// Lightning: a guest under the sky in the targeted column is a candidate.
#[test]
fn lightning_targets_guests() {
    let mut t = open("lightning");
    publish(&mut t, false);
    let _g = join(&mut t, "Guest", 2);
    let gpos = [20.5, f64::from(FLOOR + 1), 20.5];
    t.s.lan.as_mut().unwrap().guests[0].ctx.as_mut().unwrap().pos = gpos;
    t.s.pos = [-40.5, f64::from(FLOOR + 1), -40.5];
    let target = t.s.lightning_target_around(21, 21);
    assert_eq!(target, [20, FLOOR + 1, 20]);
}

/// Tracking range: a mounted player raises that of their mount (32 chunks).
#[test]
fn passenger_range() {
    let mut t = open("range");
    let mut h = t.s.new_mob(crate::mobs::species::Kind::Horse, t.s.pos, 0.0);
    let alone = t.s.effective_tracking(&h);
    assert!(alone < 32);
    assert!(t.s.player_start_riding(&mut h));
    assert_eq!(t.s.effective_tracking(&h), 32);
}

/// Arrival announced with the displayed name (hover, insertion, click), not the raw name.
#[test]
fn arrival_with_display_name() {
    let mut t = open("arrival");
    publish(&mut t, false);
    let _g = join(&mut t, "Guest", 2);
    let host = drain(&t);
    let joined = host
        .iter()
        .find_map(|e| match e {
            Event::Chat { component, .. } => match &component.content {
                rc_text::Content::Translate { key, with, .. } if key == "multiplayer.player.joined" => Some(with.clone()),
                _ => None,
            },
            _ => None,
        })
        .expect("arrival announced");
    let rc_text::Arg::Component(c) = &joined[0] else { panic!("raw name: {joined:?}") };
    assert_eq!(c.style.insertion.as_deref(), Some("Guest"));
    assert!(c.style.hover.is_some());
}

/// Hits between players: the host hits the guest (`pvp` rule allowed); forbidden by the
/// rule or by a common team without friendly fire.
#[test]
fn hits_between_players() {
    let mut t = open("pvp");
    publish(&mut t, false);
    let g = join(&mut t, "Guest", 2);
    {
        let c = t.s.lan.as_mut().unwrap().guests[0].ctx.as_mut().unwrap();
        c.loaded = true;
        c.teleport = None;
        c.game_mode = 0;
    }
    let health = |t: &T| t.s.lan.as_ref().unwrap().guests[0].ctx.as_ref().unwrap().vit.health;
    let h0 = health(&t);
    t.s.mobs.attack_ticker = 100;
    t.s.player_attack(g.id);
    let h1 = health(&t);
    assert!(h1 < h0, "hit landed: {h0} → {h1}");
    // `pvp` rule off: no more damage.
    crate::nbtio::set(&mut t.s.rules, "minecraft:pvp", rc_proto::nbt::Nbt::Byte(0));
    t.s.lan.as_mut().unwrap().guests[0].ctx.as_mut().unwrap().vit.cooldown = 0;
    t.s.mobs.attack_ticker = 100;
    t.s.player_attack(g.id);
    assert_eq!(health(&t), h1, "pvp forbidden");
    assert_eq!(t.s.stat_value("minecraft:custom", "minecraft:damage_dealt"), ((h0 - h1) * 10.0).round() as i32, "hit refused: nothing more");
}

/// G5-F04: a mace smash with Wind Burst on another player runs the weapon's after-hit effect
/// after the smash: the attacker is launched, its fall reset.
#[test]
fn mace_wind_burst_between_players() {
    let mut t = open("pvpmace");
    publish(&mut t, false);
    let g = join(&mut t, "Guest", 2);
    {
        let c = t.s.lan.as_mut().unwrap().guests[0].ctx.as_mut().unwrap();
        c.loaded = true;
        c.teleport = None;
        c.game_mode = 0;
    }
    let health = |t: &T| t.s.lan.as_ref().unwrap().guests[0].ctx.as_ref().unwrap().vit.health;
    let h0 = health(&t);
    let mut mace = crate::player::Stack { id: "minecraft:mace".into(), count: 1, components: None };
    let fields = vec![("minecraft:wind_burst".to_owned(), rc_proto::nbt::Nbt::Int(1))];
    crate::items::set_component(&mut mace, "minecraft:enchantments", rc_proto::nbt::Nbt::Compound(fields.into_iter().collect()));
    let slot = 36 + t.s.held as usize;
    t.s.slots[slot] = Some(mace);
    t.s.equipment_tick();
    t.s.on_ground = false;
    t.s.life.vit.fall = 4.0;
    t.s.mobs.player_vel = [0.0; 3];
    t.s.mobs.attack_ticker = 100;
    t.s.player_attack(g.id);
    assert!(health(&t) < h0, "hit landed");
    assert!(t.s.mobs.player_vel[1] > 0.5, "attacker not launched: {:?}", t.s.mobs.player_vel);
    assert_eq!(t.s.life.vit.fall, 0.0, "fall not reset");
}

/// L9-COMBAT: the smash knockback reaches another player standing near the target (pushed away
/// from it and 0.7 upwards, its velocity sent to it), and a Wind Burst sends that player an
/// explosion packet carrying its own knockback (before, only the current player was reached and
/// its knockback went to everyone).
#[test]
fn smash_and_wind_burst_reach_other_player() {
    let mut t = open("pvpreach");
    publish(&mut t, false);
    let g = join(&mut t, "Guest", 2);
    fn guest(t: &mut T) -> &mut crate::lan::ctx::PlayerCtx {
        t.s.lan.as_mut().unwrap().guests[0].ctx.as_mut().unwrap()
    }
    {
        let c = guest(&mut t);
        c.loaded = true;
        c.teleport = None;
        c.game_mode = 0;
        c.pos = [0.5, f64::from(FLOOR + 1), 3.5];
        c.player_vel = [0.0; 3];
    }
    let mut z = t.s.new_mob(crate::mobs::species::Kind::Zombie, [0.5, f64::from(FLOOR + 1), 1.5], 180.0);
    z.attrs.max_health.base = 500.0;
    z.health = 500.0;
    let slot = 36 + t.s.held as usize;
    t.s.slots[slot] = Some(crate::player::Stack { id: "minecraft:mace".into(), count: 1, components: None });
    t.s.equipment_tick();
    t.s.on_ground = false;
    t.s.life.vit.fall = 4.0;
    t.s.mobs.player_vel = [0.0; 3];
    t.s.mobs.attack_ticker = 100;
    let _ = guest_packets(&g);
    t.s.player_attack_mob(&mut z);
    let v = guest(&mut t).player_vel;
    assert_eq!(v[1], f64::from(0.7f32), "other player not lifted: {v:?}");
    assert!(v[2] > 0.0, "other player not pushed away: {v:?}");
    let motion = ids(&t).set_entity_motion;
    assert!(guest_packets(&g).iter().any(|p| id_of(p) == motion), "velocity not sent to the other player");
    // Wind Burst centred between both players: each packet carries its own knockback.
    guest(&mut t).player_vel = [0.0; 3];
    t.s.wind_burst_with(None, None, [0.5, f64::from(FLOOR + 1) + 0.5, 2.5], 3.5, 1.2, "minecraft:entity.wind_charge.wind_burst", &mut []);
    let v = guest(&mut t).player_vel;
    assert!(v[2] > 0.0, "other player not pushed by the burst: {v:?}");
    let explode = ids(&t).explode;
    let ps = guest_packets(&g);
    let p = ps.iter().find(|p| id_of(p) == explode).expect("explosion packet to the other player");
    // Centre (3 doubles), radius (float), block count (int), then the knockback flag and vector.
    let b = body(p);
    assert_eq!(b[32], 1, "no knockback in the other player's packet");
    let kz = f64::from_be_bytes(b[49..57].try_into().unwrap());
    assert!((kz - v[2]).abs() < 1e-12, "the other player received another knockback: {kz} vs {}", v[2]);
}

/// L10-COMBAT: an ordinary explosion next to another player hurts and pushes that player like the
/// host (its server velocity, and its own packet's knockback); the host, far away, is untouched.
/// Before, only the current player was reached.
#[test]
fn ordinary_explosion_hurts_and_pushes_other_player() {
    let mut t = open("blastpeer");
    publish(&mut t, false);
    let g = join(&mut t, "Guest", 2);
    host_ready(&mut t, feet([12, FLOOR + 1, 12]));
    guest_ready(&mut t, [0.5, f64::from(FLOOR + 1), 2.5]);
    t.s.lan.as_mut().unwrap().guests[0].ctx.as_mut().unwrap().player_vel = [0.0; 3];
    let _ = guest_packets(&g);
    let center = [0.5, f64::from(FLOOR + 1), 0.5];
    t.s.explode_with(None, None, None, center, 2.0, false, "minecraft:explosion", crate::mobs::explode::Blast::None(false));
    assert_eq!(t.s.life.vit.health, 20.0, "the host, out of range, was hurt");
    let (h, _) = guest_vitals(&t);
    assert!(h < 20.0, "other player not hurt: {h}");
    let v = t.s.lan.as_ref().unwrap().guests[0].ctx.as_ref().unwrap().player_vel;
    assert!(v[2] > 0.0, "other player not pushed: {v:?}");
    let explode = ids(&t).explode;
    let ps = guest_packets(&g);
    let p = ps.iter().find(|p| id_of(p) == explode).expect("explosion packet to the other player");
    let b = body(p);
    assert_eq!(b[32], 1, "no knockback in the other player's packet");
    let kz = f64::from_be_bytes(b[49..57].try_into().unwrap());
    assert!((kz - v[2]).abs() < 1e-12, "packet knockback {kz} vs server velocity {}", v[2]);
}

/// L10-COMBAT: an explosion whose author is the host (its primed TNT) does not hurt another player
/// while `pvp` is off, but pushes it all the same, like the game; allowed, it names the host as the
/// attacker: the other player killed by it counts `killed_by` player, the host a player kill.
#[test]
fn player_explosion_follows_pvp_and_credits_author() {
    use crate::mobs::explode::Blast;
    use crate::mobs::mob::Ent;
    let mut t = open("blastpvp");
    publish(&mut t, false);
    let _g = join(&mut t, "Guest", 2);
    host_ready(&mut t, feet([12, FLOOR + 1, 12]));
    guest_ready(&mut t, [0.5, f64::from(FLOOR + 1), 2.5]);
    let center = [0.5, f64::from(FLOOR + 1), 0.5];
    let blast = |t: &mut T| t.s.explode_with(None, Some(Ent::Player), None, center, 2.0, false, "minecraft:player_explosion", Blast::None(false));
    crate::nbtio::set(&mut t.s.rules, "minecraft:pvp", rc_proto::nbt::Nbt::Byte(0));
    t.s.lan.as_mut().unwrap().guests[0].ctx.as_mut().unwrap().player_vel = [0.0; 3];
    blast(&mut t);
    assert_eq!(guest_vitals(&t).0, 20.0, "hurt by the host's explosion with pvp off");
    let v = t.s.lan.as_ref().unwrap().guests[0].ctx.as_ref().unwrap().player_vel;
    assert!(v[2] > 0.0, "refused hurt, but not pushed: {v:?}");
    crate::nbtio::set(&mut t.s.rules, "minecraft:pvp", rc_proto::nbt::Nbt::Byte(1));
    t.s.lan.as_mut().unwrap().guests[0].ctx.as_mut().unwrap().vit.health = 1.0;
    blast(&mut t);
    assert!(t.s.lan.as_ref().unwrap().guests[0].ctx.as_ref().unwrap().vit.dead, "other player not killed");
    assert_eq!(t.s.stat_value("minecraft:custom", "minecraft:player_kills"), 1, "host not credited");
    let killed_by = t.s.as_guest(0, |sv| sv.stat_value("minecraft:killed_by", "minecraft:player"));
    assert_eq!(killed_by, 1, "killed by a player");
}

/// L10-COMBAT: the sweep of a sword hurts another player standing next to the target (not an
/// ally). Before, only mobs were swept.
#[test]
fn sweep_reaches_other_player() {
    let mut t = open("sweeppeer");
    publish(&mut t, false);
    let _g = join(&mut t, "Guest", 2);
    host_ready(&mut t, [0.5, f64::from(FLOOR + 1), 0.5]);
    guest_ready(&mut t, [1.3, f64::from(FLOOR + 1), 1.5]);
    let slot = 36 + t.s.held as usize;
    t.s.slots[slot] = Some(crate::player::Stack { id: "minecraft:diamond_sword".into(), count: 1, components: None });
    t.s.equipment_tick();
    t.s.on_ground = true;
    t.s.mobs.attack_ticker = 100;
    t.s.life.vit.sprinting = false;
    t.s.life.vit.fall = 0.0;
    t.s.proj_player.known_move = [0.0; 3];
    let mut z = t.s.new_mob(crate::mobs::species::Kind::Zombie, [0.5, f64::from(FLOOR + 1), 1.5], 180.0);
    z.attrs.max_health.base = 500.0;
    z.health = 500.0;
    t.s.player_attack_mob(&mut z);
    assert!(z.health < 500.0, "main hit without damage");
    let (h, _) = guest_vitals(&t);
    assert!(h < 20.0, "the swept player was not hurt: {h}");
}

/// Hit and kill between players, as on a mob: weapon used, damage dealt, player killed
/// (statistic and objective criteria) on the attacker's side; `killed_by` of the player
/// on the victim's side.
#[test]
fn stats_between_players() {
    let mut t = open("pvpstats");
    publish(&mut t, false);
    let g = join(&mut t, "Guest", 2);
    for c in ["scoreboard objectives add kills playerKillCount", "scoreboard objectives add total totalKillCount"] {
        let mut src = t.s.player_source();
        src.level = 4;
        assert!(t.s.perform(src, c).0, "{c}");
    }
    {
        let c = t.s.lan.as_mut().unwrap().guests[0].ctx.as_mut().unwrap();
        c.loaded = true;
        c.teleport = None;
        c.game_mode = 0;
        c.vit.health = 1.0;
    }
    let slot = 36 + t.s.held as usize;
    t.s.slots[slot] = Some(crate::player::Stack { id: "minecraft:diamond_sword".into(), count: 1, components: None });
    t.s.mobs.attack_ticker = 100;
    t.s.player_attack(g.id);
    let sv = &t.s;
    assert_eq!(sv.stat_value("minecraft:used", "minecraft:diamond_sword"), 1, "sword used");
    assert_eq!(sv.stat_value("minecraft:custom", "minecraft:damage_dealt"), 10, "1 point dealt");
    assert_eq!(sv.stat_value("minecraft:custom", "minecraft:player_kills"), 1, "player killed");
    assert_eq!(sv.stat_value("minecraft:custom", "minecraft:mob_kills"), 0);
    for name in ["kills", "total"] {
        assert_eq!(sv.cmd.scores.get("G3F05", name), Some(1), "objective {name}");
    }
    let killed_by = t.s.as_guest(0, |sv| sv.stat_value("minecraft:killed_by", "minecraft:player"));
    assert_eq!(killed_by, 1, "killed by a player");
}

/// Crack of a hit door: only to players within 32 blocks.
#[test]
fn door_crack_to_nearby_players() {
    let t = open("door");
    t.s.door_break_progress(999, [100, FLOOR + 1, 100], 3);
    assert!(!drain(&t).iter().any(|e| matches!(e, Event::BlockDestruction { .. })), "player far: nothing");
    t.s.door_break_progress(999, [2, FLOOR + 1, 2], 3);
    assert!(drain(&t).iter().any(|e| matches!(e, Event::BlockDestruction { .. })), "player near");
}

/// Singleplayer F3 "Biome builder" line: computed by the server at the requested block (world of
/// seed 7, value read off this generator); nothing before the server's first loop turn.
#[test]
fn generator_biome_builder_line() {
    let mut t = open("gendbg");
    let gd = t.s.gen_debug.clone();
    assert!(gd.request([0, 64, 0]).is_empty());
    t.s.gen_debug_tick();
    assert_eq!(gd.request([0, 64, 0]), vec!["Biome builder PV: Mid C: Ocean E: 3 T: 2 H: 0".to_owned()]);
}

/// Hands swapped by the host: the guest receives event 55 and no update of the two hands;
/// a single slot changed afterwards goes out alone.
#[test]
fn hand_swap_to_observers() {
    let stack = |id: &str| Some(crate::player::Stack { id: id.into(), count: 1, components: None });
    let mut t = open("hands");
    publish(&mut t, false);
    let main = 36 + t.s.held as usize;
    t.s.slots[main] = stack("minecraft:stone");
    t.s.slots[45] = stack("minecraft:torch");
    let g = join(&mut t, "Guest", 2);
    for _ in 0..3 {
        t.s.lan_tick();
    }
    let _ = guest_packets(&g);
    let hid = t.s.player_id;
    let eq = ids(&t).set_equipment;
    t.s.slots.swap(main, 45);
    t.s.lan_tick();
    let ps = guest_packets(&g);
    assert!(entity_events(&t, &ps).contains(&(hid, 55)), "event 55 missing");
    assert!(!ps.iter().any(|p| id_of(p) == eq && varint_at(body(p)) == hid), "hands resent as equipment");
    // Helmet put on: only the head slot (5) goes out.
    t.s.slots[5] = stack("minecraft:iron_helmet");
    t.s.lan_tick();
    let ps = guest_packets(&g);
    assert!(!entity_events(&t, &ps).contains(&(hid, 55)));
    let sent: Vec<&Vec<u8>> = ps.iter().filter(|p| id_of(p) == eq && varint_at(body(p)) == hid).collect();
    assert_eq!(sent.len(), 1, "one equipment update");
    let b = body(sent[0]);
    assert_eq!(b[varint(hid).len()], 5, "head slot alone, last of the list");
}

// ---------- G5-F02a: state specific to each player ----------

/// Feet at the center of the cell `p`.
fn feet(p: [i32; 3]) -> [f64; 3] {
    [f64::from(p[0]) + 0.5, f64::from(p[1]), f64::from(p[2]) + 0.5]
}

/// Host ready at `pos`: survival, full health, fire at rest, no pending teleport.
fn host_ready(t: &mut T, pos: [f64; 3]) {
    let s = &mut t.s;
    s.loaded = true;
    s.teleport = None;
    s.game_mode = 0;
    s.pos = pos;
    s.last_good = pos;
    s.life.vit.health = 20.0;
    s.life.vit.cooldown = 0;
    s.life.vit.fire = -20;
}

/// First guest ready at `pos`, in the same state.
fn guest_ready(t: &mut T, pos: [f64; 3]) {
    let c = t.s.lan.as_mut().unwrap().guests[0].ctx.as_mut().unwrap();
    c.loaded = true;
    c.teleport = None;
    c.game_mode = 0;
    c.pos = pos;
    c.last_good = pos;
    c.vit.health = 20.0;
    c.vit.cooldown = 0;
    c.vit.fire = -20;
}

/// Health and fire of the first guest.
fn guest_vitals(t: &T) -> (f32, i32) {
    let c = t.s.lan.as_ref().unwrap().guests[0].ctx.as_ref().unwrap();
    (c.vit.health, c.vit.fire)
}

/// One tick of the integrated server: world and host, then the guests.
fn server_tick(t: &mut T) {
    t.s.game_tick();
    t.s.lan_tick();
    t.s.tick += 1;
}

/// Crossed-block pass: the guest starts from THEIR position at their last pass. Before, it started
/// from the host's and suffered, from afar, the lava the host was standing in.
#[test]
fn host_lava_not_on_guest() {
    let mut t = open("lava");
    publish(&mut t, false);
    let _g = join(&mut t, "Guest", 2);
    let host_at = [-5, FLOOR + 1, -5];
    let lava = t.s.default_state("minecraft:lava");
    t.s.set_block(host_at, lava, 2);
    host_ready(&mut t, feet(host_at));
    guest_ready(&mut t, feet([5, FLOOR + 1, 5]));
    server_tick(&mut t);
    assert_eq!((t.s.life.vit.health, t.s.life.vit.fire), (16.0, 300), "host in lava");
    assert_eq!(guest_vitals(&t), (20.0, -20), "guest far from the lava: intact, fire at rest");
}

/// The reverse: the host starts from THEIR position. Before, it started from the guest's and the
/// water the guest was swimming in put out the burning host.
#[test]
fn guest_water_not_on_host() {
    let mut t = open("water");
    publish(&mut t, false);
    let _g = join(&mut t, "Guest", 2);
    let guest_at = [5, FLOOR + 1, 5];
    let water = t.s.default_state("minecraft:water");
    t.s.set_block(guest_at, water, 2);
    host_ready(&mut t, feet([-5, FLOOR + 1, -5]));
    t.s.life.vit.fire = 100;
    guest_ready(&mut t, feet(guest_at));
    server_tick(&mut t);
    server_tick(&mut t);
    assert_eq!(t.s.life.vit.fire, 98, "host far from the water: still burning");
}

/// Path received from a guest (movement packet out of the lava): swept for the guest alone.
/// Before, it landed in the host's list, and the host took the guest's lava.
#[test]
fn guest_path_swept_for_guest() {
    let mut t = open("path");
    publish(&mut t, false);
    let _g = join(&mut t, "Guest", 2);
    let g0 = [4, FLOOR + 1, 4];
    let lava = t.s.default_state("minecraft:lava");
    t.s.set_block(g0, lava, 2);
    host_ready(&mut t, feet([-4, FLOOR + 1, -4]));
    guest_ready(&mut t, feet(g0));
    // One block to the east: the arrival box no longer touches the lava cell.
    let to = [f64::from(g0[0]) + 1.5, f64::from(FLOOR + 1), f64::from(g0[2]) + 0.5];
    t.s.as_guest(0, |sv| sv.handle(crate::input::Input::Move { pos: Some(to), rot: None, on_ground: true }));
    assert_eq!(t.s.lan.as_ref().unwrap().guests[0].ctx.as_ref().unwrap().pos, to, "movement accepted");
    server_tick(&mut t);
    assert_eq!((t.s.life.vit.health, t.s.life.vit.fire), (20.0, -20), "host: nothing of the guest's path");
    assert_eq!(guest_vitals(&t), (16.0, 300), "guest: lava crossed");
}

/// Lidded container: the guest closes their own, not the one the host has opened since.
#[test]
fn opened_container_specific_to_each_player() {
    let mut t = open("barrels");
    publish(&mut t, false);
    let _g = join(&mut t, "Guest", 2);
    let barrel = t.s.default_state("minecraft:barrel");
    let (a, b) = ([3, FLOOR + 1, 0], [-3, FLOOR + 1, 0]);
    t.s.set_block(a, barrel, 2);
    t.s.set_block(b, barrel, 2);
    host_ready(&mut t, feet([-3, FLOOR + 1, 1]));
    guest_ready(&mut t, feet([3, FLOOR + 1, 1]));
    assert!(t.s.as_guest(0, |sv| sv.use_block(a)), "guest: barrel opened");
    assert!(t.s.use_block(b), "host: barrel opened");
    t.s.as_guest(0, |sv| sv.close_menu(false));
    let openers = |t: &T, p: [i32; 3]| t.s.life.openers.get(&p).copied().unwrap_or(0);
    assert_eq!((openers(&t, a), openers(&t, b)), (0, 1), "openers (guest's barrel, host's)");
}

/// Tracked equipment: the guest does not wear the host's chestplate, and their tick does not
/// remove it from the host.
#[test]
fn equipment_specific_to_each_player() {
    let mut t = open("armor");
    publish(&mut t, false);
    let _g = join(&mut t, "Guest", 2);
    host_ready(&mut t, feet([-3, FLOOR + 1, 0]));
    guest_ready(&mut t, feet([3, FLOOR + 1, 0]));
    t.s.slots[6] = Some(crate::player::Stack { id: "minecraft:diamond_chestplate".into(), count: 1, components: None });
    t.s.equipment_tick();
    assert!(!t.s.player_mods("minecraft:armor").is_empty(), "host: chestplate counted");
    let guest = t.s.as_guest(0, |sv| sv.player_mods("minecraft:armor"));
    assert!(guest.is_empty(), "guest without armor: {guest:?}");
    t.s.as_guest(0, |sv| sv.equipment_tick());
    assert!(!t.s.player_mods("minecraft:armor").is_empty(), "host: chestplate kept after the guest's tick");
}
