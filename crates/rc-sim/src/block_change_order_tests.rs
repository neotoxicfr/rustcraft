//! Order of the block packets within a tick, like the game: the blocks changed up to the chunk
//! source (actions and commands before the tick, scheduled and random ticks) leave before the
//! block events; what a block event changes leaves with the next tick's broadcast; the light of
//! a change leaves with the broadcast that follows its relight, before the blocks.
//! Fresh flat world: the 26.3 data cache and the client jar are required (absent: FAILURE).

use std::path::PathBuf;
use std::sync::Arc;

use rc_client::Event;
use rc_world::block_index;

use crate::blocks::P;
use crate::blocks::probe::Probe;
use crate::newworld::{NewWorld, Preset, create_world};

/// First air layer of the flat world.
const Y: i32 = -60;

/// Ticks for a setup to come to rest: an observer set by a command sees its front at once and
/// pulses (as in the game), and the piston it powers extends then retracts.
const SETTLE_TICKS: usize = 20;

fn version_data() -> Arc<rc_data::VersionData> {
    let data = Arc::new(rc_testdata::version_data().expect("block change order tests: 26.3 data cache missing"));
    assert!(crate::registries::client_jar(&data).is_some(), "block change order tests: 26.3 client jar missing");
    data
}

/// Flat world around the origin, the chunks around it loaded and known to the client.
fn new_world(tag: &str) -> (Probe, PathBuf) {
    new_world_of_radius(tag, 1)
}

fn new_world_of_radius(tag: &str, chunks: i32) -> (Probe, PathBuf) {
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-block-order-{tag}-{}", std::process::id())));
    let mut w = NewWorld::new(tag);
    w.seed = 1;
    w.preset = Preset::Flat;
    w.allow_commands = true;
    create_world(&dir, &version_data(), &w).expect("world creation");
    let mut p = Probe::open(&dir, version_data());
    p.s.pos = [0.5, f64::from(Y), 0.5];
    p.s.loaded = true;
    p.s.teleport = None;
    let around: Vec<(i32, i32)> = (-chunks..=chunks).flat_map(|x| (-chunks..=chunks).map(move |z| (x, z))).collect();
    p.s.load_now(&around);
    p.s.sent.extend(around);
    let _ = p.take_events();
    (p, dir)
}

fn cmd(p: &mut Probe, text: &str) {
    let src = p.s.server_source();
    assert!(p.s.perform(src, text).0, "command refused: {text}");
}

/// The events of each of `count` game ticks.
fn ticks(p: &mut Probe, count: usize) -> Vec<Vec<Event>> {
    (0..count)
        .map(|_| {
            p.s.game_tick();
            p.s.tick += 1;
            p.take_events()
        })
        .collect()
}

fn touches_block(e: &Event, at: P) -> bool {
    let section = [at[0] >> 4, at[1] >> 4, at[2] >> 4];
    let index = block_index((at[0] & 15) as usize, (at[1] & 15) as usize, (at[2] & 15) as usize) as u16;
    match e {
        Event::BlockUpdate { pos, .. } => [pos.x, pos.y, pos.z] == at,
        Event::SectionUpdate { section: s, changes } => *s == section && changes.iter().any(|c| c.0 == index),
        _ => false,
    }
}

/// Block packets of `evs` about the `watched` blocks, in the order received: `update <name>`
/// for a changed block, `event <name>` for a block event, `light` for the light of the origin's
/// column, `ahead` when a watched block's section is announced ahead of its light.
fn block_packets(evs: &[Event], watched: &[(P, &str)]) -> Vec<String> {
    let mut out = Vec::new();
    for e in evs {
        match e {
            Event::BlockEvent { pos, .. } => {
                out.extend(watched.iter().filter(|(at, _)| at == pos).map(|(_, name)| format!("event {name}")));
            }
            Event::LightUpdate { x: 0, z: 0, .. } => out.push("light".to_owned()),
            Event::BlocksAheadOfLight { section } => {
                if watched.iter().any(|(at, _)| [at[0] >> 4, at[1] >> 4, at[2] >> 4] == *section) {
                    out.push("ahead".to_owned());
                }
            }
            _ => out.extend(watched.iter().filter(|(at, _)| touches_block(e, *at)).map(|(_, name)| format!("update {name}"))),
        }
    }
    out
}

fn without_light(packets: &[String]) -> Vec<String> {
    packets.iter().filter(|p| *p != "light" && *p != "ahead").cloned().collect()
}

fn light_updates_of(evs: &[Event], column: (i32, i32)) -> usize {
    evs.iter().filter(|e| matches!(e, Event::LightUpdate { x, z, .. } if (*x, *z) == column)).count()
}

/// An observer's scheduled tick powers a piston: the observer's change leaves before the
/// piston's block event, and the piston base the event extends leaves on the next tick (the
/// game broadcasts the changed blocks in the chunk source, before running the block events).
#[test]
fn a_scheduled_tick_change_leaves_before_the_block_event_it_causes() {
    let (mut p, _dir) = new_world("piston");
    let (watched_block, observer, piston) = ([0, Y, 4], [1, Y, 4], [2, Y, 4]);
    cmd(&mut p, "setblock 2 -60 4 minecraft:piston[facing=east]");
    cmd(&mut p, "setblock 1 -60 4 minecraft:observer[facing=west]");
    let _ = ticks(&mut p, SETTLE_TICKS);
    cmd(&mut p, "setblock 0 -60 4 minecraft:stone");
    let watched = [(watched_block, "stone"), (observer, "observer"), (piston, "piston")];
    let per_tick: Vec<Vec<String>> = ticks(&mut p, 6).iter().map(|evs| without_light(&block_packets(evs, &watched))).collect();
    assert_eq!(per_tick[0], ["update stone"], "the command's block leaves with the first broadcast");
    let fired = per_tick.iter().position(|t| t.iter().any(|x| x == "event piston")).expect("the piston never extended");
    assert_eq!(per_tick[fired], ["update observer", "event piston"], "tick of the observer's scheduled tick");
    assert_eq!(per_tick[fired + 1], ["update piston"], "the extended base leaves with the next broadcast");
}

/// A lamp lit by an observer's scheduled tick: the lamp leaves in that tick's broadcast, its
/// light, computed at the end of the tick, with the next one, before the blocks. The lamp's
/// section is announced ahead of its light (the singleplayer client relights it itself); the
/// command's stone, whose light the head of the tick computed, is not.
#[test]
fn the_light_of_a_change_leaves_with_the_next_broadcast() {
    let (mut p, _dir) = new_world("lamp");
    let (watched_block, observer, lamp) = ([0, Y, 8], [1, Y, 8], [2, Y, 8]);
    cmd(&mut p, "setblock 2 -60 8 minecraft:redstone_lamp");
    cmd(&mut p, "setblock 1 -60 8 minecraft:observer[facing=west]");
    let _ = ticks(&mut p, SETTLE_TICKS);
    cmd(&mut p, "setblock 0 -60 8 minecraft:stone");
    let watched = [(watched_block, "stone"), (observer, "observer"), (lamp, "lamp")];
    let per_tick: Vec<Vec<String>> = ticks(&mut p, 6).iter().map(|evs| block_packets(evs, &watched)).collect();
    assert_eq!(per_tick[0], ["light", "update stone"], "the command's stone, its light computed first");
    let lit = per_tick.iter().position(|t| t.iter().any(|x| x == "update lamp")).expect("the lamp never lit");
    assert_eq!(per_tick[lit], ["ahead", "update observer", "update lamp"], "the lamp leaves without its light");
    assert_eq!(per_tick[lit + 1].first().map(String::as_str), Some("light"), "its light opens the next broadcast");
}

/// Two light computations before one broadcast (the end of a tick, then the head of the next
/// for a command run between them) send one `light_update` for the column, like the game's
/// single packet per chunk, with the sections of both.
#[test]
fn one_light_update_per_column_and_broadcast() {
    let (mut p, _dir) = new_world("one-light");
    // Two lights far enough apart that their computations touch different sections.
    let (low, high) = (-60, 0);
    cmd(&mut p, &format!("setblock 2 {low} 8 minecraft:glowstone"));
    cmd(&mut p, &format!("setblock 2 {high} 8 minecraft:glowstone"));
    let _ = ticks(&mut p, 3);
    // End of a tick for the first removal, head of the next one for the second.
    cmd(&mut p, &format!("setblock 2 {low} 8 minecraft:air"));
    p.s.relight_changed_blocks();
    cmd(&mut p, &format!("setblock 2 {high} 8 minecraft:air"));
    let evs = ticks(&mut p, 1).remove(0);
    assert_eq!(light_updates_of(&evs, (0, 0)), 1, "light updates of the column in one broadcast");
    let Some(Event::LightUpdate { light, .. }) = evs.iter().find(|e| matches!(e, Event::LightUpdate { x: 0, z: 0, .. })) else {
        unreachable!()
    };
    let column = p.s.chunks[&(0, 0)].as_ref().expect("column loaded");
    for y in [low, high] {
        let i = (((y - p.s.dim.height.min_y) >> 4) + 1) as usize;
        assert!(light.block[i].is_some(), "section of y {y} missing from the joined packet");
        assert_eq!(light.block[i], column.block[i], "section of y {y}: not the current layer");
    }
}

/// A column the client forgets between the light computation and the broadcast gets no
/// `light_update`, like its blocks.
#[test]
fn no_light_update_for_a_column_forgotten_before_the_broadcast() {
    let (mut p, _dir) = new_world("forgotten");
    cmd(&mut p, "setblock 20 -60 4 minecraft:glowstone");
    p.s.relight_changed_blocks();
    assert!(p.s.pending_light_updates.iter().any(|e| matches!(e, Event::LightUpdate { x: 1, z: 0, .. })), "no light computed");
    p.s.sent.remove(&(1, 0));
    let evs = ticks(&mut p, 1).remove(0);
    assert_eq!(light_updates_of(&evs, (1, 0)), 0, "light of a forgotten column sent");
}

/// Under `tick freeze` the chunk source still broadcasts: a block set by a command leaves on
/// the next tick.
#[test]
fn a_frozen_world_still_broadcasts_its_changed_blocks() {
    let (mut p, _dir) = new_world("frozen");
    p.s.player_command("tick freeze");
    let _ = ticks(&mut p, 2);
    cmd(&mut p, "setblock 0 -60 4 minecraft:stone");
    let per_tick = ticks(&mut p, 1);
    assert_eq!(without_light(&block_packets(&per_tick[0], &[([0, Y, 4], "stone")])), ["update stone"]);
}

/// Server tick of the bench's 128 redstone clocks (`perf:redstone`, rows on stone) with every
/// chunk known to the client: median and p99 of the tick, block packets per tick. Loaded
/// machine figures only; run with `--ignored --nocapture`.
#[test]
#[ignore = "bench"]
fn bench_redstone_clocks_tick() {
    let (mut p, _dir) = new_world_of_radius("bench", 3);
    for row in 0..16 {
        let bz = -16 + row * 2;
        cmd(&mut p, &format!("fill -21 -61 {bz} 25 -61 {bz} minecraft:stone replace minecraft:grass_block"));
        for col in 0..8 {
            let bx = -20 + col * 6;
            for (dx, block) in [
                (-1, "redstone_lamp"),
                (2, "sticky_piston[facing=east]"),
                (3, "slime_block"),
                (0, "observer[facing=east]"),
                (1, "observer[facing=west]"),
            ] {
                cmd(&mut p, &format!("setblock {} {Y} {bz} minecraft:{block}", bx + dx));
            }
        }
    }
    let _ = ticks(&mut p, 100);
    let mut times = Vec::new();
    let mut packets = 0usize;
    const MEASURED: usize = 600;
    for _ in 0..MEASURED {
        let start = std::time::Instant::now();
        p.s.game_tick();
        times.push(start.elapsed());
        p.s.tick += 1;
        packets += p.take_events().iter().filter(|e| matches!(e, Event::BlockUpdate { .. } | Event::SectionUpdate { .. } | Event::BlockEvent { .. } | Event::LightUpdate { .. })).count();
    }
    times.sort_unstable();
    eprintln!(
        "redstone clocks: tick median {:?}, p99 {:?}, {:.1} block packets per tick",
        times[MEASURED / 2],
        times[MEASURED * 99 / 100],
        packets as f64 / MEASURED as f64
    );
}
