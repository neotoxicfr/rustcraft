//! Conversion of saved data written by an older version of the game (`docs/specs/datafix.md`).
//!
//! Every saved file carries the data version it was written with (`DataVersion`). A file of an
//! older version is brought to the current one by applying, in order, every rule of a newer
//! version ([`chain`]). Each rule targets one kind of data (an item stack, a block entity, a
//! chunk...) wherever it lies in the file: the map of nested data ([`map`]) says, per version,
//! where every kind holds the others, and [`walk`] visits them. A file older than the oldest
//! version the chain covers is left untouched and reported [`Outcome::TooOld`], so that its
//! reader keeps it as it is on the disk rather than loading or overwriting a misread copy.

mod chain;
mod fixes;
mod kind;
mod map;
mod walk;

use rc_proto::nbt::Nbt;

pub use chain::OLDEST_CONVERTIBLE;
pub use kind::Kind;

/// Data version written by the game this crate converts to (26.3).
pub const CURRENT_VERSION: i32 = 5023;

/// Last version of the game's world file moves: a world whose `level.dat` is older needs its
/// files reorganised before its data can be converted ("upgrade and play"), a newer one opens
/// with "play" and has its data converted as it is read.
pub const LATEST_FILE_FIX: i32 = 4899;

/// A world whose `level.dat` was written before [`LATEST_FILE_FIX`] has files to move first.
pub fn requires_file_fixing(level_data_version: i32) -> bool {
    // The game counts every version before its first file move as 0.
    let as_file_fix_version = if level_data_version < 4772 { 0 } else { level_data_version };
    as_file_fix_version < LATEST_FILE_FIX
}

/// The saved files the game converts, each with the version assumed when it carries none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    /// The `Data` compound of `level.dat`.
    Level,
    /// The `Data` compound of `level.dat`, read for the world list only.
    LevelSummary,
    /// A file of `players/data`.
    Player,
    /// A chunk of a `region` file.
    Chunk,
    /// A chunk of an `entities` file.
    EntityChunk,
    /// A chunk of a `poi` file.
    PoiChunk,
    /// The saved hotbars (`hotbar.nbt`).
    Hotbar,
    /// A structure template (`.nbt`).
    Structure,
    /// A player's statistics.
    Stats,
    /// A player's advancements.
    Advancements,
    /// The client's options (`options.txt`, as key/value strings).
    Options,
    /// The client's debug screen profile.
    DebugProfile,
    /// World generation settings taken out of an old `level.dat` (the game's file moves).
    WorldGenSettings,
    /// A file of `data` (saved data of the world).
    SavedData(SavedData),
}

/// The world's saved data files (`data/<name>.dat`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SavedData {
    CommandStorage,
    CustomBossEvents,
    EnderDragonFight,
    GameRules,
    Tickets,
    MapData,
    MapIndex,
    Raids,
    RandomSequences,
    ScheduledEvents,
    Scoreboard,
    Stopwatches,
    StructureFeatureIndices,
    WanderingTrader,
    Weather,
    WorldBorder,
    WorldClocks,
    WorldGenSettings,
}

impl FileKind {
    /// Version of a file of this kind that carries none, as each of the game's readers assumes.
    pub fn default_version(self) -> i32 {
        match self {
            FileKind::PoiChunk => 1945,
            FileKind::Hotbar | FileKind::Stats | FileKind::Advancements | FileKind::SavedData(_) => 1343,
            FileKind::Structure => 500,
            FileKind::Options => 0,
            FileKind::DebugProfile => 4649,
            FileKind::Level
            | FileKind::LevelSummary
            | FileKind::Player
            | FileKind::Chunk
            | FileKind::EntityChunk
            | FileKind::WorldGenSettings => -1,
        }
    }

    /// The kind of data at the root of such a file.
    pub fn root_kind(self) -> Kind {
        match self {
            FileKind::Level => Kind::Level,
            FileKind::LevelSummary => Kind::LevelSummary,
            FileKind::Player => Kind::Player,
            FileKind::Chunk => Kind::Chunk,
            FileKind::EntityChunk => Kind::EntityChunk,
            FileKind::PoiChunk => Kind::PoiChunk,
            FileKind::Hotbar => Kind::Hotbar,
            FileKind::Structure => Kind::Structure,
            FileKind::Stats => Kind::Stats,
            FileKind::Advancements => Kind::Advancements,
            FileKind::Options => Kind::Options,
            FileKind::DebugProfile => Kind::DebugProfile,
            FileKind::WorldGenSettings => Kind::WorldGenSettings,
            FileKind::SavedData(d) => Kind::SavedData(d),
        }
    }
}

/// The data version a file says it was written with: its `DataVersion`, or the kind's default.
pub fn data_version(root: &Nbt, kind: FileKind) -> i32 {
    // Any integer width is read, cut to 32 bits like the game's integer read.
    root.get("DataVersion").and_then(Nbt::as_i64).map_or_else(|| kind.default_version(), |v| v as i32)
}

/// What [`upgrade`] did to a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// Already at the current version: untouched.
    Current,
    /// Written by a newer version: untouched (the game reads it as it is).
    Newer(i32),
    /// Converted from this version; its `DataVersion` is now the current one.
    Upgraded(i32),
    /// Older than [`OLDEST_CONVERTIBLE`]: untouched, to be kept as it is on the disk.
    TooOld(i32),
}

impl Outcome {
    /// The data can be read as current data (it is, or it was converted).
    pub fn readable(self) -> bool {
        matches!(self, Outcome::Current | Outcome::Upgraded(_) | Outcome::Newer(_))
    }
}

/// What a chunk's conversion is told about where the chunk lives (the game injects it into the
/// chunk during the conversion and removes it afterwards).
#[derive(Debug, Clone, Copy, Default)]
pub struct Context<'a> {
    /// The dimension's identifier (`minecraft:overworld`...).
    pub dimension: Option<&'a str>,
    /// The identifier of the dimension's chunk generator type, when it has one.
    pub generator: Option<&'a str>,
}

/// Brings `root` (a file of `kind`) from the version it carries to [`CURRENT_VERSION`].
pub fn upgrade(kind: FileKind, root: &mut Nbt, context: &Context<'_>) -> Outcome {
    let from = data_version(root, kind);
    upgrade_from(kind, root, from, context)
}

/// [`upgrade`] of a file whose version was read elsewhere (`options.txt` keeps it as a string).
pub fn upgrade_from(kind: FileKind, root: &mut Nbt, from: i32, context: &Context<'_>) -> Outcome {
    if from == CURRENT_VERSION {
        return Outcome::Current;
    }
    if from > CURRENT_VERSION {
        return Outcome::Newer(from);
    }
    if from < OLDEST_CONVERTIBLE {
        return Outcome::TooOld(from);
    }
    chain::apply(kind.root_kind(), root, from, context);
    // The options file keeps its version as a `version` string, written by its own reader.
    if kind != FileKind::Options
        && let Nbt::Compound(fields) = root
    {
        match fields.iter_mut().find(|(k, _)| k == "DataVersion") {
            Some((_, v)) => *v = Nbt::Int(CURRENT_VERSION),
            None => fields.push(("DataVersion".to_owned(), Nbt::Int(CURRENT_VERSION))),
        }
    }
    Outcome::Upgraded(from)
}

#[cfg(test)]
mod tests;
