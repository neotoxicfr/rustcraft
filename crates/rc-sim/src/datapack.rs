//! Game data read from the base pack of the jar (`server-gameplay.md` first): recipes,
//! loot tables, value providers, predicates and damage types. Nothing is copied
//! into the repository: everything comes from the player's jar when the world opens.

use hashbrown::HashMap;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::Value;

/// Folders kept (the rest of the pack serves the registries and is not kept).
const KEPT: [&str; 37] = [
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
    // S4: spawn lists of biomes, mob variants.
    "worldgen/biome/",
    "cow_variant/",
    "pig_variant/",
    "chicken_variant/",
    "wolf_variant/",
    "cat_variant/",
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
}

impl DataPack {
    pub(crate) fn from_jar(jar: &crate::registries::Jar) -> DataPack {
        let mut files = BTreeMap::new();
        for (name, bytes) in &jar.files {
            let Some(rest) = name.strip_prefix("data/") else { continue };
            let Some((ns, path)) = rest.split_once('/') else { continue };
            if !KEPT.iter().any(|k| path.starts_with(k)) {
                continue;
            }
            let path = path.trim_end_matches(".json");
            let key = if ns == "minecraft" { path.to_owned() } else { format!("{ns}:{path}") };
            files.insert(key, bytes.clone());
        }
        DataPack {
            files,
            cache: Mutex::default(),
            nbt: Mutex::default(),
            biome_spawns: Mutex::default(),
            structure_spawns: Mutex::default(),
            ench: std::sync::OnceLock::new(),
        }
    }

    /// Pack built by hand (tests).
    #[cfg(test)]
    pub fn from_files(files: &[(&str, &str)]) -> DataPack {
        DataPack {
            files: files.iter().map(|(k, v)| ((*k).to_owned(), Arc::from(v.as_bytes()))).collect(),
            cache: Mutex::default(),
            nbt: Mutex::default(),
            biome_spawns: Mutex::default(),
            structure_spawns: Mutex::default(),
            ench: std::sync::OnceLock::new(),
        }
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
