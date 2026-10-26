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

struct T {
    s: Server,
    session: rc_client::Session,
    dir: std::path::PathBuf,
}

impl Drop for T {
    fn drop(&mut self) {
        self.s.lan_close();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

const FLOOR: i32 = 199;

fn open(tag: &str) -> T {
    let data = Arc::new(rc_testdata::version_data().expect("version data missing: G3-F05 tests impossible"));
    crate::registries::client_jar(&data).expect("client jar missing: G3-F05 tests impossible");
    let dir = std::env::temp_dir().join(format!("rc-g3f05-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
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

fn drain(t: &T) -> Vec<Event> {
    let mut out = Vec::new();
    while let Ok(e) = t.session.events.try_recv() {
        out.push(e);
    }
    out
}

fn publish(t: &mut T, guest_commands: bool) {
    let req = super::Publish { open: true, port: 0, guest_commands, force_mode: None };
    t.s.lan_publish(&req).expect("opening");
}

/// Plugged-in guest: packets received, sending packets to the server.
struct G {
    from: Receiver<Vec<u8>>,
    to: Sender<Vec<u8>>,
    id: i32,
}

fn join(t: &mut T, name: &str, view: u8) -> G {
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
        let mut m = sv.mobs.list[i].take().expect("camel");
        let ok = sv.player_start_riding(&mut m);
        sv.mobs.list[i] = Some(m);
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
