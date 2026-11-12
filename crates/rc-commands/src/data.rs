//! Client data useful to parsing and suggestions (registries, tags, blocks,
//! players, permissions, crosshair target). Filled by the application.

use std::cell::Cell;
use std::sync::{Arc, Mutex, OnceLock, PoisonError, Weak};

use hashbrown::{HashMap, HashSet};
use rc_data::{NameIndex, VersionData, name_index_on};

/// Properties of a block: (name, values) in definition order.
#[derive(Debug, Clone, Default)]
pub struct BlockDef {
    pub props: Vec<(String, Vec<String>)>,
    pub entity: bool,
}

/// Check of a particle's options: text `name{…}` → the game's error message.
pub type ParticleCheck = fn(&Data, &str) -> Result<(), String>;

/// Members of one tag, shared by every holder of the same tag list (the server keeps the tags of
/// the jar and gives the same lists to its parsing data).
pub type TagMembers = Arc<[String]>;

/// Tags of one registry: tag identifier (without `#`) → members.
pub type RegistryTags = HashMap<String, TagMembers>;

thread_local! {
    /// Tests and benches: `RUSTCRAFT_OPT_CMDDATASHARE` forced on this thread.
    static FORCED_SHARE: Cell<Option<bool>> = const { Cell::new(None) };
}

/// `RUSTCRAFT_OPT_CMDDATASHARE` (on by default, `=0` = former path): the registries of the
/// version data read where they are instead of copied with a second copy as a set, the tags of
/// the server's jar given to its parsing data without a copy, the block table shared by every
/// command data made from the same version data (the client's and the integrated server's).
pub fn shared_command_data() -> bool {
    FORCED_SHARE.with(Cell::get).unwrap_or_else(|| rc_data::opt_switch!("RUSTCRAFT_OPT_CMDDATASHARE"))
}

#[cfg(test)]
thread_local! {
    /// Deliberate error of the equivalence proofs: a registry read in the version data finds nothing.
    pub(crate) static SABOTAGE_VERSION_LOOKUP: Cell<bool> = const { Cell::new(false) };
}

/// Tests and benches: `RUSTCRAFT_OPT_CMDDATASHARE` forced (`Some`) or back to the environment
/// (`None`) on this thread.
#[doc(hidden)]
pub fn force_shared_command_data(on: Option<bool>) {
    FORCED_SHARE.with(|f| f.set(on));
}

/// Entries of one registry known to the commands.
#[derive(Clone)]
enum RegistryEntries {
    /// A registry of the version data, read where it is: its own index answers the lookups.
    InVersion { version: Arc<VersionData>, position: usize },
    /// A registry received from a server, listed from the data packs, or copied (former path).
    Listed(Arc<ListedEntries>),
}

/// Entries and their name index (positions only, built at the first lookup).
#[derive(Default)]
struct ListedEntries {
    entries: Vec<String>,
    index: NameIndex,
}

impl RegistryEntries {
    fn listed(entries: Vec<String>) -> RegistryEntries {
        RegistryEntries::Listed(Arc::new(ListedEntries { entries, index: NameIndex::default() }))
    }

    fn entries(&self) -> &[String] {
        match self {
            RegistryEntries::InVersion { version, position } => &version.registries[*position].entries,
            RegistryEntries::Listed(l) => &l.entries,
        }
    }

    fn contains(&self, id: &str) -> bool {
        match self {
            #[cfg(test)]
            RegistryEntries::InVersion { .. } if SABOTAGE_VERSION_LOOKUP.with(Cell::get) => false,
            RegistryEntries::InVersion { version, position } => version.registries[*position].id(id).is_some(),
            RegistryEntries::Listed(l) => l.index.position_with(&l.entries, String::as_str, id, name_index_on()).is_some(),
        }
    }
}

impl std::fmt::Debug for RegistryEntries {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let kind = if matches!(self, RegistryEntries::InVersion { .. }) { "in version data" } else { "listed" };
        write!(f, "{} entries {kind}", self.entries().len())
    }
}

/// Blocks: identifier → properties, and the identifiers in order (sorted once, for the
/// suggestions). Reads go through `Deref`; a change through `DerefMut` forgets the order.
#[derive(Debug, Clone, Default)]
pub struct BlockTable {
    by_id: HashMap<String, BlockDef>,
    ids_in_order: OnceLock<Vec<String>>,
}

impl BlockTable {
    fn from_version(d: &VersionData) -> BlockTable {
        let mut by_id = HashMap::with_capacity(d.blocks.len());
        for b in &d.blocks {
            let name = if b.name.contains(':') { b.name.clone() } else { format!("minecraft:{}", b.name) };
            let props = b
                .properties
                .iter()
                .filter_map(|&p| d.properties.get(p as usize))
                .map(|p| (p.name.clone(), p.values.clone()))
                .collect();
            by_id.insert(name, BlockDef { props, entity: false });
        }
        BlockTable { by_id, ids_in_order: OnceLock::new() }
    }

    /// Every block identifier, sorted (byte order), worked out once.
    pub fn ids_in_order(&self) -> &[String] {
        self.ids_in_order.get_or_init(|| {
            let mut ids: Vec<String> = self.by_id.keys().cloned().collect();
            ids.sort();
            ids
        })
    }
}

impl std::ops::Deref for BlockTable {
    type Target = HashMap<String, BlockDef>;

    fn deref(&self) -> &Self::Target {
        &self.by_id
    }
}

impl std::ops::DerefMut for BlockTable {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.ids_in_order = OnceLock::new();
        &mut self.by_id
    }
}

/// The block table of `version`, shared by every command data made from it: the client and the
/// integrated server hold the same version data, so the table exists once in singleplayer. Found
/// by the identity of the version data, never by its content: a weak reference keeps the address
/// from being given to another allocation while its entry exists, and a version data dropped
/// takes its entry with it.
fn shared_blocks(version: &Arc<VersionData>) -> Arc<BlockTable> {
    type Built = Vec<(Weak<VersionData>, Weak<BlockTable>)>;
    static BUILT: Mutex<Built> = Mutex::new(Vec::new());
    let mut built = BUILT.lock().unwrap_or_else(PoisonError::into_inner);
    built.retain(|(v, b)| v.strong_count() > 0 && b.strong_count() > 0);
    let same = |v: &Weak<VersionData>| std::ptr::eq(v.as_ptr(), Arc::as_ptr(version));
    if let Some(blocks) = built.iter().find(|(v, _)| same(v)).and_then(|(_, b)| b.upgrade()) {
        return blocks;
    }
    let blocks = Arc::new(BlockTable::from_version(version));
    built.push((Arc::downgrade(version), Arc::downgrade(&blocks)));
    blocks
}

#[derive(Debug, Clone, Default)]
pub struct Data {
    /// Known registries (built-in and synced): full identifier → full entries.
    registries: HashMap<String, RegistryEntries>,
    /// Former path (`RUSTCRAFT_OPT_CMDDATASHARE=0`): every registry copied again as a set, for
    /// the lookups.
    sets: HashMap<String, HashSet<String>>,
    /// Tags per registry: tag identifier (without `#`) → members.
    pub tags: HashMap<String, RegistryTags>,
    /// Blocks: identifier → properties.
    pub blocks: Arc<BlockTable>,
    pub players: Vec<String>,
    pub teams: Vec<String>,
    /// Known dimensions (identifiers).
    pub dimensions: Vec<String>,
    /// Sounds offered by `available_sounds` when set (former path); empty: the sound event
    /// registry, read where it is.
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
    /// Objective criteria (about 9 000, one per statistic), listed at the first suggestion that
    /// needs them and kept until a registry changes: they name the entries of several registries,
    /// which the client receives from each server it joins (so never a list shared by the process).
    criteria: OnceLock<Vec<String>>,
}

const SOUND_EVENT: &str = "minecraft:sound_event";

impl Data {
    /// Built-in registries and blocks copied from the version data.
    pub fn from_version(d: &VersionData) -> Data {
        let mut out = Data::default();
        for r in &d.registries {
            out.set_registry(&r.name, r.entries.clone());
        }
        out.blocks = Arc::new(BlockTable::from_version(d));
        out
    }

    /// Built-in registries and blocks of the version data, read where they are and shared with
    /// every other command data made from it (`RUSTCRAFT_OPT_CMDDATASHARE=0`: copied, like
    /// [`from_version`](Self::from_version), and the sounds listed apart).
    pub fn from_shared_version(d: &Arc<VersionData>) -> Data {
        if !shared_command_data() {
            let mut out = Data::from_version(d);
            out.sounds = out.registry(SOUND_EVENT).map(<[String]>::to_vec).unwrap_or_default();
            return out;
        }
        let mut out = Data::default();
        for (position, r) in d.registries.iter().enumerate() {
            out.registries.insert(r.name.clone(), RegistryEntries::InVersion { version: Arc::clone(d), position });
        }
        out.blocks = shared_blocks(d);
        out
    }

    /// Tags of a registry (replaced).
    pub fn set_tags(&mut self, registry: &str, tags: RegistryTags) {
        self.tags.insert(registry.to_owned(), tags);
    }

    /// Tags of a registry (replaced) taken from a list the caller keeps: its member lists are
    /// shared (`RUSTCRAFT_OPT_CMDDATASHARE=0`: copied, like before).
    pub fn set_tags_from(&mut self, registry: &str, tags: &RegistryTags) {
        let tags = if shared_command_data() {
            tags.clone()
        } else {
            tags.iter().map(|(name, members)| (name.clone(), TagMembers::from(&members[..]))).collect()
        };
        self.set_tags(registry, tags);
    }

    /// Adds or replaces a registry.
    pub fn set_registry(&mut self, name: &str, entries: Vec<String>) {
        if shared_command_data() {
            self.sets.remove(name);
        } else {
            self.sets.insert(name.to_owned(), entries.iter().cloned().collect());
        }
        self.registries.insert(name.to_owned(), RegistryEntries::listed(entries));
        self.criteria = OnceLock::new();
    }

    /// The objective criteria listed by `list` from these registries, worked out once and kept
    /// until a registry changes. Kept in the order the suggestions end in (stable sort, so the
    /// suggestions built from them come out the same): sorting them again at each key press then
    /// only checks the order.
    pub(crate) fn objective_criteria(&self, list: fn(&Data) -> Vec<String>) -> &[String] {
        self.criteria.get_or_init(|| {
            let mut criteria = list(self);
            criteria.sort_by(|a, b| crate::suggest::cmp_ignore_case(a, b));
            criteria
        })
    }

    pub fn registry(&self, name: &str) -> Option<&[String]> {
        self.registries.get(name).map(RegistryEntries::entries)
    }

    pub fn contains(&self, registry: &str, id: &str) -> bool {
        match self.sets.get(registry) {
            Some(set) => set.contains(id),
            None => self.registries.get(registry).is_some_and(|r| r.contains(id)),
        }
    }

    pub fn has_registry(&self, registry: &str) -> bool {
        self.registries.contains_key(registry)
    }

    /// Sounds offered by the `available_sounds` suggestions.
    pub fn sound_ids(&self) -> &[String] {
        if self.sounds.is_empty() { self.registry(SOUND_EVENT).unwrap_or(&[]) } else { &self.sounds }
    }

    pub fn tag_names(&self, registry: &str) -> Vec<&String> {
        self.tags.get(registry).map(|t| t.keys().collect()).unwrap_or_default()
    }

    pub fn has_tag(&self, registry: &str, tag: &str) -> bool {
        self.tags.get(registry).is_some_and(|t| t.contains_key(tag))
    }

    pub fn tag_members(&self, registry: &str, tag: &str) -> &[String] {
        self.tags.get(registry).and_then(|t| t.get(tag)).map_or(&[], |m| &m[..])
    }

    /// `@…` selectors allowed (§ 4.5).
    pub fn selectors(&self) -> bool {
        self.op_level >= 2
    }
}
