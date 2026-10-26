//! Connection to a server (offline for now): handshake, login,
//! configuration, play. The network thread answers service packets on its own (keep-alive, ping,
//! chunk batches, teleportation) and publishes an ordered event stream; chunks are
//! decompressed and decoded on the pool. Flow: `docs/specs/connection.md`.

pub mod auth;
pub mod chat;
pub mod chat_verify;
mod conn;
pub mod cooldowns;
pub mod misc;
mod dns;
pub mod extra;
pub mod ids;
pub mod link;
pub mod memory;
pub mod netstats;
pub mod progress;
pub mod protolog;
pub mod reason;
pub mod recipes;
pub mod session;
pub mod signing;
pub mod status;
pub mod sync;

mod seq;

use std::sync::Arc;

pub use conn::connect;
use rc_proto::BlockPos;
use rc_proto_v777::chunk::ChunkPacket;
use rc_proto_v777::config::{DimensionHeight, Login};

#[derive(Debug, Clone, Default)]
pub struct Config {
    pub host: String,
    pub port: u16,
    pub name: String,
    /// Client information sent during configuration (language, render distance, skin parts…).
    pub info: rc_proto_v777::play::ClientInformation,
    /// Connection opened by a transfer: intent 3 of the handshake
    /// (`protocol-supplements.md` § 10).
    pub transfer: bool,
    /// Cookies carried over from the previous connection (transfer), otherwise empty table.
    pub cookies: Cookies,
    /// Profile UUID (launcher account); failing that, the offline UUID of the username.
    pub uuid: Option<u128>,
    /// Access token (launcher account): without it, offline connection only.
    pub token: Option<auth::AccessToken>,
}

/// Cookies of a connection: key → bytes (≤ 5120), in memory only.
pub type Cookies = Arc<std::sync::Mutex<hashbrown::HashMap<String, Vec<u8>>>>;

/// Standard offline UUID: version 3 (MD5) of `OfflinePlayer:<username>`.
/// Signable arguments of a command without `/` (`commands.md` § 2.6), parsing without registries.
pub fn commands_signable(tree: &rc_commands::Tree, command: &str) -> Vec<(String, String)> {
    static EMPTY: std::sync::OnceLock<rc_commands::Data> = std::sync::OnceLock::new();
    let data = EMPTY.get_or_init(rc_commands::Data::default);
    let input: Vec<char> = std::iter::once('/').chain(command.chars()).collect();
    let src = rc_commands::parse::Source { restricted: true, selectors: true };
    let p = rc_commands::parse(tree, &input, data, src);
    rc_commands::parse::signable(tree, &p, &input)
}

pub fn offline_uuid(name: &str) -> u128 {
    use md5::{Digest, Md5};
    let mut h: [u8; 16] = Md5::digest(format!("OfflinePlayer:{name}").as_bytes()).into();
    h[6] = (h[6] & 0x0f) | 0x30;
    h[8] = (h[8] & 0x3f) | 0x80;
    u128::from_be_bytes(h)
}

/// Label of a chat message (side bar).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatTag {
    System,
    NotSecure,
    Error,
    /// Signed and verified player message: no bar.
    Secure,
    /// Signed message whose displayed content differs from the signed text (dark gray bar, icon).
    Modified,
    /// Disguised message (`disguised_chat`): "system" bar (`chat.tag.system`), goes through the chat
    /// delay queue like player messages.
    Disguised,
}

impl ChatTag {
    /// Message going through the chat delay queue (player messages, errors, disguised).
    pub fn delayed(self) -> bool {
        !matches!(self, ChatTag::System)
    }
}

#[derive(Debug)]
pub enum Event {
    /// Entering the game (or dimension change): height of the current world.
    Joined {
        login: Box<Login>,
        height: DimensionHeight,
        /// Biomes in the order of their network identifier.
        biomes: Vec<String>,
        /// Dimension type (registry name) and "flat" world.
        dimension: String,
        flat: bool,
    },
    Respawn {
        height: DimensionHeight,
        dimension: String,
        flat: bool,
        /// Retention flags: 1 = attributes, 2 = entity data (dimension
        /// change), 0 = death (new player).
        keep: u8,
        /// Last death location of the player (dimension, block), from the spawn info.
        last_death: Option<(String, [i32; 3])>,
    },
    /// Time packet: game time and updated clocks (name, total ticks, fraction, speed).
    Time {
        game_time: i64,
        clocks: Vec<(String, i64, f32, f32)>,
    },
    /// Light update of a column (present or empty sheets; the others unchanged).
    LightUpdate {
        x: i32,
        z: i32,
        light: Box<rc_proto_v777::chunk::LightData>,
    },
    /// Decorated chat line, ready to display, with its tag (`chat.md` § 3.6).
    Chat {
        component: rc_text::Component,
        tag: ChatTag,
    },
    /// Signature of the player message emitted just before (for `delete_chat`).
    ChatSignature(chat::Signature),
    /// Original signed text of the "modified" message emitted just before (tooltip of the tag).
    ChatOriginal(String),
    /// Signed message to delete (`chat.md` § 1.5).
    DeleteChat(chat::Signature),
    /// Command tree (`commands.md` § 1), replaces the previous one.
    Commands(Box<rc_proto_v777::commands::Commands>),
    /// Reply to a suggestions request (§ 3.2).
    CommandSuggestions(rc_proto_v777::commands::ServerSuggestions),
    /// Custom chat completions (§ 3.5): action and entries.
    ChatCompletions {
        action: i32,
        entries: Vec<String>,
    },
    /// Action bar text.
    ActionBar(rc_text::Component),
    /// Game event (rain: types 1, 2, 7, 8).
    GameEvent {
        kind: u8,
        param: f32,
    },
    /// Player list: additions and updates (`players.md`).
    PlayerInfo(rc_proto_v777::players::InfoUpdate),
    /// Player list: removals.
    PlayersRemoved(Vec<u128>),
    /// One-shot sound or one attached to an entity (`sounds.md` § 1.3, 1.4).
    Sound(rc_proto_v777::play::SoundPacket),
    /// Particles (`particles.md` § 1.1).
    Particles(rc_proto_v777::entity::LevelParticles),
    /// World event (`sounds.md` § 1.6, `particles.md`): type, block, data, global.
    LevelEvent {
        kind: i32,
        pos: [i32; 3],
        data: i32,
        global: bool,
    },
    /// Sound stop (§ 1.5).
    StopSound {
        category: Option<i32>,
        name: Option<String>,
    },
    /// Scoreboard and teams (`scoreboard-teams.md`).
    Score(rc_proto_v777::scores::ScoreEvent),
    /// Boss bar (`hud-advanced.md` § 1).
    Boss(rc_proto_v777::play::BossEvent),
    /// Waypoint of the locator bar (`hud.md` § 4.8).
    Waypoint(rc_proto_v777::waypoint::Waypoint),
    /// Map data (`maps.md` § 1).
    MapData(rc_proto_v777::map::MapItemData),
    /// Titles (§ 3): title, subtitle, durations, clear (with durations reset).
    Title(Option<rc_proto::nbt::Nbt>),
    Subtitle(Option<rc_proto::nbt::Nbt>),
    TitleTimes([i32; 3]),
    ClearTitles(bool),
    /// Header and footer of the player list.
    /// World border (`weather-border.md` § 4.1).
    Border(rc_proto_v777::border::BorderPacket),
    TabList {
        header: Option<rc_proto::nbt::Nbt>,
        footer: Option<rc_proto::nbt::Nbt>,
    },
    /// Local player's health (`hud.md` § 6.1).
    Health {
        health: f32,
        food: i32,
        saturation: f32,
    },
    Experience {
        progress: f32,
        level: i32,
        total: i32,
    },
    /// Hotbar slot imposed by the server.
    HeldSlot(i32),
    /// Inventory packet (decoded stacks).
    Inventory(rc_proto_v777::item::Inventory),
    /// Opening of a container screen: window, type (`menu` registry), title.
    OpenScreen {
        window: i32,
        kind: i32,
        title: Option<rc_text::Component>,
    },
    /// Close imposed by the server.
    ContainerClosed,
    /// Merchant offers (`special-screens.md` § 1.1).
    MerchantOffers(Box<rc_proto_v777::special::MerchantOffers>),
    /// Item sets of the recipes and stonecutter recipes.
    UpdateRecipes(Box<rc_proto_v777::special::Recipes>),
    /// Mount inventory: window, columns, entity (§ 2.1).
    MountScreen {
        window: i32,
        columns: i32,
        entity: i32,
    },
    /// Written book to read, held in this hand (0 main) (§ 4.1).
    OpenBook(i32),
    /// Sign editor: position, front face (§ 5).
    OpenSignEditor {
        pos: [i32; 3],
        front: bool,
    },
    /// Property of a menu (flame, arrow…).
    ContainerData {
        window: i32,
        prop: i16,
        value: i16,
    },
    /// Entity packet.
    Entity(EntityEvent),
    /// Synchronised registry received during configuration (variants): entries and content if it is
    /// sent (absent for the entries of a known pack).
    Registry {
        name: String,
        entries: RegistryEntries,
    },
    /// World spawn point (compasses).
    SpawnPosition {
        dimension: String,
        pos: [i32; 3],
    },
    /// Death of a player (death screen if it is the local player).
    Died {
        entity: i32,
        message: Option<rc_text::Component>,
    },
    Chunk(Box<ChunkPacket>),
    ForgetChunk {
        x: i32,
        z: i32,
    },
    BlockUpdate {
        pos: BlockPos,
        state: u32,
    },
    SectionUpdate {
        section: [i32; 3],
        changes: Vec<(u16, u32)>,
    },
    /// Acknowledgement of the predicted actions up to this sequence (`interactions.md` § 0.1).
    BlockChangedAck(i32),
    /// Miner's crack: entity id, block, stage (`interactions.md` § 2).
    BlockDestruction {
        id: i32,
        pos: [i32; 3],
        stage: u8,
    },
    /// Block event (`block-entities.md` § 1.4): position, type, parameter, block.
    BlockEvent {
        pos: [i32; 3],
        b0: u8,
        b1: u8,
        block: i32,
    },
    /// Block entity data (§ 1.2): position, type, NBT.
    BlockEntityData {
        pos: [i32; 3],
        kind: i32,
        nbt: Option<rc_proto::nbt::Nbt>,
    },
    ChunkCenter {
        x: i32,
        z: i32,
    },
    ViewDistance(i32),
    /// Raw server teleportation: the play thread resolves it against the player's current state
    /// (relative or absolute positions, rotations and speed) then accepts it, like the game.
    Teleport(rc_proto_v777::play::PlayerPosition),
    Disconnected(String),
    /// Disconnection reason sent by the server (login, configuration or play), just
    /// before `Disconnected`.
    Kicked(rc_text::Component),
    /// Connection accepted (`login_finished`): the connection screen switches to "joining".
    LoginFinished,
    /// State of the connection screen during the login phase (translation key:
    /// `connect.authorizing`, `connect.encrypting`, `connect.negotiating`).
    LoginStatus(&'static str),
    /// Cooldown of an item group (`cooldown`): duration in ticks, 0 = cooldown removed.
    Cooldown {
        group: String,
        ticks: i32,
    },
    Abilities(rc_proto_v777::play::Abilities),
    /// Attributes of an entity (the local player: identifier received on entering the game).
    Attributes {
        entity: i32,
        attributes: Vec<rc_proto_v777::play::Attribute>,
    },
    /// Tags per registry (received in configuration and in play).
    Tags(rc_proto_v777::play::Tags),
    /// Heavy packet decoded on the pool but not yet used (keeps its rank).
    Skipped,
    /// Bundle (`bundle_delimiter`): events applied as one block, in the same frame.
    Bundle(Vec<Event>),
    /// Server tick rate (`protocol-supplements.md` § 2).
    TickingState {
        rate: f32,
        frozen: bool,
    },
    TickingStep(i32),
    /// Explosion (§ 3).
    Explode(Box<extra::Explosion>),
    /// Enforced rotation (§ 4): the reply is sent from the play thread.
    PlayerRotation {
        yaw: f32,
        yaw_relative: bool,
        pitch: f32,
        pitch_relative: bool,
    },
    /// Look turned towards a point or an entity (§ 5): from the eyes or the feet.
    LookAt {
        eyes: bool,
        target: [f64; 3],
        /// Targeted entity and "eyes" anchor.
        entity: Option<(i32, bool)>,
    },
    /// Camera placed on an entity (§ 6).
    SetCamera(i32),
    /// Enforced vehicle position (§ 7).
    MoveVehicle {
        pos: [f64; 3],
        yaw: f32,
        pitch: f32,
    },
    /// Transfer to another server (§ 10): last event of the session.
    Transfer {
        host: String,
        port: u16,
    },
    /// Enforced resource pack (§ 9, `resource-packs.md` § 5); URL already validated (http/https).
    PackPush {
        id: u128,
        url: String,
        hash: String,
        required: bool,
        prompt: Option<rc_text::Component>,
    },
    /// Removal of a server pack (`None`: all).
    PackPop(Option<u128>),
    /// Enabled features (§ 11).
    EnabledFeatures(Vec<String>),
    /// Recipe book (`creative-inventory.md` § 9).
    Recipes(recipes::RecipeEvent),
    /// Chat carried over from the previous session discarded (§ 11).
    ResetChat,
    ReportDetails(Vec<(String, String)>),
    ServerLinks(Vec<extra::ServerLink>),
    /// Server dialogs (§ 12).
    ShowDialog(extra::DialogRef),
    ClearDialog,
    /// Biomes of replaced columns (§ 13): (x, z, containers of all the sections).
    ChunkBiomes(Vec<(i32, i32, Vec<rc_world::Container>)>),
    SimulationDistance(i32),
    Difficulty {
        id: i32,
        locked: bool,
    },
    /// Isolated packets (`misc.rs`).
    Misc(misc::Misc),
    /// Progress and statistics (`progress.rs`).
    Progress(progress::Progress),
    /// Requested game rule values (`game_rule_values`): identifier, value.
    GameRuleValues(Vec<(String, String)>),
    /// Server code of conduct (configuration): text; the reply waits for the player.
    CodeOfConduct(String),
    /// Reply to a block or entity tag query (F3+I): transaction, data.
    TagQuery {
        id: i32,
        nbt: Option<rc_proto::nbt::Nbt>,
    },
    /// State of a test instance block (screen open): text, optional size.
    TestInstanceStatus {
        status: Option<rc_text::Component>,
        size: Option<[i32; 3]>,
    },
    /// Position reported by `/test pos`: absolute block, coordinates relative to the test.
    GameTestHighlight {
        absolute: [i32; 3],
        relative: [i32; 3],
    },
}

/// Entries of a synchronised registry: name and optional content.
pub type RegistryEntries = Vec<(String, Option<rc_proto::nbt::Nbt>)>;

/// Decoded entity packets (`docs/specs/entities.md` part 1).
#[derive(Debug, Clone, PartialEq)]
pub enum EntityEvent {
    Add(Box<rc_proto_v777::entity::AddEntity>),
    Remove(Vec<i32>),
    Move(rc_proto_v777::entity::Move),
    Teleport(rc_proto_v777::entity::TeleportEntity),
    Sync(rc_proto_v777::entity::PositionSync),
    Head { id: i32, yaw: f32 },
    Motion { id: i32, velocity: [f64; 3] },
    Data { id: i32, values: Vec<(u8, rc_proto_v777::entity::Meta)> },
    Equipment { id: i32, slots: rc_proto_v777::entity::Equipment },
    Event { id: i32, event: i8 },
    Swing { id: i32, hand: i32, kind: i32, duration: i32 },
    /// `hurt_animation`: shake and direction of the hit (living).
    Hurt { id: i32, yaw: f32 },
    /// `damage_event`: entity hit, damage type (index of the `damage_type` registry).
    Damage { id: i32, source: i32 },
    Passengers { vehicle: i32, passengers: Vec<i32> },
    Effect { id: i32, effect: i32, amplifier: i32, duration: i32, flags: i8 },
    RemoveEffect { id: i32, effect: i32 },
    /// Entity animation: 0 wake-up (end of sleep), 1 critical hit, 2 magic hit.
    Animate { id: i32, action: u8 },
}

pub struct Session {
    pub events: crossbeam_channel::Receiver<Event>,
    pub data: Arc<rc_data::VersionData>,
    /// Sending to the server, usable from the game logic.
    pub link: Arc<link::Link>,
}

impl Session {
    /// Session without a server (out-of-world screens): no events, sends have no effect.
    pub fn offline(data: Arc<rc_data::VersionData>) -> Result<Session, Error> {
        let ids = ids::Ids::resolve(&data)?;
        let (_, events) = crossbeam_channel::unbounded();
        let names = protolog::Names::new(&data);
        Ok(Session { events, data, link: Arc::new(link::Link::new(None, ids, names, Cookies::default())) })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("network: {0}")]
    Io(#[from] std::io::Error),
    #[error("unknown packet in the version data: {0}")]
    MissingPacket(&'static str),
    #[error("protocol: {0}")]
    Proto(#[from] rc_proto::Error),
    #[error("transport: {0}")]
    Net(#[from] rc_net::Error),
    #[error("{0}")]
    Invalid(String),
}

/// Injury sound of the player according to the `effects` field of the damage type (common to the client, which
/// plays it on `damage_event`, and to the integrated server).
pub fn player_hurt_sound(effects: &str) -> &'static str {
    match effects {
        "drowning" => "minecraft:entity.player.hurt_drown",
        "burning" => "minecraft:entity.player.hurt_on_fire",
        "poking" => "minecraft:entity.player.hurt_sweet_berry_bush",
        "freezing" => "minecraft:entity.player.hurt_freeze",
        // "hurt" and "thorns".
        _ => "minecraft:entity.player.hurt",
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn offline_uuid_matches_reference() {
        // Reference value computed by the official server for "Banc" (usercache.json).
        assert_eq!(format!("{:032x}", super::offline_uuid("Banc")), REFERENCE.replace('-', ""),);
    }

    const REFERENCE: &str = "6c36f763-6526-3b48-9820-8f008743cbeb";
}
