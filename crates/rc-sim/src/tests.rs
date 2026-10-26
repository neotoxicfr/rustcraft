//! Tests on a real copied world (`RUSTCRAFT_SOLO_WORLD` = folder of a COPY of the world of the
//! test map; never the world of a running server). Absent: tests ignored.

use std::path::PathBuf;
use std::time::{Duration, Instant};

use crate::chunk::Skip;
use crate::level::Level;
use crate::states::States;
use crate::world::{Dimension, Load};

pub(crate) fn test_world() -> Option<PathBuf> {
    std::env::var_os("RUSTCRAFT_SOLO_WORLD").map(PathBuf::from).filter(|p| p.join("level.dat").is_file())
}

pub(crate) const DIMENSIONS: [&str; 3] = ["minecraft:overworld", "minecraft:the_nether", "minecraft:the_end"];

/// Registries and tags rebuilt from the jar = those sent by the vanilla server in the
/// reference capture (same registries, same entry order, same flattened tags).
#[test]
fn registries_match_capture() {
    use rc_data::{Bound, Phase};
    use rc_proto::Reader;
    use rc_proto::varint::read_varint;
    let (Some(data), Some(cap)) = (rc_testdata::version_data(), rc_testdata::reference_capture()) else { return };
    let Some(jar) = crate::registries::client_jar(&data) else { return };
    let ours = crate::registries::load(&jar, &data).expect("registries");
    let reg_id = data.packet_id(Phase::Configuration, Bound::ToClient, "minecraft:registry_data").unwrap();
    let tags_id = data.packet_id(Phase::Configuration, Bound::ToClient, "minecraft:update_tags").unwrap();
    let mut theirs = Vec::new();
    let mut their_tags = None;
    for r in rc_testdata::records(&cap).into_iter().filter(|r| !r.to_server && r.phase == 3) {
        let mut rd = Reader::new(r.packet);
        let id = read_varint(&mut rd).unwrap();
        if id == reg_id {
            let d = rc_proto_v777::config::read_registry_data(&mut rd).unwrap();
            theirs.push((d.registry, d.entries.into_iter().map(|e| e.0).collect::<Vec<_>>()));
        } else if id == tags_id {
            their_tags = Some(rc_proto_v777::play::read_update_tags(&mut rd).unwrap());
        }
    }
    let names = |v: &[(String, Vec<String>)]| v.iter().map(|x| x.0.clone()).collect::<Vec<_>>();
    if std::env::var_os("RUSTCRAFT_DUMP").is_some() {
        eprintln!("capture: {:?}", names(&theirs));
        for (n, t) in their_tags.iter().flatten() {
            eprintln!("tags {n}: {}", t.len());
        }
    }
    assert_eq!(names(&ours.synced), names(&theirs), "registry list");
    for ((n, a), (_, b)) in ours.synced.iter().zip(&theirs) {
        assert_eq!(a, b, "order of {n}");
    }
    let mut theirs_t = their_tags.expect("update_tags");
    let mut ours_t = ours.tags.clone();
    for t in [&mut theirs_t, &mut ours_t] {
        t.sort_by(|a, b| a.0.cmp(&b.0));
        for (_, l) in t.iter_mut() {
            l.sort_by(|a, b| a.0.cmp(&b.0));
            for (_, ids) in l.iter_mut() {
                ids.sort();
            }
        }
    }
    let tnames = |t: &crate::registries::Tags| t.iter().map(|x| x.0.clone()).collect::<Vec<_>>();
    assert_eq!(tnames(&ours_t), tnames(&theirs_t), "tagged registries");
    for ((n, a), (_, b)) in ours_t.iter().zip(&theirs_t) {
        assert_eq!(a, b, "tags of {n}");
    }
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap().flatten() {
        let p = e.path();
        if p.is_dir() {
            copy_dir(&p, &to.join(e.file_name()));
        } else if e.file_name() != "session.lock" {
            std::fs::copy(&p, to.join(e.file_name())).unwrap();
        }
    }
}

/// Fresh copy of the test world in a temporary folder specific to the test, erased at the end
/// of the test's thread even if it fails (a test that panics skips its own erasing: these
/// 70 MB copies piled up by the hundreds in the temporary folder).
pub(crate) fn fresh_copy(world: &std::path::Path, tag: &str) -> PathBuf {
    use std::cell::RefCell;
    struct Copies(Vec<PathBuf>);
    impl Drop for Copies {
        fn drop(&mut self) {
            for d in &self.0 {
                let _ = std::fs::remove_dir_all(d);
            }
        }
    }
    thread_local! {
        static COPIES: RefCell<Copies> = const { RefCell::new(Copies(Vec::new())) };
    }
    let dir = std::env::temp_dir().join(format!("rc-sim-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy_dir(world, &dir);
    COPIES.with(|c| c.borrow_mut().0.push(dir.clone()));
    dir
}

/// Entering the game through the in-memory link: registries, `login`, teleport, chunks around the
/// player, time that advances; on closing, the lock is released.
#[test]
fn solo_join_and_ticks() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = fresh_copy(&world, "join");
    let data = std::sync::Arc::new(data);
    let opts = crate::Options {
        world: dir.clone(),
        name: "AgentSolo".into(),
        uuid: rc_client::offline_uuid("AgentSolo"),
        info: rc_proto_v777::play::ClientInformation { view_distance: 4, ..Default::default() },
        sim_distance: 12,
    };
    solo_slot();
    let (session, handle) = crate::start(opts, data).expect("solo");
    let t = Instant::now();
    let (mut joined, mut chunks, mut times, mut teleport) = (false, 0, Vec::new(), None);
    // Up to 3 time syncs (≈ 2 s of game), 15 s at most on a loaded machine.
    while times.len() < 4 && t.elapsed() < Duration::from_secs(15) {
        let Ok(e) = session.events.recv_timeout(Duration::from_millis(100)) else { continue };
        match e {
            rc_client::Event::Joined { .. } => joined = true,
            rc_client::Event::Chunk(_) => chunks += 1,
            rc_client::Event::Time { game_time, .. } => times.push(game_time),
            rc_client::Event::Teleport(p) => teleport = Some(p),
            rc_client::Event::Disconnected(r) => panic!("{r}"),
            _ => {}
        }
    }
    let p = teleport.expect("teleport");
    session.link.send_accept_teleport(p.id, p.pos, p.yaw, p.pitch).unwrap();
    eprintln!("chunks: {chunks}, times: {times:?}, position {:?}", p.pos);
    assert!(joined);
    // Lock taken by the server thread before entering the game.
    assert!(crate::lock::SessionLock::is_locked(&dir));
    assert!(chunks > 40, "{chunks} chunks");
    assert!(times.len() >= 3 && times.last() > times.first(), "{times:?}");
    session.link.disconnect();
    handle.wait();
    assert!(!crate::lock::SessionLock::is_locked(&dir));
    let _ = std::fs::remove_dir_all(&dir);
}

/// Waits for an event satisfying `f` for `secs` seconds of GAME time (20 ticks per second,
/// counted on the time syncs sent every 20 ticks), not wall-clock time: under load, the
/// server drops below 20 ticks/s (13 measured, full suite in parallel) and a wall-clock wait
/// cut scenes before their end (salmon not yet suffocated, volley not yet fired). Wide
/// wall-clock bound (`5 × secs + 10` s) if the time no longer advances (frozen
/// world).
pub(crate) fn wait_game(s: &rc_client::Session, secs: u64, mut f: impl FnMut(&rc_client::Event) -> bool) -> bool {
    let t = Instant::now();
    let cap = Duration::from_secs(secs * 5 + 10);
    let (mut first, mut last) = (None::<i64>, 0i64);
    while t.elapsed() < cap {
        if first.is_some_and(|f0| last - f0 >= secs as i64 * 20) {
            return false;
        }
        let Ok(e) = s.events.recv_timeout(Duration::from_millis(20)) else { continue };
        if let rc_client::Event::Time { game_time, .. } = &e {
            first.get_or_insert(*game_time);
            last = *game_time;
        }
        if f(&e) {
            return true;
        }
    }
    false
}

/// Waits for an event satisfying `f` (10 s at most); also returns those seen before.
fn wait_for(
    s: &rc_client::Session,
    mut f: impl FnMut(&rc_client::Event) -> bool,
) -> (Option<rc_client::Event>, Vec<rc_client::Event>) {
    let t = Instant::now();
    let mut seen = Vec::new();
    while t.elapsed() < Duration::from_secs(10) {
        let Ok(e) = s.events.recv_timeout(Duration::from_millis(50)) else { continue };
        if f(&e) {
            return (Some(e), seen);
        }
        seen.push(e);
    }
    (None, seen)
}

/// Singleplayer servers open at the same time in the suite (real time or step by step). Opening a
/// server decompresses the jar (registries, then generation data) and starts a generator;
/// with one test per core, all these simultaneous openings made the process stall: an
/// opening of 300 ms took 20 to 40 (measured), and the real-time tests, which count
/// in seconds, failed by the dozen ("no teleport"). The number is therefore bounded
/// (a quarter of the cores, `RUSTCRAFT_SOLO_TESTS` to force it); the slot is held by the test's thread
/// until its end (even on failure), and reused if it reopens the world.
pub(crate) fn solo_slot() {
    use std::cell::RefCell;
    use std::sync::{Condvar, Mutex};
    static BUSY: Mutex<usize> = Mutex::new(0);
    static FREED: Condvar = Condvar::new();
    struct Slot;
    impl Drop for Slot {
        fn drop(&mut self) {
            *BUSY.lock().unwrap_or_else(|e| e.into_inner()) -= 1;
            FREED.notify_one();
        }
    }
    thread_local! {
        static HELD: RefCell<Option<Slot>> = const { RefCell::new(None) };
    }
    let limit = std::env::var("RUSTCRAFT_SOLO_TESTS")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or_else(|| std::thread::available_parallelism().map_or(1, |n| n.get() / 4));
    let limit = limit.max(1);
    HELD.with(|h| {
        if h.borrow().is_some() {
            return;
        }
        let mut busy = BUSY.lock().unwrap_or_else(|e| e.into_inner());
        while *busy >= limit {
            busy = FREED.wait(busy).unwrap_or_else(|e| e.into_inner());
        }
        *busy += 1;
        *h.borrow_mut() = Some(Slot);
    });
}

pub(crate) fn start_test(
    dir: &std::path::Path,
    data: std::sync::Arc<rc_data::VersionData>,
) -> (rc_client::Session, crate::Handle, rc_proto_v777::play::PlayerPosition) {
    solo_slot();
    let opts = crate::Options {
        world: dir.to_owned(),
        name: "AgentSolo".into(),
        uuid: rc_client::offline_uuid("AgentSolo"),
        info: rc_proto_v777::play::ClientInformation { view_distance: 3, ..Default::default() },
        sim_distance: 12,
    };
    let (session, handle) = crate::start(opts, data).expect("solo");
    let t0 = Instant::now();
    let (tp, seen) = wait_for(&session, |e| matches!(e, rc_client::Event::Teleport(_)));
    let Some(rc_client::Event::Teleport(p)) = tp else {
        let end: Vec<String> = seen
            .iter()
            .filter(|e| matches!(e, rc_client::Event::Disconnected(_) | rc_client::Event::Kicked(_)))
            .map(|e| format!("{e:?}"))
            .collect();
        panic!("no teleport after {:?} ({} events, end: {end:?})", t0.elapsed(), seen.len())
    };
    session.link.send_accept_teleport(p.id, p.pos, p.yaw, p.pitch).unwrap();
    session.link.send_player_loaded().unwrap();
    (session, handle, p)
}

/// Playing: accepted movement, ground crossing reframed, creative breaking and placing with
/// `block_update` then sequence acknowledgement, light recomputed around a glowstone.
#[test]
fn solo_play() {
    use rc_client::Event;
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = fresh_copy(&world, "play");
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    let link = &s.link;
    let [x0, y0, z0] = p.pos;
    // Legal movement of one block to the south.
    link.send_move(rc_client::link::Move::Pos { pos: [x0, y0, z0 + 1.0] }, true, false).unwrap();
    link.send_tick_end().unwrap();
    // Ground crossing: reframing.
    link.send_move(rc_client::link::Move::Pos { pos: [x0, y0 - 2.0, z0 + 1.0] }, true, false).unwrap();
    let (tp, _) = wait_for(&s, |e| matches!(e, Event::Teleport(_)));
    let Some(Event::Teleport(back)) = tp else { panic!("no reframing") };
    assert_eq!(back.pos, [x0, y0, z0 + 1.0]);
    link.send_accept_teleport(back.id, back.pos, back.yaw, back.pitch).unwrap();
    // Breaking (creative) of the grass under (x0 + 2).
    let ground = [x0.floor() as i32 + 2, y0 as i32 - 1, z0.floor() as i32];
    link.send_player_action(0, ground, 1, 7).unwrap();
    let (u, _) = wait_for(&s, |e| matches!(e, Event::BlockUpdate { pos, .. } if [pos.x, pos.y, pos.z] == ground));
    let Some(Event::BlockUpdate { state, .. }) = u else { panic!("no block_update") };
    assert!(data.block_of(state).is_some_and(|b| b.name == "minecraft:air"), "{state}");
    let (a, _) = wait_for(&s, |e| matches!(e, Event::BlockChangedAck(_)));
    assert!(matches!(a, Some(Event::BlockChangedAck(7))));
    // Placing a glowstone on the neighbouring grass (item put in the hotbar in creative).
    let items = &data.registry("minecraft:item").unwrap().entries;
    let glow = items.iter().position(|e| e == "minecraft:glowstone").unwrap() as i32;
    let stack = rc_proto_v777::item::ItemStack { item: glow, count: 1, added: Vec::new(), removed: Vec::new() };
    link.send_carried_item(0).unwrap();
    link.send_set_creative_slot(36, Some(&stack)).unwrap();
    let base = [ground[0] + 1, ground[1], ground[2]];
    link.send_use_item_on(base, 1, [0.5, 1.0, 0.5], false, 8).unwrap();
    let above = [base[0], base[1] + 1, base[2]];
    let (u, _) = wait_for(&s, |e| {
        matches!(e, Event::BlockUpdate { pos, state } if [pos.x, pos.y, pos.z] == above
            && data.block_of(*state).is_some_and(|b| b.name == "minecraft:glowstone"))
    });
    assert!(u.is_some(), "glowstone not placed");
    let col = (above[0] >> 4, above[2] >> 4);
    let (l, _) = wait_for(&s, |e| matches!(e, Event::LightUpdate { x, z, .. } if (*x, *z) == col));
    let Some(Event::LightUpdate { x, z, light }) = l else { panic!("no light_update") };
    assert_eq!((x, z), (above[0] >> 4, above[2] >> 4));
    // Block light next to the glowstone: 14.
    let side = [above[0], above[1] + 1, above[2]];
    let ly = side[1] + 64;
    let layer = light.block[(ly >> 4) as usize + 1].as_ref().expect("layer");
    let i = rc_world::block_index((side[0] & 15) as usize, (ly & 15) as usize, (side[2] & 15) as usize);
    assert_eq!((layer[i >> 1] >> ((i & 1) * 4)) & 15, 14);
    s.link.disconnect();
    handle.wait();
    let _ = std::fs::remove_dir_all(&dir);
}

/// All the files of a folder (relative path → bytes), without the lock.
fn snapshot(dir: &std::path::Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut out = std::collections::BTreeMap::new();
    fn walk(root: &std::path::Path, d: &std::path::Path, out: &mut std::collections::BTreeMap<String, Vec<u8>>) {
        for e in std::fs::read_dir(d).unwrap().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(root, &p, out);
            } else if e.file_name() != "session.lock" {
                let rel = p.strip_prefix(root).unwrap().to_string_lossy().replace('\\', "/");
                out.insert(rel, std::fs::read(&p).unwrap());
            }
        }
    }
    walk(dir, dir, &mut out);
    out
}

fn dat(bytes: &[u8]) -> rc_proto::nbt::Nbt {
    crate::nbtio::parse_root(&crate::nbtio::decompress_auto(bytes).unwrap()).unwrap()
}

/// Compound without the given keys (comparison of the unmanaged fields).
fn without(v: &rc_proto::nbt::Nbt, keys: &[&str]) -> rc_proto::nbt::Nbt {
    let mut v = v.clone();
    for k in keys {
        crate::nbtio::remove(&mut v, k);
    }
    v
}

/// All the chunks of a region file: index → decompressed NBT.
fn region_chunks(bytes: &[u8], tag: &str) -> std::collections::BTreeMap<usize, Vec<u8>> {
    let path = std::env::temp_dir().join(format!("rc-sim-region-{tag}-{}.mca", std::process::id()));
    std::fs::write(&path, bytes).unwrap();
    let mut f = crate::region::RegionFile::open(&path, false).unwrap();
    let present: Vec<usize> = f.present().collect();
    let out = present.into_iter().filter_map(|i| Some((i, f.read(i, 0, 0).ok()??))).collect();
    drop(f);
    let _ = std::fs::remove_file(&path);
    out
}

/// Round trip without modification: no region rewritten, `level.dat` and the player change
/// only on their managed fields, everything else byte for byte.
#[test]
fn save_round_trip_untouched() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = fresh_copy(&world, "rt0");
    let before = snapshot(&dir);
    let (s, handle, _) = start_test(&dir, std::sync::Arc::new(data));
    std::thread::sleep(Duration::from_millis(800));
    s.link.disconnect();
    handle.wait();
    let after = snapshot(&dir);
    let player = format!("players/data/{}.dat", crate::nbtio::uuid_text(rc_client::offline_uuid("AgentSolo")));
    for (k, v) in &before {
        let new = after.get(k).unwrap_or_else(|| panic!("{k} disappeared"));
        if k == "level.dat" {
            let (a, b) = (dat(v), dat(new));
            let keys = ["Time", "LastPlayed", "singleplayer_uuid", "version_history"];
            assert_eq!(without(a.get("Data").unwrap(), &keys), without(b.get("Data").unwrap(), &keys));
            assert_eq!(after.get("level.dat_old"), Some(v), "backup copy");
        } else if *k == player {
            let (a, b) = (dat(v), dat(new));
            // `XpSeed` null in the file: drawn at loading like the game (S7).
            let managed = ["abilities", "OnGround", "Inventory", "equipment", "XpSeed"];
            assert_eq!(without(&a, &managed), without(&b, &managed), "player");
        } else if k.starts_with("players/stats/") {
            // S9: play time and time counters advance during the session, as in the game.
            assert!(new.starts_with(b"{\n  \"stats\": {"), "{k}");
        } else if k != "level.dat_old" && *k != format!("{player}_old") {
            assert!(v == new, "{k} modified");
        }
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// Round trip with modifications: only the touched chunks are rewritten; in those,
/// unmanaged keys and unchanged sections identical; the world reads back with the changes.
#[test]
fn save_round_trip_modified() {
    use rc_client::Event;
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let dir = fresh_copy(&world, "rt1");
    let before = snapshot(&dir);
    let data = std::sync::Arc::new(data);
    let (s, handle, p) = start_test(&dir, data.clone());
    let link = &s.link;
    let ground = [p.pos[0].floor() as i32 + 2, p.pos[1] as i32 - 1, p.pos[2].floor() as i32];
    link.send_player_action(0, ground, 1, 1).unwrap();
    let items = &data.registry("minecraft:item").unwrap().entries;
    let glow = items.iter().position(|e| e == "minecraft:glowstone").unwrap() as i32;
    let stack = rc_proto_v777::item::ItemStack { item: glow, count: 1, added: Vec::new(), removed: Vec::new() };
    link.send_set_creative_slot(36, Some(&stack)).unwrap();
    let base = [ground[0] + 1, ground[1], ground[2]];
    link.send_use_item_on(base, 1, [0.5, 1.0, 0.5], false, 2).unwrap();
    let (a, _) = wait_for(&s, |e| matches!(e, Event::BlockChangedAck(2)));
    assert!(a.is_some());
    s.link.disconnect();
    let t = Instant::now();
    handle.wait();
    eprintln!("closing (save included): {:.1} ms", t.elapsed().as_secs_f64() * 1e3);
    let after = snapshot(&dir);
    let mut rewritten = Vec::new();
    for (k, v) in &before {
        if !k.contains("/region/") {
            continue;
        }
        let (old, new) = (region_chunks(v, "a"), region_chunks(&after[k], "b"));
        assert_eq!(old.keys().collect::<Vec<_>>(), new.keys().collect::<Vec<_>>(), "{k}");
        for (i, o) in &old {
            if o == &new[i] {
                continue;
            }
            rewritten.push((k.clone(), *i));
            let (a, b) = (crate::nbtio::parse_root(o).unwrap(), crate::nbtio::parse_root(&new[i]).unwrap());
            // `InhabitedTime` advances by one per tick in the spawn chunks, like the game.
            let managed = [
                "sections",
                "Heightmaps",
                "LastUpdate",
                "block_entities",
                "block_ticks",
                "fluid_ticks",
                "InhabitedTime",
            ];
            assert_eq!(without(&a, &managed), without(&b, &managed), "{k} [{i}]: unmanaged keys");
            // Sections: unchanged apart from light, except that of the changed blocks.
            let secs = |v: &rc_proto::nbt::Nbt| crate::nbtio::list(v.get("sections")).to_vec();
            let (sa, sb) = (secs(&a), secs(&b));
            for x in &sa {
                let y = x.get("Y").and_then(|v| v.as_i64()).unwrap();
                let Some(z) = sb.iter().find(|z| z.get("Y").and_then(|v| v.as_i64()) == Some(y)) else {
                    panic!("{k} [{i}]: section {y} lost")
                };
                let light = ["SkyLight", "BlockLight"];
                if y != i64::from(ground[1] >> 4) {
                    assert_eq!(without(x, &light), without(z, &light), "{k} [{i}]: section {y}");
                }
                assert_eq!(x.get("biomes"), z.get("biomes"), "{k} [{i}]: biomes {y}");
            }
        }
    }
    eprintln!("chunks rewritten: {rewritten:?}");
    assert!(!rewritten.is_empty() && rewritten.len() <= 9, "{rewritten:?}");
    // Re-read: changes present, palettes consistent.
    let states = crate::states::States::new(
        &data,
        crate::registries::load(&crate::registries::client_jar(&data).unwrap(), &data)
            .unwrap()
            .entries("minecraft:worldgen/biome")
            .to_vec(),
    );
    let dim = Dimension::new(&dir, "minecraft:overworld", crate::dimension_height(&data, "minecraft:overworld"));
    let col = (ground[0] >> 4, ground[2] >> 4);
    let loaded = dim.load(&[col], &data, &states);
    let Load::Loaded(c) = &loaded[0].1 else { panic!("{:?}", loaded[0].1) };
    let at = |p: [i32; 3]| {
        let ly = p[1] + 64;
        let i = rc_world::block_index((p[0] & 15) as usize, (ly & 15) as usize, (p[2] & 15) as usize);
        data.block_of(c.sections[(ly >> 4) as usize].blocks.get(i)).unwrap().name.clone()
    };
    assert_eq!(at(ground), "minecraft:air");
    assert_eq!(at([base[0], base[1] + 1, base[2]]), "minecraft:glowstone");
    assert!(c.light_on);
    let _ = std::fs::remove_dir_all(&dir);
}

/// The container encoder gives back the game's data: same palette (up to the order of
/// properties) and same `data` array, on all the sections of the world.
#[test]
fn container_encoding_matches_game() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let biomes: Vec<String> = data.biomes.iter().map(|b| b.name.clone()).collect();
    let states = States::new(&data, biomes);
    let dim = Dimension::new(&world, "minecraft:overworld", crate::dimension_height(&data, "minecraft:overworld"));
    let norm = |v: &rc_proto::nbt::Nbt| -> rc_proto::nbt::Nbt {
        let mut v = crate::nbtio::unwrap_hetero(v).clone();
        if let Some(rc_proto::nbt::Nbt::Compound(f)) = crate::nbtio::get_mut(&mut v, "properties") {
            f.sort_by(|a, b| a.0.cmp(&b.0));
        }
        v
    };
    let mut n = 0;
    for (_, l) in dim.load_everything(&data, &states) {
        let Load::Loaded(c) = l else { continue };
        for (i, s) in c.sections.iter().enumerate() {
            let y = i as i64 - 4;
            let Some(orig) = crate::nbtio::list(c.root.get("sections"))
                .iter()
                .find(|x| x.get("Y").and_then(|v| v.as_i64()) == Some(y))
                .cloned()
            else {
                continue;
            };
            let Some(ob) = orig.get("block_states") else { continue };
            let enc = crate::chunk::encode_blocks(&s.blocks, &data, &states);
            let pal =
                |v: &rc_proto::nbt::Nbt| crate::nbtio::list(v.get("palette")).iter().map(norm).collect::<Vec<_>>();
            assert_eq!(pal(ob), pal(&enc), "({}, {}) section {y}", c.x, c.z);
            assert_eq!(ob.get("data"), enc.get("data"), "({}, {}) section {y}", c.x, c.z);
            n += 1;
        }
    }
    eprintln!("{n} sections re-encoded identically");
}

/// All the chunks of the world read; palettes resolved, states and biomes within bounds.
#[test]
fn every_chunk_reads() {
    let (Some(world), Some(data)) = (test_world(), rc_testdata::version_data()) else { return };
    let level = Level::read(&world).expect("level.dat");
    assert_eq!(level.data_version, crate::chunk::DATA_VERSION);
    let biomes: Vec<String> = data.biomes.iter().map(|b| b.name.clone()).collect();
    let states = States::new(&data, biomes);
    for name in DIMENSIONS {
        let dim = Dimension::new(&world, name, crate::dimension_height(&data, name));
        let t = Instant::now();
        let all = dim.load_everything(&data, &states);
        let elapsed = t.elapsed();
        let (mut loaded, mut not_full, mut sections) = (0, 0, 0);
        for ((x, z), l) in &all {
            match l {
                Load::Loaded(c) => {
                    loaded += 1;
                    assert!(!c.lossy, "({x}, {z}): unknown identifier");
                    assert_eq!(c.sections.len(), dim.height.sections);
                    assert_eq!(c.sky.len(), dim.height.sections + 2);
                    for s in &c.sections {
                        sections += 1;
                        let mut v = [0u32; rc_world::SECTION_BLOCKS];
                        s.blocks.unpack_into(&mut v);
                        assert!(v.iter().all(|&s| (s as usize) < data.states.len()), "({x}, {z}): state outside the table");
                        let mut b = [0u32; rc_world::SECTION_BIOMES];
                        s.biomes.unpack_into(&mut b);
                        assert!(b.iter().all(|&b| (b as usize) < states.biome_names.len()), "({x}, {z}): biome");
                        assert_eq!(s.non_empty, crate::chunk::count_non_empty(&s.blocks, &states));
                    }
                }
                Load::Skipped(Skip::NotFull(_)) => not_full += 1,
                other => panic!("({x}, {z}) : {other:?}"),
            }
        }
        eprintln!(
            "{name}: {} chunks present, {loaded} loaded, {not_full} not \"full\", {sections} sections, {:.1} ms ({:.1} µs/chunk, {} threads)",
            all.len(),
            elapsed.as_secs_f64() * 1e3,
            elapsed.as_secs_f64() * 1e6 / all.len().max(1) as f64,
            rayon::current_num_threads()
        );
    }
}
