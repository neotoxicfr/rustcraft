//! Stacked resource sources: player packs (folders or zip) above the jar of the
//! version. The first one that owns a path wins (a pack's overlays before its root); the
//! filter of a pack hides the files of the lower packs. Nothing is copied to disk.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use crate::{Error, Result};

/// Shared file read by positioned reads (no common cursor): each clone has its own
/// position and buffer, the content stays on disk (system cache), not on our heap.
struct PosFile {
    file: Arc<std::fs::File>,
    len: u64,
    pos: u64,
    /// Read buffer: bytes starting at `buf_at`.
    buf: Vec<u8>,
    buf_at: u64,
}

const POS_BUF: usize = 64 * 1024;

impl Clone for PosFile {
    fn clone(&self) -> Self {
        PosFile { file: self.file.clone(), len: self.len, pos: self.pos, buf: Vec::new(), buf_at: 0 }
    }
}

#[cfg(windows)]
fn read_at(f: &std::fs::File, buf: &mut [u8], at: u64) -> std::io::Result<usize> {
    std::os::windows::fs::FileExt::seek_read(f, buf, at)
}

#[cfg(unix)]
fn read_at(f: &std::fs::File, buf: &mut [u8], at: u64) -> std::io::Result<usize> {
    std::os::unix::fs::FileExt::read_at(f, buf, at)
}

impl Read for PosFile {
    fn read(&mut self, out: &mut [u8]) -> std::io::Result<usize> {
        let end = self.buf_at + self.buf.len() as u64;
        if self.pos < self.buf_at || self.pos >= end {
            // Large read: direct; small: through the buffer.
            if out.len() >= POS_BUF {
                let n = read_at(&self.file, out, self.pos)?;
                self.pos += n as u64;
                return Ok(n);
            }
            self.buf.resize(POS_BUF, 0);
            let n = read_at(&self.file, &mut self.buf, self.pos)?;
            self.buf.truncate(n);
            self.buf_at = self.pos;
            if n == 0 {
                return Ok(0);
            }
        }
        let off = (self.pos - self.buf_at) as usize;
        let n = out.len().min(self.buf.len() - off);
        out[..n].copy_from_slice(&self.buf[off..off + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl std::io::Seek for PosFile {
    fn seek(&mut self, to: std::io::SeekFrom) -> std::io::Result<u64> {
        let p = match to {
            std::io::SeekFrom::Start(p) => Some(p),
            std::io::SeekFrom::End(d) => self.len.checked_add_signed(d),
            std::io::SeekFrom::Current(d) => self.pos.checked_add_signed(d),
        };
        self.pos = p.ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "negative position"))?;
        Ok(self.pos)
    }
}

enum Kind {
    /// Archive read from disk on demand; each read clones the index (shared).
    Zip(zip::ZipArchive<PosFile>),
    Dir(PathBuf),
    /// Launcher asset index: path (`assets/<ns>/…`) → object file by fingerprint.
    Index {
        objects: PathBuf,
        map: hashbrown::HashMap<String, String>,
    },
}

/// How to open a source (opening at first access for lazy sources).
enum Spec {
    Zip(PathBuf),
    Dir(PathBuf),
    Index(PathBuf, PathBuf),
}

/// Layers of a resource pack (read from its `pack.mcmeta` on opening): applicable overlay folders
/// (`dir/` prefixes, decreasing priority, before the root) and filter of the lower packs.
#[derive(Default)]
struct PackLayers {
    overlays: Vec<String>,
    filter: Option<crate::packs::Filter>,
}

struct Opened {
    kind: Kind,
    layers: PackLayers,
}

pub struct Source {
    pub name: String,
    spec: Spec,
    /// Player or server pack (overlays and filter read); false for the jar and the index.
    pack: bool,
    /// Opened source (`None`: unreadable), at first access if lazy.
    kind: std::sync::OnceLock<Option<Opened>>,
}

fn open_zip(path: &Path) -> Result<Kind> {
    crate::cache::trace(format_args!("opening jar/zip {}", path.display()));
    let file = std::fs::File::open(path).map_err(|e| Error::io(path, e))?;
    let len = file.metadata().map_err(|e| Error::io(path, e))?.len();
    let reader = PosFile { file: Arc::new(file), len, pos: 0, buf: Vec::new(), buf_at: 0 };
    let archive = zip::ZipArchive::new(reader).map_err(|e| Error::Format(format!("{} : {e}", path.display())))?;
    Ok(Kind::Zip(archive))
}

fn open_index(index: &Path, objects: &Path) -> Result<Kind> {
    crate::cache::trace(format_args!("opening index {}", index.display()));
    let text = std::fs::read(index).map_err(|e| Error::io(index, e))?;
    let v: serde_json::Value =
        serde_json::from_slice(&text).map_err(|e| Error::Format(format!("{} : {e}", index.display())))?;
    let map = v["objects"]
        .as_object()
        .map(|o| o.iter().filter_map(|(k, e)| Some((format!("assets/{k}"), e["hash"].as_str()?.to_owned()))).collect())
        .unwrap_or_default();
    Ok(Kind::Index { objects: objects.to_owned(), map })
}

fn raw_read(kind: &Kind, path: &str) -> Option<Vec<u8>> {
    match kind {
        Kind::Zip(z) => {
            let mut z = z.clone();
            let mut e = z.by_name(path).ok()?;
            let mut out = Vec::with_capacity(e.size() as usize);
            e.read_to_end(&mut out).ok()?;
            Some(out)
        }
        Kind::Dir(root) => std::fs::read(root.join(path)).ok(),
        Kind::Index { objects, map } => {
            let h = map.get(path)?;
            std::fs::read(objects.join(h.get(..2)?).join(h)).ok()
        }
    }
}

fn raw_contains(kind: &Kind, path: &str) -> bool {
    match kind {
        Kind::Zip(z) => z.index_for_name(path).is_some(),
        Kind::Dir(root) => root.join(path).is_file(),
        Kind::Index { map, .. } => map.contains_key(path),
    }
}

/// Overlays and filter of an opened pack; meta absent or unreadable: none.
fn pack_layers(kind: &Kind) -> PackLayers {
    let Some(meta) = raw_read(kind, "pack.mcmeta").and_then(|b| crate::packs::meta_json(&b)) else {
        return PackLayers::default();
    };
    let overlays = crate::packs::overlays(&meta).unwrap_or_default().into_iter().map(|d| format!("{d}/")).collect();
    PackLayers { overlays, filter: crate::packs::filter(&meta) }
}

impl Source {
    fn ready(name: String, spec: Spec, kind: Kind) -> Self {
        let cell = std::sync::OnceLock::new();
        let _ = cell.set(Some(Opened { kind, layers: PackLayers::default() }));
        Source { name, spec, pack: false, kind: cell }
    }

    /// Marks the source as a resource pack: overlays and filter of its `pack.mcmeta`.
    pub fn as_pack(mut self) -> Self {
        self.pack = true;
        if let Some(Some(o)) = self.kind.get_mut() {
            o.layers = pack_layers(&o.kind);
        }
        self
    }

    pub fn zip(path: &Path) -> Result<Self> {
        Ok(Source::ready(path.display().to_string(), Spec::Zip(path.to_owned()), open_zip(path)?))
    }

    pub fn dir(path: &Path) -> Self {
        Source::ready(path.display().to_string(), Spec::Dir(path.to_owned()), Kind::Dir(path.to_owned()))
    }

    /// Asset index (`assets/indexes/<id>.json`) and its `objects` folder.
    pub fn index(index: &Path, objects: &Path) -> Result<Self> {
        let spec = Spec::Index(index.to_owned(), objects.to_owned());
        Ok(Source::ready(index.display().to_string(), spec, open_index(index, objects)?))
    }

    /// Source opened only at first access (pack or jar when the cache serves the rest).
    pub fn lazy(path: &Path, index_objects: Option<&Path>) -> Self {
        let spec = match index_objects {
            Some(o) => Spec::Index(path.to_owned(), o.to_owned()),
            None if path.is_dir() => Spec::Dir(path.to_owned()),
            None => Spec::Zip(path.to_owned()),
        };
        Source { name: path.display().to_string(), spec, pack: false, kind: std::sync::OnceLock::new() }
    }

    fn opened(&self) -> Option<&Opened> {
        self.kind
            .get_or_init(|| {
                let k = match &self.spec {
                    Spec::Zip(p) => open_zip(p),
                    Spec::Dir(p) => Ok(Kind::Dir(p.clone())),
                    Spec::Index(i, o) => open_index(i, o),
                };
                let kind = k.map_err(|e| eprintln!("resources: {e}")).ok()?;
                let layers = if self.pack { pack_layers(&kind) } else { PackLayers::default() };
                Some(Opened { kind, layers })
            })
            .as_ref()
    }

    /// The filter of this pack hides `path` in the lower packs.
    fn hides(&self, path: &str) -> bool {
        self.pack && self.opened().and_then(|o| o.layers.filter.as_ref()).is_some_and(|f| f.hides_file(path))
    }

    fn has_filter(&self) -> bool {
        self.pack && self.opened().is_some_and(|o| o.layers.filter.is_some())
    }

    /// Presence of `path` (overlays included) without reading it.
    fn contains(&self, path: &str) -> bool {
        let Some(o) = self.opened() else { return false };
        o.layers.overlays.iter().any(|d| raw_contains(&o.kind, &format!("{d}{path}"))) || raw_contains(&o.kind, path)
    }

    fn tag(&self) -> &'static str {
        match self.spec {
            Spec::Zip(_) => "zip",
            Spec::Dir(_) => "directory",
            Spec::Index(..) => "index",
        }
    }

    fn read(&self, path: &str) -> Option<Vec<u8>> {
        let got = self.read_inner(path);
        let found = if got.is_some() { "found" } else { "absent" };
        crate::cache::trace(format_args!("read {} {path}: {found} ({})", self.tag(), self.name));
        got
    }

    /// Read: applicable overlays first (the last declared wins), then the root.
    fn read_inner(&self, path: &str) -> Option<Vec<u8>> {
        let o = self.opened()?;
        o.layers
            .overlays
            .iter()
            .find_map(|d| raw_read(&o.kind, &format!("{d}{path}")))
            .or_else(|| raw_read(&o.kind, path))
    }

    /// Paths of the root and the overlays (overlay prefix removed), duplicates possible.
    fn list(&self, prefix: &str, suffix: &str) -> Vec<String> {
        crate::cache::trace(format_args!("list {} {prefix}*{suffix} ({})", self.tag(), self.name));
        let Some(o) = self.opened() else { return Vec::new() };
        let overlays = &o.layers.overlays;
        let keep = |n: &str| -> Option<String> {
            let n = overlays.iter().find_map(|d| n.strip_prefix(d.as_str())).unwrap_or(n);
            (n.starts_with(prefix) && n.ends_with(suffix)).then(|| n.to_owned())
        };
        match &o.kind {
            Kind::Zip(z) => z.file_names().filter_map(|n| keep(&n.ok()?)).collect(),
            Kind::Dir(root) => {
                let mut out = Vec::new();
                walk(root, root, &mut out);
                out.iter().filter_map(|n| keep(n)).collect()
            }
            Kind::Index { map, .. } => map.keys().filter_map(|n| keep(n)).collect(),
        }
    }
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<String>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() {
            walk(root, &p, out);
        } else if let Ok(rel) = p.strip_prefix(root) {
            out.push(rel.to_string_lossy().replace('\\', "/"));
        }
    }
}

/// Stack of sources: the first has priority. In front of it, possibly, the resource
/// cache (`cache.rs`), which is authoritative for the paths and listings it knows (absences
/// included); the rest falls through to the stack (sounds, or a resource the cache forgot).
pub struct Assets {
    /// Stack and recording, shared with the baking views.
    shared: Arc<Shared>,
    store: Option<Arc<crate::cache::Store>>,
    /// Baking view of a section (`cooking_view`): its reads never go into
    /// the snapshot, those of the other views and the other threads keep going there.
    unrecorded: bool,
}

struct Shared {
    sources: Vec<Source>,
    /// Cold launch: reads to put in the cache snapshot.
    recorder: std::sync::Mutex<Option<crate::cache::Recorder>>,
    /// Recording suspended for everyone (old path, `RUSTCRAFT_OPT_RECORDLOCAL=0`).
    paused: std::sync::atomic::AtomicUsize,
}

/// Suspends recording as long as it lives.
pub struct Pause<'a>(&'a std::sync::atomic::AtomicUsize);

impl Drop for Pause<'_> {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::Relaxed);
    }
}

/// Prefix of the stack keys in the snapshot (never a pack path: they start with
/// `assets/`); value: each file framed by its length (u32 LE), top pack first.
const STACK: &str = "pile:";

fn frame(files: &[Vec<u8>]) -> Option<Vec<u8>> {
    if files.is_empty() {
        return None;
    }
    let mut out = Vec::with_capacity(files.iter().map(|f| f.len() + 4).sum());
    for f in files {
        out.extend_from_slice(&(f.len() as u32).to_le_bytes());
        out.extend_from_slice(f);
    }
    Some(out)
}

fn unframe(mut b: &[u8]) -> Vec<Vec<u8>> {
    let mut out = Vec::new();
    while let Some((len, rest)) = b.split_first_chunk::<4>() {
        let n = u32::from_le_bytes(*len) as usize;
        let Some((f, rest)) = rest.split_at_checked(n) else { break };
        out.push(f.to_vec());
        b = rest;
    }
    out
}

/// Files never put in the snapshot (sounds: read on demand in the index).
fn unrecorded(path: &str) -> bool {
    path.ends_with(".ogg")
}

impl Assets {
    pub fn new(sources: Vec<Source>) -> Self {
        let shared =
            Shared { sources, recorder: std::sync::Mutex::new(None), paused: std::sync::atomic::AtomicUsize::new(0) };
        Assets { shared: Arc::new(shared), store: None, unrecorded: false }
    }

    /// Same stack and same cache, reads never recorded (baking of a section, whose
    /// result is cached). The other views and the other threads always record.
    pub fn cooking_view(&self) -> Assets {
        Assets { shared: self.shared.clone(), store: self.store.clone(), unrecorded: true }
    }

    /// Same stack, all sources opened at first access only.
    pub fn lazy(client_jar: &Path, packs: &[PathBuf], index: Option<(&Path, &Path)>) -> Self {
        let mut sources: Vec<Source> = packs.iter().map(|p| Source::lazy(p, None).as_pack()).collect();
        if let Some((i, o)) = index {
            sources.push(Source::lazy(i, Some(o)));
        }
        sources.push(Source::lazy(client_jar, None));
        Assets::new(sources)
    }

    /// Puts the resource cache in front of the stack.
    pub fn with_store(mut self, store: Arc<crate::cache::Store>) -> Self {
        self.store = Some(store);
        self
    }

    /// Starts recording the reads (cold launch).
    pub fn start_recording(&self) {
        if let Ok(mut r) = self.shared.recorder.lock() {
            *r = Some(crate::cache::Recorder::default());
        }
    }

    /// Old path: suspends recording for all threads as long as it lives.
    pub fn pause_recording(&self) -> Pause<'_> {
        self.shared.paused.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Pause(&self.shared.paused)
    }

    pub(crate) fn take_recording(&self) -> Option<crate::cache::Recorder> {
        self.shared.recorder.lock().ok()?.take()
    }

    fn record(&self, f: impl FnOnce(&mut crate::cache::Recorder)) {
        if self.unrecorded || self.shared.paused.load(std::sync::atomic::Ordering::Relaxed) > 0 {
            return;
        }
        if let Ok(mut r) = self.shared.recorder.lock()
            && let Some(r) = r.as_mut()
        {
            f(r);
        }
    }

    pub(crate) fn stack_list(&self, prefix: &str, suffix: &str) -> Vec<String> {
        if !self.shared.sources.iter().any(Source::has_filter) {
            let mut all: Vec<String> = self.shared.sources.iter().flat_map(|s| s.list(prefix, suffix)).collect();
            all.sort_unstable();
            all.dedup();
            return all;
        }
        // From bottom to top: a pack's filter removes what the lower packs listed.
        let mut all = std::collections::BTreeSet::new();
        for s in self.shared.sources.iter().rev() {
            if s.has_filter() {
                all.retain(|p: &String| !s.hides(p));
            }
            all.extend(s.list(prefix, suffix));
        }
        all.into_iter().collect()
    }

    /// Rank (0 = top) of the pack that provides `path`; `None`: absent or hidden by a filter.
    fn rank_of(&self, path: &str) -> Option<usize> {
        for (i, s) in self.shared.sources.iter().enumerate() {
            if s.contains(path) {
                return Some(i);
            }
            if s.hides(path) {
                return None;
            }
        }
        None
    }

    /// First pack that has `path` among `sources` (top to bottom), stopping at the filter.
    fn first_of(sources: &[Source], path: &str) -> Option<Vec<u8>> {
        for s in sources {
            if let Some(b) = s.read(path) {
                return Some(b);
            }
            if s.hides(path) {
                return None;
            }
        }
        None
    }

    /// File of the topmost pack. Metadata `<resource>.mcmeta`: only from the pack of the
    /// resource or a pack above it (a texture replaced by a pack without `.mcmeta` does not keep
    /// the animation of a lower pack); without a base resource, first found.
    pub(crate) fn stack_read(&self, path: &str) -> Option<Vec<u8>> {
        if let Some(base) = path.strip_suffix(".mcmeta")
            && let Some(rank) = self.rank_of(base)
        {
            return Self::first_of(&self.shared.sources[..=rank], path);
        }
        Self::first_of(&self.shared.sources, path)
    }

    /// The file of each pack that has it, top to bottom, stopping at the first pack whose filter
    /// hides it (included).
    pub(crate) fn stack_read_all(&self, path: &str) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        for s in &self.shared.sources {
            out.extend(s.read(path));
            if s.hides(path) {
                break;
            }
        }
        out
    }

    /// Read of a snapshot key: simple path, or framed stack ([`STACK`]).
    pub(crate) fn stack_read_key(&self, key: &str) -> Option<Vec<u8>> {
        match key.strip_prefix(STACK) {
            Some(path) => frame(&self.stack_read_all(path)),
            None => self.stack_read(key),
        }
    }

    /// Like [`Assets::read`], the file of each pack that has it (top pack first):
    /// stacked definitions (fonts, blockstates).
    pub fn read_all(&self, path: &str) -> Vec<Vec<u8>> {
        self.read(&format!("{STACK}{path}")).map(|b| unframe(&b)).unwrap_or_default()
    }

    /// Decoded image (RGBA) of a PNG; taken from the cache without decoding when it knows it.
    pub fn image(&self, path: &str) -> Option<crate::texture::Image> {
        use crate::cache::Hit;
        if let Some(s) = &self.store {
            match s.image(path) {
                Hit::Found(i) => return Some(i),
                Hit::Absent => return None,
                Hit::Unknown => crate::cache::trace(format_args!("cache: unknown image {path}")),
            }
        }
        let img = self.stack_read(path).and_then(|b| crate::texture::Image::decode_png(&b));
        if !unrecorded(path) {
            self.record(|r| {
                let v = img.clone().map_or(crate::cache::Snap::Absent, crate::cache::Snap::Image);
                r.files.insert(path.to_owned(), v);
            });
        }
        img
    }

    /// Client jar of the installed version, plus the given packs (decreasing priority).
    /// `index`: launcher asset index and its objects folder (Unicode fonts, languages, sounds),
    /// placed before the jar like in the game; ignored if unreadable.
    pub fn for_install(client_jar: &Path, packs: &[PathBuf], index: Option<(&Path, &Path)>) -> Result<Self> {
        let mut sources = Vec::new();
        for p in packs {
            sources.push(if p.is_dir() { Source::dir(p) } else { Source::zip(p)? }.as_pack());
        }
        if let Some(src) = index.and_then(|(i, o)| Source::index(i, o).ok()) {
            sources.push(src);
        }
        sources.push(Source::zip(client_jar)?);
        Ok(Assets::new(sources))
    }

    pub fn read(&self, path: &str) -> Option<Vec<u8>> {
        use crate::cache::Hit;
        if let Some(s) = &self.store {
            match s.read(path) {
                Hit::Found(b) => return Some(b),
                Hit::Absent => return None,
                Hit::Unknown if unrecorded(path) => {}
                Hit::Unknown => crate::cache::trace(format_args!("cache: unknown file {path}")),
            }
        }
        let got = self.stack_read_key(path);
        if !unrecorded(path) {
            self.record(|r| {
                let v = got.clone().map_or(crate::cache::Snap::Absent, crate::cache::Snap::Raw);
                r.files.insert(path.to_owned(), v);
            });
        }
        got
    }

    /// Paths present in at least one source, without duplicates.
    pub fn list(&self, prefix: &str, suffix: &str) -> Vec<String> {
        if let Some(l) = self.store.as_ref().and_then(|s| s.list(prefix, suffix)) {
            return l;
        }
        let all = self.stack_list(prefix, suffix);
        self.record(|r| {
            r.lists.insert((prefix.to_owned(), suffix.to_owned()), all.clone());
        });
        all
    }

    pub fn source_names(&self) -> impl Iterator<Item = &str> {
        self.shared.sources.iter().map(|s| s.name.as_str())
    }
}

/// `minecraft:block/stone` → (`minecraft`, `block/stone`); without namespace → `minecraft`.
pub fn split_id(id: &str) -> (&str, &str) {
    id.split_once(':').unwrap_or(("minecraft", id))
}

/// Test packs written in the temporary folder (the worktree's under the build queue).
#[cfg(test)]
pub(crate) mod testkit {
    use std::path::PathBuf;

    pub(crate) fn dir(name: &str, files: &[(&str, &[u8])]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("rc-assets-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for (p, b) in files {
            let f = root.join(p);
            std::fs::create_dir_all(f.parent().expect("parent")).expect("directory");
            std::fs::write(f, b).expect("write");
        }
        std::fs::create_dir_all(&root).expect("directory");
        root
    }

    /// Stack: packs (top to bottom) above a base that plays the jar.
    pub(crate) fn stack(packs: &[&PathBuf], base: &std::path::Path) -> super::Assets {
        let mut s: Vec<super::Source> = packs.iter().map(|p| super::Source::dir(p).as_pack()).collect();
        s.push(super::Source::dir(base));
        super::Assets::new(s)
    }
}

#[cfg(test)]
mod tests {
    use super::testkit::{dir, stack};
    use super::*;

    const PNG: &str = "assets/minecraft/textures/block/x.png";
    const META: &str = "assets/minecraft/textures/block/x.png.mcmeta";

    #[test]
    fn metadata_never_comes_from_below_the_resource() {
        let base = dir("meta-base", &[(PNG, b"base"), (META, b"anim")]);
        // The pack replaces the image without meta: the animation of the base does not apply.
        let pack = dir("meta-pack", &[(PNG, b"fixed")]);
        let a = stack(&[&pack], &base);
        assert_eq!(a.read(PNG).as_deref(), Some(&b"fixed"[..]));
        assert_eq!(a.read(META), None);
        // Meta alone in a pack above the image: it applies.
        let only_meta = dir("meta-only", &[(META, b"top")]);
        let a = stack(&[&only_meta], &base);
        assert_eq!(a.read(META).as_deref(), Some(&b"top"[..]));
        // File without a base resource (pack meta): first found.
        let b2 = dir("meta-root", &[("assets/pack.mcmeta", b"lang")]);
        assert_eq!(stack(&[&pack], &b2).read("assets/pack.mcmeta").as_deref(), Some(&b"lang"[..]));
    }

    #[test]
    fn stacked_reads_top_first() {
        let base = dir("all-base", &[("assets/minecraft/font/default.json", b"bottom")]);
        let mid = dir("all-mid", &[("assets/minecraft/font/other.json", b"x")]);
        let top = dir("all-top", &[("assets/minecraft/font/default.json", b"top")]);
        let a = stack(&[&top, &mid], &base);
        assert_eq!(a.read_all("assets/minecraft/font/default.json"), vec![b"top".to_vec(), b"bottom".to_vec()]);
        assert!(a.read_all("assets/minecraft/font/absent.json").is_empty());
        let framed = frame(&[b"a".to_vec(), Vec::new(), b"bcd".to_vec()]).expect("stack");
        assert_eq!(unframe(&framed), vec![b"a".to_vec(), Vec::new(), b"bcd".to_vec()]);
        assert_eq!(frame(&[]), None);
    }

    #[test]
    fn overlays_win_over_the_pack_root_in_declared_order() {
        let meta = br#"{"pack":{"description":"t","min_format":97,"max_format":97},
            "overlays":{"entries":[
                {"directory":"ov_a","min_format":97,"max_format":98},
                {"directory":"ov_b","min_format":[97,1],"max_format":97},
                {"directory":"ov_old","formats":[40,50]}]}}"#;
        // `ov_old` targets old versions: all the entries must then have `formats`.
        assert_eq!(crate::packs::overlays(&crate::packs::meta_json(meta).expect("json")), None);
        let meta = br#"{"pack":{"description":"t","min_format":97,"max_format":97},
            "overlays":{"entries":[
                {"directory":"ov_a","min_format":97,"max_format":98},
                {"directory":"ov_b","min_format":[97,1],"max_format":97},
                {"directory":"ov_c","min_format":[97,2],"max_format":99}]}}"#;
        let base = dir("ov-base", &[(PNG, b"base")]);
        let pack = dir(
            "ov-pack",
            &[
                ("pack.mcmeta", meta),
                (PNG, b"root"),
                ("ov_a/assets/minecraft/textures/block/x.png", b"a"),
                ("ov_a/assets/minecraft/textures/block/y.png", b"y"),
                ("ov_b/assets/minecraft/textures/block/x.png", b"b"),
                ("ov_c/assets/minecraft/textures/block/x.png", b"c"),
            ],
        );
        let a = stack(&[&pack], &base);
        // ov_c does not target 97.1; ov_b, declared after ov_a, wins.
        assert_eq!(a.read(PNG).as_deref(), Some(&b"b"[..]));
        assert_eq!(a.read("assets/minecraft/textures/block/y.png").as_deref(), Some(&b"y"[..]));
        let l = a.list("assets/minecraft/textures/block/", ".png");
        assert_eq!(l, ["assets/minecraft/textures/block/x.png", "assets/minecraft/textures/block/y.png"]);
    }

    #[test]
    fn filter_hides_lower_packs_only() {
        let base = dir(
            "flt-base",
            &[(PNG, b"base"), ("assets/minecraft/textures/block/z.png", b"z"), ("assets/other/textures/block/x.png", b"o")],
        );
        let meta = br#"{"pack":{"description":"t","min_format":97,"max_format":97},
            "filter":{"block":[{"namespace":"minecraft","path":"block/x"}]}}"#;
        let pack = dir("flt-pack", &[("pack.mcmeta", meta)]);
        let mine = dir("flt-mine", &[("pack.mcmeta", meta), (PNG, b"mine")]);
        let a = stack(&[&pack], &base);
        assert_eq!(a.read(PNG), None);
        assert!(a.read("assets/minecraft/textures/block/z.png").is_some());
        // Namespace not targeted: nothing hidden.
        assert!(a.read("assets/other/textures/block/x.png").is_some());
        assert_eq!(a.list("assets/minecraft/textures/block/", ".png"), ["assets/minecraft/textures/block/z.png"]);
        // The content of the filtering pack stays visible, and the stack stops at it.
        let a = stack(&[&mine], &base);
        assert_eq!(a.read(PNG).as_deref(), Some(&b"mine"[..]));
        assert_eq!(a.read_all(PNG), vec![b"mine".to_vec()]);
        // Invalid pattern: section ignored.
        let bad = dir(
            "flt-bad",
            &[("pack.mcmeta", br#"{"pack":{"description":"t"},"filter":{"block":[{"path":"("}]}}"#)],
        );
        assert!(stack(&[&bad], &base).read(PNG).is_some());
    }
}
