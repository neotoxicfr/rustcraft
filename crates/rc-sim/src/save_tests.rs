//! Proofs of the save: the parallel encoding hands over the serial one's jobs, every writer plan
//! leaves the same files, the forced writes cover exactly the regions written, libdeflate streams
//! read back to the same chunks, a crash before the rename keeps the old file, the region sync sits
//! where the game puts it; each deliberate fault is seen. Benches of the same paths (ignored).
//! On copies of the test world (`RUSTCRAFT_SOLO_WORLD`); absent: tests skipped.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use super::*;
use crate::nbtio::ZlibCompressor;
use crate::region::RegionFile;

/// A pool of `n` threads, whatever `RUSTCRAFT_THREADS` says.
fn pool(n: usize) -> rayon::ThreadPool {
    rayon::ThreadPoolBuilder::new().num_threads(n).build().expect("pool")
}

/// Logical cores, or `RUSTCRAFT_THREADS`.
fn cores() -> usize {
    crate::storage::worker_threads()
}

/// 1 thread, then powers of 2 up to every core.
fn thread_counts() -> Vec<usize> {
    let mut v: Vec<usize> = std::iter::successors(Some(1usize), |n| Some(n * 2)).take_while(|&n| n < cores()).collect();
    v.push(cores());
    v
}

// ---------------------------------------------------------------- snapshot: parallel encoding

/// Server on a copy of the test world, the chunks within `radius` of the player loaded.
fn open(tag: &str, radius: i32) -> Option<Server> {
    let (world, data) = (crate::tests::test_world()?, rc_testdata::version_data()?);
    let dir = crate::tests::fresh_copy(&world, tag);
    let mut s = crate::portal_tests::open_world(&dir, Arc::new(data));
    let (cx, cz) = ((s.pos[0].floor() as i32) >> 4, (s.pos[2].floor() as i32) >> 4);
    let list: Vec<(i32, i32)> =
        (cx - radius..=cx + radius).flat_map(|x| (cz - radius..=cz + radius).map(move |z| (x, z))).collect();
    s.load_now(&list);
    s.loaded = true;
    s.teleport = None;
    Some(s)
}

fn close(mut s: Server) {
    s.stop_gen();
    s.stop_parked();
}

/// Blocks and a chest placed over several chunks; `round` changes what is placed.
fn edit(s: &mut Server, round: i32) {
    let [x, y, z] = s.pos.map(|v| v.floor() as i32);
    let block = if round == 0 { "stone" } else { "oak_planks" };
    for (dx, dz) in [(-60, -60), (-20, 40), (30, -10), (50, 50)] {
        let (x0, y0, z0) = (x + dx, y + 3 + round, z + dz);
        crate::tests_opt::cmd(s, &format!("fill {x0} {y0} {z0} {} {y0} {} minecraft:{block}", x0 + 20, z0 + 20));
    }
    let chest = format!("minecraft:chest{{Items:[{{Slot:0b,id:\"minecraft:dirt\",count:{}}}]}}", 3 + round);
    crate::tests_opt::cmd(s, &format!("setblock {} {} {} {chest}", x + 5, y + 6, z + 5));
}

/// A chunk job as the proofs compare it: path inside the world, position, uncompressed NBT.
type SavedChunk = (PathBuf, i32, i32, Vec<u8>);

/// Every loaded chunk saved with `plan`: its chunk jobs, in their order.
fn save_all(s: &mut Server, plan: EncodePlan, pool: &rayon::ThreadPool) -> Vec<SavedChunk> {
    for c in s.chunks.values_mut().flatten() {
        c.dirty = true;
    }
    let mut jobs = Vec::new();
    s.chunk_jobs_with(&mut jobs, plan, Some(pool));
    let world = s.world.clone();
    jobs.into_iter()
        .filter_map(|j| match j {
            Job::Chunk { path, cx, cz, nbt } => {
                Some((path.strip_prefix(&world).map(Path::to_path_buf).unwrap_or(path), cx, cz, nbt))
            }
            _ => None,
        })
        .collect()
}

/// Two saves with edits before each, under `plan`.
fn two_saves(tag: &str, plan: EncodePlan) -> Option<[Vec<SavedChunk>; 2]> {
    let mut s = open(tag, 6)?;
    let pool = pool(4);
    edit(&mut s, 0);
    let first = save_all(&mut s, plan, &pool);
    edit(&mut s, 1);
    let second = save_all(&mut s, plan, &pool);
    close(s);
    Some([first, second])
}

/// The chunks encoded side by side give the jobs of the serial encoding, in the same order, at a
/// first save and at a second one (which reads what the first left in each chunk). Handing the
/// chunks over in another order, or not giving a chunk its compound back, is seen.
#[test]
fn parallel_encoding_gives_the_jobs_of_the_serial_one() {
    let serial = EncodePlan { parallel: false, sabotage: EncodeSabotage::None };
    let Some(reference) = two_saves("encode-serial", serial) else { return };
    assert!(reference[0].len() > 100, "{} chunks saved", reference[0].len());
    let parallel = EncodePlan { parallel: true, sabotage: EncodeSabotage::None };
    let got = two_saves("encode-parallel", parallel).expect("test world");
    assert!(got[0] == reference[0], "first save differs");
    assert!(got[1] == reference[1], "second save differs");
    let reversed = two_saves("encode-reversed", EncodePlan { sabotage: EncodeSabotage::ReversedOrder, ..parallel });
    assert!(reversed.expect("test world")[0] != reference[0], "order sabotage not seen");
    let root_lost =
        two_saves("encode-root-lost", EncodePlan { sabotage: EncodeSabotage::RootNotGivenBack, ..parallel });
    let root_lost = root_lost.expect("test world");
    assert!(root_lost[0] == reference[0] && root_lost[1] != reference[1], "lost compound not seen");
}

/// Autosave and pause push no region sync; closing pushes it after the chunks and before
/// `level.dat`, like the game. The old path syncs at every save, after `level.dat`.
#[test]
fn region_sync_comes_before_level_dat_only_at_closing() {
    let Some(mut s) = open("sync-order", 1) else { return };
    let position = |jobs: &[Job]| {
        let sync = jobs.iter().position(|j| matches!(j, Job::Sync));
        let level = jobs.iter().position(|j| matches!(j, Job::Dat { path, .. } if path.ends_with("level.dat")));
        let last_chunk = jobs.iter().rposition(|j| matches!(j, Job::Chunk { .. }));
        (sync, level, last_chunk)
    };
    let mut phases = SnapshotPhases::starting_at(Instant::now());
    for c in s.chunks.values_mut().flatten() {
        c.dirty = true;
    }
    let (autosave, _) = s.save_jobs(false, true, &mut phases);
    assert_eq!(position(&autosave).0, None, "autosave forces the regions");
    for c in s.chunks.values_mut().flatten() {
        c.dirty = true;
    }
    let (closing, written) = s.save_jobs(true, true, &mut phases);
    let (sync, level, last_chunk) = position(&closing);
    assert!(written > 0);
    assert!(sync > last_chunk && sync < level, "closing: sync {sync:?}, level.dat {level:?}, chunk {last_chunk:?}");
    let (old, _) = s.save_jobs(false, false, &mut phases);
    let (sync, level, _) = position(&old);
    assert!(sync > level, "old path: sync {sync:?} after level.dat {level:?}");
    close(s);
}

// ---------------------------------------------------------------- write thread

/// Decompressed chunks of the test world's region files in `folder` (overworld): at most `per_file`
/// from each file, files in name order.
fn real_chunks(folder: &str, per_file: usize) -> Option<Vec<(i32, i32, Vec<u8>)>> {
    let world = crate::tests::test_world()?;
    let dir = world.join("dimensions").join("minecraft").join("overworld").join(folder);
    let mut files: Vec<PathBuf> = std::fs::read_dir(&dir).ok()?.flatten().map(|e| e.path()).collect();
    files.sort();
    let mut out = Vec::new();
    for path in files {
        let name = path.file_name()?.to_str()?.to_owned();
        let mut parts = name.split('.');
        let (Some("r"), Some(rx), Some(rz), Some("mca")) = (parts.next(), parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        let (Ok(rx), Ok(rz)) = (rx.parse::<i32>(), rz.parse::<i32>()) else { continue };
        let mut region = RegionFile::open(&path, false).ok()?;
        let present: Vec<usize> = region.present().take(per_file).collect();
        for i in present {
            let (cx, cz) = (rx * 32 + (i % 32) as i32, rz * 32 + (i / 32) as i32);
            if let Ok(Some(bytes)) = region.read(i, cx, cz) {
                out.push((cx, cz, bytes));
            }
        }
    }
    Some(out)
}

/// Batches of a session: an autosave, a second autosave rewriting part of it (other sizes, an
/// oversized chunk going to its external stream), a closing save. Paths under `dir`.
fn session_batches(dir: &Path) -> Option<Vec<Vec<Job>>> {
    let folders = [
        ("region", real_chunks("region", 40)?),
        ("entities", real_chunks("entities", 15)?),
        ("poi", real_chunks("poi", 15)?),
    ];
    let chunk = |folder: &str, (cx, cz, nbt): &(i32, i32, Vec<u8>)| Job::Chunk {
        path: dir.join(folder).join(region_name(*cx, *cz)),
        cx: *cx,
        cz: *cz,
        nbt: nbt.clone(),
    };
    let dat = |name: &str, n: i64, backup: bool| Job::Dat {
        path: dir.join(name),
        nbt: nbtio::encode_root(&Nbt::Compound(vec![("n".into(), Nbt::Long(n))])),
        backup,
    };
    let done = || Job::Done(Arc::new(std::sync::atomic::AtomicBool::new(true)));
    let mut first = vec![dat("data/a.dat", 1, false), dat("data/b.dat", 1, false), dat("playerdata/p.dat", 1, true)];
    first.push(Job::Raw { path: dir.join("stats/p.json"), bytes: b"{\"n\":1}".to_vec() });
    for (folder, chunks) in &folders {
        first.extend(chunks.iter().map(|c| chunk(folder, c)));
    }
    first.extend([dat("level.dat", 1, true), Job::Sync, done()]);
    // Every third region chunk takes the compound of a chunk further on: other sizes.
    let (region, entities) = (&folders[0].1, &folders[1].1);
    let mut second: Vec<Job> = (0..region.len())
        .step_by(3)
        .map(|i| {
            let (cx, cz, _) = &region[i];
            chunk("region", &(*cx, *cz, region[(i + 7) % region.len()].2.clone()))
        })
        .collect();
    // Pseudo-random bytes: no compressor brings them under 255 sectors.
    let (bx, bz, _) = &region[2];
    let mut seed = 7u64;
    let oversized: Vec<u8> = (0..1_300_000)
        .map(|_| {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            (seed >> 33) as u8
        })
        .collect();
    second.push(chunk("region", &(*bx, *bz, oversized)));
    second.extend([dat("data/a.dat", 2, false), dat("playerdata/p.dat", 2, true), Job::Sync, done()]);
    let mut closing: Vec<Job> = entities.iter().step_by(4).map(|c| chunk("entities", c)).collect();
    closing.push(chunk("region", &region[1]));
    closing.extend([Job::Sync, dat("level.dat", 2, true), done()]);
    Some(vec![first, second, closing])
}

/// Every file under `dir`: path inside it and bytes, sorted.
fn files_under(dir: &Path) -> Vec<(PathBuf, Vec<u8>)> {
    fn walk(root: &Path, dir: &Path, out: &mut Vec<(PathBuf, Vec<u8>)>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let path = e.path();
            if path.is_dir() {
                walk(root, &path, out);
            } else {
                let bytes = std::fs::read(&path).unwrap_or_default();
                out.push((path.strip_prefix(root).map(Path::to_path_buf).unwrap_or(path), bytes));
            }
        }
    }
    let mut out = Vec::new();
    walk(dir, dir, &mut out);
    out.sort();
    out
}

/// What a session left: the files, and the regions each sync forced.
struct Session {
    files: Vec<(PathBuf, Vec<u8>)>,
    synced: Vec<Vec<PathBuf>>,
    dir: PathBuf,
}

/// Runs the session's batches through a write thread with `plan`, on this thread with the region
/// clock fixed (timestamps in the headers do not depend on the time of the run).
fn run_session(tag: &str, plan: WriterPlan) -> Option<Session> {
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-save-{tag}-{}", std::process::id())));
    let batches = session_batches(&dir)?;
    crate::region::tests::FIXED_NOW.with(|c| c.set(Some(1_700_000_000)));
    let mut w = WriteThread::new(plan);
    for batch in batches {
        w.run_batch(batch);
    }
    let synced = std::mem::take(&mut w.synced);
    w.finish();
    crate::region::tests::FIXED_NOW.with(|c| c.set(None));
    Some(Session { files: files_under(&dir), synced, dir })
}

const OLD_FLATE2: WriterPlan = WriterPlan::OLD;

const ALL_ON_FLATE2: WriterPlan = WriterPlan {
    parallel_compression: true,
    skip_clean_regions: true,
    pad_before_sync: true,
    parallel_file_sync: true,
    max_open_per_folder: Some(2),
    compressor: ZlibCompressor::Flate2,
    sabotage: WriterSabotage::None,
};

/// Every writer plan, each switch alone and all together (two region files open per folder at
/// most, to close and reopen them), leaves the files of the old path byte for byte: regions,
/// external streams, `.dat`, backups. Writing two chunks of a batch in the other order is seen.
#[test]
fn every_writer_plan_leaves_the_same_files() {
    let Some(reference) = run_session("old", OLD_FLATE2) else { return };
    assert!(reference.files.iter().any(|(p, _)| p.extension().is_some_and(|e| e == "mcc")), "no external stream");
    assert!(reference.files.iter().filter(|(p, _)| p.starts_with("region")).count() > 2, "too few region files");
    let alone = [
        ("parallel compression", WriterPlan { parallel_compression: true, ..OLD_FLATE2 }),
        ("skip clean regions", WriterPlan { skip_clean_regions: true, ..OLD_FLATE2 }),
        ("pad before sync", WriterPlan { pad_before_sync: true, ..OLD_FLATE2 }),
        ("parallel file sync", WriterPlan { parallel_file_sync: true, ..OLD_FLATE2 }),
        ("bounded open regions", WriterPlan { max_open_per_folder: Some(2), ..OLD_FLATE2 }),
        ("all", ALL_ON_FLATE2),
        ("all, 256 open", WriterPlan { max_open_per_folder: Some(256), ..ALL_ON_FLATE2 }),
    ];
    for (name, plan) in alone {
        let got = run_session(&name.replace([' ', ','], "-"), plan).expect("test world");
        assert!(got.files == reference.files, "{name}: files differ");
    }
    let swapped = run_session("swapped", WriterPlan { sabotage: WriterSabotage::SwapFirstTwoChunks, ..ALL_ON_FLATE2 });
    assert!(swapped.expect("test world").files != reference.files, "swapped writes not seen");
}

/// Regions written by chunk jobs between two syncs (and before the first).
fn written_between_syncs(batches: &[Vec<Job>]) -> Vec<Vec<PathBuf>> {
    let mut out = Vec::new();
    let mut written = BTreeSet::new();
    for job in batches.iter().flatten() {
        match job {
            Job::Chunk { path, .. } => {
                written.insert(path.clone());
            }
            Job::Sync => out.push(std::mem::take(&mut written).into_iter().collect()),
            _ => {}
        }
    }
    out
}

/// With clean regions skipped, each sync forces exactly the regions written since the previous
/// one, padded first or not. Forcing the clean ones too, or leaving a written one out, is seen (the
/// sabotages run without padding first: padding a written region would force it anyway).
#[test]
fn syncs_force_exactly_the_regions_written_since_the_last_one() {
    for pad_before_sync in [true, false] {
        let plan = WriterPlan { max_open_per_folder: None, pad_before_sync, ..ALL_ON_FLATE2 };
        let Some(got) = run_session(&format!("synced-{pad_before_sync}"), plan) else { return };
        let expected = written_between_syncs(&session_batches(&got.dir).expect("test world"));
        assert_eq!(expected.len(), 3);
        assert!(expected[1].len() < expected[0].len(), "the second sync must leave clean regions");
        assert!(got.synced == expected, "forced {:?}, written {:?}", got.synced, expected);
    }
    let plan = WriterPlan { max_open_per_folder: None, pad_before_sync: false, ..ALL_ON_FLATE2 };
    for sabotage in [WriterSabotage::SyncCleanRegions, WriterSabotage::ForgetOneWrittenRegion] {
        let got = run_session(&format!("synced-{sabotage:?}"), WriterPlan { sabotage, ..plan }).expect("test world");
        let expected = written_between_syncs(&session_batches(&got.dir).expect("test world"));
        assert!(got.synced != expected, "{sabotage:?} not seen");
    }
}

/// Every chunk of every region file of `dir`, decompressed, and the other files as they are.
fn decompressed_contents(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut out = Vec::new();
    for (rel, bytes) in files_under(dir) {
        let name = rel.to_string_lossy().into_owned();
        if rel.extension().is_some_and(|e| e == "mca") {
            let mut parts = rel.file_name().and_then(|n| n.to_str()).unwrap_or("").split('.').skip(1);
            let rx: i32 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            let rz: i32 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
            let mut region = RegionFile::open(&dir.join(&rel), false).expect("region");
            let present: Vec<usize> = region.present().collect();
            for i in present {
                let (cx, cz) = (rx * 32 + (i % 32) as i32, rz * 32 + (i / 32) as i32);
                let chunk = region.read(i, cx, cz).expect("readable").expect("present");
                out.push((format!("{name} {cx} {cz}"), chunk));
            }
        } else if rel.extension().is_some_and(|e| e == "mcc") {
            let chunk =
                crate::region::decompress(2, &bytes).unwrap_or_else(|e| panic!("{name}: {e} ({} bytes)", bytes.len()));
            out.push((name, chunk));
        } else {
            out.push((name, bytes));
        }
    }
    out
}

/// libdeflate streams (other compressed bytes) read back to the chunks flate2's streams hold,
/// external stream included; the other files are identical. A chunk changed afterwards is seen.
#[test]
fn libdeflate_streams_read_back_to_the_same_chunks() {
    let Some(reference) = run_session("flate2", OLD_FLATE2) else { return };
    let libdeflate = WriterPlan { compressor: ZlibCompressor::Libdeflate { level: 6 }, ..ALL_ON_FLATE2 };
    let got = run_session("libdeflate", libdeflate).expect("test world");
    assert!(got.files != reference.files, "libdeflate wrote flate2's bytes: the proof would prove nothing");
    let expected = decompressed_contents(&reference.dir);
    assert!(expected.len() > 200, "{} entries", expected.len());
    assert!(decompressed_contents(&got.dir) == expected, "chunks read back differ");
    // The comparison sees a changed chunk.
    let (rel, ..) = expected.iter().find(|(n, _)| n.contains(".mca ")).expect("a region chunk");
    let mut words = rel.split(' ');
    let (file, cx, cz) = (words.next().unwrap_or(""), words.next().unwrap_or("0"), words.next().unwrap_or("0"));
    let (cx, cz): (i32, i32) = (cx.parse().unwrap_or(0), cz.parse().unwrap_or(0));
    let mut r = RegionFile::open(&got.dir.join(file), true).expect("region");
    r.write(crate::region::local_index(cx, cz), cx, cz, b"changed").expect("write");
    r.close().expect("close");
    assert!(decompressed_contents(&got.dir) != expected, "changed chunk not seen");
}

/// A crash between the forced temporary and the rename leaves the old file whole, replaced
/// one after the other or side by side; the other files of the group are replaced.
#[test]
fn crash_before_the_rename_keeps_the_old_file() {
    for parallel in [false, true] {
        let dir =
            rc_testdata::scratch(std::env::temp_dir().join(format!("rc-save-crash-{parallel}-{}", std::process::id())));
        let dat = |name: &str, n: i64| Job::Dat {
            path: dir.join(name),
            nbt: nbtio::encode_root(&Nbt::Compound(vec![("n".into(), Nbt::Long(n))])),
            backup: name == "c.dat",
        };
        let read = |name: &str| nbtio::read_dat(&dir.join(name)).ok().and_then(|v| nbtio::int(&v, "n"));
        let mut w = WriteThread::new(WriterPlan { parallel_file_sync: parallel, ..WriterPlan::OLD });
        w.run_batch(vec![dat("a.dat", 1), dat("b.dat", 1), dat("c.dat", 1)]);
        let crashed = dir.join("b.dat");
        CRASH_BEFORE_RENAME.lock().unwrap_or_else(std::sync::PoisonError::into_inner).push(crashed.clone());
        w.run_batch(vec![dat("a.dat", 2), dat("b.dat", 2), dat("c.dat", 2)]);
        CRASH_BEFORE_RENAME.lock().unwrap_or_else(std::sync::PoisonError::into_inner).retain(|p| *p != crashed);
        w.finish();
        assert_eq!(
            [read("a.dat"), read("b.dat"), read("c.dat"), read("c.dat_old")],
            [Some(2), Some(1), Some(2), Some(1)]
        );
        let temporaries = files_under(&dir).iter().filter(|(p, _)| p.to_string_lossy().starts_with("b-")).count();
        assert_eq!(temporaries, 1, "the crashed replacement leaves its temporary");
    }
}

// ---------------------------------------------------------------- benches (measurement, not proofs)

/// Bench (loaded machine): the save's chunk encoding over the chunks within
/// `RUSTCRAFT_BENCH_RADIUS` (default 10) of the player, serial then side by side on 1 thread and
/// powers of 2 up to every core. `RUSTCRAFT_SOLO_WORLD=<world> cargo test --release -p rc-sim --
/// --ignored bench_save_encode --nocapture`.
#[test]
#[ignore = "bench"]
fn bench_save_encode() {
    let radius = rc_data::env_str!("RUSTCRAFT_BENCH_RADIUS").and_then(|v| v.parse().ok()).unwrap_or(10);
    let mut s = open("bench-encode", radius).expect("test world");
    let serial = EncodePlan { parallel: false, sabotage: EncodeSabotage::None };
    let parallel = EncodePlan { parallel: true, ..serial };
    let pools: Vec<(usize, rayon::ThreadPool)> = thread_counts().into_iter().map(|n| (n, pool(n))).collect();
    // A first save turns every read compound into its kept form: the later ones are autosaves.
    let chunks = save_all(&mut s, serial, &pools[0].1).len();
    for pass in 0..3 {
        let mut line = format!("bench_save_encode pass {pass}, {chunks} chunks, ms per save:");
        let t = Instant::now();
        save_all(&mut s, serial, &pools[0].1);
        line += &format!(" serial {:.1}", t.elapsed().as_secs_f64() * 1e3);
        for (n, p) in &pools {
            let t = Instant::now();
            save_all(&mut s, parallel, p);
            line += &format!(", {n} thr {:.1}", t.elapsed().as_secs_f64() * 1e3);
        }
        eprintln!("{line}");
    }
    close(s);
}

/// Bench (loaded machine): compression of the test world's region chunks, flate2 against
/// libdeflate at levels 1 to 6 (time per chunk and total size), then flate2 and libdeflate 6 side
/// by side on 1 thread and powers of 2 up to every core.
#[test]
#[ignore = "bench"]
fn bench_save_compression() {
    let chunks = real_chunks("region", 1024).expect("test world");
    let raw: usize = chunks.iter().map(|c| c.2.len()).sum();
    let jobs: Vec<Job> = chunks
        .iter()
        .map(|(cx, cz, nbt)| Job::Chunk { path: PathBuf::new(), cx: *cx, cz: *cz, nbt: nbt.clone() })
        .collect();
    let mut compressors: Vec<(String, ZlibCompressor)> = vec![("flate2 6".into(), ZlibCompressor::Flate2)];
    compressors.extend((1..=6).map(|level| (format!("libdeflate {level}"), ZlibCompressor::Libdeflate { level })));
    eprintln!("bench_save_compression: {} chunks, {:.1} MiB uncompressed", chunks.len(), raw as f64 / 1048576.0);
    for pass in 0..2 {
        for (name, c) in &compressors {
            let t = Instant::now();
            let packed: usize = chunks.iter().map(|(_, _, nbt)| nbtio::zlib_with(nbt, *c).len()).sum();
            let dt = t.elapsed().as_secs_f64();
            eprintln!(
                "pass {pass} {name}: 1 thread {:.1} µs/chunk, {:.0} MiB/s in, {:.2} MiB out (ratio {:.2})",
                dt * 1e6 / chunks.len() as f64,
                raw as f64 / 1048576.0 / dt,
                packed as f64 / 1048576.0,
                raw as f64 / packed as f64
            );
        }
    }
    for (name, c) in [("flate2 6", ZlibCompressor::Flate2), ("libdeflate 6", ZlibCompressor::Libdeflate { level: 6 })] {
        let mut line = format!("{name}, ms per batch of {}:", jobs.len());
        for n in thread_counts() {
            let p = pool(n);
            compress_chunks_on(&p, &jobs, c);
            let t = Instant::now();
            for _ in 0..3 {
                compress_chunks_on(&p, &jobs, c);
            }
            line += &format!(" {n} thr {:.1},", t.elapsed().as_secs_f64() * 1e3 / 3.0);
        }
        eprintln!("{line}");
    }
}

/// Bench (loaded machine, real disk): the session's batches through the write thread, old plan,
/// each switch alone and all together, three alternated passes; time and forced writes.
#[test]
#[ignore = "bench"]
fn bench_write_thread() {
    let mut plans: Vec<(&str, WriterPlan)> = vec![
        ("old", OLD_FLATE2),
        ("parallel compression", WriterPlan { parallel_compression: true, ..OLD_FLATE2 }),
        ("skip clean regions", WriterPlan { skip_clean_regions: true, ..OLD_FLATE2 }),
        ("pad before sync", WriterPlan { pad_before_sync: true, ..OLD_FLATE2 }),
        ("parallel file sync", WriterPlan { parallel_file_sync: true, ..OLD_FLATE2 }),
        ("libdeflate", WriterPlan { compressor: ZlibCompressor::Libdeflate { level: 6 }, ..OLD_FLATE2 }),
    ];
    plans.push((
        "all (256 open)",
        WriterPlan {
            max_open_per_folder: Some(256),
            compressor: ZlibCompressor::Libdeflate { level: 6 },
            ..ALL_ON_FLATE2
        },
    ));
    for pass in 0..3 {
        for (name, plan) in &plans {
            let dir =
                rc_testdata::scratch(std::env::temp_dir().join(format!("rc-save-bench-{pass}-{}", std::process::id())));
            let batches = session_batches(&dir).expect("test world");
            let chunks = batches.iter().flatten().filter(|j| matches!(j, Job::Chunk { .. })).count();
            let syncs = crate::region::file_syncs();
            let t = Instant::now();
            let mut w = WriteThread::new(*plan);
            for batch in batches {
                w.run_batch(batch);
            }
            w.finish();
            eprintln!(
                "bench_write_thread pass {pass} {name}: {chunks} chunks, {:.1} ms, {} forced writes",
                t.elapsed().as_secs_f64() * 1e3,
                crate::region::file_syncs() - syncs
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }
}
