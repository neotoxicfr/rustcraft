//! Generation data read at run time: JSON `data/<ns>/worldgen/**` of the jar (or of an extracted
//! folder) and official reports `reports/biome_parameters/**` (the game's data generator,
//! launched by rc-data).

use hashbrown::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use crate::Error;
use crate::json::{self, Json};

/// Raw files, indexed by `<type>/<ns>:<path without .json>` (e.g. `noise/minecraft:offset`,
/// `density_function/minecraft:overworld/continents`, `biome_parameters/minecraft:overworld`,
/// `tags/block/minecraft:uncarvable`). All the `worldgen/` folders are kept, plus the
/// block tags.
#[derive(Debug, Default, Clone)]
pub struct DataPack {
    /// Texts borrowed from the process's jar index (`rc_data::jar`), without copy.
    files: HashMap<String, Arc<str>>,
}

/// Index of the process's jar, errors converted (old form of the messages).
pub(crate) fn jar_index(jar: &Path) -> Result<Arc<rc_data::jar::JarIndex>, Error> {
    rc_data::jar::index(jar).map_err(|e| match e {
        rc_data::jar::JarError::Io(e) => Error::Io(jar.display().to_string(), e),
        rc_data::jar::JarError::Zip(e) => Error::Format(format!("{} : {e}", jar.display())),
    })
}

fn classify(entry: &str) -> Option<String> {
    let rest = entry.strip_prefix("data/")?;
    let (ns, rest) = rest.split_once('/')?;
    let (kind, name) = if let Some(r) = rest.strip_prefix("worldgen/") {
        r.split_once('/')?
    } else {
        ("tags/block", rest.strip_prefix("tags/block/")?)
    };
    let name = name.strip_suffix(".json")?;
    Some(format!("{kind}/{ns}:{name}"))
}

impl DataPack {
    /// Reads the generation JSON of the client (or server) jar: from the process's shared index
    /// (default), or by re-reading the jar (`RUSTCRAFT_OPT_JARINDEX=0`).
    pub fn from_jar(jar: &Path) -> Result<Self, Error> {
        if rc_data::jar::enabled() { Self::from_index(jar) } else { Self::from_jar_direct(jar) }
    }

    pub fn from_index(jar: &Path) -> Result<Self, Error> {
        let i = jar_index(jar)?;
        if let Some(b) = i.broken() {
            return Err(Error::Format(b.to_owned()));
        }
        if let Some((name, e)) = i.failed().iter().find(|(n, _)| classify(n).is_some()) {
            return Err(Error::Io(name.clone(), std::io::Error::other(e.clone())));
        }
        let mut files = HashMap::new();
        for (name, blob) in i.with_prefix("data/") {
            let Some(key) = classify(name) else { continue };
            let text = blob.text().ok_or_else(|| {
                Error::Io(name.clone(), std::io::Error::new(std::io::ErrorKind::InvalidData, "invalid UTF-8"))
            })?;
            files.insert(key, text);
        }
        if files.is_empty() {
            return Err(Error::Missing(format!("no generation data in {}", jar.display())));
        }
        Ok(DataPack { files })
    }

    /// Old path: the jar re-read, each entry opened.
    pub fn from_jar_direct(jar: &Path) -> Result<Self, Error> {
        let f = std::fs::File::open(jar).map_err(|e| Error::Io(jar.display().to_string(), e))?;
        let mut z = zip::ZipArchive::new(f).map_err(|e| Error::Format(format!("{} : {e}", jar.display())))?;
        let mut files = HashMap::new();
        for i in 0..z.len() {
            let mut e = z.by_index(i).map_err(|e| Error::Format(e.to_string()))?;
            let Ok(name) = e.name().map(|n| n.into_owned()) else { continue };
            let Some(key) = classify(&name) else { continue };
            let mut text = String::new();
            e.read_to_string(&mut text).map_err(|e| Error::Io(name.clone(), e))?;
            files.insert(key, Arc::from(text));
        }
        if files.is_empty() {
            return Err(Error::Missing(format!("no generation data in {}", jar.display())));
        }
        Ok(DataPack { files })
    }

    /// Reads the JSON of a root folder containing `data/` (extracted jar).
    pub fn from_dir(root: &Path) -> Result<Self, Error> {
        let mut files = HashMap::new();
        let mut stack = vec![root.join("data")];
        while let Some(dir) = stack.pop() {
            let rd = std::fs::read_dir(&dir).map_err(|e| Error::Io(dir.display().to_string(), e))?;
            for ent in rd.flatten() {
                let p = ent.path();
                if p.is_dir() {
                    stack.push(p);
                    continue;
                }
                let Ok(rel) = p.strip_prefix(root) else { continue };
                let rel = rel.to_string_lossy().replace('\\', "/");
                if let Some(key) = classify(&rel) {
                    let text = std::fs::read_to_string(&p).map_err(|e| Error::Io(p.display().to_string(), e))?;
                    files.insert(key, Arc::from(text));
                }
            }
        }
        if files.is_empty() {
            return Err(Error::Missing(format!("no generation data under {}", root.display())));
        }
        Ok(DataPack { files })
    }

    /// Adds the reports `biome_parameters/<ns>/<name>.json` of a `reports/` folder.
    pub fn add_reports(&mut self, reports: &Path) -> Result<(), Error> {
        let base = reports.join("biome_parameters");
        let rd = std::fs::read_dir(&base).map_err(|e| Error::Io(base.display().to_string(), e))?;
        for ns in rd.flatten() {
            let Ok(files) = std::fs::read_dir(ns.path()) else { continue };
            let nsn = ns.file_name().to_string_lossy().into_owned();
            for f in files.flatten() {
                let p = f.path();
                let Some(stem) = p.file_name().and_then(|n| n.to_str()).and_then(|n| n.strip_suffix(".json")) else {
                    continue;
                };
                let text = std::fs::read_to_string(&p).map_err(|e| Error::Io(p.display().to_string(), e))?;
                self.files.insert(format!("biome_parameters/{nsn}:{stem}"), Arc::from(text));
            }
        }
        Ok(())
    }

    /// Adds or replaces a file (tests, data packs).
    pub fn insert(&mut self, key: &str, text: String) {
        self.files.insert(key.to_owned(), Arc::from(text));
    }

    pub fn raw(&self, kind: &str, id: &str) -> Option<&str> {
        with_file_key(kind, id, |k| self.files.get(k)).map(|t| &**t)
    }

    pub fn json(&self, kind: &str, id: &str) -> Result<Json, Error> {
        let text = self.raw(kind, id).ok_or_else(|| Error::Missing(format!("{kind}/{id}")))?;
        json::parse(text).map_err(|e| Error::Format(format!("{kind}/{id} : {e}")))
    }

    /// Identifiers (`ns:path`) of a type, sorted.
    pub fn ids(&self, kind: &str) -> Vec<String> {
        let prefix = format!("{kind}/");
        let mut v: Vec<String> = self.files.keys().filter_map(|k| k.strip_prefix(&prefix)).map(str::to_owned).collect();
        v.sort();
        v
    }

    /// All the files, sorted (equivalence proofs).
    #[cfg(test)]
    pub(crate) fn entries(&self) -> std::collections::BTreeMap<&str, &[u8]> {
        self.files.iter().map(|(k, v)| (k.as_str(), v.as_bytes())).collect()
    }

    pub fn len(&self) -> usize {
        self.files.len()
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

/// `x` → `minecraft:x`.
pub fn full_id(id: &str) -> String {
    if id.contains(':') { id.to_owned() } else { format!("minecraft:{id}") }
}

/// Calls `f` with the full identifier of `id`, for a lookup: without allocation
/// (`RUSTCRAFT_OPT_TAG_BORROW`, default) or through [`full_id`] (old path).
#[inline]
pub fn with_full_id<R>(id: &str, f: impl FnOnce(&str) -> R) -> R {
    with_full_id_mode(id, crate::opt::tag_borrow(), f)
}

/// [`with_full_id`] on the chosen path (equivalence proofs). Name already full: borrowed; otherwise
/// `minecraft:` and the name composed in a stack buffer, a string allocated only beyond.
#[inline]
pub fn with_full_id_mode<R>(id: &str, borrow: bool, f: impl FnOnce(&str) -> R) -> R {
    rc_data::ids::with_full_id_mode(id, borrow, f)
}

/// Calls `f` with the key `<kind>/<full identifier>` of the file indexes, composed on the
/// stack (`RUSTCRAFT_OPT_TAG_BORROW`, default) or allocated (old path).
#[inline]
pub fn with_file_key<R>(kind: &str, id: &str, f: impl FnOnce(&str) -> R) -> R {
    with_file_key_mode(kind, id, crate::opt::tag_borrow(), f)
}

/// [`with_file_key`] on the chosen path (equivalence proofs).
pub fn with_file_key_mode<R>(kind: &str, id: &str, borrow: bool, f: impl FnOnce(&str) -> R) -> R {
    let ns = if id.contains(':') { "" } else { "minecraft:" };
    rc_data::ids::with_key_mode(&[kind, "/", ns, id], borrow, f)
}
