//! `.mca` region files (`anvil-world.md` § 3): header of 1,024 slots and
//! timestamps, chunks compressed in 4,096-byte sectors, external `.mcc` streams.
//!
//! Reading ([`ReadPlan`]): under the region lock, the slot word is read again (the write thread
//! may have moved the chunk) and then the chunk's compressed bytes; the stream is inflated once the
//! lock is released. [`SharedRegion`] is one open file shared by every task of a load.
//! Writing ([`WritePlan`]): sectors first, then the header, then the old sectors freed, like the
//! game.

use std::borrow::Cow;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use hashbrown::{HashMap, HashSet};

pub const SECTOR: u64 = 4096;
pub const HEADER: usize = 8192;

/// Why a chunk present in the header cannot be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unreadable {
    Io(String),
    /// Length announced beyond the reserved sectors.
    Truncated,
    /// Unknown or unsupported compression (LZ4, custom).
    Compression(u8),
    Inflate(String),
}

impl std::fmt::Display for Unreadable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Unreadable::Io(e) => write!(f, "read: {e}"),
            Unreadable::Truncated => f.write_str("truncated"),
            Unreadable::Compression(t) => write!(f, "compression {t} not supported"),
            Unreadable::Inflate(e) => write!(f, "decompression: {e}"),
        }
    }
}

/// How chunks are read (`RUSTCRAFT_OPT_REGION_*`, each read once; tests build their own).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReadPlan {
    /// Slot word and sectors read at their offset, the sectors in one call bounded by the end of
    /// the file, the file length asked again only when a slot points past the known end
    /// (`RUSTCRAFT_OPT_REGION_POSITIONED_READS`). Readers then share the region lock.
    pub positioned: bool,
    /// Stream inflated after the region lock is released (`RUSTCRAFT_OPT_REGION_INFLATE_UNLOCKED`):
    /// tasks of one region inflate side by side, the write thread no longer waits behind them.
    pub inflate_unlocked: bool,
    /// zlib streams inflated by libdeflate, state and output buffer kept per thread
    /// (`RUSTCRAFT_OPT_REGION_LIBDEFLATE_READ`); any failure goes back to the old decoder.
    pub libdeflate: bool,
    /// Tests: what the proofs break on purpose.
    #[cfg(test)]
    pub sabotage: ReadSabotage,
}

/// Deliberate faults of the read path, to check that the proofs see them.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ReadSabotage {
    None,
    /// Slot word and sectors read without the region lock, the reader paused between the two (a
    /// thread put aside by the system): copy on write alone leaves only that window.
    NoLock,
    /// Slot taken from the header read at opening, never read again.
    StaleSlot,
}

impl ReadPlan {
    /// The old path: seek and read, length asked at every read, inflate under the lock, flate2.
    #[cfg(test)]
    pub(crate) const OLD: ReadPlan = ReadPlan {
        positioned: false,
        inflate_unlocked: false,
        libdeflate: false,
        #[cfg(test)]
        sabotage: ReadSabotage::None,
    };

    pub(crate) fn from_switches() -> ReadPlan {
        ReadPlan {
            positioned: rc_data::opt_switch!("RUSTCRAFT_OPT_REGION_POSITIONED_READS"),
            inflate_unlocked: rc_data::opt_switch!("RUSTCRAFT_OPT_REGION_INFLATE_UNLOCKED"),
            libdeflate: rc_data::opt_switch!("RUSTCRAFT_OPT_REGION_LIBDEFLATE_READ"),
            #[cfg(test)]
            sabotage: ReadSabotage::None,
        }
    }

    /// The slot word is read again before each read (always, except under a test sabotage).
    fn rereads_slot(self) -> bool {
        #[cfg(test)]
        let stale = self.sabotage == ReadSabotage::StaleSlot;
        #[cfg(not(test))]
        let stale = false;
        !stale
    }
}

/// How chunks are written (`RUSTCRAFT_OPT_REGION_*`, each read once; tests build their own).
/// Every variant writes the same bytes at the same place.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct WritePlan {
    /// Sectors in use counted per sector and kept up to date, instead of rebuilt from the 1,024
    /// slots at every write (`RUSTCRAFT_OPT_REGION_SECTOR_BITMAP`).
    pub sector_bitmap: bool,
    /// Only the two header words of the chunk written (slot and timestamp), sectors and words
    /// written at their offset (`RUSTCRAFT_OPT_REGION_HEADER_WORDS`); the game rewrites the 8 KiB.
    pub header_words: bool,
    /// External `.mcc` streams of the region listed once, at the first write, instead of one file
    /// check per chunk written (`RUSTCRAFT_OPT_REGION_EXTERNAL_LIST`).
    pub external_list: bool,
}

impl WritePlan {
    /// The old path: occupancy rebuilt, full header, one `.mcc` check per write.
    #[cfg(test)]
    pub(crate) const OLD: WritePlan = WritePlan { sector_bitmap: false, header_words: false, external_list: false };

    pub(crate) fn from_switches() -> WritePlan {
        WritePlan {
            sector_bitmap: rc_data::opt_switch!("RUSTCRAFT_OPT_REGION_SECTOR_BITMAP"),
            header_words: rc_data::opt_switch!("RUSTCRAFT_OPT_REGION_HEADER_WORDS"),
            external_list: rc_data::opt_switch!("RUSTCRAFT_OPT_REGION_EXTERNAL_LIST"),
        }
    }
}

/// Lock per region file, shared by the whole process: the write thread and the
/// readers (generation thread, loading) never see a header and sectors
/// out of sync (header read after the file length, sectors freed then reused).
/// Readers with positioned reads share it; a writer, or a reader that moves the file cursor,
/// holds it alone.
type RegionLock = Arc<RwLock<()>>;

/// Number of known region locks from which the ones no file holds any more are forgotten.
const FORGET_UNUSED_LOCKS_FROM: usize = 1024;

fn file_lock(path: &Path) -> RegionLock {
    use std::sync::OnceLock;
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, RegionLock>>> = OnceLock::new();
    let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let mut m = LOCKS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    // One entry per file ever opened otherwise (thousands over a test run or a long flight).
    // A lock only the table holds protects nobody: dropping it is safe.
    if m.len() >= FORGET_UNUSED_LOCKS_FROM && !m.contains_key(&key) {
        m.retain(|_, l| Arc::strong_count(l) > 1);
    }
    m.entry(key).or_default().clone()
}

/// Runs `f` under the region lock: shared by positioned readers, alone otherwise.
fn locked<R>(lock: &RwLock<()>, plan: ReadPlan, f: impl FnOnce() -> R) -> R {
    #[cfg(test)]
    if plan.sabotage == ReadSabotage::NoLock {
        return f();
    }
    if plan.positioned {
        let _shared = lock.read().unwrap_or_else(|e| e.into_inner());
        f()
    } else {
        let _alone = lock.write().unwrap_or_else(|e| e.into_inner());
        f()
    }
}

fn now_secs() -> u32 {
    // Proofs never depend on the clock: their timestamps are fixed.
    #[cfg(test)]
    if let Some(t) = tests::FIXED_NOW.with(std::cell::Cell::get) {
        return t;
    }
    std::time::SystemTime::UNIX_EPOCH.elapsed().map_or(0, |d| d.as_secs() as u32)
}

/// Region file name of a chunk.
pub fn region_name(cx: i32, cz: i32) -> String {
    format!("r.{}.{}.mca", cx >> 5, cz >> 5)
}

/// Local index of a chunk in its region.
pub fn local_index(cx: i32, cz: i32) -> usize {
    ((cx & 31) + (cz & 31) * 32) as usize
}

fn parent_dir(path: &Path) -> &Path {
    path.parent().unwrap_or_else(|| Path::new("."))
}

fn external_name(cx: i32, cz: i32) -> String {
    format!("c.{cx}.{cz}.mcc")
}

/// A slot points at sectors that exist: past the header, not empty, starting inside the file.
fn valid_location(loc: u32, len: u64) -> bool {
    let (sector, count) = (u64::from(loc >> 8), u64::from(loc & 0xff));
    sector >= 2 && count > 0 && sector * SECTOR < len.max(1)
}

/// Reads at `offset` until `buf` is full or the file ends; number of bytes read. The file cursor
/// is not used, so several threads can read one handle at once.
fn read_at(file: &File, buf: &mut [u8], offset: u64) -> std::io::Result<usize> {
    let mut done = 0;
    while done < buf.len() {
        #[cfg(windows)]
        let n = std::os::windows::fs::FileExt::seek_read(file, &mut buf[done..], offset + done as u64);
        #[cfg(unix)]
        let n = std::os::unix::fs::FileExt::read_at(file, &mut buf[done..], offset + done as u64);
        match n {
            Ok(0) => break,
            Ok(n) => done += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(done)
}

/// Writes all of `buf` at `offset` (no separate seek).
#[cfg(unix)]
fn write_all_at(file: &File, buf: &[u8], offset: u64) -> std::io::Result<()> {
    std::os::unix::fs::FileExt::write_all_at(file, buf, offset)
}

/// Writes all of `buf` at `offset` (no separate seek).
#[cfg(windows)]
fn write_all_at(file: &File, buf: &[u8], offset: u64) -> std::io::Result<()> {
    let mut done = 0;
    while done < buf.len() {
        match std::os::windows::fs::FileExt::seek_write(file, &buf[done..], offset + done as u64) {
            Ok(0) => return Err(std::io::ErrorKind::WriteZero.into()),
            Ok(n) => done += n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(())
}

/// Which chunk of a region file: local index, and absolute coordinates (name of its `.mcc`).
#[derive(Clone, Copy)]
struct ChunkAt {
    index: usize,
    cx: i32,
    cz: i32,
}

/// What a read knows of the file: the chunk's slot and the file length, both refreshed by the read.
struct Known {
    slot: u32,
    len: u64,
}

/// Compressed stream of a chunk, read under the lock: compression type (external flag removed)
/// and the bytes, the stream being `bytes[range]`.
struct Compressed {
    kind: u8,
    bytes: Vec<u8>,
    range: std::ops::Range<usize>,
}

impl Compressed {
    fn whole(kind: u8, bytes: Vec<u8>) -> Compressed {
        let range = 0..bytes.len();
        Compressed { kind, bytes, range }
    }

    fn inflate_then<R>(&self, plan: ReadPlan, use_bytes: impl FnOnce(Cow<'_, [u8]>) -> R) -> Result<R, Unreadable> {
        inflate_then(self.kind, &self.bytes[self.range.clone()], plan.libdeflate, use_bytes)
    }
}

/// External stream `c.<x>.<z>.mcc` of a chunk.
fn read_external(dir: &Path, cx: i32, cz: i32, kind: u8) -> Result<Option<Compressed>, Unreadable> {
    let bytes = std::fs::read(dir.join(external_name(cx, cz))).map_err(io_error)?;
    Ok(Some(Compressed::whole(kind & 0x7f, bytes)))
}

fn io_error(e: std::io::Error) -> Unreadable {
    Unreadable::Io(e.to_string())
}

/// Under the region lock: the slot word read again (a writer may have moved the chunk since
/// opening: sectors freed and reused), then the chunk's stream.
fn fetch(
    file: &File,
    dir: &Path,
    at: ChunkAt,
    known: &mut Known,
    plan: ReadPlan,
) -> Result<Option<Compressed>, Unreadable> {
    if plan.positioned { fetch_positioned(file, dir, at, known, plan) } else { fetch_sequential(file, dir, at, known) }
}

/// Old read (`RUSTCRAFT_OPT_REGION_POSITIONED_READS=0`): seek and read the slot word, length of
/// the file, seek and read the 5-byte head, then the stream; moves the file cursor.
fn fetch_sequential(
    mut file: &File,
    dir: &Path,
    at: ChunkAt,
    known: &mut Known,
) -> Result<Option<Compressed>, Unreadable> {
    let mut word = [0u8; 4];
    file.seek(SeekFrom::Start(at.index as u64 * 4)).map_err(io_error)?;
    if file.read_exact(&mut word).is_ok() {
        known.len = file.metadata().map_err(io_error)?.len();
        let fresh = u32::from_be_bytes(word);
        known.slot = if fresh == 0 || valid_location(fresh, known.len) { fresh } else { 0 };
    }
    if known.slot == 0 {
        return Ok(None);
    }
    let (sector, count) = (u64::from(known.slot >> 8), u64::from(known.slot & 0xff));
    file.seek(SeekFrom::Start(sector * SECTOR)).map_err(io_error)?;
    let mut head = [0u8; 5];
    file.read_exact(&mut head).map_err(io_error)?;
    let length = u64::from(u32::from_be_bytes([head[0], head[1], head[2], head[3]]));
    if length == 0 {
        return Ok(None);
    }
    let kind = head[4];
    if kind & 0x80 != 0 {
        return read_external(dir, at.cx, at.cz, kind);
    }
    if length - 1 > count * SECTOR - 5 {
        return Err(Unreadable::Truncated);
    }
    let mut buf = vec![0u8; (length - 1) as usize];
    file.read_exact(&mut buf).map_err(|_| Unreadable::Truncated)?;
    Ok(Some(Compressed::whole(kind, buf)))
}

/// Two calls instead of six: the slot word, then the whole run of sectors at once (bounded by
/// the end of the file, like the game). The length is asked only when the slot or its sectors
/// go past the end known so far: region files only grow while they are open.
fn fetch_positioned(
    file: &File,
    dir: &Path,
    at: ChunkAt,
    known: &mut Known,
    plan: ReadPlan,
) -> Result<Option<Compressed>, Unreadable> {
    let mut word = [0u8; 4];
    if plan.rereads_slot() && matches!(read_at(file, &mut word, at.index as u64 * 4), Ok(4)) {
        let fresh = u32::from_be_bytes(word);
        if fresh != 0 && !valid_location(fresh, known.len) {
            known.len = file.metadata().map_err(io_error)?.len();
        }
        known.slot = if fresh == 0 || valid_location(fresh, known.len) { fresh } else { 0 };
    }
    #[cfg(test)]
    if plan.sabotage == ReadSabotage::NoLock {
        std::thread::sleep(std::time::Duration::from_micros(300));
    }
    if known.slot == 0 {
        return Ok(None);
    }
    let (sector, count) = (u64::from(known.slot >> 8), u64::from(known.slot & 0xff));
    let (offset, wanted) = (sector * SECTOR, count * SECTOR);
    if offset + wanted > known.len {
        known.len = file.metadata().map_err(io_error)?.len();
    }
    let mut buf = vec![0u8; wanted.min(known.len.saturating_sub(offset)) as usize];
    let got = read_at(file, &mut buf, offset).map_err(io_error)?;
    buf.truncate(got);
    let mut head = [0u8; 5];
    // Fewer than 5 bytes left: the same error as the old read of the head.
    (&buf[..]).read_exact(&mut head).map_err(io_error)?;
    let length = u64::from(u32::from_be_bytes([head[0], head[1], head[2], head[3]]));
    if length == 0 {
        return Ok(None);
    }
    let kind = head[4];
    if kind & 0x80 != 0 {
        return read_external(dir, at.cx, at.cz, kind);
    }
    let end = 5 + (length - 1) as usize;
    if length - 1 > count * SECTOR - 5 || end > buf.len() {
        return Err(Unreadable::Truncated);
    }
    Ok(Some(Compressed { kind, bytes: buf, range: 5..end }))
}

/// Reads a chunk under the region lock and hands its decompressed bytes to `use_bytes`. The
/// stream is inflated after the lock is released (`inflate_unlocked`), or still under it (old path).
fn read_then<R>(
    lock: &RwLock<()>,
    plan: ReadPlan,
    fetch: impl FnOnce() -> Result<Option<Compressed>, Unreadable>,
    use_bytes: impl FnOnce(Cow<'_, [u8]>) -> R,
) -> Result<Option<R>, Unreadable> {
    if plan.inflate_unlocked {
        let compressed = locked(lock, plan, fetch)?;
        compressed.map(|c| c.inflate_then(plan, use_bytes)).transpose()
    } else {
        let bytes = locked(lock, plan, || fetch()?.map(|c| c.inflate_then(plan, |b| b.into_owned())).transpose())?;
        Ok(bytes.map(|b| use_bytes(Cow::Owned(b))))
    }
}

pub struct RegionFile {
    pub path: PathBuf,
    pub(crate) file: File,
    /// `sector << 8 | count`, 0 = absent. Changed only through `set_location` (occupancy).
    pub(crate) locations: Box<[u32; 1024]>,
    pub(crate) timestamps: Box<[u32; 1024]>,
    pub(crate) len: u64,
    lock: RegionLock,
    /// Invalid slots discarded on opening.
    pub dropped: usize,
    pub(crate) read_plan: ReadPlan,
    pub(crate) write_plan: WritePlan,
    /// Sectors in use, counted at the first allocation then kept up to date (`sector_bitmap`).
    sectors: Option<SectorUse>,
    /// A header word in memory differs from the file (invalid slot cleared, truncated header):
    /// the next header write rewrites the whole header, as the old path always did.
    header_stale_on_disk: bool,
    /// Local indices that have an external `.mcc` stream, listed at the first write that needs it
    /// (`external_list`).
    external: Option<HashSet<usize>>,
}

impl RegionFile {
    /// Opens an existing region (read-only, or read and write).
    pub fn open(path: &Path, writable: bool) -> std::io::Result<RegionFile> {
        let mut file = std::fs::OpenOptions::new().read(true).write(writable).open(path)?;
        let lock = file_lock(path);
        let guard = lock.read().unwrap_or_else(|e| e.into_inner());
        let len = file.metadata()?.len();
        let mut header = vec![0u8; HEADER];
        // Truncated header: what is missing is 0.
        let n = (len as usize).min(HEADER);
        file.read_exact(&mut header[..n])?;
        drop(guard);
        let word = |i: usize| u32::from_be_bytes(header[i * 4..i * 4 + 4].try_into().unwrap_or_default());
        let mut locations = Box::new([0u32; 1024]);
        let mut timestamps = Box::new([0u32; 1024]);
        for i in 0..1024 {
            locations[i] = word(i);
            timestamps[i] = word(1024 + i);
        }
        let mut r = RegionFile {
            path: path.to_owned(),
            file,
            locations,
            timestamps,
            len,
            lock,
            dropped: 0,
            read_plan: ReadPlan::from_switches(),
            write_plan: WritePlan::from_switches(),
            sectors: None,
            header_stale_on_disk: n < HEADER,
            external: None,
        };
        for i in 0..1024 {
            if r.locations[i] != 0 && !valid_location(r.locations[i], r.len) {
                eprintln!("region {}: slot {i} invalid ({:#x}), ignored", path.display(), r.locations[i]);
                r.locations[i] = 0;
                r.dropped += 1;
                r.header_stale_on_disk = true;
            }
        }
        Ok(r)
    }

    /// Opens for writing, creating the file (null header) if it does not exist.
    pub fn open_or_create(path: &Path) -> std::io::Result<RegionFile> {
        if !path.is_file() {
            if let Some(d) = path.parent() {
                std::fs::create_dir_all(d)?;
            }
            std::fs::write(path, vec![0u8; HEADER])?;
        }
        RegionFile::open(path, true)
    }

    /// Changes a slot, the sector occupancy following it.
    fn set_location(&mut self, i: usize, loc: u32) {
        if let Some(s) = &mut self.sectors {
            s.release(self.locations[i]);
            s.take(loc);
        }
        self.locations[i] = loc;
    }

    /// First free hole of `n` sectors (the end of the file counts as free).
    fn allocate(&mut self, n: usize) -> usize {
        if !self.write_plan.sector_bitmap {
            return self.allocate_rebuilt(n);
        }
        let locations = &self.locations;
        self.sectors.get_or_insert_with(|| SectorUse::of(locations)).first_free(n)
    }

    /// Occupied sectors (header included).
    fn used(&self) -> Vec<bool> {
        let total = (self.len.div_ceil(SECTOR) as usize).max(2);
        let mut used = vec![false; total];
        used[0] = true;
        used[1] = true;
        for &loc in self.locations.iter().filter(|&&l| l != 0) {
            let (s, n) = ((loc >> 8) as usize, (loc & 0xff) as usize);
            if used.len() < s + n {
                used.resize(s + n, false);
            }
            used[s..s + n].iter_mut().for_each(|u| *u = true);
        }
        used
    }

    /// Old allocation (`RUSTCRAFT_OPT_REGION_SECTOR_BITMAP=0`): occupancy rebuilt from the slots.
    fn allocate_rebuilt(&self, n: usize) -> usize {
        let used = self.used();
        let mut run = 0;
        for (i, &u) in used.iter().enumerate() {
            run = if u { 0 } else { run + 1 };
            if run == n {
                return i + 1 - n;
            }
        }
        used.len() - run
    }

    fn write_header(&mut self) -> std::io::Result<()> {
        let mut h = Vec::with_capacity(HEADER);
        self.locations.iter().for_each(|l| h.extend_from_slice(&l.to_be_bytes()));
        self.timestamps.iter().for_each(|t| h.extend_from_slice(&t.to_be_bytes()));
        self.file.seek(SeekFrom::Start(0))?;
        self.file.write_all(&h)
    }

    /// Header after a change of slot `i`: its two words only when the rest of the file already
    /// matches memory (`header_words`), otherwise the whole 8 KiB (same bytes on disk either way).
    fn write_header_for(&mut self, i: usize) -> std::io::Result<()> {
        let whole = !self.write_plan.header_words || self.header_stale_on_disk;
        // Marked until the write succeeds: after a failed write, the next one rewrites everything.
        self.header_stale_on_disk = true;
        if whole {
            self.write_header()?;
        } else {
            write_all_at(&self.file, &self.locations[i].to_be_bytes(), i as u64 * 4)?;
            write_all_at(&self.file, &self.timestamps[i].to_be_bytes(), 4096 + i as u64 * 4)?;
        }
        self.header_stale_on_disk = false;
        Ok(())
    }

    fn write_sectors(&mut self, data: &[u8], start: usize) -> std::io::Result<()> {
        let offset = start as u64 * SECTOR;
        if self.write_plan.header_words {
            return write_all_at(&self.file, data, offset);
        }
        self.file.seek(SeekFrom::Start(offset))?;
        self.file.write_all(data)
    }

    /// Whether chunk `i` has an external stream to remove (taken out of the list if so).
    fn take_external(&mut self, i: usize, cx: i32, cz: i32, mcc: &Path) -> bool {
        if !self.write_plan.external_list {
            return mcc.is_file();
        }
        let dir = parent_dir(&self.path);
        self.external.get_or_insert_with(|| list_external(dir, cx >> 5, cz >> 5)).remove(&i)
    }

    /// Writes a chunk (§ 3.3): zlib stream in the first free hole (or `.mcc` beyond
    /// 255 sectors), header rewritten, then only the old sectors freed.
    pub fn write(&mut self, i: usize, cx: i32, cz: i32, nbt: &[u8]) -> std::io::Result<()> {
        self.write_compressed(i, cx, cz, &crate::nbtio::zlib(nbt))
    }

    /// Like [`RegionFile::write`], for a zlib stream already compressed (type 2).
    pub fn write_compressed(&mut self, i: usize, cx: i32, cz: i32, stream: &[u8]) -> std::io::Result<()> {
        let lock = self.lock.clone();
        let _guard = lock.write().unwrap_or_else(|e| e.into_inner());
        let dir = parent_dir(&self.path).to_path_buf();
        let mcc = dir.join(external_name(cx, cz));
        let external = (5 + stream.len()).div_ceil(SECTOR as usize) >= 256;
        let (kind, body): (u8, &[u8]) = if external { (2 | 0x80, &[]) } else { (2, stream) };
        let mut sector_data = Vec::with_capacity(5 + body.len());
        sector_data.extend_from_slice(&((body.len() + 1) as u32).to_be_bytes());
        sector_data.push(kind);
        sector_data.extend_from_slice(body);
        let n = sector_data.len().div_ceil(SECTOR as usize);
        // The old version still counts as occupied (copy on write: it stays intact
        // until the header is rewritten).
        let start = self.allocate(n);
        let tmp = dir.join(format!("c.{cx}.{cz}.mcc.tmp"));
        if external {
            std::fs::write(&tmp, stream)?;
        }
        self.write_sectors(&sector_data, start)?;
        self.len = self.len.max(start as u64 * SECTOR + sector_data.len() as u64);
        self.set_location(i, ((start as u32) << 8) | n as u32);
        self.timestamps[i] = now_secs();
        self.write_header_for(i)?;
        if external {
            std::fs::rename(&tmp, &mcc)?;
            if let Some(list) = &mut self.external {
                list.insert(i);
            }
        } else if self.take_external(i, cx, cz, &mcc) {
            let _ = std::fs::remove_file(&mcc);
        }
        Ok(())
    }

    /// Erases a chunk (§ 3.3.5).
    pub fn erase(&mut self, i: usize, cx: i32, cz: i32) -> std::io::Result<()> {
        let lock = self.lock.clone();
        let _guard = lock.write().unwrap_or_else(|e| e.into_inner());
        self.set_location(i, 0);
        self.timestamps[i] = now_secs();
        self.write_header_for(i)?;
        let _ = std::fs::remove_file(parent_dir(&self.path).join(external_name(cx, cz)));
        if let Some(list) = &mut self.external {
            list.remove(&i);
        }
        Ok(())
    }

    pub fn sync(&mut self) -> std::io::Result<()> {
        self.file.sync_all()
    }

    /// Closing (§ 3.3.6): padding to the next multiple of 4,096, forced write.
    pub fn close(mut self) -> std::io::Result<()> {
        let padded = self.len.div_ceil(SECTOR) * SECTOR;
        if padded > self.len {
            self.file.seek(SeekFrom::Start(padded - 1))?;
            self.file.write_all(&[0])?;
        }
        self.file.sync_all()
    }

    /// Local indices of the chunks present.
    pub fn present(&self) -> impl Iterator<Item = usize> + '_ {
        (0..1024).filter(|&i| self.locations[i] != 0)
    }

    pub fn timestamp(&self, i: usize) -> u32 {
        self.timestamps[i]
    }

    /// Decompressed NBT bytes of a chunk (`None`: absent). `cx`, `cz`: absolute coordinates
    /// (name of the external stream).
    pub fn read(&mut self, i: usize, cx: i32, cz: i32) -> Result<Option<Vec<u8>>, Unreadable> {
        let plan = self.read_plan;
        let mut known = Known { slot: self.locations[i], len: self.len };
        let (file, dir, at) = (&self.file, parent_dir(&self.path), ChunkAt { index: i, cx, cz });
        let bytes = read_then(&self.lock, plan, || fetch(file, dir, at, &mut known, plan), |b| b.into_owned());
        self.len = known.len;
        if known.slot != self.locations[i] {
            // Taken again from the file: memory may now differ from the words on disk.
            self.header_stale_on_disk = true;
            self.set_location(i, known.slot);
        }
        bytes
    }
}

/// External streams `c.<x>.<z>.mcc` of region (`rx`, `rz`) in `dir`: their local indices.
fn list_external(dir: &Path, rx: i32, rz: i32) -> HashSet<usize> {
    let Ok(entries) = std::fs::read_dir(dir) else { return HashSet::new() };
    entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().into_string().ok()?;
            // Windows file names ignore case, like the file check this list replaces.
            #[cfg(windows)]
            let name = name.to_ascii_lowercase();
            let mut parts = name.strip_prefix("c.")?.strip_suffix(".mcc")?.split('.');
            let (cx, cz): (i32, i32) = (parts.next()?.parse().ok()?, parts.next()?.parse().ok()?);
            (parts.next().is_none() && name == external_name(cx, cz) && (cx >> 5, cz >> 5) == (rx, rz))
                .then(|| local_index(cx, cz))
        })
        .collect()
}

/// How many slots cover each sector (the two header sectors count once): a sector is free at 0,
/// and every sector past the end is free. Overlapping slots (damaged file) each count, so a sector
/// is reused only once no slot points at it any more, like the occupancy rebuilt from the slots.
struct SectorUse(Vec<u16>);

impl SectorUse {
    fn of(locations: &[u32; 1024]) -> SectorUse {
        let mut s = SectorUse(vec![1, 1]);
        locations.iter().for_each(|&l| s.take(l));
        s
    }

    fn sectors(loc: u32) -> std::ops::Range<usize> {
        let (start, count) = ((loc >> 8) as usize, (loc & 0xff) as usize);
        start..start + count
    }

    fn take(&mut self, loc: u32) {
        let r = SectorUse::sectors(loc);
        if self.0.len() < r.end {
            self.0.resize(r.end, 0);
        }
        self.0[r].iter_mut().for_each(|c| *c += 1);
    }

    fn release(&mut self, loc: u32) {
        let r = SectorUse::sectors(loc);
        let end = r.end.min(self.0.len());
        self.0[r.start.min(end)..end].iter_mut().for_each(|c| *c = c.saturating_sub(1));
    }

    /// First run of `n` free sectors (first fit, the end of the file included).
    fn first_free(&self, n: usize) -> usize {
        let mut run = 0;
        for (i, &c) in self.0.iter().enumerate() {
            run = if c == 0 { run + 1 } else { 0 };
            if run == n {
                return i + 1 - n;
            }
        }
        self.0.len() - run
    }
}

/// A region file opened once for reading and shared by every task that reads it: positioned
/// reads leave no file cursor to share, and the header and the lock are fetched once. Each read
/// still reads the slot word again under the lock.
pub(crate) struct SharedRegion {
    region: RegionFile,
    /// File length known so far (it only grows).
    len: AtomicU64,
}

impl SharedRegion {
    pub(crate) fn open(path: &Path) -> std::io::Result<SharedRegion> {
        let region = RegionFile::open(path, false)?;
        Ok(SharedRegion { len: AtomicU64::new(region.len), region })
    }

    /// Local indices of the chunks present at opening.
    pub(crate) fn present(&self) -> impl Iterator<Item = usize> + '_ {
        self.region.present()
    }

    /// Like [`RegionFile::read`], the decompressed bytes lent to `use_bytes` (they live in a buffer
    /// of the thread with libdeflate) instead of returned.
    pub(crate) fn read_then<R>(
        &self,
        i: usize,
        cx: i32,
        cz: i32,
        plan: ReadPlan,
        use_bytes: impl FnOnce(&[u8]) -> R,
    ) -> Result<Option<R>, Unreadable> {
        let r = &self.region;
        let mut known = Known { slot: r.locations[i], len: self.len.load(Ordering::Relaxed) };
        let (dir, at) = (parent_dir(&r.path), ChunkAt { index: i, cx, cz });
        let out = read_then(&r.lock, plan, || fetch(&r.file, dir, at, &mut known, plan), |b| use_bytes(&b));
        self.len.fetch_max(known.len, Ordering::Relaxed);
        out
    }
}

/// Open region files kept by a dimension between loads (`RUSTCRAFT_OPT_REGION_HANDLE_CACHE`).
/// At most as many as the game keeps open; dropped with the dimension or on closing the world.
#[derive(Default)]
pub(crate) struct SharedRegions(Mutex<HashMap<PathBuf, Arc<SharedRegion>>>);

/// The game keeps up to 256 region files open per folder kind.
const MAX_OPEN_REGIONS: usize = 256;

impl SharedRegions {
    fn files(&self) -> std::sync::MutexGuard<'_, HashMap<PathBuf, Arc<SharedRegion>>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The region file at `path`, opened if needed. A missing file is not remembered: the write
    /// thread may create it later.
    pub(crate) fn get(&self, path: &Path) -> Option<Arc<SharedRegion>> {
        if let Some(r) = self.files().get(path) {
            return Some(r.clone());
        }
        // Opened outside the table lock: other folders and regions keep being served meanwhile.
        let opened = Arc::new(SharedRegion::open(path).ok()?);
        let mut files = self.files();
        if files.len() >= MAX_OPEN_REGIONS {
            files.clear();
        }
        Some(files.entry(path.to_owned()).or_insert(opened).clone())
    }

    /// Closes every kept file (the ones a load still reads close when it ends).
    pub(crate) fn close_all(&self) {
        self.files().clear();
    }

    #[cfg(test)]
    pub(crate) fn open_count(&self) -> usize {
        self.files().len()
    }
}

thread_local! {
    /// libdeflate state of this thread, reused from chunk to chunk.
    static INFLATER: std::cell::RefCell<libdeflater::Decompressor> =
        std::cell::RefCell::new(libdeflater::Decompressor::new());
    /// Output buffer of this thread, as large as the largest chunk seen (taken while in use).
    static INFLATED: std::cell::Cell<Vec<u8>> = const { std::cell::Cell::new(Vec::new()) };
}

/// First output size tried for a stream: 8 times the stream (chunks inflate about 7 times), at
/// least 64 KiB (chunks of the bench world: 21 KB on average). Too small: doubled and inflated again.
const FIRST_GUESS_MIN: usize = 64 << 10;
/// Beyond this size the old decoder takes over.
const MAX_INFLATED: usize = 512 << 20;
/// A thread's buffer is kept up to this size (larger: an external stream, dropped after use).
const KEEP_AT_MOST: usize = 8 << 20;

/// Inflates a zlib stream into `out`, grown until it fits; length of the result, `None` on any
/// failure (bad data, result past [`MAX_INFLATED`]).
fn zlib_into(stream: &[u8], out: &mut Vec<u8>) -> Option<usize> {
    let guess = (stream.len().saturating_mul(8)).clamp(FIRST_GUESS_MIN, MAX_INFLATED);
    if out.len() < guess {
        out.resize(guess, 0);
    }
    INFLATER.with(|d| {
        let mut d = d.borrow_mut();
        loop {
            match d.zlib_decompress(stream, out) {
                Ok(n) => return Some(n),
                Err(libdeflater::DecompressionError::InsufficientSpace) if out.len() < MAX_INFLATED => {
                    let bigger = out.len().saturating_mul(2).min(MAX_INFLATED);
                    out.resize(bigger, 0);
                }
                Err(_) => return None,
            }
        }
    })
}

/// Decompresses a chunk stream and hands the bytes to `use_bytes`: borrowed from the thread's
/// buffer (libdeflate) or the stream itself (uncompressed), owned otherwise, so that a caller who
/// keeps them copies only what is borrowed. With `libdeflate`, a zlib stream goes to libdeflate
/// first; if it refuses it (damaged, truncated, wrong checksum), the old decoder decides, so that
/// the outcome (partial data, error message) stays the same.
fn inflate_then<R>(
    kind: u8,
    stream: &[u8],
    libdeflate: bool,
    use_bytes: impl FnOnce(Cow<'_, [u8]>) -> R,
) -> Result<R, Unreadable> {
    match kind {
        2 if libdeflate => {
            let mut out = INFLATED.take();
            let result = match zlib_into(stream, &mut out) {
                Some(n) => Ok(use_bytes(Cow::Borrowed(&out[..n]))),
                None => decompress(kind, stream).map(|v| use_bytes(Cow::Owned(v))),
            };
            if out.len() <= KEEP_AT_MOST {
                INFLATED.set(out);
            }
            result
        }
        3 => Ok(use_bytes(Cow::Borrowed(stream))),
        _ => decompress(kind, stream).map(|v| use_bytes(Cow::Owned(v))),
    }
}

/// Decompression of a chunk stream according to its type (§ 3.2).
pub fn decompress(kind: u8, stream: &[u8]) -> Result<Vec<u8>, Unreadable> {
    let mut out = Vec::with_capacity(stream.len() * 6);
    let r = match kind {
        1 => flate2::read::GzDecoder::new(stream).read_to_end(&mut out),
        2 => flate2::read::ZlibDecoder::new(stream).read_to_end(&mut out),
        3 => return Ok(stream.to_vec()),
        k => return Err(Unreadable::Compression(k)),
    };
    r.map(|_| out).map_err(|e| Unreadable::Inflate(e.to_string()))
}

#[cfg(test)]
#[path = "region_tests.rs"]
pub(crate) mod tests;
