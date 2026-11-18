//! Synchronised registries and tags (`integrated-server.md` § 2.2), read from the base pack of the
//! player's jar (`data/minecraft/…`). Order of the entries: that of a vanilla server, checked against
//! the reference capture of the official client (test `registries_match_capture`): sorted
//! identifiers (namespace then path), like the game's resource list.

use hashbrown::HashMap;
use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;

use rc_data::VersionData;
use rc_proto::nbt::Nbt;

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

/// Id of the game's base pack as the clients know it (`minecraft:core`).
pub const CORE_PACK: &str = "core";

#[derive(Debug, Clone, Default)]
pub struct Registries {
    /// Registry → entries in the order of their identifier.
    pub synced: Vec<(String, Vec<String>)>,
    pub tags: Tags,
    /// Where each synchronised entry comes from, to send its contents to a client that may not
    /// know it.
    pub contents: RegistryContents,
    /// Width and height (blocks) of each painting variant, in the order of its entries, read from
    /// the pack stack (`None`: an unreadable size).
    pub painting_sizes: Vec<Option<(i32, i32)>>,
    /// The known packs the server asks its clients about: those of the current selection (the
    /// entries stay those of the opening, the selection follows `/datapack` and `/reload`).
    pub known_packs: Vec<String>,
}

impl Registries {
    pub fn entries(&self, name: &str) -> &[String] {
        self.synced.iter().find(|(n, _)| n == name).map_or(&[], |(_, e)| e.as_slice())
    }
}

/// The file behind a synchronised entry and the pack that placed it.
#[derive(Debug, Clone, Default)]
pub struct EntrySource {
    /// Its JSON on the pack stack (`None`: an entry built into the game).
    pub file: Option<std::sync::Arc<[u8]>>,
    /// Index in [`RegistryContents::pack_ids`] of the known pack that placed it; `None`: a pack
    /// of the world's folder, which no client knows.
    pub known_pack: Option<u8>,
}

/// The sources of the synchronised entries, by registry (aligned with `Registries::synced`), and
/// the ids of the known packs they name.
#[derive(Debug, Clone, Default)]
pub struct RegistryContents {
    pub sources: Vec<Vec<EntrySource>>,
    /// Known pack ids: the base pack (`core`) first, then the jar's feature packs met.
    pub pack_ids: Vec<String>,
}

impl RegistryContents {
    /// Contents of entry `entry` of synchronised registry `registry` for a client that knows the
    /// packs `known`, like the game: omitted when the entry's file comes from one of them,
    /// otherwise its stacked JSON as NBT (`None` for a built-in entry, which has no file).
    pub fn contents_for(&self, registry: usize, entry: usize, known: &[String]) -> Option<Nbt> {
        let src = self.sources.get(registry)?.get(entry)?;
        let from_known = src.known_pack.and_then(|i| self.pack_ids.get(usize::from(i))).is_some_and(|id| known.contains(id));
        if from_known {
            return None;
        }
        let json: serde_json::Value = serde_json::from_slice(src.file.as_deref()?).ok()?;
        Some(crate::recipes::json_to_nbt(&json))
    }

    /// Whether some entry of registry `registry` has to carry its contents for a client that
    /// knows the packs `known`.
    pub fn needs_contents(&self, registry: usize, known: &[String]) -> bool {
        let Some(list) = self.sources.get(registry) else { return false };
        list.iter().any(|s| s.known_pack.and_then(|i| self.pack_ids.get(usize::from(i))).is_none_or(|id| !known.contains(id)))
    }
}

/// Pack that placed a file of the stack, when it is not the game's base pack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StackedFrom {
    /// A feature pack of the jar, by its id (clients of the same version know it).
    FeaturePack(String),
    /// A pack of the world's `datapacks` folder.
    WorldPack,
}

/// Files `data/<namespace>/<folder>/**.json` of the jar: identifier → contents. After
/// `packs::stack`, the stack of the world's chosen packs (`.mcfunction` functions included).
pub(crate) struct Jar {
    /// Bytes borrowed from the process's jar index (`rc_data::jar`), without copy.
    pub(crate) files: BTreeMap<String, std::sync::Arc<[u8]>>,
    /// The pack behind each file that the game's base pack did not place (empty for the jar
    /// alone): what the sending of the registries asks.
    pub(crate) origins: HashMap<String, StackedFrom>,
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
        Ok(Jar { files, origins: HashMap::new() })
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
        Ok(Jar { files, origins: HashMap::new() })
    }

    /// Identifiers of the resources of a folder (`worldgen/biome`), sorted.
    #[cfg(test)]
    fn ids(&self, dir: &str) -> Vec<String> {
        self.resources_in(dir).into_iter().map(|(id, _, _)| id).collect()
    }

    /// The resources of a folder (`worldgen/biome`), sorted by identifier: identifier, then the
    /// file's key and bytes on the stack.
    fn resources_in(&self, dir: &str) -> Vec<(String, &str, &std::sync::Arc<[u8]>)> {
        let mut out = Vec::new();
        for (name, bytes) in &self.files {
            let Some(rest) = name.strip_prefix("data/") else { continue };
            let Some((ns, path)) = rest.split_once('/') else { continue };
            let Some(file) = path.strip_prefix(dir).and_then(|p| p.strip_prefix('/')) else { continue };
            out.push((format!("{ns}:{}", file.trim_end_matches(".json")), name.as_str(), bytes));
        }
        out.sort_by(|(a, ..), (b, ..)| {
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
    let (synced, contents) = synced_registries(jar, data);
    let tags = tags_for(jar, data, &synced);
    let painting_sizes = painting_sizes(&synced, &contents);
    Registries { synced, tags, contents, painting_sizes, known_packs: vec![CORE_PACK.to_owned()] }
}

/// The synchronised registries' entries and their sources. Each registry folder is listed once,
/// its files taken from that listing (the list of every jar file is long: it was walked twice
/// per registry before).
fn synced_registries(jar: &Jar, data: &VersionData) -> (Vec<(String, Vec<String>)>, RegistryContents) {
    let mut synced = Vec::new();
    let mut contents = RegistryContents { sources: Vec::new(), pack_ids: vec![CORE_PACK.to_owned()] };
    for name in SYNCED {
        let dir = name.split_once(':').map_or(name, |x| x.1);
        let listed = jar.resources_in(dir);
        // Built-in registry (in code): order of the version data, no files.
        if listed.is_empty()
            && let Some(r) = data.registry(name)
        {
            contents.sources.push(vec![EntrySource { file: None, known_pack: Some(0) }; r.entries.len()]);
            synced.push((name.to_owned(), r.entries.clone()));
            continue;
        }
        let mut entries = Vec::with_capacity(listed.len());
        let mut sources = Vec::with_capacity(listed.len());
        for (id, key, file) in listed {
            let known_pack = known_pack_of(jar, key, &mut contents.pack_ids);
            sources.push(EntrySource { file: Some(file.clone()), known_pack });
            entries.push(id);
        }
        contents.sources.push(sources);
        synced.push((name.to_owned(), entries));
    }
    (synced, contents)
}

/// The known pack behind file `key` of the stack, as an index in `pack_ids` (feature pack ids are
/// added as they are met); `None`: a world folder's pack.
fn known_pack_of(jar: &Jar, key: &str, pack_ids: &mut Vec<String>) -> Option<u8> {
    match jar.origins.get(key) {
        None => Some(0),
        Some(StackedFrom::WorldPack) => None,
        Some(StackedFrom::FeaturePack(pack)) => {
            let i = pack_ids.iter().position(|p| p == pack).unwrap_or_else(|| {
                pack_ids.push(pack.clone());
                pack_ids.len() - 1
            });
            u8::try_from(i).ok()
        }
    }
}

/// Width and height of each painting variant from its stacked JSON (`width`, `height`).
fn painting_sizes(synced: &[(String, Vec<String>)], contents: &RegistryContents) -> Vec<Option<(i32, i32)>> {
    let Some(i) = synced.iter().position(|(n, _)| n == "minecraft:painting_variant") else { return Vec::new() };
    let size = |src: &EntrySource| {
        let v: serde_json::Value = serde_json::from_slice(src.file.as_deref()?).ok()?;
        let side = |k: &str| v.get(k).and_then(serde_json::Value::as_i64).and_then(|n| i32::try_from(n).ok());
        Some((side("width")?, side("height")?))
    };
    contents.sources[i].iter().map(size).collect()
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

    /// Opening bench (loaded machine allowed): the synchronised registries as listed before (each
    /// folder of the jar listed twice, nothing else kept) against now (listed once, with every
    /// entry's source and the paintings' sizes); same entries. Data missing: FAILURE.
    /// `cargo test --release -p rc-sim --lib synced_registries_on_the_jar -- --nocapture`.
    #[test]
    fn synced_registries_on_the_jar() {
        let data = rc_testdata::version_data().expect("26.3 data cache missing");
        let jar = Jar::read(&client_jar(&data).expect("26.3 client jar missing")).expect("jar");
        let before = || -> Vec<(String, Vec<String>)> {
            SYNCED
                .iter()
                .map(|name| {
                    let dir = name.split_once(':').map_or(*name, |x| x.1);
                    let entries = match data.registry(name) {
                        Some(r) if jar.ids(dir).is_empty() => r.entries.clone(),
                        _ => jar.ids(dir),
                    };
                    ((*name).to_owned(), entries)
                })
                .collect()
        };
        let now = || {
            let (synced, contents) = synced_registries(&jar, &data);
            let sizes = painting_sizes(&synced, &contents);
            (synced, contents, sizes)
        };
        let (synced, contents, sizes) = now();
        assert_eq!(before(), synced, "entries differ");
        assert!(contents.sources.iter().flatten().all(|s| s.known_pack == Some(0)), "the jar alone: all the base pack's");
        assert!(sizes.iter().all(Option::is_some), "every painting of the jar sized");
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
        let a = median(&mut || {
            std::hint::black_box(before());
        });
        let b = median(&mut || {
            std::hint::black_box(now());
        });
        let n: usize = synced.iter().map(|(_, e)| e.len()).sum();
        eprintln!("[bench] synced registries ({n} entries): before {a:.2} ms, now {b:.2} ms (medians of 9)");
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
            let paths = crate::commands::paths::CommandPaths::from_switches();
            std::hint::black_box(crate::commands::exec::load_functions(&jar, &[], &d, &paths));
        });
        eprintln!(
            "[bench] opening: registry tags before {a:.2} ms, after {b:.2} ms; tags for the parser {:.2} ms; jar functions {e:.2} ms (medians of 9)",
            (c - d0).max(0.0)
        );
    }
}
