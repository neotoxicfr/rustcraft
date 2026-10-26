//! Client data useful to parsing and suggestions (registries, tags, blocks,
//! players, permissions, crosshair target). Filled by the application.

use hashbrown::{HashMap, HashSet};

/// Properties of a block: (name, values) in definition order.
#[derive(Debug, Clone, Default)]
pub struct BlockDef {
    pub props: Vec<(String, Vec<String>)>,
    pub entity: bool,
}

/// Check of a particle's options: text `name{…}` → the game's error message.
pub type ParticleCheck = fn(&Data, &str) -> Result<(), String>;

#[derive(Debug, Clone, Default)]
pub struct Data {
    /// Known registries (built-in and synced): full identifier → full entries.
    pub registries: HashMap<String, Vec<String>>,
    /// Lookup sets (same content as `registries`).
    sets: HashMap<String, HashSet<String>>,
    /// Tags per registry: tag identifier (without `#`) → members.
    pub tags: HashMap<String, HashMap<String, Vec<String>>>,
    /// Blocks: identifier → properties.
    pub blocks: HashMap<String, BlockDef>,
    pub players: Vec<String>,
    pub teams: Vec<String>,
    /// Known dimensions (identifiers).
    pub dimensions: Vec<String>,
    pub sounds: Vec<String>,
    pub post_effects: Vec<String>,
    /// Summonable entity types: identifier and name translation key.
    pub summonable: Vec<(String, String)>,
    /// Operator level (events 24 to 28).
    pub op_level: i32,
    /// Targeted block (integer coordinates) and exact targeted point.
    pub target_block: Option<[i32; 3]>,
    pub target_point: Option<[f64; 3]>,
    /// UUID of the targeted entity (text).
    pub target_entity: Option<String>,
    /// Check of a particle's options (`name{…}`), supplied by the simulator: the game
    /// decodes them during parsing. `None`: no check (parsing only, without simulator).
    pub particle_check: Option<ParticleCheck>,
}

impl Data {
    /// Built-in registries and blocks from the version data.
    pub fn from_version(d: &rc_data::VersionData) -> Data {
        let mut out = Data::default();
        for r in &d.registries {
            out.set_registry(&r.name, r.entries.clone());
        }
        for b in &d.blocks {
            let name = if b.name.contains(':') { b.name.clone() } else { format!("minecraft:{}", b.name) };
            let props = b
                .properties
                .iter()
                .filter_map(|&p| d.properties.get(p as usize))
                .map(|p| (p.name.clone(), p.values.clone()))
                .collect();
            out.blocks.insert(name, BlockDef { props, entity: false });
        }
        out
    }

    /// Tags of a registry (replaced).
    pub fn set_tags(&mut self, registry: &str, tags: HashMap<String, Vec<String>>) {
        self.tags.insert(registry.to_owned(), tags);
    }

    /// Adds or replaces a registry.
    pub fn set_registry(&mut self, name: &str, entries: Vec<String>) {
        self.sets.insert(name.to_owned(), entries.iter().cloned().collect());
        self.registries.insert(name.to_owned(), entries);
    }

    pub fn registry(&self, name: &str) -> Option<&[String]> {
        self.registries.get(name).map(Vec::as_slice)
    }

    pub fn contains(&self, registry: &str, id: &str) -> bool {
        self.sets.get(registry).is_some_and(|s| s.contains(id))
    }

    pub fn has_registry(&self, registry: &str) -> bool {
        self.sets.contains_key(registry)
    }

    pub fn tag_names(&self, registry: &str) -> Vec<&String> {
        self.tags.get(registry).map(|t| t.keys().collect()).unwrap_or_default()
    }

    pub fn has_tag(&self, registry: &str, tag: &str) -> bool {
        self.tags.get(registry).is_some_and(|t| t.contains_key(tag))
    }

    pub fn tag_members(&self, registry: &str, tag: &str) -> &[String] {
        self.tags.get(registry).and_then(|t| t.get(tag)).map_or(&[], Vec::as_slice)
    }

    /// `@…` selectors allowed (§ 4.5).
    pub fn selectors(&self) -> bool {
        self.op_level >= 2
    }
}
