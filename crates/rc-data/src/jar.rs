//! Index of the game's jar, built once per process and shared: the entries of the base pack
//! (`data/**`) and the English language (`assets/minecraft/lang/en_us.json`), decompressed only
//! once. Registries, recipes, advancements, generation (two readers), the server's
//! language, world creation and `/reload` borrow the same bytes (`Arc`) instead of reopening
//! the jar each on their own (five openings and four copies of the same JSONs per singleplayer opening).
//!
//! The jar of a version is immutable: the cache key (path, size, modification date)
//! is enough to recognise a replaced jar, which is then re-read.
//!
//! Switches (read once):
//! - `RUSTCRAFT_OPT_JARINDEX=0`: each reader re-reads the jar as before (old path);
//! - `RUSTCRAFT_OPT_JARIO=file|read|mmap`: unbuffered file (old), jar read in one block into
//!   memory, or memory-mapped;
//! - `RUSTCRAFT_OPT_JARFILTER=0`: each entry is opened (local header read) before sorting by name;
//! - `RUSTCRAFT_OPT_JARPAR=0/1`: decompression of the retained entries on several threads.

use std::collections::BTreeMap;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

/// English language, the only `assets/` resource read by the integrated server.
pub const EN_US: &str = "assets/minecraft/lang/en_us.json";

/// Entry retained by the index.
pub fn retained(name: &str) -> bool {
    name.starts_with("data/") || name == EN_US
}

/// Content of an entry: text (UTF-8 checked once) or binary (`.nbt` templates).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Blob {
    Text(Arc<str>),
    Bin(Arc<[u8]>),
}

impl Blob {
    fn new(v: Vec<u8>) -> Blob {
        match String::from_utf8(v) {
            Ok(s) => Blob::Text(Arc::from(s)),
            Err(e) => Blob::Bin(Arc::from(e.into_bytes())),
        }
    }

    pub fn as_bytes(&self) -> &[u8] {
        match self {
            Blob::Text(t) => t.as_bytes(),
            Blob::Bin(b) => b,
        }
    }

    /// Shared bytes (no copy: an `Arc<str>` reads as an `Arc<[u8]>`).
    pub fn bytes(&self) -> Arc<[u8]> {
        match self {
            Blob::Text(t) => Arc::<[u8]>::from(t.clone()),
            Blob::Bin(b) => b.clone(),
        }
    }

    /// Shared text, `None` if the entry is not valid UTF-8.
    pub fn text(&self) -> Option<Arc<str>> {
        match self {
            Blob::Text(t) => Some(t.clone()),
            Blob::Bin(_) => None,
        }
    }
}

/// Retained entries of the jar, by name (`BTreeMap` order).
#[derive(Debug, Default, PartialEq, Eq)]
pub struct JarIndex {
    files: BTreeMap<String, Blob>,
    /// Retained entries that are unreadable (decompression or checksum): name → message.
    failed: BTreeMap<String, String>,
    /// First entry whose header is unreadable (only without a filter: with a filter, the
    /// non-retained entries are never opened).
    broken: Option<String>,
}

impl JarIndex {
    pub fn files(&self) -> &BTreeMap<String, Blob> {
        &self.files
    }

    pub fn get(&self, name: &str) -> Option<&Blob> {
        self.files.get(name)
    }

    /// Retained entries whose read failed (strict readers fail as before).
    pub fn failed(&self) -> &BTreeMap<String, String> {
        &self.failed
    }

    pub fn broken(&self) -> Option<&str> {
        self.broken.as_deref()
    }

    /// Entries whose name starts with `prefix` (`BTreeMap` range, no full walk).
    pub fn with_prefix<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = (&'a String, &'a Blob)> + 'a {
        self.files.range::<str, _>((std::ops::Bound::Included(prefix), std::ops::Bound::Unbounded)).take_while(move |(k, _)| k.starts_with(prefix))
    }

    /// Decompressed bytes kept (memory report).
    pub fn bytes_len(&self) -> usize {
        self.files.values().map(|b| b.as_bytes().len()).sum()
    }
}

/// Failure to open the jar: I/O (file missing…) or invalid archive.
#[derive(Debug)]
pub enum JarError {
    Io(std::io::Error),
    Zip(String),
}

impl std::fmt::Display for JarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            JarError::Io(e) => write!(f, "{e}"),
            JarError::Zip(e) => write!(f, "{e}"),
        }
    }
}

/// Reading the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Io {
    /// Unbuffered file (old path: two system calls per header).
    File,
    /// Jar read in one block (jar size in memory for the duration of the read).
    Read,
    /// Memory-mapped jar (system cache pages, no extra private memory).
    Mmap,
}

/// Settings of a read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Opts {
    pub io: Io,
    /// Sort by name before opening the entry (otherwise each entry is opened, as before).
    pub filter: bool,
    /// Decompression threads (1: on the calling thread). Ignored with `Io::File`.
    pub threads: usize,
}

fn opt(name: &str, default: bool) -> bool {
    match crate::env_flags::read_os(name).as_deref().and_then(std::ffi::OsStr::to_str) {
        Some("0") => false,
        Some("1") => true,
        _ => default,
    }
}

/// Shared index in service (default); `RUSTCRAFT_OPT_JARINDEX=0`: each reader re-reads the jar.
pub fn enabled() -> bool {
    static C: OnceLock<bool> = OnceLock::new();
    *C.get_or_init(|| opt("RUSTCRAFT_OPT_JARINDEX", true))
}

/// Process settings (switches read once).
pub fn opts() -> Opts {
    static C: OnceLock<Opts> = OnceLock::new();
    *C.get_or_init(|| {
        let io = match crate::env_flags::read_os("RUSTCRAFT_OPT_JARIO").as_deref().and_then(std::ffi::OsStr::to_str) {
            Some("0" | "file") => Io::File,
            Some("mmap") => Io::Mmap,
            Some("read") => Io::Read,
            _ => DEFAULT_IO,
        };
        let par = opt("RUSTCRAFT_OPT_JARPAR", DEFAULT_PAR);
        Opts { io, filter: opt("RUSTCRAFT_OPT_JARFILTER", true), threads: if par { rayon::current_num_threads() } else { 1 } }
    })
}

/// Default I/O mode (micro-bench `examples/jar.rs`, see `docs/perf.md`).
const DEFAULT_IO: Io = Io::Mmap;
/// Default parallel decompression.
const DEFAULT_PAR: bool = true;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Key {
    path: PathBuf,
    len: u64,
    modified: Option<std::time::SystemTime>,
}

fn key(path: &Path) -> Result<Key, JarError> {
    let m = std::fs::metadata(path).map_err(JarError::Io)?;
    let path = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    Ok(Key { path, len: m.len(), modified: m.modified().ok() })
}

/// Jar index for this process: built on first call (settings from [`opts`]), then
/// shared. A failure is not kept (the next call retries).
pub fn index(path: &Path) -> Result<Arc<JarIndex>, JarError> {
    static CACHE: Mutex<Vec<(Key, Arc<JarIndex>)>> = Mutex::new(Vec::new());
    let k = key(path)?;
    // The lock is held during construction: two threads opening the same jar at the same
    // time (generator and main thread) do only one read.
    let mut c = CACHE.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((_, i)) = c.iter().find(|(kk, _)| *kk == k) {
        return Ok(i.clone());
    }
    let t = std::time::Instant::now();
    let i = Arc::new(build(path, opts())?);
    if crate::env_flag!("RUSTCRAFT_JARLOG") {
        eprintln!(
            "jar: index of {} entries ({:.1} MB) in {:.1} ms",
            i.files.len(),
            i.bytes_len() as f64 / 1e6,
            t.elapsed().as_secs_f64() * 1e3
        );
    }
    c.retain(|(kk, _)| kk.path != k.path);
    c.push((k, i.clone()));
    Ok(i)
}

/// Reading the jar's bytes through a memory mapping.
#[derive(Clone)]
struct Mapped(Arc<memmap2::Mmap>);

impl AsRef<[u8]> for Mapped {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

#[allow(unsafe_code)]
fn map(f: &std::fs::File) -> std::io::Result<memmap2::Mmap> {
    // SAFETY: the jar of an installed version is never rewritten in place (the launcher writes a
    // new file then renames it); a modification during the read would return
    // inconsistent bytes, which the CRC check of each entry reports as an unreadable entry.
    unsafe { memmap2::Mmap::map(f) }
}

/// Builds the index of a jar (no cache).
pub fn build(path: &Path, o: Opts) -> Result<JarIndex, JarError> {
    let zip_err = |e: zip::result::ZipError| JarError::Zip(e.to_string());
    match o.io {
        Io::File => {
            let f = std::fs::File::open(path).map_err(JarError::Io)?;
            let mut z = zip::ZipArchive::new(f).map_err(zip_err)?;
            Ok(serial(&mut z, o.filter))
        }
        Io::Read => {
            let b: Arc<[u8]> = std::fs::read(path).map_err(JarError::Io)?.into();
            let z = zip::ZipArchive::new(std::io::Cursor::new(b)).map_err(zip_err)?;
            Ok(parallel(z, o))
        }
        Io::Mmap => {
            let f = std::fs::File::open(path).map_err(JarError::Io)?;
            let m = Mapped(Arc::new(map(&f).map_err(JarError::Io)?));
            let z = zip::ZipArchive::new(std::io::Cursor::new(m)).map_err(zip_err)?;
            Ok(parallel(z, o))
        }
    }
}

fn read_entry<R: Read + Seek>(z: &mut zip::ZipArchive<R>, i: usize) -> Result<Vec<u8>, String> {
    let mut e = z.by_index(i).map_err(|e| e.to_string())?;
    let mut v = Vec::with_capacity(e.size() as usize);
    e.read_to_end(&mut v).map_err(|e| e.to_string())?;
    Ok(v)
}

/// Reading on the calling thread, in archive order.
fn serial<R: Read + Seek>(z: &mut zip::ZipArchive<R>, filter: bool) -> JarIndex {
    let mut out = JarIndex::default();
    for i in 0..z.len() {
        if !filter {
            // Old order: entry opened (local header read) then name examined.
            if let Err(e) = z.by_index(i) {
                out.broken.get_or_insert_with(|| format!("entry {i}: {e}"));
                continue;
            }
        }
        let Some(Ok(name)) = z.name_for_index(i) else { continue };
        if !retained(&name) {
            continue;
        }
        let name = name.into_owned();
        match read_entry(z, i) {
            Ok(v) => {
                out.files.insert(name, Blob::new(v));
            }
            Err(e) => {
                out.failed.insert(name, e);
            }
        }
    }
    out
}

/// Retained entries split into contiguous slices, one cloned archive (shared central directory)
/// per slice; results stored by name, identical to the serial read.
fn parallel<R: Read + Seek + Clone + Send + Sync>(mut z: zip::ZipArchive<R>, o: Opts) -> JarIndex {
    if o.threads <= 1 || !o.filter {
        return serial(&mut z, o.filter);
    }
    use rayon::prelude::*;
    let picked: Vec<(usize, String)> = (0..z.len())
        .filter_map(|i| match z.name_for_index(i) {
            Some(Ok(n)) if retained(&n) => Some((i, n.into_owned())),
            _ => None,
        })
        .collect();
    // Slices of at least 256 entries: beyond that, waking more threads costs more than it
    // gains (micro-bench, loaded machine: 16 and 32 threads slower than 4 to 8).
    let chunk = picked.len().div_ceil(o.threads * 4).max(256);
    type Part = Vec<(String, Result<Vec<u8>, String>)>;
    let parts: Vec<Part> = picked
        .par_chunks(chunk)
        .map(|c| {
            let mut z = z.clone();
            c.iter().map(|(i, n)| (n.clone(), read_entry(&mut z, *i))).collect()
        })
        .collect();
    let mut out = JarIndex::default();
    for (name, r) in parts.into_iter().flatten() {
        match r {
            Ok(v) => {
                out.files.insert(name, Blob::new(v));
            }
            Err(e) => {
                out.failed.insert(name, e);
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Client jar of the target version; absent: test ignored, except with `RUSTCRAFT_OPT_REQUIRE=1`.
    fn jar() -> Option<PathBuf> {
        let p = crate::locate("26.3").ok().map(|i| i.client_jar).filter(|p| p.is_file());
        if p.is_none() && crate::env_str!("RUSTCRAFT_OPT_REQUIRE") == Some("1") {
            panic!("26.3 client jar not found (RUSTCRAFT_OPT_REQUIRE=1)");
        }
        p
    }

    /// Reference reader, independent of the module: each entry opened by its name.
    fn reference(path: &Path) -> BTreeMap<String, Vec<u8>> {
        let mut z = zip::ZipArchive::new(std::fs::File::open(path).unwrap()).unwrap();
        let names: Vec<String> = z.file_names().map(|n| n.unwrap().into_owned()).collect();
        let mut out = BTreeMap::new();
        for n in names {
            if n.starts_with("data/") || n == EN_US {
                let mut v = Vec::new();
                z.by_name(&n).unwrap().read_to_end(&mut v).unwrap();
                out.insert(n, v);
            }
        }
        out
    }

    fn digest(i: &JarIndex) -> String {
        use sha1::Digest;
        let mut h = sha1::Sha1::new();
        for (k, v) in &i.files {
            h.update(k.as_bytes());
            h.update([0]);
            h.update((v.as_bytes().len() as u64).to_le_bytes());
            h.update(v.as_bytes());
        }
        h.finalize().iter().map(|b| format!("{b:02x}")).collect()
    }

    /// Expected fingerprint (sorted names + bytes) of the retained entries of the 26.3 jar, and their count.
    const EXPECTED: (&str, usize) = ("f2fe5517d58d220e2cb97d161bd88768e8280336", 10450);

    #[test]
    fn all_modes_give_the_same_index() {
        let Some(p) = jar() else { return };
        let want = reference(&p);
        let modes = [
            Opts { io: Io::File, filter: false, threads: 1 },
            Opts { io: Io::File, filter: true, threads: 1 },
            Opts { io: Io::Read, filter: true, threads: 1 },
            Opts { io: Io::Read, filter: true, threads: 8 },
            Opts { io: Io::Read, filter: false, threads: 8 },
            Opts { io: Io::Mmap, filter: true, threads: 1 },
            Opts { io: Io::Mmap, filter: true, threads: 8 },
        ];
        let first = build(&p, modes[0]).unwrap();
        for o in modes {
            let i = build(&p, o).unwrap();
            assert!(i.failed.is_empty() && i.broken.is_none(), "{o:?}: unreadable entries {:?}", i.failed);
            let got: BTreeMap<String, Vec<u8>> =
                i.files.iter().map(|(k, v)| (k.clone(), v.as_bytes().to_vec())).collect();
            assert_eq!(got.len(), want.len(), "{o:?}: number of entries");
            assert!(got == want, "{o:?}: content differs from the reference reader");
            assert_eq!(i, first, "{o:?}: index differs (text / binary)");
        }
        let d = digest(&first);
        eprintln!("jar index: fingerprint {d} ({} entries)", first.files.len());
        assert!(
            (d.as_str(), first.files.len()) == EXPECTED,
            "fingerprint {d} ({}) instead of {EXPECTED:?}: record it in jar.rs if the jar changed",
            first.files.len()
        );
    }

    #[test]
    fn shared_index_and_range() {
        let Some(p) = jar() else { return };
        let a = index(&p).unwrap();
        let b = index(&p).unwrap();
        assert!(Arc::ptr_eq(&a, &b), "the jar is re-read on the second call");
        let pre = "data/minecraft/worldgen/noise/";
        let by_range: Vec<&String> = a.with_prefix(pre).map(|(k, _)| k).collect();
        let by_scan: Vec<&String> = a.files.keys().filter(|k| k.starts_with(pre)).collect();
        assert!(!by_scan.is_empty());
        assert_eq!(by_range, by_scan);
        assert!(a.get(EN_US).and_then(Blob::text).is_some(), "English language absent");
        assert!(a.files.values().any(|b| matches!(b, Blob::Bin(_))), "no binary template");
    }

    #[test]
    fn missing_jar_io_error() {
        let e = index(Path::new("missing.jar")).unwrap_err();
        assert!(matches!(e, JarError::Io(_)));
    }
}
