//! Synchronised registries and tags (`integrated-server.md` § 2.2), read from the base pack of the
//! player's jar (`data/minecraft/…`). Order of the entries: that of a vanilla server, checked against
//! the reference capture of the official client (test `registries_match_capture`): sorted
//! identifiers (namespace then path), like the game's resource list.

use hashbrown::HashMap;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use rc_data::VersionData;

use crate::Error;

/// Synchronised registries of 26.3, in the sending order of a vanilla server (capture).
pub const SYNCED: [&str; 32] = [
    "minecraft:worldgen/biome",
    "minecraft:chat_type",
    "minecraft:trim_pattern",
    "minecraft:trim_material",
    "minecraft:wolf_variant",
    "minecraft:wolf_sound_variant",
    "minecraft:pig_variant",
    "minecraft:pig_sound_variant",
    "minecraft:frog_variant",
    "minecraft:cat_variant",
    "minecraft:cat_sound_variant",
    "minecraft:cow_sound_variant",
    "minecraft:cow_variant",
    "minecraft:chicken_sound_variant",
    "minecraft:chicken_variant",
    "minecraft:zombie_nautilus_variant",
    "minecraft:painting_variant",
    "minecraft:sulfur_cube_archetype",
    "minecraft:dimension_type",
    "minecraft:damage_type",
    "minecraft:banner_pattern",
    "minecraft:enchantment",
    "minecraft:jukebox_song",
    "minecraft:instrument",
    "minecraft:test_environment",
    "minecraft:test_instance",
    "minecraft:dialog",
    "minecraft:world_clock",
    "minecraft:timeline",
    "minecraft:decorated_pot_pattern",
    "minecraft:block_transformer",
    "minecraft:worldgen/block_state_provider",
];

pub type Tags = Vec<(String, Vec<(String, Vec<i32>)>)>;

#[derive(Debug, Clone, Default)]
pub struct Registries {
    /// Registry → entries in the order of their identifier.
    pub synced: Vec<(String, Vec<String>)>,
    pub tags: Tags,
}

impl Registries {
    pub fn entries(&self, name: &str) -> &[String] {
        self.synced.iter().find(|(n, _)| n == name).map_or(&[], |(_, e)| e.as_slice())
    }
}

/// Files `data/<namespace>/<folder>/**.json` of the jar: identifier → contents. After
/// `packs::stack`, the stack of the world's chosen packs (`.mcfunction` functions included).
pub(crate) struct Jar {
    /// Bytes borrowed from the process's jar index (`rc_data::jar`), without copy.
    pub(crate) files: BTreeMap<String, std::sync::Arc<[u8]>>,
}

impl Jar {
    /// `data/**.json` files: from the process's shared index (default), or read again from the jar
    /// (`RUSTCRAFT_OPT_JARINDEX=0`). Unreadable entries are ignored in both cases.
    pub(crate) fn read(path: &Path) -> Result<Jar, Error> {
        if rc_data::jar::enabled() { Jar::from_index(path) } else { Jar::read_direct(path) }
    }

    pub(crate) fn from_index(path: &Path) -> Result<Jar, Error> {
        let i = rc_data::jar::index(path).map_err(|e| match e {
            rc_data::jar::JarError::Io(e) => Error::io(path, e),
            rc_data::jar::JarError::Zip(e) => Error::Format(format!("{}: {e}", path.display())),
        })?;
        let files = i.with_prefix("data/").filter(|(k, _)| kept(k)).map(|(k, v)| (k.clone(), v.bytes())).collect();
        Ok(Jar { files })
    }

    /// Old path: the jar read again, each entry opened.
    pub(crate) fn read_direct(path: &Path) -> Result<Jar, Error> {
        let f = std::fs::File::open(path).map_err(|e| Error::io(path, e))?;
        let mut z = zip::ZipArchive::new(f).map_err(|e| Error::Format(format!("{}: {e}", path.display())))?;
        let mut files = BTreeMap::new();
        for i in 0..z.len() {
            let Ok(mut e) = z.by_index(i) else { continue };
            let Ok(name) = e.name().map(|n| n.into_owned()) else { continue };
            if !name.starts_with("data/") || !kept(&name) {
                continue;
            }
            let mut v = Vec::with_capacity(e.size() as usize);
            if e.read_to_end(&mut v).is_ok() {
                files.insert(name, v.into());
            }
        }
        Ok(Jar { files })
    }

    /// Identifiers of the resources of a folder (`worldgen/biome`), sorted.
    fn ids(&self, dir: &str) -> Vec<String> {
        let mut out = Vec::new();
        for name in self.files.keys() {
            let Some(rest) = name.strip_prefix("data/") else { continue };
            let Some((ns, path)) = rest.split_once('/') else { continue };
            let Some(file) = path.strip_prefix(dir).and_then(|p| p.strip_prefix('/')) else { continue };
            out.push(format!("{ns}:{}", file.trim_end_matches(".json")));
        }
        out.sort_by(|a, b| {
            let (an, ap) = a.split_once(':').unwrap_or(("", a));
            let (bn, bp) = b.split_once(':').unwrap_or(("", b));
            (an, ap).cmp(&(bn, bp))
        });
        out
    }
}

/// `data/…` entry kept (shared index as re-read): JSON data, and headers of the jar's
/// function packs (`packs.rs`).
fn kept(name: &str) -> bool {
    name.ends_with(".json") || name.starts_with(crate::packs::BUILTIN_DIR) && name.ends_with("/pack.mcmeta")
}

/// Tags of a registry as numeric identifiers, resolved like the game (`packs::resolve_tags`:
/// references followed, tag refused entirely if a required entry is missing), in the order of the names
/// (namespace then path).
fn resolve_tags(jar: &Jar, registry: &str, ids: &HashMap<&str, i32>) -> Vec<(String, Vec<i32>)> {
    let raw = crate::packs::raw_tags(jar, registry.split_once(':').map_or(registry, |x| x.1));
    let map: BTreeMap<String, &[serde_json::Value]> =
        raw.iter().map(|(n, v)| (n.clone(), v.as_array().map_or(&[][..], Vec::as_slice))).collect();
    let mut out: Vec<(String, Vec<i32>)> =
        crate::packs::resolve_tags(&map, &|id| ids.get(id).copied()).into_iter().collect();
    out.sort_by(|a, b| {
        let (an, ap) = a.0.split_once(':').unwrap_or(("", &a.0));
        let (bn, bp) = b.0.split_once(':').unwrap_or(("", &b.0));
        (an, ap).cmp(&(bn, bp))
    });
    out
}

/// Registries carrying tags sent to the client, in addition to the synchronised registries.
const TAGGED_BUILTIN: [&str; 7] = [
    "minecraft:block",
    "minecraft:item",
    "minecraft:fluid",
    "minecraft:entity_type",
    "minecraft:game_event",
    "minecraft:potion",
    "minecraft:point_of_interest_type",
];

pub fn load(jar_path: &Path, data: &VersionData) -> Result<Registries, Error> {
    Ok(load_jar(&Jar::read(jar_path)?, data))
}

pub(crate) fn load_jar(jar: &Jar, data: &VersionData) -> Registries {
    let mut synced = Vec::new();
    for name in SYNCED {
        let dir = name.split_once(':').map_or(name, |x| x.1);
        // Built-in registry (in code): order of the version data.
        let entries = match data.registry(name) {
            Some(r) if jar.ids(dir).is_empty() => r.entries.clone(),
            _ => jar.ids(dir),
        };
        synced.push((name.to_owned(), entries));
    }
    let tags = tags_for(jar, data, &synced);
    Registries { synced, tags }
}

/// Tags of the `jar` stack resolved against the `synced` registries. On reload
/// (`/reload`, `/datapack`), the game keeps the registries frozen at opening: we then pass
/// those of the opening, an entry added since by a pack is unknown (the tag that
/// requires it is refused) and the numbers stay those announced to the client.
pub(crate) fn tags_for(jar: &Jar, data: &VersionData, synced: &[(String, Vec<String>)]) -> Tags {
    let mut tags = Vec::new();
    for name in TAGGED_BUILTIN {
        let Some(r) = data.registry(name) else { continue };
        let ids: HashMap<&str, i32> = r.entries.iter().enumerate().map(|(i, e)| (e.as_str(), i as i32)).collect();
        tags.push((name.to_owned(), resolve_tags(jar, name, &ids)));
    }
    for (name, entries) in synced {
        let ids: HashMap<&str, i32> = entries.iter().enumerate().map(|(i, e)| (e.as_str(), i as i32)).collect();
        let t = resolve_tags(jar, name, &ids);
        if !t.is_empty() {
            tags.push((name.clone(), t));
        }
    }
    tags
}

/// Player's jar for the version of this data.
pub fn client_jar(data: &VersionData) -> Option<std::path::PathBuf> {
    rc_data::locate(&data.version).ok().map(|i| i.client_jar).filter(|p| p.is_file())
}

#[cfg(test)]
mod tag_bench {
    use super::*;

    /// Resolution from before G3-F03 (missing entries ignored, without refusal), kept for
    /// comparison: same output on the jar alone, duration measured.
    fn previous(jar: &Jar, registry: &str, ids: &HashMap<&str, i32>) -> Vec<(String, Vec<i32>)> {
        let dir = format!("tags/{}", registry.split_once(':').map_or(registry, |x| x.1));
        let json = |ns: &str, id: &str| -> Option<serde_json::Value> {
            serde_json::from_slice(jar.files.get(&format!("data/{ns}/{dir}/{id}.json"))?).ok()
        };
        fn walk(
            json: &dyn Fn(&str, &str) -> Option<serde_json::Value>,
            name: &str,
            ids: &HashMap<&str, i32>,
            memo: &mut HashMap<String, Vec<i32>>,
            depth: usize,
        ) -> Vec<i32> {
            if let Some(v) = memo.get(name) {
                return v.clone();
            }
            let mut out: Vec<i32> = Vec::new();
            let (ns, path) = name.split_once(':').unwrap_or(("minecraft", name));
            if depth < 64
                && let Some(v) = json(ns, path)
                && let Some(values) = v["values"].as_array()
            {
                for e in values {
                    let id = e.as_str().or_else(|| e["id"].as_str()).unwrap_or("");
                    let here = match id.strip_prefix('#') {
                        Some(t) => walk(json, t, ids, memo, depth + 1),
                        None => ids.get(id).map(|&i| vec![i]).unwrap_or_default(),
                    };
                    for i in here {
                        if !out.contains(&i) {
                            out.push(i);
                        }
                    }
                }
            }
            memo.insert(name.to_owned(), out.clone());
            out
        }
        let mut memo = HashMap::new();
        jar.ids(&dir).iter().map(|n| (n.clone(), walk(&json, n, ids, &mut memo, 0))).collect()
    }

    /// Bench (loaded machine allowed): `cargo test --release -p rc-sim --lib tag_bench -- --nocapture`.
    #[test]
    fn tag_resolution_matches_previous_on_the_jar() {
        let Some(data) = rc_testdata::version_data() else { return };
        let Some(path) = client_jar(&data) else { return };
        let jar = Jar::read(&path).expect("jar");
        let r = data.registry("minecraft:block").expect("block registry");
        let ids: HashMap<&str, i32> = r.entries.iter().enumerate().map(|(i, e)| (e.as_str(), i as i32)).collect();
        let time = |f: &dyn Fn() -> Vec<(String, Vec<i32>)>| {
            let mut v: Vec<f64> = (0..9)
                .map(|_| {
                    let t = std::time::Instant::now();
                    std::hint::black_box(f());
                    t.elapsed().as_secs_f64() * 1e3
                })
                .collect();
            v.sort_by(f64::total_cmp);
            v[v.len() / 2]
        };
        let old = previous(&jar, "minecraft:block", &ids);
        let new = resolve_tags(&jar, "minecraft:block", &ids);
        assert_eq!(old, new, "block tags of the jar differ");
        let a = time(&|| previous(&jar, "minecraft:block", &ids));
        let b = time(&|| resolve_tags(&jar, "minecraft:block", &ids));
        eprintln!("[bench] block tags ({} tags): before {a:.2} ms, after {b:.2} ms (median of 9)", new.len());
    }

    /// Opening bench (loaded machine allowed): all the registries' tags, old resolution against
    /// the game's resolution (same output on the jar alone), then the walk added
    /// for the command parser (tags of the other registries) and the jar's functions.
    #[test]
    fn opening_tag_passes_on_the_jar() {
        let Some(data) = rc_testdata::version_data() else { return };
        let Some(path) = client_jar(&data) else { return };
        let jar = Jar::read(&path).expect("jar");
        let regs = load_jar(&jar, &data);
        let median = |f: &mut dyn FnMut()| {
            let mut v: Vec<f64> = (0..9)
                .map(|_| {
                    let t = std::time::Instant::now();
                    f();
                    t.elapsed().as_secs_f64() * 1e3
                })
                .collect();
            v.sort_by(f64::total_cmp);
            v[v.len() / 2]
        };
        let old_all = || -> Tags {
            let mut out = Vec::new();
            for name in TAGGED_BUILTIN {
                let Some(r) = data.registry(name) else { continue };
                let ids: HashMap<&str, i32> = r.entries.iter().enumerate().map(|(i, e)| (e.as_str(), i as i32)).collect();
                out.push((name.to_owned(), previous(&jar, name, &ids)));
            }
            for (name, entries) in &regs.synced {
                let ids: HashMap<&str, i32> = entries.iter().enumerate().map(|(i, e)| (e.as_str(), i as i32)).collect();
                let t = previous(&jar, name, &ids);
                if !t.is_empty() {
                    out.push((name.clone(), t));
                }
            }
            out
        };
        assert_eq!(old_all(), tags_for(&jar, &data, &regs.synced), "jar tags differ");
        let a = median(&mut || {
            std::hint::black_box(old_all());
        });
        let b = median(&mut || {
            std::hint::black_box(tags_for(&jar, &data, &regs.synced));
        });
        let base = crate::commands::registry_tags(&data, &regs);
        let c = median(&mut || {
            let mut d = rc_commands::Data::from_version(&data);
            crate::commands::exec::apply_pack_tags(&jar, &mut d, &base);
            std::hint::black_box(d);
        });
        let d0 = median(&mut || {
            std::hint::black_box(rc_commands::Data::from_version(&data));
        });
        let e = median(&mut || {
            let d = rc_commands::Data::default();
            std::hint::black_box(crate::commands::exec::load_functions(&jar, &[], &d));
        });
        eprintln!(
            "[bench] opening: registry tags before {a:.2} ms, after {b:.2} ms; tags for the parser {:.2} ms; jar functions {e:.2} ms (medians of 9)",
            (c - d0).max(0.0)
        );
    }
}
