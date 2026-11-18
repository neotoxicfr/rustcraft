//! Decoration data read from the jar (or an extracted folder): configured and placed features,
//! biomes, named state providers, block tags (`generation-decoration.md` § 2–6).

use hashbrown::HashMap;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;

use crate::Error;
use crate::json::{self, Json};

/// Folders kept under `data/<ns>/`.
const KINDS: [&str; 12] = [
    "worldgen/feature/",
    "worldgen/placed_feature/",
    "worldgen/biome/",
    "worldgen/block_state_provider/",
    "worldgen/structure/",
    "worldgen/structure_set/",
    "worldgen/template_pool/",
    "worldgen/processor_list/",
    "tags/block/",
    "tags/fluid/",
    "tags/worldgen/biome/",
    "structure/",
];

/// Raw files indexed by `<type>/<ns>:<path without extension>` (type = `feature`,
/// `placed_feature`, `biome`, `block_state_provider`, `structure`, `structure_set`,
/// `template_pool`, `processor_list`, `tags_block`, `tags_fluid`, `tags_worldgen_biome`, and
/// `template` for the `structure/*.nbt` templates).
#[derive(Debug, Default, Clone)]
pub struct DecorData {
    /// Bytes borrowed from the process's jar index (`rc_data::jar`), without copy.
    files: HashMap<String, Arc<[u8]>>,
}

fn classify(entry: &str) -> Option<String> {
    let rest = entry.strip_prefix("data/")?;
    let (ns, rest) = rest.split_once('/')?;
    for k in KINDS {
        if let Some(name) = rest.strip_prefix(k) {
            if k == "structure/" {
                return Some(format!("template/{ns}:{}", name.strip_suffix(".nbt")?));
            }
            let name = name.strip_suffix(".json")?;
            let kind = k.trim_end_matches('/').trim_start_matches("worldgen/").replace('/', "_");
            return Some(format!("{kind}/{ns}:{name}"));
        }
    }
    None
}

impl DecorData {
    /// From the process's shared index (default), or by re-reading the jar
    /// (`RUSTCRAFT_OPT_JARINDEX=0`).
    pub fn from_jar(jar: &Path) -> Result<Self, Error> {
        if rc_data::jar::enabled() { Self::from_index(jar) } else { Self::from_jar_direct(jar) }
    }

    pub fn from_index(jar: &Path) -> Result<Self, Error> {
        let i = crate::data::jar_index(jar)?;
        if let Some(b) = i.broken() {
            return Err(Error::Format(b.to_owned()));
        }
        if let Some((name, e)) = i.failed().iter().find(|(n, _)| classify(n).is_some()) {
            return Err(Error::Io(name.clone(), std::io::Error::other(e.clone())));
        }
        let files: HashMap<String, Arc<[u8]>> =
            i.with_prefix("data/").filter_map(|(name, blob)| Some((classify(name)?, blob.bytes()))).collect();
        if !files.keys().any(|k| k.starts_with("feature/")) {
            return Err(Error::Missing(format!("no decoration feature in {}", jar.display())));
        }
        Ok(DecorData { files })
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
            let mut buf = Vec::with_capacity(e.size() as usize);
            e.read_to_end(&mut buf).map_err(|e| Error::Io(name.clone(), e))?;
            files.insert(key, Arc::from(buf));
        }
        if !files.keys().any(|k| k.starts_with("feature/")) {
            return Err(Error::Missing(format!("no decoration feature in {}", jar.display())));
        }
        Ok(DecorData { files })
    }

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
                    let b = std::fs::read(&p).map_err(|e| Error::Io(p.display().to_string(), e))?;
                    files.insert(key, Arc::from(b));
                }
            }
        }
        Ok(DecorData { files })
    }

    /// All the files, sorted (equivalence proofs).
    #[cfg(test)]
    pub(crate) fn entries(&self) -> std::collections::BTreeMap<&str, &[u8]> {
        self.files.iter().map(|(k, v)| (k.as_str(), &**v)).collect()
    }

    pub fn insert(&mut self, key: &str, text: String) {
        self.files.insert(key.to_owned(), Arc::from(text.into_bytes()));
    }

    /// All the files of a type: `ns:path` → bytes.
    /// Shared bytes (default); `RUSTCRAFT_OPT_TEMPLATES_SHARE=0`: deep copy (old path).
    pub fn files_of(&self, kind: &str) -> HashMap<String, Arc<[u8]>> {
        static SHARE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
        let share = *SHARE.get_or_init(|| rc_data::env_str!("RUSTCRAFT_OPT_TEMPLATES_SHARE") != Some("0"));
        let prefix = format!("{kind}/");
        self.files
            .iter()
            .filter_map(|(k, v)| {
                let v = if share { v.clone() } else { Arc::from(v.to_vec()) };
                Some((k.strip_prefix(&prefix)?.to_owned(), v))
            })
            .collect()
    }

    pub fn raw(&self, kind: &str, id: &str) -> Option<&[u8]> {
        crate::data::with_file_key(kind, id, |k| self.files.get(k)).map(|b| &**b)
    }

    pub fn json(&self, kind: &str, id: &str) -> Result<Json, Error> {
        let b = self.raw(kind, id).ok_or_else(|| Error::Missing(format!("{kind}/{id}")))?;
        let text = std::str::from_utf8(b).map_err(|e| Error::Format(format!("{kind}/{id} : {e}")))?;
        json::parse(text).map_err(|e| Error::Format(format!("{kind}/{id} : {e}")))
    }

    /// Identifiers (`ns:path`) of a type, sorted.
    pub fn ids(&self, kind: &str) -> Vec<String> {
        let prefix = format!("{kind}/");
        let mut v: Vec<String> = self.files.keys().filter_map(|k| k.strip_prefix(&prefix)).map(str::to_owned).collect();
        v.sort();
        v
    }

    /// Raw block tags: full name → entries (`#` for a tag), required or not.
    pub fn block_tags(&self) -> HashMap<String, Vec<(String, bool)>> {
        self.tags("tags_block")
    }

    pub fn fluid_tags(&self) -> HashMap<String, Vec<(String, bool)>> {
        self.tags("tags_fluid")
    }

    fn tags(&self, kind: &str) -> HashMap<String, Vec<(String, bool)>> {
        let mut out = HashMap::new();
        for id in self.ids(kind) {
            let Ok(j) = self.json(kind, &id) else { continue };
            let mut v = Vec::new();
            for e in j.get("values").and_then(Json::as_array).unwrap_or(&[]) {
                match e {
                    Json::Str(s) => v.push((full(s), true)),
                    Json::Obj(_) => {
                        if let Some(s) = e.get("id").and_then(Json::as_str) {
                            v.push((full(s), e.get("required").and_then(Json::as_bool).unwrap_or(true)));
                        }
                    }
                    _ => {}
                }
            }
            out.insert(id, v);
        }
        out
    }
}

fn full(s: &str) -> String {
    match s.strip_prefix('#') {
        Some(t) => format!("#{}", crate::data::full_id(t)),
        None => crate::data::full_id(s),
    }
}
