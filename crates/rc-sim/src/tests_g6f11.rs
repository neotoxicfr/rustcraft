//! G6-F11: dimension bed rules (straw bed), lying down and getting up (height, standing
//! position, gaze, animation), sleep announcements of the open world, multiplayer spawns
//! (phantoms, patrols), sign click commands, dragon fight (gateways,
//! explosion source), "heard everywhere" events, spectator chunks and
//! game events (anchor, sniffer egg). Fresh flat world (no test world required;
//! 26.3 version data mandatory).

use std::path::PathBuf;
use std::sync::Arc;

use rc_client::{EntityEvent, Event};
use rc_proto::nbt::Nbt;
use rc_proto_v777::play::SoundRef;

use crate::blocks::probe::Probe;
use crate::mobs::species::Kind;
use crate::newworld::{NewWorld, Preset, create_world};
use crate::player::Stack;

fn open(tag: &str) -> (Probe, PathBuf) {
    let data = Arc::new(rc_testdata::version_data().expect("26.3 version data missing"));
    assert!(crate::registries::client_jar(&data).is_some(), "26.3 client jar missing");
    let dir = std::env::temp_dir().join(format!("rc-g6f11-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut w = NewWorld::new(tag);
    w.seed = 1;
    w.preset = Preset::Flat;
    create_world(&dir, &data, &w).expect("world creation");
    let mut p = Probe::open(&dir, data);
    p.s.pos = [0.5, -60.0, 0.5];
    p.s.yaw = 0.0;
    p.s.pitch = 0.0;
    p.s.loaded = true;
    p.s.game_mode = 0;
    p.s.teleport = None;
    let around: Vec<(i32, i32)> = (-2..=2).flat_map(|x| (-2..=2).map(move |z| (x, z))).collect();
    p.s.load_now(&around);
    (p, dir)
}

fn finish(p: Probe, dir: PathBuf) {
    drop(p);
    let _ = std::fs::remove_dir_all(&dir);
}

fn cmd(p: &mut Probe, c: &str) {
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, c);
    assert!(ok, "command refused: {c}");
}

fn set_clock(p: &mut Probe, t: i64) {
    for c in &mut p.s.clocks {
        if c.0 == "minecraft:overworld" {
            c.1 = t;
        }
    }
}

fn name(p: &Probe, at: [i32; 3]) -> String {
    p.s.data.block_of(p.s.get(at)).map_or_else(String::new, |b| b.name.clone())
}

fn sounds(p: &Probe, evs: &[Event]) -> Vec<String> {
    let reg = p.s.data.registry("minecraft:sound_event").expect("sound registry");
    evs.iter()
        .filter_map(|e| match e {
            Event::Sound(s) => Some(match &s.sound {
                SoundRef::Id(i) => reg.entries.get(*i as usize).cloned().unwrap_or_default(),
                SoundRef::Named(n) => n.clone(),
            }),
            _ => None,
        })
        .collect()
}

/// Keys (and arguments as text) of the action bar messages.
fn bars(evs: &[Event]) -> Vec<(String, Vec<String>)> {
    evs.iter()
        .filter_map(|e| match e {
            Event::ActionBar(c) => match &c.content {
                rc_text::Content::Translate { key, with, .. } => Some((
                    key.clone(),
                    with.iter()
                        .map(|a| match a {
                            rc_text::Arg::Text(t) => t.clone(),
                            rc_text::Arg::Component(c) => format!("{c:?}"),
                        })
                        .collect(),
                )),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

fn custom(p: &Probe, key: &str) -> i32 {
    p.s.stat_value("minecraft:custom", &format!("minecraft:{key}"))
}

/// Opens the world to the local network and adds a guest who has entered, without a real connection; returns the
/// reception of its packets.
fn add_guest(p: &mut Probe, who: &str, pos: [f64; 3]) -> crossbeam_channel::Receiver<Vec<u8>> {
    crate::lan::announce::LOCAL_ONLY.store(true, std::sync::atomic::Ordering::Relaxed);
    if p.s.lan.is_none() {
        let req = crate::lan::Publish { open: true, port: 0, guest_commands: false, force_mode: Some(0) };
        p.s.lan_publish(&req).expect("opening");
    }
    let uuid = rc_client::offline_uuid(who);
    let mut ctx = p.s.new_player_ctx(who, uuid, 2, 0, Some(0));
    ctx.loaded = true;
    ctx.pos = pos;
    let (out, rx) = crossbeam_channel::unbounded();
    let (_keep, inp) = crossbeam_channel::unbounded();
    std::mem::forget(_keep);
    p.s.lan.as_mut().unwrap().guests.push(crate::lan::Guest {
        name: who.into(),
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
    rx
}

/// Straw bed in the normal world, at night: lying at 0.375 above the block, respawn
/// point NOT set, own statistic, time since rest reset to zero; on getting up,
/// animation 0, bed destroyed ("on leave" sound, 2014 particles of the other half), stood up
/// beside the bed, turned towards it, pitch zero.
#[test]
fn straw_bed_sleep_and_leave() {
    let (mut p, dir) = open("straw");
    p.run("setblock 2 -60 0 minecraft:straw_bed[facing=east,part=foot]\nsetblock 3 -60 0 minecraft:straw_bed[facing=east,part=head]\n");
    p.s.pos = [1.5, -60.0, 1.5];
    p.s.pitch = 30.0;
    set_clock(&mut p, 18000);
    p.s.award_custom("time_since_rest", 500);
    p.s.life.vit.respawn = None;
    let _ = p.take_events();
    assert!(p.s.use_bed([2, -60, 0]));
    assert_eq!(p.s.life.vit.sleeping, Some([3, -60, 0]), "lying on the straw bed");
    assert!(p.s.life.vit.respawn.is_none(), "the straw bed never sets the respawn point");
    assert_eq!(p.s.pos[1], -60.0 + 0.25 + 0.125, "lying height of the straw bed");
    assert_eq!(custom(&p, "sleep_in_straw_bed"), 1);
    assert_eq!(custom(&p, "sleep_in_bed"), 0);
    assert_eq!(custom(&p, "time_since_rest"), 0, "time since rest reset to zero on lying down");
    let _ = p.take_events();
    p.s.wake_up();
    p.s.flush_block_changes();
    let evs = p.take_events();
    assert_eq!(name(&p, [3, -60, 0]), "minecraft:air", "head destroyed on getting up");
    assert_eq!(name(&p, [2, -60, 0]), "minecraft:air", "foot destroyed by the shape update");
    assert!(sounds(&p, &evs).iter().any(|s| s == "minecraft:block.straw_bed.break_leave"), "break sound on getting up");
    assert!(
        evs.iter().any(|e| matches!(e, Event::LevelEvent { kind: 2014, pos: [2, -60, 0], .. })),
        "own particles (2014) of the other half"
    );
    let id = p.s.player_id;
    assert!(
        evs.iter().any(|e| matches!(e, Event::Entity(EntityEvent::Animate { id: i, action: 0 }) if *i == id)),
        "arm animation on getting up"
    );
    assert!(p.s.life.vit.sleeping.is_none());
    assert_eq!(p.s.pos[1], -60.0, "stood up on the ground, not on the bed");
    assert_eq!(p.s.pitch, 0.0, "zero pitch on getting up");
    assert_eq!(p.s.yaw, crate::respawn::look_at_yaw(p.s.pos, [3, -60, 0]), "turned towards the bed");
    finish(p, dir);
}

/// Ordinary bed: point set, lying at 0.6875; stood up beside the bed (first safe cell on the
/// side chosen by the yaw), turned towards the bottom center of the bed; the bed stays.
#[test]
fn bed_stand_up_beside_and_look() {
    let (mut p, dir) = open("bed");
    p.run("setblock 2 -60 0 minecraft:red_bed[facing=east,part=foot]\nsetblock 3 -60 0 minecraft:red_bed[facing=east,part=head]\n");
    p.s.pos = [1.5, -60.0, 1.5];
    set_clock(&mut p, 18000);
    assert!(p.s.use_bed([2, -60, 0]));
    assert_eq!(p.s.life.vit.respawn.as_ref().map(|r| r.1), Some([3, -60, 0]), "point set at the head");
    assert_eq!(p.s.pos[1], -60.0 + 0.5625 + 0.125);
    assert_eq!(custom(&p, "sleep_in_bed"), 1);
    p.s.yaw = 0.0;
    p.s.wake_up();
    assert_eq!(name(&p, [3, -60, 0]), "minecraft:red_bed", "the ordinary bed stays");
    let want = p.s.bed_stand_up([3, -60, 0], crate::blocks::Dir::East, 0.0).expect("free cell");
    assert_eq!(p.s.pos, want, "game's standing position");
    assert_ne!(p.s.pos, [3.5, -60.0 + 0.5625, 0.5], "not on the bed");
    assert_eq!(p.s.yaw, crate::respawn::look_at_yaw(want, [3, -60, 0]));
    finish(p, dir);
}

/// Nether: the straw bed breaks without explosion ("on leave" sound), the ordinary bed explodes.
#[test]
fn nether_straw_bed_breaks_without_explosion() {
    let (mut p, dir) = open("nether");
    let rule = p.s.bed_rule("minecraft:the_nether", true);
    assert!(rule.destroy_on_use && !rule.destroy_on_leave, "Nether rule read");
    p.s.mobs.rec.vib.log.clear();
    let (straw_air, bed_air) = p.s.in_world("minecraft:the_nether", |s| {
        s.load_now(&[(0, 0)]);
        let put = |s: &mut crate::server::Server, at: [i32; 3], text: &str| {
            let st = s.states.state_of(&s.data, &state_nbt(text)).expect("state");
            s.set_block(at, st, crate::blocks::flag::ALL);
        };
        for x in 0..8 {
            for z in 0..4 {
                for y in 100..104 {
                    let a = s.air();
                    s.set_block([x, y, z], a, crate::blocks::flag::ALL);
                }
            }
        }
        put(s, [1, 100, 1], "minecraft:straw_bed|facing=east|part=foot");
        put(s, [2, 100, 1], "minecraft:straw_bed|facing=east|part=head");
        assert!(s.use_bed([1, 100, 1]));
        s.flush_block_changes();
        let straw = s.v().is_air(s.get([1, 100, 1])) && s.v().is_air(s.get([2, 100, 1]));
        put(s, [5, 100, 1], "minecraft:red_bed|facing=east|part=foot");
        put(s, [6, 100, 1], "minecraft:red_bed|facing=east|part=head");
        assert!(s.use_bed([5, 100, 1]));
        let bed = s.v().is_air(s.get([6, 100, 1]));
        (straw, bed)
    });
    assert!(straw_air, "straw bed broken on use");
    assert!(bed_air, "ordinary bed destroyed");
    // The player stayed in the normal world (Nether sounds not received): explosions counted
    // by their game event.
    let explosions = p.s.mobs.rec.vib.log.iter().filter(|e| e.0 == "minecraft:explode").count();
    assert_eq!(explosions, 1, "a single explosion: the ordinary bed's");
    finish(p, dir);
}

/// Block state written `name|key=value|…` (the game's state NBT).
fn state_nbt(text: &str) -> Nbt {
    let mut it = text.split('|');
    let name = it.next().unwrap_or_default().to_owned();
    let props: Vec<(String, Nbt)> = it
        .filter_map(|kv| kv.split_once('='))
        .map(|(k, v)| (k.to_owned(), Nbt::String(v.to_owned())))
        .collect();
    Nbt::Compound(vec![("Name".into(), Nbt::String(name)), ("Properties".into(), Nbt::Compound(props))])
}

/// Open world, two players: the host lying down announces "1/2 players sleeping" to all;
/// closed world: no announcement.
#[test]
fn sleep_announcements_when_published() {
    let (mut p, dir) = open("announce");
    p.run("setblock 2 -60 0 minecraft:red_bed[facing=east,part=foot]\nsetblock 3 -60 0 minecraft:red_bed[facing=east,part=head]\n");
    p.s.pos = [1.5, -60.0, 1.5];
    set_clock(&mut p, 18000);
    // Closed world: nothing.
    let _ = p.take_events();
    assert!(p.s.use_bed([2, -60, 0]));
    let evs = p.take_events();
    assert!(!bars(&evs).iter().any(|b| b.0.starts_with("sleep.players")), "closed singleplayer: no announcement");
    p.s.wake_up();
    // Open world, a guest standing.
    let rx = add_guest(&mut p, "Guest", [5.5, -60.0, 5.5]);
    p.s.update_sleeping_list();
    while rx.try_recv().is_ok() {}
    p.s.pos = [1.5, -60.0, 1.5];
    let _ = p.take_events();
    assert!(p.s.use_bed([2, -60, 0]));
    let evs = p.take_events();
    let b = bars(&evs);
    assert!(
        b.iter().any(|(k, a)| k == "sleep.players_sleeping" && a == &["1".to_owned(), "2".to_owned()]),
        "announcement 1/2: {b:?}"
    );
    assert!(rx.try_recv().is_ok(), "the guest also receives the announcement");
    // Percentage at 50: one sleeper is enough, the night will be skipped.
    p.s.wake_up();
    cmd(&mut p, "gamerule minecraft:players_sleeping_percentage 50");
    p.s.pos = [1.5, -60.0, 1.5];
    let _ = p.take_events();
    assert!(p.s.use_bed([2, -60, 0]));
    let b = bars(&p.take_events());
    assert!(b.iter().any(|(k, _)| k == "sleep.skipping_night"), "night skip announcement: {b:?}");
    finish(p, dir);
}

fn clock(p: &Probe) -> i64 {
    p.s.clocks.iter().find(|c| c.0 == "minecraft:overworld").map_or(-1, |c| c.1)
}

fn guest_sleeping(p: &Probe) -> Option<[i32; 3]> {
    p.s.lan.as_ref().and_then(|l| l.guests[0].ctx.as_ref()).and_then(|c| c.vit.sleeping)
}

/// Night skip from the cached sleeper list: one deep sleeper out of two is not
/// enough; a sleeper turned spectator stays deep but no longer counts in the list (nothing);
/// both lying: wake-up at marker 24000, all up (counter 100), list emptied, then a
/// new lying down announces "1/2" again.
#[test]
fn night_skip_follows_the_sleeping_list() {
    let (mut p, dir) = open("skip");
    p.run(concat!(
        "setblock 2 -60 0 minecraft:red_bed[facing=east,part=foot]\n",
        "setblock 3 -60 0 minecraft:red_bed[facing=east,part=head]\n",
        "setblock 2 -60 4 minecraft:blue_bed[facing=east,part=foot]\n",
        "setblock 3 -60 4 minecraft:blue_bed[facing=east,part=head]\n",
    ));
    p.s.pos = [1.5, -60.0, 1.5];
    set_clock(&mut p, 18000);
    let rx = add_guest(&mut p, "Sleeper", [1.5, -60.0, 4.5]);
    p.s.update_sleeping_list();
    assert!(p.s.use_bed([2, -60, 0]));
    assert!(p.s.life.vit.sleeping.is_some());
    p.s.life.vit.sleep_timer = 100;
    p.s.sleep_world_tick();
    assert_eq!(clock(&p), 18000, "one sleeper out of two at 100%: the night stays");
    p.s.set_game_mode(3);
    assert_eq!(p.s.life.weather.sleep, (1, 0), "the spectator no longer counts");
    p.s.sleep_world_tick();
    assert_eq!(clock(&p), 18000, "no active sleeper: the night stays");
    p.s.set_game_mode(0);
    assert!(p.s.life.vit.sleeping.is_some(), "still lying down");
    p.s.as_guest(0, |sv| assert!(sv.use_bed([2, -60, 4])));
    assert_eq!(guest_sleeping(&p), Some([3, -60, 4]), "guest lying down");
    assert_eq!(p.s.life.weather.sleep, (2, 2));
    if let Some(c) = p.s.lan.as_mut().unwrap().guests[0].ctx.as_mut() {
        c.vit.sleep_timer = 100;
    }
    p.s.sleep_world_tick();
    assert_eq!(clock(&p), 24000, "night skipped at the wake-up marker");
    assert!(p.s.life.vit.sleeping.is_none() && guest_sleeping(&p).is_none(), "all up");
    assert_eq!(p.s.life.weather.sleep.1, 0, "sleeper list emptied");
    assert_eq!(p.s.life.vit.sleep_timer, 100, "collective wake-up not forced");
    set_clock(&mut p, 42000);
    p.s.pos = [1.5, -60.0, 1.5];
    while rx.try_recv().is_ok() {}
    let _ = p.take_events();
    assert!(p.s.use_bed([2, -60, 0]));
    let b = bars(&p.take_events());
    assert!(
        b.iter().any(|(k, a)| k == "sleep.players_sleeping" && a == &["1".to_owned(), "2".to_owned()]),
        "new announcement 1/2: {b:?}"
    );
    finish(p, dir);
}

/// Sleep counter on waking: 100 on the requested exit and on the day come back (then 0
/// ten ticks later), 0 when the bed disappears.
#[test]
fn sleep_timer_after_each_wake_up() {
    let (mut p, dir) = open("timer");
    let bed = "setblock 2 -60 0 minecraft:red_bed[facing=east,part=foot]\nsetblock 3 -60 0 minecraft:red_bed[facing=east,part=head]\n";
    p.run(bed);
    p.s.pos = [1.5, -60.0, 1.5];
    set_clock(&mut p, 18000);
    assert!(p.s.use_bed([2, -60, 0]));
    p.s.life.vit.sleep_timer = 40;
    p.s.wake_up();
    assert_eq!(p.s.life.vit.sleep_timer, 100, "exit requested: 100");
    for _ in 0..9 {
        p.s.sleep_tick();
    }
    assert_eq!(p.s.life.vit.sleep_timer, 109, "standing: the counter advances");
    p.s.sleep_tick();
    assert_eq!(p.s.life.vit.sleep_timer, 0, "back to 0 at the tenth tick");
    // Day come back: wake-up not forced.
    p.s.pos = [1.5, -60.0, 1.5];
    assert!(p.s.use_bed([2, -60, 0]));
    set_clock(&mut p, 25000);
    p.s.sleep_tick();
    assert!(p.s.life.vit.sleeping.is_none(), "up at daybreak");
    assert_eq!(p.s.life.vit.sleep_timer, 100, "day back: 100");
    // Bed gone: forced wake-up.
    set_clock(&mut p, 42000);
    p.s.pos = [1.5, -60.0, 1.5];
    p.s.sleep_tick();
    assert!(p.s.use_bed([2, -60, 0]));
    p.s.life.vit.sleep_timer = 100;
    p.run("setblock 3 -60 0 minecraft:air\nsetblock 2 -60 0 minecraft:air\n");
    p.s.sleep_tick();
    assert!(p.s.life.vit.sleeping.is_none(), "up, bed gone");
    assert_eq!(p.s.life.vit.sleep_timer, 0, "bed gone: 0");
    finish(p, dir);
}

/// Guest who leaves the game lying down: forced wake-up, bed freed; the host can sleep in it.
#[test]
fn guest_leaving_in_bed_frees_it() {
    let (mut p, dir) = open("leave");
    p.run("setblock 2 -60 0 minecraft:red_bed[facing=east,part=foot]\nsetblock 3 -60 0 minecraft:red_bed[facing=east,part=head]\n");
    set_clock(&mut p, 18000);
    let _rx = add_guest(&mut p, "Leaver", [1.5, -60.0, 1.5]);
    p.s.as_guest(0, |sv| assert!(sv.use_bed([2, -60, 0])));
    assert_eq!(p.s.prop([3, -60, 0], "occupied").as_deref(), Some("true"));
    p.s.lan_leave(0);
    assert_eq!(p.s.prop([3, -60, 0], "occupied").as_deref(), Some("false"), "bed freed on departure");
    assert_eq!(p.s.life.weather.sleep, (1, 0), "list re-read after removal");
    p.s.pos = [1.5, -60.0, 1.5];
    assert!(p.s.use_bed([2, -60, 0]));
    assert_eq!(p.s.life.vit.sleeping, Some([3, -60, 0]), "the host sleeps in the freed bed");
    finish(p, dir);
}

/// Bed without a standing cell (surrounded by stone) at y = -1: fallback above the bed, height
/// (y + 1) + 0.1 exactly, like the game.
#[test]
fn bed_fallback_height_matches_the_game() {
    let (mut p, dir) = open("fallback");
    p.run(concat!(
        "fill 0 -3 -2 5 1 2 minecraft:stone\n",
        "setblock 2 -1 0 minecraft:red_bed[facing=east,part=foot]\n",
        "setblock 3 -1 0 minecraft:red_bed[facing=east,part=head]\n",
    ));
    let (stand, _) = p.s.leave_bed([3, -1, 0], 0.0, crate::respawn::PLAYER_DIMS, true).expect("bed present");
    assert_eq!(stand, [3.5, 0.1, 0.5], "fallback above the bed");
    assert_eq!(stand[1].to_bits(), 0.1f64.to_bits());
    finish(p, dir);
}

/// Phantoms: the guest who has not slept for a long time is a candidate even if the host is
/// a spectator.
#[test]
fn phantoms_consider_every_player() {
    let (mut p, dir) = open("phantom");
    cmd(&mut p, "difficulty hard");
    set_clock(&mut p, 18000);
    p.s.game_mode = 3;
    let _rx = add_guest(&mut p, "Insomniac", [8.5, -60.0, 8.5]);
    if let Some(c) = p.s.lan.as_mut().unwrap().guests[0].ctx.as_mut() {
        c.prog.stats.set("minecraft:custom", "minecraft:time_since_rest", i32::MAX);
    }
    let mut spawned = 0;
    for _ in 0..60 {
        p.s.mobs.phantom_next = 1;
        p.s.phantom_spawner_tick();
        spawned = p.s.mobs.list.iter().flatten().filter(|m| m.kind == Kind::Phantom).count();
        if spawned > 0 {
            break;
        }
    }
    assert!(spawned > 0, "no phantom for the awake guest");
    let m = p.s.mobs.list.iter().flatten().find(|m| m.kind == Kind::Phantom).unwrap();
    assert!((m.pos[0] - 8.5).abs() <= 11.0 && (m.pos[2] - 8.5).abs() <= 11.0, "above the guest: {:?}", m.pos);
    finish(p, dir);
}

/// Patrols: the player drawn is drawn among all (host spectator, guest in survival); the
/// patrol spawns around the guest.
#[test]
fn patrols_pick_among_all_players() {
    let (mut p, dir) = open("patrol");
    cmd(&mut p, "difficulty hard");
    set_clock(&mut p, 126_000);
    p.s.game_mode = 3;
    let _rx = add_guest(&mut p, "Walker", [8.5, -60.0, 8.5]);
    let around: Vec<(i32, i32)> = (-5..=5).flat_map(|x| (-5..=5).map(move |z| (x, z))).collect();
    p.s.load_now(&around);
    assert_eq!(p.s.world_players().len(), 2);
    let mut found = None;
    for _ in 0..300 {
        p.s.mobs.ill.patrol_next = 1;
        p.s.patrol_spawner_tick();
        if let Some(m) = p.s.mobs.list.iter().flatten().find(|m| m.kind == Kind::Pillager) {
            found = Some(m.pos);
            break;
        }
    }
    let pos = found.expect("no patrol: the guest is never drawn");
    let (dx, dz) = ((pos[0] - 8.5).abs(), (pos[2] - 8.5).abs());
    assert!((19.0..=52.0).contains(&dx) && (19.0..=52.0).contains(&dz), "around the guest: {pos:?}");
    finish(p, dir);
}

/// Waxed sign allowing operator functions: the click command runs (by the
/// player), then the waxed failure sound; without the permission: red message, nothing run;
/// waxed hanging sign: own failure sound.
#[test]
fn waxed_sign_runs_click_commands() {
    let (mut p, dir) = open("sign");
    p.s.pos = [4.5, -60.0, 7.5];
    p.s.yaw = 180.0;
    let line = |c: &str| {
        format!("{{text:\"go\",click_event:{{action:\"run_command\",command:\"/setblock {c} minecraft:gold_block\"}}}}")
    };
    cmd(
        &mut p,
        &format!(
            "setblock 4 -60 4 minecraft:oak_sign[rotation=0]{{is_waxed:1b,allow_op_features:1b,front_text:{{messages:[{},\"\",\"\",\"\"]}}}}",
            line("6 -60 6")
        ),
    );
    let _ = p.take_events();
    assert!(p.s.sign_use([4, -60, 4]));
    let evs = p.take_events();
    assert_eq!(name(&p, [6, -60, 6]), "minecraft:gold_block", "click command run");
    assert!(sounds(&p, &evs).iter().any(|s| s == "minecraft:block.sign.waxed_interact_fail"));
    cmd(
        &mut p,
        &format!(
            "setblock 2 -60 4 minecraft:oak_sign[rotation=0]{{is_waxed:1b,front_text:{{messages:[{},\"\",\"\",\"\"]}}}}",
            line("7 -60 7")
        ),
    );
    p.s.pos = [2.5, -60.0, 7.5];
    let _ = p.take_events();
    assert!(p.s.sign_use([2, -60, 4]));
    let evs = p.take_events();
    assert_eq!(name(&p, [7, -60, 7]), "minecraft:air", "without operator functions: nothing");
    assert!(bars(&evs).iter().any(|b| b.0 == "sign.click_actions_disabled"));
    cmd(&mut p, "setblock 0 -60 4 minecraft:oak_hanging_sign[rotation=0]{is_waxed:1b}");
    p.s.pos = [0.5, -60.0, 7.5];
    let _ = p.take_events();
    assert!(p.s.sign_use([0, -60, 4]));
    let evs = p.take_events();
    assert!(sounds(&p, &evs).iter().any(|s| s == "minecraft:block.hanging_sign.waxed_interact_fail"));
    finish(p, dir);
}

/// Dragon fight: a gateway counts as an active exit portal.
#[test]
fn end_gateway_counts_as_exit_portal() {
    let (mut p, dir) = open("gateway");
    assert!(!p.s.has_active_exit_portal());
    p.run("setblock 3 -55 3 minecraft:end_gateway\n");
    assert!(p.s.has_active_exit_portal(), "End gateway in the fight's chunks");
    finish(p, dir);
}

/// Explosion whose source is a crystal: its blast does not touch that crystal, but
/// touches the others.
#[test]
fn crystal_explosion_spares_its_source() {
    let (mut p, dir) = open("crystal");
    let a = p.s.spawn_crystal([0.5, -59.0, 0.5], 0.0, false);
    let b = p.s.spawn_crystal([3.5, -59.0, 0.5], 0.0, false);
    let pos = [0.5, -59.0, 0.5];
    p.s.explode_with(None, None, Some(a), pos, 6.0, false, "minecraft:explosion", crate::mobs::explode::Blast::None);
    let alive = |id: i32| p.s.mobs.end.crystals.iter().any(|c| c.id == id && !c.removed);
    assert!(alive(a), "the source crystal is not hit by its own blast");
    assert!(!alive(b), "the other crystal is destroyed");
    finish(p, dir);
}

/// Eye placed that opens an End portal far from the player: "heard everywhere" event
/// brought back to 32 blocks from them; rule `global_sound_events` false: ordinary event at the
/// center.
#[test]
fn global_sound_events_rule() {
    let (mut p, dir) = open("global");
    let base = [96, -60, 96];
    let around: Vec<(i32, i32)> = (5..=6).flat_map(|x| (5..=6).map(move |z| (x, z))).collect();
    p.s.load_now(&around);
    let frame = p.s.default_state("minecraft:end_portal_frame");
    let build = |p: &mut Probe, last_eye: bool| {
        for i in 0..3 {
            for (q, d) in [
                ([base[0] + i, base[1], base[2] - 1], crate::blocks::Dir::South),
                ([base[0] + i, base[1], base[2] + 3], crate::blocks::Dir::North),
                ([base[0] - 1, base[1], base[2] + i], crate::blocks::Dir::East),
                ([base[0] + 3, base[1], base[2] + i], crate::blocks::Dir::West),
            ] {
                let fs = p.s.v().set_dir(frame, "facing", d);
                let fs = p.s.v().set_bool(fs, "eye", !(q == [base[0] + 1, base[1], base[2] + 3]) || last_eye);
                p.s.set_block(q, fs, crate::blocks::flag::ALL);
            }
        }
        for x in 0..3 {
            for z in 0..3 {
                let a = p.s.air();
                p.s.set_block([base[0] + x, base[1], base[2] + z], a, crate::blocks::flag::ALL);
            }
        }
    };
    p.s.game_mode = 1;
    build(&mut p, false);
    p.s.slots[36 + p.s.held as usize] = Some(Stack { id: "minecraft:ender_eye".into(), count: 1, components: None });
    let _ = p.take_events();
    assert!(p.s.use_ender_eye([base[0] + 1, base[1], base[2] + 3]));
    let evs = p.take_events();
    let ev: Vec<([i32; 3], bool)> = evs
        .iter()
        .filter_map(|e| match e {
            Event::LevelEvent { kind: 1038, pos, global, .. } => Some((*pos, *global)),
            _ => None,
        })
        .collect();
    assert_eq!(ev.len(), 1, "one opening event: {ev:?}");
    let (at, global) = ev[0];
    assert!(global, "rule true: global event");
    let d = ((f64::from(at[0]) - p.s.pos[0]).powi(2) + (f64::from(at[2]) - p.s.pos[2]).powi(2)).sqrt();
    assert!(d <= 33.0, "position brought back to 32 blocks from the player: {at:?} (distance {d})");
    // Rule false: ordinary event at the center of the portal.
    cmd(&mut p, "gamerule minecraft:global_sound_events false");
    build(&mut p, false);
    let _ = p.take_events();
    assert!(p.s.use_ender_eye([base[0] + 1, base[1], base[2] + 3]));
    let evs = p.take_events();
    let ev: Vec<([i32; 3], bool)> = evs
        .iter()
        .filter_map(|e| match e {
            Event::LevelEvent { kind: 1038, pos, global, .. } => Some((*pos, *global)),
            _ => None,
        })
        .collect();
    assert_eq!(ev, vec![([base[0] + 1, base[1], base[2] + 1], false)], "rule false: local event");
    finish(p, dir);
}

/// Spectator, rule `spectators_generate_chunks` false: a never-loaded chunk is not loaded
/// for them (it waits in the queue); rule true: loaded and sent.
#[test]
fn spectators_do_not_generate_chunks() {
    let (mut p, dir) = open("spect");
    p.s.game_mode = 3;
    cmd(&mut p, "gamerule minecraft:spectators_generate_chunks false");
    let far = (40, 40);
    p.s.queue.push(far);
    p.s.send_chunks();
    assert!(!p.s.chunks.contains_key(&far), "chunk loaded by a spectator despite the rule");
    assert!(p.s.queue.contains(&far), "the chunk waits in the queue");
    cmd(&mut p, "gamerule minecraft:spectators_generate_chunks true");
    p.s.send_chunks();
    assert!(p.s.chunks.contains_key(&far), "rule true: loaded");
    finish(p, dir);
}

/// Game events: anchor charge (block changed, player, new state) and sniffer egg
/// placed (block placed, no source).
#[test]
fn anchor_and_sniffer_egg_game_events() {
    let (mut p, dir) = open("events");
    p.run("setblock 2 -60 2 minecraft:respawn_anchor\n");
    p.s.mobs.rec.vib.log.clear();
    assert!(p.s.use_anchor([2, -60, 2], Some("minecraft:glowstone")));
    let log = p.s.mobs.rec.vib.log.clone();
    let change: Vec<_> = log.iter().filter(|e| e.0 == "minecraft:block_change").collect();
    assert_eq!(change.len(), 1, "charge: one 'block changed' event: {log:?}");
    assert_eq!(change[0].2, Some(crate::mobs::recent::vibration::Src::Living(crate::mobs::mob::Ent::Player)));
    assert_eq!(change[0].3.map(|s| p.s.v().get_int(s, "charges")), Some(1));
    p.s.mobs.rec.vib.log.clear();
    cmd(&mut p, "setblock 4 -60 4 minecraft:sniffer_egg");
    let log = p.s.mobs.rec.vib.log.clone();
    let placed: Vec<_> = log.iter().filter(|e| e.0 == "minecraft:block_place").collect();
    assert_eq!(placed.len(), 1, "egg placed: one 'block placed' event: {log:?}");
    assert_eq!(placed[0].2, None);
    assert_eq!(placed[0].3.and_then(|s| p.s.data.block_of(s)).map(|b| b.name.as_str()), Some("minecraft:sniffer_egg"));
    finish(p, dir);
}
