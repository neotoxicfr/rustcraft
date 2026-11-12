//! LAN guests in another world than the host's, like the game where each player stands in its own
//! world and every world with players or force-loaded chunks ticks: a guest's portal, respawn and
//! `execute in … tp` take it alone to its own world, its commands run there, the world it stands
//! in keeps ticking while the host is elsewhere, a parked world's forced chunks tick, and a guest
//! enters the game in the world of its file. Fresh flat world: the 26.3 data cache and the client
//! jar are required (absent: FAILURE).

use crossbeam_channel::{Receiver, Sender};
use rc_proto::varint::read_varint;

use crate::blocks::probe::Probe;
use crate::dims::{NETHER, OVERWORLD};
use crate::other_dimension_send_tests::{new_world, run};
use crate::server::Server;

/// Opens the world to the network and adds a joined guest at `pos` in the overworld, holding the
/// chunk it stands in: what it is sent, and the sender that keeps its connection open.
fn add_guest(p: &mut Probe, pos: [f64; 3]) -> (Receiver<Vec<u8>>, Sender<Vec<u8>>) {
    let guest = crate::mobs::tests_spawn_paths::add_guest_with_packets(&mut p.s, "Guest", pos);
    let ctx = guest_ctx_mut(&mut p.s);
    ctx.sent.insert(((pos[0].floor() as i32) >> 4, (pos[2].floor() as i32) >> 4));
    ctx.op = 4;
    guest
}

fn guest_ctx(s: &Server) -> &super::ctx::PlayerCtx {
    s.lan.as_ref().and_then(|l| l.guests[0].ctx.as_deref()).expect("guest state")
}

fn guest_ctx_mut(s: &mut Server) -> &mut super::ctx::PlayerCtx {
    s.lan.as_mut().and_then(|l| l.guests[0].ctx.as_deref_mut()).expect("guest state")
}

/// Whole server ticks: the host's world and the server-wide parts, then the guests.
fn step(p: &mut Probe, ticks: usize) {
    for _ in 0..ticks {
        p.s.game_tick();
        p.s.lan_tick();
        p.s.tick += 1;
    }
}

/// `command` typed by the guest, with its own state installed.
fn run_as_guest(p: &mut Probe, command: &str) -> bool {
    p.s.as_player(Some(0), |s| {
        let src = s.player_source();
        s.perform(src, command).0
    })
}

/// Chunks within two of `centre` in world `world`, made complete (a ticking chunk needs its
/// neighbours loaded).
fn load_around(p: &mut Probe, world: &str, centre: (i32, i32)) {
    let around: Vec<(i32, i32)> = (-2..=2).flat_map(|x| (-2..=2).map(move |z| (centre.0 + x, centre.1 + z))).collect();
    p.s.in_world(world, |s| s.load_now(&around));
}

fn block_name(p: &mut Probe, world: &str, at: [i32; 3]) -> String {
    p.s.in_world(world, |s| {
        let state = s.get(at);
        s.data.block_of(state).map(|b| b.name.clone()).unwrap_or_default()
    })
}

fn received(guest: &Receiver<Vec<u8>>, packet: i32) -> bool {
    guest.try_iter().any(|b| read_varint(&mut rc_proto::Reader::new(&b)).ok() == Some(packet))
}

/// The host alone goes to the Nether (the guest's client keeps showing the overworld).
fn host_to_nether(p: &mut Probe) {
    let mut host = p.s.player_source();
    host.level = 4;
    let (moved, _) = p.s.perform(host, "execute in minecraft:the_nether run tp @s 0.5 80 0.5");
    assert!(moved && p.s.dim.name == NETHER, "host not in the Nether");
}

/// Sand put in the air at `at` in `world`, from the server (it falls once its chunk ticks).
fn floating_sand(p: &mut Probe, world: &str, at: [i32; 3]) {
    let [x, y, z] = at;
    // Fails when the column is already empty: nothing to check.
    let src = p.s.server_source();
    let _ = p.s.perform(src, &format!("execute in {world} run fill {x} {} {z} {x} {y} {z} minecraft:air", y - 6));
    let _ = run(p, &format!("execute in {world} run setblock {x} {} {z} minecraft:stone", y - 6));
    let _ = run(p, &format!("execute in {world} run setblock {x} {y} {z} minecraft:sand"));
    assert_eq!(block_name(p, world, at), "minecraft:sand", "setup: sand in the air");
}

/// While the host stands in the Nether, the overworld where the guest stands keeps ticking around
/// the guest, like the game which ticks every world with players: sand put in the air near it
/// falls. Before: only the host's world ticked, the guest's stood still.
#[test]
fn the_world_a_guest_stands_in_ticks_while_the_host_is_elsewhere() {
    let (mut p, _dir) = new_world("guest-world-ticks");
    let (_guest, _keep) = add_guest(&mut p, [0.5, -60.0, 0.5]);
    load_around(&mut p, OVERWORLD, (0, 0));
    host_to_nether(&mut p);
    floating_sand(&mut p, OVERWORLD, [3, -54, 3]);
    step(&mut p, 40);
    assert_eq!(block_name(&mut p, OVERWORLD, [3, -54, 3]), "minecraft:air", "the sand never fell");
    assert_eq!(block_name(&mut p, OVERWORLD, [3, -59, 3]), "minecraft:sand", "the sand did not land");
    assert_eq!(p.s.dim.name, NETHER, "the server no longer works in the host's world");
    p.s.lan_close();
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

/// When the host leaves for the Nether, the overworld goes on for the guest standing there: a cow
/// its client was shown stays known (neither forgotten by the server nor announced again), like
/// the game where the world left keeps tracking its entities for the players still there.
/// Before: the cow was forgotten with the host's departure.
#[test]
fn the_host_leaving_keeps_the_guests_world_entities_known() {
    let (mut p, _dir) = new_world("host-leaves-guest-world");
    let (guest, _keep) = add_guest(&mut p, [0.5, -60.0, 0.5]);
    let ids = *p.s.peer.ids();
    p.s.sent.insert((0, 0));
    let _ = run(&mut p, "summon minecraft:cow 2.5 -60 2.5 {NoAI:1b}");
    let cow = p.s.mobs.list.iter().find(|m| m.kind == crate::mobs::species::Kind::Cow).map(|m| m.id).expect("cow");
    step(&mut p, 2);
    assert!(added_and_removed(&guest, &ids).0.contains(&cow), "setup: the guest was shown the cow");
    host_to_nether(&mut p);
    let known = p.s.in_world(OVERWORLD, |s| s.mobs.list.iter().find(|m| m.id == cow).is_some_and(|m| m.track.seen));
    assert!(known, "the cow was forgotten with the host's departure");
    step(&mut p, 5);
    let (added, removed) = added_and_removed(&guest, &ids);
    assert!(!added.contains(&cow), "the cow was announced again to the guest");
    assert!(!removed.contains(&cow), "the cow was taken from the guest");
    p.s.lan_close();
}

/// Whole server ticks until the chunks within two of `centre` in `world` are there (read, or
/// generated in the background and collected by that world's tick). Anti-hang guard only: 300 s.
fn tick_until_loaded_around(p: &mut Probe, world: &str, centre: (i32, i32)) {
    let started = std::time::Instant::now();
    let loaded = |p: &mut Probe| {
        p.s.in_world(world, |s| {
            let present = |c: (i32, i32)| matches!(s.chunks.get(&c), Some(Some(_)));
            (-2..=2).all(|x| (-2..=2).all(|z| present((centre.0 + x, centre.1 + z))))
        })
    };
    while !loaded(p) {
        let guard = std::time::Duration::from_secs(300);
        assert!(started.elapsed() < guard, "anti-hang guard: chunks around {centre:?} of {world} missing after 300 s");
        step(p, 1);
    }
}

/// Sand put in the air in the forced Nether chunk (0, 0) falls within 40 server ticks while the
/// only player stands in the overworld.
fn sand_falls_in_the_forced_nether_chunk(p: &mut Probe) {
    floating_sand(p, NETHER, [3, 100, 3]);
    step(p, 40);
    assert_eq!(block_name(p, NETHER, [3, 100, 3]), "minecraft:air", "the sand never fell");
    assert_eq!(block_name(p, NETHER, [3, 95, 3]), "minecraft:sand", "the sand did not land");
    assert_eq!(p.s.dim.name, OVERWORLD, "the server no longer works in the host's world");
}

/// Force-loaded chunks of a world nobody stands in tick like the game's: `forceload add` loads the
/// forced chunk with its neighbours (nothing loaded beforehand), and sand put in the air there falls
/// while the only player stands in the overworld. Before: a parked world's forced chunks stood
/// still, and `forceload add` loaded the forced chunk alone, without the neighbours it needs to
/// tick.
#[test]
fn forced_chunks_of_a_world_nobody_stands_in_tick() {
    let (mut p, _dir) = new_world("parked-forced");
    let _ = run(&mut p, "execute in minecraft:the_nether run forceload add 0 0");
    tick_until_loaded_around(&mut p, NETHER, (0, 0));
    sand_falls_in_the_forced_nether_chunk(&mut p);
}

/// A world reopened with forced chunks in the Nether opens the Nether and loads them, like the
/// game which loads every world's forced chunks before the player enters: they tick while the
/// player stands in the overworld. Before: the Nether stayed closed until a visit, its forced
/// chunks still.
#[test]
fn forced_chunks_of_another_world_tick_after_reopening() {
    let (mut p, dir) = new_world("parked-forced-reopened");
    let _ = run(&mut p, "execute in minecraft:the_nether run forceload add 0 0");
    p.s.save(true);
    drop(p);
    let mut p = crate::other_dimension_send_tests::open_world(&dir);
    assert!(p.s.dims.parked.iter().any(|w| w.dim.name == NETHER), "the Nether was not opened with its forced chunk");
    tick_until_loaded_around(&mut p, NETHER, (0, 0));
    sand_falls_in_the_forced_nether_chunk(&mut p);
}

/// A cow standing still at `at` in `world`: its entity id.
fn still_cow(p: &mut Probe, world: &str, at: [f64; 3]) -> i32 {
    let [x, y, z] = at;
    let _ = run(p, &format!("execute in {world} run summon minecraft:cow {x} {y} {z} {{NoAI:1b,NoGravity:1b}}"));
    let cow = |s: &mut Server| s.mobs.list.iter().rev().find(|m| m.kind == crate::mobs::species::Kind::Cow).map(|m| m.id);
    p.s.in_world(world, cow).expect("cow")
}

/// Ticks lived by the mob `id` of `world`.
fn ticks_lived(p: &mut Probe, world: &str, id: i32) -> i32 {
    p.s.in_world(world, |s| s.mobs.list.iter().find(|m| m.id == id).map(|m| m.tick_count)).expect("mob")
}

/// The creative host steps into a lit Nether portal of the overworld and the server ticks, one by
/// one, until three ticks after the host reached the Nether: how many ticks the mob `id` of
/// `world` lived in each server tick.
fn ticks_lived_while_the_host_goes_through_a_portal(p: &mut Probe, world: &str, id: i32) -> Vec<i32> {
    // Obsidian frame 4 × 5 standing on the ground, lit by a fire inside.
    let _ = run(p, "fill 3 -61 0 6 -57 0 minecraft:obsidian");
    let _ = run(p, "fill 4 -60 0 5 -58 0 minecraft:air");
    let _ = run(p, "setblock 4 -60 0 minecraft:fire");
    assert_eq!(block_name(p, OVERWORLD, [5, -59, 0]), "minecraft:nether_portal", "setup: portal lit");
    p.s.game_mode = 1; // creative: no portal delay
    p.s.pos = [5.0, -60.0, 0.5];
    let mut lived = Vec::new();
    let mut before = ticks_lived(p, world, id);
    let mut ticks_in_the_nether = 0;
    // A count of server ticks, not a duration: the creative host goes through at once.
    for _ in 0..40 {
        step(p, 1);
        let now = ticks_lived(p, world, id);
        lived.push(now - before);
        before = now;
        if p.s.dim.name == NETHER {
            ticks_in_the_nether += 1;
            if ticks_in_the_nether == 4 {
                break;
            }
        }
    }
    assert_eq!(p.s.dim.name, NETHER, "setup: the host did not go through the portal");
    lived
}

/// The world the host leaves through a portal ticks once in that server tick, like the game which
/// ticks every world before the players move: a cow by the guest staying in the overworld lives one
/// tick per server tick, the host's passage included. Before: `game_tick` ticked the overworld,
/// then `dims_tick`, the host gone, ticked it a second time as a guest's world.
#[test]
fn the_world_the_host_leaves_ticks_once_in_that_tick() {
    let (mut p, _dir) = new_world("host-leaves-ticks-once");
    let (_guest, _keep) = add_guest(&mut p, [0.5, -60.0, 0.5]);
    let cow = still_cow(&mut p, OVERWORLD, [1.5, -60.0, 4.5]);
    let lived = ticks_lived_while_the_host_goes_through_a_portal(&mut p, OVERWORLD, cow);
    assert!(lived.iter().all(|&n| n == 1), "overworld cow's ticks per server tick: {lived:?}");
    p.s.lan_close();
}

/// The world the host reaches through a portal ticks once in that server tick: a cow by the guest
/// standing in the Nether lives one tick per server tick, the host's arrival included. Before: the
/// Nether, ticked as the guest's world only while parked, missed the tick of the host's arrival.
#[test]
fn the_world_the_host_reaches_ticks_once_in_that_tick() {
    let (mut p, _dir) = new_world("host-reaches-ticks-once");
    let (_guest, _keep) = add_guest(&mut p, [0.5, -60.0, 0.5]);
    load_around(&mut p, NETHER, (0, 0));
    let ctx = guest_ctx_mut(&mut p.s);
    ctx.client_world = NETHER.to_owned();
    ctx.pos = [0.5, 100.0, 0.5];
    p.s.player_enters_active_world(); // the players' worlds noted apart
    let cow = still_cow(&mut p, NETHER, [2.5, 100.0, 2.5]);
    let lived = ticks_lived_while_the_host_goes_through_a_portal(&mut p, NETHER, cow);
    assert!(lived.iter().all(|&n| n == 1), "Nether cow's ticks per server tick: {lived:?}");
    p.s.lan_close();
}

/// Ticks lived by each cow, (overworld, Nether), in each of `ticks` server ticks.
fn ticks_lived_per_server_tick(p: &mut Probe, overworld_cow: i32, nether_cow: i32, ticks: usize) -> Vec<(i32, i32)> {
    let lived_now = |p: &mut Probe| (ticks_lived(p, OVERWORLD, overworld_cow), ticks_lived(p, NETHER, nether_cow));
    let mut lived = Vec::new();
    for _ in 0..ticks {
        let before = lived_now(p);
        step(p, 1);
        let after = lived_now(p);
        lived.push((after.0 - before.0, after.1 - before.1));
    }
    lived
}

/// An impulse command block running `command` once, placed at `at` in `world` (it runs in that
/// world's scheduled ticks of the next tick).
fn command_block(p: &mut Probe, world: &str, at: [i32; 3], command: &str) {
    let [x, y, z] = at;
    let block = format!("minecraft:command_block{{auto:1b,Command:'{command}'}}");
    let _ = run(p, &format!("execute in {world} run setblock {x} {y} {z} {block}"));
}

/// A command block of the overworld sends the host to the Nether during the overworld's scheduled
/// ticks: like the game, where the player leaves at once but the world it left finishes its tick,
/// the overworld's later scheduled ticks run there, and each world ticks once. A still cow by a
/// guest in each world lives one tick per server tick. Before: the rest of the overworld's tick
/// ran in the Nether (the later command block's `setblock` there), `dims_tick` then ticked the
/// Nether a second time (2 ticks for its cow) and the overworld's entities skipped that tick (0).
#[test]
fn a_command_block_sending_the_host_elsewhere_ticks_each_world_once() {
    let (mut p, _dir) = new_world("command-block-sends-host");
    let (_first, _keep_first) = add_guest(&mut p, [0.5, -60.0, 0.5]);
    let (_second, _keep_second) =
        crate::mobs::tests_spawn_paths::add_guest_with_packets(&mut p.s, "Second", [0.5, 100.0, 0.5]);
    load_around(&mut p, NETHER, (0, 0));
    let second = p.s.lan.as_mut().and_then(|l| l.guests[1].ctx.as_deref_mut()).expect("second guest");
    second.client_world = NETHER.to_owned();
    p.s.player_enters_active_world(); // the players' worlds noted apart
    let overworld_cow = still_cow(&mut p, OVERWORLD, [1.5, -60.0, 4.5]);
    let nether_cow = still_cow(&mut p, NETHER, [2.5, 100.0, 2.5]);
    // The host is the player nearest to the command blocks; both tick in the same tick, in order.
    p.s.pos = [8.5, -60.0, 8.5];
    command_block(&mut p, OVERWORLD, [9, -61, 9], "execute as @p in minecraft:the_nether run tp @s 0.5 100 4.5");
    command_block(&mut p, OVERWORLD, [10, -61, 9], "setblock 2 -60 2 minecraft:gold_block");
    let mut lived = ticks_lived_per_server_tick(&mut p, overworld_cow, nether_cow, 1);
    assert_eq!(p.s.client_world, NETHER, "setup: the command block did not send the host to the Nether");
    let placed = block_name(&mut p, OVERWORLD, [2, -60, 2]);
    assert_eq!(placed, "minecraft:gold_block", "the overworld's later scheduled tick did not run there in that tick");
    lived.extend(ticks_lived_per_server_tick(&mut p, overworld_cow, nether_cow, 5));
    assert!(lived.iter().all(|&l| l == (1, 1)), "cows' ticks per server tick (overworld, Nether): {lived:?}");
    p.s.lan_close();
}

/// The same for a guest's world ticked away from the host: a command block of the Nether sends
/// the guest standing there to the host's overworld during the Nether's scheduled ticks; the
/// Nether finishes its tick there (its later command block's `setblock` included) and each world
/// ticks once. Before: the rest of the Nether's tick ran in the overworld.
#[test]
fn a_command_block_sending_a_guest_elsewhere_ticks_each_world_once() {
    let (mut p, _dir) = new_world("command-block-sends-guest");
    let (_guest, _keep) = add_guest(&mut p, [0.5, 100.0, 0.5]);
    load_around(&mut p, NETHER, (0, 0));
    guest_ctx_mut(&mut p.s).client_world = NETHER.to_owned();
    p.s.player_enters_active_world(); // the players' worlds noted apart
    p.s.pos = [0.5, -60.0, 0.5];
    let overworld_cow = still_cow(&mut p, OVERWORLD, [1.5, -60.0, 4.5]);
    let nether_cow = still_cow(&mut p, NETHER, [2.5, 100.0, 2.5]);
    command_block(&mut p, NETHER, [1, 99, 1], "execute as @p in minecraft:overworld run tp @s 0.5 -60 8.5");
    command_block(&mut p, NETHER, [2, 99, 1], "setblock 3 100 0 minecraft:gold_block");
    let mut lived = ticks_lived_per_server_tick(&mut p, overworld_cow, nether_cow, 1);
    assert_eq!(guest_ctx(&p.s).client_world, OVERWORLD, "setup: the command block did not send the guest away");
    assert_eq!(p.s.dim.name, OVERWORLD, "the server no longer works in the host's world");
    let placed = block_name(&mut p, NETHER, [3, 100, 0]);
    assert_eq!(placed, "minecraft:gold_block", "the Nether's later scheduled tick did not run there in that tick");
    lived.extend(ticks_lived_per_server_tick(&mut p, overworld_cow, nether_cow, 5));
    // Once the guest has gone, nobody holds the Nether cow's chunk: it may stop; never twice.
    let once = lived.iter().all(|&(overworld, nether)| overworld == 1 && nether <= 1);
    assert!(once, "cows' ticks per server tick (overworld, Nether): {lived:?}");
    p.s.lan_close();
}

/// Alone in its overworld, the host stands by two command blocks that run in the same tick, the
/// first sending it to the Nether's (0.5, 100, 0.5), then `second_command`. A still cow stands in
/// each world: their ticks lived in each of the three server ticks from there, (overworld, Nether).
fn host_sent_to_the_nether_then(p: &mut Probe, second_command: &str) -> Vec<(i32, i32)> {
    load_around(p, NETHER, (0, 0));
    let overworld_cow = still_cow(p, OVERWORLD, [9.5, -60.0, 12.5]);
    let nether_cow = still_cow(p, NETHER, [2.5, 100.0, 2.5]);
    p.s.pos = [8.5, -60.0, 8.5];
    command_block(p, OVERWORLD, [9, -61, 9], "execute in minecraft:the_nether run tp @p 0.5 100 0.5");
    command_block(p, OVERWORLD, [10, -61, 9], second_command);
    ticks_lived_per_server_tick(p, overworld_cow, nether_cow, 3)
}

/// A command run in the overworld's tick after a command block of that tick sent the host to the
/// Nether finds the host where it stands, in its world: the obsidian goes under its feet in the
/// Nether, like the game. And the Nether, where nobody else stands, ticks once in that tick with
/// the host there, like the game which ticks it after the overworld: its cow lives a tick in each
/// server tick. The overworld cow by the host's departure point stops at once: the game removes the
/// host's simulation ticket when it leaves, and the overworld's chunk source recomputes which
/// chunks tick before the overworld's entities. Before: the host was read at the place it was
/// hidden at (the obsidian went nowhere), and the Nether missed that tick (its cow lived 0).
#[test]
fn a_command_after_the_host_left_its_world_targets_it_where_it_stands() {
    let (mut p, _dir) = new_world("command-after-host-left");
    let lived = host_sent_to_the_nether_then(&mut p, "execute at @p run setblock ~ ~-1 ~ minecraft:obsidian");
    assert_eq!(p.s.client_world, NETHER, "setup: the command block did not send the host to the Nether");
    assert_eq!(block_name(&mut p, NETHER, [0, 99, 0]), "minecraft:obsidian", "the host was not found where it stands");
    assert_eq!(lived, [(0, 1); 3], "cows' ticks per server tick (overworld, Nether)");
}

/// A teleport in the overworld's tick after a command block of that tick sent the host to the
/// Nether brings the host back to the overworld, like the game. Before: the teleport was taken for
/// one within the host's world (a position packet to a client showing the Nether), then the
/// host's Nether position was put back at the end of the overworld's tick.
#[test]
fn a_teleport_after_the_host_left_its_world_brings_it_back() {
    let (mut p, _dir) = new_world("teleport-after-host-left");
    let _ = host_sent_to_the_nether_then(&mut p, "tp @p 0.5 -60 4.5");
    assert_eq!(p.s.client_world, OVERWORLD, "the host was not brought back to the overworld");
    assert_eq!(p.s.dim.name, OVERWORLD, "the server does not work in the host's world");
    assert_eq!(p.s.pos, [0.5, -60.0, 4.5], "the host is not where the teleport sent it");
}

/// The host stands in the overworld at (0.5, -60, 0.5); the Nether's chunk (0, 0) is force-loaded
/// and a command block there runs `command` in the Nether's next tick, nobody standing there.
fn forced_nether_command_block(p: &mut Probe, command: &str) {
    let _ = run(p, "execute in minecraft:the_nether run forceload add 0 0");
    tick_until_loaded_around(p, NETHER, (0, 0));
    p.s.pos = [0.5, -60.0, 0.5];
    command_block(p, NETHER, [1, 99, 1], command);
    step(p, 2);
}

/// A command block of a force-loaded Nether nobody stands in places relative to the host where it
/// stands, in its overworld, like the game. Before: the host, hidden from the Nether's tick, was
/// read far out of the world and the gold block went nowhere.
#[test]
fn a_command_of_a_world_the_host_is_absent_from_finds_it_where_it_stands() {
    let (mut p, _dir) = new_world("absent-host-found");
    forced_nether_command_block(&mut p, "execute at @a run setblock ~ ~2 ~ minecraft:gold_block");
    assert_eq!(block_name(&mut p, OVERWORLD, [0, -58, 0]), "minecraft:gold_block", "the host was not found where it stands");
}

/// `tp @a` from a command block of a force-loaded Nether takes the host standing in the overworld
/// to the Nether, like the game (the command's world), and the server goes on in the host's world.
/// Before: the teleport was taken for one within the Nether, and the host's overworld position was
/// put back at the end of the Nether's tick.
#[test]
fn a_teleport_from_a_world_the_host_is_absent_from_takes_it_there() {
    let (mut p, _dir) = new_world("absent-host-teleported");
    forced_nether_command_block(&mut p, "tp @a 0.5 100 0.5");
    assert_eq!(p.s.client_world, NETHER, "the host was not taken to the Nether");
    assert_eq!(p.s.dim.name, NETHER, "the server does not work in the host's world");
    assert_eq!(p.s.pos, [0.5, 100.0, 0.5], "the host is not where the teleport sent it");
}

/// `execute in the_nether run tp @s …` typed by a guest takes the guest alone to the Nether: its
/// client is told (`respawn`), the server keeps working in the host's overworld. Before: the
/// server's world moved to the Nether under the host.
#[test]
fn a_guest_sent_to_another_world_by_its_own_command_goes_alone() {
    let (mut p, _dir) = new_world("guest-execute-in-tp");
    let (guest, _keep) = add_guest(&mut p, [0.5, -60.0, 0.5]);
    let _ = guest.try_iter().count();
    assert!(run_as_guest(&mut p, "execute in minecraft:the_nether run tp @s 0.5 80 0.5"), "command refused");
    assert_eq!(guest_ctx(&p.s).client_world, NETHER, "the guest is not in the Nether");
    assert_eq!(guest_ctx(&p.s).pos, [0.5, 80.0, 0.5]);
    assert!(received(&guest, p.s.peer.ids().respawn), "the guest's client was not told");
    assert_eq!(p.s.dim.name, OVERWORLD, "the server's world moved under the host");
    assert_eq!(p.s.client_world, OVERWORLD, "the host's client changed world");
    p.s.lan_close();
}

/// A guest standing in the Nether while the host stands in the overworld runs its own commands
/// in the Nether, around itself, like the game. Before: they ran in the host's world.
#[test]
fn a_guests_own_commands_run_in_its_world() {
    let (mut p, _dir) = new_world("guest-own-commands");
    let (_guest, _keep) = add_guest(&mut p, [0.5, -60.0, 0.5]);
    load_around(&mut p, NETHER, (0, 0));
    let ctx = guest_ctx_mut(&mut p.s);
    ctx.client_world = NETHER.to_owned();
    ctx.pos = [0.5, 100.0, 0.5];
    assert!(run_as_guest(&mut p, "setblock ~ ~ ~ minecraft:gold_block"), "command refused");
    assert_eq!(block_name(&mut p, NETHER, [0, 100, 0]), "minecraft:gold_block", "not placed in the guest's world");
    assert_ne!(block_name(&mut p, OVERWORLD, [0, 100, 0]), "minecraft:gold_block", "placed in the host's world");
    p.s.lan_close();
}

/// A guest who respawns in another world (dead in the Nether, back at the overworld spawn) goes
/// there alone; the server keeps working in the host's world. Before: the respawn moved the
/// server's world under the host.
#[test]
fn a_guest_respawned_in_another_world_goes_alone() {
    let (mut p, _dir) = new_world("guest-respawn-elsewhere");
    let (_guest, _keep) = add_guest(&mut p, [0.5, -60.0, 0.5]);
    host_to_nether(&mut p);
    guest_ctx_mut(&mut p.s).client_world = NETHER.to_owned();
    p.s.as_player(Some(0), |s| {
        s.life.vit.dead = true;
        s.perform_respawn();
    });
    assert_eq!(guest_ctx(&p.s).client_world, OVERWORLD, "the guest did not respawn in the overworld");
    assert_eq!(p.s.dim.name, NETHER, "the server's world moved under the host");
    assert_eq!(p.s.client_world, NETHER, "the host's client changed world");
    p.s.lan_close();
}

/// A guest standing in a lit Nether portal goes through it like the host: it reaches the Nether
/// alone (its client told), the host's overworld stays the server's world. Before: guests had no
/// portal.
#[test]
fn a_guest_goes_through_a_nether_portal_alone() {
    let (mut p, _dir) = new_world("guest-portal");
    let (guest, _keep) = add_guest(&mut p, [5.0, -60.0, 0.5]);
    // Obsidian frame 4 × 5 standing on the ground, lit by a fire inside.
    let _ = run(&mut p, "fill 3 -61 0 6 -57 0 minecraft:obsidian");
    let _ = run(&mut p, "fill 4 -60 0 5 -58 0 minecraft:air");
    let _ = run(&mut p, "setblock 4 -60 0 minecraft:fire");
    assert_eq!(block_name(&mut p, OVERWORLD, [5, -59, 0]), "minecraft:nether_portal", "setup: portal lit");
    guest_ctx_mut(&mut p.s).game_mode = 1; // creative: no portal delay
    let _ = guest.try_iter().count();
    step(&mut p, 3);
    assert_eq!(guest_ctx(&p.s).client_world, NETHER, "the guest did not go through the portal");
    assert!(received(&guest, p.s.peer.ids().respawn), "the guest's client was not told");
    assert_eq!(p.s.dim.name, OVERWORLD, "the server's world moved under the host");
    assert_eq!(p.s.client_world, OVERWORLD, "the host's client changed world");
    p.s.lan_close();
}

/// A guest standing in the Nether does not count among the overworld's sleepers, like the game's
/// per-world sleep status: the host asleep alone in the overworld is all of its players. Before:
/// every player counted, the guest away kept the night from passing.
#[test]
fn a_guest_in_another_world_does_not_count_among_the_sleepers() {
    let (mut p, _dir) = new_world("guest-away-sleep");
    let (_guest, _keep) = add_guest(&mut p, [0.5, -60.0, 0.5]);
    p.s.life.vit.sleeping = Some([0, -60, 0]);
    p.s.update_sleeping_list();
    assert_eq!(p.s.life.weather.sleep, (2, 1), "setup: both players in the overworld");
    guest_ctx_mut(&mut p.s).client_world = NETHER.to_owned();
    p.s.update_sleeping_list();
    assert_eq!(p.s.life.weather.sleep, (1, 1), "the guest in the Nether counted among the overworld's players");
    p.s.lan_close();
}

fn overworld_clock(p: &Probe) -> i64 {
    p.s.clocks.iter().find(|c| c.0 == "minecraft:overworld").map_or(-1, |c| c.1)
}

/// A bed at (3, -60, 3) in the overworld, and the night there.
fn bed_at_night(p: &mut Probe) {
    let _ = run(p, "execute in minecraft:overworld run setblock 3 -60 3 minecraft:red_bed[part=head]");
    let _ = run(p, "execute in minecraft:overworld run time set 18000");
}

/// The installed player lies in the bed of `bed_at_night` for 100 ticks already (deep sleep).
fn asleep_in_bed(s: &mut Server) {
    s.life.vit.sleeping = Some([3, -60, 3]);
    s.life.vit.sleep_timer = 100;
    s.update_sleeping_list();
}

/// A guest asleep alone in the overworld while the host stands in the Nether passes the
/// overworld's night, like the game where each world decides its own night among its own players;
/// only the overworld's players are told. Before: the guest's count announced the night skip to
/// everyone, the host included, while the decision, taken among the host's Nether players, never
/// came.
#[test]
fn a_guest_asleep_alone_in_its_world_passes_its_night() {
    let (mut p, _dir) = new_world("guest-sleeps-away");
    let (_guest, _keep) = add_guest(&mut p, [0.5, -60.0, 0.5]);
    bed_at_night(&mut p);
    host_to_nether(&mut p);
    let _ = p.take_events();
    p.s.as_player(Some(0), asleep_in_bed);
    step(&mut p, 1);
    assert_eq!(overworld_clock(&p).rem_euclid(24000), 0, "the overworld's night did not pass");
    assert!(guest_ctx(&p.s).vit.sleeping.is_none(), "the guest stayed in bed");
    let host_told = p.take_events().iter().any(|e| matches!(e, rc_client::Event::ActionBar(_)));
    assert!(!host_told, "the host in the Nether was told about the overworld's sleepers");
    p.s.lan_close();
}

/// The host asleep alone in the overworld passes its night although a guest standing in the Nether
/// changes game mode: that change counts the Nether's players only. Before: one count for the
/// server, which the guest's change set to the Nether's (one player, nobody in bed), and the
/// host's night never passed.
#[test]
fn a_guest_elsewhere_changing_mode_leaves_the_hosts_night_alone() {
    let (mut p, _dir) = new_world("guest-mode-away");
    let (_guest, _keep) = add_guest(&mut p, [0.5, -60.0, 0.5]);
    let ctx = guest_ctx_mut(&mut p.s);
    ctx.client_world = NETHER.to_owned();
    ctx.pos = [0.5, 100.0, 0.5];
    p.s.player_enters_active_world(); // the players' worlds noted apart
    bed_at_night(&mut p);
    asleep_in_bed(&mut p.s);
    assert!(run_as_guest(&mut p, "gamemode creative"), "command refused");
    step(&mut p, 1);
    assert_eq!(overworld_clock(&p).rem_euclid(24000), 0, "the host's night did not pass");
    assert!(p.s.life.vit.sleeping.is_none(), "the host stayed in bed");
    p.s.lan_close();
}

/// The host turning spectator by a command run in the Nether (`execute in`) while it stands in the
/// overworld refreshes the overworld's sleepers, like the game where a mode change updates the
/// world the player stands in: the guest asleep there is then all of its active players and the
/// night passes. Before: the refresh went to the Nether, where nobody stands, and the overworld
/// kept needing two sleepers.
#[test]
fn the_hosts_mode_changed_from_another_world_refreshes_its_own_worlds_sleepers() {
    let (mut p, _dir) = new_world("host-mode-from-nether");
    let (_guest, _keep) = add_guest(&mut p, [0.5, -60.0, 0.5]);
    bed_at_night(&mut p);
    p.s.as_player(Some(0), asleep_in_bed);
    assert_eq!(p.s.life.weather.sleep, (2, 1), "setup: both players in the overworld, the guest asleep");
    let mut host = p.s.player_source();
    host.level = 4;
    let (changed, _) = p.s.perform(host, "execute in minecraft:the_nether run gamemode spectator");
    assert!(changed && p.s.dim.name == OVERWORLD, "setup: mode not changed, or the host moved");
    assert_eq!(p.s.life.weather.sleep, (1, 1), "the overworld's sleepers were not refreshed");
    step(&mut p, 1);
    assert_eq!(overworld_clock(&p).rem_euclid(24000), 0, "the overworld's night did not pass");
    p.s.lan_close();
}

/// A guest leaving the game from the Nether is taken from the view of the other guest standing
/// there, like the game's removal from its world. Before: the removal was sent in the host's
/// overworld, the other guest kept the departed player in sight.
#[test]
fn a_guest_leaving_from_another_world_is_removed_for_the_players_there() {
    let (mut p, _dir) = new_world("guest-leaves-nether");
    let (_first, _keep_first) = add_guest(&mut p, [0.5, -60.0, 0.5]);
    let (second, _keep_second) =
        crate::mobs::tests_spawn_paths::add_guest_with_packets(&mut p.s, "Second", [1.5, -60.0, 0.5]);
    let ids = *p.s.peer.ids();
    for g in p.s.lan.as_mut().expect("LAN open").guests.iter_mut() {
        g.ctx.as_mut().expect("guest state").client_world = NETHER.to_owned();
    }
    p.s.player_enters_active_world(); // the players' worlds noted apart
    let leaving = guest_ctx(&p.s).player_id;
    let _ = second.try_iter().count();
    p.s.lan_leave(0);
    assert!(added_and_removed(&second, &ids).1.contains(&leaving), "the other guest of the Nether was not told");
    p.s.lan_close();
}

/// A guest who left the game in the Nether enters it again in the Nether, where its file says,
/// like the game: its file records the world it stood in, and its entry reads it back. Before: the
/// file recorded the host's world and a guest always entered the host's.
#[test]
fn a_guest_enters_the_game_in_the_world_of_its_file() {
    let (mut p, _dir) = new_world("guest-login-world");
    let (_guest, _keep) = add_guest(&mut p, [0.5, -60.0, 0.5]);
    load_around(&mut p, NETHER, (0, 0));
    let ctx = guest_ctx_mut(&mut p.s);
    ctx.client_world = NETHER.to_owned();
    ctx.pos = [0.5, 100.0, 0.5];
    let (path, saved) = p.s.as_player(Some(0), |s| (s.pdata.path.clone(), s.player_nbt()));
    std::fs::create_dir_all(path.parent().expect("players folder")).expect("players folder");
    std::fs::write(&path, crate::nbtio::gzip(&crate::nbtio::encode_root(&saved))).expect("player file");
    let uuid = guest_ctx(&p.s).uuid;
    let back = p.s.new_player_ctx("Guest", uuid, 2, 0, None);
    assert_eq!(back.client_world, NETHER, "entered in another world than its file's");
    assert_eq!(back.pos, [0.5, 100.0, 0.5]);
    p.s.lan_close();
}
