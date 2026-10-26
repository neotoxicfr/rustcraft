//! Disk cache of baked resources: extracted once, then read back without opening the jar.
//!
//! One file per key in the cache folder (`assets-<key>.bin`). The key covers the version,
//! each resource source (path, size, modification date, content never read),
//! the executable and the version data (baking depends on it) and `FORMAT`.
//!
//! Split cache (`RUSTCRAFT_OPT_CACHESPLIT`, default): the file snapshot goes into
//! `files-<key>.bin`, whose key covers only the sources, `FORMAT` and the code the
//! snapshot depends on, this module included (`SNAP_CODE`); the sections go into `assets-<key>.bin` (full key). A rebuild
//! only re-bakes and rewrites the sections.
//!
//! Content: named sections (baked structures, home-made binary codec) and a snapshot of
//! files (`files`): what startup and lazy loads read outside baking,
//! PNGs already decoded to RGBA, absences included, plus the requested directory listings. Only
//! the header, the section table and the file index are kept in memory; each
//! section is read in one block when used, each file by positioned read.
//!
//! Atomic write: temporary file private to the process, then rename.

use hashbrown::HashMap;
use std::hash::Hash;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use crate::texture::Image;

/// Format version: change it when a codec or the content changes shape.
pub const FORMAT: u32 = 5;

const MAGIC: &[u8; 4] = b"RCAC";

/// Cache files kept in the folder (different executables side by side).
const KEEP: usize = 3;

/// Read large enough for a separate descriptor without zeroing (sections, large images).
const BIG_READ: u64 = 1 << 20;

// ---------------------------------------------------------------------------------------------
// Codec

/// Write buffer.
#[derive(Default)]
pub struct W {
    pub buf: Vec<u8>,
}

impl W {
    pub fn bytes(&mut self, b: &[u8]) {
        (b.len() as u64).enc(self);
        self.buf.extend_from_slice(b);
    }
}

/// Bounded reader; any out-of-bounds read returns `None`.
pub struct R<'a> {
    b: &'a [u8],
    pos: usize,
}

impl<'a> R<'a> {
    pub fn new(b: &'a [u8]) -> Self {
        R { b, pos: 0 }
    }

    pub fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        let s = self.b.get(self.pos..self.pos.checked_add(n)?)?;
        self.pos += n;
        Some(s)
    }

    pub fn bytes(&mut self) -> Option<&'a [u8]> {
        let n = usize::try_from(u64::dec(self)?).ok()?;
        self.take(n)
    }

    /// List length, bounded by the remaining bytes (a corrupted file does not allocate everything).
    pub fn len(&mut self) -> Option<usize> {
        let n = usize::try_from(u64::dec(self)?).ok()?;
        (n <= self.b.len() - self.pos).then_some(n)
    }

    pub fn is_empty(&self) -> bool {
        self.pos >= self.b.len()
    }
}

// ---------------------------------------------------------------------------------------------
// Optimisation switches (`RUSTCRAFT_OPT_<NAME>=0`: old path), read once.

/// New (true) or old (false) cache paths, read once on first use.
#[derive(Clone, Copy, Debug)]
pub struct Opt {
    /// `[T; N]` decoded without an intermediate array on the heap.
    pub codec_array: bool,
    /// Numbers decoded in bulk (arrays and lists) instead of one by one.
    pub codec_bulk: bool,
    /// Sections read without zeroing the buffer beforehand.
    pub cache_read: bool,
    /// Baking a section: reads kept out of the snapshot by a view private to the computation
    /// (instead of a global counter that also hides the reads of other threads).
    pub record_local: bool,
    /// File snapshot and baked sections in two files, distinct keys.
    pub cache_split: bool,
    /// Streamed write: table then each blob, without a buffer holding all the blobs.
    pub cache_stream: bool,
    /// A cache's date refreshed when it is opened (cleanup keeps the most recently served).
    pub cache_lru: bool,
    /// Write thread at normal priority, images decoded on a few threads.
    pub cache_lowprio: bool,
}

/// Switch active unless `=0`, one read per site (`env_os!` wants a literal name).
macro_rules! opt_env {
    ($name:literal) => {
        rc_data::env_os!($name).is_none_or(|v| v != "0")
    };
}

impl Opt {
    pub fn get() -> Opt {
        static F: std::sync::OnceLock<Opt> = std::sync::OnceLock::new();
        *F.get_or_init(|| Opt {
            codec_array: opt_env!("RUSTCRAFT_OPT_CODECARRAY"),
            codec_bulk: opt_env!("RUSTCRAFT_OPT_CODECBULK"),
            cache_read: opt_env!("RUSTCRAFT_OPT_CACHEREAD"),
            record_local: opt_env!("RUSTCRAFT_OPT_RECORDLOCAL"),
            cache_split: opt_env!("RUSTCRAFT_OPT_CACHESPLIT"),
            cache_stream: opt_env!("RUSTCRAFT_OPT_CACHESTREAM"),
            cache_lru: opt_env!("RUSTCRAFT_OPT_CACHELRU"),
            cache_lowprio: opt_env!("RUSTCRAFT_OPT_CACHELOWPRIO"),
        })
    }
}

#[cfg(test)]
thread_local! {
    /// Tests: codec (arrays, block) forced on this thread.
    static CODEC_TEST: std::cell::Cell<Option<(bool, bool)>> = const { std::cell::Cell::new(None) };
}

#[inline]
fn codec_array() -> bool {
    #[cfg(test)]
    if let Some((a, _)) = CODEC_TEST.with(std::cell::Cell::get) {
        return a;
    }
    Opt::get().codec_array
}

#[inline]
fn codec_bulk() -> bool {
    #[cfg(test)]
    if let Some((_, b)) = CODEC_TEST.with(std::cell::Cell::get) {
        return b;
    }
    Opt::get().codec_bulk
}

/// Binary encoding of a baked structure.
pub trait Codec: Sized {
    fn enc(&self, w: &mut W);
    fn dec(r: &mut R<'_>) -> Option<Self>;

    /// `n` consecutive values (list). Overridden by the numbers (bulk decoding).
    fn dec_vec(r: &mut R<'_>, n: usize) -> Option<Vec<Self>> {
        dec_vec_each(r, n)
    }

    /// `N` consecutive values (array). Overridden by the numbers (bulk decoding).
    fn dec_arr<const N: usize>(r: &mut R<'_>) -> Option<[Self; N]> {
        if codec_array() { dec_arr_each(r) } else { dec_arr_heap(r) }
    }
}

/// List element by element.
fn dec_vec_each<T: Codec>(r: &mut R<'_>, n: usize) -> Option<Vec<T>> {
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(T::dec(r)?);
    }
    Some(out)
}

/// Array without going through the heap: reading stops at the first failure.
fn dec_arr_each<T: Codec, const N: usize>(r: &mut R<'_>) -> Option<[T; N]> {
    let mut ok = true;
    let a: [Option<T>; N] = std::array::from_fn(|_| {
        let v = if ok { T::dec(r) } else { None };
        ok = v.is_some();
        v
    });
    // `ok`: every slot is filled (the first failure makes `ok` false for good).
    ok.then(|| a.map(Option::unwrap))
}

/// Old path: intermediate array on the heap (`RUSTCRAFT_OPT_CODECARRAY=0`).
fn dec_arr_heap<T: Codec, const N: usize>(r: &mut R<'_>) -> Option<[T; N]> {
    let mut v = Vec::with_capacity(N);
    for _ in 0..N {
        v.push(T::dec(r)?);
    }
    v.try_into().ok()
}

macro_rules! num {
    ($($t:ty),*) => {$(
        impl Codec for $t {
            fn enc(&self, w: &mut W) {
                w.buf.extend_from_slice(&self.to_le_bytes());
            }
            fn dec(r: &mut R<'_>) -> Option<Self> {
                Some(<$t>::from_le_bytes(r.take(size_of::<$t>())?.try_into().ok()?))
            }
            fn dec_vec(r: &mut R<'_>, n: usize) -> Option<Vec<Self>> {
                if !codec_bulk() {
                    return dec_vec_each(r, n);
                }
                const SZ: usize = size_of::<$t>();
                let b = r.take(n.checked_mul(SZ)?)?;
                Some(b.chunks_exact(SZ).map(|c| <[u8; SZ]>::try_from(c).map_or(<$t>::default(), <$t>::from_le_bytes)).collect())
            }
            fn dec_arr<const N: usize>(r: &mut R<'_>) -> Option<[Self; N]> {
                if !codec_bulk() {
                    return if codec_array() { dec_arr_each(r) } else { dec_arr_heap(r) };
                }
                const SZ: usize = size_of::<$t>();
                let b = r.take(N.checked_mul(SZ)?)?;
                let mut it = b.chunks_exact(SZ);
                Some(std::array::from_fn(|_| {
                    it.next().and_then(|c| <[u8; SZ]>::try_from(c).ok()).map_or(<$t>::default(), <$t>::from_le_bytes)
                }))
            }
        }
    )*};
}
num!(u8, u16, u32, u64, i8, i16, i32, i64, f32, f64);

impl Codec for usize {
    fn enc(&self, w: &mut W) {
        (*self as u64).enc(w);
    }
    fn dec(r: &mut R<'_>) -> Option<Self> {
        usize::try_from(u64::dec(r)?).ok()
    }
}

impl Codec for bool {
    fn enc(&self, w: &mut W) {
        u8::from(*self).enc(w);
    }
    fn dec(r: &mut R<'_>) -> Option<Self> {
        match u8::dec(r)? {
            0 => Some(false),
            1 => Some(true),
            _ => None,
        }
    }
}

impl Codec for String {
    fn enc(&self, w: &mut W) {
        w.bytes(self.as_bytes());
    }
    fn dec(r: &mut R<'_>) -> Option<Self> {
        String::from_utf8(r.bytes()?.to_vec()).ok()
    }
}

impl<T: Codec> Codec for Vec<T> {
    fn enc(&self, w: &mut W) {
        self.len().enc(w);
        for v in self {
            v.enc(w);
        }
    }
    fn dec(r: &mut R<'_>) -> Option<Self> {
        let n = r.len()?;
        T::dec_vec(r, n)
    }
}

impl<T: Codec> Codec for Option<T> {
    fn enc(&self, w: &mut W) {
        match self {
            None => 0u8.enc(w),
            Some(v) => {
                1u8.enc(w);
                v.enc(w);
            }
        }
    }
    fn dec(r: &mut R<'_>) -> Option<Self> {
        match u8::dec(r)? {
            0 => Some(None),
            1 => Some(Some(T::dec(r)?)),
            _ => None,
        }
    }
}

impl<T: Codec> Codec for Box<T> {
    fn enc(&self, w: &mut W) {
        (**self).enc(w);
    }
    fn dec(r: &mut R<'_>) -> Option<Self> {
        T::dec(r).map(Box::new)
    }
}

impl<T: Codec> Codec for Arc<T> {
    fn enc(&self, w: &mut W) {
        (**self).enc(w);
    }
    fn dec(r: &mut R<'_>) -> Option<Self> {
        T::dec(r).map(Arc::new)
    }
}

impl<T: Codec, const N: usize> Codec for [T; N] {
    fn enc(&self, w: &mut W) {
        for v in self {
            v.enc(w);
        }
    }
    fn dec(r: &mut R<'_>) -> Option<Self> {
        T::dec_arr::<N>(r)
    }
}

macro_rules! tuple {
    ($($n:ident),*) => {
        impl<$($n: Codec),*> Codec for ($($n,)*) {
            #[allow(non_snake_case)]
            fn enc(&self, w: &mut W) {
                let ($($n,)*) = self;
                $($n.enc(w);)*
            }
            fn dec(r: &mut R<'_>) -> Option<Self> {
                Some(($($n::dec(r)?,)*))
            }
        }
    };
}
tuple!(A, B);
tuple!(A, B, C);
tuple!(A, B, C, D);
tuple!(A, B, C, D, E);

impl<K: Codec + Eq + Hash, V: Codec, S: std::hash::BuildHasher + Default> Codec for HashMap<K, V, S> {
    /// Entries sorted by encoded key: same table, same bytes (hot/cold comparison).
    fn enc(&self, w: &mut W) {
        self.len().enc(w);
        let mut keyed: Vec<(Vec<u8>, &V)> = self.iter().map(|(k, v)| (encode(k), v)).collect();
        keyed.sort_unstable_by(|a, b| a.0.cmp(&b.0));
        for (k, v) in keyed {
            w.buf.extend_from_slice(&k);
            v.enc(w);
        }
    }
    fn dec(r: &mut R<'_>) -> Option<Self> {
        let n = r.len()?;
        let mut out = HashMap::with_capacity_and_hasher(n, S::default());
        for _ in 0..n {
            let k = K::dec(r)?;
            out.insert(k, V::dec(r)?);
        }
        Some(out)
    }
}

/// Image: dimensions then pixels in one block.
impl Codec for Image {
    fn enc(&self, w: &mut W) {
        self.width.enc(w);
        self.height.enc(w);
        w.bytes(&self.rgba);
    }
    fn dec(r: &mut R<'_>) -> Option<Self> {
        let (width, height) = (u32::dec(r)?, u32::dec(r)?);
        let rgba = r.bytes()?;
        (rgba.len() as u64 == u64::from(width) * u64::from(height) * 4).then(|| Image {
            width,
            height,
            rgba: rgba.to_vec(),
        })
    }
}

/// Field-by-field codec of a structure (to invoke in its module if the fields are private).
#[macro_export]
macro_rules! codec_struct {
    ($t:ty { $($f:ident),* $(,)? }) => {
        impl $crate::cache::Codec for $t {
            fn enc(&self, w: &mut $crate::cache::W) {
                $($crate::cache::Codec::enc(&self.$f, w);)*
            }
            fn dec(r: &mut $crate::cache::R<'_>) -> Option<Self> {
                Some(Self { $($f: $crate::cache::Codec::dec(r)?,)* })
            }
        }
    };
}

/// Codec of an enumeration without data (rank of the variant).
#[macro_export]
macro_rules! codec_enum {
    ($t:ident { $($v:ident),* $(,)? }) => {
        impl $crate::cache::Codec for $t {
            fn enc(&self, w: &mut $crate::cache::W) {
                let all = [$($t::$v),*];
                let i = all.iter().position(|x| x == self).unwrap_or(0) as u8;
                $crate::cache::Codec::enc(&i, w);
            }
            fn dec(r: &mut $crate::cache::R<'_>) -> Option<Self> {
                let all = [$($t::$v),*];
                all.get(usize::from(<u8 as $crate::cache::Codec>::dec(r)?)).copied()
            }
        }
    };
}

/// Encodes a single value.
pub fn encode<T: Codec>(v: &T) -> Vec<u8> {
    let mut w = W::default();
    v.enc(&mut w);
    w.buf
}

/// Decodes a value that must fill the whole buffer.
pub fn decode<T: Codec>(b: &[u8]) -> Option<T> {
    let mut r = R::new(b);
    let v = T::dec(&mut r)?;
    r.is_empty().then_some(v)
}

// ---------------------------------------------------------------------------------------------
// Trace of source accesses (`RUSTCRAFT_ASSETLOG=1`)

pub(crate) fn trace_on() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| rc_data::env_os!("RUSTCRAFT_ASSETLOG").is_some_and(|v| !v.is_empty() && v != "0"))
}

pub(crate) fn trace(what: std::fmt::Arguments<'_>) {
    if trace_on() {
        eprintln!("[assetlog] {what}");
    }
}

// ---------------------------------------------------------------------------------------------
// Key

/// FNV-1a 64-bit fingerprint.
pub fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |h, &b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3))
}

/// Identity of a file without reading its content: path, size, modification date (ns).
/// A directory (unpacked pack): each file, sorted.
pub fn identity(path: &Path, out: &mut String) {
    use std::fmt::Write;
    let Ok(meta) = std::fs::metadata(path) else {
        let _ = writeln!(out, "{}|absent", path.display());
        return;
    };
    if meta.is_dir() {
        let mut files = Vec::new();
        list_dir(path, &mut files);
        files.sort();
        let _ = writeln!(out, "{}|folder {}", path.display(), files.len());
        for f in files {
            identity(&f, out);
        }
        return;
    }
    // File named by its SHA-1 (server pack in `downloads/`): content fixed by the name,
    // the date (refreshed at each reuse) does not count.
    let named_by_hash = path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.len() == 40 && n.bytes().all(|b| b.is_ascii_hexdigit()));
    if named_by_hash {
        let _ = writeln!(out, "{}|{}|sha1", path.display(), meta.len());
        return;
    }
    let mtime =
        meta.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_nanos());
    let _ = writeln!(out, "{}|{}|{mtime}", path.display(), meta.len());
}

fn list_dir(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            list_dir(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// Key text: version, format, then the identity of each file in the given order
/// (resource sources by priority, then executable and version data).
pub fn key_text(version: &str, files: &[PathBuf]) -> String {
    let mut s = format!("rustcraft assets {version} format {FORMAT}\n");
    for f in files {
        identity(f, &mut s);
    }
    s
}

/// Key of the baked sections: `key_text` plus the switches that decide the written shape
/// (item models). Without them, relaunching with a switch at 0 would read back the shape of the
/// previous session instead of the old path.
pub fn sections_key_text(version: &str, files: &[PathBuf]) -> String {
    key_text(version, files) + &crate::item::Opt::get().cache_tag()
}

// ---------------------------------------------------------------------------------------------
// File snapshot

/// Snapshot entry in memory (write).
pub(crate) enum Snap {
    Raw(Vec<u8>),
    Image(Image),
    Absent,
}

/// Snapshot entry on disk: kind, position, length, image dimensions.
#[derive(Clone, Copy)]
struct Loc {
    kind: u8,
    off: u64,
    len: u64,
    w: u32,
    h: u32,
}

const K_RAW: u8 = 0;
const K_DEFLATE: u8 = 1;
const K_IMAGE: u8 = 2;
const K_ABSENT: u8 = 3;

/// Snapshot answer for a path.
pub(crate) enum Hit<T> {
    Found(T),
    Absent,
    Unknown,
}

/// Recording of reads (cold launch): what will go into the snapshot.
#[derive(Default)]
pub(crate) struct Recorder {
    pub(crate) files: HashMap<String, Snap>,
    pub(crate) lists: HashMap<(String, String), Vec<String>>,
}

/// Open cache file (hot launch).
pub struct Store {
    file: std::fs::File,
    path: PathBuf,
    sections: HashMap<String, (u64, u64)>,
    files: HashMap<String, Loc>,
    lists: HashMap<(String, String), Vec<String>>,
}

#[cfg(windows)]
fn read_at(f: &std::fs::File, buf: &mut [u8], at: u64) -> std::io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(f, buf, at)
}

#[cfg(unix)]
fn read_at(f: &std::fs::File, buf: &mut [u8], at: u64) -> std::io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(f, buf, at)
}

fn read_exact_at(f: &std::fs::File, buf: &mut [u8], mut at: u64) -> std::io::Result<()> {
    let mut done = 0;
    while done < buf.len() {
        let n = read_at(f, &mut buf[done..], at)?;
        if n == 0 {
            return Err(std::io::ErrorKind::UnexpectedEof.into());
        }
        done += n;
        at += n as u64;
    }
    Ok(())
}

/// Positioned read into a raw pointer (no reference to unwritten memory).
/// Returns the number of bytes read, 0 at end of file.
#[cfg(windows)]
#[allow(unsafe_code)]
unsafe fn raw_read_at(f: &std::fs::File, ptr: *mut u8, len: usize, at: u64) -> std::io::Result<usize> {
    use std::os::windows::io::AsRawHandle;
    #[repr(C)]
    struct Overlapped {
        internal: usize,
        internal_high: usize,
        offset: u32,
        offset_high: u32,
        event: *mut std::ffi::c_void,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn ReadFile(h: *mut std::ffi::c_void, buf: *mut u8, n: u32, read: *mut u32, ov: *mut Overlapped) -> i32;
    }
    let mut ov = Overlapped {
        internal: 0,
        internal_high: 0,
        offset: at as u32,
        offset_high: (at >> 32) as u32,
        event: std::ptr::null_mut(),
    };
    let mut read = 0u32;
    let n = u32::try_from(len).unwrap_or(u32::MAX);
    // SAFETY: valid descriptor (borrowed from `f`), `ptr` is valid for `n` bytes (guaranteed by
    // the caller), OVERLAPPED alive during the call (synchronous descriptor: blocking call).
    let ok = unsafe { ReadFile(f.as_raw_handle(), ptr, n, &mut read, &mut ov) };
    if ok == 0 {
        let e = std::io::Error::last_os_error();
        // ERROR_HANDLE_EOF: read past the end.
        return if e.raw_os_error() == Some(38) { Ok(0) } else { Err(e) };
    }
    Ok(read as usize)
}

#[cfg(unix)]
#[allow(unsafe_code)]
unsafe fn raw_read_at(f: &std::fs::File, ptr: *mut u8, len: usize, at: u64) -> std::io::Result<usize> {
    use std::os::fd::AsRawFd;
    unsafe extern "C" {
        fn pread(fd: i32, buf: *mut std::ffi::c_void, n: usize, off: i64) -> isize;
    }
    let off = i64::try_from(at).map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?;
    // SAFETY: valid descriptor (borrowed from `f`), `ptr` is valid for `len` bytes (guaranteed by the caller).
    let r = unsafe { pread(f.as_raw_fd(), ptr.cast(), len.min(isize::MAX as usize), off) };
    if r < 0 { Err(std::io::Error::last_os_error()) } else { Ok(r as usize) }
}

/// `n` bytes read at `at` into the reserved capacity of a fresh buffer, without zeroing beforehand.
/// Always through the descriptor kept open: never a file replaced since under the same name.
#[allow(unsafe_code)]
fn read_exact_at_uninit(f: &std::fs::File, n: usize, at: u64) -> std::io::Result<Vec<u8>> {
    let mut buf: Vec<u8> = Vec::with_capacity(n);
    let mut done = 0;
    while done < n {
        // SAFETY: `done < n <= capacity`, the window [done, n) is inside the allocation and is
        // only written by the system.
        let got = match unsafe { raw_read_at(f, buf.as_mut_ptr().add(done), n - done, at + done as u64) } {
            Ok(0) => return Err(std::io::ErrorKind::UnexpectedEof.into()),
            Ok(g) => g,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        done += got.min(n - done);
    }
    // SAFETY: the first `n` bytes were written by the reads above.
    unsafe { buf.set_len(n) };
    Ok(buf)
}

impl Store {
    /// Opens the cache if it exists and its key matches.
    fn open(path: &Path, key: u64) -> Option<Store> {
        let file = std::fs::File::open(path).ok()?;
        let len = file.metadata().ok()?.len();
        let mut head = [0u8; 24];
        read_exact_at(&file, &mut head, 0).ok()?;
        let mut r = R::new(&head);
        if r.take(4)? != MAGIC || u32::dec(&mut r)? != FORMAT || u64::dec(&mut r)? != key {
            return None;
        }
        let toc_len = u64::dec(&mut r)?;
        if toc_len > len {
            return None;
        }
        let mut toc = vec![0u8; usize::try_from(toc_len).ok()?];
        read_exact_at(&file, &mut toc, 24).ok()?;
        let mut r = R::new(&toc);
        let base = 24 + toc_len;
        let mut sections = HashMap::new();
        for _ in 0..r.len()? {
            let name = String::dec(&mut r)?;
            let (off, n) = (u64::dec(&mut r)?, u64::dec(&mut r)?);
            if base + off + n > len {
                return None;
            }
            sections.insert(name, (base + off, n));
        }
        let mut files = HashMap::new();
        for _ in 0..r.len()? {
            let p = String::dec(&mut r)?;
            let loc = Loc {
                kind: u8::dec(&mut r)?,
                off: base + u64::dec(&mut r)?,
                len: u64::dec(&mut r)?,
                w: u32::dec(&mut r)?,
                h: u32::dec(&mut r)?,
            };
            if loc.off + loc.len > len {
                return None;
            }
            files.insert(p, loc);
        }
        let lists = HashMap::dec(&mut r)?;
        Some(Store { file, path: path.to_owned(), sections, files, lists })
    }

    fn read_range(&self, off: u64, len: u64) -> Option<Vec<u8>> {
        if len >= BIG_READ && Opt::get().cache_read {
            return self.read_range_uninit(off, len);
        }
        self.read_range_zeroed(off, len)
    }

    /// Old path: buffer zeroed then filled (`RUSTCRAFT_OPT_CACHEREAD=0`, small reads).
    fn read_range_zeroed(&self, off: u64, len: u64) -> Option<Vec<u8>> {
        let mut buf = vec![0u8; usize::try_from(len).ok()?];
        read_exact_at(&self.file, &mut buf, off).ok()?;
        Some(buf)
    }

    /// Large read without zeroing, positioned on the descriptor opened by `open`: a
    /// file rewritten since under the same name (the split snapshot can change table under the
    /// same key) cannot be read at the old offsets.
    fn read_range_uninit(&self, off: u64, len: u64) -> Option<Vec<u8>> {
        read_exact_at_uninit(&self.file, usize::try_from(len).ok()?, off).ok()
    }

    /// Bytes of a section, read in one block.
    pub fn section(&self, name: &str) -> Option<Vec<u8>> {
        let &(off, len) = self.sections.get(name)?;
        self.read_range(off, len)
    }

    pub(crate) fn read(&self, path: &str) -> Hit<Vec<u8>> {
        let Some(l) = self.files.get(path) else { return Hit::Unknown };
        let got = match l.kind {
            K_ABSENT => return Hit::Absent,
            K_RAW => self.read_range(l.off, l.len),
            K_DEFLATE => self.read_range(l.off, l.len).and_then(|z| {
                use std::io::Read;
                let mut out = Vec::new();
                flate2::read::ZlibDecoder::new(&z[..]).read_to_end(&mut out).ok().map(|_| out)
            }),
            _ => return Hit::Unknown,
        };
        got.map_or(Hit::Unknown, Hit::Found)
    }

    pub(crate) fn image(&self, path: &str) -> Hit<Image> {
        let Some(l) = self.files.get(path) else { return Hit::Unknown };
        match l.kind {
            K_ABSENT => Hit::Absent,
            K_IMAGE => self
                .read_range(l.off, l.len)
                .filter(|b| b.len() as u64 == u64::from(l.w) * u64::from(l.h) * 4)
                .map_or(Hit::Unknown, |rgba| Hit::Found(Image { width: l.w, height: l.h, rgba })),
            _ => Hit::Unknown,
        }
    }

    pub(crate) fn list(&self, prefix: &str, suffix: &str) -> Option<Vec<String>> {
        self.lists.get(&(prefix.to_owned(), suffix.to_owned())).cloned()
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Check: each snapshot file compared with the source stack (images decoded
    /// again, absences confirmed) and each listing redone.
    fn verify_files(&self, assets: &crate::Assets) {
        let t = std::time::Instant::now();
        let mut bad = Vec::new();
        for (p, l) in &self.files {
            let fresh = assets.stack_read_key(p);
            let ok = match l.kind {
                K_ABSENT => fresh.is_none(),
                K_IMAGE => match (self.image(p), fresh.as_deref().and_then(Image::decode_png)) {
                    (Hit::Found(a), Some(b)) => a.width == b.width && a.height == b.height && a.rgba == b.rgba,
                    _ => false,
                },
                _ => matches!(self.read(p), Hit::Found(a) if Some(&a) == fresh.as_ref()),
            };
            if !ok {
                bad.push(p.clone());
            }
        }
        for ((prefix, suffix), l) in &self.lists {
            if assets.stack_list(prefix, suffix) != *l {
                bad.push(format!("list {prefix}*{suffix}"));
            }
        }
        eprintln!(
            "cache check: {} files and {} listings, {} different{}, in {:.0} ms",
            self.files.len(),
            self.lists.len(),
            bad.len(),
            bad.first().map(|b| format!(" (ex. {b})")).unwrap_or_default(),
            t.elapsed().as_secs_f64() * 1e3
        );
    }
}

/// Hot check (`RUSTCRAFT_ASSETCHECK=1`): everything is re-baked from the sources and compared.
fn check_on() -> bool {
    rc_data::env_os!("RUSTCRAFT_ASSETCHECK").is_some_and(|v| !v.is_empty() && v != "0")
}

/// Paths whose raw content is compressed in the snapshot (large, rarely read).
fn deflated(path: &str) -> bool {
    path.starts_with("assets/minecraft/lang/") && !path.ends_with("/en_us.json")
}

/// Blob of a snapshot entry: kind, bytes (compressed language files), dimensions.
fn snap_blob<'a>(path: &str, s: &'a Snap) -> std::io::Result<(u8, std::borrow::Cow<'a, [u8]>, u32, u32)> {
    use std::io::Write;
    Ok(match s {
        Snap::Absent => (K_ABSENT, (&[][..]).into(), 0, 0),
        Snap::Image(i) => (K_IMAGE, (&i.rgba[..]).into(), i.width, i.height),
        Snap::Raw(b) if deflated(path) => {
            let mut z = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::default());
            z.write_all(b)?;
            (K_DEFLATE, z.finish()?.into(), 0, 0)
        }
        Snap::Raw(b) => (K_RAW, (&b[..]).into(), 0, 0),
    })
}

/// Writes the cache file: header, table (sections, file index, listings), blobs.
fn write_file(path: &Path, key: u64, sections: &[(String, Vec<u8>)], rec: &Recorder) -> std::io::Result<u64> {
    if Opt::get().cache_stream {
        write_file_stream(path, key, sections, rec)
    } else {
        write_file_buffered(path, key, sections, rec)
    }
}

/// Old path (`RUSTCRAFT_OPT_CACHESTREAM=0`): all blobs copied into a buffer, then written.
fn write_file_buffered(path: &Path, key: u64, sections: &[(String, Vec<u8>)], rec: &Recorder) -> std::io::Result<u64> {
    use std::io::Write;
    let mut blobs: Vec<u8> = Vec::new();
    let mut toc = W::default();
    sections.len().enc(&mut toc);
    for (name, b) in sections {
        name.enc(&mut toc);
        (blobs.len() as u64).enc(&mut toc);
        (b.len() as u64).enc(&mut toc);
        blobs.extend_from_slice(b);
    }
    let mut paths: Vec<&String> = rec.files.keys().collect();
    paths.sort();
    paths.len().enc(&mut toc);
    for p in paths {
        let (kind, data, w, h) = snap_blob(p, &rec.files[p])?;
        p.enc(&mut toc);
        kind.enc(&mut toc);
        (blobs.len() as u64).enc(&mut toc);
        (data.len() as u64).enc(&mut toc);
        w.enc(&mut toc);
        h.enc(&mut toc);
        blobs.extend_from_slice(&data);
    }
    rec.lists.enc(&mut toc);
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    f.write_all(MAGIC)?;
    f.write_all(&FORMAT.to_le_bytes())?;
    f.write_all(&key.to_le_bytes())?;
    f.write_all(&(toc.buf.len() as u64).to_le_bytes())?;
    f.write_all(&toc.buf)?;
    f.write_all(&blobs)?;
    f.into_inner().map_err(std::io::IntoInnerError::into_error)?.sync_all()?;
    Ok(24 + toc.buf.len() as u64 + blobs.len() as u64)
}

/// Streamed write: the table first (known lengths, only the compressed language files are
/// computed beforehand), then each blob as is from its place in memory. Same bytes.
fn write_file_stream(path: &Path, key: u64, sections: &[(String, Vec<u8>)], rec: &Recorder) -> std::io::Result<u64> {
    use std::io::Write;
    let mut toc = W::default();
    let mut off = 0u64;
    sections.len().enc(&mut toc);
    for (name, b) in sections {
        name.enc(&mut toc);
        off.enc(&mut toc);
        (b.len() as u64).enc(&mut toc);
        off += b.len() as u64;
    }
    let mut paths: Vec<&String> = rec.files.keys().collect();
    paths.sort();
    let blobs = paths.iter().map(|p| snap_blob(p, &rec.files[*p])).collect::<std::io::Result<Vec<_>>>()?;
    paths.len().enc(&mut toc);
    for (p, (kind, data, w, h)) in paths.iter().zip(&blobs) {
        p.enc(&mut toc);
        kind.enc(&mut toc);
        off.enc(&mut toc);
        (data.len() as u64).enc(&mut toc);
        w.enc(&mut toc);
        h.enc(&mut toc);
        off += data.len() as u64;
    }
    rec.lists.enc(&mut toc);
    let mut f = std::io::BufWriter::with_capacity(1 << 20, std::fs::File::create(path)?);
    f.write_all(MAGIC)?;
    f.write_all(&FORMAT.to_le_bytes())?;
    f.write_all(&key.to_le_bytes())?;
    f.write_all(&(toc.buf.len() as u64).to_le_bytes())?;
    f.write_all(&toc.buf)?;
    for (_, b) in sections {
        f.write_all(b)?;
    }
    for (_, data, _, _) in &blobs {
        f.write_all(data)?;
    }
    f.into_inner().map_err(std::io::IntoInnerError::into_error)?.sync_all()?;
    Ok(24 + toc.buf.len() as u64 + off)
}

/// Writes through a temporary private to the process then renames; errors logged.
fn write_atomic(path: &Path, key: u64, sections: &[(String, Vec<u8>)], rec: &Recorder) -> Option<u64> {
    let dir = path.parent()?;
    let _ = std::fs::create_dir_all(dir);
    let stem = path.file_stem()?.to_string_lossy();
    let tmp = dir.join(format!("{stem}.{}.tmp", std::process::id()));
    match write_file(&tmp, key, sections, rec) {
        Ok(n) => {
            if let Err(e) = std::fs::rename(&tmp, path) {
                let _ = std::fs::remove_file(&tmp);
                eprintln!("resource cache: {}: {e}", path.display());
                return None;
            }
            Some(n)
        }
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            eprintln!("resource cache: {}: {e}", tmp.display());
            None
        }
    }
}

/// Keeps the `keep` most recent `<prefix>*.bin` caches in the folder, deletes the orphaned
/// temporaries.
fn prune(dir: &Path, current: &Path, prefix: &str, keep: usize) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut caches: Vec<(std::time::SystemTime, PathBuf)> = Vec::new();
    for e in rd.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with(prefix) {
            continue;
        }
        let modified = e.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
        if name.ends_with(".tmp") {
            // Temporary of another process: left if recent (write in progress).
            if modified.elapsed().is_ok_and(|d| d.as_secs() > 600) {
                let _ = std::fs::remove_file(&p);
            }
        } else if name.ends_with(".bin") && p != current {
            caches.push((modified, p));
        }
    }
    caches.sort_by_key(|c| std::cmp::Reverse(c.0));
    for (_, p) in caches.into_iter().skip(keep - 1) {
        let _ = std::fs::remove_file(p);
    }
}

/// Modification date set to now: cleanup keeps recently served caches.
fn touch(path: &Path) {
    if let Ok(f) = std::fs::OpenOptions::new().write(true).open(path) {
        let _ = f.set_modified(std::time::SystemTime::now());
    }
}

/// Current thread at normal priority (rendering and the integrated server go first).
#[allow(unsafe_code)]
fn lower_thread_priority() {
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentThread() -> isize;
            fn SetThreadPriority(thread: isize, priority: i32) -> i32;
        }
        // SAFETY: pseudo-descriptor of the current thread; THREAD_PRIORITY_BELOW_NORMAL = −1.
        unsafe {
            SetThreadPriority(GetCurrentThread(), -1);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Startup cache

/// Prefix of the snapshot files (split cache).
const SNAP_PREFIX: &str = "files-";
/// Snapshots kept (different pack stacks side by side).
const KEEP_SNAP: usize = 2;

/// What the snapshot content depends on besides sources and `FORMAT`: PNG decoding (our code
/// and the decoder version, pinned by the lock file), stack semantics, and
/// this module (file layout: table, entries, kinds, listing codec). Forgetting to
/// bump `FORMAT` therefore cannot make a snapshot of another layout be read back.
const SNAP_SOURCES: [&[u8]; 4] = [
    include_bytes!("texture.rs"),
    include_bytes!("source.rs"),
    include_bytes!("../../../Cargo.lock"),
    include_bytes!("cache.rs"),
];
const SNAP_CODE: u64 = snap_code(&SNAP_SOURCES);

const fn snap_code(srcs: &[&[u8]]) -> u64 {
    let mut h = 0;
    let mut i = 0;
    while i < srcs.len() {
        h ^= fnv_const(srcs[i]).rotate_left((i as u32 * 21) % 64);
        i += 1;
    }
    h
}

const fn fnv_const(b: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut i = 0;
    while i < b.len() {
        h = (h ^ b[i] as u64).wrapping_mul(0x0100_0000_01b3);
        i += 1;
    }
    h
}

/// Baking outside recording: view without recording, or paused (`RUSTCRAFT_OPT_RECORDLOCAL=0`).
fn cook<T>(assets: &crate::Assets, compute: impl FnOnce(&crate::Assets) -> T) -> T {
    if Opt::get().record_local {
        compute(&assets.cooking_view())
    } else {
        let _pause = assets.pause_recording();
        compute(assets)
    }
}

/// File snapshot in its own file (split cache, `RUSTCRAFT_OPT_CACHESPLIT`).
struct SnapFile {
    path: PathBuf,
    key: u64,
    store: Option<Arc<Store>>,
}

/// Resource cache of a launch: read back (hot) or being filled (cold).
pub struct AssetCache {
    /// Sections file (split cache) or single file (old path).
    path: PathBuf,
    key: u64,
    store: Option<Arc<Store>>,
    /// Split cache: the file snapshot, separate.
    snap: Option<SnapFile>,
    /// Sections baked this launch (cold), in order.
    pending: Mutex<Vec<(String, Vec<u8>)>>,
    disabled: bool,
}

fn cache_disabled() -> bool {
    rc_data::env_os!("RUSTCRAFT_ASSETCACHE").is_some_and(|v| v == "0")
}

fn open_store(path: &Path, key: u64, disabled: bool) -> Option<Arc<Store>> {
    if disabled {
        return None;
    }
    let s = Store::open(path, key)?;
    if Opt::get().cache_lru {
        touch(path);
    }
    Some(Arc::new(s))
}

fn state(disabled: bool, valid: bool) -> &'static str {
    if disabled {
        "disabled"
    } else if valid {
        "valid"
    } else {
        "missing or stale"
    }
}

impl AssetCache {
    /// Opens the single cache of `dir` for this key. `RUSTCRAFT_ASSETCACHE=0`: neither read nor written.
    pub fn open(dir: &Path, key_text: &str) -> AssetCache {
        let key = fnv(key_text.as_bytes());
        let path = dir.join(format!("assets-{key:016x}.bin"));
        let disabled = cache_disabled();
        let store = open_store(&path, key, disabled);
        trace(format_args!("cache {} : {}", path.display(), state(disabled, store.is_some())));
        AssetCache { path, key, store, snap: None, pending: Mutex::new(Vec::new()), disabled }
    }

    /// Split cache: file snapshot (key `snap_key_text`, without the executable) and baked
    /// sections (key `sections_key_text`). A rebuild only redoes the sections.
    pub fn open_split(dir: &Path, snap_key_text: &str, sections_key_text: &str) -> AssetCache {
        let disabled = cache_disabled();
        let skey = fnv(format!("{snap_key_text}snapshot {SNAP_CODE:016x}\n").as_bytes());
        let spath = dir.join(format!("{SNAP_PREFIX}{skey:016x}.bin"));
        let sstore = open_store(&spath, skey, disabled);
        trace(format_args!("snapshot {}: {}", spath.display(), state(disabled, sstore.is_some())));
        let key = fnv(format!("{sections_key_text}sections\n").as_bytes());
        let path = dir.join(format!("assets-{key:016x}.bin"));
        let store = open_store(&path, key, disabled);
        trace(format_args!("sections {} : {}", path.display(), state(disabled, store.is_some())));
        let snap = Some(SnapFile { path: spath, key: skey, store: sstore });
        AssetCache { path, key, store, snap, pending: Mutex::new(Vec::new()), disabled }
    }

    /// The cache is valid (sections and snapshot): the resources come from it.
    pub fn hot(&self) -> bool {
        self.store.is_some() && self.snap.as_ref().is_none_or(|s| s.store.is_some())
    }

    /// What serves the file reads: the snapshot (split) or the single file.
    pub fn store(&self) -> Option<Arc<Store>> {
        match &self.snap {
            Some(s) => s.store.clone(),
            None => self.store.clone(),
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Section `name` read back from the cache, otherwise computed by `compute` on a view of `assets` whose
    /// reads do not go into the snapshot, then set aside for writing.
    pub fn get<T: Codec>(&self, name: &str, assets: &crate::Assets, compute: impl FnOnce(&crate::Assets) -> T) -> T {
        let (v, bytes) = self.get_unkept(name, assets, compute);
        self.keep(name, bytes);
        v
    }

    /// Like [`AssetCache::get`], without setting anything aside: also returns the encoded section to
    /// write (cold, cache active), which [`AssetCache::keep`] then hands to the writer. A
    /// section never handed over is never written (failed stack, `resources.rs`).
    pub fn get_unkept<T: Codec>(
        &self,
        name: &str,
        assets: &crate::Assets,
        compute: impl FnOnce(&crate::Assets) -> T,
    ) -> (T, Option<Vec<u8>>) {
        self.get_checked(name, assets, compute, check_on())
    }

    /// Sets aside for writing a section returned by [`AssetCache::get_unkept`].
    pub fn keep(&self, name: &str, bytes: Option<Vec<u8>>) {
        if let Some(b) = bytes
            && let Ok(mut p) = self.pending.lock()
        {
            p.push((name.to_owned(), b));
        }
    }

    fn get_checked<T: Codec>(
        &self,
        name: &str,
        assets: &crate::Assets,
        compute: impl FnOnce(&crate::Assets) -> T,
        check: bool,
    ) -> (T, Option<Vec<u8>>) {
        if let Some(s) = &self.store {
            let bytes = s.section(name);
            match bytes.as_deref().and_then(decode::<T>) {
                Some(v) => {
                    if check {
                        // Check: re-baking from the sources, compared byte for byte with
                        // the section read back then re-encoded. Outside recording, like any
                        // baking: sections read back and snapshot absent, recording is running.
                        let t = std::time::Instant::now();
                        let fresh = encode(&cook(assets, compute));
                        let back = encode(&v);
                        eprintln!(
                            "cache check: section {name}: {} ({} bytes, re-baked in {:.0} ms)",
                            if fresh == back && Some(&back) == bytes.as_ref() { "identical" } else { "DIFFERENT" },
                            back.len(),
                            t.elapsed().as_secs_f64() * 1e3
                        );
                    }
                    return (v, None);
                }
                None => eprintln!("resource cache: section {name} unreadable, re-baked"),
            }
        }
        let v = cook(assets, compute);
        let bytes = (self.store.is_none() && !self.disabled).then(|| encode(&v));
        (v, bytes)
    }

    /// End of startup. Cold: additionally reads `images` (textures read on demand later)
    /// and `raw` (language files…) to put them in the snapshot, then writes what is missing (single
    /// file, or sections and/or snapshot), all on a separate thread.
    pub fn finish(self, assets: Arc<crate::Assets>, images: Vec<String>, raw: Vec<String>) {
        if check_on()
            && let Some(s) = self.store()
        {
            s.verify_files(&assets);
        }
        if self.hot() || self.disabled {
            return;
        }
        let sections = self.pending.into_inner().unwrap_or_default();
        let (path, key, sections_stale) = (self.path, self.key, self.store.is_none());
        let snap = self.snap.map(|s| (s.path, s.key, s.store.is_some()));
        let opt = Opt::get();
        let spawned = std::thread::Builder::new().name("cache-resources".into()).spawn(move || {
            if opt.cache_lowprio {
                lower_thread_priority();
            }
            let t = std::time::Instant::now();
            read_lazies(&assets, &images, opt.cache_lowprio);
            for p in &raw {
                let _ = assets.read(p);
            }
            let rec = assets.take_recording();
            let count = images.len() + raw.len();
            let Some(dir) = path.parent() else { return };
            let ms = |t: std::time::Instant| t.elapsed().as_secs_f64() * 1e3;
            let Some((spath, skey, snap_valid)) = snap else {
                // Single file (old path).
                let Some(rec) = rec else { return };
                if let Some(n) = write_atomic(&path, key, &sections, &rec) {
                    drop(rec);
                    prune(dir, &path, "assets-", KEEP);
                    eprintln!(
                        "resource cache written: {} ({:.1} MB, {} sections, {count} files), in {:.0} ms",
                        path.display(),
                        n as f64 / 1_048_576.0,
                        sections.len(),
                        ms(t)
                    );
                }
                return;
            };
            if sections_stale && let Some(n) = write_atomic(&path, key, &sections, &Recorder::default()) {
                prune(dir, &path, "assets-", KEEP);
                eprintln!(
                    "resource cache written: {} ({:.1} MB, {} sections), in {:.0} ms",
                    path.display(),
                    n as f64 / 1_048_576.0,
                    sections.len(),
                    ms(t)
                );
            }
            let rec = rec.unwrap_or_default();
            if !snap_valid {
                if let Some(n) = write_atomic(&spath, skey, &[], &rec) {
                    drop(rec);
                    prune(dir, &spath, SNAP_PREFIX, KEEP_SNAP);
                    eprintln!(
                        "resource cache written: {} ({:.1} MB, snapshot of {count} files), in {:.0} ms",
                        spath.display(),
                        n as f64 / 1_048_576.0,
                        ms(t)
                    );
                }
            } else if !rec.files.is_empty() || !rec.lists.is_empty() {
                // Reads the snapshot does not know about (code changed): it is redone at the
                // next launch, otherwise the jar would be reopened at every launch.
                let _ = std::fs::remove_file(&spath);
                eprintln!(
                    "resource cache: incomplete snapshot ({} files, {} listings outside the snapshot, e.g. {}), redone at next launch",
                    rec.files.len(),
                    rec.lists.len(),
                    rec.files.keys().next().cloned().unwrap_or_default()
                );
            }
        });
        match spawned {
            Ok(h) => {
                if let Ok(mut w) = WRITER.lock() {
                    // Several writes possible (resource reload in game).
                    w.retain(|h| !h.is_finished());
                    w.push(h);
                }
            }
            Err(e) => eprintln!("resource cache: write thread: {e}"),
        }
    }
}

/// Images read on demand later: decoded for the snapshot, on a few low-priority
/// threads (`par`) or one by one on the calling thread.
fn read_lazies(assets: &crate::Assets, images: &[String], par: bool) {
    let n = if par {
        std::thread::available_parallelism().map_or(1, |n| (n.get() / 4).clamp(1, 4)).min(images.len())
    } else {
        1
    };
    if n <= 1 {
        for p in images {
            let _ = assets.image(p);
        }
        return;
    }
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..n {
            s.spawn(|| {
                lower_thread_priority();
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let Some(p) = images.get(i) else { break };
                    let _ = assets.image(p);
                }
            });
        }
    });
}

static WRITER: Mutex<Vec<std::thread::JoinHandle<()>>> = Mutex::new(Vec::new());

/// Waits for the cache writes to finish (call before exiting the process).
pub fn wait_writer() {
    let hs = WRITER.lock().map(|mut w| std::mem::take(&mut *w)).unwrap_or_default();
    for h in hs {
        let _ = h.join();
    }
}

/// Resources of an installation with their cache. Key: `version`, packs (decreasing
/// priority), asset index, jar, then `extra` (executable, version data…); split
/// (`RUSTCRAFT_OPT_CACHESPLIT`), the file snapshot does not depend on `extra`.
/// Valid cache: sources opened only if a read escapes it (sounds). Otherwise: sources
/// opened right away and reads recorded for the snapshot (only those that
/// escape it when the snapshot is valid and the sections stale).
pub fn open_assets(
    dir: &Path,
    version: &str,
    jar: &Path,
    packs: &[PathBuf],
    index: Option<(&Path, &Path)>,
    extra: &[PathBuf],
) -> crate::Result<(crate::Assets, AssetCache)> {
    let mut files: Vec<PathBuf> = packs.to_vec();
    files.extend(index.map(|(i, _)| i.to_owned()));
    files.push(jar.to_owned());
    let n_sources = files.len();
    files.extend_from_slice(extra);
    let cache = if Opt::get().cache_split {
        AssetCache::open_split(dir, &key_text(version, &files[..n_sources]), &sections_key_text(version, &files))
    } else {
        AssetCache::open(dir, &sections_key_text(version, &files))
    };
    if cache.hot()
        && let Some(store) = cache.store()
    {
        return Ok((crate::Assets::lazy(jar, packs, index).with_store(store), cache));
    }
    let mut assets = crate::Assets::for_install(jar, packs, index)?;
    if let Some(store) = cache.store() {
        // Valid snapshot, sections to re-bake: the known files come from it.
        assets = assets.with_store(store);
    }
    if !cache.disabled {
        assets.start_recording();
    }
    Ok((assets, cache))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[derive(Debug, PartialEq, Clone, Copy)]
    enum E {
        A,
        B,
    }
    crate::codec_enum!(E { A, B });

    #[derive(Debug, PartialEq)]
    struct S {
        a: u32,
        b: Vec<(String, Option<f32>)>,
        c: [i16; 3],
        e: E,
        m: HashMap<String, Vec<u8>>,
    }
    crate::codec_struct!(S { a, b, c, e, m });

    #[test]
    fn round_trip() {
        let s = S {
            a: 7,
            b: vec![("x".into(), Some(1.5)), ("é".into(), None)],
            c: [-1, 0, 1],
            e: E::B,
            m: HashMap::from([("k".into(), vec![1, 2, 3])]),
        };
        let b = encode(&s);
        assert_eq!(decode::<S>(&b), Some(s));
        assert!(decode::<S>(&b[..b.len() - 1]).is_none());
    }

    // -----------------------------------------------------------------------------------------
    // Codec: new paths (arrays without heap, numbers in bulk) == old, on every combination.

    /// The four combinations (arrays without heap, numbers in bulk); (false, false) = old path.
    const COMBOS: [(bool, bool); 4] = [(false, false), (true, false), (false, true), (true, true)];

    fn with_codec<T>(c: (bool, bool), f: impl FnOnce() -> T) -> T {
        CODEC_TEST.with(|x| x.set(Some(c)));
        let v = f();
        CODEC_TEST.with(|x| x.set(None));
        v
    }

    #[derive(Debug, PartialEq, Clone)]
    struct Quad {
        pos: [[f32; 3]; 4],
        uv: [[f32; 2]; 4],
        tex: u32,
        cull: Option<E>,
        mats: Vec<[[f32; 4]; 4]>,
        idx: Vec<u32>,
        bytes: Vec<u8>,
        small: Vec<[u16; 2]>,
        flags: [bool; 3],
        names: [String; 2],
        wide: [i64; 2],
    }
    crate::codec_struct!(Quad { pos, uv, tex, cull, mats, idx, bytes, small, flags, names, wide });

    fn quads() -> Vec<Quad> {
        (0..40u32)
            .map(|i| {
                let f = i as f32;
                Quad {
                    pos: std::array::from_fn(|v| std::array::from_fn(|a| f * 0.5 + (v * 3 + a) as f32 - 1.25)),
                    uv: std::array::from_fn(|v| [f / 16.0 + v as f32, -(v as f32) / 3.0]),
                    tex: i * 7919,
                    cull: (i % 3 != 0).then_some(if i % 2 == 0 { E::A } else { E::B }),
                    mats: (0..i % 3)
                        .map(|m| {
                            std::array::from_fn(|r| {
                                std::array::from_fn(|c| (m * 16 + r as u32 * 4 + c as u32) as f32 - f)
                            })
                        })
                        .collect(),
                    idx: (0..i).map(|k| k.wrapping_mul(2_654_435_761)).collect(),
                    bytes: (0..i * 3).map(|k| (k * 31 + i) as u8).collect(),
                    small: (0..i % 5).map(|k| [k as u16, u16::MAX - k as u16]).collect(),
                    flags: [i % 2 == 0, i % 3 == 0, true],
                    names: [format!("q{i}"), "é".repeat(i as usize % 4)],
                    wide: [-(i as i64) << 40, i as i64],
                }
            })
            .collect()
    }

    #[test]
    fn codec_arrays_and_blocks_equivalent() {
        let v = quads();
        let b = encode(&v);
        for c in COMBOS {
            let got = with_codec(c, || decode::<Vec<Quad>>(&b));
            assert_eq!(got.as_ref(), Some(&v), "combination {c:?}");
            assert_eq!(encode(got.as_ref().unwrap()), b, "re-encoding {c:?}");
        }
        // Truncated anywhere (including in the middle of an array): None everywhere, never a panic.
        for cut in 0..b.len() {
            for c in COMBOS {
                assert!(with_codec(c, || decode::<Vec<Quad>>(&b[..cut])).is_none(), "cut {cut} {c:?}");
            }
        }
        // Invalid boolean in an array: rejected by every path.
        let mut bad = encode(&[true, false]);
        bad[1] = 2;
        for c in COMBOS {
            assert!(with_codec(c, || decode::<[bool; 2]>(&bad)).is_none());
        }
    }

    /// Sabotage of the test itself: two elements of an array swapped in the bytes must
    /// be seen (the comparison is not blind).
    #[test]
    fn codec_inversion_detected() {
        let v: [[f32; 3]; 2] = [[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]];
        let mut b = encode(&v);
        b.swap(0, 4);
        b.swap(1, 5);
        b.swap(2, 6);
        b.swap(3, 7);
        for c in COMBOS {
            assert_ne!(with_codec(c, || decode::<[[f32; 3]; 2]>(&b)), Some(v));
        }
    }

    // -----------------------------------------------------------------------------------------
    // Real cache (`RUSTCRAFT_ASSETCACHE_FILE`, otherwise the most recent in the cache folder).

    fn required() -> bool {
        rc_data::env_str!("RUSTCRAFT_OPT_REQUIRE") == Some("1")
    }

    /// The most recent `<prefix>*.bin` file in the real cache folder.
    fn newest_real(prefix: &str) -> Option<PathBuf> {
        let dir = rc_data::cache_root().join("26.3");
        let mut v: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(dir)
            .ok()?
            .flatten()
            .filter(|e| {
                e.file_name().to_string_lossy().starts_with(prefix) && e.file_name().to_string_lossy().ends_with(".bin")
            })
            .filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path())))
            .collect();
        v.sort();
        v.pop().map(|p| p.1)
    }

    /// Sections file (or single file) of the real cache.
    fn real_cache() -> Option<Store> {
        let path = rc_data::env_os!("RUSTCRAFT_ASSETCACHE_FILE").map(PathBuf::from).or_else(|| newest_real("assets-"));
        let Some(path) = path else {
            assert!(!required(), "real resource cache missing (RUSTCRAFT_OPT_REQUIRE=1)");
            eprintln!("real resource cache missing: test skipped");
            return None;
        };
        open_real(&path)
    }

    /// Sections and snapshot of the real cache: with the split cache, large images are in
    /// `files-*.bin`, absent from the sections file.
    fn real_caches() -> Vec<Store> {
        let mut v: Vec<Store> = real_cache().into_iter().collect();
        if rc_data::env_os!("RUSTCRAFT_ASSETCACHE_FILE").is_none() {
            v.extend(newest_real(SNAP_PREFIX).and_then(|p| open_real(&p)));
        }
        v
    }

    fn open_real(path: &Path) -> Option<Store> {
        let mut head = [0u8; 24];
        read_exact_at(&std::fs::File::open(path).ok()?, &mut head, 0).ok()?;
        let key = u64::from_le_bytes(head[8..16].try_into().ok()?);
        let s = Store::open(path, key);
        assert!(s.is_some() || !required(), "real cache unreadable: {}", path.display());
        eprintln!("real cache: {}", path.display());
        s
    }

    /// A section of the real cache decoded by each combination: re-encoded, same bytes; duration
    /// of the best of 5 decodes (1 thread) and allocations, per combination.
    fn real_section<T: Codec>(s: &Store, name: &str) {
        // Old read (isolated from `RUSTCRAFT_OPT_CACHEREAD`, tested separately).
        let Some(b) = s.sections.get(name).and_then(|&(o, l)| s.read_range_zeroed(o, l)) else {
            assert!(!required(), "section {name} missing from the real cache");
            return;
        };
        if with_codec((false, false), || decode::<T>(&b)).is_none() {
            // Section written by code other than this one (shape changed since): nothing to compare.
            assert!(!required(), "section {name}: shape differs from the current code");
            eprintln!("section {name}: shape differs from the current code, skipped");
            return;
        }
        let mut line = format!("section {name} ({:.1} MB):", b.len() as f64 / 1_048_576.0);
        for c in COMBOS {
            let mut best = f64::MAX;
            let mut allocs = 0;
            for _ in 0..5 {
                let a0 = ALLOCS.load(std::sync::atomic::Ordering::Relaxed);
                let t = std::time::Instant::now();
                let v = with_codec(c, || decode::<T>(&b)).expect("decoding");
                best = best.min(t.elapsed().as_secs_f64() * 1e3);
                allocs = ALLOCS.load(std::sync::atomic::Ordering::Relaxed) - a0;
                assert!(encode(&v) == b, "section {name}: re-encoding differs ({c:?})");
            }
            line += &format!(" {c:?} {best:.2} ms {allocs} allocs;");
        }
        // Truncated in the middle: None, no panic.
        for c in COMBOS {
            assert!(with_codec(c, || decode::<T>(&b[..b.len() / 2])).is_none());
            assert!(with_codec(c, || decode::<T>(&b[..b.len() - 3])).is_none());
        }
        eprintln!("{line}");
    }

    #[test]
    fn real_cache_equivalent_sections() {
        let Some(s) = real_cache() else { return };
        real_section::<crate::BlockModels>(&s, "blocks");
        real_section::<crate::font::Fonts>(&s, "fonts");
        real_section::<crate::item::ItemModels>(&s, "items");
        real_section::<crate::gui::GuiAtlas>(&s, "interface");
        // All cores: the four sections decoded at the same time (one thread each), per
        // combination, against their serial decoding.
        let names = ["blocks", "fonts", "items", "interface"];
        let bytes: Vec<Vec<u8>> =
            names.iter().filter_map(|n| s.sections.get(*n).and_then(|&(o, l)| s.read_range_zeroed(o, l))).collect();
        if bytes.len() != names.len() {
            return;
        }
        fn dec_one(i: usize, b: &[u8]) -> bool {
            match i {
                0 => decode::<crate::BlockModels>(b).is_some(),
                1 => decode::<crate::font::Fonts>(b).is_some(),
                2 => decode::<crate::item::ItemModels>(b).is_some(),
                _ => decode::<crate::gui::GuiAtlas>(b).is_some(),
            }
        }
        let mut line = String::from("4 sections, serial / 4 threads:");
        for c in COMBOS {
            let (mut ser, mut par) = (f64::MAX, f64::MAX);
            for _ in 0..3 {
                let t = std::time::Instant::now();
                assert!(with_codec(c, || bytes.iter().enumerate().all(|(i, b)| dec_one(i, b))));
                ser = ser.min(t.elapsed().as_secs_f64() * 1e3);
                let t = std::time::Instant::now();
                std::thread::scope(|sc| {
                    let hs: Vec<_> = bytes
                        .iter()
                        .enumerate()
                        .map(|(i, b)| sc.spawn(move || with_codec(c, || dec_one(i, b))))
                        .collect();
                    assert!(hs.into_iter().all(|h| h.join().unwrap_or(false)));
                });
                par = par.min(t.elapsed().as_secs_f64() * 1e3);
            }
            line += &format!(" {c:?} {ser:.1} / {par:.1} ms;");
        }
        eprintln!("{line}");
    }

    /// Read without zeroing: same bytes as the old one, each section and large image
    /// (sections and snapshot, split cache or single file).
    #[test]
    fn real_cache_read_without_zeroing() {
        let stores = real_caches();
        if stores.is_empty() {
            return;
        }
        let mut ranges: Vec<(&Store, String, u64, u64)> = Vec::new();
        for s in &stores {
            ranges.extend(s.sections.iter().map(|(n, &(o, l))| (s, n.clone(), o, l)));
            ranges.extend(s.files.iter().filter(|(_, l)| l.len >= BIG_READ).map(|(p, l)| (s, p.clone(), l.off, l.len)));
        }
        let big_files = stores.iter().flat_map(|s| s.files.values()).filter(|l| l.len >= BIG_READ).count();
        assert!(big_files > 0 || !required(), "no snapshot file ≥ 1 MiB: large images not covered");
        let total: u64 = ranges.iter().map(|r| r.3).sum();
        // Alternating passes (order reversed every other time), best of 3 per path.
        let (mut t_old, mut t_new) = (f64::MAX, f64::MAX);
        for pass in 0..6 {
            let new_first = pass % 2 == 1;
            let (mut a_s, mut b_s) = (0.0, 0.0);
            for (s, n, off, len) in &ranges {
                let run = |uninit: bool| {
                    let t = std::time::Instant::now();
                    let v = if uninit { s.read_range_uninit(*off, *len) } else { s.read_range_zeroed(*off, *len) };
                    (v.expect("read"), t.elapsed().as_secs_f64())
                };
                let (x, y) = if new_first {
                    let y = run(true);
                    (run(false), y)
                } else {
                    let x = run(false);
                    (x, run(true))
                };
                assert!(fnv(&x.0) == fnv(&y.0) && x.0 == y.0, "{n}: different bytes");
                a_s += x.1;
                b_s += y.1;
            }
            t_old = t_old.min(a_s);
            t_new = t_new.min(b_s);
        }
        eprintln!(
            "read of {} blocks ({:.1} MB, of which {big_files} files), best of 6 alternating passes: zeroing {:.1} ms, without {:.1} ms",
            ranges.len(),
            total as f64 / 1_048_576.0,
            t_old * 1e3,
            t_new * 1e3
        );
    }

    // -----------------------------------------------------------------------------------------
    // Allocation counter (codec micro-bench).

    pub(crate) static ALLOCS: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    /// Live bytes and their peak (reset to the current level by `reset_peak`).
    static LIVE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    static PEAK: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

    fn grow(n: usize) {
        let now = LIVE.fetch_add(n, std::sync::atomic::Ordering::Relaxed) + n;
        PEAK.fetch_max(now, std::sync::atomic::Ordering::Relaxed);
    }

    /// Peak reset to the current level; returns the current level.
    fn reset_peak() -> usize {
        let now = LIVE.load(std::sync::atomic::Ordering::Relaxed);
        PEAK.store(now, std::sync::atomic::Ordering::Relaxed);
        now
    }

    struct Counting;

    #[allow(unsafe_code)]
    // SAFETY: delegates everything to the system allocator, only counts the calls and the bytes.
    unsafe impl std::alloc::GlobalAlloc for Counting {
        unsafe fn alloc(&self, l: std::alloc::Layout) -> *mut u8 {
            ALLOCS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            grow(l.size());
            // SAFETY: same contract as the caller.
            unsafe { std::alloc::System.alloc(l) }
        }
        unsafe fn dealloc(&self, p: *mut u8, l: std::alloc::Layout) {
            LIVE.fetch_sub(l.size(), std::sync::atomic::Ordering::Relaxed);
            // SAFETY: same contract as the caller.
            unsafe { std::alloc::System.dealloc(p, l) }
        }
        unsafe fn realloc(&self, p: *mut u8, l: std::alloc::Layout, n: usize) -> *mut u8 {
            ALLOCS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            LIVE.fetch_sub(l.size(), std::sync::atomic::Ordering::Relaxed);
            grow(n);
            // SAFETY: same contract as the caller.
            unsafe { std::alloc::System.realloc(p, l, n) }
        }
    }

    /// The real cache rewritten from its own content: streamed write == old write,
    /// bit for bit; duration and memory peak of each (1 thread, loaded machine).
    #[test]
    fn real_cache_streamed_rewrite() {
        for s in real_caches() {
            real_rewrite(&s);
        }
    }

    fn real_rewrite(s: &Store) {
        let mut rec = Recorder::default();
        for (p, l) in &s.files {
            let v = match l.kind {
                K_ABSENT => Snap::Absent,
                K_IMAGE => match s.image(p) {
                    Hit::Found(i) => Snap::Image(i),
                    _ => panic!("{p} unreadable"),
                },
                _ => match s.read(p) {
                    Hit::Found(b) => Snap::Raw(b),
                    _ => panic!("{p} unreadable"),
                },
            };
            rec.files.insert(p.clone(), v);
        }
        rec.lists = s.lists.clone();
        let mut secs: Vec<(u64, String, Vec<u8>)> =
            s.sections.iter().map(|(n, &(o, l))| (o, n.clone(), s.read_range_zeroed(o, l).expect("section"))).collect();
        secs.sort_by_key(|x| x.0);
        let secs: Vec<(String, Vec<u8>)> = secs.into_iter().map(|(_, n, b)| (n, b)).collect();
        let d = tmp_dir("rewrite");
        let (a, b) = (d.join("a.bin"), d.join("b.bin"));
        let base = reset_peak();
        let t = std::time::Instant::now();
        write_file_buffered(&a, 1, &secs, &rec).expect("write");
        let (ta, pa) = (t.elapsed().as_secs_f64() * 1e3, PEAK.load(std::sync::atomic::Ordering::Relaxed) - base);
        let base = reset_peak();
        let t = std::time::Instant::now();
        write_file_stream(&b, 1, &secs, &rec).expect("write");
        let (tb, pb) = (t.elapsed().as_secs_f64() * 1e3, PEAK.load(std::sync::atomic::Ordering::Relaxed) - base);
        let (ba, bb) = (std::fs::read(&a).expect("a"), std::fs::read(&b).expect("b"));
        assert!(ba == bb, "streamed write: bytes differ from the rewritten real cache");
        let orig = std::fs::read(&s.path).expect("original");
        eprintln!(
            "rewrite of the real cache ({:.1} MB, {}): old {ta:.0} ms, peak +{:.1} MB; streamed {tb:.0} ms, peak +{:.1} MB",
            ba.len() as f64 / 1_048_576.0,
            if orig[24..] == ba[24..] {
                "identical to the original except the key"
            } else {
                "language files recompressed differently from the original"
            },
            pa as f64 / 1_048_576.0,
            pb as f64 / 1_048_576.0
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[global_allocator]
    static A: Counting = Counting;

    // -----------------------------------------------------------------------------------------
    // Streamed write, split cache, per-view recording.

    fn tmp_dir(name: &str) -> PathBuf {
        let d = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-tmp")
            .join(format!("c4m05-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).expect("temporary directory");
        d
    }

    fn png(w: u32, h: u32, seed: u8) -> Vec<u8> {
        let mut out = Vec::new();
        let mut e = png::Encoder::new(&mut out, w, h);
        e.set_color(png::ColorType::Rgba);
        e.set_depth(png::BitDepth::Eight);
        let mut wr = e.write_header().expect("png");
        let data: Vec<u8> = (0..w * h * 4).map(|i| (i as u8).wrapping_mul(seed)).collect();
        wr.write_image_data(&data).expect("png");
        drop(wr);
        out
    }

    #[test]
    fn streamed_write_same_bytes() {
        let d = tmp_dir("stream");
        let mut rec = Recorder::default();
        rec.files.insert("assets/minecraft/lang/fr_fr.json".into(), Snap::Raw(b"{\"a\":\"b\"}".repeat(500)));
        rec.files.insert("assets/minecraft/lang/en_us.json".into(), Snap::Raw(b"{}".to_vec()));
        rec.files.insert("a/b.txt".into(), Snap::Raw(vec![1, 2, 3]));
        rec.files.insert("absent.png".into(), Snap::Absent);
        rec.files.insert("i.png".into(), Snap::Image(Image { width: 2, height: 3, rgba: (0..24).collect() }));
        rec.lists.insert(("p/".into(), ".json".into()), vec!["p/a.json".into()]);
        let sections = vec![("x".to_owned(), vec![9u8; 1000]), ("y".to_owned(), vec![])];
        let (a, b) = (d.join("a.bin"), d.join("b.bin"));
        let na = write_file_buffered(&a, 42, &sections, &rec).expect("write");
        let nb = write_file_stream(&b, 42, &sections, &rec).expect("write");
        let (ba, bb) = (std::fs::read(&a).expect("a"), std::fs::read(&b).expect("b"));
        assert_eq!(na, nb);
        assert_eq!(na, ba.len() as u64);
        assert!(ba == bb, "streamed write: bytes differ");
        // Read back: sections, files, listings.
        let s = Store::open(&b, 42).expect("read back");
        assert_eq!(s.section("x"), Some(vec![9u8; 1000]));
        assert!(
            matches!(s.read("assets/minecraft/lang/fr_fr.json"), Hit::Found(v) if v == b"{\"a\":\"b\"}".repeat(500))
        );
        assert!(matches!(s.image("i.png"), Hit::Found(i) if i.rgba == (0..24).collect::<Vec<u8>>()));
        assert!(matches!(s.read("absent.png"), Hit::Absent));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Per-view recording: baking does not record its reads, but another thread that
    /// reads in the meantime is recorded (the old global counter lost it).
    #[test]
    fn baking_view_does_not_mask_other_threads() {
        let d = tmp_dir("view");
        std::fs::write(d.join("a.txt"), b"a").expect("a");
        std::fs::write(d.join("b.txt"), b"b").expect("b");
        let assets = crate::Assets::new(vec![crate::source::Source::dir(&d)]);
        // New path.
        assets.start_recording();
        let view = assets.cooking_view();
        assert_eq!(view.read("a.txt").as_deref(), Some(&b"a"[..]));
        std::thread::scope(|s| {
            s.spawn(|| assets.read("b.txt"));
        });
        let rec = assets.take_recording().expect("recording");
        assert!(rec.files.contains_key("b.txt"), "read from another thread lost");
        assert!(!rec.files.contains_key("a.txt"), "baking read recorded");
        // Old path: the global counter also hides the other thread (fixed defect).
        assets.start_recording();
        {
            let _p = assets.pause_recording();
            let _ = assets.read("a.txt");
            std::thread::scope(|s| {
                s.spawn(|| assets.read("b.txt"));
            });
        }
        let rec = assets.take_recording().expect("recording");
        assert!(rec.files.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Full split cycle: cold (everything written), hot with no source at all (nothing outside the cache),
    /// rebuild (only sections re-baked, snapshot kept as is), then valid.
    #[test]
    fn split_cache_cold_hot_rebuild() {
        if !Opt::get().cache_split || !Opt::get().record_local {
            eprintln!("RUSTCRAFT_OPT_CACHESPLIT=0 or RECORDLOCAL=0: test skipped");
            return;
        }
        let d = tmp_dir("split");
        let (src, cache) = (d.join("src"), d.join("cache"));
        std::fs::create_dir_all(&src).expect("src");
        for (n, b) in [
            ("baked.txt", &b"baked"[..]),
            ("outside.txt", b"outside"),
            ("during.txt", b"during"),
            ("extra.txt", b"extra"),
        ] {
            std::fs::write(src.join(n), b).expect("file");
        }
        let images: Vec<String> = (0..6).map(|i| format!("img{i}.png")).collect();
        for (i, p) in images.iter().enumerate() {
            std::fs::write(src.join(p), png(3 + i as u32, 2, 17 + i as u8)).expect("png");
        }
        let cold = |sections_key: &str, extra: Option<&str>| {
            let c = AssetCache::open_split(&cache, "sources", sections_key);
            let mut a = crate::Assets::new(vec![crate::source::Source::dir(&src)]);
            if let Some(s) = c.store() {
                a = a.with_store(s);
            }
            a.start_recording();
            let a = Arc::new(a);
            let v = c.get("s", &a, |view| {
                let _ = view.read("baked.txt");
                // Another thread reads during baking: recorded.
                std::thread::scope(|s| {
                    s.spawn(|| a.read("during.txt"));
                });
                vec![7u32, 8, 9]
            });
            assert_eq!(v, vec![7, 8, 9]);
            let _ = a.read("outside.txt");
            if let Some(p) = extra {
                let _ = a.read(p);
            }
            let hot = c.hot();
            c.finish(a, images.clone(), vec![]);
            wait_writer();
            hot
        };
        assert!(!cold("exe1", None));
        let snap: Vec<PathBuf> = std::fs::read_dir(&cache)
            .expect("cache")
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with(SNAP_PREFIX)))
            .collect();
        assert_eq!(snap.len(), 1, "one snapshot written");
        let snap_bytes = std::fs::read(&snap[0]).expect("snapshot");

        // Hot, sources erased: everything comes from the cache; the baking read is not in it.
        let hot = |sections_key: &str| {
            let c = AssetCache::open_split(&cache, "sources", sections_key);
            assert!(c.hot(), "valid cache expected");
            let a = crate::Assets::new(vec![crate::source::Source::lazy(&d.join("gone"), None)])
                .with_store(c.store().expect("snapshot"));
            let v: Vec<u32> = c.get("s", &a, |_| panic!("hot re-baking"));
            assert_eq!(v, vec![7, 8, 9]);
            assert_eq!(a.read("outside.txt").as_deref(), Some(&b"outside"[..]));
            assert_eq!(a.read("during.txt").as_deref(), Some(&b"during"[..]));
            assert_eq!(a.read("baked.txt"), None, "baking read in the snapshot");
            for (i, p) in images.iter().enumerate() {
                let img = a.image(p).expect("snapshot image");
                assert_eq!((img.width, img.height), (3 + i as u32, 2));
            }
        };
        hot("exe1");

        // Rebuild (other executable): sections re-baked, snapshot intact (same bytes).
        assert!(!cold("exe2", None));
        assert!(std::fs::read(&snap[0]).expect("snapshot") == snap_bytes, "snapshot rewritten");
        hot("exe2");
        hot("exe1");

        // Rebuild that reads a new file outside baking: snapshot removed, redone afterwards.
        assert!(!cold("exe3", Some("extra.txt")));
        assert!(!snap[0].exists(), "incomplete snapshot kept");
        assert!(!cold("exe3", Some("extra.txt")));
        let c = AssetCache::open_split(&cache, "sources", "exe3");
        assert!(c.hot());
        let a = crate::Assets::new(vec![]).with_store(c.store().expect("snapshot"));
        assert_eq!(a.read("extra.txt").as_deref(), Some(&b"extra"[..]));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// File replaced under the same name while a `Store` reads it (split snapshot rewritten
    /// under the same key with another table): the large read returns the bytes of the open
    /// file, not those of the new one at the old offsets.
    #[test]
    fn large_read_through_the_open_descriptor() {
        let d = tmp_dir("descriptor");
        let big =
            |seed: u8| -> Vec<u8> { (0..BIG_READ as usize + 4099).map(|i| (i as u8).wrapping_mul(seed)).collect() };
        let (p, t) = (d.join("c.bin"), d.join("t.bin"));
        write_file_stream(&p, 7, &[("x".to_owned(), big(3))], &Recorder::default()).expect("write");
        let s = Store::open(&p, 7).expect("open");
        write_file_stream(&t, 7, &[("x".to_owned(), big(5))], &Recorder::default()).expect("write");
        std::fs::rename(&t, &p).expect("replacement by rename, like the writer");
        let (o, l) = s.sections["x"];
        assert!(s.read_range_uninit(o, l) == Some(big(3)), "large read served by the replaced file");
        assert_eq!(s.read_range_zeroed(o, l), Some(big(3)));
        assert_eq!(s.section("x"), Some(big(3)));
        // Past the end: None, no panic.
        assert_eq!(s.read_range_uninit(o, l + 1), None);
        drop(s);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// The snapshot key covers this module (file layout, listing codec): a
    /// change here without bumping `FORMAT` is never read back with the old layout.
    #[test]
    fn snapshot_key_covers_this_module() {
        assert!(SNAP_SOURCES.contains(&&include_bytes!("cache.rs")[..]), "cache.rs outside SNAP_CODE");
        assert_eq!(SNAP_CODE, snap_code(&SNAP_SOURCES));
        assert_ne!(SNAP_CODE, snap_code(&SNAP_SOURCES[..SNAP_SOURCES.len() - 1]));
    }

    /// Hot check (`RUSTCRAFT_ASSETCHECK=1`) with sections read back and snapshot absent:
    /// recording is running, the check's re-baking must not enter it.
    #[test]
    fn hot_check_outside_recording() {
        let d = tmp_dir("check");
        let (src, cache) = (d.join("src"), d.join("cache"));
        std::fs::create_dir_all(&src).expect("src");
        std::fs::write(src.join("baked.txt"), b"baked").expect("file");
        let cook_s = |v: &crate::Assets| {
            let _ = v.read("baked.txt");
            vec![4u32, 2]
        };
        let c = AssetCache::open_split(&cache, "sources", "exe");
        let a = Arc::new(crate::Assets::new(vec![crate::source::Source::dir(&src)]));
        assert_eq!(c.get("s", &a, cook_s), vec![4, 2]);
        c.finish(a, vec![], vec![]);
        wait_writer();
        for e in std::fs::read_dir(&cache).expect("cache").flatten() {
            if e.file_name().to_string_lossy().starts_with(SNAP_PREFIX) {
                std::fs::remove_file(e.path()).expect("snapshot removed");
            }
        }
        let c = AssetCache::open_split(&cache, "sources", "exe");
        assert!(c.store.is_some() && !c.hot(), "valid sections, absent snapshot expected");
        let a = crate::Assets::new(vec![crate::source::Source::dir(&src)]);
        a.start_recording();
        assert_eq!(c.get_checked("s", &a, cook_s, true).0, vec![4, 2]);
        let rec = a.take_recording().expect("recording");
        assert!(!rec.files.contains_key("baked.txt"), "read of the check's re-baking recorded");
        drop(c);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Baking without trust (`get_unkept`, failed stack): nothing set aside, no file
    /// written. Handed over afterwards (stack kept): written, in the order of the handovers.
    #[test]
    fn section_not_handed_over_never_written() {
        let d = tmp_dir("handed");
        let (src, cache) = (d.join("src"), d.join("cache"));
        std::fs::create_dir_all(&src).expect("src");
        let files = |dir: &Path| std::fs::read_dir(dir).map_or(0, |r| r.flatten().count());
        // Failed stack: the default font is missing, the baked blocks are never handed over.
        let c = AssetCache::open_split(&cache, "sources", "exe");
        let a = Arc::new(crate::Assets::new(vec![crate::source::Source::dir(&src)]));
        let (v, b) = c.get_unkept("blocks", &a, |_| vec![1u32, 2]);
        assert_eq!(v, vec![1, 2]);
        assert!(b.is_some(), "bytes to write expected when cold");
        assert!(c.pending.lock().expect("lock").is_empty(), "section set aside without trust");
        drop(c);
        wait_writer();
        assert_eq!(files(&cache), 0, "cache file written for a failed stack");
        // Stack kept: blocks baked in parallel with the fonts, handed over after them.
        let c = AssetCache::open_split(&cache, "sources", "exe");
        let (_, b) = c.get_unkept("blocks", &a, |_| vec![1u32, 2]);
        assert_eq!(c.get("fonts", &a, |_| vec![3u32]), vec![3]);
        c.keep("blocks", b);
        c.finish(a, vec![], vec![]);
        wait_writer();
        let c = AssetCache::open_split(&cache, "sources", "exe");
        let s = c.store.clone().expect("sections written");
        assert!(s.sections["fonts"].0 < s.sections["blocks"].0, "section order changed");
        let a = crate::Assets::new(vec![]);
        assert_eq!(c.get("blocks", &a, |_| -> Vec<u32> { panic!("hot re-baking") }), vec![1, 2]);
        drop((c, s));
        let _ = std::fs::remove_dir_all(&d);
    }
}
