//! Proofs of mission C2-M04 (server relight, `light_update` limited to the sections that
//! the game signals, empty mask): a single scenario (slabs that cover the sky, glowstone
//! at the corner of a section and a column, lamp turned on then off, water flow
//! on a roof, removals, emitter placed then removed in the same tick) played with the old and the
//! new path. At each tick:
//! - the server light is identical in the two paths (fingerprint of all the layers);
//! - a client that keeps only what it receives (chunk packets then updates) has the same
//!   layers as the server;
//! - each changed light cell has its section and those of its neighbours at ±1 in the filter of the
//!   tick (game rule), a created layer its section and its 26 neighbours;
//! - a null layer is sent through the empty mask, a non-null layer through its data.
//!
//! Sabotages (`play::LIGHT_SABOTAGE`): edge neighbours omitted, changed section omitted, empty mask
//! never set, created layers not signalled: each must be seen.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use rc_client::Event;
use rc_testdata::need;
use rc_world::{LIGHT_BYTES, LightLayer};

use crate::play::{LIGHT_SABOTAGE, LightOpts};
use crate::server::Server;
use crate::tests_opt::{SEED, Scratch, cmd};

type Layers = (Vec<LightLayer>, Vec<LightLayer>);

/// What the scenario saw.
struct Outcome {
    /// Fingerprint of the server light after each tick.
    hashes: Vec<String>,
    /// First breach of the rules (client, filter, empty mask), if any.
    fault: Option<String>,
    /// Light updates received and bytes of layers they carry (2048 per layer).
    updates: usize,
    bytes: usize,
    /// Light cells changed over the whole scenario (the setup must light up).
    changed: usize,
    /// Columns received by the client.
    cols: usize,
}

fn nib(l: &[u8; LIGHT_BYTES], i: usize) -> u8 {
    (l[i >> 1] >> ((i & 1) * 4)) & 15
}

fn bit(m: &[u64], i: usize) -> bool {
    m.get(i / 64).is_some_and(|w| w >> (i % 64) & 1 == 1)
}

fn layers_hash(s: &Server) -> String {
    let mut keys: Vec<_> = s.chunks.iter().filter_map(|(k, c)| c.as_ref().map(|c| (*k, c))).collect();
    keys.sort_by_key(|(k, _)| *k);
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA256);
    for ((x, z), c) in keys {
        ctx.update(&x.to_le_bytes());
        ctx.update(&z.to_le_bytes());
        for l in c.sky.iter().chain(&c.block) {
            match l {
                Some(l) => ctx.update(&l[..]),
                None => ctx.update(b"-"),
            }
        }
    }
    ctx.finish().as_ref().iter().map(|b| format!("{b:02x}")).collect()
}

/// Light received: layers present or declared empty; check of the empty mask.
fn check_light(l: &rc_proto_v777::chunk::LightData, rule_empty: bool, fault: &mut Option<String>, what: &str) {
    for (layers, mask, empty, kind) in [(&l.sky, &l.sky_mask, &l.sky_empty, "sky"), (&l.block, &l.block_mask, &l.block_empty, "block")] {
        for (i, x) in layers.iter().enumerate() {
            let Some(x) = x else {
                if bit(mask, i) || bit(empty, i) {
                    fault.get_or_insert_with(|| format!("{what} {kind} {i}: bit without layer"));
                }
                continue;
            };
            let zero = x.iter().all(|&b| b == 0);
            if rule_empty && zero != bit(empty, i) {
                fault.get_or_insert_with(|| format!("{what} {kind} {i}: null layer {zero}, empty mask {}", bit(empty, i)));
            }
            if bit(mask, i) == bit(empty, i) {
                fault.get_or_insert_with(|| format!("{what} {kind} {i}: masks {} / {}", bit(mask, i), bit(empty, i)));
            }
        }
    }
}

/// The scenario in a new thread: the server's randoms (`rng::test_seed`, specific to the thread and
/// advancing at each server created) follow the same sequence for each compared variant. In the
/// same thread, the second run drew other random ticks (grass spreading or not),
/// and the light fingerprint differed unrelated to the path under test.
fn run(opts: LightOpts, sabotage: u8) -> Option<Outcome> {
    std::thread::spawn(move || run_here(opts, sabotage)).join().unwrap_or_else(|p| std::panic::resume_unwind(p))
}

fn run_here(opts: LightOpts, sabotage: u8) -> Option<Outcome> {
    let data = Arc::new(need(rc_testdata::version_data(), "26.3 data cache")?);
    need(crate::registries::client_jar(&data), "26.3 client jar")?;
    let dir = std::env::temp_dir().join(format!("rc-sim-m04-{}-{:?}", std::process::id(), std::thread::current().id()));
    let _ = std::fs::remove_dir_all(&dir);
    let _guard = Scratch(dir.clone());
    let mut w = crate::newworld::NewWorld::new("m04");
    w.seed = SEED;
    crate::newworld::create_world(&dir, &data, &w).expect("world creation");
    let (mut s, session) = crate::portal_tests::open_world_session(&dir, data);
    s.light_opts = opts;
    for r in ["minecraft:spawn_mobs", "minecraft:advance_time", "minecraft:advance_weather"] {
        crate::nbtio::set(&mut s.rules, r, rc_proto::nbt::Nbt::Byte(0));
    }
    s.enter_world(crate::dims::OVERWORLD);
    s.pos = [16.5, 201.0, 16.5];
    let list: Vec<(i32, i32)> = (-3..=4).flat_map(|x| (-3..=4).map(move |z| (x, z))).collect();
    for p in &list {
        s.load_now(std::slice::from_ref(p));
    }
    s.loaded = true;
    s.teleport = None;
    // Columns sent to the client: 5 × 5 around the site (chunks 0 and 1 in the centre).
    s.queue = (-1..=3).flat_map(|x| (-1..=3).map(move |z| (x, z))).collect();
    s.send_chunks();
    LIGHT_SABOTAGE.with(|c| c.set(sabotage));
    let mut client: BTreeMap<(i32, i32), Layers> = BTreeMap::new();
    let mut out = Outcome { hashes: Vec::new(), fault: None, updates: 0, bytes: 0, changed: 0, cols: 0 };
    let steps: Vec<(Vec<&str>, usize)> = vec![
        (vec![], 10),
        (vec!["fill 0 199 0 31 199 31 minecraft:stone"], 5),
        (vec!["setblock 15 207 15 minecraft:glowstone"], 3),
        (vec!["setblock 16 200 16 minecraft:redstone_lamp", "setblock 17 200 16 minecraft:redstone_block"], 4),
        (vec!["setblock 17 200 16 minecraft:air"], 8),
        (vec!["fill 2 203 2 12 203 12 minecraft:stone"], 3),
        (vec!["setblock 5 204 5 minecraft:water"], 30),
        (vec!["setblock 15 207 15 minecraft:air"], 3),
        (vec!["fill 2 203 2 12 203 12 minecraft:air"], 30),
        (vec!["setblock 8 220 8 minecraft:sea_lantern", "setblock 8 220 8 minecraft:air"], 2),
        (vec!["setblock 31 215 0 minecraft:glowstone"], 3),
        // Light front (level 1) on the edges x = 15 and y = 223: neighbours unchanged.
        (vec!["setblock 1 209 8 minecraft:glowstone"], 3),
    ];
    for (cmds, n) in steps {
        for c in cmds {
            cmd(&mut s, c);
        }
        for _ in 0..n {
            // Layers before the tick (shared copies: a write makes a new one).
            let before: BTreeMap<(i32, i32), Layers> = s
                .chunks
                .iter()
                .filter_map(|(k, c)| Some((*k, (c.as_ref()?.sky.clone(), c.as_ref()?.block.clone()))))
                .collect();
            s.game_tick();
            s.tick += 1;
            // Filters received during the tick (sky, block) per column; `None`: whole column.
            let mut got: BTreeMap<(i32, i32), (BTreeSet<usize>, BTreeSet<usize>)> = BTreeMap::new();
            for e in session.events.try_iter() {
                match e {
                    Event::Chunk(c) => {
                        check_light(&c.light, opts.sections, &mut out.fault, "chunk");
                        let n = c.light.sky.len();
                        let g = got.entry((c.x, c.z)).or_default();
                        g.0.extend(0..n);
                        g.1.extend(0..n);
                        client.insert((c.x, c.z), (c.light.sky.clone(), c.light.block.clone()));
                    }
                    Event::LightUpdate { x, z, light } => {
                        check_light(&light, opts.sections, &mut out.fault, "update");
                        out.updates += 1;
                        out.bytes += (light.sky_mask.iter().chain(&light.block_mask).map(|w| w.count_ones()).sum::<u32>()
                            as usize)
                            * LIGHT_BYTES;
                        let g = got.entry((x, z)).or_default();
                        for (i, _) in light.sky.iter().enumerate().filter(|(_, l)| l.is_some()) {
                            g.0.insert(i);
                        }
                        for (i, _) in light.block.iter().enumerate().filter(|(_, l)| l.is_some()) {
                            g.1.insert(i);
                        }
                        // Filter bits without a stored layer: none, otherwise no trace here.
                        if let Some(col) = client.get_mut(&(x, z)) {
                            for (dst, src) in [(&mut col.0, &light.sky), (&mut col.1, &light.block)] {
                                for (d, s) in dst.iter_mut().zip(src) {
                                    if s.is_some() {
                                        d.clone_from(s);
                                    }
                                }
                            }
                        }
                    }
                    _ => {}
                }
            }
            out.hashes.push(layers_hash(&s));
            // Cells changed → sections that the game would signal.
            let mut want: BTreeSet<((i32, i32), usize, bool)> = BTreeSet::new();
            for (&(cx, cz), (bs, bb)) in &before {
                let Some(Some(c)) = s.chunks.get(&(cx, cz)) else { continue };
                for (sky, old, new) in [(true, bs, &c.sky), (false, bb, &c.block)] {
                    let n = new.len() as i32;
                    for (i, (o, nw)) in old.iter().zip(new.iter()).enumerate() {
                        let i = i as i32;
                        match (o, nw) {
                            (None, Some(_)) => {
                                for dx in -1..=1 {
                                    for dz in -1..=1 {
                                        for di in -1..=1 {
                                            if (0..n).contains(&(i + di)) {
                                                want.insert(((cx + dx, cz + dz), (i + di) as usize, sky));
                                            }
                                        }
                                    }
                                }
                            }
                            (Some(o), Some(nw)) if !Arc::ptr_eq(o, nw) => {
                                for k in 0..LIGHT_BYTES * 2 {
                                    if nib(o, k) == nib(nw, k) {
                                        continue;
                                    }
                                    out.changed += 1;
                                    let (lx, lz, ly) = ((k & 15) as i32, ((k >> 4) & 15) as i32, (k >> 8) as i32);
                                    let (bx, by, bz) = (cx * 16 + lx, (i - 1) * 16 + ly, cz * 16 + lz);
                                    for dx in -1..=1 {
                                        for dy in -1..=1 {
                                            for dz in -1..=1 {
                                                let pi = ((by + dy) >> 4) + 1;
                                                if (0..n).contains(&pi) {
                                                    want.insert(((((bx + dx) >> 4), ((bz + dz) >> 4)), pi as usize, sky));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
            if opts.sections {
                for &(col, i, sky) in &want {
                    if !client.contains_key(&col) || !s.sent.contains(&col) {
                        continue;
                    }
                    // Layer absent on the server side: the game would not send anything either.
                    let Some(Some(c)) = s.chunks.get(&col) else { continue };
                    let stored = if sky { c.sky[i].is_some() } else { c.block[i].is_some() };
                    let g = got.get(&col);
                    let ok = g.is_some_and(|g| if sky { g.0.contains(&i) } else { g.1.contains(&i) });
                    if stored && !ok {
                        out.fault.get_or_insert_with(|| format!(
                            "tick {}: section {i} ({}) of {col:?} not signalled",
                            s.tick,
                            if sky { "sky" } else { "block" }
                        ));
                    }
                }
            }
            // Client rebuilt from the packets alone = server.
            for (col, (sky, block)) in &client {
                let Some(Some(c)) = s.chunks.get(col) else { continue };
                if !s.sent.contains(col) {
                    continue;
                }
                for (kind, mine, theirs) in [("sky", sky, &c.sky), ("block", block, &c.block)] {
                    for (i, (a, b)) in mine.iter().zip(theirs.iter()).enumerate() {
                        let same = match (a, b) {
                            (Some(a), Some(b)) => a[..] == b[..],
                            (None, None) => true,
                            // Old path: a layer created around a change in a column that stayed
                            // intact was never sent (fixed by `sections`, like the game which
                            // signals every created layer).
                            (None, Some(_)) => !opts.sections,
                            _ => false,
                        };
                        if !same {
                            out.fault.get_or_insert_with(|| format!("tick {}: client {col:?} {kind} {i} differs from the server", s.tick));
                        }
                    }
                }
            }
        }
    }
    out.cols = client.len();
    LIGHT_SABOTAGE.with(|c| c.set(0));
    s.stop_gen();
    s.stop_parked();
    drop(s);
    Some(out)
}

/// Old and new path: same light at every tick, rules held, fewer bytes.
#[test]
fn relight_and_sections_equivalent() {
    let Some(old) = run(LightOpts::OLD, 0) else { return };
    let Some(new) = run(LightOpts::NEW, 0) else { return };
    eprintln!(
        "C2-M04: old {} updates, {} bytes of layers; new {} updates, {} bytes; {} cells changed",
        old.updates, old.bytes, new.updates, new.bytes, new.changed
    );
    assert!(new.changed > 1000, "setup without light effect: {}", new.changed);
    assert!(new.cols >= 9, "client without columns: {}", new.cols);
    assert_eq!(old.fault, None, "old path");
    assert_eq!(new.fault, None, "new path");
    assert_eq!(old.hashes.len(), new.hashes.len());
    for (t, (a, b)) in old.hashes.iter().zip(&new.hashes).enumerate() {
        assert_eq!(a, b, "server light differs at tick {t}");
    }
    assert!(new.bytes < old.bytes, "no byte gained: {} against {}", new.bytes, old.bytes);
    // Each switch alone: same light.
    for o in [
        LightOpts { sky_list: true, ..LightOpts::OLD },
        LightOpts { sections: true, ..LightOpts::OLD },
        LightOpts { misc: true, ..LightOpts::OLD },
    ] {
        let Some(r) = run(o, 0) else { return };
        assert_eq!(r.fault, None, "{o:?}");
        assert_eq!(r.hashes, old.hashes, "{o:?}");
    }
}

/// Each sabotage must be seen by the rules above.
#[test]
fn sabotages_seen() {
    for n in 1..=4u8 {
        let Some(r) = run(LightOpts::NEW, n) else { return };
        assert!(r.fault.is_some(), "sabotage {n} not seen");
        eprintln!("sabotage {n}: {}", r.fault.unwrap());
    }
}

#[test]
fn dimension_type_read() {
    let Some(data) = need(rc_testdata::version_data(), "26.3 data cache") else { return };
    for t in &data.dimension_types {
        let k = crate::world::DimKind::of(&data, &t.name);
        let first = data.dimension_types.iter().find(|d| d.name == t.name).unwrap();
        assert_eq!(k.sky, first.has_skylight);
        assert_eq!(k.ceiling, first.has_ceiling);
        assert_eq!(k.weather, first.has_skylight && !first.nether_light);
    }
    let k = crate::world::DimKind::of(&data, "test:absent");
    assert_eq!((k.sky, k.ceiling, k.weather), (true, false, true));
}

/// Micro-bench (1 core): light data of a column of 26 layers (half null), all
/// (old), empty mask, filter of 3 sections. `cargo test --release -p rc-sim bench_light_data
/// -- --ignored --nocapture`.
#[test]
#[ignore]
fn bench_light_data() {
    let full = Arc::new([0x5au8; LIGHT_BYTES]);
    let layers: Vec<LightLayer> =
        (0..26).map(|i| Some(if i % 2 == 0 { Arc::new([0u8; LIGHT_BYTES]) } else { full.clone() })).collect();
    let f = vec![0b111u64 << 12];
    let n = 200_000;
    let time = |name: &str, g: &dyn Fn() -> usize| {
        let t0 = std::time::Instant::now();
        let mut acc = 0;
        for _ in 0..n {
            acc += std::hint::black_box(g());
        }
        eprintln!("{name}: {:.0} ns per column ({acc})", t0.elapsed().as_nanos() as f64 / f64::from(n));
    };
    time("all (old)", &|| crate::server::light_data(&layers, &layers, false).sky_mask.len());
    time("empty mask", &|| crate::server::light_data(&layers, &layers, true).sky_empty.len());
    time("3-section filter", &|| crate::server::light_data_filtered(&layers, &layers, &f, &f).sky_mask.len());
}
