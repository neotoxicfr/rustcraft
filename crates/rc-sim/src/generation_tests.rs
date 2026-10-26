//! Disk race of the generation thread: a chunk forgotten from memory then requested again before
//! its write is on disk must come back as it was (decorations received from neighbours
//! included). The result therefore depends neither on the memory cap nor on the write speed.

use std::collections::BTreeMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rc_data::VersionData;

use super::{Out, Setup, Worker};

const DIM: &str = "minecraft:overworld";

/// Simulated write thread.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Disk {
    /// Nothing reaches the disk during the walk (late writer).
    Slow,
    /// Each proto-chunk is written and acknowledged as soon as it leaves the batch.
    Fast,
    /// Like `Fast`, and a save after each batch: full chunks received or read back written
    /// (encoded like `chunk_jobs`), then acknowledged.
    Save,
}

/// New world: only its generation settings are used (copied into each pass).
fn template(tag: usize) -> Option<(Arc<VersionData>, PathBuf, Vec<String>)> {
    let data = Arc::new(rc_testdata::version_data()?);
    let jar = crate::registries::client_jar(&data)?;
    let regs = crate::registries::load(&jar, &data).ok()?;
    let dir = std::env::temp_dir().join(format!("rc-gen-inflight-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let mut w = crate::newworld::NewWorld::new("layer");
    w.seed = 20_261_022;
    crate::newworld::create_world(&dir, &data, &w).expect("creation");
    Some((data, dir, regs.entries("minecraft:worldgen/biome").to_vec()))
}

/// 3×3 batches: a first group, a distant group (the first is forgotten under a small
/// cap), then batches that touch both: their decorated neighbours are requested again, and
/// full chunks already returned become neighbours again (light, post-processing, relight).
fn batches() -> Vec<Vec<(i32, i32)>> {
    let around = |x: i32, z: i32| -> Vec<(i32, i32)> {
        (-1..=1).flat_map(|dz| (-1..=1).map(move |dx| (x + dx, z + dz))).collect()
    };
    vec![around(0, 0), around(12, 0), around(3, 0), around(8, 0), around(0, 4), around(10, 3), around(5, 3)]
}

/// Relight with a forgotten and already written side neighbour, six times (z = 0, 6 … 30: light
/// coming from the side does not exist everywhere): (0, z) full, (-1, z) decorated; the distant batch
/// makes them forgotten; (1, z) targeted makes (0, z) relight, whose neighbour (-1, z) is two chunks from
/// any target and is only read back for the relight.
fn relight_batches() -> Vec<Vec<(i32, i32)>> {
    let col = |x: i32| (0..6).map(|i| (x, 6 * i)).collect::<Vec<_>>();
    vec![col(0), vec![(40, 0)], col(1)]
}

fn hash_bytes(h: &mut impl Hasher, layers: &[rc_world::LightLayer]) {
    for l in layers {
        l.as_ref().map(|a| &a[..]).hash(h);
    }
}

/// Fingerprints (full chunks: NBT, final light and number of relights; proto-chunks never
/// full: last version sent, final flush included, what remains on disk).
/// `lazy`: lazy writing of proto-chunks (`RUSTCRAFT_OPT_PROTO_LAZY`).
fn run(
    data: &Arc<VersionData>,
    template: &Path,
    biomes: &[String],
    keep: usize,
    disk: Disk,
    lazy: bool,
    batches: &[Vec<(i32, i32)>],
) -> BTreeMap<(i32, i32), (u64, u64)> {
    let tag = batches.len();
    let dir = std::env::temp_dir()
        .join(format!("rc-gen-inflight-{tag}-{keep}-{disk:?}-{lazy}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let wgs = ["data", "minecraft", "world_gen_settings.dat"].iter().collect::<PathBuf>();
    std::fs::create_dir_all(dir.join("data").join("minecraft")).unwrap();
    std::fs::copy(template.join(&wgs), dir.join(&wgs)).unwrap();
    let (spec, seed) = super::read_spec(&dir, DIM).expect("settings");
    let height = crate::dimension_height(data, DIM);
    let (tx, rx) = crossbeam_channel::unbounded();
    let setup = Setup {
        data: data.clone(),
        world: dir.clone(),
        dimension: DIM.into(),
        height,
        biome_registry: biomes.to_vec(),
        spec,
        seed,
    };
    let mut w = Worker::new(setup, tx).expect("generator");
    w.keep = keep;
    w.lazy = lazy;
    let region = crate::world::dimension_dir(&dir, DIM).join("region");
    let mut fulls: hashbrown::HashMap<(i32, i32), crate::chunk::Chunk> = hashbrown::HashMap::new();
    let mut relit: hashbrown::HashMap<(i32, i32), u32> = hashbrown::HashMap::new();
    let mut protos: hashbrown::HashMap<(i32, i32), Vec<u8>> = hashbrown::HashMap::new();
    let write = |p: (i32, i32), b: &[u8]| {
        std::fs::create_dir_all(&region).unwrap();
        let mut f = crate::region::RegionFile::open_or_create(&region.join(crate::region::region_name(p.0, p.1)))
            .expect("region");
        f.write(crate::region::local_index(p.0, p.1), p.0, p.1, b).expect("write");
    };
    let (mut proto_writes, mut proto_bytes) = (0usize, 0usize);
    let (mut t_gen, mut t_flush) = (std::time::Duration::ZERO, std::time::Duration::ZERO);
    // Last round: closing flush (no batch).
    for batch in batches.iter().map(Some).chain([None]) {
        let t = std::time::Instant::now();
        match batch {
            Some(b) => {
                w.process(b);
            }
            None => w.flush(),
        }
        t_gen += t.elapsed();
        if batch.is_none() {
            t_flush = t.elapsed();
        }
        let mut touched = Vec::new();
        for o in rx.try_iter() {
            match o {
                Out::Full(c, _) => {
                    touched.push((c.x, c.z));
                    assert!(fulls.insert((c.x, c.z), *c).is_none(), "full chunk returned twice");
                }
                Out::Relight(p, sky, block) => {
                    let c = fulls.get_mut(&p).expect("relight of a returned full chunk");
                    c.sky = sky;
                    c.block = block;
                    touched.push(p);
                    *relit.entry(p).or_default() += 1;
                }
                Out::Proto(p, b) => {
                    if disk != Disk::Slow {
                        write(p, &b);
                    }
                    proto_writes += 1;
                    proto_bytes += b.len();
                    protos.insert(p, b);
                }
                Out::Disabled(e) => panic!("generation: {e}"),
                Out::Batch(_) if disk == Disk::Slow => {}
                // Everything before is written, acknowledged like `poll_gen` (and `chunk_jobs`).
                Out::Batch(s) => {
                    if disk == Disk::Save {
                        for p in &touched {
                            let nbt = fulls[p].encode(data, w.states(), height, 0).expect("encoding");
                            write(*p, &crate::nbtio::encode_root(&nbt));
                        }
                        w.saved.fetch_max(s, std::sync::atomic::Ordering::Release);
                    }
                    w.written.fetch_max(s, std::sync::atomic::Ordering::Release);
                }
            }
        }
    }
    eprintln!(
        "cap {keep}, {disk:?}, lazy {lazy}: {} batches in {:.0} ms (final flush {:.1} ms), {proto_writes} proto-chunks sent ({} MB of NBT), {} full chunks",
        batches.len(),
        t_gen.as_secs_f64() * 1e3,
        t_flush.as_secs_f64() * 1e3,
        proto_bytes >> 20,
        fulls.len()
    );
    let mut out = BTreeMap::new();
    for (p, c) in &fulls {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        let nbt = c.encode(data, w.states(), height, 0).expect("encoding");
        crate::nbtio::encode_root(&nbt).hash(&mut h);
        hash_bytes(&mut h, &c.sky);
        hash_bytes(&mut h, &c.block);
        relit.get(p).hash(&mut h);
        out.insert(*p, (h.finish(), 0));
    }
    for (p, b) in protos.iter().filter(|(p, _)| !fulls.contains_key(*p)) {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        b.hash(&mut h);
        out.entry(*p).or_insert((0, 0)).1 = h.finish();
    }
    // Layer cost: set aside at the end of the walk, bytes of the blocks per run, compaction
    // time of a chunk (blocks of the table).
    let (mut protos_n, mut fulls_n, mut runs_b) = (0, 0, 0);
    for k in w.parked.values() {
        match k {
            super::Parked::Proto(_) => protos_n += 1,
            super::Parked::Full { runs, .. } => {
                fulls_n += 1;
                runs_b += runs.len() * 8;
            }
        }
    }
    let t = std::time::Instant::now();
    let packed: usize = w.map.values().map(|s| super::pack(&s.c.blocks).len() * 8).sum();
    let per = t.elapsed().as_secs_f64() * 1e3 / w.map.len().max(1) as f64;
    eprintln!(
        "cap {keep}, {disk:?}: table {} ({} full chunks), set aside {protos_n} protos, {fulls_n} full chunks ({} KB), returned {}; runs {:.0} KB/chunk, {per:.2} ms/chunk",
        w.map.len(),
        w.map.values().filter(|s| s.stage == super::Stage::Full).count(),
        runs_b / 1024,
        w.made.len(),
        packed as f64 / 1024.0 / w.map.len().max(1) as f64
    );
    drop(w);
    let _ = std::fs::remove_dir_all(&dir);
    out
}

fn diff(a: &BTreeMap<(i32, i32), (u64, u64)>, b: &BTreeMap<(i32, i32), (u64, u64)>) -> Vec<(i32, i32)> {
    a.keys()
        .chain(b.keys())
        .filter(|p| a.get(p) != b.get(p))
        .copied()
        .collect::<std::collections::BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Blocks of a full chunk set aside: runs returned identically.
#[test]
fn packed_blocks_round_trip() {
    let mut blocks: Vec<u32> = vec![0; 98_304];
    for (i, b) in blocks.iter_mut().enumerate() {
        *b = match i % 4099 {
            0..=2000 => 1,
            2001 => 7,
            n => (n % 3) as u32,
        };
    }
    let runs = super::pack(&blocks);
    assert!(runs.len() < blocks.len());
    let mut back = vec![9; 5];
    super::unpack(&runs, &mut back);
    assert_eq!(back, blocks);
    super::unpack(&super::pack(&[]), &mut back);
    assert!(back.is_empty());
}

/// Walks `batches` without cap or write, protos written at each batch (reference), then under
/// a cap of 16 chunks with each writer, and without cap with save, protos written
/// at each batch then lazily: everything must coincide.
fn same_everywhere(batches: &[Vec<(i32, i32)>], min: usize) {
    let (data, template, biomes) =
        template(batches.len()).expect("26.3 data missing (rc-data cache and client jar)");
    let reference = run(&data, &template, &biomes, usize::MAX, Disk::Slow, false, batches);
    assert!(reference.len() > min, "walk too short: {}", reference.len());
    assert!(reference.values().any(|v| v.1 != 0), "no proto-chunk stayed proto: nothing is compared");
    let runs = [(16, Disk::Slow), (16, Disk::Fast), (16, Disk::Save), (usize::MAX, Disk::Save), (usize::MAX, Disk::Slow)];
    let mut bad = Vec::new();
    for lazy in [false, true] {
        for (keep, disk) in runs {
            let d = diff(&reference, &run(&data, &template, &biomes, keep, disk, lazy, batches));
            eprintln!("{} chunks; cap {keep}, {disk:?}, lazy {lazy}: {} differences", reference.len(), d.len());
            if !d.is_empty() {
                bad.push(format!(
                    "cap {keep}, {disk:?}, lazy {lazy}: {} different, including {:?}",
                    d.len(),
                    &d[..d.len().min(8)]
                ));
            }
        }
    }
    let _ = std::fs::remove_dir_all(&template);
    assert!(bad.is_empty(), "{bad:?}");
}

/// Under a cap of 16 chunks, neither a late writer nor the saves change the
/// result: forgotten chunks not yet on disk are taken back from memory.
#[test]
fn evicted_chunks_survive_slow_writer() {
    same_everywhere(&batches(), 100);
}

/// A forgotten and already written neighbour is read back from disk for the relight, not counted as absent.
#[test]
fn relight_rereads_written_neighbour() {
    same_everywhere(&relight_batches(), 100);
}

/// Server side: the save acknowledgement follows all the chunk writes of the save
/// and carries the last batch received; after an awaited save, it is returned to the thread.
#[test]
fn save_ack_follows_chunk_writes() {
    let Some((mut s, dir)) = crate::portal_tests::open_new_world("gen-ack", 20_261_022) else {
        panic!("26.3 data missing (rc-data cache and client jar)");
    };
    s.load_now(&[(0, 0), (1, 0)]);
    // Bounded wait for the first batch (a fixed wait is not enough when the suite loads the machine).
    let t0 = std::time::Instant::now();
    while s.generator.as_ref().expect("generator").seen == 0 && t0.elapsed() < std::time::Duration::from_secs(60) {
        s.poll_gen();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let seen = s.generator.as_ref().expect("generator").seen;
    assert!(seen > 0, "no batch received");
    let mut jobs = Vec::new();
    assert!(s.chunk_jobs(&mut jobs) > 0, "no chunk to write");
    let last_chunk = jobs.iter().rposition(|j| matches!(j, crate::save::Job::Chunk { .. }));
    let ack = jobs.iter().position(|j| matches!(j, crate::save::Job::Ack(..)));
    assert!(ack > last_chunk, "acknowledgement before a write: {ack:?} / {last_chunk:?}");
    assert!(matches!(jobs[ack.unwrap()], crate::save::Job::Ack(_, n) if n == seen));
    s.save(true);
    // The flush of the save (lazy writing) adds a batch: the acknowledgement follows the last one.
    let g = s.generator.as_ref().expect("generator");
    assert!(g.seen >= seen);
    assert_eq!(g.saved.load(std::sync::atomic::Ordering::Acquire), g.seen);
    drop(s);
    let _ = std::fs::remove_dir_all(&dir);
}

/// Memory bench (run by hand, `--ignored`): straight-line flight, corridor 9 chunks
/// wide, 40 rows of one batch each, cap 64, no write (worst case: no
/// save). What remains in the table and set aside gives the cost per row.
#[test]
#[ignore]
fn long_flight_memory() {
    let rows = 40;
    let batches: Vec<Vec<(i32, i32)>> = (0..rows).map(|z| (-4..=4).map(|x| (x, z)).collect()).collect();
    let (data, template, biomes) = template(batches.len()).expect("26.3 data missing");
    run(&data, &template, &biomes, 64, Disk::Slow, true, &batches);
    let _ = std::fs::remove_dir_all(&template);
}

/// Micro-bench (run by hand, `--ignored --nocapture`): straight-line flight of 40 rows,
/// default cap, fast writer; protos written at each batch then lazily, all
/// cores then 1 thread. Gives the total time of the batches and the number of protos encoded and sent.
#[test]
#[ignore]
fn flight_proto_writes() {
    let rows = 40;
    let batches: Vec<Vec<(i32, i32)>> = (0..rows).map(|z| (-4..=4).map(|x| (x, z)).collect()).collect();
    let (data, template, biomes) = template(batches.len()).expect("26.3 data missing");
    let one = rayon::ThreadPoolBuilder::new().num_threads(1).build().expect("thread");
    for pass in 0..2 {
        for lazy in [false, true] {
            eprintln!("pass {pass}, all cores:");
            run(&data, &template, &biomes, super::KEEP, Disk::Fast, lazy, &batches);
            eprintln!("pass {pass}, 1 thread:");
            one.install(|| run(&data, &template, &biomes, super::KEEP, Disk::Fast, lazy, &batches));
        }
    }
    let _ = std::fs::remove_dir_all(&template);
}
