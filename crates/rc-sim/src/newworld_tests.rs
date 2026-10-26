//! Round trip of a new world: creation, generation of a few hundred chunks
//! while the write thread saves (concurrent reads and writes of the same
//! regions, as in game), then re-reading everything with no invalid slot.

use std::path::Path;
use std::sync::Arc;

use crate::generation::Out;
use crate::newworld::{NewWorld, create_world};
use crate::region::RegionFile;
use crate::save::{Job, Writer};

/// (invalid slots, chunks present, unreadable chunks) of the regions of a folder.
fn scan(dir: &Path) -> (usize, usize, usize) {
    let (mut bad, mut n, mut unreadable) = (0, 0, 0);
    let Ok(rd) = std::fs::read_dir(dir) else { return (0, 0, 0) };
    for e in rd.flatten() {
        let p = e.path();
        if p.extension().is_none_or(|x| x != "mca") {
            continue;
        }
        let mut r = RegionFile::open(&p, false).expect("region");
        bad += r.dropped;
        let name = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let parts: Vec<i32> = name.split('.').skip(1).filter_map(|v| v.parse().ok()).collect();
        let present: Vec<usize> = r.present().collect();
        for i in present {
            n += 1;
            let (cx, cz) = (parts[0] * 32 + (i % 32) as i32, parts[1] * 32 + (i / 32) as i32);
            match r.read(i, cx, cz) {
                Ok(Some(b)) if crate::nbtio::parse_root(&b).is_ok() => {}
                _ => unreadable += 1,
            }
        }
    }
    (bad, n, unreadable)
}

/// Real run: new world opened by the integrated server (generation around the player,
/// automatic and closing saves), two sessions in a row, regions re-read after each.
#[test]
fn new_world_regions_survive_two_sessions() {
    use std::time::{Duration, Instant};
    let Some(data) = rc_testdata::version_data() else { return };
    let data = Arc::new(data);
    if crate::registries::client_jar(&data).is_none() {
        return;
    }
    let dir = std::env::temp_dir().join(format!("rc-newworld-sess-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut w = NewWorld::new("sessions");
    w.seed = 42;
    create_world(&dir, &data, &w).expect("creation");
    let region = crate::world::dimension_dir(&dir, "minecraft:overworld").join("region");
    for round in 0..2 {
        let opts = crate::Options {
            world: dir.clone(),
            name: "AgentX23".into(),
            uuid: rc_client::offline_uuid("AgentX23"),
            info: rc_proto_v777::play::ClientInformation { view_distance: 10, ..Default::default() },
            sim_distance: 12,
        };
        crate::tests::solo_slot();
        let (session, handle) = crate::start(opts, data.clone()).expect("solo");
        let t = Instant::now();
        let mut chunks = 0;
        while t.elapsed() < Duration::from_secs(25) {
            let Ok(e) = session.events.recv_timeout(Duration::from_millis(100)) else { continue };
            match e {
                rc_client::Event::Teleport(p) => {
                    session.link.send_accept_teleport(p.id, p.pos, p.yaw, p.pitch).unwrap();
                    session.link.send_player_loaded().unwrap();
                }
                rc_client::Event::Chunk(_) => chunks += 1,
                rc_client::Event::Disconnected(r) => panic!("{r}"),
                _ => {}
            }
        }
        session.link.disconnect();
        handle.wait();
        let (bad, n, unreadable) = scan(&region);
        eprintln!("session {round}: {chunks} chunks received; {n} re-read, {bad} invalid, {unreadable} unreadable");
        assert_eq!((bad, unreadable), (0, 0), "session {round}");
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn new_world_regions_survive_generation_and_saves() {
    let Some(data) = rc_testdata::version_data() else { return };
    let data = Arc::new(data);
    let Some(jar) = crate::registries::client_jar(&data) else { return };
    let dir = std::env::temp_dir().join(format!("rc-newworld-rt-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut w = NewWorld::new("round-trip");
    w.seed = 1234567890123456789;
    create_world(&dir, &data, &w).expect("creation");
    let region = crate::world::dimension_dir(&dir, "minecraft:overworld").join("region");
    let (bad, n, unreadable) = scan(&region);
    assert_eq!((bad, unreadable), (0, 0), "new world: {n} chunks");

    // Generation concurrent with writes (like `poll_gen` + saves).
    let regs = crate::registries::load(&jar, &data).expect("registries");
    let mut g = crate::generation::start_for(&data, &dir, "minecraft:overworld", &regs).expect("generator");
    let height = crate::dimension_height(&data, "minecraft:overworld");
    let states = crate::states::States::new(&data, regs.entries("minecraft:worldgen/biome").to_vec());
    let writer = Writer::new();
    let mut want: Vec<(i32, i32)> = Vec::new();
    for r in 0..12i32 {
        for x in -r..=r {
            for z in -r..=r {
                if x.abs().max(z.abs()) == r {
                    want.push((x + 3, z - 2));
                }
            }
        }
    }
    for chunk in want.chunks(40) {
        g.want(chunk.to_vec());
    }
    let t = std::time::Instant::now();
    while !g.pending.is_empty() && t.elapsed().as_secs() < 600 {
        let Ok(o) = g.rx.recv_timeout(std::time::Duration::from_millis(200)) else { continue };
        let job = match o {
            Out::Full(c, _) => {
                g.pending.remove(&(c.x, c.z));
                let Some(n) = c.encode(&data, &states, height, 0) else { continue };
                Job::Chunk {
                    path: region.join(crate::region::region_name(c.x, c.z)),
                    cx: c.x,
                    cz: c.z,
                    nbt: crate::nbtio::encode_root(&n),
                }
            }
            Out::Proto((cx, cz), nbt) => {
                Job::Chunk { path: region.join(crate::region::region_name(cx, cz)), cx, cz, nbt }
            }
            Out::Relight(..) | Out::Batch(_) => continue,
            Out::Disabled(e) => panic!("generation: {e}"),
        };
        writer.send(vec![job]);
    }
    assert!(g.pending.is_empty(), "generation unfinished");
    g.stop();
    while let Ok(o) = g.rx.try_recv() {
        if let Out::Proto((cx, cz), nbt) = o {
            writer.send(vec![Job::Chunk { path: region.join(crate::region::region_name(cx, cz)), cx, cz, nbt }]);
        }
    }
    writer.finish();
    let (bad, n, unreadable) = scan(&region);
    eprintln!("round trip: {n} chunks re-read, {bad} invalid slots, {unreadable} unreadable");
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!((bad, unreadable), (0, 0));
    assert!(n >= want.len());
}
