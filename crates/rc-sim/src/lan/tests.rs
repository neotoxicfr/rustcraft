//! Local network: a real network client (`rc_client::connect`, compression, offline) joins
//! the open singleplayer world, sees the host, exchanges a message, then leaves.

use std::time::{Duration, Instant};

use rc_client::Event;

/// Key of a translated message.
fn key(c: &rc_text::Component) -> Option<&str> {
    match &c.content {
        rc_text::Content::Translate { key, .. } => Some(key),
        _ => None,
    }
}

/// Waits for an event (delay `secs`); also returns those seen before.
fn wait(s: &rc_client::Session, secs: u64, mut f: impl FnMut(&Event) -> bool) -> (Option<Event>, Vec<Event>) {
    let t = Instant::now();
    let mut seen = Vec::new();
    while t.elapsed() < Duration::from_secs(secs) {
        let Ok(e) = s.events.recv_timeout(Duration::from_millis(50)) else { continue };
        if f(&e) {
            return (Some(e), seen);
        }
        seen.push(e);
    }
    (None, seen)
}

/// Value of the first click found (copyable argument, between brackets).
fn click_value(c: &rc_text::Component) -> Option<String> {
    if let Some(k) = &c.style.click {
        return Some(k.value.clone());
    }
    if let rc_text::Content::Translate { with, .. } = &c.content {
        for a in with {
            if let rc_text::Arg::Component(x) = a
                && let Some(v) = click_value(x)
            {
                return Some(v);
            }
        }
    }
    c.extra.iter().find_map(click_value)
}

fn chat_key(e: &Event, k: &str) -> bool {
    matches!(e, Event::Chat { component, .. } if key(component) == Some(k))
}

#[test]
fn game_ping_string() {
    assert_eq!(super::announce::ping_string("Agent - World", 51234), "[MOTD]Agent - World[/MOTD][AD]51234[/AD]");
}

#[test]
fn guest_joins_and_leaves() {
    let (Some(world), Some(data)) = (crate::tests::test_world(), rc_testdata::version_data()) else { return };
    super::announce::LOCAL_ONLY.store(true, std::sync::atomic::Ordering::Relaxed);
    // Test client without an account: authentication not required, link encrypted all the same.
    super::auth::VERIFY.store(false, std::sync::atomic::Ordering::Relaxed);
    let dir = crate::tests::fresh_copy(&world, "lan");
    let data = std::sync::Arc::new(data);
    let (host, handle, _) = crate::tests::start_test(&dir, data.clone());
    handle.publish(super::Publish { open: true, port: 0, guest_commands: false, force_mode: None });
    let (e, _) = wait(&host, 10, |e| chat_key(e, "menu.multiplayerOptions.publish.started.lan"));
    let Some(Event::Chat { component, .. }) = e else { panic!("no opening") };
    let port: u16 = click_value(&component).expect("copyable port").parse().unwrap();
    // Real guest over TCP.
    let cfg = rc_client::Config {
        host: "127.0.0.1".into(),
        port,
        name: "Guest".into(),
        info: rc_proto_v777::play::ClientInformation { view_distance: 2, ..Default::default() },
        transfer: false,
        cookies: Default::default(),
        uuid: None,
        token: None,
    };
    rc_client::status::skip_blocked_servers();
    let guest = rc_client::connect(cfg, data).expect("connection");
    let (tp, seen) = wait(&guest, 20, |e| matches!(e, Event::Teleport(_)));
    if let Some(Event::Disconnected(r)) = seen.iter().find(|e| matches!(e, Event::Disconnected(_))) {
        panic!("guest disconnected: {r}");
    }
    assert!(seen.iter().any(|e| matches!(e, Event::Joined { .. })), "login");
    // Encryption request received and link encrypted, like the game's integrated server.
    assert!(seen.iter().any(|e| matches!(e, Event::LoginStatus("connect.encrypting"))), "encryption");
    let Some(Event::Teleport(p)) = tp else { panic!("no teleport") };
    guest.link.send_accept_teleport(p.id, p.pos, p.yaw, p.pitch).unwrap();
    guest.link.send_player_loaded().unwrap();
    // The guest receives chunks and sees the host (list and entity).
    let host_uuid = rc_client::offline_uuid("AgentSolo");
    let (mut chunks, mut listed, mut entity) = (0, false, false);
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(15) && !(chunks > 8 && listed && entity) {
        let Ok(e) = guest.events.recv_timeout(Duration::from_millis(50)) else { continue };
        match e {
            Event::Chunk(_) => chunks += 1,
            Event::PlayerInfo(u) => listed |= u.entries.iter().any(|x| x.uuid == host_uuid),
            Event::Entity(rc_client::EntityEvent::Add(a)) => entity |= a.uuid == host_uuid,
            Event::Disconnected(r) => panic!("guest disconnected: {r}"),
            _ => {}
        }
    }
    eprintln!("guest: {chunks} chunks, host listed {listed}, entity {entity}");
    assert!(chunks > 8 && listed && entity);
    // The host sees the arrival, then the guest's message.
    let (j, _) = wait(&host, 10, |e| chat_key(e, "multiplayer.player.joined"));
    assert!(j.is_some(), "arrival announced to the host");
    guest.link.send_chat("hello").unwrap();
    let (m, _) =
        wait(&host, 10, |e| matches!(e, Event::Chat { component, .. } if format!("{component:?}").contains("hello")));
    assert!(m.is_some(), "guest's message at the host");
    // Departure.
    guest.link.disconnect();
    let (l, _) = wait(&host, 10, |e| chat_key(e, "multiplayer.player.left"));
    assert!(l.is_some(), "departure announced");
    host.link.disconnect();
    handle.wait();
    // Guest's files written (written in a background task, done at closing).
    let u = crate::nbtio::uuid_text(rc_client::offline_uuid("Guest"));
    for f in
        [format!("players/data/{u}.dat"), format!("players/advancements/{u}.json"), format!("players/stats/{u}.json")]
    {
        assert!(dir.join(&f).is_file(), "{f}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Keep-alive: challenge after 15 s, latency smoothed at the answer, kick with
/// `disconnect.timeout` if the next challenge arrives with no answer to the previous one.
#[test]
fn keep_alive_challenge() {
    let (Some(world), Some(data)) = (crate::tests::test_world(), rc_testdata::version_data()) else { return };
    super::announce::LOCAL_ONLY.store(true, std::sync::atomic::Ordering::Relaxed);
    let dir = crate::tests::fresh_copy(&world, "lan-keepalive");
    let mut s = crate::portal_tests::open_world(&dir, std::sync::Arc::new(data));
    let req = super::Publish { open: true, port: 0, guest_commands: false, force_mode: Some(0) };
    s.lan_publish(&req).expect("opening");
    let uuid = rc_client::offline_uuid("Guest");
    let ctx = s.new_player_ctx("Guest", uuid, 2, 0, Some(0));
    let (out, from_server) = crossbeam_channel::unbounded();
    let (to_server, inp) = crossbeam_channel::unbounded();
    let past = Instant::now() - Duration::from_secs(16);
    s.lan.as_mut().unwrap().guests.push(super::Guest {
        name: "Guest".into(),
        uuid,
        out,
        inp,
        ctx: Some(Box::new(ctx)),
        enc: Default::default(),
        gone: false,
        joined: true,
        net: super::GuestNet { keep_time: Some(past), latency: 100, ..Default::default() },
        req_view: 2,
        chunks: Default::default(),
        moves: Default::default(),
    });
    let ids = *s.peer.ids();
    let id_of = |p: &[u8]| rc_proto::varint::read_varint(&mut rc_proto::Reader::new(p)).ok();
    // Challenge sent.
    s.lan_keep_alive();
    let ka = from_server.try_iter().find(|p| id_of(p) == Some(ids.keep_alive)).expect("challenge sent");
    let challenge = &ka[ka.len() - 8..];
    // Answer 0 ms later (or nearly): latency (100 × 3 + ~0) / 4.
    let mut answer = Vec::new();
    rc_proto::varint::write_varint(&mut answer, ids.keep_alive_out);
    answer.extend_from_slice(challenge);
    to_server.send(answer).unwrap();
    s.lan_poll();
    let g = &s.lan.as_ref().unwrap().guests[0];
    assert!(g.net.pending.is_none());
    assert!((75..=80).contains(&g.net.latency), "latency {}", g.net.latency);
    assert_eq!(s.lan_latency(uuid), g.net.latency);
    // Next challenge with no answer: at the third pass, kick.
    s.lan.as_mut().unwrap().guests[0].net.keep_time = Some(past);
    s.lan_keep_alive();
    assert!(s.lan.as_ref().unwrap().guests[0].net.pending.is_some());
    s.lan.as_mut().unwrap().guests[0].net.keep_time = Some(past);
    s.lan_keep_alive();
    assert!(s.lan.as_ref().unwrap().guests[0].gone, "kicked");
    let kicked = from_server.try_iter().any(|p| id_of(&p) == Some(ids.disconnect));
    assert!(kicked, "kick message");
    s.lan_close();
    let _ = std::fs::remove_dir_all(&dir);
}

/// Status: description as plain text, sample of at most 12 players shuffled, hidden
/// players anonymous, world icon; no empty sample.
#[test]
fn status_like_the_game() {
    let Some(data) = rc_testdata::version_data() else { return };
    let ids = rc_client::ids::Ids::resolve(&data).unwrap();
    let sh = super::conn::Shared {
        data: std::sync::Arc::new(data),
        ids,
        motd: "Agent - World".into(),
        max_players: 8,
        online: std::sync::atomic::AtomicUsize::new(14),
        names: Default::default(),
        synced: Vec::new(),
        tags: Default::default(),
        stop: Default::default(),
        icon: Some("data:image/png;base64,AA==".into()),
        keys: Default::default(),
        host: ("Agent".into(), 1),
        status: Default::default(),
        features: Vec::new(),
    };
    let names: Vec<(String, u128, bool)> = (0..14).map(|i| (format!("J{i}"), i as u128 + 1, i != 3)).collect();
    let mut rng = super::conn::Rng(0x1234_5678_9abc_def1);
    let json = super::conn::status_json(&sh, "26.3", &names, &mut rng);
    assert!(json.starts_with("{\"description\":\"Agent - World\",\"players\":{\"max\":8,\"online\":14,\"sample\":["));
    assert!(
        json.ends_with(",\"version\":{\"name\":\"26.3\",\"protocol\":777},\"favicon\":\"data:image/png;base64,AA==\"}")
    );
    let v: serde_json::Value = serde_json::from_str(&json).unwrap();
    let sample = v["players"]["sample"].as_array().unwrap();
    assert_eq!(sample.len(), 12);
    let order: Vec<&str> = sample.iter().map(|p| p["name"].as_str().unwrap()).collect();
    let sorted = {
        let mut o = order.clone();
        o.sort();
        o
    };
    assert_ne!(order, sorted, "sample shuffled");
    if order.contains(&"Anonymous Player") {
        assert!(sample.iter().any(|p| p["id"] == "00000000-0000-0000-0000-000000000000"));
    }
    let json = super::conn::status_json(&sh, "26.3", &[], &mut rng);
    assert!(!json.contains("sample"));
}

/// Maps and trading of guests: the written packets are re-read identically by the
/// client's decoder.
#[test]
fn maps_and_trading_reread() {
    use rc_proto_v777::item::ItemStack;
    use rc_proto_v777::map::{MapDecoration, MapItemData, MapPatch};
    let m = MapItemData {
        id: 7,
        scale: 2,
        locked: false,
        decorations: Some(vec![MapDecoration { kind: 1, x: -4, y: 10, rot: 3, name: None }]),
        patch: Some(MapPatch { x0: 5, y0: 6, width: 2, height: 1, colors: vec![0x1d, 0x22] }),
    };
    let mut w = Vec::new();
    super::encode::map_item_data(&mut w, &m);
    let mut r = rc_proto::Reader::new(&w);
    assert_eq!(rc_proto_v777::map::read_map_item_data(&mut r).unwrap(), m);
    assert!(r.is_empty());
    let empty = MapItemData { id: 3, scale: 0, locked: true, decorations: None, patch: None };
    let mut w = Vec::new();
    super::encode::map_item_data(&mut w, &empty);
    assert_eq!(w, [3, 0, 1, 0, 0]);
    let st = |item, count| ItemStack { item, count, added: Vec::new(), removed: Vec::new() };
    let offer = rc_proto_v777::special::Offer {
        cost_a: st(1, 12),
        result: Some(st(2, 1)),
        cost_b: Some(st(3, 1)),
        out_of_stock: false,
        uses: 2,
        max_uses: 12,
        xp: 5,
        special_diff: -1,
        price_multiplier: 0.05,
        demand: 3,
    };
    let mo = rc_proto_v777::special::MerchantOffers {
        window: 4,
        offers: vec![offer.clone(), rc_proto_v777::special::Offer { cost_b: None, ..offer }],
        level: 2,
        xp: 30,
        show_progress: true,
        can_restock: true,
    };
    let mut w = Vec::new();
    super::encode::merchant_offers(&mut w, &mo);
    let items = rc_proto_v777::item::ItemCodec::new(&[], &[]);
    let mut r = rc_proto::Reader::new(&w);
    assert_eq!(rc_proto_v777::special::read_merchant_offers(&items, &mut r).unwrap(), mo);
    assert!(r.is_empty());
}

/// Demo guest (screenshots): `RUSTCRAFT_LAN_JOIN=<port>` joins 127.0.0.1, turns
/// around its arrival point for 40 s, writes a message, then leaves. Client without an
/// account: the host must be started with `RUSTCRAFT_LAN_AUTH=0`.
#[test]
#[ignore = "manual: world opened by a Rustcraft client"]
fn demo_guest() {
    let (Some(port), Some(data)) =
        (std::env::var("RUSTCRAFT_LAN_JOIN").ok().and_then(|p| p.parse().ok()), rc_testdata::version_data())
    else {
        return;
    };
    let cfg = rc_client::Config {
        host: "127.0.0.1".into(),
        port,
        name: "Guest".into(),
        info: rc_proto_v777::play::ClientInformation { view_distance: 2, ..Default::default() },
        transfer: false,
        cookies: Default::default(),
        uuid: None,
        token: None,
    };
    rc_client::status::skip_blocked_servers();
    // The world opens after the host client starts: retries for 2 min.
    let data = std::sync::Arc::new(data);
    let t0 = Instant::now();
    let guest = loop {
        match rc_client::connect(cfg.clone(), data.clone()) {
            Ok(g) => break g,
            Err(e) if t0.elapsed() > Duration::from_secs(120) => panic!("connection: {e}"),
            Err(_) => std::thread::sleep(Duration::from_millis(500)),
        }
    };
    let (tp, _) = wait(&guest, 30, |e| matches!(e, Event::Teleport(_)));
    let Some(Event::Teleport(p)) = tp else { panic!("no teleport") };
    guest.link.send_accept_teleport(p.id, p.pos, p.yaw, p.pitch).unwrap();
    guest.link.send_player_loaded().unwrap();
    eprintln!("guest: arrival at {:?}", p.pos);
    let t = Instant::now();
    let mut said = false;
    while t.elapsed() < Duration::from_secs(40) {
        while guest.events.try_recv().is_ok() {}
        let a = t.elapsed().as_secs_f64() * 0.5;
        let pos = [p.pos[0] + 2.0 * a.cos(), p.pos[1], p.pos[2] + 2.0 * a.sin()];
        let m = rc_client::link::Move::PosRot { pos, yaw: (a.to_degrees() as f32) % 360.0, pitch: 0.0 };
        let _ = guest.link.send_move(m, true, false);
        if !said && t.elapsed() > Duration::from_secs(5) {
            let _ = guest.link.send_chat("Hello from the local network");
            said = true;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    guest.link.disconnect();
}
