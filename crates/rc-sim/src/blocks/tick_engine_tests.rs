//! World tick engine (`ticks.rs`): former and optimised paths compared (`BlockTickPaths`).
//! Ticking chunks listed from what simulates them, scheduled-tick collection, random-tick test
//! of a section, ticker walk; each proof fails if the new path changes a result (see the
//! sabotage checks inside the tests).

use std::sync::Arc;

use rc_world::{Container, Section};

use super::P;
use super::probe::Probe;
use super::ticks::{BlockTickPaths, Tick, TickQueue, Ticker, TickingChunks, section_has_random_ticks};
use crate::tests_g3f13::{finish, open};

/// Xorshift: the random sequences of the tests, the same on every run.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }

    /// In `lo..=hi`.
    fn range(&mut self, lo: i32, hi: i32) -> i32 {
        lo + self.below((hi - lo + 1) as u64) as i32
    }
}

fn cmd(p: &mut Probe, c: &str) {
    let src = p.s.server_source();
    let (ok, _) = p.s.perform(src, c);
    assert!(ok, "command refused: {c}");
}

fn chebyshev(a: (i32, i32), b: (i32, i32)) -> i32 {
    (a.0 - b.0).abs().max((a.1 - b.1).abs())
}

// ---------- Scheduled-tick queue ----------

/// The ticking chunks of one step of the queue test, in both shapes.
fn ticking_shapes(rng: &mut Rng, wide: bool) -> (TickingChunks, TickingChunks) {
    let span = if wide { 7 } else { 2 };
    let mut loaded = hashbrown::HashSet::new();
    for x in -8..=8 {
        for z in -8..=8 {
            // A few holes: their neighbours do not tick.
            if rng.below(20) != 0 {
                loaded.insert((x, z));
            }
        }
    }
    let mut simulated = hashbrown::HashSet::new();
    for x in -span..=span {
        for z in -span..=span {
            if loaded.contains(&(x, z)) && rng.below(3) != 0 {
                simulated.insert((x, z));
            }
        }
    }
    let sets = TickingChunks::Sets { loaded, simulated };
    let mut list: Vec<(i32, i32)> =
        (-8..=8).flat_map(|x| (-8..=8).map(move |z| (x, z))).filter(|&c| sets.contains(c)).collect();
    list.sort_unstable();
    (sets, TickingChunks::Sorted(list))
}

/// Three queues fed the same operations: former collection, listed ticking chunks, ticking
/// chunks as sets (walk over `next`).
struct Queues([TickQueue; 3]);

impl Queues {
    fn schedule(&mut self, pos: P, kind: u32, at: i64, priority: i8, sub: i64) {
        let answers: Vec<bool> = self.0.iter_mut().map(|q| q.schedule(pos, kind, at, priority, sub)).collect();
        assert!(answers.iter().all(|&a| a == answers[0]), "schedule answers differ at {pos:?}");
    }

    /// One tick: collection, then the ticks run (some of them schedule new ones).
    fn tick(
        &mut self,
        now: i64,
        max: usize,
        shapes: &(TickingChunks, TickingChunks),
        rng: &mut Rng,
        sub: &mut i64,
    ) -> usize {
        let (sets, sorted) = shapes;
        self.0[0].collect(now, max, sorted, false);
        self.0[1].collect(now, max, sorted, true);
        self.0[2].collect(now, max, sets, true);
        let mut ran = 0;
        loop {
            let popped: Vec<Option<Tick>> = self.0.iter_mut().map(TickQueue::pop_next).collect();
            assert!(popped.iter().all(|t| *t == popped[0]), "tick {now}: popped {popped:?}");
            let Some(t) = popped[0] else { break };
            ran += 1;
            let probe = [t.pos[0] + rng.range(-1, 1), t.pos[1], t.pos[2]];
            let soon: Vec<bool> = self.0.iter().map(|q| q.will_tick_this_tick(probe, t.kind)).collect();
            assert!(soon.iter().all(|&s| s == soon[0]), "tick {now}: will_tick_this_tick differs");
            // A ticking block often schedules its neighbour.
            if rng.below(3) == 0 {
                *sub += 1;
                self.schedule(probe, t.kind, now + i64::from(rng.range(0, 4)), t.priority, *sub);
            }
        }
        for q in &mut self.0 {
            q.cleanup();
        }
        ran
    }

    fn assert_same_pending(&self, label: &str) {
        for x in -9..=9 {
            for z in -9..=9 {
                let ticks: Vec<Vec<Tick>> = self.0.iter().map(|q| q.chunk_ticks((x, z))).collect();
                assert!(ticks.iter().all(|t| *t == ticks[0]), "{label}: pending ticks of chunk ({x}, {z})");
            }
        }
    }
}

/// 10⁵ random operations (schedules, collections with a ticking set that changes on the way, a
/// small collection limit now and then, box copies and clears): the three paths pop the same
/// ticks in the same order and keep the same pending ticks.
#[test]
fn tick_queue_collection_matches_the_key_copy() {
    let mut rng = Rng(0x7157_C011_EC75);
    let mut q = Queues(Default::default());
    let (mut now, mut sub, mut ran) = (0i64, 0i64, 0usize);
    let mut shapes = ticking_shapes(&mut rng, true);
    for _ in 0..100_000 {
        match rng.below(100) {
            0..=59 => {
                sub += 1;
                let pos = [rng.range(-140, 140), rng.range(-2, 2), rng.range(-140, 140)];
                let at = now + i64::from(rng.range(0, 12));
                q.schedule(pos, rng.below(4) as u32, at, rng.range(-3, 3) as i8, sub);
            }
            60..=61 => {
                let min = [rng.range(-140, 120), -2, rng.range(-140, 120)];
                let max = [min[0] + rng.range(0, 40), 2, min[2] + rng.range(0, 40)];
                for queue in &mut q.0 {
                    queue.clear_box(min, max);
                }
            }
            62..=63 => {
                let min = [rng.range(-140, 120), -2, rng.range(-140, 120)];
                let max = [min[0] + rng.range(0, 40), 2, min[2] + rng.range(0, 40)];
                let mut boxes: Vec<Vec<Tick>> = q.0.iter().map(|queue| queue.in_box(min, max)).collect();
                for b in &mut boxes {
                    b.sort_unstable();
                }
                assert!(boxes.iter().all(|b| *b == boxes[0]), "in_box differs");
            }
            64..=66 => {
                let wide = rng.below(2) == 0;
                shapes = ticking_shapes(&mut rng, wide);
            }
            _ => {
                now += 1;
                let max = if rng.below(5) == 0 { 1 + rng.below(6) as usize } else { 65536 };
                ran += q.tick(now, max, &shapes, &mut rng, &mut sub);
            }
        }
    }
    q.assert_same_pending("end of the sequence");
    assert!(ran > 10_000, "too few ticks ran ({ran}): the sequence proves nothing");
}

/// Chunks left behind with due fluid ticks, outside the ticking chunks for 50 ticks, then ticking
/// again: their ticks run on the same tick and in the same order on every path.
#[test]
fn due_ticks_of_left_chunks_run_in_order_when_revisited() {
    let mut rng = Rng(0xFA12_0FF5);
    let mut q = Queues(Default::default());
    let mut sub = 0;
    for x in -8..=8 {
        for z in -8..=8 {
            for _ in 0..12 {
                sub += 1;
                let pos = [x * 16 + rng.range(0, 15), rng.range(-2, 2), z * 16 + rng.range(0, 15)];
                q.schedule(pos, rng.below(5) as u32, i64::from(rng.range(1, 6)), 0, sub);
            }
        }
    }
    let near = {
        let mut r = Rng(1);
        ticking_shapes(&mut r, false)
    };
    let all = {
        let loaded: hashbrown::HashSet<(i32, i32)> = (-9..=9).flat_map(|x| (-9..=9).map(move |z| (x, z))).collect();
        let simulated = loaded.clone();
        let sets = TickingChunks::Sets { loaded, simulated };
        let mut list: Vec<(i32, i32)> =
            (-9..=9).flat_map(|x| (-9..=9).map(move |z| (x, z))).filter(|&c| sets.contains(c)).collect();
        list.sort_unstable();
        (sets, TickingChunks::Sorted(list))
    };
    let mut ran_near = 0;
    for now in 1..=50 {
        ran_near += q.tick(now, 65536, &near, &mut rng, &mut sub);
    }
    let back = q.tick(51, 65536, &all, &mut rng, &mut sub);
    assert!(ran_near > 0 && back > 1_000, "scenario too small: {ran_near} near, {back} on return");
    q.assert_same_pending("after the return");
}

// ---------- Ticking chunks ----------

/// Listed ticking chunks equal the scanned ones, random-tick chunks too, and the squares hold
/// every simulated loaded chunk; returns the number of ticking chunks.
fn assert_listed_matches_scan(p: &Probe, label: &str) -> usize {
    let centers = p.s.sim_centers();
    let scanned = p.s.ticking_chunks(&centers, false);
    let listed_with = |grid: bool| {
        let paths = BlockTickPaths { loaded_grid: grid, ..BlockTickPaths::NEW };
        paths.run(|| p.s.ticking_chunks(&centers, true))
    };
    let listed = listed_with(true);
    let TickingChunks::Sets { loaded, .. } = &scanned else { panic!("{label}: former path not scanned") };
    let mut from_scan: Vec<(i32, i32)> = loaded.iter().copied().filter(|&c| scanned.contains(c)).collect();
    from_scan.sort_unstable();
    let TickingChunks::Sorted(list) = &listed else { panic!("{label}: new path not listed") };
    assert_eq!(list, &from_scan, "{label}: ticking chunks");
    let TickingChunks::Sorted(without_grid) = listed_with(false) else { panic!("{label}: new path not listed") };
    assert_eq!(without_grid, from_scan, "{label}: ticking chunks without the loaded grid");
    assert_eq!(
        p.s.random_tick_chunks(&listed, &centers),
        p.s.random_tick_chunks(&scanned, &centers),
        "{label}: random-tick chunks"
    );
    let squares = p.s.simulated_squares(&centers);
    for &c in loaded {
        if p.s.chunk_simulated(c, &centers) {
            assert!(
                squares.iter().any(|&(q, r)| chebyshev(c, q) <= r),
                "{label}: simulated chunk {c:?} outside the squares"
            );
        }
    }
    list.len()
}

fn load_square(p: &mut Probe, center: (i32, i32), r: i32) {
    let list: Vec<(i32, i32)> =
        (center.0 - r..=center.0 + r).flat_map(|x| (center.1 - r..=center.1 + r).map(move |z| (x, z))).collect();
    p.s.load_now(&list);
}

/// Every source of simulation, alone and together: players (host moved, spectator, LAN guest),
/// forced chunks (a few, a few dozen, 300: the loaded chunks are then walked), an applied pearl
/// ticket far away (and one not applied yet), the dragon fight around the origin. Sabotages: the
/// squares without the pearl, without the fight: the comparison catches each one.
#[test]
fn listed_ticking_chunks_match_the_scan() {
    let (mut p, dir) = open("tick-candidates");
    load_square(&mut p, (0, 0), 10);
    load_square(&mut p, (43, 3), 3);
    load_square(&mut p, (-25, -25), 4);
    p.s.sim_dist = 2;
    assert!(assert_listed_matches_scan(&p, "host") > 0);
    p.s.sim_dist = 6;
    assert_listed_matches_scan(&p, "host, simulation 6");
    p.s.pos = [150.5, -60.0, 70.5];
    assert_listed_matches_scan(&p, "host moved to the edge of the loaded chunks");
    p.s.pos = [0.5, -60.0, 0.5];
    p.s.sim_dist = 2;
    p.s.game_mode = 3;
    cmd(&mut p, "gamerule minecraft:spectators_generate_chunks false");
    assert_eq!(assert_listed_matches_scan(&p, "spectator holding no ticket"), 0);
    p.s.game_mode = 0;
    for (n, label) in [(5usize, "5 forced"), (40, "40 forced"), (300, "300 forced")] {
        p.s.cmd.forced.clear();
        for i in 0..n as i32 {
            p.s.cmd.forced.insert((i % 20 - 10, i / 20 - 8));
        }
        assert_listed_matches_scan(&p, label);
    }
    p.s.cmd.forced.clear();
    p.s.cmd.forced.insert((-25, -22));
    // Pearl tickets: an applied one far from the player, one placed this tick (not counted yet).
    use crate::projectiles::PearlTicket;
    p.s.proj.tickets.push(PearlTicket { chunk: (43, 3), left: 40, live: true });
    p.s.proj.tickets.push(PearlTicket { chunk: (-25, -27), left: 40, live: false });
    let with_pearl = assert_listed_matches_scan(&p, "pearl tickets");
    let centers = p.s.sim_centers();
    let without_pearls: Vec<_> = p.s.simulated_squares(&centers).into_iter().filter(|&(c, _)| c != (43, 3)).collect();
    assert!(p.s.ticking_chunks_in(&without_pearls, &centers).len() < with_pearl, "sabotage (pearl forgotten) not seen");
    p.s.mobs.end.fight = Some(Box::new(crate::mobs::end::fight::Fight::with_players_in_bar()));
    let with_fight = assert_listed_matches_scan(&p, "dragon fight");
    let without_fight: Vec<_> = p.s.simulated_squares(&centers).into_iter().filter(|&(_, r)| r != 8).collect();
    assert!(p.s.ticking_chunks_in(&without_fight, &centers).len() < with_fight, "sabotage (fight forgotten) not seen");
    let _keep = crate::mobs::tests_spawn_paths::add_guest(&mut p.s, "Guest", [-399.5, -60.0, -399.5]);
    assert_listed_matches_scan(&p, "LAN guest, every source");
    finish(p, dir);
}

/// One world, 400 ticks with every source of simulation coming and going (host walking, forced
/// chunks, a far pearl ticket, the dragon fight, a LAN guest), water and lava spreading, grass
/// random ticks: fingerprint of every tick (world LCG and random, pending ticks, watched blocks,
/// ticker list).
fn moving_sources_scenario() -> Vec<u64> {
    let tag = format!("tick-sources-{}", BlockTickPaths::current() == BlockTickPaths::NEW);
    let (mut p, dir) = open(&tag);
    load_square(&mut p, (0, 0), 10);
    load_square(&mut p, (43, 3), 3);
    load_square(&mut p, (-25, -25), 4);
    p.s.sim_dist = 3;
    for (x, z) in [(8, 8), (40, -20), (120, 30), (-100, 90), (690, 50), (-395, -395)] {
        cmd(&mut p, &format!("setblock {x} -60 {z} minecraft:water"));
        cmd(&mut p, &format!("setblock {} -60 {} minecraft:lava", x + 6, z + 6));
    }
    // Water waiting in half the chunks around the origin, most of them outside the simulation:
    // more chunks with pending ticks than ticking chunks, so the collection looks the due chunks
    // up from the ticking ones (and those left behind tick again as the sources come near).
    for x in -10..=10 {
        for z in (-10..=10).filter(|z| (x + z) % 2 == 0) {
            cmd(&mut p, &format!("setblock {} -60 {} minecraft:water", x * 16 + 3, z * 16 + 11));
        }
    }
    let mut out = Vec::with_capacity(400);
    let mut keep = None;
    for tick in 0..400 {
        match tick {
            50 => p.s.pos = [60.5, -60.0, 20.5],
            100 => {
                p.s.cmd.forced.extend((0..30).map(|i| (i % 6 - 10, i / 6 + 4)));
                use crate::projectiles::PearlTicket;
                p.s.proj.tickets.push(PearlTicket { chunk: (43, 3), left: 40, live: true });
            }
            150 => p.s.pos = [-40.5, -60.0, -30.5],
            200 => p.s.mobs.end.fight = Some(Box::new(crate::mobs::end::fight::Fight::with_players_in_bar())),
            260 => p.s.cmd.forced.clear(),
            300 => {
                p.s.mobs.end.fight = None;
                keep = Some(crate::mobs::tests_spawn_paths::add_guest(&mut p.s, "Guest", [-399.5, -60.0, -399.5]));
            }
            _ => {}
        }
        p.s.game_tick();
        p.s.tick += 1;
        out.push(tick_fingerprint(&p));
    }
    drop(keep);
    finish(p, dir);
    out
}

/// FNV-1a over the state a tick of the block engine can change.
fn tick_fingerprint(p: &Probe) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut mix = |text: &str| {
        for b in text.bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
    };
    mix(&format!("{} {:?}", p.s.bs.lcg, p.s.bs.rng));
    for x in -9..=9 {
        for z in -9..=9 {
            mix(&format!("{:?}{:?}", p.s.bs.block_ticks.chunk_ticks((x, z)), p.s.bs.fluid_ticks.chunk_ticks((x, z))));
        }
    }
    for (x, z) in [(8, 8), (40, -20), (120, 30), (-100, 90), (690, 50), (-395, -395)] {
        for dx in -3..=9 {
            for dz in -3..=9 {
                mix(p.s.state_name(p.s.get([x + dx, -60, z + dz])));
            }
        }
    }
    let tickers: Vec<(u64, P)> = p.s.bs.tickers.iter().map(|t| (t.id, t.pos)).collect();
    mix(&format!("{tickers:?}"));
    h
}

fn on_own_thread<T: Send + 'static>(paths: BlockTickPaths, scenario: fn() -> T) -> T {
    std::thread::spawn(move || paths.run(scenario)).join().expect("scenario thread")
}

fn assert_same_ticks(label: &str, old: &[u64], new: &[u64]) {
    assert_eq!(old.len(), new.len(), "{label}: tick counts");
    if let Some(i) = old.iter().zip(new).position(|(a, b)| a != b) {
        panic!("{label}: first difference at tick {i}");
    }
}

/// Fingerprint of the last tick of `moving_sources_scenario` on the new paths, recorded on
/// 2026-11-07. It moves only with a change of the block engine's behaviour: then check the
/// former/new comparison above it first, and record the new value.
const MOVING_SOURCES_LAST_TICK: u64 = 0xd20f_1208_0ec6_197a;

/// The whole world tick on every former path, on every new path, on each new path alone: same
/// fingerprint tick by tick (and the stored one at the end).
#[test]
fn block_tick_paths_keep_the_world_tick() {
    let old = on_own_thread(BlockTickPaths::OLD, moving_sources_scenario);
    eprintln!("moving sources, former paths: last tick {:#x}", old.last().expect("ticks"));
    let new = on_own_thread(BlockTickPaths::NEW, moving_sources_scenario);
    assert_same_ticks("every new path", &old, &new);
    let alone = [
        BlockTickPaths { simulated_candidates: true, ..BlockTickPaths::OLD },
        BlockTickPaths { simulated_candidates: true, loaded_grid: true, ..BlockTickPaths::OLD },
        BlockTickPaths { tick_queue_candidates: true, ..BlockTickPaths::OLD },
    ];
    for paths in alone {
        assert_same_ticks(&format!("{paths:?}"), &old, &on_own_thread(paths, moving_sources_scenario));
    }
    let last = *new.last().expect("ticks");
    // Recorded under Windows (the C library's sin and cos differ elsewhere, see the CI notes).
    if cfg!(windows) {
        assert_eq!(last, MOVING_SOURCES_LAST_TICK, "stored fingerprint of the last tick (new value {last:#x})");
    }
}

// ---------- Random-tick sections ----------

/// Sections built by hand: single state (random or not), stone with ores, surface, Nether with
/// lava, a palette keeping a random state no cell uses, a section without palette, an empty one.
/// The palette path answers like the scan; ignoring lava (sabotage) is caught on the Nether one.
#[test]
fn random_tick_palette_matches_the_cell_scan() {
    let (p, dir) = open("tick-palette");
    let state = |name: &str| {
        let b = p.s.bs.t.block_id(name);
        assert_ne!(b, u16::MAX, "block {name} missing");
        p.s.view().default_state(b)
    };
    let v = p.s.view();
    let random = |s: u32| v.random_ticks(s) || p.s.fluid_of(s).kind == 2;
    // `cells`: index into `states` of each cell.
    let packed = |states: &[u32], cells: &dyn Fn(usize) -> usize| {
        let mut values = vec![0u32; 4096];
        for (i, value) in values.iter_mut().enumerate() {
            *value = states[cells(i)];
        }
        Section { non_empty: 4096, blocks: Container::pack(&values, 4, 8, 15), biomes: Container::Single(0) }
    };
    let single = |s: u32| Section { non_empty: 4096, blocks: Container::Single(s), biomes: Container::Single(0) };
    let (stone, grass, dirt, lava) =
        (state("minecraft:stone"), state("minecraft:grass_block"), state("minecraft:dirt"), state("minecraft:lava"));
    let ores = [stone, state("minecraft:iron_ore"), state("minecraft:coal_ore"), state("minecraft:deepslate")];
    let nether = packed(&[state("minecraft:netherrack"), lava], &|i| usize::from(i % 97 == 0));
    let stale = Section {
        non_empty: 4096,
        blocks: Container::Packed { bits: 4, palette: Some(vec![dirt, grass].into()), data: vec![0u64; 256].into() },
        biomes: Container::Single(0),
    };
    // Global identifiers on 15 bits, four per word: stone everywhere, grass in the last cell.
    let direct = {
        let mut data = vec![0u64; 1024];
        for (i, word) in (0..4096).map(|i| (i, i / 4)) {
            let s = if i == 4095 { grass } else { stone };
            data[word] |= u64::from(s) << ((i % 4) * 15);
        }
        Section {
            non_empty: 4096,
            blocks: Container::Packed { bits: 15, palette: None, data: data.into() },
            biomes: Container::Single(0),
        }
    };
    let cases = [
        ("stone, single", single(stone), false),
        ("grass, single", single(grass), true),
        ("stone with ores", packed(&ores, &|i| (i * 7) % 4), false),
        ("surface", packed(&[dirt, grass], &|i| usize::from(i >= 3840)), true),
        ("Nether with lava", nether, true),
        ("palette keeping a random state", stale, false),
        ("no palette", direct, true),
        ("empty", Section::empty(state("minecraft:air"), 0), false),
    ];
    for (label, sec, expected) in &cases {
        assert_eq!(section_has_random_ticks(sec, false, random), *expected, "{label}: cell scan");
        assert_eq!(section_has_random_ticks(sec, true, random), *expected, "{label}: palette first");
    }
    // Sabotages the cases catch: the palette trusted on a "maybe" (stale entry), a single state
    // answered without its state. Ignoring the fluid clause would not be one: the lava block
    // carries the random-tick flag itself, the clause only restates it.
    let stale_case = &cases[5].1;
    assert!(stale_case.blocks.may_contain(random), "the stale palette must say \"maybe\"");
    assert!(!section_has_random_ticks(stale_case, true, random), "palette trusted on a \"maybe\"");
    assert!(v.random_ticks(lava), "lava block without the random-tick flag: the fluid clause matters again");
    finish(p, dir);
}

/// Every section of the test world (overworld around the spawn, then the Nether): the palette
/// path answers like the cell scan.
#[test]
fn random_tick_palette_matches_the_cell_scan_on_the_test_world() {
    let (Some(world), Some(data)) = (crate::tests::test_world(), rc_testdata::version_data()) else { return };
    let dir = crate::tests::fresh_copy(&world, "tick-palette-world");
    let mut p = Probe::open(&dir, Arc::new(data));
    let mut sections = 0;
    let mut random = 0;
    for dim in [crate::dims::OVERWORLD, crate::dims::NETHER] {
        p.s.enter_world(dim);
        let (cx, cz) = ((p.s.pos[0].floor() as i32) >> 4, (p.s.pos[2].floor() as i32) >> 4);
        load_square(&mut p, (cx, cz), 5);
        let v = p.s.view();
        let pred = |s: u32| v.random_ticks(s) || p.s.fluid_of(s).kind == 2;
        for c in p.s.chunks.values().flatten() {
            for sec in &c.sections {
                let old = section_has_random_ticks(sec, false, pred);
                assert_eq!(section_has_random_ticks(sec, true, pred), old, "{dim}: chunk ({}, {})", c.x, c.z);
                sections += 1;
                random += usize::from(old);
            }
        }
    }
    assert!(sections > 1_000 && random > 0, "too few sections ({sections}, {random} random)");
    eprintln!("random-tick palette on the test world: {sections} sections, {random} with random ticks");
    drop(p);
}

// ---------- Ticker walk ----------

#[test]
fn ticker_entry_keeps_its_size() {
    assert_eq!(std::mem::size_of::<Ticker>(), 24, "the family must sit in the padding of the former pair");
}

/// Hoppers dying and coming back every few ticks among shulker lids, hives, sculk sensors,
/// furnaces and a pushing piston: ticker list, registrations and the watched blocks identical
/// tick by tick, every former path against every new one.
fn ticker_scenario() -> Vec<u64> {
    let tag = format!("tick-tickers-{}", BlockTickPaths::current() == BlockTickPaths::NEW);
    let (mut p, dir) = open(&tag);
    p.s.sim_dist = 4;
    let mut rng = Rng(0x71C4_E125);
    let hopper = |i: i32| [i % 20, -59, i / 20 + 2];
    for i in 0..200 {
        let [x, y, z] = hopper(i);
        cmd(&mut p, &format!("setblock {x} {y} {z} minecraft:hopper[facing=east]"));
    }
    // Items for the hopper line (the first hopper pulls them from the chest above it).
    cmd(&mut p, "setblock 0 -58 2 minecraft:chest{Items:[{Slot:0b,id:\"minecraft:cobblestone\",count:64}]}");
    for (i, block) in
        ["minecraft:beehive", "minecraft:sculk_sensor", "minecraft:furnace", "minecraft:shulker_box"].iter().enumerate()
    {
        let x = 30 + 3 * i as i32;
        cmd(&mut p, &format!("setblock {x} -59 5 {block}"));
    }
    p.s.shulker_lid_event([39, -59, 5], 1);
    cmd(&mut p, "setblock 40 -59 9 minecraft:piston[facing=east]");
    cmd(&mut p, "setblock 41 -59 9 minecraft:stone");
    let mut made: Vec<(u64, P)> = Vec::new();
    let mut out = Vec::new();
    for tick in 0..240 {
        if tick % 5 == 0 {
            // About one hopper in ten dies, as many come back elsewhere (a `setblock` that changes
            // nothing is refused, like the game's: its answer is not checked here).
            for _ in 0..20 {
                let [x, y, z] = hopper(rng.range(0, 199));
                let block = if rng.below(2) == 0 { "air" } else { "hopper[facing=south]" };
                let src = p.s.server_source();
                let _ = p.s.perform(src, &format!("setblock {x} {y} {z} minecraft:{block}"));
            }
        }
        if tick % 40 == 10 {
            cmd(&mut p, "setblock 39 -59 9 minecraft:redstone_block");
        }
        if tick % 40 == 30 {
            cmd(&mut p, "setblock 39 -59 9 minecraft:air");
        }
        if tick == 120 {
            cmd(&mut p, "setblock 30 -59 5 minecraft:air");
            cmd(&mut p, "setblock 33 -59 5 minecraft:air");
        }
        if tick == 180 {
            cmd(&mut p, "setblock 30 -59 5 minecraft:bee_nest");
            cmd(&mut p, "setblock 33 -59 5 minecraft:sculk_shrieker");
        }
        p.s.game_tick();
        p.s.tick += 1;
        for t in p.s.bs.tickers.iter().chain(p.s.bs.pending_tickers.iter()) {
            if !made.contains(&(t.id, t.pos)) {
                made.push((t.id, t.pos));
            }
        }
        let registered: Vec<bool> = made.iter().map(|&(id, at)| p.s.ticker_registered(id, at)).collect();
        let mut h = tick_fingerprint(&p);
        let hoppers: std::collections::BTreeMap<String, String> =
            p.s.bs.hoppers.iter().map(|(k, v)| (format!("{k:?}"), format!("{v:?}"))).collect();
        let text = format!("{registered:?}{hoppers:?}{:?}", p.s.be_get([19, -59, 11]));
        for b in text.bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        out.push(h);
    }
    let alive = made.iter().filter(|&&(id, at)| p.s.ticker_registered(id, at)).count();
    assert!(made.len() > 250 && alive > 100, "scenario too small: {} tickers made, {alive} alive", made.len());
    finish(p, dir);
    out
}

#[test]
fn ticker_walk_paths_match() {
    // Each lead alone first: a failure names the lead.
    let old = on_own_thread(BlockTickPaths::OLD, ticker_scenario);
    let compaction_only = BlockTickPaths { ticker_compaction: true, ..BlockTickPaths::OLD };
    assert_same_ticks("compaction alone", &old, &on_own_thread(compaction_only, ticker_scenario));
    let family_only = BlockTickPaths { ticker_family: true, ..BlockTickPaths::OLD };
    assert_same_ticks("family alone", &old, &on_own_thread(family_only, ticker_scenario));
    let new = on_own_thread(BlockTickPaths::NEW, ticker_scenario);
    assert_same_ticks("every new path", &old, &new);
}
