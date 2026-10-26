//! G3-F07: basic commands (`/difficulty` in hardcore, `/kick`, `/unpublish`, `/playsound`,
//! `/teammsg`, `/version`), "server" source, debug world and demo mode. New world created
//! by the generator: version data and jar required (absent: explicit failure).

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use crossbeam_channel::Receiver;
use rc_client::Event;
use rc_proto::nbt::Nbt;

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

fn data() -> Arc<rc_data::VersionData> {
    let data = Arc::new(rc_testdata::version_data().expect("version data missing: G3-F07 tests impossible"));
    crate::registries::client_jar(&data).expect("client jar missing: G3-F07 tests impossible");
    data
}

fn world_dir(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("rc-g3f07-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

fn open(tag: &str) -> T {
    let data = data();
    let dir = world_dir(tag);
    let mut w = crate::newworld::NewWorld::new(tag);
    w.seed = 7;
    w.allow_commands = true;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    crate::tests::solo_slot();
    let uuid = rc_client::offline_uuid("G3F07");
    let (session, peer) = rc_client::memory::session(data.clone(), uuid).expect("session");
    let opts = crate::Options {
        world: dir.clone(),
        name: "G3F07".into(),
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
    for x in -4..=4 {
        for z in -4..=4 {
            s.set_block([x, FLOOR, z], stone, 2);
            for y in FLOOR + 1..=FLOOR + 4 {
                s.set_block([x, y, z], air, 2);
            }
        }
    }
    s.pos = [0.5, f64::from(FLOOR + 1), 0.5];
    s.last_good = s.pos;
    s.sent.insert((0, 0));
    s.level.spawn_pos = [0, FLOOR + 1, 2];
    crate::lan::announce::LOCAL_ONLY.store(true, std::sync::atomic::Ordering::Relaxed);
    let t = T { s, session, dir };
    drain(&t);
    t
}

fn drain(t: &T) -> Vec<Event> {
    t.session.events.try_iter().collect()
}

/// Translation keys of the chat messages received by the host (a failure is a red empty text
/// that carries the message as a following part).
fn chat_keys(ev: &[Event]) -> Vec<String> {
    fn key(c: &rc_text::Component) -> Option<String> {
        match &c.content {
            rc_text::Content::Translate { key, .. } => Some(key.clone()),
            _ => c.extra.first().and_then(key),
        }
    }
    ev.iter()
        .filter_map(|e| match e {
            Event::Chat { component, .. } => key(component),
            _ => None,
        })
        .collect()
}

fn run(t: &mut T, c: &str) -> (bool, i32) {
    let mut src = t.s.player_source();
    src.level = 4;
    t.s.perform(src, c)
}

fn publish(t: &mut T) {
    let req = super::Publish { open: true, port: 0, guest_commands: false, force_mode: None };
    t.s.lan_publish(&req).expect("opening");
}

fn join(t: &mut T, name: &str) -> Receiver<Vec<u8>> {
    let (out, from) = crossbeam_channel::unbounded();
    let (_to, inp) = crossbeam_channel::unbounded();
    let info = rc_proto_v777::play::ClientInformation { view_distance: 2, ..Default::default() };
    let j = super::conn::Joined {
        name: name.into(),
        uuid: rc_client::offline_uuid(name),
        properties: Vec::new(),
        info,
        out,
        inp,
    };
    // Input channel kept open: otherwise the guest would be taken as gone at the first poll.
    std::mem::forget(_to);
    t.s.lan_join(j);
    from
}

/// A packet received by the guest contains this text (translation key encoded as is).
fn got(from: &Receiver<Vec<u8>>, text: &str) -> bool {
    from.try_iter().any(|p| p.windows(text.len()).any(|w| w == text.as_bytes()))
}

/// Hardcore: `/difficulty easy` succeeds (message, return 0) but the difficulty stays hard;
/// the lock, however, is ignored.
#[test]
fn hardcore_difficulty_stays_hard() {
    let mut t = open("diff");
    t.s.level.hardcore = true;
    t.s.level.difficulty = "hard".into();
    assert_eq!(run(&mut t, "difficulty easy"), (true, 0));
    assert_eq!(t.s.level.difficulty, "hard");
    let ev = drain(&t);
    assert!(ev.iter().any(|e| matches!(e, Event::Difficulty { id: 3, .. })), "difficulty sent back: hard");
    assert!(chat_keys(&ev).contains(&"commands.difficulty.success".to_owned()));
    t.s.level.hardcore = false;
    t.s.level.difficulty_locked = true;
    assert_eq!(run(&mut t, "difficulty peaceful"), (true, 0));
    assert_eq!(t.s.level.difficulty, "peaceful");
}

/// `/kick`: refused in singleplayer, refused for the host, dismissal of a named guest (departure before success).
#[test]
fn kick_singleplayer_host_and_guest() {
    let mut t = open("kick");
    assert!(!run(&mut t, "kick G3F07").0);
    assert!(chat_keys(&drain(&t)).contains(&"commands.kick.singleplayer.failed".to_owned()));
    publish(&mut t);
    let bob = join(&mut t, "Bob");
    drain(&t);
    assert!(!run(&mut t, "kick @s").0);
    assert!(chat_keys(&drain(&t)).contains(&"commands.kick.owner.failed".to_owned()));
    assert_eq!(run(&mut t, "kick bob Goodbye"), (true, 1));
    assert!(got(&bob, "Goodbye"), "reason sent to the guest");
    assert!(t.s.lan.as_ref().unwrap().guests.is_empty(), "guest gone");
    let keys = chat_keys(&drain(&t));
    let left = keys.iter().position(|k| k == "multiplayer.player.left");
    let ok = keys.iter().position(|k| k == "commands.kick.success");
    assert!(left.is_some() && left < ok, "departure then success: {keys:?}");
}

/// `/unpublish`: "not open" in singleplayer; otherwise guests sent away, world closed, success.
#[test]
fn unpublish_closes_the_world() {
    let mut t = open("unpub");
    assert!(!run(&mut t, "unpublish").0);
    assert!(chat_keys(&drain(&t)).contains(&"commands.unpublish.notPublished".to_owned()));
    publish(&mut t);
    let bob = join(&mut t, "Bob");
    let _carl = join(&mut t, "Carl");
    let dan = join(&mut t, "Dan");
    // Dan: connection not yet entered in the game.
    t.s.lan.as_mut().unwrap().guests[2].joined = false;
    drain(&t);
    assert_eq!(run(&mut t, "unpublish"), (true, 1));
    assert!(t.s.lan.is_none());
    assert!(got(&bob, "multiplayer.disconnect.server_shutdown"));
    assert!(got(&dan, "multiplayer.disconnect.server_shutdown"), "closing sent to everyone");
    let ev = drain(&t);
    let left: Vec<String> = ev
        .iter()
        .filter_map(|e| match e {
            Event::Chat { component, .. } => Some(format!("{component:?}")),
            _ => None,
        })
        .filter(|d| d.contains("multiplayer.player.left"))
        .collect();
    assert_eq!(left.len(), 2, "Bob and Carl only: {left:?}");
    assert!(left[0].contains("Bob") && left[1].contains("Carl"), "arrival order: {left:?}");
    let keys = chat_keys(&ev);
    assert_eq!(keys.last().map(String::as_str), Some("commands.unpublish.success"), "{keys:?}");
}

/// `/playsound`: direct sound (name, variable range) even for a registry entry; nothing for
/// a player in a dimension other than the source's.
#[test]
fn playsound_direct_sound() {
    let mut t = open("sound");
    assert!(run(&mut t, "playsound minecraft:block.note_block.harp master @s").0);
    let named = drain(&t).into_iter().find_map(|e| match e {
        Event::Sound(p) => Some(p.sound),
        _ => None,
    });
    assert!(
        matches!(named, Some(rc_proto_v777::play::SoundRef::Named(ref n)) if n == "minecraft:block.note_block.harp"),
        "{named:?}"
    );
    assert!(!run(&mut t, "execute in minecraft:the_nether run playsound minecraft:block.note_block.harp master @s").0);
    assert!(!drain(&t).iter().any(|e| matches!(e, Event::Sound(_))));
}

/// `/teammsg`: sent to the sender, received only by the team's players; return = number of
/// recipients; without an entity: refused.
#[test]
fn teammsg_recipients() {
    let mut t = open("team");
    publish(&mut t);
    let bob = join(&mut t, "Bob");
    let carl = join(&mut t, "Carl");
    assert!(run(&mut t, "team add blue").0);
    assert!(run(&mut t, "team join blue G3F07").0);
    assert!(run(&mut t, "team join blue Bob").0);
    drain(&t);
    let _ = (got(&bob, "-"), got(&carl, "-"));
    assert_eq!(run(&mut t, "teammsg hi"), (true, 2));
    assert!(chat_keys(&drain(&t)).contains(&"chat.type.team.sent".to_owned()));
    assert!(got(&bob, "chat.type.team.text"), "teammate");
    assert!(!got(&carl, "chat.type.team"), "outside the team");
    let src = t.s.server_source();
    assert!(!t.s.perform(src, "teammsg hi").0, "without an entity");
}

/// `/version`: system messages (shown even without `send_command_feedback`), values from the jar.
#[test]
fn version_values_from_the_jar() {
    let mut t = open("version");
    assert!(run(&mut t, "gamerule send_command_feedback false").0);
    drain(&t);
    assert_eq!(run(&mut t, "version"), (true, 1));
    let ev = drain(&t);
    let keys = chat_keys(&ev);
    assert_eq!(keys.len(), 10, "{keys:?}");
    assert_eq!(keys[0], "commands.version.header");
    assert_eq!(keys[9], "commands.version.stable.yes");
    let dump = format!("{ev:?}");
    for v in ["5023", "\"main\"", "0x309", "97.1", "121.0", "Sep 15", "2026"] {
        assert!(dump.contains(v), "{v} missing: {dump}");
    }
    // Launched by the player then passed to a mob: the original output (the player) still
    // receives all 10 lines.
    assert!(run(&mut t, &format!("summon minecraft:pig 0.5 {} 2.5", FLOOR + 1)).0);
    drain(&t);
    assert_eq!(run(&mut t, "execute as @e[type=pig,limit=1] run version"), (true, 1));
    assert_eq!(chat_keys(&drain(&t)).len(), 10, "execute as <mob>: messages to the player");
}

/// Java date: day, month, local time, zone name.
#[test]
fn date_java() {
    use crate::version_info::{java_date, java_zone_name, parse_iso};
    let ms = parse_iso("2026-09-15T11:20:48+00:00").unwrap();
    assert_eq!(ms, 1_789_471_248_000);
    assert_eq!(parse_iso("2026-09-15T13:20:48.5+02:00"), Some(ms + 500));
    assert_eq!(parse_iso("2026-09-15T11:20:48Z"), Some(ms));
    assert_eq!(java_date(ms, 7200, "CEST"), "Tue Sep 15 13:20:48 CEST 2026");
    assert_eq!(java_date(ms, -36_000, "x"), "Tue Sep 15 01:20:48 x 2026");
    assert_eq!(java_zone_name("Romance Standard Time", true, 7200), "CEST");
    assert_eq!(java_zone_name("Romance Standard Time", false, 3600), "CET");
    assert_eq!(java_zone_name("?", false, 19_800), "GMT+05:30");
    assert_eq!(java_zone_name("?", false, -3600), "GMT-01:00");
    // Readings on a JVM 25: names outside Europe and the United States, zone without a name (Dubai).
    assert_eq!(java_zone_name("Tokyo Standard Time", false, 32_400), "JST");
    assert_eq!(java_zone_name("India Standard Time", false, 19_800), "IST");
    assert_eq!(java_zone_name("AUS Eastern Standard Time", true, 39_600), "AEDT");
    assert_eq!(java_zone_name("Russian Standard Time", false, 10_800), "MSK");
    assert_eq!(java_zone_name("Arabian Standard Time", false, 14_400), "GMT+04:00");
}

/// "Server" source: lower corner of the spawn point, in its dimension.
#[test]
fn server_source() {
    let mut t = open("srcsrv");
    t.s.level.spawn_pos = [3, 70, -5];
    t.s.level.spawn_dimension = "minecraft:the_nether".into();
    let s = t.s.server_source();
    assert_eq!((s.pos, s.dim.as_str()), ([3.0, 70.0, -5.0], "minecraft:the_nether"));
    t.s.level.spawn_dimension = "demo:unknown".into();
    assert_eq!(t.s.server_source().dim, "minecraft:overworld");
}

/// Debug world: peaceful locked, spectator, weather frozen, noon (chosen rules
/// kept).
#[test]
fn debug_world_settings() {
    let data = data();
    let dir = world_dir("debug");
    let mut w = crate::newworld::NewWorld::new("debug");
    w.preset = crate::newworld::Preset::Debug;
    w.hardcore = true;
    w.game_rules =
        vec![("minecraft:advance_weather".into(), Nbt::Byte(1)), ("minecraft:keep_inventory".into(), Nbt::Byte(1))];
    crate::newworld::create_world(&dir, &data, &w).expect("creation");
    let lv = crate::level::Level::read(&dir).unwrap();
    let rd = |f: &str| crate::nbtio::read_dat(&dir.join("data").join("minecraft").join(f)).unwrap();
    let rules = rd("game_rules.dat");
    let clocks = rd("world_clocks.dat");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!((lv.difficulty.as_str(), lv.difficulty_locked, lv.game_type), ("peaceful", true, 3));
    let r = rules.get("data").unwrap();
    assert_eq!(r.get("minecraft:advance_weather"), Some(&Nbt::Byte(0)));
    assert_eq!(r.get("minecraft:keep_inventory"), Some(&Nbt::Byte(1)));
    let c = clocks.get("data").and_then(|d| d.get("minecraft:overworld")).and_then(|c| c.get("total_ticks"));
    assert_eq!(c, Some(&Nbt::Long(6000)));
}

fn demo_events(ev: &[Event]) -> Vec<f32> {
    ev.iter()
        .filter_map(|e| match e {
            Event::GameEvent { kind: 5, param } => Some(*param),
            _ => None,
        })
        .collect()
}

/// Demo mode: welcome after 20 ticks, first-day hints, day messages; end past
/// 120,500 ticks: breaking and placing refused (acknowledgement sent), reminder after more than 100 ticks.
#[test]
fn mode_demo() {
    let mut t = open("demo");
    t.s.demo_world = true;
    t.s.time = 50;
    for _ in 0..21 {
        t.s.demo_tick();
    }
    assert_eq!(demo_events(&drain(&t)), [0.0]);
    for (time, want) in [(100, 101.0), (175, 102.0), (250, 103.0), (5 * 24000 + 500, 104.0)] {
        t.s.time = time;
        t.s.demo_tick();
        assert_eq!(demo_events(&drain(&t)), [want], "time {time}");
    }
    for (time, key) in [(500, "demo.day.1"), (24000 * 3 + 500, "demo.day.4"), (24000 * 4 + 22000, "demo.day.warning")] {
        t.s.time = time;
        t.s.demo_tick();
        assert_eq!(chat_keys(&drain(&t)), [key], "time {time}");
    }
    // End: refusal without reminder as long as 100 ticks of finished demo have not passed.
    t.s.time = super::super::demo::TOTAL_PLAY_TICKS + 1;
    t.s.demo_tick();
    let stone = t.s.default_state("minecraft:stone");
    t.s.on_action(0, [1, FLOOR, 1], 1, 7);
    assert_eq!(t.s.ack, Some(7));
    assert!(chat_keys(&drain(&t)).is_empty());
    for _ in 0..100 {
        t.s.demo_tick();
    }
    t.s.game_mode = 1;
    t.s.on_action(0, [1, FLOOR, 1], 1, 8);
    assert_eq!(t.s.get([1, FLOOR, 1]), stone, "block kept");
    assert_eq!(chat_keys(&drain(&t)), ["demo.reminder"]);
    // Use of a held item (survival: a thrown snowball is consumed): refused, acknowledged.
    t.s.game_mode = 0;
    let hand = 36 + t.s.held as usize;
    let snow = || Some(crate::player::Stack { id: "minecraft:snowball".into(), count: 4, components: None });
    t.s.slots[hand] = snow();
    let use_id = t.s.peer.ids().use_item;
    let use_item = |seq: i32| {
        let mut p = Vec::new();
        rc_proto::varint::write_varint(&mut p, use_id);
        rc_proto::varint::write_varint(&mut p, 0);
        rc_proto::varint::write_varint(&mut p, seq);
        p.extend(0f32.to_be_bytes());
        p.extend(0f32.to_be_bytes());
        p
    };
    let pkt = use_item(10);
    assert!(t.s.life_packet(&pkt));
    assert_eq!(t.s.ack, Some(10));
    assert_eq!(t.s.slots[hand].as_ref().map(|s| s.count), Some(4), "demo over: item not used");
    // Use against a block / placement: nothing placed, both cells sent back, acknowledged.
    t.s.slots[hand] = Some(crate::player::Stack { id: "minecraft:stone".into(), count: 4, components: None });
    let above = [1, FLOOR + 1, 1];
    let air = t.s.default_state("minecraft:air");
    t.s.on_use_item_on(0, [1, FLOOR, 1], 1, [0.5, 1.0, 0.5], false, 11);
    assert_eq!(t.s.ack, Some(11));
    assert_eq!(t.s.get(above), air, "demo over: nothing placed");
    let ev = drain(&t);
    let sent: Vec<[i32; 3]> = ev
        .iter()
        .filter_map(|e| match e {
            Event::BlockUpdate { pos, .. } => Some([pos.x, pos.y, pos.z]),
            _ => None,
        })
        .collect();
    assert!(sent.contains(&[1, FLOOR, 1]) && sent.contains(&above), "{sent:?}");
    assert!(chat_keys(&ev).is_empty(), "reminder already shown less than 100 ticks ago");
    for _ in 0..101 {
        t.s.demo_tick();
    }
    t.s.on_use_item_on(0, [1, FLOOR, 1], 1, [0.5, 1.0, 0.5], false, 12);
    assert_eq!(chat_keys(&drain(&t)), ["demo.reminder"]);
    // Outside the demo, the same gestures work (proof that the refusal comes from the demo).
    t.s.demo_world = false;
    t.s.on_use_item_on(0, [1, FLOOR, 1], 1, [0.5, 1.0, 0.5], false, 13);
    assert_ne!(t.s.get(above), air, "outside demo: stone placed");
    t.s.slots[hand] = snow();
    let pkt = use_item(14);
    assert!(t.s.life_packet(&pkt));
    assert_eq!(t.s.slots[hand].as_ref().map(|s| s.count), Some(3), "outside demo: snowball thrown");
    t.s.set_block(above, air, 2);
    t.s.game_mode = 1;
    // Outside the demo: none of this.
    t.s.demo_world = false;
    t.s.on_action(0, [1, FLOOR, 1], 1, 9);
    assert_ne!(t.s.get([1, FLOOR, 1]), stone, "creative: broken outside demo");
}
