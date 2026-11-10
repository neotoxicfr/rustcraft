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

thread_local! {
    /// Batches of `run` timed phase by phase (`RUSTCRAFT_GENLOG` path) and their timing checked.
    static TIMED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    /// Timings of the timed batches of this thread (read by the phase micro-bench).
    static TIMINGS: std::cell::RefCell<Vec<super::BatchTiming>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Batch timing coherent with what the batch sent: phases add up to the batch, the light of
/// the new chunks counted in its own phase (≥: the light counter is process-wide, other tests add to it).
fn check_batch_timing(w: &mut Worker, fulls: usize, relights: usize) {
    use crate::generation_timing::Phase;
    let t = w.last_timing.take().expect("timed batch without its timing");
    let sum: std::time::Duration = t.phases.iter().sum();
    assert_eq!(sum, t.total, "phases must add up to the batch");
    let light = |p: Phase| t.light[p as usize].chunks as usize;
    assert!(light(Phase::LightAndConversion) >= fulls, "light of {fulls} new chunks outside its phase: {t:?}");
    assert!(light(Phase::Relight) >= relights, "light of {relights} relit chunks outside its phase: {t:?}");
    assert!(light(Phase::PostProcessing) >= t.mushroom_chunks, "mushroom light outside post-processing: {t:?}");
    assert!(t.decoration_groups.len() <= 9, "more than 9 decoration classes: {t:?}");
    assert!(t.single_threaded() <= t.total);
    TIMINGS.with_borrow_mut(|v| v.push(t));
}

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
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-gen-inflight-{tag}-{}", std::process::id())));
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

/// Generation thread's worker (without its thread) on a copy of `template`'s generation settings in
/// `dir`, and the receiving end of its outputs.
fn worker_in(
    data: &Arc<VersionData>,
    template: &Path,
    biomes: &[String],
    dir: &Path,
) -> (Worker, crossbeam_channel::Receiver<Out>) {
    let wgs = ["data", "minecraft", "world_gen_settings.dat"].iter().collect::<PathBuf>();
    std::fs::create_dir_all(dir.join("data").join("minecraft")).unwrap();
    std::fs::copy(template.join(&wgs), dir.join(&wgs)).unwrap();
    let (spec, seed) = super::read_spec(dir, DIM).expect("settings");
    let (tx, rx) = crossbeam_channel::unbounded();
    let setup = Setup {
        data: data.clone(),
        world: dir.to_owned(),
        dimension: DIM.into(),
        height: crate::dimension_height(data, DIM),
        biome_registry: biomes.to_vec(),
        spec,
        seed,
    };
    (Worker::new(setup, tx).expect("generator"), rx)
}

/// A batch sends the new light of its already returned neighbours before its new chunks, so the
/// server never holds a new chunk without the light it changed around it, whatever moment it polls
/// (see `Worker::process`). Two adjacent chunks, one batch each: the second batch relights
/// the first chunk.
#[test]
fn neighbour_light_leaves_before_the_new_chunks() {
    let (data, template, biomes) = template(2).expect("26.3 data missing (rc-data cache and client jar)");
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-gen-send-order-{}", std::process::id())));
    let (mut w, rx) = worker_in(&data, &template, &biomes, &dir);
    w.process(&[(0, 0)]);
    let _ = rx.try_iter().count();
    w.process(&[(1, 0)]);
    let kinds: Vec<String> = rx
        .try_iter()
        .map(|o| match o {
            Out::Full(c, _) => format!("full {} {}", c.x, c.z),
            Out::Relight(p, ..) => format!("relight {} {}", p.0, p.1),
            Out::Proto(p, _) => format!("proto {} {}", p.0, p.1),
            Out::Disabled(e) => format!("disabled {e}"),
            Out::Batch(s) => format!("batch {s}"),
            Out::ChangesApplied(n) => format!("seen {n}"),
        })
        .collect();
    let relight = kinds.iter().position(|k| k == "relight 0 0");
    let full = kinds.iter().position(|k| k == "full 1 0");
    assert!(relight.is_some() && full.is_some(), "second batch must relight (0, 0) and return (1, 0): {kinds:?}");
    assert!(relight < full, "new chunk sent before its neighbour's new light: {kinds:?}");
    drop(w);
    let _ = std::fs::remove_dir_all(&template);
}

fn hash_bytes(h: &mut impl Hasher, layers: &[rc_world::LightLayer]) {
    for l in layers {
        l.as_ref().map(|a| &a[..]).hash(h);
    }
}

/// Fingerprints (full chunks: NBT, final light and number of relights; proto-chunks never
/// full: last version sent, final flush included, what remains on disk).
/// `lazy`: lazy writing of proto-chunks (`RUSTCRAFT_OPT_PROTO_LAZY`).
#[allow(clippy::disallowed_types, reason = "test fingerprint: the same fixed SipHash for every run it compares")]
fn run(
    data: &Arc<VersionData>,
    template: &Path,
    biomes: &[String],
    keep: usize,
    disk: Disk,
    lazy: bool,
    batches: &[Vec<(i32, i32)>],
) -> BTreeMap<(i32, i32), (u64, u64)> {
    // A folder of its own per call: `scratch` deletes it when the creating thread ends, and when
    // that thread belongs to a rayon pool (the per-thread-count bench), the pool of the previous
    // call ends while the next call with the same settings is already using its folder.
    static CALLS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let call = CALLS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tag = batches.len();
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!(
        "rc-gen-inflight-{tag}-{keep}-{disk:?}-{lazy}-{}-{call}",
        std::process::id()
    )));
    let height = crate::dimension_height(data, DIM);
    let (mut w, rx) = worker_in(data, template, biomes, &dir);
    w.keep = keep;
    w.lazy = lazy;
    w.timed = TIMED.get();
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
        let mut made = 0;
        match batch {
            Some(b) => made = w.process(b),
            None => w.flush(),
        }
        t_gen += t.elapsed();
        if batch.is_none() {
            t_flush = t.elapsed();
        }
        let mut touched = Vec::new();
        let (mut batch_fulls, mut batch_relights) = (0, 0);
        for o in rx.try_iter() {
            match o {
                Out::Full(c, _) => {
                    batch_fulls += 1;
                    touched.push((c.x, c.z));
                    assert!(fulls.insert((c.x, c.z), *c).is_none(), "full chunk returned twice");
                }
                Out::Relight(p, sky, block) => {
                    let c = fulls.get_mut(&p).expect("relight of a returned full chunk");
                    c.sky = sky;
                    c.block = block;
                    batch_relights += 1;
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
                Out::ChangesApplied(_) => {}
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
        if w.timed && made > 0 {
            check_batch_timing(&mut w, batch_fulls, batch_relights);
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
    let (data, template, biomes) = template(batches.len()).expect("26.3 data missing (rc-data cache and client jar)");
    let reference = run(&data, &template, &biomes, usize::MAX, Disk::Slow, false, batches);
    assert!(reference.len() > min, "walk too short: {}", reference.len());
    assert!(reference.values().any(|v| v.1 != 0), "no proto-chunk stayed proto: nothing is compared");
    let runs =
        [(16, Disk::Slow), (16, Disk::Fast), (16, Disk::Save), (usize::MAX, Disk::Save), (usize::MAX, Disk::Slow)];
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

/// The per-phase clocks of the generation log change nothing that is generated, written or
/// relit (eager and lazy proto writes, with evictions), and every timed batch is coherent.
#[test]
fn batch_timing_changes_nothing() {
    // Six batches: `run` names its folders after the batch count, no other test walks six.
    let walk = batches()[..6].to_vec();
    let (data, template, biomes) = template(walk.len()).expect("26.3 data missing (rc-data cache and client jar)");
    for (keep, disk, lazy) in [(usize::MAX, Disk::Slow, false), (16, Disk::Save, true)] {
        let untimed = run(&data, &template, &biomes, keep, disk, lazy, &walk);
        TIMED.set(true);
        let timed = run(&data, &template, &biomes, keep, disk, lazy, &walk);
        TIMED.set(false);
        assert!(untimed.len() > 100, "walk too short: {}", untimed.len());
        let d = diff(&untimed, &timed);
        assert!(
            d.is_empty(),
            "cap {keep}, {disk:?}, lazy {lazy}: timing changed {} chunks, including {:?}",
            d.len(),
            &d[..d.len().min(8)]
        );
    }
    let _ = std::fs::remove_dir_all(&template);
}

/// A forgotten and already written neighbour is read back from disk for the relight, not counted as absent.
#[test]
fn relight_rereads_written_neighbour() {
    same_everywhere(&relight_batches(), 100);
}

/// Thread side of the server's block changes: applied to a chunk held at once; kept for a chunk
/// not held until a save writes them, and whatever the save says while that chunk is set aside
/// (its copy predates them); applied when the chunk comes back; reported once.
#[test]
fn server_changes_wait_for_their_chunk_unless_saved() {
    use std::sync::atomic::Ordering;
    let (data, template, biomes) = template(11).expect("26.3 data missing (rc-data cache and client jar)");
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-gen-server-changes-{}", std::process::id())));
    let (mut w, rx) = worker_in(&data, &template, &biomes, &dir);
    w.process(&[(0, 0)]);
    let _ = rx.try_iter().count();
    let (held, away, state) = ([3, 70, 3], [5 * 16 + 1, 70, 1], 77);
    w.apply_server_changes(1, vec![(held, state), (away, state)]);
    let at = |w: &Worker, p: [i32; 3]| {
        let s = w.map.get(&(p[0] >> 4, p[2] >> 4))?;
        Some(s.c.blocks[s.c.index(p[0], p[1], p[2])])
    };
    assert_eq!(at(&w, held), Some(state), "chunk held not updated");
    assert_eq!(w.server_changes.chunks_not_on_disk(), 2);
    w.report_server_changes_seen();
    w.report_server_changes_seen();
    let reported: Vec<u64> =
        rx.try_iter().filter_map(|o| if let Out::ChangesApplied(n) = o { Some(n) } else { None }).collect();
    assert_eq!(reported, vec![1], "message 1 must be reported exactly once");
    // Chunk (5, 0) set aside: its changes stay although a save wrote them.
    let (min_y, height) = (w.height.min_y, w.height.sections as i32 * 16);
    let air = super::pack(&vec![0; (16 * 16 * height) as usize]);
    w.parked.insert((5, 0), super::Parked::Full { runs: air, min_y, height, sent: 1 });
    w.server_changes.saved.store(1, Ordering::Release);
    w.forget_saved_server_changes();
    assert_eq!(w.server_changes.chunks_not_on_disk(), 1, "only the set-aside chunk keeps its changes");
    w.restore(&[(5, 0)]);
    assert_eq!(at(&w, away), Some(state), "chunk back from the waiting table without the server's change");
    w.forget_saved_server_changes();
    assert_eq!(w.server_changes.chunks_not_on_disk(), 0, "saved changes of a chunk no longer set aside kept");
    drop(w);
    let _ = std::fs::remove_dir_all(&template);
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

/// Micro-bench (run by hand, `--ignored --nocapture`): where a batch's time goes at 1, 2, 4…
/// threads up to the logical cores. Flight of 12 rows of 24 chunks (the server's batch size),
/// default cap, fast writer, lazy protos. Per thread count: ms per chunk of each phase and the
/// single-threaded share (Amdahl). The test build lights every chunk twice (check of
/// `light_batch`): "light and conversion" and "relight" read about double the release.
#[test]
#[ignore]
fn bench_batch_phases_by_threads() {
    use crate::generation_timing::PHASES;
    let batches: Vec<Vec<(i32, i32)>> = (0..12).map(|z| (-12..12).map(|x| (x, 2 * z)).collect()).collect();
    let (data, template, biomes) = template(batches.len()).expect("26.3 data missing");
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    let counts: Vec<usize> =
        std::iter::successors(Some(1), |n| Some(n * 2)).take_while(|&n| n < cores).chain([cores]).collect();
    for threads in counts {
        let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("pool");
        let timings = pool.install(|| {
            TIMED.set(true);
            run(&data, &template, &biomes, super::KEEP, Disk::Fast, true, &batches);
            TIMED.set(false);
            TIMINGS.take()
        });
        let chunks = (batches.len() * batches[0].len()) as f64;
        let ms = |d: std::time::Duration| d.as_secs_f64() * 1e3;
        let total: f64 = timings.iter().map(|t| ms(t.total)).sum();
        let single: f64 = timings.iter().map(|t| ms(t.single_threaded())).sum();
        let phases: Vec<String> = PHASES
            .iter()
            .map(|p| format!("{} {:.3}", p.name(), timings.iter().map(|t| ms(t.phase(*p))).sum::<f64>() / chunks))
            .collect();
        eprintln!(
            "{threads:>2} threads: {:.3} ms/chunk, single-threaded {:.1} %; ms/chunk: {}",
            total / chunks,
            100.0 * single / total.max(1e-9),
            phases.join(", ")
        );
    }
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
