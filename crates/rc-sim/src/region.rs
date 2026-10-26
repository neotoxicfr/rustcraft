//! `.mca` region files (`anvil-world.md` § 3): header of 1,024 slots and
//! timestamps, chunks compressed in 4,096-byte sectors, external `.mcc` streams.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

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

/// Lock per region file, shared by the whole process: the write thread and the
/// readers (generation thread, loading) never see a header and sectors
/// out of sync (header read after the file length, sectors freed then reused).
fn file_lock(path: &Path) -> std::sync::Arc<std::sync::Mutex<()>> {
    use hashbrown::HashMap;
    use std::sync::{Arc, Mutex, OnceLock};
    static LOCKS: OnceLock<Mutex<HashMap<PathBuf, Arc<Mutex<()>>>>> = OnceLock::new();
    let key = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_owned());
    let mut m = LOCKS.get_or_init(Default::default).lock().unwrap_or_else(|e| e.into_inner());
    m.entry(key).or_default().clone()
}

fn now_secs() -> u32 {
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

pub struct RegionFile {
    pub path: PathBuf,
    pub(crate) file: File,
    /// `sector << 8 | count`, 0 = absent.
    pub(crate) locations: Box<[u32; 1024]>,
    pub(crate) timestamps: Box<[u32; 1024]>,
    pub(crate) len: u64,
    lock: std::sync::Arc<std::sync::Mutex<()>>,
    /// Invalid slots discarded on opening.
    pub dropped: usize,
}

impl RegionFile {
    /// Opens an existing region (read-only, or read and write).
    pub fn open(path: &Path, writable: bool) -> std::io::Result<RegionFile> {
        let mut file = std::fs::OpenOptions::new().read(true).write(writable).open(path)?;
        let lock = file_lock(path);
        let guard = lock.lock().unwrap_or_else(|e| e.into_inner());
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
        let mut r = RegionFile { path: path.to_owned(), file, locations, timestamps, len, lock, dropped: 0 };
        for i in 0..1024 {
            if r.locations[i] != 0 && !r.valid_location(r.locations[i]) {
                eprintln!("region {}: slot {i} invalid ({:#x}), ignored", path.display(), r.locations[i]);
                r.locations[i] = 0;
                r.dropped += 1;
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

    /// First free hole of `n` sectors (the end of the file counts as free).
    fn allocate(&self, n: usize) -> usize {
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

    /// Writes a chunk (§ 3.3): zlib stream in the first free hole (or `.mcc` beyond
    /// 255 sectors), header rewritten, then only the old sectors freed.
    pub fn write(&mut self, i: usize, cx: i32, cz: i32, nbt: &[u8]) -> std::io::Result<()> {
        let stream = crate::nbtio::zlib(nbt);
        let lock = self.lock.clone();
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        let dir = self.path.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let mcc = dir.join(format!("c.{cx}.{cz}.mcc"));
        let external = (5 + stream.len()).div_ceil(SECTOR as usize) >= 256;
        let (kind, body): (u8, &[u8]) = if external { (2 | 0x80, &[]) } else { (2, &stream) };
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
            std::fs::write(&tmp, &stream)?;
        }
        self.file.seek(SeekFrom::Start(start as u64 * SECTOR))?;
        self.file.write_all(&sector_data)?;
        self.len = self.len.max(start as u64 * SECTOR + sector_data.len() as u64);
        self.locations[i] = ((start as u32) << 8) | n as u32;
        self.timestamps[i] = now_secs();
        self.write_header()?;
        if external {
            std::fs::rename(&tmp, &mcc)?;
        } else if mcc.is_file() {
            let _ = std::fs::remove_file(&mcc);
        }
        Ok(())
    }

    /// Erases a chunk (§ 3.3.5).
    pub fn erase(&mut self, i: usize, cx: i32, cz: i32) -> std::io::Result<()> {
        let lock = self.lock.clone();
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        self.locations[i] = 0;
        self.timestamps[i] = now_secs();
        self.write_header()?;
        let dir = self.path.parent().map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let _ = std::fs::remove_file(dir.join(format!("c.{cx}.{cz}.mcc")));
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

    fn valid_location(&self, loc: u32) -> bool {
        let (sector, count) = (u64::from(loc >> 8), u64::from(loc & 0xff));
        sector >= 2 && count > 0 && sector * SECTOR < self.len.max(1)
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
        let io = |e: std::io::Error| Unreadable::Io(e.to_string());
        let lock = self.lock.clone();
        let _guard = lock.lock().unwrap_or_else(|e| e.into_inner());
        // Slot re-read under the lock: a writer may have moved the chunk since
        // opening (sectors freed and reused).
        let mut word = [0u8; 4];
        self.file.seek(SeekFrom::Start(i as u64 * 4)).map_err(io)?;
        if self.file.read_exact(&mut word).is_ok() {
            self.len = self.file.metadata().map_err(io)?.len();
            let fresh = u32::from_be_bytes(word);
            self.locations[i] = if fresh == 0 || self.valid_location(fresh) { fresh } else { 0 };
        }
        let loc = self.locations[i];
        if loc == 0 {
            return Ok(None);
        }
        let (sector, count) = (u64::from(loc >> 8), u64::from(loc & 0xff));
        self.file.seek(SeekFrom::Start(sector * SECTOR)).map_err(io)?;
        let mut head = [0u8; 5];
        self.file.read_exact(&mut head).map_err(io)?;
        let length = u32::from_be_bytes([head[0], head[1], head[2], head[3]]) as u64;
        if length == 0 {
            return Ok(None);
        }
        let kind = head[4];
        let stream = if kind & 0x80 != 0 {
            let dir = self.path.parent().unwrap_or_else(|| Path::new("."));
            std::fs::read(dir.join(format!("c.{cx}.{cz}.mcc"))).map_err(io)?
        } else {
            if length - 1 > count * SECTOR - 5 {
                return Err(Unreadable::Truncated);
            }
            let mut buf = vec![0u8; (length - 1) as usize];
            self.file.read_exact(&mut buf).map_err(|_| Unreadable::Truncated)?;
            buf
        };
        decompress(kind & 0x7f, &stream).map(Some)
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
mod tests {
    use super::*;

    #[test]
    fn names_and_indices() {
        assert_eq!(region_name(-1, 31), "r.-1.0.mca");
        assert_eq!(region_name(32, -33), "r.1.-2.mca");
        assert_eq!(local_index(-1, -1), 1023);
        assert_eq!(local_index(33, 2), 65);
    }

    /// Reader opening the region while the writer appends chunks at the end of the file
    /// (generation thread against write thread): never an invalid slot, and each
    /// announced chunk can be re-read.
    #[test]
    fn concurrent_reader_never_sees_invalid_locations() {
        let dir = std::env::temp_dir().join(format!("rc-region-race-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("r.0.0.mca");
        let _ = std::fs::remove_file(&path);
        let mut w = RegionFile::open_or_create(&path).unwrap();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let (p2, s2) = (path.clone(), stop.clone());
        let reader = std::thread::spawn(move || {
            let (mut dropped, mut bad, mut opens) = (0usize, 0usize, 0usize);
            while !s2.load(std::sync::atomic::Ordering::Relaxed) {
                let Ok(mut r) = RegionFile::open(&p2, false) else { continue };
                opens += 1;
                dropped += r.dropped;
                let present: Vec<usize> = r.present().collect();
                for i in present {
                    let res = r.read(i, (i % 32) as i32, (i / 32) as i32);
                    if !matches!(res, Ok(Some(_))) {
                        if bad < 3 {
                            eprintln!(
                                "  {i}: {:?} (slot {:#x})",
                                res.map(|o| o.map(|v| v.len())),
                                r.locations[i]
                            );
                        }
                        bad += 1;
                    }
                }
            }
            (dropped, bad, opens)
        });
        // Poorly compressible content: several sectors per chunk, rewrites moved.
        let mut seed = 1u64;
        for round in 0..6 {
            for i in 0..1024usize {
                let n = 2000 + (i * 37 + round * 101) % 9000;
                let body: Vec<u8> = (0..n)
                    .map(|_| {
                        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                        (seed >> 56) as u8
                    })
                    .collect();
                w.write(i, (i % 32) as i32, (i / 32) as i32, &body).unwrap();
            }
        }
        stop.store(true, std::sync::atomic::Ordering::Relaxed);
        let (dropped, bad, opens) = reader.join().unwrap();
        w.close().unwrap();
        let _ = std::fs::remove_dir_all(&dir);
        eprintln!("{opens} openings, {dropped} invalid slots, {bad} unreadable chunks");
        assert_eq!((dropped, bad), (0, 0));
    }
}
