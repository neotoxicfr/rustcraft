//! Proofs of the region read and write paths: every plan gives the old path's result, and the
//! deliberate faults are seen.

use super::*;
use rc_proto::nbt::Nbt;
use std::cell::Cell;

thread_local! {
    /// Clock of the writes made by this thread (`None`: the real one).
    pub(crate) static FIXED_NOW: Cell<Option<u32>> = const { Cell::new(None) };
}

fn lcg(s: &mut u64) -> u64 {
    *s = s.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    *s >> 33
}

/// Every combination of the three read switches.
pub(crate) fn read_plans() -> Vec<ReadPlan> {
    (0..8u8)
        .map(|b| ReadPlan {
            positioned: b & 1 != 0,
            inflate_unlocked: b & 2 != 0,
            libdeflate: b & 4 != 0,
            sabotage: ReadSabotage::None,
        })
        .collect()
}

/// Every combination of the three write switches.
fn write_plans() -> Vec<WritePlan> {
    (0..8u8)
        .map(|b| WritePlan { sector_bitmap: b & 1 != 0, header_words: b & 2 != 0, external_list: b & 4 != 0 })
        .collect()
}

const ALL_READS_ON: ReadPlan =
    ReadPlan { positioned: true, inflate_unlocked: true, libdeflate: true, sabotage: ReadSabotage::None };
const ALL_WRITES_ON: WritePlan = WritePlan { sector_bitmap: true, header_words: true, external_list: true };

fn scratch(tag: &str) -> PathBuf {
    let dir = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-region-{tag}-{}", std::process::id())));
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Uncompressed NBT of a test chunk: a compound with `longs` values (compressible).
fn sample_nbt(seed: u64, longs: usize) -> Vec<u8> {
    let mut s = seed;
    let data: Vec<i64> = (0..longs).map(|_| (lcg(&mut s) % 1000) as i64).collect();
    crate::nbtio::encode_root(&Nbt::Compound(vec![
        ("seed".into(), Nbt::Long(seed as i64)),
        ("data".into(), Nbt::LongArray(data)),
    ]))
}

fn random_bytes(seed: u64, n: usize) -> Vec<u8> {
    let mut s = seed;
    (0..n).map(|_| lcg(&mut s) as u8).collect()
}

fn framed(length: u32, kind: u8, body: &[u8]) -> Vec<u8> {
    let mut v = length.to_be_bytes().to_vec();
    v.push(kind);
    v.extend_from_slice(body);
    v
}

/// Builds a region file from `(slot index, bytes at its sectors, sector count)`; the bytes of
/// the last entry end the file (no padding). Raw slot words are added from `raw_slots`.
fn build_region(path: &Path, entries: &[(usize, Vec<u8>, u32)], raw_slots: &[(usize, u32)]) {
    let mut header = vec![0u8; HEADER];
    let mut body = Vec::new();
    let mut sector = 2u32;
    for (i, bytes, count) in entries {
        header[i * 4..i * 4 + 4].copy_from_slice(&((sector << 8) | count).to_be_bytes());
        header[4096 + i * 4..4096 + i * 4 + 4].copy_from_slice(&(1_000 + *i as u32).to_be_bytes());
        // Each entry at its first sector (bytes past its sectors cut by the next entry).
        body.resize((sector as usize - 2) * SECTOR as usize, 0);
        body.extend_from_slice(bytes);
        sector += count;
    }
    for &(i, word) in raw_slots {
        header[i * 4..i * 4 + 4].copy_from_slice(&word.to_be_bytes());
    }
    header.extend_from_slice(&body);
    std::fs::write(path, header).unwrap();
}

fn sectors_for(bytes: &[u8]) -> u32 {
    bytes.len().div_ceil(SECTOR as usize).max(1) as u32
}

/// Names of the damaged cases of region (0, 0), by slot.
pub(crate) const DAMAGED_CASES: [&str; 21] = [
    "zlib",
    "gzip",
    "uncompressed",
    "unknown compression",
    "length zero",
    "length past its sectors",
    "truncated zlib",
    "wrong checksum",
    "trailing bytes",
    "external stream",
    "external stream missing",
    "preset dictionary",
    "garbage",
    "empty compound",
    "truncated gzip",
    "external flag with inline bytes",
    "slot in the header",
    "slot without sectors",
    "slot past the end",
    "zlib cut by the end of the file",
    "head cut by the end of the file (region 1, 0)",
];

/// Region files `r.0.0.mca` and `r.1.0.mca` in `dir` holding every kind of stream and damage a
/// reader must handle (cases by slot: [`DAMAGED_CASES`]); external streams next to them. Slot 20
/// is local index 0 of region (1, 0), alone, its file ending three bytes into its sector.
pub(crate) fn write_damaged_regions(dir: &Path) -> Vec<(i32, i32)> {
    let nbt = sample_nbt(7, 3000);
    let zlib = crate::nbtio::zlib(&nbt);
    let gzip = crate::nbtio::gzip(&nbt);
    let mut wrong_adler = zlib.clone();
    *wrong_adler.last_mut().unwrap() ^= 0x55;
    let mut trailing = zlib.clone();
    trailing.extend_from_slice(b"trailing bytes after the stream");
    // Dictionary flag set, check bits fixed so that only the flag is wrong.
    let mut fdict = zlib.clone();
    fdict[1] |= 0x20;
    fdict[1] &= !0x1f;
    let check = (u16::from(fdict[0]) * 256 + u16::from(fdict[1])) % 31;
    fdict[1] += ((31 - check) % 31) as u8;
    let empty = crate::nbtio::zlib(&crate::nbtio::encode_root(&Nbt::Compound(Vec::new())));
    let entry = |i: usize, kind: u8, body: &[u8]| {
        let f = framed(body.len() as u32 + 1, kind, body);
        let n = sectors_for(&f);
        (i, f, n)
    };
    let whole = sectors_for(&framed(zlib.len() as u32 + 1, 2, &zlib));
    let past = framed(whole * SECTOR as u32 + 100, 2, &zlib);
    let cut = framed(zlib.len() as u32 + 1, 2, &zlib[..zlib.len() / 3]);
    let entries = vec![
        entry(0, 2, &zlib),
        entry(1, 1, &gzip),
        entry(2, 3, &nbt),
        entry(3, 7, b"123456789"),
        (4, framed(0, 2, &[]), 1),
        (5, past, whole),
        entry(6, 2, &zlib[..zlib.len() / 2]),
        entry(7, 2, &wrong_adler),
        entry(8, 2, &trailing),
        (9, framed(1, 0x82, &[]), 1),
        (10, framed(1, 0x82, &[]), 1),
        entry(11, 2, &fdict),
        entry(12, 2, &random_bytes(3, 5000)),
        entry(13, 2, &empty),
        entry(14, 1, &gzip[..gzip.len() / 2]),
        entry(15, 0x82, &zlib),
        (19, cut, whole),
    ];
    let end_sector: u32 = 2 + entries.iter().map(|e| e.2).sum::<u32>();
    let raw = [(16, (1 << 8) | 1), (17, 5 << 8), (18, ((end_sector + 50) << 8) | 1)];
    build_region(&dir.join("r.0.0.mca"), &entries, &raw);
    std::fs::write(dir.join("c.9.0.mcc"), &zlib).unwrap();
    std::fs::write(dir.join("c.15.0.mcc"), &zlib).unwrap();
    build_region(&dir.join("r.1.0.mca"), &[(0, framed(40, 2, &zlib[..0]).into_iter().take(3).collect(), 1)], &[]);
    let mut chunks: Vec<(i32, i32)> = (0..20).map(|i| (i, 0)).collect();
    chunks.push((32, 0));
    chunks
}

type ReadResult = Result<Option<Vec<u8>>, Unreadable>;

fn read_all_old_way(dir: &Path, chunks: &[(i32, i32)], plan: ReadPlan) -> Vec<ReadResult> {
    chunks
        .iter()
        .map(|&(x, z)| {
            let mut r = RegionFile::open(&dir.join(region_name(x, z)), false).unwrap();
            r.read_plan = plan;
            r.read(local_index(x, z), x, z)
        })
        .collect()
}

fn read_all_shared(dir: &Path, chunks: &[(i32, i32)], plan: ReadPlan) -> Vec<ReadResult> {
    chunks
        .iter()
        .map(|&(x, z)| {
            let r = SharedRegion::open(&dir.join(region_name(x, z))).unwrap();
            r.read_then(local_index(x, z), x, z, plan, <[u8]>::to_vec)
        })
        .collect()
}

/// Every damaged slot gives the same result (bytes, absence, error and its text) through every
/// read plan, through a file of its own or a shared one.
#[test]
fn damaged_streams_read_the_same_for_every_plan() {
    let dir = scratch("damaged");
    let chunks = write_damaged_regions(&dir);
    let reference = read_all_old_way(&dir, &chunks, ReadPlan::OLD);
    let nbt = sample_nbt(7, 3000);
    // What the cases are, so that a wrong corpus cannot pass unnoticed.
    assert_eq!(reference[0], Ok(Some(nbt.clone())));
    assert_eq!(reference[1], Ok(Some(nbt.clone())));
    assert_eq!(reference[2], Ok(Some(nbt.clone())));
    assert_eq!(reference[3], Err(Unreadable::Compression(7)));
    assert_eq!(reference[4], Ok(None));
    assert_eq!(reference[5], Err(Unreadable::Truncated));
    assert!(matches!(reference[7], Err(Unreadable::Inflate(_))), "{:?}", reference[7]);
    assert_eq!(reference[8], Ok(Some(nbt.clone())));
    assert_eq!(reference[9], Ok(Some(nbt.clone())));
    assert!(matches!(reference[10], Err(Unreadable::Io(_))));
    assert_eq!(reference[15], Ok(Some(nbt)));
    assert_eq!(&reference[16..19], &[Ok(None), Ok(None), Ok(None)]);
    assert!(matches!(reference[20], Err(Unreadable::Io(_))), "{:?}", reference[20]);
    for plan in read_plans() {
        for (way, got) in
            [("own file", read_all_old_way(&dir, &chunks, plan)), ("shared file", read_all_shared(&dir, &chunks, plan))]
        {
            for (i, (a, b)) in reference.iter().zip(&got).enumerate() {
                assert_eq!(a, b, "{way}, {plan:?}, case {i} ({})", DAMAGED_CASES[i]);
            }
        }
    }
}

/// Compressed streams of every chunk of a world folder (all region folders).
fn world_streams(world: &Path) -> Vec<(u8, Vec<u8>)> {
    fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = e.path();
            if p.is_dir() {
                walk(&p, out);
            } else if p.extension().is_some_and(|x| x == "mca") {
                out.push(p);
            }
        }
    }
    let mut files = Vec::new();
    walk(world, &mut files);
    files.sort();
    let mut out = Vec::new();
    for f in files {
        let Ok(r) = RegionFile::open(&f, false) else { continue };
        for i in r.present() {
            let mut known = Known { slot: r.locations[i], len: r.len };
            let at = ChunkAt { index: i, cx: 0, cz: 0 };
            if let Ok(Some(c)) = fetch(&r.file, parent_dir(&r.path), at, &mut known, ReadPlan::OLD) {
                out.push((c.kind, c.bytes[c.range].to_vec()));
            }
        }
    }
    out
}

/// Damaged copies of a stream: cut at many lengths, one bit flipped at many places, bytes added,
/// checksum changed.
fn damaged_copies(kind: u8, s: &[u8], seed: u64) -> Vec<(u8, Vec<u8>)> {
    let mut out = Vec::new();
    let step = (s.len() / 40).max(1);
    for cut in (0..s.len()).step_by(step).chain([s.len().saturating_sub(1), s.len().saturating_sub(4)]) {
        out.push((kind, s[..cut].to_vec()));
    }
    let mut r = seed;
    for _ in 0..40 {
        let mut f = s.to_vec();
        if !f.is_empty() {
            let at = lcg(&mut r) as usize % f.len();
            f[at] ^= 1 << (lcg(&mut r) % 8);
        }
        out.push((kind, f));
    }
    let mut longer = s.to_vec();
    longer.extend_from_slice(&random_bytes(seed, 100));
    out.push((kind, longer));
    let mut adler = s.to_vec();
    if let Some(b) = adler.last_mut() {
        *b = b.wrapping_add(1);
    }
    out.push((kind, adler));
    out
}

/// libdeflate with its fall back = flate2, on every stream of the test world and on damaged
/// copies (cut, flipped bits, bytes added, wrong checksum, gzip): same bytes, same error and
/// text. Without the fall back (libdeflate's own refusal), the damaged streams differ: the
/// comparison sees a missing fall back.
#[test]
fn inflate_matches_the_old_decoder() {
    let Some(world) = crate::tests::test_world() else { return };
    let streams = world_streams(&world);
    assert!(streams.len() > 1000, "too few streams read: {}", streams.len());
    let mut corpus = streams.clone();
    for (n, (kind, s)) in streams.iter().enumerate().step_by(97) {
        corpus.extend(damaged_copies(*kind, s, n as u64));
        let raw = decompress(*kind, s).unwrap();
        corpus.extend(damaged_copies(1, &crate::nbtio::gzip(&raw), n as u64));
    }
    let (mut differ_without_fall_back, mut damaged_ok) = (0usize, 0usize);
    for (kind, s) in &corpus {
        let old = decompress(*kind, s);
        let new = inflate_then(*kind, s, true, |b| b.into_owned());
        assert_eq!(old, new, "kind {kind}, {} bytes", s.len());
        if *kind == 2 {
            let mut out = Vec::new();
            let alone = zlib_into(s, &mut out)
                .map(|n| out[..n].to_vec())
                .ok_or_else(|| Unreadable::Inflate("libdeflate: bad data".into()));
            differ_without_fall_back += usize::from(alone != old);
        }
        damaged_ok += usize::from(old.is_ok());
    }
    eprintln!(
        "{} streams ({} from the world), {damaged_ok} decoded, {differ_without_fall_back} need the fall back",
        corpus.len(),
        streams.len()
    );
    assert!(differ_without_fall_back > 0, "the corpus never needs the fall back: it proves nothing about it");
}

/// A stream inflating to far more than the first output size tried (2 MiB of nearly constant
/// bytes from a 2 KiB stream): the buffer doubles until it fits, same bytes as flate2; then a
/// small stream after it on the same thread (buffer kept).
#[test]
fn libdeflate_grows_its_buffer() {
    let mut raw = vec![7u8; 2 << 20];
    raw[1000] = 1;
    let big = crate::nbtio::zlib(&raw);
    let first_guess = (big.len() * 8).max(FIRST_GUESS_MIN);
    assert!(first_guess * 4 < raw.len(), "the stream must need several doublings");
    for s in [big, crate::nbtio::zlib(b"small")] {
        assert_eq!(inflate_then(2, &s, true, |b| b.into_owned()), decompress(2, &s));
    }
    let kept = INFLATED.take();
    assert!(kept.len() >= raw.len(), "buffer kept at the largest size seen: {}", kept.len());
}

/// Payload of chunk `i` written by the race test: its index, length and seed first, so that a
/// read of another chunk's sectors is seen.
fn race_payload(i: usize, round: usize, seed: &mut u64) -> Vec<u8> {
    let n = 2000 + (i * 37 + round * 101) % 9000;
    let mut v = Vec::with_capacity(n);
    v.extend_from_slice(&(i as u32).to_le_bytes());
    v.extend_from_slice(&(n as u32).to_le_bytes());
    v.extend((8..n).map(|_| lcg(seed) as u8));
    v
}

fn race_payload_fits(i: usize, b: &[u8]) -> bool {
    b.len() >= 8 && b[..4] == (i as u32).to_le_bytes() && b[4..8] == (b.len() as u32).to_le_bytes()
}

/// How the readers of the race test read.
#[derive(Clone, Copy, Debug)]
enum Readers {
    /// One reader opening its own file each round (the old way).
    OwnFile(ReadPlan),
    /// `n` readers sharing one file opened each round.
    Shared(ReadPlan, usize),
}

/// Readers against the write thread rewriting `chunks` chunks `rounds` times at least (moved,
/// sectors reused; with few chunks, each write takes the sectors the previous one freed), and
/// then until the reader has opened the file during the writes and, with `until_bad`, read a
/// wrong chunk: a loaded machine may not schedule the reader during a fixed number of rounds.
/// (invalid slots at opening, wrong reads, openings).
fn race(readers: Readers, chunks: usize, rounds: usize, until_bad: bool) -> (usize, usize, usize) {
    use std::sync::atomic::AtomicUsize;
    let dir = scratch(&format!("race-{chunks}-{}", tag_of(readers)));
    let path = dir.join("r.0.0.mca");
    let _ = std::fs::remove_file(&path);
    let mut w = RegionFile::open_or_create(&path).unwrap();
    let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    // Openings and wrong reads so far, seen by the writer.
    let seen = Arc::new((AtomicUsize::new(0), AtomicUsize::new(0)));
    let (p2, s2, seen2) = (path.clone(), stop.clone(), seen.clone());
    let reader = std::thread::spawn(move || {
        let (mut dropped, mut bad, mut opens) = (0usize, 0usize, 0usize);
        while !s2.load(Ordering::Relaxed) {
            match readers {
                Readers::OwnFile(plan) => {
                    let Ok(mut r) = RegionFile::open(&p2, false) else { continue };
                    r.read_plan = plan;
                    opens += 1;
                    dropped += r.dropped;
                    let present: Vec<usize> = r.present().collect();
                    for i in present {
                        let res = r.read(i, (i % 32) as i32, (i / 32) as i32);
                        bad += usize::from(!matches!(&res, Ok(Some(b)) if race_payload_fits(i, b)));
                    }
                }
                Readers::Shared(plan, n) => {
                    let Ok(r) = SharedRegion::open(&p2) else { continue };
                    opens += 1;
                    dropped += r.region.dropped;
                    let present: Vec<usize> = r.present().collect();
                    let r = &r;
                    bad += std::thread::scope(|s| {
                        let parts: Vec<_> = present
                            .chunks(present.len().div_ceil(n).max(1))
                            .map(|part| {
                                s.spawn(move || {
                                    part.iter()
                                        .filter(|&&i| {
                                            let res = r.read_then(i, (i % 32) as i32, (i / 32) as i32, plan, |b| {
                                                race_payload_fits(i, b)
                                            });
                                            res != Ok(Some(true))
                                        })
                                        .count()
                                })
                            })
                            .collect();
                        parts.into_iter().map(|p| p.join().unwrap()).sum::<usize>()
                    });
                }
            }
            seen2.0.store(opens, Ordering::Relaxed);
            seen2.1.store(bad, Ordering::Relaxed);
        }
        (dropped, bad, opens)
    });
    // Poorly compressible content: several sectors per chunk, rewrites moved.
    let mut seed = 1u64;
    let start = std::time::Instant::now();
    let mut round = 0;
    while round < rounds
        || seen.0.load(Ordering::Relaxed) == 0
        || (until_bad && seen.1.load(Ordering::Relaxed) == 0)
    {
        assert!(
            start.elapsed() < std::time::Duration::from_secs(120),
            "anti-hang guard: {readers:?} after {round} rounds, {} openings, {} wrong reads",
            seen.0.load(Ordering::Relaxed),
            seen.1.load(Ordering::Relaxed)
        );
        for i in 0..chunks {
            w.write(i, (i % 32) as i32, (i / 32) as i32, &race_payload(i, round, &mut seed)).unwrap();
        }
        round += 1;
    }
    stop.store(true, Ordering::Relaxed);
    let out = reader.join().unwrap();
    w.close().unwrap();
    eprintln!("{readers:?}: {} openings, {} invalid slots, {} wrong reads", out.2, out.0, out.1);
    out
}

/// Folder name of a race variant.
fn tag_of(r: Readers) -> String {
    format!("{r:?}").chars().filter(char::is_ascii_alphanumeric).take(60).collect()
}

/// Readers opening the region while the writer rewrites chunks (generation thread against write
/// thread): never an invalid slot, and each announced chunk read back whole, with the old read,
/// the new one, and several readers sharing one file. Reading without the lock, or with the
/// slot of the opening instead of the one on disk, is seen.
#[test]
fn concurrent_reader_never_sees_invalid_locations() {
    for readers in [Readers::OwnFile(ReadPlan::OLD), Readers::OwnFile(ALL_READS_ON), Readers::Shared(ALL_READS_ON, 4)] {
        for (chunks, rounds) in [(1024, 6), (16, 400)] {
            let (dropped, bad, opens) = race(readers, chunks, rounds, false);
            assert!(opens > 0);
            assert_eq!((dropped, bad), (0, 0), "{readers:?}, {chunks} chunks");
        }
    }
    for sabotage in [ReadSabotage::NoLock, ReadSabotage::StaleSlot] {
        let (_, bad, _) = race(Readers::Shared(ReadPlan { sabotage, ..ALL_READS_ON }, 4), 16, 400, true);
        assert!(bad > 0, "sabotage {sabotage:?} not seen");
    }
}

/// Damaged starting file for the write proofs: two slots sharing a sector, one invalid slot,
/// one external stream.
fn write_damaged_start(dir: &Path) {
    let a = framed(4000 + 1, 2, &random_bytes(11, 4000));
    let b = framed(6000 + 1, 2, &random_bytes(12, 6000));
    // Slot 1 starts in the middle of slot 0 (shares its second sector); slot 500 (never written
    // by the script) points past the end.
    build_region(&dir.join("r.0.0.mca"), &[(0, a, 2), (3, b, 2)], &[(1, (3 << 8) | 2), (500, (400 << 8) | 1)]);
    std::fs::write(dir.join("c.7.0.mcc"), crate::nbtio::zlib(&sample_nbt(5, 10))).unwrap();
}

/// One step of the write script.
#[derive(Clone)]
enum Step {
    Write(usize, Vec<u8>),
    Erase(usize),
}

/// Writes of every size (several sectors, more than 255: external), rewrites smaller and
/// larger, erasures, a chunk going external then back. Second value: where the script is cut for
/// the sabotages (two external streams exist there, to be removed after).
fn write_script() -> (Vec<Step>, usize) {
    let mut steps = Vec::new();
    let big = random_bytes(99, 1_200_000);
    for i in 0..120usize {
        steps.push(Step::Write(i, sample_nbt(i as u64, 50 + (i * 731) % 6000)));
    }
    steps.push(Step::Write(7, big.clone()));
    steps.push(Step::Write(8, big.clone()));
    let middle = steps.len();
    for i in (0..120usize).step_by(3) {
        steps.push(Step::Write(i, random_bytes(i as u64, 200 + (i * 977) % 12_000)));
    }
    // Chunk 7 back inside the region: its external stream removed (and never erased after).
    steps.push(Step::Write(7, sample_nbt(1, 10)));
    for i in (1..120usize).step_by(7) {
        steps.push(Step::Erase(i));
    }
    steps.push(Step::Erase(8));
    steps.push(Step::Write(8, sample_nbt(2, 10)));
    steps.push(Step::Write(9, big));
    for i in (0..120usize).step_by(5) {
        steps.push(Step::Write(i, sample_nbt(i as u64 + 1000, 30 + (i * 313) % 9000)));
    }
    (steps, middle)
}

fn run_script(r: &mut RegionFile, steps: &[Step]) {
    for s in steps {
        match s {
            Step::Write(i, nbt) => r.write(*i, (*i % 32) as i32, (*i / 32) as i32, nbt).unwrap(),
            Step::Erase(i) => r.erase(*i, (*i % 32) as i32, (*i / 32) as i32).unwrap(),
        }
    }
}

/// File names and contents of a folder, sorted.
fn folder(dir: &Path) -> Vec<(String, Vec<u8>)> {
    let mut v: Vec<(String, Vec<u8>)> = std::fs::read_dir(dir)
        .unwrap()
        .flatten()
        .map(|e| (e.file_name().into_string().unwrap(), std::fs::read(e.path()).unwrap()))
        .collect();
    v.sort();
    v
}

type Folder = Vec<(String, Vec<u8>)>;

/// When a write sabotage acts.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Moment {
    Opened,
    Middle,
}

/// What a write sabotage does to the open file at a given moment.
type WriteSabotage = dyn Fn(&mut RegionFile, Moment);

/// Runs the write script from the damaged start with `plan`; `sabotage` acts on the open file
/// right after opening and in the middle of the script. Folder contents after the script and
/// after closing.
fn written(tag: &str, plan: WritePlan, sabotage: &WriteSabotage) -> (Folder, Folder) {
    FIXED_NOW.with(|c| c.set(Some(1_700_000_000)));
    let dir = scratch(tag);
    write_damaged_start(&dir);
    let mut r = RegionFile::open(&dir.join("r.0.0.mca"), true).unwrap();
    r.write_plan = plan;
    let (steps, middle) = write_script();
    sabotage(&mut r, Moment::Opened);
    run_script(&mut r, &steps[..middle]);
    sabotage(&mut r, Moment::Middle);
    run_script(&mut r, &steps[middle..]);
    let open = folder(&dir);
    r.close().unwrap();
    FIXED_NOW.with(|c| c.set(None));
    (open, folder(&dir))
}

/// Every write plan leaves the same files byte for byte (region, external streams), before and
/// after closing, from a damaged start (shared sectors, invalid slot). Each switch's sabotage is
/// seen: a live chunk's sectors released, the header not rewritten whole after an invalid slot,
/// the list of external streams lost.
#[test]
fn writes_same_bytes_for_every_plan() {
    let none = |_: &mut RegionFile, _: Moment| {};
    let reference = written("w-old", WritePlan::OLD, &none);
    assert!(reference.0.iter().any(|(n, _)| n == "c.9.0.mcc"), "the script must leave an external stream");
    assert!(!reference.0.iter().any(|(n, _)| n == "c.7.0.mcc"), "the script must remove an external stream");
    for (n, plan) in write_plans().into_iter().enumerate() {
        let got = written(&format!("w-{n}"), plan, &none);
        assert!(got == reference, "{plan:?}: files differ");
    }
    let release_live = |r: &mut RegionFile, at: Moment| {
        if at == Moment::Middle {
            let live = r.locations[1];
            r.sectors.as_mut().expect("occupancy built").release(live);
        }
    };
    let keep_stale_header = |r: &mut RegionFile, at: Moment| {
        if at == Moment::Opened {
            r.header_stale_on_disk = false;
        }
    };
    let lose_external_list = |r: &mut RegionFile, at: Moment| {
        if at == Moment::Middle {
            r.external = Some(HashSet::new());
        }
    };
    let sabotages: [(&str, &WriteSabotage); 3] =
        [("occupancy", &release_live), ("header", &keep_stale_header), ("external list", &lose_external_list)];
    for (name, sabotage) in sabotages {
        let got = written(&format!("w-sabotage-{}", name.replace(' ', "-")), ALL_WRITES_ON, sabotage);
        assert!(got != reference, "sabotage of the {name} not seen");
    }
}

/// The header written whole at the first write only for the old damaged start; the header
/// words after that. Bytes written per chunk: 8 KiB of header before, 8 after.
#[test]
fn header_words_rewrite_the_damaged_header_once() {
    let dir = scratch("words");
    write_damaged_start(&dir);
    let mut r = RegionFile::open(&dir.join("r.0.0.mca"), true).unwrap();
    r.write_plan = ALL_WRITES_ON;
    assert!(r.header_stale_on_disk, "invalid slot cleared at opening");
    r.write(40, 8, 1, &sample_nbt(1, 10)).unwrap();
    assert!(!r.header_stale_on_disk);
    let on_disk = std::fs::read(dir.join("r.0.0.mca")).unwrap();
    assert_eq!(&on_disk[2000..2004], &[0; 4], "invalid slot 500 cleared on disk by the whole header");
}

/// Crash between the sectors and the header (header never written): the old version read back
/// intact, for every write plan. Releasing the old sectors before allocating is seen.
#[test]
fn crash_before_header_keeps_the_old_version() {
    let case = |plan: WritePlan, free_old_first: bool| -> bool {
        FIXED_NOW.with(|c| c.set(Some(1_700_000_000)));
        let dir = scratch(&format!(
            "crash-{}{}{}{}",
            plan.sector_bitmap as u8, plan.header_words as u8, plan.external_list as u8, free_old_first as u8
        ));
        let path = dir.join("r.0.0.mca");
        let mut r = RegionFile::open_or_create(&path).unwrap();
        r.write_plan = plan;
        let v1 = random_bytes(1, 9000);
        r.write(5, 5, 0, &v1).unwrap();
        for i in 6..12 {
            r.write(i, i as i32, 0, &random_bytes(i as u64, 3000)).unwrap();
        }
        let header = std::fs::read(&path).unwrap()[..HEADER].to_vec();
        if free_old_first {
            let old = r.locations[5];
            if let Some(s) = r.sectors.as_mut() {
                s.release(old);
            }
        }
        r.write(5, 5, 0, &random_bytes(2, 5000)).unwrap();
        drop(r);
        // The header as it was: the second version never announced.
        let mut bytes = std::fs::read(&path).unwrap();
        bytes[..HEADER].copy_from_slice(&header);
        std::fs::write(&path, bytes).unwrap();
        FIXED_NOW.with(|c| c.set(None));
        let mut back = RegionFile::open(&path, false).unwrap();
        back.read_plan = ReadPlan::OLD;
        back.read(5, 5, 0) == Ok(Some(v1))
    };
    for plan in write_plans() {
        assert!(case(plan, false), "{plan:?}: old version lost");
    }
    assert!(!case(ALL_WRITES_ON, true), "old sectors released first: not seen");
}

/// `write_compressed` of a stream = `write` of its NBT, byte for byte.
#[test]
fn write_compressed_matches_write() {
    FIXED_NOW.with(|c| c.set(Some(1_700_000_000)));
    let (a, b) = (scratch("wc-a"), scratch("wc-b"));
    let mut ra = RegionFile::open_or_create(&a.join("r.0.0.mca")).unwrap();
    let mut rb = RegionFile::open_or_create(&b.join("r.0.0.mca")).unwrap();
    for i in 0..50usize {
        let nbt = sample_nbt(i as u64, 100 + i * 50);
        ra.write(i, i as i32 % 32, 0, &nbt).unwrap();
        rb.write_compressed(i, i as i32 % 32, 0, &crate::nbtio::zlib(&nbt)).unwrap();
    }
    ra.close().unwrap();
    rb.close().unwrap();
    FIXED_NOW.with(|c| c.set(None));
    assert!(folder(&a) == folder(&b));
}

#[test]
fn names_and_indices() {
    assert_eq!(region_name(-1, 31), "r.-1.0.mca");
    assert_eq!(region_name(32, -33), "r.1.-2.mca");
    assert_eq!(local_index(-1, -1), 1023);
    assert_eq!(local_index(33, 2), 65);
}

/// The external streams of a region found by one listing (other regions, other names ignored).
#[test]
fn external_streams_listed_by_region() {
    let dir = scratch("mcc-list");
    for name in ["c.3.4.mcc", "c.-1.0.mcc", "c.40.2.mcc", "c.3.4.mcc.tmp", "c.03.5.mcc", "r.0.0.mca"] {
        std::fs::write(dir.join(name), b"x").unwrap();
    }
    let mut got: Vec<usize> = list_external(&dir, 0, 0).into_iter().collect();
    got.sort_unstable();
    assert_eq!(got, vec![local_index(3, 4)]);
    assert_eq!(list_external(&dir, -1, 0).into_iter().collect::<Vec<_>>(), vec![local_index(-1, 0)]);
}

/// Inflated length of a zlib stream (benches), through libdeflate or flate2.
pub(crate) fn inflate_len(stream: &[u8], libdeflate: bool) -> usize {
    inflate_then(2, stream, libdeflate, |b| b.len()).unwrap_or(0)
}

/// Micro-bench (measurement, not a proof): inflate of every stream of the bench world
/// (`RUSTCRAFT_OPT_WORLD`, otherwise the test world), flate2 against libdeflate, one thread.
#[test]
#[ignore = "measurement bench"]
fn bench_region_inflate() {
    let world = rc_data::env_os!("RUSTCRAFT_OPT_WORLD").map(PathBuf::from).or_else(crate::tests::test_world);
    let Some(world) = world else { return };
    let streams = world_streams(&world);
    let (packed, raw): (usize, usize) =
        streams.iter().fold((0, 0), |(p, r), (k, s)| (p + s.len(), r + decompress(*k, s).map_or(0, |v| v.len())));
    for pass in 0..2 {
        for libdeflate in [false, true] {
            let t = std::time::Instant::now();
            let mut n = 0usize;
            for (k, s) in &streams {
                n += inflate_then(*k, s, libdeflate, |b| b.len()).unwrap_or(0);
            }
            let dt = t.elapsed().as_secs_f64();
            eprintln!(
                "bench_inflate pass {pass} {}: {} streams, {:.1} MiB -> {:.1} MiB, {:.1} ms, {:.1} µs/stream, {:.0} MiB/s out",
                if libdeflate { "libdeflate" } else { "flate2" },
                streams.len(),
                packed as f64 / 1048576.0,
                n as f64 / 1048576.0,
                dt * 1e3,
                dt * 1e6 / streams.len() as f64,
                raw as f64 / 1048576.0 / dt
            );
        }
    }
}

/// Micro-bench (measurement, not a proof): the write script repeated, per write plan, one
/// thread; time per chunk written and bytes of header written.
#[test]
#[ignore = "measurement bench"]
fn bench_region_write() {
    let mut payloads: Vec<(usize, Vec<u8>)> = Vec::new();
    for round in 0..4usize {
        for i in 0..1024usize {
            payloads.push((
                i,
                crate::nbtio::zlib(&sample_nbt((i + round * 7919) as u64, 400 + (i * 37 + round * 101) % 6000)),
            ));
        }
    }
    let plans = [
        ("old", WritePlan::OLD),
        ("bitmap", WritePlan { sector_bitmap: true, ..WritePlan::OLD }),
        ("header words", WritePlan { header_words: true, ..WritePlan::OLD }),
        ("external list", WritePlan { external_list: true, ..WritePlan::OLD }),
        ("all but header words", WritePlan { header_words: false, ..ALL_WRITES_ON }),
        ("all", ALL_WRITES_ON),
    ];
    for pass in 0..2 {
        for (name, plan) in plans {
            let dir = scratch(&format!("bench-write-{pass}-{}", name.replace(' ', "-")));
            let mut r = RegionFile::open_or_create(&dir.join("r.0.0.mca")).unwrap();
            r.write_plan = plan;
            let t = std::time::Instant::now();
            for (i, s) in &payloads {
                r.write_compressed(*i, (*i % 32) as i32, (*i / 32) as i32, s).unwrap();
            }
            let dt = t.elapsed().as_secs_f64();
            r.close().unwrap();
            let header = if plan.header_words { 8 } else { HEADER };
            eprintln!(
                "bench_write pass {pass} {name}: {} chunks, {:.1} ms, {:.1} µs/chunk, header {} B/chunk",
                payloads.len(),
                dt * 1e3,
                dt * 1e6 / payloads.len() as f64,
                header
            );
        }
    }
}
