//! In-memory model of the data of a version. Everything is indexed by integers: a block
//! state is a `u32` (network identifier), its properties are in a flat table.

use std::hash::BuildHasher;
use std::sync::{Arc, OnceLock};

/// Aligned box, in fractions of a block (exact doubles: physics depends on it).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Aabb {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

/// Per-state flags, aligned with the extractor (`tools/extractor/Extractor.java`).
pub mod flags {
    pub const AIR: u16 = 1;
    pub const LIQUID: u16 = 1 << 1;
    pub const SKYLIGHT_THROUGH: u16 = 1 << 2;
    pub const CAN_OCCLUDE: u16 = 1 << 3;
    pub const SHAPE_LIGHT_OCCLUSION: u16 = 1 << 4;
    pub const SOLID_RENDER: u16 = 1 << 5;
    pub const REPLACEABLE: u16 = 1 << 6;
    pub const RANDOM_TICK: u16 = 1 << 7;
    pub const LARGE_COLLISION: u16 = 1 << 8;
    pub const FULL_COLLISION: u16 = 1 << 9;
    pub const HAS_OFFSET: u16 = 1 << 10;
    pub const FLUID_SOURCE: u16 = 1 << 11;
    /// "Solid" in the historical sense (height of fluid corners).
    pub const LEGACY_SOLID: u16 = 1 << 12;
    /// Falling fluid (level ≥ 8).
    pub const FLUID_FALLING: u16 = 1 << 13;
    pub const ICE: u16 = 1 << 14;
    /// Neighbour that makes the water overlay texture be drawn (glass, ice, leaves…).
    pub const FLUID_OVERLAY: u16 = 1 << 15;
}

/// Facts about a block state. 24 bytes, dense array indexed by state identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StateInfo {
    pub block: u16,
    pub collision: u16,
    pub outline: u16,
    pub occlusion: u16,
    pub support: u16,
    pub flags: u16,
    pub light_emission: u8,
    pub light_dampening: u8,
    pub fluid: u8,
    pub fluid_amount: u8,
    /// Start of the property value indices of this state in `VersionData::state_values`.
    pub values: u32,
    /// Position offset for the variant seed: `(dx+1) | (dy+1) << 2 | (dz+1) << 4`.
    pub seed_offset: u8,
    /// Bit d (official order down, up, north, south, west, east): "sturdy" face (full support).
    pub sturdy: u8,
    /// Lighting at meshing (`light_bits`): emissive rendering, dark occlusion shadow.
    pub light: u8,
    /// Sound set of the state: index into `VersionData::sound_types`.
    pub sound: u8,
    /// Map colour (0 = none).
    pub map_color: u8,
}

/// Sound set of a block (`sounds.md` § 5.2): volume, pitch and five events.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SoundType {
    pub volume: f32,
    pub pitch: f32,
    pub break_sound: String,
    pub step: String,
    pub place: String,
    pub hit: String,
    pub fall: String,
}

/// Music track (`sounds.md` § 6.1).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct MusicTrack {
    pub sound: String,
    pub min_delay: i32,
    pub max_delay: i32,
    pub replace: bool,
}

/// `audio/background_music` attribute: default track, creative, underwater.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct BackgroundMusic {
    pub default: Option<MusicTrack>,
    pub creative: Option<MusicTrack>,
    pub underwater: Option<MusicTrack>,
}

/// Mood of an ambience (`sounds.md` § 5.4).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AmbientMood {
    pub sound: String,
    pub tick_delay: i32,
    pub block_search_extent: i32,
    pub offset: f64,
}

/// `audio/ambient_sounds` attribute: loop, mood, additions (sound, chance per tick).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AmbientSounds {
    pub loop_sound: Option<String>,
    pub mood: Option<AmbientMood>,
    pub additions: Vec<(String, f64)>,
}

/// Discrete audio attributes of a layer (dimension or biome): `None` = not defined here.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AudioAttrs {
    pub ambient: Option<AmbientSounds>,
    pub music: Option<BackgroundMusic>,
}

/// Bits of `StateInfo::light`.
pub mod light_bits {
    /// Always fully lit (magma, active sculk sensors).
    pub const EMISSIVE: u8 = 1;
    /// Ambient occlusion shadow 0.2 instead of 1.0.
    pub const DARK_SHADE: u8 = 2;
}

impl StateInfo {
    #[inline]
    pub fn has(&self, flag: u16) -> bool {
        self.flags & flag != 0
    }

    /// Offset (dx, dy, dz) applied to the position for the variant seed.
    #[inline]
    pub fn seed_offset(&self) -> [i32; 3] {
        let o = i32::from(self.seed_offset);
        [(o & 3) - 1, ((o >> 2) & 3) - 1, ((o >> 4) & 3) - 1]
    }
}

/// Effect of a block on an entity passing through it or walking on it (`BlockInfo::inside`).
pub mod inside {
    pub const NONE: u8 = 0;
    pub const WEB: u8 = 1;
    pub const BERRY_BUSH: u8 = 2;
    pub const POWDER_SNOW: u8 = 3;
    pub const HONEY: u8 = 4;
    pub const BUBBLE_COLUMN: u8 = 5;
    /// Slows down when walked on.
    pub const SLIME: u8 = 6;
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlockInfo {
    pub name: String,
    pub first_state: u32,
    pub state_count: u32,
    pub default_state: u32,
    pub friction: f32,
    pub speed_factor: f32,
    pub jump_factor: f32,
    /// Bounce restitution when landing on it (slime 1, beds 0.75).
    pub bounce: f32,
    /// Effect on an entity passing through it (`INSIDE_*`).
    pub inside: u8,
    /// Random model offset: 0 none, 1 horizontal, 2 horizontal and vertical; bounds.
    pub offset_kind: u8,
    pub offset_h: f32,
    pub offset_v: f32,
    pub dynamic_shape: bool,
    /// Indices into `VersionData::properties`, by increasing property name.
    pub properties: Vec<u16>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Property {
    pub name: String,
    pub values: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Phase {
    Handshake,
    Status,
    Login,
    Configuration,
    Play,
}

impl Phase {
    pub const ALL: [Phase; 5] = [Phase::Handshake, Phase::Status, Phase::Login, Phase::Configuration, Phase::Play];

    pub fn report_key(self) -> &'static str {
        match self {
            Phase::Handshake => "handshake",
            Phase::Status => "status",
            Phase::Login => "login",
            Phase::Configuration => "configuration",
            Phase::Play => "play",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Bound {
    ToClient,
    ToServer,
}

impl Bound {
    pub const ALL: [Bound; 2] = [Bound::ToClient, Bound::ToServer];

    pub fn report_key(self) -> &'static str {
        match self {
            Bound::ToClient => "clientbound",
            Bound::ToServer => "serverbound",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacketDef {
    pub phase: Phase,
    pub bound: Bound,
    pub id: i32,
    pub name: String,
}

/// Dimension type of the base data pack (read from the jar at preparation): used
/// when the client announces it knows this pack and the server no longer sends the content.
#[derive(Debug, Clone, PartialEq)]
pub struct DimensionType {
    pub name: String,
    pub min_y: i32,
    pub height: i32,
    /// "Nether" cardinal lighting (`cardinal_light` field).
    pub nether_light: bool,
    /// Environment attributes of the dimension (constant layer, `docs/specs/environment.md`).
    pub attributes: Vec<EnvEntry>,
    /// Timelines applied, tag resolved, in order.
    pub timelines: Vec<String>,
    /// Sky: 0 none, 1 Overworld, 2 End.
    pub skybox: u8,
    pub has_skylight: bool,
    pub has_ceiling: bool,
    /// Default clock (`default_clock`), empty if there is none.
    pub default_clock: String,
    /// `ambient_light`: raises the brightness curve (vignette, `hud.md` § 5.3).
    pub ambient_light: f32,
    /// Discrete audio attributes (music, ambience).
    pub audio: AudioAttrs,
}

/// Operation of an environment attribute modifier (environment spec § 1.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum EnvOp {
    Override,
    Add,
    Subtract,
    Multiply,
    Minimum,
    Maximum,
    AlphaBlend,
    BlendToGray,
    And,
    Or,
    Nand,
    Nor,
    Xor,
    Xnor,
}

impl EnvOp {
    pub const ALL: [EnvOp; 14] = [
        EnvOp::Override,
        EnvOp::Add,
        EnvOp::Subtract,
        EnvOp::Multiply,
        EnvOp::Minimum,
        EnvOp::Maximum,
        EnvOp::AlphaBlend,
        EnvOp::BlendToGray,
        EnvOp::And,
        EnvOp::Or,
        EnvOp::Nand,
        EnvOp::Nor,
        EnvOp::Xor,
        EnvOp::Xnor,
    ];

    pub fn parse(s: &str) -> Option<EnvOp> {
        Some(match s {
            "override" => EnvOp::Override,
            "add" => EnvOp::Add,
            "subtract" => EnvOp::Subtract,
            "multiply" => EnvOp::Multiply,
            "minimum" => EnvOp::Minimum,
            "maximum" => EnvOp::Maximum,
            "alpha_blend" => EnvOp::AlphaBlend,
            "blend_to_gray" => EnvOp::BlendToGray,
            "and" => EnvOp::And,
            "or" => EnvOp::Or,
            "nand" => EnvOp::Nand,
            "nor" => EnvOp::Nor,
            "xor" => EnvOp::Xor,
            "xnor" => EnvOp::Xnor,
            _ => return None,
        })
    }
}

/// Environment attribute entry: identifier without `minecraft:` (e.g. `visual/fog_color`),
/// operation and argument. Argument: colour (r, g, b, a) in [0, 1] sRGB (a = 1 for an
/// RGB colour); float or boolean (0/1) in [0]; `alpha_blend` float: (value, alpha);
/// `blend_to_gray`: (brightness, factor).
#[derive(Debug, Clone, PartialEq)]
pub struct EnvEntry {
    pub attr: String,
    pub op: EnvOp,
    pub arg: [f32; 4],
}

/// Curve of a timeline track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ease {
    Linear,
    Constant,
    Bezier([f32; 4]),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Track {
    pub attr: String,
    pub op: EnvOp,
    pub ease: Ease,
    /// (tick, argument) sorted by tick.
    pub keys: Vec<(u32, [f32; 4])>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Timeline {
    pub name: String,
    /// Clock (registry `world_clock`).
    pub clock: String,
    /// Period in ticks, 0 = no looping.
    pub period: u32,
    pub tracks: Vec<Track>,
}

/// Colour-type environment attributes (the others are floats or booleans).
pub fn env_is_color(attr: &str) -> bool {
    matches!(
        attr,
        "visual/fog_color"
            | "visual/water_fog_color"
            | "visual/sky_color"
            | "visual/sunrise_sunset_color"
            | "visual/cloud_color"
            | "visual/block_light_tint"
            | "visual/sky_light_color"
            | "visual/night_vision_color"
            | "visual/ambient_light_color"
    )
}

/// Tint kind of a model layer (client extractor).
pub mod tint {
    pub const NONE: u8 = 0;
    pub const CONSTANT: u8 = 1;
    pub const GRASS: u8 = 2;
    pub const FOLIAGE: u8 = 3;
    pub const DRY_FOLIAGE: u8 = 4;
    pub const WATER: u8 = 5;
    /// Colour without position = default colour of the grass palette (temperature 0.5,
    /// humidity 1), read by the client in the loaded palette; zero-alpha witness of
    /// the extractor (`ClientExtractor.GRASS_WITNESS`).
    pub const GRASS_DEFAULT: u32 = 0x00C0_FFEE;
    /// Biome colour modified in an unrecognised way: treated as untinted.
    pub const OTHER: u8 = 6;
}

/// Client-side facts per state (absent if the client jar was missing at preparation).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ClientState {
    /// Bit d (official order down, up, north, south, west, east): face removed against an identical block.
    pub skip_self: u8,
    /// Tint kind per layer (`tint::*`).
    pub tint_kind: [u8; 4],
    /// ARGB colour (constant kind) or consulted position offset (`(d+1)` per axis on 2 bits).
    pub tint_value: [u32; 4],
    /// Tint of block pieces (layer 0 seen as a terrain particle): (kind, value),
    /// same coding; kind `NONE`: no tint.
    pub particle_tint: (u8, u32),
    /// Colour of falling dust: own colour of a falling block, otherwise terrain particle
    /// tint, otherwise map colour; (kind, value), never `NONE`.
    pub dust: (u8, u32),
    /// ARGB colour without position per layer (block drawn as an entity); white without a tint
    /// source; `tint::GRASS_DEFAULT`: default colour of the grass palette.
    pub entity_tint: [u32; 4],
}

/// Biome of the base data pack (read from the jar at preparation).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Biome {
    pub name: String,
    pub temperature: f32,
    pub downfall: f32,
    pub grass_color: Option<u32>,
    pub foliage_color: Option<u32>,
    pub dry_foliage_color: Option<u32>,
    pub water_color: u32,
    /// 0 none, 1 dark forest, 2 swamp.
    pub grass_modifier: u8,
    pub sky_color: Option<u32>,
    /// Environment attributes of the biome (positional layer).
    pub attributes: Vec<EnvEntry>,
    pub has_precipitation: bool,
    /// "Frozen" temperature modifier (frozen oceans, `weather-border.md` § 1.2).
    pub frozen: bool,
    /// Discrete audio attributes (music, ambience).
    pub audio: AudioAttrs,
}

/// Name → lazy indexes (`RUSTCRAFT_OPT_NAMEINDEX=0`: linear searches, old path).
pub fn name_index_on() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| crate::env_str!("RUSTCRAFT_OPT_NAMEINDEX") != Some("0"))
}

/// Name → position index of a list (FIRST occurrence, like `iter().position`), built on
/// first use and shared between threads. The table keeps only positions: each answer is
/// checked against the list itself. A list whose length changed since construction
/// (registry pushed afterwards in a test) goes back to the linear search.
///
/// Neutral for equality (two equal data stay equal, index built or not); a clone restarts
/// empty (rebuilt at its first use).
#[derive(Default)]
pub struct NameIndex(OnceLock<BuiltIndex>);

struct BuiltIndex {
    len: usize,
    hasher: hashbrown::DefaultHashBuilder,
    table: hashbrown::HashTable<u32>,
}

impl NameIndex {
    /// Position of `name` in `items` (key `key`); `indexed` false: linear search.
    fn position_with<T>(&self, items: &[T], key: impl Fn(&T) -> &str, name: &str, indexed: bool) -> Option<usize> {
        if !indexed {
            return items.iter().position(|e| key(e) == name);
        }
        let b = self.0.get_or_init(|| {
            let hasher = hashbrown::DefaultHashBuilder::default();
            let mut table = hashbrown::HashTable::with_capacity(items.len());
            for (i, e) in items.iter().enumerate() {
                let k = key(e);
                let h = hasher.hash_one(k);
                // First occurrence kept: a duplicate further on never overwrites the first.
                if table.find(h, |&j: &u32| key(&items[j as usize]) == k).is_none() {
                    table.insert_unique(h, i as u32, |&j: &u32| hasher.hash_one(key(&items[j as usize])));
                }
            }
            BuiltIndex { len: items.len(), hasher, table }
        });
        if b.len != items.len() {
            return items.iter().position(|e| key(e) == name);
        }
        let h = b.hasher.hash_one(name);
        b.table.find(h, |&j| items.get(j as usize).is_some_and(|e| key(e) == name)).map(|&j| j as usize)
    }

    fn position<T>(&self, items: &[T], key: impl Fn(&T) -> &str, name: &str) -> Option<usize> {
        self.position_with(items, key, name, name_index_on())
    }
}

impl Clone for NameIndex {
    fn clone(&self) -> Self {
        NameIndex::default()
    }
}

impl PartialEq for NameIndex {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for NameIndex {}

impl std::fmt::Debug for NameIndex {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.get().is_some() { "NameIndex(built)" } else { "NameIndex" })
    }
}

/// Built-in registry: entries in the order of their network identifier. Built by
/// [`Registry::new`] (name → identifier index kept alongside).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Registry {
    pub name: String,
    pub entries: Vec<String>,
    index: NameIndex,
    shared: SharedEntries,
}

/// Shared copy of the entries, made once on first request (neutral for equality).
#[derive(Default)]
struct SharedEntries(OnceLock<Arc<[Box<str>]>>);

impl Clone for SharedEntries {
    fn clone(&self) -> Self {
        SharedEntries::default()
    }
}

impl PartialEq for SharedEntries {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl Eq for SharedEntries {}

impl std::fmt::Debug for SharedEntries {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SharedEntries")
    }
}

impl Registry {
    pub fn new(name: impl Into<String>, entries: Vec<String>) -> Registry {
        Registry { name: name.into(), entries, index: NameIndex::default(), shared: SharedEntries::default() }
    }

    /// Network identifier of `name` (first occurrence), `None` if absent.
    pub fn id(&self, name: &str) -> Option<u32> {
        self.index.position(&self.entries, String::as_str, name).map(|i| i as u32)
    }

    #[doc(hidden)]
    pub fn id_with(&self, name: &str, indexed: bool) -> Option<u32> {
        self.index.position_with(&self.entries, String::as_str, name, indexed).map(|i| i as u32)
    }

    /// Shared entries (a single copy per registry, then `Arc::clone`): to be preferred over
    /// `entries.clone()` for a name table kept for a long time.
    pub fn shared(&self) -> Arc<[Box<str>]> {
        self.shared.0.get_or_init(|| self.entries.iter().map(|e| Box::from(e.as_str())).collect()).clone()
    }
}

/// Most consulted registries, resolved once (positions in `VersionData::registries`).
const KNOWN: [&str; 8] = [
    "minecraft:item",
    "minecraft:entity_type",
    "minecraft:data_component_type",
    "minecraft:potion",
    "minecraft:sound_event",
    "minecraft:block_entity_type",
    "minecraft:particle_type",
    "minecraft:mob_effect",
];

/// Lazy indexes of `VersionData` (registries, blocks, known registries). To be left at its
/// default value: filled on first use.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Lookup {
    registries: NameIndex,
    blocks: NameIndex,
    known: KnownRegistries,
}

#[derive(Default)]
struct KnownRegistries(OnceLock<[Option<u32>; KNOWN.len()]>);

impl Clone for KnownRegistries {
    fn clone(&self) -> Self {
        KnownRegistries::default()
    }
}

impl PartialEq for KnownRegistries {
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

impl std::fmt::Debug for KnownRegistries {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("KnownRegistries")
    }
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct VersionData {
    pub version: String,
    pub protocol: i32,
    /// Boxes of all shapes, concatenated.
    pub aabbs: Vec<Aabb>,
    /// For each shape: `[start, end)` in `aabbs`. Shape 0 is empty.
    pub shapes: Vec<(u32, u32)>,
    pub states: Vec<StateInfo>,
    pub blocks: Vec<BlockInfo>,
    pub properties: Vec<Property>,
    /// Property value indices, per state (see `StateInfo::values`).
    pub state_values: Vec<u8>,
    pub fluids: Vec<String>,
    pub packets: Vec<PacketDef>,
    pub registries: Vec<Registry>,
    /// Empty if the client jar was not available at preparation.
    pub dimension_types: Vec<DimensionType>,
    /// One per state, or empty without a client jar.
    pub client_states: Vec<ClientState>,
    pub biomes: Vec<Biome>,
    /// Timelines of the base pack (`data/minecraft/timeline`).
    pub timelines: Vec<Timeline>,
    /// Default components useful to the client, by `minecraft:item` identifier.
    pub items: Vec<ItemInfo>,
    /// Base effects of each potion, by `minecraft:potion` identifier.
    pub potions: Vec<Vec<PotionEffect>>,
    /// Colour (RGB) of each effect, by `minecraft:mob_effect` identifier.
    pub effect_colors: Vec<u32>,
    /// Width, height, eye height and name tag height, by identifier of
    /// `minecraft:entity_type`.
    pub entity_dims: Vec<[f32; 4]>,
    /// Entity model layers (`entities.md` part 2), sorted by name; empty without a client jar.
    pub entity_models: Vec<EntityLayer>,
    /// Client keyframe animations (`camel.walk`…), sorted by name; empty without a client jar.
    pub entity_animations: Vec<KeyframeAnimation>,
    /// Creative inventory tabs (`creative-inventory.md` § 3.2); empty without a client jar.
    pub creative: CreativeData,
    /// Sound sets of blocks (`StateInfo::sound`).
    pub sound_types: Vec<SoundType>,
    /// Hardness per state (−1 unbreakable), `interactions.md` § 1.2.
    pub hardness: Vec<f32>,
    /// Per state: the block requires the right tool for its loot.
    pub needs_tool: Vec<bool>,
    /// Per state: simulation facts (`sim_bits`), for the integrated server.
    pub sim: Vec<u16>,
    /// Per state: the block's blast resistance.
    pub blast: Vec<f32>,
    /// Per state: instrument given to a neighbouring note block (rank in the game's order, that
    /// of the values of the note block's `instrument` property).
    pub instrument: Vec<u8>,
    /// Name → position index (lazy); leave `Default::default()`.
    pub lookup: Lookup,
}

/// Bits of `VersionData::sim` (`block-behaviours.md` § 3.1, § 3.9).
pub mod sim_bits {
    /// Redstone conductor.
    pub const CONDUCTOR: u16 = 1;
    /// Signal source.
    pub const SIGNAL_SOURCE: u16 = 1 << 1;
    /// Piston reaction on 3 bits (`PUSH_*`).
    pub const PUSH_SHIFT: u16 = 2;
    pub const PUSH_MASK: u16 = 7 << 2;
    /// Has a block entity.
    pub const BLOCK_ENTITY: u16 = 1 << 5;
    /// Analog output (comparators).
    pub const ANALOG: u16 = 1 << 6;
    /// Rigid top face (diodes, rails).
    pub const RIGID_UP: u16 = 1 << 7;
    /// Top face with centre support (torches, dust on hopper…).
    pub const CENTER_UP: u16 = 1 << 8;
    /// Bottom face with centre support (hanging lanterns).
    pub const CENTER_DOWN: u16 = 1 << 9;
    /// Ignited by lava.
    pub const IGNITED_BY_LAVA: u16 = 1 << 10;
    /// Can receive water in this state (waterloggable, not waterlogged).
    pub const PLACE_WATER: u16 = 1 << 11;
    /// Liquid container (waterloggable, kelp…).
    pub const LIQUID_CONTAINER: u16 = 1 << 12;

    pub const PUSH_NORMAL: u16 = 0;
    pub const PUSH_DESTROY: u16 = 1;
    pub const PUSH_BLOCK: u16 = 2;
    pub const PUSH_IGNORE: u16 = 3;
    pub const PUSH_ONLY: u16 = 4;
}

/// Creative tabs extracted from the jar and fallback data of dynamic registries.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CreativeData {
    /// Order of the `creative_mode_tab` registry.
    pub tabs: Vec<CreativeTab>,
    /// Base painting variants: name, width, height.
    pub paintings: Vec<(String, u8, u8)>,
    /// Base enchantments: name, min level, max level.
    pub enchantments: Vec<(String, u8, u8)>,
}

/// Kind of creative tab.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum CreativeKind {
    #[default]
    Category,
    Hotbar,
    Search,
    Inventory,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CreativeTab {
    pub name: String,
    /// Translation key of the title.
    pub title: String,
    /// 0 up, 1 down.
    pub row: u8,
    pub column: u8,
    pub kind: CreativeKind,
    pub right: bool,
    pub scroll: bool,
    pub show_title: bool,
    /// Background texture (`minecraft:textures/gui/…png`).
    pub background: String,
    pub icon: CreativeStack,
    pub entries: Vec<CreativeEntry>,
}

/// Element of a tab's sequence: fixed stack, or marker of a sequence to generate from the
/// named dynamic registry (`minecraft:painting_variant`, `minecraft:instrument`,
/// `minecraft:enchantment`).
#[derive(Debug, Clone, PartialEq)]
pub enum CreativeEntry {
    Stack(CreativeStack),
    Dynamic(String),
}

/// Stack of quantity 1 of a tab.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct CreativeStack {
    pub item: String,
    /// 0 tab and search, 1 tab only, 2 search only.
    pub visibility: u8,
    /// Required features (item and potion).
    pub features: Vec<String>,
    /// Components added: type, value in network format.
    pub added: Vec<(String, Vec<u8>)>,
    pub removed: Vec<String>,
    /// Banner patterns by name (dynamic registry reference) and colour.
    pub banner: Vec<(String, u8)>,
}

/// Keyframe animation: duration (seconds), loop, channels.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct KeyframeAnimation {
    pub name: String,
    pub length: f32,
    pub looping: bool,
    pub channels: Vec<AnimChannel>,
}

/// Target of a channel: offset added to the part's pose.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum AnimTarget {
    /// Pixels (y already in the parts' frame).
    #[default]
    Position,
    /// Radians.
    Rotation,
    /// Scale − 1.
    Scale,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnimChannel {
    pub part: String,
    pub target: AnimTarget,
    pub keyframes: Vec<Keyframe>,
}

/// Keyframe: time (seconds), values before and after the time, interpolation toward it
/// (Catmull-Rom, otherwise linear).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Keyframe {
    pub time: f32,
    pub pre: [f32; 3],
    pub post: [f32; 3],
    pub catmull: bool,
}

/// Entity model layer: texture size and parts in drawing order (parent before
/// children; `parent` = index of the parent part, `u32::MAX` for the root).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct EntityLayer {
    pub name: String,
    pub texture: [u16; 2],
    pub parts: Vec<EntityPart>,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct EntityPart {
    pub name: String,
    pub parent: u32,
    /// x, y, z, rotations x, y, z (radians), scales x, y, z.
    pub pose: [f32; 9],
    pub cubes: Vec<EntityCube>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct EntityCube {
    pub uv: [f32; 2],
    pub from: [f32; 3],
    pub size: [f32; 3],
    pub grow: [f32; 3],
    pub mirror: bool,
    pub tex_scale: [f32; 2],
    /// Visible faces: bit = order down, up, north, south, west, east.
    pub faces: u8,
}

impl VersionData {
    /// Model layer by name (`minecraft:zombie#main`).
    pub fn entity_layer(&self, name: &str) -> Option<&EntityLayer> {
        self.entity_models.binary_search_by(|l| l.name.as_str().cmp(name)).ok().map(|i| &self.entity_models[i])
    }

    /// Animation by name (`camel.walk`).
    pub fn entity_animation(&self, name: &str) -> Option<&KeyframeAnimation> {
        self.entity_animations.binary_search_by(|a| a.name.as_str().cmp(name)).ok().map(|i| &self.entity_animations[i])
    }
}

/// Effect of a base potion (potion colour, `items.md` part 1 § 4.3).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PotionEffect {
    /// Identifier in `minecraft:mob_effect`.
    pub effect: u16,
    pub amplifier: u8,
    pub visible: bool,
    /// Duration in ticks (−1: infinite, 0: instant).
    pub duration: i32,
}

/// Default attribute modifier of an item (`attribute_modifiers`): attribute without
/// namespace, modifier identifier, value, operation (0 add, 1 base
/// multiplier, 2 total multiplier) and slot group (order of the equipment slot group: 0 all,
/// 1 main hand, 2 off hand, 3 hand, 4 feet, 5 legs, 6 chest, 7 head, 8 armour,
/// 9 body, 10 saddle).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AttributeMod {
    pub attribute: String,
    pub id: String,
    pub amount: f64,
    pub operation: u8,
    pub slot: u8,
}

/// Block set of a tool rule: tag (`minecraft:mineable/pickaxe`, without `#`) or
/// list of block identifiers (indices into `VersionData::blocks`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlockSet {
    Tag(String),
    Blocks(Vec<u32>),
}

/// Rule of the `tool` component: blocks, speed and optional "correct for loot".
#[derive(Debug, Clone, PartialEq)]
pub struct ToolRule {
    pub blocks: BlockSet,
    pub speed: Option<f32>,
    pub correct: Option<bool>,
}

/// `tool` component (`interactions.md` § 1.2).
#[derive(Debug, Clone, PartialEq)]
pub struct Tool {
    pub rules: Vec<ToolRule>,
    pub default_speed: f32,
    pub damage_per_block: u32,
    pub creative_destroy: bool,
}

/// `consumable` component: duration (s), use animation (0 none, 1 eat, 2 drink…),
/// sound, consumption particles.
#[derive(Debug, Clone, PartialEq)]
pub struct Consumable {
    pub seconds: f32,
    pub animation: u8,
    pub sound: String,
    pub particles: bool,
}

/// `kinetic_weapon` component (spear charge): delay before effect, then three conditions
/// (dismount, knockback, damage) with a maximum duration (ticks after the delay), minimum
/// attacker speed and minimum relative speed (blocks/s).
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct KineticParams {
    pub contact_cooldown: i32,
    pub delay: i32,
    /// Dismount, knockback, damage: (maximum duration, min. speed, min. relative speed).
    pub conditions: [Option<(i32, f32, f32)>; 3],
    pub forward_movement: f32,
    pub damage_multiplier: f32,
}

/// Prototype of an item ("default components" report of the official generator): what the
/// network does not send since stacks only carry a patch.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ItemInfo {
    pub max_stack_size: u8,
    /// 0: item without durability.
    pub max_damage: u32,
    /// 0 common, 1 uncommon, 2 rare, 3 epic.
    pub rarity: u8,
    /// Translation key of the name (`item_name`), empty if the name is not a simple translation.
    pub name_key: String,
    /// `item_model`, empty if absent.
    pub model: String,
    /// `enchantment_glint_override`: 0 absent, 1 false, 2 true.
    pub glint: u8,
    /// `equippable`: slot + 1 (0 = no equipment; 1 hand, 2 feet, 3 legs,
    /// 4 chest, 5 head, 6 off hand, 7 body, 8 saddle) and `asset_id` (empty without).
    pub equip_slot: u8,
    pub equip_asset: String,
    pub tool: Option<Tool>,
    pub consumable: Option<Consumable>,
    /// `food`: present, and "can always be eaten".
    pub food: Option<bool>,
    /// `attack_range`: min, max, creative min, creative max ranges, margin, mobs factor.
    pub attack_range: Option<[f32; 6]>,
    /// `minimum_attack_charge` (0 without).
    pub min_attack_charge: f32,
    /// `piercing_weapon`, `kinetic_weapon`, `blocks_attacks` present.
    pub piercing: bool,
    pub kinetic: bool,
    /// Parameters of the default `kinetic_weapon` component (spears).
    pub kinetic_params: Option<KineticParams>,
    pub blocks_attacks: bool,
    /// `attack_animation` / `interact_animation`: kind (0 none, 1 hit, 2 thrust), duration.
    pub attack_anim: [u8; 2],
    pub interact_anim: [u8; 2],
    /// `use_effects`: sprint allowed, speed multiplier.
    pub use_sprint: bool,
    pub use_speed: f32,
    /// `equippable.camera_overlay`: full-screen texture worn in first person (empty without).
    pub equip_overlay: String,
    /// Default `attribute_modifiers` (tooltip).
    pub attributes: Vec<AttributeMod>,
    /// Default component types (indices of `minecraft:data_component_type`): their count
    /// makes the "N components" line of the advanced tooltip.
    pub components: Vec<u16>,
}

impl VersionData {
    #[inline]
    pub fn state(&self, id: u32) -> Option<&StateInfo> {
        self.states.get(id as usize)
    }

    #[inline]
    pub fn shape(&self, index: u16) -> &[Aabb] {
        let (a, b) = self.shapes[index as usize];
        &self.aabbs[a as usize..b as usize]
    }

    pub fn block_of(&self, state: u32) -> Option<&BlockInfo> {
        self.state(state).map(|s| &self.blocks[s.block as usize])
    }

    /// Property values of a state, as (name, value).
    pub fn state_properties(&self, state: u32) -> impl Iterator<Item = (&str, &str)> {
        let info = self.state(state);
        let block = info.map(|s| &self.blocks[s.block as usize]);
        let start = info.map_or(0, |s| s.values as usize);
        block.into_iter().flat_map(|b| b.properties.iter()).enumerate().map(move |(i, &p)| {
            let prop = &self.properties[p as usize];
            (prop.name.as_str(), prop.values[self.state_values[start + i] as usize].as_str())
        })
    }

    pub fn packet_id(&self, phase: Phase, bound: Bound, name: &str) -> Option<i32> {
        self.packets.iter().find(|p| p.phase == phase && p.bound == bound && p.name == name).map(|p| p.id)
    }

    pub fn registry(&self, name: &str) -> Option<&Registry> {
        self.lookup.registries.position(&self.registries, |r| r.name.as_str(), name).map(|i| &self.registries[i])
    }

    #[doc(hidden)]
    pub fn registry_with(&self, name: &str, indexed: bool) -> Option<&Registry> {
        self.lookup
            .registries
            .position_with(&self.registries, |r| r.name.as_str(), name, indexed)
            .map(|i| &self.registries[i])
    }

    /// Network identifier of `name` in registry `reg` (first occurrence).
    pub fn registry_id(&self, reg: &str, name: &str) -> Option<u32> {
        self.registry(reg)?.id(name)
    }

    /// Index of block `name` in `blocks` (first occurrence).
    pub fn block_id(&self, name: &str) -> Option<u32> {
        self.lookup.blocks.position(&self.blocks, |b| b.name.as_str(), name).map(|i| i as u32)
    }

    #[doc(hidden)]
    pub fn block_id_with(&self, name: &str, indexed: bool) -> Option<u32> {
        self.lookup.blocks.position_with(&self.blocks, |b| b.name.as_str(), name, indexed).map(|i| i as u32)
    }

    /// Block by name.
    pub fn block_named(&self, name: &str) -> Option<&BlockInfo> {
        self.block_id(name).map(|i| &self.blocks[i as usize])
    }

    /// Default state of block `name`.
    pub fn default_state_of(&self, name: &str) -> Option<u32> {
        self.block_named(name).map(|b| b.default_state)
    }

    /// Known registry `KNOWN[k]`, position resolved once (fallback: search by name if the list
    /// of registries changed length or if the index is off).
    fn known(&self, k: usize) -> Option<&Registry> {
        if !name_index_on() {
            return self.registry(KNOWN[k]);
        }
        let pos = self.lookup.known.0.get_or_init(|| {
            KNOWN.map(|n| self.lookup.registries.position(&self.registries, |r| r.name.as_str(), n).map(|i| i as u32))
        });
        match pos[k] {
            Some(i) => {
                self.registries.get(i as usize).filter(|r| r.name == KNOWN[k]).or_else(|| self.registry(KNOWN[k]))
            }
            None => self.registry(KNOWN[k]),
        }
    }

    pub fn items(&self) -> Option<&Registry> {
        self.known(0)
    }

    pub fn entity_types(&self) -> Option<&Registry> {
        self.known(1)
    }

    pub fn data_component_types(&self) -> Option<&Registry> {
        self.known(2)
    }

    pub fn potion_registry(&self) -> Option<&Registry> {
        self.known(3)
    }

    pub fn sound_events(&self) -> Option<&Registry> {
        self.known(4)
    }

    pub fn block_entity_types(&self) -> Option<&Registry> {
        self.known(5)
    }

    pub fn particle_types(&self) -> Option<&Registry> {
        self.known(6)
    }

    pub fn mob_effects(&self) -> Option<&Registry> {
        self.known(7)
    }

    /// Number of bits of the direct palette format of block states.
    pub fn state_bits(&self) -> u32 {
        usize::BITS - (self.states.len().max(2) - 1).leading_zeros()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reg(names: &[&str]) -> Registry {
        Registry::new("t", names.iter().map(|s| (*s).to_owned()).collect())
    }

    /// Duplicate: the first occurrence wins (like `position`); absent: `None`; list lengthened
    /// after construction: exact linear fallback.
    #[test]
    fn index_matches_position() {
        let mut r = reg(&["a", "b", "a", "c", "b"]);
        for n in ["a", "b", "c", "zz", ""] {
            let want = r.entries.iter().position(|e| e == n).map(|i| i as u32);
            assert_eq!(r.id_with(n, true), want, "{n}");
            assert_eq!(r.id_with(n, false), want, "{n}");
        }
        assert_eq!(r.id_with("a", true), Some(0));
        r.entries.push("d".into());
        assert_eq!(r.id_with("d", true), Some(5), "stale index after an addition");
        assert_eq!(reg(&[]).id_with("a", true), None);
    }

    #[test]
    fn known_registries_and_blocks() {
        let mut d = VersionData::default();
        for n in ["minecraft:a", "minecraft:item", "minecraft:mob_effect", "minecraft:item"] {
            d.registries.push(Registry::new(n, vec![n.to_owned()]));
        }
        d.registries[3].entries[0] = "second".into();
        assert_eq!(d.items().map(|r| r.entries[0].as_str()), Some("minecraft:item"));
        assert_eq!(d.mob_effects().map(|r| r.entries[0].as_str()), Some("minecraft:mob_effect"));
        assert!(d.entity_types().is_none());
        assert_eq!(d.registry_id("minecraft:item", "minecraft:item"), Some(0));
        let b = |n: &str| BlockInfo {
            name: n.into(),
            first_state: 0,
            state_count: 1,
            default_state: 7,
            friction: 0.6,
            speed_factor: 1.0,
            jump_factor: 1.0,
            bounce: 0.0,
            inside: 0,
            offset_kind: 0,
            offset_h: 0.0,
            offset_v: 0.0,
            dynamic_shape: false,
            properties: Vec::new(),
        };
        d.blocks = vec![b("x"), b("y"), b("x")];
        assert_eq!((d.block_id("x"), d.block_id("y"), d.block_id("z")), (Some(0), Some(1), None));
        assert_eq!(d.default_state_of("y"), Some(7));
        // The index does not change equality.
        let e = d.clone();
        assert_eq!(d, e);
    }
}
