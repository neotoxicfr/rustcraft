//! Resource packs (`docs/specs/resource-packs.md`): `pack.mcmeta` meta, compatibility with
//! the version's format, discovery of the `resourcepacks` folder, files at the root of a pack.

use std::io::Read;
use std::path::{Path, PathBuf};

use serde_json::Value;

/// Resource format of 26.3 (major, minor).
pub const RESOURCE_FORMAT: (u32, u32) = (97, 1);

/// Compatibility of a pack with `RESOURCE_FORMAT` (§ 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Compat {
    Compatible,
    TooOld,
    TooNew,
    Unknown,
}

impl Compat {
    pub fn ok(self) -> bool {
        self == Compat::Compatible
    }

    /// Suffix of the keys `pack.incompatible.<…>` and `pack.incompatible.confirm.<…>`.
    pub fn key(self) -> &'static str {
        match self {
            Compat::Compatible => "compatible",
            Compat::TooOld => "old",
            Compat::TooNew => "new",
            Compat::Unknown => "unknown",
        }
    }
}

/// Useful content of `pack.mcmeta`.
#[derive(Debug, Clone, PartialEq)]
pub struct PackMeta {
    /// Description: JSON text (string or component).
    pub description: Value,
    pub compat: Compat,
}

type Format = (u32, u32);

fn int(v: &Value) -> Option<u32> {
    v.as_u64().and_then(|n| u32::try_from(n).ok())
}

/// Integer `M` (minor by default) or list `[M, m]`.
fn format(v: &Value, default_minor: u32) -> Option<Format> {
    match v {
        Value::Array(a) => match a.as_slice() {
            [m] => Some((int(m)?, default_minor)),
            [m, n, ..] => Some((int(m)?, int(n)?)),
            [] => None,
        },
        v => Some((int(v)?, default_minor)),
    }
}

/// `supported_formats`: integer, `[a, b]` or `{min_inclusive, max_inclusive}` (majors).
fn majors(v: &Value) -> Option<(u32, u32)> {
    match v {
        Value::Array(a) => match a.as_slice() {
            [x] => Some((int(x)?, int(x)?)),
            [x, y] => Some((int(x)?, int(y)?)),
            _ => None,
        },
        Value::Object(o) => Some((int(o.get("min_inclusive")?)?, int(o.get("max_inclusive")?)?)),
        v => int(v).map(|x| (x, x)),
    }
}

/// Range declared by the `pack` object; `None`: nothing readable.
fn declared(pack: &serde_json::Map<String, Value>) -> Option<(Format, Format)> {
    let (min, max) = (pack.get("min_format"), pack.get("max_format"));
    if min.is_some() || max.is_some() {
        let lo = format(min?, 0)?;
        let hi = format(max?, u32::MAX)?;
        return (lo <= hi).then_some((lo, hi));
    }
    if let Some(s) = pack.get("supported_formats") {
        let (a, b) = majors(s)?;
        return (a <= b).then_some(((a, 0), (b, u32::MAX)));
    }
    let f = int(pack.get("pack_format")?)?;
    // Beyond 64, the min/max fields are mandatory.
    (f <= 64).then_some(((f, 0), (f, 0)))
}

/// Compatibility of a declared range with the version's format.
pub fn compat((lo, hi): (Format, Format), game: Format) -> Compat {
    if hi < game {
        Compat::TooOld
    } else if game < lo {
        Compat::TooNew
    } else {
        Compat::Compatible
    }
}

/// Last major format without a minor number (resource packs).
const LAST_PRE_MINOR: u32 = 64;

/// Overlay entry read, before validation.
struct OverlayEntry {
    min: Option<Format>,
    max: Option<Format>,
    formats: Option<(u32, u32)>,
    dir: String,
}

fn overlay_entry(v: &Value) -> Option<OverlayEntry> {
    let o = v.as_object()?;
    let dir = o.get("directory")?.as_str()?;
    let valid = !dir.is_empty() && dir.bytes().all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b));
    if !valid {
        return None;
    }
    // Field present but unreadable: error (no silent absence).
    let opt = |k: &str, f: &dyn Fn(&Value) -> Option<Format>| match o.get(k) {
        None => Some(None),
        Some(v) => f(v).map(Some),
    };
    let min = opt("min_format", &|v| format(v, 0))?;
    let max = opt("max_format", &|v| format(v, u32::MAX))?;
    let formats = match o.get("formats") {
        None => None,
        Some(v) => Some(majors(v).filter(|(a, b)| a <= b)?),
    };
    Some(OverlayEntry { min, max, formats, dir: dir.to_owned() })
}

/// Overlay folders of `pack.mcmeta` applicable to the version's format, decreasing
/// priority (the last of the list first). `None`: invalid section, and then the whole
/// pack meta is unreadable (pack ignored), like the game.
pub fn overlays(meta: &Value) -> Option<Vec<String>> {
    let Some(section) = meta.get("overlays") else { return Some(Vec::new()) };
    let entries: Vec<OverlayEntry> =
        section.get("entries")?.as_array()?.iter().map(overlay_entry).collect::<Option<_>>()?;
    let effective_min = |e: &OverlayEntry| match (e.min, e.formats) {
        (Some(m), Some(f)) => m.0.min(f.0),
        (Some(m), None) => m.0,
        (None, Some(f)) => f.0,
        (None, None) => u32::MAX,
    };
    // An entry targeting old versions imposes the old `formats` field on all.
    let require_old = entries.iter().map(effective_min).min().unwrap_or(u32::MAX) <= LAST_PRE_MINOR;
    let mut out = Vec::new();
    for e in &entries {
        if e.min.is_none() && e.max.is_none() && e.formats.is_none() {
            continue; // entry without version: ignored
        }
        if e.min.is_some() != e.max.is_some() || (require_old && e.formats.is_none()) {
            return None;
        }
        let range = match (e.min, e.max) {
            (Some(lo), Some(hi)) => {
                if lo > hi {
                    return None;
                }
                if lo.0 > LAST_PRE_MINOR && !require_old {
                    if e.formats.is_some() {
                        return None;
                    }
                } else {
                    let f = e.formats?;
                    if f.0 != lo.0 || (f.1 != hi.0 && f.1 != LAST_PRE_MINOR) {
                        return None;
                    }
                }
                (lo, hi)
            }
            _ => {
                let f = e.formats?;
                if f.1 > LAST_PRE_MINOR {
                    return None;
                }
                ((f.0, 0), (f.1, 0))
            }
        };
        if compat(range, RESOURCE_FORMAT).ok() {
            out.push(e.dir.clone());
        }
    }
    out.reverse();
    Some(out)
}

pub use rc_data::pack_filter::{Filter, filter};

/// JSON of `pack.mcmeta` (byte order mark tolerated).
pub fn meta_json(bytes: &[u8]) -> Option<Value> {
    let text = String::from_utf8_lossy(bytes);
    serde_json::from_str(text.trim_start_matches('\u{feff}')).ok()
}

/// Reads `pack.mcmeta`; `None`: invalid pack (ignored). Description only: unknown compatibility.
pub fn parse_meta(bytes: &[u8]) -> Option<PackMeta> {
    let v = meta_json(bytes)?;
    overlays(&v)?;
    let pack = v.get("pack")?.as_object()?;
    let description = pack.get("description")?.clone();
    let compat = match declared(pack) {
        Some(range) => compat(range, RESOURCE_FORMAT),
        None => Compat::Unknown,
    };
    Some(PackMeta { description, compat })
}

/// File `name` at the root of a pack (folder or zip).
pub fn read_root(pack: &Path, name: &str) -> Option<Vec<u8>> {
    if pack.is_dir() {
        return std::fs::read(pack.join(name)).ok();
    }
    let file = std::fs::File::open(pack).ok()?;
    let mut zip = zip::ZipArchive::new(std::io::BufReader::new(file)).ok()?;
    let mut e = zip.by_name(name).ok()?;
    let mut out = Vec::with_capacity(e.size().min(1 << 20) as usize);
    e.read_to_end(&mut out).ok()?;
    Some(out)
}

/// Pack found in a folder.
#[derive(Debug, Clone)]
pub struct FoundPack {
    /// `file/<name>`.
    pub id: String,
    /// Name of the file or folder.
    pub name: String,
    pub path: PathBuf,
    pub meta: PackMeta,
}

/// Packs of the `resourcepacks` folder (§ 2): folders with `pack.mcmeta` and `.zip` files, sorted
/// by identifier; unreadable meta → pack ignored. Symbolic links checked against `allow`
/// (linked entry, or link inside a pack folder, refused → pack ignored); an allowed link is
/// followed (identifier and name: those of the link).
pub fn discover(dir: &Path, allow: &rc_data::symlinks::SymlinkRules) -> Vec<FoundPack> {
    let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
    let mut out: Vec<FoundPack> = rd
        .flatten()
        .filter_map(|e| {
            let content = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            let mut refused = Vec::new();
            let path = match allow.pack_entry(&content, &mut refused) {
                Ok(p) => p,
                Err(err) => {
                    eprintln!("resource pack ignored ({err}): {}", content.display());
                    return None;
                }
            };
            if let Some(f) = refused.first() {
                eprintln!(
                    "resource pack ignored (symbolic link refused {} -> {}): {}",
                    f.link.display(),
                    f.target.display(),
                    content.display()
                );
                return None;
            }
            let path = path?;
            let meta = read_root(&path, "pack.mcmeta").and_then(|b| parse_meta(&b));
            let Some(meta) = meta else {
                eprintln!("resource pack ignored (unreadable meta): {}", path.display());
                return None;
            };
            Some(FoundPack { id: format!("file/{name}"), name, path, meta })
        })
        .collect();
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta(s: &str) -> Option<PackMeta> {
        parse_meta(s.as_bytes())
    }

    #[test]
    fn formats() {
        let m = |s: &str| meta(s).map(|m| m.compat);
        assert_eq!(m(r#"{"pack":{"description":"x","min_format":97,"max_format":97}}"#), Some(Compat::Compatible));
        assert_eq!(m(r#"{"pack":{"description":"x","min_format":[97,2],"max_format":99}}"#), Some(Compat::TooNew));
        assert_eq!(m(r#"{"pack":{"description":"x","min_format":90,"max_format":[97,0]}}"#), Some(Compat::TooOld));
        assert_eq!(m(r#"{"pack":{"description":"x","supported_formats":[90,100]}}"#), Some(Compat::Compatible));
        assert_eq!(
            m(r#"{"pack":{"description":"x","supported_formats":{"min_inclusive":98,"max_inclusive":99}}}"#),
            Some(Compat::TooNew)
        );
        assert_eq!(m(r#"{"pack":{"description":"x","pack_format":34}}"#), Some(Compat::TooOld));
        // Beyond 64 without min/max: unreadable → unknown.
        assert_eq!(m(r#"{"pack":{"description":"x","pack_format":97}}"#), Some(Compat::Unknown));
        assert_eq!(m(r#"{"pack":{"description":"x"}}"#), Some(Compat::Unknown));
        assert_eq!(m(r#"{"pack":{"min_format":97}}"#), None);
        assert_eq!(m("not json"), None);
    }

    /// Pack folder: a pack containing a forbidden symbolic link is ignored (the old code
    /// took it), the ordinary pack stays; an allowed linked zip takes the identifier of the link.
    #[test]
    fn discover_refuses_forbidden_links() {
        let root = std::env::temp_dir().join(format!("rc-packs-links-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let packs = root.join("resourcepacks");
        let meta = br#"{"pack":{"description":"x","min_format":97,"max_format":97}}"#;
        for p in ["plain", "linked"] {
            std::fs::create_dir_all(packs.join(p)).expect("directory");
            std::fs::write(packs.join(p).join("pack.mcmeta"), meta).expect("meta");
        }
        let outside = root.join("elsewhere");
        std::fs::create_dir_all(&outside).expect("directory");
        {
            let mut z = zip::ZipWriter::new(std::fs::File::create(outside.join("target.zip")).expect("zip"));
            z.start_file("pack.mcmeta", zip::write::SimpleFileOptions::default()).expect("entry");
            std::io::Write::write_all(&mut z, meta).expect("meta");
            z.finish().expect("zip");
        }
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(&outside, packs.join("linked").join("inside")).is_ok()
            && std::os::windows::fs::symlink_file(outside.join("target.zip"), packs.join("symlink.zip")).is_ok();
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&outside, packs.join("linked").join("inside")).is_ok()
            && std::os::unix::fs::symlink(outside.join("target.zip"), packs.join("symlink.zip")).is_ok();
        if !made {
            eprintln!("symbolic links not allowed on this system: test skipped");
            let _ = std::fs::remove_dir_all(&root);
            return;
        }
        let ids = |allow: &rc_data::symlinks::SymlinkRules| -> Vec<String> {
            discover(&packs, allow).into_iter().map(|p| p.id).collect()
        };
        assert_eq!(ids(&rc_data::symlinks::SymlinkRules::default()), ["file/plain"]);
        let prefix = outside.to_string_lossy().trim_start_matches(r"\\?\").to_owned();
        let allow = rc_data::symlinks::SymlinkRules::parse(&prefix).expect("list");
        assert_eq!(ids(&allow), ["file/linked", "file/plain", "file/symlink.zip"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn overlay_sections() {
        let ov = |s: &str| overlays(&serde_json::from_str(s).expect("json"));
        assert_eq!(ov(r#"{}"#), Some(vec![]));
        // Old versions targeted: `formats` everywhere, consistent with min/max.
        assert_eq!(
            ov(r#"{"overlays":{"entries":[
                {"directory":"a","formats":[50,64],"min_format":50,"max_format":97},
                {"directory":"b","formats":[30,40]},
                {"directory":"c"}]}}"#),
            Some(vec!["a".to_owned()])
        );
        // `formats` that does not start at min_format: invalid section.
        assert_eq!(
            ov(r#"{"overlays":{"entries":[{"directory":"a","formats":[51,64],"min_format":50,"max_format":97}]}}"#),
            None
        );
        // New versions only: `formats` forbidden.
        assert_eq!(
            ov(r#"{"overlays":{"entries":[{"directory":"a","formats":[97,97],"min_format":97,"max_format":97}]}}"#),
            None
        );
        // min without max, invalid folder, absent entries.
        assert_eq!(ov(r#"{"overlays":{"entries":[{"directory":"a","min_format":97}]}}"#), None);
        assert_eq!(ov(r#"{"overlays":{"entries":[{"directory":"a/b","min_format":97,"max_format":97}]}}"#), None);
        assert_eq!(ov(r#"{"overlays":{}}"#), None);
        // Invalid section: pack unreadable, hence ignored.
        assert_eq!(meta(r#"{"pack":{"description":"x"},"overlays":{}}"#), None);
    }
}
