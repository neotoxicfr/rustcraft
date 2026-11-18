//! Equivalence proofs of the optimised spawn paths (`SpawnPaths`): each scenario is played on the
//! former paths and on the new ones, each run in its own thread (same test random sequence), and
//! its tick-by-tick fingerprint compared; the structure spawn lists are compared position by
//! position and their fingerprint frozen.

use std::path::PathBuf;

use rc_proto::nbt::Nbt;

use super::spawn::SpawnPaths;
use super::species::{Category, Kind};
use crate::blocks::probe::{Probe, state_string};
use crate::nbtio;
use crate::player::Stack;

const TRIAL: [i32; 3] = [216, -59, 212];
/// In the chunk north of the trial spawner's, 2 blocks from the player when near.
const VAULT: [i32; 3] = [218, -59, 207];
const NEAR: &str = "player 216.5 -59 206.5 0 0\n";
const FAR: &str = "player 216.5 -59 231.5 0 0\n";

pub(crate) fn open(tag: &str) -> Option<(Probe, PathBuf)> {
    let (world, data) = (crate::tests::test_world()?, rc_testdata::version_data()?);
    let dir = super::tests::prep(&world, tag, 0, "normal", &[]);
    let mut p = Probe::open(&dir, std::sync::Arc::new(data));
    p.run("fill 200 -60 200 231 -60 231 minecraft:stone\nfill 200 -59 200 231 -40 231 minecraft:air\n");
    p.run(FAR);
    assert_eq!(state_string(&p.s, p.s.get([216, -60, 216])), "minecraft:stone", "area 200..231 missing from the copy");
    p.s.game_mode = 0;
    p.s.loaded = true;
    p.s.view = 8;
    Some((p, dir))
}

/// Runs `scenario` with `paths` in a fresh thread (the test seeds are per thread).
fn play<T: Send + 'static>(paths: SpawnPaths, scenario: fn() -> Option<T>) -> Option<T> {
    std::thread::spawn(move || paths.run(scenario)).join().expect("scenario thread")
}

/// FNV-1a.
fn mix(h: &mut u64, text: &str) {
    for b in text.bytes() {
        *h ^= u64::from(b);
        *h = h.wrapping_mul(0x0100_0000_01b3);
    }
}

fn fresh_hash() -> u64 {
    0xcbf2_9ce4_8422_2325
}

/// Same fingerprints tick by tick, otherwise the first differing tick.
fn assert_same_ticks(label: &str, old: &[u64], new: &[u64]) {
    assert_eq!(old.len(), new.len(), "{label}: tick counts");
    if let Some(i) = old.iter().zip(new).position(|(a, b)| a != b) {
        panic!("{label}: first difference at tick {i}");
    }
}

fn cmd(p: &mut Probe, c: &str) {
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, c);
    assert!(ok, "command refused: {c}");
}

/// Block entity, block state, mobs (identity, place, removal) and world random of the tick.
fn mix_world(h: &mut u64, p: &Probe, at: &[[i32; 3]]) {
    for &q in at {
        mix(h, &format!("{:?}|{}", p.s.be_get(q), state_string(&p.s, p.s.get(q))));
    }
    for m in p.s.mobs.list.iter() {
        mix(h, &format!("{}|{:?}|{:?}|{}", m.uuid, m.kind, m.pos(), m.removed));
    }
    mix(h, &format!("{:?}", p.s.mobs.rng));
}

/// One block tick (`Probe::tick` with its events kept) and the dirty flags it left on the chunks
/// of `watched` (cleared before).
fn tick_watching(p: &mut Probe, watched: &[(i32, i32)]) -> (Vec<rc_client::Event>, Vec<bool>) {
    for c in watched {
        if let Some(Some(ch)) = p.s.chunks.get_mut(c) {
            ch.dirty = false;
        }
    }
    p.s.time += 1;
    for c in &mut p.s.clocks {
        c.1 += 1;
    }
    p.s.tick_world_blocks();
    p.s.flush_block_changes();
    p.s.ominous_items_tick();
    let dirty =
        watched.iter().map(|c| p.s.chunks.get(c).is_some_and(|ch| ch.as_ref().is_some_and(|ch| ch.dirty))).collect();
    (p.take_events(), dirty)
}

fn zombie_config(total: f32) -> Nbt {
    let zombie = Nbt::Compound(vec![(
        "entity".into(),
        Nbt::Compound(vec![("id".into(), Nbt::String("minecraft:zombie".into()))]),
    )]);
    Nbt::Compound(vec![
        (
            "spawn_potentials".into(),
            Nbt::List(vec![Nbt::Compound(vec![("data".into(), zombie), ("weight".into(), Nbt::Int(1))])]),
        ),
        ("total_mobs".into(), Nbt::Float(total)),
        ("simultaneous_mobs".into(), Nbt::Float(1.0)),
        ("ticks_between_spawn".into(), Nbt::Int(20)),
    ])
}

/// What the trial chamber scenario leaves: per-tick fingerprints, and the ticks that marked the
/// trial spawner's chunk for saving (total, and on ticks 10 to 99, nobody near yet).
struct ChamberRun {
    ticks: Vec<u64>,
    trial_chunk_dirty: usize,
    trial_chunk_dirty_idle: usize,
}

/// Trial chamber over 2,000 ticks: trial spawner (normal then ominous configuration) and vault,
/// a player who comes, leaves and comes back with Bad Omen, mobs killed, a key inserted, a
/// `/data merge` in a non-normalized form while the fight goes on.
fn chamber_scenario() -> Option<ChamberRun> {
    let tag = format!("spawn-paths-chamber-{}", SpawnPaths::current() == SpawnPaths::NEW);
    let (mut p, _dir) = open(&tag)?;
    nbtio::set(&mut p.s.rules, "minecraft:spawn_mobs", Nbt::Byte(1));
    p.run("setblock 216 -59 212 minecraft:trial_spawner\nsetblock 218 -59 207 minecraft:vault\n");
    let be = p.s.be_get_mut(TRIAL).expect("trial spawner block entity");
    nbtio::set(be, "normal_config", zombie_config(2.0));
    nbtio::set(be, "ominous_config", zombie_config(3.0));
    let watched = [(TRIAL[0] >> 4, TRIAL[2] >> 4), (VAULT[0] >> 4, VAULT[2] >> 4)];
    let mut run = ChamberRun { ticks: Vec::new(), trial_chunk_dirty: 0, trial_chunk_dirty_idle: 0 };
    let mut near = false;
    for t in 0..2000 {
        match t {
            100 | 900 => {
                p.run(NEAR);
                near = true;
            }
            700 | 1600 => {
                p.run(FAR);
                near = false;
            }
            300 => cmd(&mut p, "data merge block 216 -59 212 {next_mob_spawns_at:5}"),
            350 => {
                let slot = 36 + p.s.held as usize;
                p.s.slots[slot] = Some(Stack { id: "minecraft:trial_key".into(), count: 1, components: None });
                let _ = p.s.vault_use_item(VAULT, slot);
            }
            880 => {
                let _ = p.s.add_effect(crate::effects::Inst::new("minecraft:bad_omen", 4000, 1));
            }
            _ => {}
        }
        if t % 40 == 0 {
            for m in p.s.mobs.list.iter_mut() {
                m.removed = true;
            }
        }
        let (events, dirty) = tick_watching(&mut p, &watched);
        let mut h = fresh_hash();
        mix_world(&mut h, &p, &[TRIAL, VAULT]);
        for e in &events {
            mix(&mut h, &format!("{e:?}"));
        }
        // The vault's chunk: marked on the very same ticks (the trial spawner's differs by design).
        mix(&mut h, if dirty[1] { "vault chunk dirty" } else { "vault chunk clean" });
        run.ticks.push(h);
        if dirty[0] {
            run.trial_chunk_dirty += 1;
            if !near && (10..100).contains(&t) {
                run.trial_chunk_dirty_idle += 1;
            }
        }
    }
    Some(run)
}

/// Trial spawner and vault: same blocks, block entities, packets, mobs and random draws on every
/// tick with each new path, alone or together; the idle trial spawner no longer marks its chunk.
#[test]
fn trial_chamber_paths_match() {
    let Some(old) = play(SpawnPaths::OLD, chamber_scenario) else { return };
    let new = play(SpawnPaths::NEW, chamber_scenario).expect("same data");
    assert_same_ticks("trial chamber, every new path", &old.ticks, &new.ticks);
    for (label, paths) in [
        ("former write", SpawnPaths { trial_write_on_change: false, ..SpawnPaths::NEW }),
        ("former trial read", SpawnPaths { trial_borrow: false, ..SpawnPaths::NEW }),
        ("former vault exit", SpawnPaths { vault_early_exit: false, ..SpawnPaths::NEW }),
        ("former vault read", SpawnPaths { vault_borrow: false, ..SpawnPaths::NEW }),
    ] {
        let alone = play(paths, chamber_scenario).expect("same data");
        assert_same_ticks(label, &old.ticks, &alone.ticks);
    }
    eprintln!(
        "trial chunk marked on {} ticks (former) / {} (new), idle {} / {}",
        old.trial_chunk_dirty, new.trial_chunk_dirty, old.trial_chunk_dirty_idle, new.trial_chunk_dirty_idle
    );
    assert_eq!(old.trial_chunk_dirty, 2000, "former path: the chunk marked on every tick");
    assert_eq!(new.trial_chunk_dirty_idle, 0, "waiting with nobody near: the chunk stays clean");
    assert!(new.trial_chunk_dirty < 400, "marked only on changes: {}", new.trial_chunk_dirty);
}

/// Monster spawner over 1,000 ticks: player out of range, then in range (short delays, several
/// spawns), killed mobs: same block entity, packets, mobs, random draws and chunk flags.
fn cage_scenario() -> Option<Vec<u64>> {
    let tag = format!("spawn-paths-cage-{}", SpawnPaths::current() == SpawnPaths::NEW);
    let (mut p, _dir) = open(&tag)?;
    p.run("setblock 216 -59 212 minecraft:spawner\n");
    cmd(
        &mut p,
        "data merge block 216 -59 212 {MinSpawnDelay:20s,MaxSpawnDelay:40s,SpawnCount:2s,RequiredPlayerRange:12s,\
         SpawnData:{entity:{id:\"minecraft:zombie\"},custom_spawn_rules:{block_light_limit:{min_inclusive:0,\
         max_inclusive:15},sky_light_limit:{min_inclusive:0,max_inclusive:15}}}}",
    );
    let watched = [(TRIAL[0] >> 4, TRIAL[2] >> 4)];
    let mut ticks = Vec::new();
    for t in 0..1000 {
        match t {
            300 => p.run(NEAR),
            700 => p.run("player 216.5 -59 224.5 0 0\n"),
            850 => p.run(NEAR),
            _ => {}
        }
        if t % 100 == 0 {
            for m in p.s.mobs.list.iter_mut() {
                m.removed = true;
            }
        }
        let (events, dirty) = tick_watching(&mut p, &watched);
        let mut h = fresh_hash();
        mix_world(&mut h, &p, &[TRIAL]);
        for e in &events {
            mix(&mut h, &format!("{e:?}"));
        }
        mix(&mut h, &format!("{dirty:?}"));
        ticks.push(h);
    }
    let spawned = p.s.mobs.list.iter().filter(|m| m.kind == Kind::Zombie).count();
    assert!(spawned >= 4, "the spawner fired several times ({spawned} zombies)");
    Some(ticks)
}

#[test]
fn monster_spawner_paths_match() {
    let Some(old) = play(SpawnPaths::OLD, cage_scenario) else { return };
    let new = play(SpawnPaths::NEW, cage_scenario).expect("same data");
    assert_same_ticks("monster spawner, every new path", &old, &new);
    let range_only = play(SpawnPaths { spawner_borrow: false, ..SpawnPaths::NEW }, cage_scenario).expect("same data");
    assert_same_ticks("monster spawner, range first only", &old, &range_only);
}

/// Spawner minecart over 1,000 ticks, same moves of the player as the block's scenario: its
/// countdown lives in the minecart's own data, written in place on the new path.
fn cart_cage_scenario() -> Option<Vec<u64>> {
    let tag = format!("spawn-paths-cart-cage-{}", SpawnPaths::current() == SpawnPaths::NEW);
    let (mut p, _dir) = open(&tag)?;
    cmd(
        &mut p,
        "summon minecraft:spawner_minecart 216.5 -59 212.5 {MinSpawnDelay:20s,MaxSpawnDelay:40s,SpawnCount:2s,\
         RequiredPlayerRange:12s,SpawnData:{entity:{id:\"minecraft:zombie\"},custom_spawn_rules:{block_light_limit:\
         {min_inclusive:0,max_inclusive:15},sky_light_limit:{min_inclusive:0,max_inclusive:15}}}}",
    );
    let mut ticks = Vec::new();
    for t in 0..1000 {
        match t {
            300 => p.run(NEAR),
            700 => p.run("player 216.5 -59 224.5 0 0\n"),
            850 => p.run(NEAR),
            _ => {}
        }
        if t % 100 == 0 {
            for m in p.s.mobs.list.iter_mut() {
                m.removed = true;
            }
        }
        p.s.time += 1;
        p.s.minecarts_tick();
        let mut h = fresh_hash();
        for c in p.s.carts.list.iter().flatten() {
            mix(&mut h, &format!("{:?}|{:?}", c.nbt, c.pos));
        }
        mix_world(&mut h, &p, &[]);
        for e in &p.take_events() {
            mix(&mut h, &format!("{e:?}"));
        }
        ticks.push(h);
    }
    let spawned = p.s.mobs.list.iter().filter(|m| m.kind == Kind::Zombie).count();
    assert!(spawned >= 4, "the spawner minecart fired several times ({spawned} zombies)");
    Some(ticks)
}

#[test]
fn spawner_minecart_paths_match() {
    let Some(old) = play(SpawnPaths::OLD, cart_cage_scenario) else { return };
    let new = play(SpawnPaths::NEW, cart_cage_scenario).expect("same data");
    assert_same_ticks("spawner minecart, every new path", &old, &new);
    let range_only =
        play(SpawnPaths { spawner_borrow: false, ..SpawnPaths::NEW }, cart_cage_scenario).expect("same data");
    assert_same_ticks("spawner minecart, range first only", &old, &range_only);
}

/// Opens the world to the LAN and adds a guest without a connection (state installed).
pub(crate) fn add_guest(
    s: &mut crate::server::Server,
    name: &str,
    pos: [f64; 3],
) -> crossbeam_channel::Sender<Vec<u8>> {
    add_guest_with_packets(s, name, pos).1
}

/// [`add_guest`], also handing back what the guest is sent.
pub(crate) fn add_guest_with_packets(
    s: &mut crate::server::Server,
    name: &str,
    pos: [f64; 3],
) -> (crossbeam_channel::Receiver<Vec<u8>>, crossbeam_channel::Sender<Vec<u8>>) {
    crate::lan::announce::LOCAL_ONLY.store(true, std::sync::atomic::Ordering::Relaxed);
    if s.lan.is_none() {
        let req = crate::lan::Publish { open: true, port: 0, guest_commands: false, force_mode: Some(0) };
        s.lan_publish(&req).expect("opening");
    }
    let uuid = rc_client::offline_uuid(name);
    let mut ctx = s.new_player_ctx(name, uuid, 2, 0, Some(0));
    ctx.loaded = true;
    ctx.pos = pos;
    let (out, from_server) = crossbeam_channel::unbounded();
    let (keep, inp) = crossbeam_channel::unbounded();
    s.lan.as_mut().expect("LAN open").guests.push(crate::lan::Guest {
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

/// World random seed whose first draws strike lightning on the first spawn chunk and turn it
/// into a skeleton horse trap (first bounded draw out of 100,000 at 0, then a draw under 1 %).
fn forced_trap_seed() -> i64 {
    (0i64..)
        .find(|&s| {
            let mut r = crate::blocks::JavaRandom::new(s);
            r.next_int_bound(100_000) == 0 && r.next_double() < 0.01
        })
        .expect("seed")
}

/// Natural spawns over 2,000 ticks with a LAN guest whose spawn square overlaps the host's: day,
/// night, a thunderstorm with a trap forced on every tick, spawns cut for a while (no category:
/// no charges gathered). Fingerprint: world random, mobs, census of each tick.
fn natural_scenario() -> Option<Vec<u64>> {
    let tag = format!("spawn-paths-natural-{}", SpawnPaths::current() == SpawnPaths::NEW);
    let (mut p, _dir) = open(&tag)?;
    p.run(NEAR);
    nbtio::set(&mut p.s.rules, "minecraft:spawn_mobs", Nbt::Byte(1));
    // Structures (lists that change from one position of a group to the next) and a biome with
    // spawn costs (the charges matter) where the groups are drawn.
    write_structures(&mut p);
    cmd(&mut p, "fillbiome 176 -64 192 239 -57 239 minecraft:soul_sand_valley");
    let _guest = add_guest(&mut p.s, "Guest", [264.5, -59.0, 206.5]);
    let seed = forced_trap_seed();
    let mut ticks = Vec::new();
    let mut traps = 0;
    for t in 0..2000 {
        match t {
            0 => cmd(&mut p, "time set day"),
            400 => cmd(&mut p, "time set midnight"),
            900 => cmd(&mut p, "weather thunder"),
            1300 => cmd(&mut p, "weather clear"),
            1500 => nbtio::set(&mut p.s.rules, "minecraft:spawn_mobs", Nbt::Byte(0)),
            1600 => nbtio::set(&mut p.s.rules, "minecraft:spawn_mobs", Nbt::Byte(1)),
            1700 => cmd(&mut p, "time set day"),
            _ => {}
        }
        // The weather levels move in the full server tick, not played here: set at once.
        let storm = (900..1300).contains(&t);
        p.s.life.weather.rain_level = if storm { 1.0 } else { 0.0 };
        p.s.life.weather.thunder_level = if storm { 1.0 } else { 0.0 };
        if (1000..1300).contains(&t) {
            p.s.bs.rng = crate::blocks::JavaRandom::new(seed);
        }
        p.s.time += 1;
        for c in &mut p.s.clocks {
            c.1 += 1;
        }
        let before = p.s.mobs.list.iter().filter(|m| m.mount.trap).count();
        p.s.mobs_world_tick();
        traps += p.s.mobs.list.iter().filter(|m| m.mount.trap).count().saturating_sub(before);
        let mut h = fresh_hash();
        mix_world(&mut h, &p, &[]);
        let census = p.s.mobs.census.as_ref().map(|c| (c.chunks, c.counts));
        mix(&mut h, &format!("{census:?}|{:?}", p.s.bs.rng));
        ticks.push(h);
    }
    let census = p.s.mobs.census.as_ref().map(|c| c.chunks);
    assert_eq!(census, Some(289 + 3 * 17), "two overlapping spawn squares");
    assert!(traps > 0, "skeleton horse traps placed during the thunderstorm");
    assert!(p.s.mobs.list.iter().any(|m| m.kind.is_monster()), "monsters spawned at night");
    let costly = |k: Kind| matches!(k, Kind::Skeleton | Kind::Ghast | Kind::Enderman | Kind::Strider);
    assert!(p.s.mobs.list.iter().any(|m| costly(m.kind)), "species with a spawn cost spawned");
    Some(ticks)
}

#[test]
fn natural_spawn_paths_match() {
    let Some(old) = play(SpawnPaths::OLD, natural_scenario) else { return };
    let new = play(SpawnPaths::NEW, natural_scenario).expect("same data");
    assert_same_ticks("natural spawns, every new path", &old, &new);
    for (label, paths) in [
        ("census only", SpawnPaths { structure_starts: false, ..SpawnPaths::NEW }),
        ("structure starts only", SpawnPaths { spawn_census: false, ..SpawnPaths::NEW }),
        ("list first", SpawnPaths { placement_first: false, ..SpawnPaths::NEW }),
        ("placement first, former lists", SpawnPaths { placement_first: true, ..SpawnPaths::OLD }),
    ] {
        let alone = play(paths, natural_scenario).expect("same data");
        assert_same_ticks(label, &old, &alone);
    }
}

/// The spawn chunk count without a set: 289 per lone center, the exact union otherwise.
#[test]
fn spawn_chunk_count_matches_the_set() {
    let cases: [&[(i32, i32)]; 5] =
        [&[], &[(0, 0)], &[(0, 0), (0, 0)], &[(0, 0), (5, -3), (16, 0)], &[(-40, 7), (-33, 7), (-40, 20), (100, 100)]];
    for centers in cases {
        assert_eq!(
            super::spawn::spawn_chunk_count(centers),
            super::spawn::spawn_chunk_count_with_set(centers),
            "{centers:?}"
        );
    }
    assert_eq!(super::spawn::spawn_chunk_count(&[(3, 3)]), 289);
}

/// The cost key is the species name itself: every name carries its namespace.
#[test]
fn species_names_carry_their_namespace() {
    for k in Kind::ALL {
        assert!(k.info().name.contains(':'), "{k:?}: {}", k.info().name);
    }
}

/// Memoized spawn costs = the pack's, for every biome of the registry and every species, read
/// in the order of a pass (hits after misses).
#[test]
fn memoized_spawn_costs_match_the_pack() {
    let Some((mut p, _dir)) = open("spawn-costs") else { return };
    let at = [216, -59, 216];
    let biomes = p.s.regs.entries("minecraft:worldgen/biome").to_vec();
    assert!(biomes.len() > 50, "biome registry");
    let mut costs_found = 0;
    for b in &biomes {
        // The eight cells the zoom may pick for the block read below.
        cmd(&mut p, &format!("fillbiome 212 -64 212 219 -57 219 {b}"));
        let mut memo = super::spawn::SpawnCostMemo::default();
        for _ in 0..2 {
            for k in Kind::ALL {
                let pack = p.s.spawn_cost(k, at);
                assert_eq!(p.s.spawn_cost_memo(&mut memo, k, at), pack, "{b} {k:?}");
                costs_found += usize::from(pack.is_some());
            }
        }
    }
    assert!(costs_found > 0, "some biome has spawn costs (soul sand valley…)");
}

/// Synthetic starts around the loaded area: a fortress of 300 pieces on Nether bricks, an outpost
/// (box grown by 12), a witch hut (piece lists), an `INVALID` start, a start without a readable
/// box, a reference to a start chunk that is not loaded.
pub(crate) fn write_structures(p: &mut Probe) {
    let key = |cx: i32, cz: i32| (i64::from(cx) & 0xFFFF_FFFF) | (i64::from(cz) << 32);
    let piece = |bb: [i32; 6]| {
        Nbt::Compound(vec![("id".into(), Nbt::String("p".into())), ("BB".into(), Nbt::IntArray(bb.to_vec()))])
    };
    let start = |id: &str, children: Vec<Nbt>| {
        Nbt::Compound(vec![("id".into(), Nbt::String(id.into())), ("Children".into(), Nbt::List(children))])
    };
    let fortress: Vec<Nbt> = (0..300)
        .map(|i: i32| {
            let (x, z, y) = (194 + (i * 7) % 36, 194 + (i * 13) % 36, -60 + i % 6);
            piece([x, y, z, x + 2 + i % 5, y + 3 + i % 4, z + 2 + (i * 3) % 6])
        })
        .collect();
    let starts = [
        ((12, 12), "minecraft:fortress", start("minecraft:fortress", fortress)),
        (
            (13, 13),
            "minecraft:pillager_outpost",
            start("minecraft:pillager_outpost", vec![piece([216, -60, 216, 220, -40, 220])]),
        ),
        ((14, 12), "minecraft:swamp_hut", start("minecraft:swamp_hut", vec![piece([226, -60, 196, 230, -52, 202])])),
        ((12, 13), "minecraft:desert_pyramid", start("INVALID", vec![piece([200, -60, 210, 206, -50, 214])])),
        ((13, 12), "minecraft:igloo", start("minecraft:igloo", vec![Nbt::Compound(vec![("BB".into(), Nbt::Int(5))])])),
    ];
    let mut refs: Vec<(String, Nbt)> =
        starts.iter().map(|(c, n, _)| ((*n).to_owned(), Nbt::LongArray(vec![key(c.0, c.1)]))).collect();
    refs.push(("minecraft:ruined_portal".into(), Nbt::LongArray(vec![key(60, 60)])));
    for cx in 11..=15 {
        for cz in 11..=15 {
            let Some(Some(ch)) = p.s.chunks.get_mut(&(cx, cz)) else { continue };
            let own: Vec<(String, Nbt)> =
                starts.iter().filter(|(c, ..)| *c == (cx, cz)).map(|(_, n, s)| ((*n).to_owned(), s.clone())).collect();
            let st = Nbt::Compound(vec![
                ("References".into(), Nbt::Compound(refs.clone())),
                ("starts".into(), Nbt::Compound(own)),
            ]);
            nbtio::set(&mut ch.root, "structures", st);
        }
    }
    p.run("fill 194 -60 194 215 -60 231 minecraft:nether_bricks\n");
}

/// Positions of the comparison: a fixed pseudo-random cloud over the structures.
pub(crate) fn probe_positions(n: usize) -> Vec<[i32; 3]> {
    let mut s = 0x2545_F491_4F6C_DD1Du64;
    (0..n)
        .map(|_| {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            [180 + (s % 64) as i32, -61 + ((s >> 8) % 24) as i32, 180 + ((s >> 16) % 64) as i32]
        })
        .collect()
}

const LIST_CATEGORIES: [Category; 4] =
    [Category::Monster, Category::Creature, Category::Ambient, Category::WaterAmbient];

/// Lists of the former and decoded paths for `positions`, and their fingerprint.
fn compare_lists(p: &Probe, positions: &[[i32; 3]]) -> u64 {
    let mut starts = super::structure_spawn::DecodedStarts::default();
    let mut h = fresh_hash();
    for &q in positions {
        for cat in LIST_CATEGORIES {
            let key = cat.key().expect("spawn key");
            let old = p.s.spawn_list_at(cat, key, q);
            let new = p.s.spawn_list_decoded(&mut starts, cat, key, q);
            assert_eq!(&*old, &*new, "{cat:?} at {q:?}");
            for e in old.iter() {
                mix(&mut h, &format!("{}{}", e["type"], e["count"]));
            }
            mix(&mut h, "|");
        }
    }
    h
}

/// Structure spawn lists: decoded starts = NBT walk for 10,000 positions (fingerprint frozen);
/// a fresh pass sees a structure placed live and a start chunk loaded after its reference.
#[test]
fn structure_spawn_lists_match() {
    let Some((mut p, _dir)) = open("structure-spawn-lists") else { return };
    write_structures(&mut p);
    let positions = probe_positions(10_000);
    let h = compare_lists(&p, &positions);
    eprintln!("structure lists fingerprint {h:#018x}");
    assert_eq!(h, EXPECTED_STRUCTURE_LISTS, "structure spawn lists changed");
    let fortress = |p: &Probe| positions.iter().filter(|&&q| p.s.in_structure(q, "minecraft:fortress")).count();
    assert!(fortress(&p) > 1000, "positions in the fortress box");
    // Placed live (as `/place structure` merges it): a fresh pass reads it.
    let hut = Nbt::Compound(vec![
        ("id".into(), Nbt::String("minecraft:swamp_hut".into())),
        (
            "Children".into(),
            Nbt::List(vec![Nbt::Compound(vec![("BB".into(), Nbt::IntArray(vec![181, -60, 181, 190, -50, 190]))])]),
        ),
    ]);
    let ch = p.s.chunks.get_mut(&(14, 12)).and_then(|c| c.as_deref_mut()).expect("chunk loaded");
    let st = nbtio::get_mut(&mut ch.root, "structures").expect("structures");
    nbtio::set(nbtio::get_mut(st, "starts").expect("starts"), "minecraft:swamp_hut", hut);
    let h_live = compare_lists(&p, &positions);
    assert_ne!(h_live, h, "the live structure changes some lists");
    // Start chunk loaded after the chunks that reference it.
    p.s.ensure_loaded(&[(60, 60)]);
    if let Some(Some(sc)) = p.s.chunks.get_mut(&(60, 60)) {
        let portal = Nbt::Compound(vec![
            ("id".into(), Nbt::String("minecraft:ruined_portal".into())),
            (
                "Children".into(),
                Nbt::List(vec![Nbt::Compound(vec![("BB".into(), Nbt::IntArray(vec![180, -61, 180, 243, -38, 243]))])]),
            ),
        ]);
        let st =
            Nbt::Compound(vec![("starts".into(), Nbt::Compound(vec![("minecraft:ruined_portal".into(), portal)]))]);
        nbtio::set(&mut sc.root, "structures", st);
        compare_lists(&p, &positions);
    }
}

/// Fingerprint of `structure_spawn_lists_match` (vanilla pack of the test data).
const EXPECTED_STRUCTURE_LISTS: u64 = 0x69e7_dca9_a6eb_1a44;
