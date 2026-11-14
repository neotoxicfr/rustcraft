//! Game data read from the base pack of the jar (`server-gameplay.md` first): recipes,
//! loot tables, value providers, predicates and damage types. Nothing is copied
//! into the repository: everything comes from the player's jar when the world opens.

use hashbrown::HashMap;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

/// Folders kept (the rest of the pack serves the registries and is not kept).
const KEPT: [&str; 42] = [
    // G6-F01: jukebox songs (duration, comparator output).
    "jukebox_song/",
    // W9: settings of the trial spawner generators.
    "trial_spawner/",
    // W8b: block transformers (axe, hoe, shovel), generator elements read back by
    // bone meal (radii, heights, biome flowers).
    "block_transformer/",
    "worldgen/feature/",
    "worldgen/placed_feature/",
    "tags/worldgen/feature/",
    // F07: structures (own spawn lists, enlarged boxes); F08: monument.
    "worldgen/structure/",
    // S7: enchantments and their tags (enchanting table, anvil, loot, enchanted offers of
    // merchants), goat horns.
    "tags/instrument/",
    // G5-F10: instruments (sound, duration, range of the goat horn).
    "instrument/",
    "tags/banner_pattern/",
    "enchantment/",
    "tags/enchantment/",
    // S6: dimension types (scale, logical height, rules).
    "dimension_type/",
    // F05: merchant trades.
    "trade_set/",
    "villager_trade/",
    "tags/villager_trade/",
    "recipe/",
    "loot_table/",
    "context_int_provider/",
    "context_float_provider/",
    "predicate/",
    "damage_type/",
    "item_modifier/",
    // G6-F17: nested sets of loot tables (`#tag`), slot sources of the `slots` entries.
    "tags/loot_table/",
    "slot_source/",
    "tags/slot_source/",
    // S4: spawn lists of biomes, mob variants.
    "worldgen/biome/",
    "cow_variant/",
    "pig_variant/",
    "chicken_variant/",
    "wolf_variant/",
    "cat_variant/",
    // Spawn conditions of the frog variants (warm in mangrove swamps and jungles, cold in
    // snowy biomes), read when a frog is finalised or a tadpole grows up.
    "frog_variant/",
    // Enchantment providers (equipment enchanted at spawn: cost bounds and candidates).
    "enchantment_provider/",
    // Sound sets of the sound variants (adult, baby).
    "pig_sound_variant/",
    "cow_sound_variant/",
    "chicken_sound_variant/",
    "wolf_sound_variant/",
    "cat_sound_variant/",
    // F03: sulfur cube archetypes.
    "sulfur_cube_archetype/",
    // F09: zombie nautilus variants.
    "zombie_nautilus_variant/",
    // F14: game event tags (vibrations of the warden, of the shrieker).
    "tags/game_event/",
];

/// Folders of `KEPT` that the game reads again on a data reload (`/reload`, `/datapack`), with
/// every `tags/…` folder; the other folders are registries frozen when the world opens.
const READ_AGAIN_ON_RELOAD: [&str; 7] = [
    "recipe/",
    "loot_table/",
    "predicate/",
    "item_modifier/",
    "context_int_provider/",
    "context_float_provider/",
    "slot_source/",
];

/// File key (`<folder>/<path>` or `ns:<folder>/<path>`) of a folder read again on a data reload.
fn read_again_on_reload(key: &str) -> bool {
    let path = key.split_once(':').map_or(key, |(_, path)| path);
    path.starts_with("tags/") || READ_AGAIN_ON_RELOAD.iter().any(|folder| path.starts_with(folder))
}

/// Files of the `KEPT` folders in the pack stack `jar`, under their pack key (`<folder>/<path>`,
/// `ns:<folder>/<path>` outside the `minecraft` namespace).
fn kept_files(jar: &crate::registries::Jar) -> impl Iterator<Item = (String, Arc<[u8]>)> + '_ {
    jar.files.iter().filter_map(|(name, bytes)| {
        let (ns, path) = name.strip_prefix("data/")?.split_once('/')?;
        if !KEPT.iter().any(|k| path.starts_with(k)) {
            return None;
        }
        let path = path.trim_end_matches(".json");
        let key = if ns == "minecraft" { path.to_owned() } else { format!("{ns}:{path}") };
        Some((key, bytes.clone()))
    })
}

/// Cache by (folder or name, identifier): see `CacheKey`.
type Cache<V> = Mutex<HashMap<(Box<str>, Box<str>), V>>;

#[derive(Default)]
pub struct DataPack {
    /// `<folder>/<path>` (`minecraft` namespace implicit, otherwise `ns:folder/path`) → bytes.
    files: BTreeMap<String, Arc<[u8]>>,
    /// Parsed files, by (folder, identifier without `minecraft:`): the lookup is done
    /// without allocating (several times per tick: dimension types, biomes, generators).
    cache: Cache<Option<Arc<Value>>>,
    /// The same files converted to NBT (settings read again every tick, e.g. trial
    /// spawners): conversion done once.
    nbt: Cache<Option<Arc<rc_proto::nbt::Nbt>>>,
    /// Spawn lists by (biome, category), read again at each spawn attempt.
    biome_spawns: Cache<Arc<[Value]>>,
    /// List replacements by (structure, category): `None` if the structure has none.
    structure_spawns: Cache<Option<Arc<SpawnOverride>>>,
    /// S7: enchantment definitions built on first request.
    pub(crate) ench: std::sync::OnceLock<crate::enchanting::Enchants>,
    /// Tags of a data-driven registry folder, all resolved at its first request.
    tags: Mutex<HashMap<Box<str>, Arc<FolderTags>>>,
    /// Loot table of each block, by block id: `None` until the block is first broken,
    /// `Some(None)` when it has no table. A new pack starts empty.
    block_tables: Mutex<Vec<Option<Option<Arc<Value>>>>>,
}

/// Resolved tags of one registry folder: tag name (`ns:path`) → members (`ns:path`), in order.
pub(crate) type FolderTags = BTreeMap<String, Arc<[String]>>;

impl DataPack {
    pub(crate) fn from_jar(jar: &crate::registries::Jar) -> DataPack {
        DataPack { files: kept_files(jar).collect(), ..DataPack::default() }
    }

    /// This pack after a data reload on the pack stack `jar`: recipes, loot data and tags come
    /// from the new stack, the frozen registries stay those of the opening (a pack enabled since
    /// does not change an enchantment or a biome). The parsed copies start empty.
    pub(crate) fn reloaded(&self, jar: &crate::registries::Jar) -> DataPack {
        let read_again = kept_files(jar).filter(|(key, _)| read_again_on_reload(key));
        let frozen = self.files.iter().filter(|(key, _)| !read_again_on_reload(key));
        let files = read_again.chain(frozen.map(|(key, bytes)| (key.clone(), bytes.clone()))).collect();
        DataPack { files, ..DataPack::default() }
    }

    /// Pack built by hand (tests).
    #[cfg(test)]
    pub fn from_files(files: &[(&str, &str)]) -> DataPack {
        DataPack {
            files: files.iter().map(|(k, v)| ((*k).to_owned(), Arc::from(v.as_bytes()))).collect(),
            ..DataPack::default()
        }
    }

    /// Adds or replaces one file of a loaded pack (tests); parsed copies are dropped.
    #[cfg(test)]
    pub fn insert_test_file(&mut self, key: &str, json: &str) {
        self.files.insert(key.to_owned(), Arc::from(json.as_bytes()));
        self.cache.get_mut().unwrap_or_else(|e| e.into_inner()).clear();
        self.block_tables.get_mut().unwrap_or_else(|e| e.into_inner()).clear();
    }

    fn key(dir: &str, id: &str) -> String {
        let (ns, path) = id.split_once(':').unwrap_or(("minecraft", id));
        if ns == "minecraft" { format!("{dir}/{path}") } else { format!("{ns}:{dir}/{path}") }
    }

    /// Parsed file `dir/<id>` (kept in cache).
    pub fn get(&self, dir: &str, id: &str) -> Option<Arc<Value>> {
        let q = CacheKey(dir, id.strip_prefix("minecraft:").unwrap_or(id));
        if let Some(v) = self.cache.lock().unwrap_or_else(|e| e.into_inner()).get(&q) {
            return v.clone();
        }
        let v = self.files.get(&Self::key(dir, id)).and_then(|b| serde_json::from_slice::<Value>(b).ok()).map(Arc::new);
        self.cache.lock().unwrap_or_else(|e| e.into_inner()).insert(q.owned(), v.clone());
        v
    }

    /// Loot table `table_key` of block `block_id` (the key the game gives the block, `None` for
    /// a block that drops nothing), looked up by name the first time only: breaking a block then
    /// costs no hashed lookup.
    pub(crate) fn block_loot_table(&self, block_id: u32, table_key: Option<&str>) -> Option<Arc<Value>> {
        let slot = block_id as usize;
        if let Some(Some(known)) = self.block_tables.lock().unwrap_or_else(|e| e.into_inner()).get(slot) {
            return known.clone();
        }
        let table = table_key.and_then(|key| self.get("loot_table", key));
        let mut tables = self.block_tables.lock().unwrap_or_else(|e| e.into_inner());
        if tables.len() <= slot {
            tables.resize(slot + 1, None);
        }
        tables[slot] = Some(table.clone());
        table
    }

    /// Spawn list `key` (category) of biome `biome` (empty without file or list), kept.
    pub(crate) fn biome_spawns(&self, biome: &str, key: &str) -> Arc<[Value]> {
        let q = CacheKey(biome.strip_prefix("minecraft:").unwrap_or(biome), key);
        if let Some(v) = self.biome_spawns.lock().unwrap_or_else(|e| e.into_inner()).get(&q) {
            return v.clone();
        }
        let v: Arc<[Value]> = self
            .get("worldgen/biome", biome)
            .and_then(|v| {
                v["attributes"]["minecraft:gameplay/natural_mob_spawns"]["argument"]["spawns_by_category"][key]
                    .as_array()
                    .cloned()
            })
            .unwrap_or_default()
            .into();
        self.biome_spawns.lock().unwrap_or_else(|e| e.into_inner()).insert(q.owned(), v.clone());
        v
    }

    /// Replacement of list `key` by structure `name` (`spawn_overrides`), kept.
    pub(crate) fn structure_spawns(&self, name: &str, key: &str) -> Option<Arc<SpawnOverride>> {
        let q = CacheKey(name.strip_prefix("minecraft:").unwrap_or(name), key);
        if let Some(v) = self.structure_spawns.lock().unwrap_or_else(|e| e.into_inner()).get(&q) {
            return v.clone();
        }
        let v = self.get("worldgen/structure", q.0).and_then(|def| {
            let ov = &def["spawn_overrides"][key];
            (!ov.is_null()).then(|| {
                Arc::new(SpawnOverride {
                    spawns: ov["spawns"].as_array().cloned().unwrap_or_default().into(),
                    piece: ov["bounding_box"].as_str() == Some("piece"),
                    grow: def["terrain_adaptation"].as_str().is_some_and(|t| t != "none"),
                })
            })
        });
        self.structure_spawns.lock().unwrap_or_else(|e| e.into_inner()).insert(q.owned(), v.clone());
        v
    }

    /// File `dir/<id>` converted to NBT (kept in cache, like `get`).
    pub(crate) fn get_nbt(&self, dir: &str, id: &str) -> Option<Arc<rc_proto::nbt::Nbt>> {
        let q = CacheKey(dir, id.strip_prefix("minecraft:").unwrap_or(id));
        if let Some(v) = self.nbt.lock().unwrap_or_else(|e| e.into_inner()).get(&q) {
            return v.clone();
        }
        let v = self.get(dir, id).map(|j| Arc::new(crate::recipes::json_to_nbt(&j)));
        self.nbt.lock().unwrap_or_else(|e| e.into_inner()).insert(q.owned(), v.clone());
        v
    }

    /// Members of tag `tag` (`#` optional, `minecraft` namespace implicit) of the data-driven
    /// registry folder `dir` (`loot_table`, `slot_source`, `instrument`…), resolved like the
    /// game's tag loader (`packs::resolve_tags`): `id`, `#tag` and `{id, required}` members,
    /// nested tags followed, duplicates dropped (first kept), the whole tag refused (empty)
    /// when a required member is missing. A member exists when the folder holds its file. All
    /// the folder's tags are resolved at its first request, then kept.
    pub(crate) fn tag(&self, dir: &str, tag: &str) -> Arc<[String]> {
        let folder = self.folder_tags(dir);
        let name = tag.strip_prefix('#').unwrap_or(tag);
        let hit = if name.contains(':') { folder.get(name) } else { folder.get(&format!("minecraft:{name}")) };
        hit.cloned().unwrap_or_else(|| Arc::from(Vec::new()))
    }

    /// Every tag of the registry folder `dir`, resolved as [`Self::tag`] says, by tag name
    /// (`ns:path`): built at the folder's first request, then kept.
    pub(crate) fn folder_tags(&self, dir: &str) -> Arc<FolderTags> {
        let mut cache = self.tags.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(t) = cache.get(dir) {
            return t.clone();
        }
        let lists: Vec<(String, Vec<Value>)> = self
            .all(&format!("tags/{dir}"))
            .into_iter()
            .map(|(n, v)| (n, crate::packs::tag_values(&v).cloned().unwrap_or_default()))
            .collect();
        let raw: BTreeMap<String, &[Value]> = lists.iter().map(|(n, v)| (n.clone(), v.as_slice())).collect();
        let exists = |id: &str| self.files.contains_key(&Self::key(dir, id)).then(|| id.to_owned());
        let t: Arc<FolderTags> =
            Arc::new(crate::packs::resolve_tags(&raw, &exists).into_iter().map(|(k, v)| (k, Arc::from(v))).collect());
        cache.insert(dir.into(), t.clone());
        t
    }

    /// All files of a folder: (identifier, contents), in identifier order.
    pub fn all(&self, dir: &str) -> Vec<(String, Value)> {
        let prefix = format!("{dir}/");
        let mut out = Vec::new();
        for (k, b) in &self.files {
            let (ns, path) = match k.split_once(':') {
                Some((ns, p)) => (ns, p),
                None => ("minecraft", k.as_str()),
            };
            let Some(id) = path.strip_prefix(&prefix) else { continue };
            if let Ok(v) = serde_json::from_slice::<Value>(b) {
                out.push((format!("{ns}:{id}"), v));
            }
        }
        out.sort_by(|a, b| {
            let (an, ap) = a.0.split_once(':').unwrap_or(("", &a.0));
            let (bn, bp) = b.0.split_once(':').unwrap_or(("", &b.0));
            (an, ap).cmp(&(bn, bp))
        });
        out
    }
}

/// Borrowed cache key: (folder, identifier without `minecraft:`). Same hash as the owned key
/// `(Box<str>, Box<str>)` (two strings hashed in order), without allocation.
#[derive(Hash)]
struct CacheKey<'a>(&'a str, &'a str);

impl CacheKey<'_> {
    fn owned(&self) -> (Box<str>, Box<str>) {
        (self.0.into(), self.1.into())
    }
}

impl hashbrown::Equivalent<(Box<str>, Box<str>)> for CacheKey<'_> {
    fn equivalent(&self, k: &(Box<str>, Box<str>)) -> bool {
        *k.0 == *self.0 && *k.1 == *self.1
    }
}

/// Spawn list specific to a structure: entries, box of the pieces alone (`piece`) or
/// of the whole structure, enlarged if the structure adapts the terrain (`grow`).
pub(crate) struct SpawnOverride {
    pub spawns: Arc<[Value]>,
    pub piece: bool,
    pub grow: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// L10: tags of a registry folder read like the game's tag loader: `{id, required}`
    /// members, nested tags, duplicates dropped (first kept), a missing required member
    /// refusing the whole tag, an optional one skipped; `#` and `minecraft:` optional.
    #[test]
    fn folder_tags_resolved_like_the_game() {
        let pack = DataPack::from_files(&[
            ("instrument/a", "{}"),
            ("x:instrument/b", "{}"),
            ("tags/instrument/plain", r#"{"values":["a","x:b"]}"#),
            (
                "x:tags/instrument/ids",
                r##"{"values":[{"id":"x:b"},{"id":"minecraft:a","required":true},"#plain","a"]}"##,
            ),
            (
                "x:tags/instrument/optional",
                r##"{"values":[{"id":"x:none","required":false},{"id":"#x:gone","required":false},"a"]}"##,
            ),
            ("x:tags/instrument/missing", r#"{"values":["a","x:none"]}"#),
            ("x:tags/instrument/uses_missing", r##"{"values":["#x:missing","a"]}"##),
        ]);
        let tag = |t: &str| pack.tag("instrument", t).to_vec();
        assert_eq!(tag("plain"), ["minecraft:a", "x:b"]);
        assert_eq!(tag("#minecraft:plain"), ["minecraft:a", "x:b"]);
        assert_eq!(tag("#x:ids"), ["x:b", "minecraft:a"]);
        assert_eq!(tag("x:optional"), ["minecraft:a"]);
        assert!(tag("x:missing").is_empty());
        assert!(tag("x:uses_missing").is_empty());
        assert!(tag("x:unknown").is_empty());
        assert!(pack.tag("loot_table", "plain").is_empty(), "another folder");
    }
}
