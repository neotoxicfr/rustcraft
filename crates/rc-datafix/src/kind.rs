//! The kinds of data the conversion knows: every saved file's root, and every piece of data one
//! can hold inside another (an item stack in a chest, an entity riding another...).

use crate::SavedData;

/// A kind of data. A conversion rule targets one kind; the map of nested data says where each
/// kind holds the others.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    // Roots of saved files.
    Level,
    LevelSummary,
    Player,
    Chunk,
    EntityChunk,
    PoiChunk,
    Hotbar,
    Structure,
    Stats,
    Advancements,
    Options,
    DebugProfile,
    SavedData(SavedData),
    // Nested data.
    /// An entity with its passengers.
    EntityTree,
    /// One entity (its own fields, its equipment).
    Entity,
    /// An entity's worn and held items (`equipment`).
    EntityEquipment,
    BlockEntity,
    ItemStack,
    /// The components of an item stack or of a block entity.
    DataComponents,
    /// A block state compound (identifier and properties).
    BlockState,
    /// A block state written as text (`minecraft:stone[...]`).
    FlatBlockState,
    /// One trade of a villager or wandering trader.
    VillagerTrade,
    Particle,
    TextComponent,
    /// A spawner's spawn data and potential spawns.
    UntaggedSpawner,
    /// A structure start saved in a chunk.
    StructureFeature,
    /// A scoreboard objective.
    Objective,
    /// A scoreboard team.
    Team,
    WorldGenSettings,
    // Identifiers (a namespaced string).
    BlockName,
    ItemName,
    EntityName,
    GameEventName,
    Biome,
    Recipe,
    MultiNoiseParameterList,
}

impl Kind {
    /// An identifier written as a namespaced string.
    pub fn is_name(self) -> bool {
        matches!(
            self,
            Kind::BlockName
                | Kind::ItemName
                | Kind::EntityName
                | Kind::GameEventName
                | Kind::Biome
                | Kind::Recipe
                | Kind::MultiNoiseParameterList
        )
    }
}
