//! Client-side block entities (`docs/specs/block-entities.md` § 1): created from the block state,
//! data received with the chunk or through `block_entity_data`, local animations (`block_event`,
//! client tick). Only the types that have their own rendering (§ 2) are tracked.

use hashbrown::HashMap;

use rc_data::VersionData;
use rc_proto::nbt::Nbt;

/// Block entity types that are drawn (§ 2).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Kind {
    Sign,
    HangingSign,
    Chest,
    TrappedChest,
    EnderChest,
    ShulkerBox,
    Bell,
    Banner,
    Skull,
    DecoratedPot,
    Campfire,
    Shelf,
    Brushable,
    Spawner,
    TrialSpawner,
    Vault,
    Beacon,
    EndPortal,
    EndGateway,
    Conduit,
    EnchantingTable,
    Lectern,
    CopperGolemStatue,
    Piston,
    TestInstance,
    PotentSulfur,
}

impl Kind {
    /// Name in the `block_entity_type` registry.
    fn registry_name(self) -> &'static str {
        match self {
            Kind::Sign => "minecraft:sign",
            Kind::HangingSign => "minecraft:hanging_sign",
            Kind::Chest => "minecraft:chest",
            Kind::TrappedChest => "minecraft:trapped_chest",
            Kind::EnderChest => "minecraft:ender_chest",
            Kind::ShulkerBox => "minecraft:shulker_box",
            Kind::Bell => "minecraft:bell",
            Kind::Banner => "minecraft:banner",
            Kind::Skull => "minecraft:skull",
            Kind::DecoratedPot => "minecraft:decorated_pot",
            Kind::Campfire => "minecraft:campfire",
            Kind::Shelf => "minecraft:shelf",
            Kind::Brushable => "minecraft:brushable_block",
            Kind::Spawner => "minecraft:mob_spawner",
            Kind::TrialSpawner => "minecraft:trial_spawner",
            Kind::Vault => "minecraft:vault",
            Kind::Beacon => "minecraft:beacon",
            Kind::EndPortal => "minecraft:end_portal",
            Kind::EndGateway => "minecraft:end_gateway",
            Kind::Conduit => "minecraft:conduit",
            Kind::EnchantingTable => "minecraft:enchanting_table",
            Kind::Lectern => "minecraft:lectern",
            Kind::CopperGolemStatue => "minecraft:copper_golem_statue",
            Kind::Piston => "minecraft:piston",
            Kind::TestInstance => "minecraft:test_instance_block",
            Kind::PotentSulfur => "minecraft:potent_sulfur",
        }
    }

    const ALL: [Kind; 26] = [
        Kind::Sign,
        Kind::HangingSign,
        Kind::Chest,
        Kind::TrappedChest,
        Kind::EnderChest,
        Kind::ShulkerBox,
        Kind::Bell,
        Kind::Banner,
        Kind::Skull,
        Kind::DecoratedPot,
        Kind::Campfire,
        Kind::Shelf,
        Kind::Brushable,
        Kind::Spawner,
        Kind::TrialSpawner,
        Kind::Vault,
        Kind::Beacon,
        Kind::EndPortal,
        Kind::EndGateway,
        Kind::Conduit,
        Kind::EnchantingTable,
        Kind::Lectern,
        Kind::CopperGolemStatue,
        Kind::Piston,
        Kind::TestInstance,
        Kind::PotentSulfur,
    ];
}

/// Dye colours in registry order.
pub const DYES: [&str; 16] = [
    "white",
    "orange",
    "magenta",
    "light_blue",
    "yellow",
    "lime",
    "pink",
    "gray",
    "light_gray",
    "cyan",
    "purple",
    "blue",
    "brown",
    "green",
    "red",
    "black",
];

pub fn dye_index(name: &str) -> Option<u8> {
    DYES.iter().position(|d| *d == name).map(|i| i as u8)
}

/// Heads and skulls (`items.md` § 1.4.9), in `sub` order.
pub const SKULLS: [&str; 7] = [
    "skeleton_skull",
    "wither_skeleton_skull",
    "zombie_head",
    "creeper_head",
    "dragon_head",
    "piglin_head",
    "player_head",
];

/// Carrier block: type, variant (`sub`: dye, oxidation, head kind…), on a wall.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Carrier {
    pub kind: Kind,
    pub sub: u8,
    pub wall: bool,
}

/// Oxidation of a copper block name (wax ignored): 0 to 3.
fn oxidation(name: &str) -> u8 {
    if name.contains("exposed") {
        1
    } else if name.contains("weathered") {
        2
    } else if name.contains("oxidized") {
        3
    } else {
        0
    }
}

/// Drawn block entity type carried by a block (name without namespace).
pub fn carrier(name: &str) -> Option<Carrier> {
    let c = |kind, sub, wall| Some(Carrier { kind, sub, wall });
    let wall = name.contains("wall_");
    match name {
        "chest" => return c(Kind::Chest, 0, false),
        "trapped_chest" => return c(Kind::TrappedChest, 0, false),
        "ender_chest" => return c(Kind::EnderChest, 0, false),
        "shulker_box" => return c(Kind::ShulkerBox, 16, false),
        "bell" => return c(Kind::Bell, 0, false),
        "decorated_pot" => return c(Kind::DecoratedPot, 0, false),
        "campfire" | "soul_campfire" => return c(Kind::Campfire, 0, false),
        "suspicious_sand" | "suspicious_gravel" => return c(Kind::Brushable, 0, false),
        "spawner" => return c(Kind::Spawner, 0, false),
        "trial_spawner" => return c(Kind::TrialSpawner, 0, false),
        "vault" => return c(Kind::Vault, 0, false),
        "beacon" => return c(Kind::Beacon, 0, false),
        "end_portal" => return c(Kind::EndPortal, 0, false),
        "end_gateway" => return c(Kind::EndGateway, 0, false),
        "conduit" => return c(Kind::Conduit, 0, false),
        "enchanting_table" => return c(Kind::EnchantingTable, 0, false),
        "lectern" => return c(Kind::Lectern, 0, false),
        "moving_piston" => return c(Kind::Piston, 0, false),
        "test_instance_block" => return c(Kind::TestInstance, 0, false),
        "potent_sulfur" => return c(Kind::PotentSulfur, 0, false),
        _ => {}
    }
    if name.ends_with("copper_chest") {
        return c(Kind::Chest, 1 + oxidation(name), false);
    }
    if name.ends_with("copper_golem_statue") {
        return c(Kind::CopperGolemStatue, oxidation(name), false);
    }
    if let Some(color) = name.strip_suffix("_shulker_box") {
        return dye_index(color).and_then(|d| c(Kind::ShulkerBox, d, false));
    }
    if name.ends_with("_banner") {
        let color = name.trim_end_matches("_banner").trim_end_matches("_wall");
        return dye_index(color).and_then(|d| c(Kind::Banner, d, wall));
    }
    if name.ends_with("_hanging_sign") {
        return c(Kind::HangingSign, 0, wall);
    }
    if name.ends_with("_sign") {
        return c(Kind::Sign, 0, wall);
    }
    if name.ends_with("_shelf") {
        return c(Kind::Shelf, 0, false);
    }
    let skull = name.replace("_wall_", "_");
    if let Some(i) = SKULLS.iter().position(|s| *s == skull) {
        return c(Kind::Skull, i as u8, wall);
    }
    None
}

/// State properties useful for rendering, read once on each state change.
#[derive(Clone, Copy, Default, Debug)]
pub struct Props {
    /// 3D direction (0 down, 1 up, 2 north, 3 south, 4 west, 5 east), 255 for none.
    pub facing: u8,
    /// Rotation on the ground (0–15).
    pub rotation: u8,
    /// Chest: 0 single, 1 left, 2 right.
    pub chest: u8,
    pub powered: bool,
    pub has_book: bool,
    /// Statue: 0 standing, 1 running, 2 sitting, 3 star.
    pub pose: u8,
    /// Trial spawner waiting for players or active.
    pub active: bool,
    pub dusted: u8,
    /// Bell: 0 floor, 1 ceiling, 2 wall, 3 between two walls.
    pub attachment: u8,
}

pub fn dir_index(name: &str) -> u8 {
    match name {
        "down" => 0,
        "up" => 1,
        "north" => 2,
        "south" => 3,
        "west" => 4,
        "east" => 5,
        _ => 255,
    }
}

impl Props {
    pub(crate) fn read(data: &VersionData, state: u32) -> Self {
        let mut p = Props { facing: 255, ..Default::default() };
        for (n, v) in data.state_properties(state) {
            match n {
                "facing" => p.facing = dir_index(v),
                "rotation" => p.rotation = v.parse().unwrap_or(0),
                "type" => {
                    p.chest = match v {
                        "left" => 1,
                        "right" => 2,
                        _ => 0,
                    }
                }
                "powered" => p.powered = v == "true",
                "has_book" => p.has_book = v == "true",
                "copper_golem_pose" => {
                    p.pose = match v {
                        "running" => 1,
                        "sitting" => 2,
                        "star" => 3,
                        _ => 0,
                    }
                }
                "trial_spawner_state" => p.active = matches!(v, "waiting_for_players" | "active"),
                "dusted" => p.dusted = v.parse().unwrap_or(0),
                "attachment" => {
                    p.attachment = match v {
                        "ceiling" => 1,
                        "single_wall" => 2,
                        "double_wall" => 3,
                        _ => 0,
                    }
                }
                _ => {}
            }
        }
        p
    }
}

/// Item stack read from an NBT: id (`item` registry), count.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stack {
    pub item: i32,
    pub count: i32,
}

/// Face of a sign.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SignFace {
    /// 4 text components (`None`: empty line).
    pub messages: [Option<Nbt>; 4],
    pub filtered: [Option<Nbt>; 4],
    pub color: u8,
    pub glowing: bool,
}

/// Player head profile: UUID, name, `textures` property (base64).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Profile {
    pub uuid: Option<u128>,
    pub name: Option<String>,
    pub textures: Option<String>,
}

/// Identity of a player head: the profile's UUID, otherwise a fingerprint of its `textures` property
/// (high bit set, so as not to collide with a player UUID).
pub fn head_key(p: &Profile) -> Option<u128> {
    if let Some(u) = p.uuid {
        return Some(u);
    }
    let t = p.textures.as_ref()?;
    let mut h: u128 = 0x6c62_272e_07bb_0142_62b8_2175_6295_c58d;
    for b in t.bytes() {
        h ^= u128::from(b);
        h = h.wrapping_mul(0x0000_0000_0100_0000_0000_0000_0000_013B);
    }
    Some(h | 1 << 127)
}

/// Data read from the NBT, per type (§ 1.3); default values if absent.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Data {
    #[default]
    None,
    Sign(Box<[SignFace; 2]>),
    /// Layers: (pattern asset, dye).
    Banner(Vec<(String, u8)>),
    Skull(Option<Box<Profile>>),
    /// Back, left, right, front faces: item (registry id) or nothing.
    Pot([Option<i32>; 4]),
    /// Slots (campfire 4, shelf 3); bottom alignment (shelf).
    Items(Vec<Option<Stack>>, bool),
    /// Brushable block: item and side hit.
    Brushable(Option<Stack>, Option<u8>),
    /// Spawner: id of the display entity, player range, delay.
    Spawner(Option<String>, i32, i32),
    /// Trial spawner: entity, next spawn (game time).
    Trial(Option<String>, i64),
    /// Vault: displayed item.
    Vault(Option<Stack>),
    /// Gateway: age.
    Gateway(i64),
    /// Moving piston: moved state, direction, progress, extending, source.
    Piston {
        state: u32,
        facing: u8,
        progress: f32,
        extending: bool,
        source: bool,
    },
    /// Test instance: data read (state beam according to the tests registry).
    TestInstance(Box<crate::tech_blocks::TestInstance>),
}

/// Local animated state (§ 1.4, 1.5).
#[derive(Clone, Debug, Default)]
pub struct Anim {
    /// Current and previous opening (lids, Shulker, slow rotation).
    pub open: f32,
    pub open_o: f32,
    /// Opening target (chests, Shulker).
    pub target: bool,
    /// Tick counter (bell, animated heads, conduit, table).
    pub ticks: i32,
    /// Swinging bell, direction hit.
    pub shaking: bool,
    pub dir: u8,
    /// Pot: start of the wobble (game time) and duration.
    pub wobble: Option<(i64, i32)>,
    /// Gateway: age and cooldown.
    pub age: i64,
    pub cooldown: i32,
    /// Rotations (spawner, vault, conduit): current and previous.
    pub spin: f64,
    pub spin_o: f64,
    /// Conduit: active, hunting.
    pub active: bool,
    pub hunting: bool,
    /// Table book: (rot, rot_o, t_rot, flip, flip_o, flip_t, flip_a).
    pub book: [f32; 7],
    pub seed: u64,
    /// Beacon: beam sections (RGB colour, height), sections in progress, last y
    /// swept, levels.
    pub beam: Vec<([u8; 3], i32)>,
    pub checking: Vec<([u8; 3], i32)>,
    pub last_y: i32,
    pub levels: i32,
    /// Vault: connected players (UUID) and range of the connection particles.
    pub vault_players: Vec<u128>,
    pub vault_range: f64,
}

/// Particle emitted by a block entity's tick: kind, position, velocity.
pub type Emit = (crate::particles::Kind, [f64; 3], [f64; 3]);

pub struct BlockEntity {
    pub kind: Kind,
    pub sub: u8,
    pub wall: bool,
    pub state: u32,
    pub props: Props,
    pub data: Data,
    /// Data version number (text formatting kept as long as it does not change).
    pub rev: u32,
    pub anim: Anim,
}

/// What the client tick reads from the world.
pub struct TickWorld<'a> {
    pub state: &'a dyn Fn(i32, i32, i32) -> u32,
    /// Surface height (`WORLD_SURFACE`: first air block above the highest non-air one).
    pub surface: &'a dyn Fn(i32, i32) -> i32,
    /// Players (non-spectators): feet positions.
    pub players: &'a [[f64; 3]],
    /// Identified players (UUID, feet position), for the vault's connection particles.
    pub named: &'a [(u128, [f64; 3])],
    pub game_time: i64,
    pub min_y: i32,
}

/// Tables per state / per block for the tick (beacon, conduit).
struct Tables {
    /// Carrier per block.
    carriers: Vec<Option<Carrier>>,
    /// Block entity type per network id.
    types: Vec<Option<Kind>>,
    /// Beam dye per block (stained glass and panes, beacon = white).
    beam_color: Vec<Option<u8>>,
    bedrock: u16,
    /// Beacon base block, conduit effect block (tags received).
    beacon_base: Vec<bool>,
    conduit_frame: Vec<bool>,
    /// Item id per name.
    items: HashMap<String, i32>,
    /// Noxious gas outlet above potent sulfur.
    gas: crate::particles::GasTables,
}

pub struct BlockEntities {
    /// Per column: absolute position → block entity.
    pub chunks: HashMap<(i32, i32), HashMap<[i32; 3], BlockEntity>>,
    tables: Tables,
    data: std::sync::Arc<VersionData>,
    /// List version (additions, removals, state changes).
    pub version: u64,
    /// Player heads whose skin must be requested: (identity, `textures` property).
    pub heads: Vec<(u128, String)>,
    /// Tests registry (red or orange beam of a failing instance).
    pub tests: crate::tech_blocks::TestDefs,
    /// Piston-moving blocks counted at the last tick (plus those created since): without them,
    /// physics looks for no piston shape.
    pub moving: usize,
}

/// What the tick of a moving block asks of the application.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PistonTick {
    /// Entities to push: block before the step and new progress.
    Push([i32; 3], rc_physics::piston::Moving, f32),
    /// End (5 ticks after arrival): moved block placed if the cell is still moving.
    Finish([i32; 3], u32),
}

fn get<'a>(n: &'a Nbt, k: &str) -> Option<&'a Nbt> {
    n.get(k)
}

fn as_bool(n: Option<&Nbt>) -> bool {
    n.and_then(Nbt::as_i64).is_some_and(|v| v != 0)
}

fn uuid_of(n: &Nbt) -> Option<u128> {
    match n {
        Nbt::IntArray(a) if a.len() == 4 => Some(a.iter().fold(0u128, |acc, &w| (acc << 32) | u128::from(w as u32))),
        Nbt::String(s) => u128::from_str_radix(&s.replace('-', ""), 16).ok(),
        _ => None,
    }
}

impl BlockEntities {
    pub fn new(data: std::sync::Arc<VersionData>) -> Self {
        let carriers =
            data.blocks.iter().map(|b| carrier(b.name.strip_prefix("minecraft:").unwrap_or(&b.name))).collect();
        let types = data
            .registry("minecraft:block_entity_type")
            .map(|r| r.entries.iter().map(|n| Kind::ALL.into_iter().find(|k| k.registry_name() == n)).collect())
            .unwrap_or_default();
        let beam_color = data
            .blocks
            .iter()
            .map(|b| {
                let n = b.name.strip_prefix("minecraft:").unwrap_or(&b.name);
                if n == "beacon" {
                    return Some(0);
                }
                let color = n.strip_suffix("_stained_glass").or_else(|| n.strip_suffix("_stained_glass_pane"))?;
                dye_index(color)
            })
            .collect();
        let bedrock = data.blocks.iter().position(|b| b.name == "minecraft:bedrock").unwrap_or(0) as u16;
        let items = data
            .registry("minecraft:item")
            .map(|r| r.entries.iter().enumerate().map(|(i, n)| (n.clone(), i as i32)).collect())
            .unwrap_or_default();
        BlockEntities {
            chunks: HashMap::new(),
            tables: Tables {
                carriers,
                types,
                beam_color,
                bedrock,
                beacon_base: Vec::new(),
                conduit_frame: Vec::new(),
                items,
                gas: crate::particles::GasTables::new(&data),
            },
            data,
            version: 0,
            heads: Vec::new(),
            tests: Default::default(),
            moving: 0,
        }
    }

    /// Moving block created by the replay of a piston (`moving_piston` block already placed):
    /// progress 0, end counter at zero.
    pub fn set_mover(&mut self, pos: [i32; 3], m: rc_sim::MovingBlock) {
        let Some(e) = self.get_mut(pos) else { return };
        if e.kind != Kind::Piston {
            return;
        }
        e.data = Data::Piston { state: m.moved, facing: m.dir, progress: 0.0, extending: m.extending, source: m.source };
        e.rev = e.rev.wrapping_add(1);
        e.anim.open = 0.0;
        e.anim.open_o = 0.0;
        e.anim.ticks = 0;
        self.moving += 1;
        self.version += 1;
    }

    /// Moving block at `pos` (current progress).
    pub fn mover(&self, pos: [i32; 3]) -> Option<rc_physics::piston::Moving> {
        let e = self.get(pos)?;
        match e.data {
            Data::Piston { state, facing, extending, source, .. } => Some(rc_physics::piston::Moving {
                moved: state,
                dir: facing,
                extending,
                source,
                progress: e.anim.open,
            }),
            _ => None,
        }
    }

    /// `test_instance` registry received (configuration).
    pub fn set_test_defs(&mut self, defs: crate::tech_blocks::TestDefs) {
        self.tests = defs;
    }

    /// Colour of the state beam of a test instance.
    pub fn test_beam(&self, e: &BlockEntity) -> Option<[u8; 3]> {
        match &e.data {
            Data::TestInstance(t) => t.beam(&self.tests),
            _ => None,
        }
    }

    /// Block tags received (beacon base, conduit frame).
    pub fn set_block_tags(&mut self, tags: &[(String, Vec<i32>)]) {
        let n = self.data.blocks.len();
        for (name, v) in [
            ("minecraft:beacon_base_blocks", &mut self.tables.beacon_base),
            ("minecraft:conduit_effect_block", &mut self.tables.conduit_frame),
        ] {
            *v = vec![false; n];
            if let Some((_, ids)) = tags.iter().find(|(t, _)| t == name) {
                for &i in ids {
                    if let Some(b) = usize::try_from(i).ok().and_then(|i| v.get_mut(i)) {
                        *b = true;
                    }
                }
            }
        }
    }

    pub fn clear(&mut self) {
        self.chunks.clear();
        self.version += 1;
    }

    pub fn len(&self) -> usize {
        self.chunks.values().map(HashMap::len).sum()
    }

    fn carrier_of(&self, state: u32) -> Option<Carrier> {
        let block = self.data.states.get(state as usize)?.block;
        self.tables.carriers.get(usize::from(block)).copied().flatten()
    }

    fn create(&self, state: u32, c: Carrier) -> BlockEntity {
        let mut anim = Anim::default();
        if c.kind == Kind::EnchantingTable {
            anim.seed = u64::from(state) ^ 0x5DEE_CE66;
        }
        if c.kind == Kind::Beacon {
            anim.last_y = i32::MIN;
        }
        BlockEntity {
            kind: c.kind,
            sub: c.sub,
            wall: c.wall,
            state,
            props: Props::read(&self.data, state),
            data: Data::None,
            rev: 0,
            anim,
        }
    }

    /// Chunk packet (§ 1.1): the column's block entities replaced; `state` = block state
    /// at a position (column already in place).
    pub fn load_chunk(
        &mut self,
        cx: i32,
        cz: i32,
        list: &[rc_proto_v777::chunk::BlockEntity],
        state: impl Fn(i32, i32, i32) -> u32,
    ) {
        let mut map = HashMap::new();
        for be in list {
            let pos = [cx * 16 + i32::from(be.x), i32::from(be.y), cz * 16 + i32::from(be.z)];
            let s = state(pos[0], pos[1], pos[2]);
            let Some(c) = self.carrier_of(s) else { continue };
            let mut e = self.create(s, c);
            let typed = usize::try_from(be.kind).ok().and_then(|k| self.tables.types.get(k)).copied().flatten();
            if typed == Some(c.kind)
                && let Some(n) = &be.data
            {
                self.load(&mut e, n);
                self.note_head(&e.data);
            }
            map.insert(pos, e);
        }
        if map.is_empty() {
            self.chunks.remove(&(cx, cz));
        } else {
            self.chunks.insert((cx, cz), map);
        }
        self.version += 1;
    }

    pub fn forget_chunk(&mut self, cx: i32, cz: i32) {
        if self.chunks.remove(&(cx, cz)).is_some() {
            self.version += 1;
        }
    }

    /// Block change (§ 1.1): removal if the block changes (except copper, which keeps its own),
    /// new state transmitted, or empty block entity created.
    pub fn on_block_change(&mut self, pos: [i32; 3], old: u32, new: u32) {
        if old == new {
            return;
        }
        let key = (pos[0] >> 4, pos[2] >> 4);
        let old_block = self.data.states.get(old as usize).map(|s| s.block);
        let new_block = self.data.states.get(new as usize).map(|s| s.block);
        let carrier = self.carrier_of(new);
        let keeps = |c: Option<Carrier>| {
            c.is_some_and(|c| (c.kind == Kind::Chest && c.sub > 0) || c.kind == Kind::CopperGolemStatue)
        };
        let props = Props::read(&self.data, new);
        let fresh = carrier.map(|c| self.create_with(new, c, props));
        let map = self.chunks.entry(key).or_default();
        if old_block != new_block
            && let Some(e) = map.get(&pos)
        {
            let same_family = keeps(carrier) && carrier.is_some_and(|c| c.kind == e.kind);
            if !same_family {
                map.remove(&pos);
                self.version += 1;
            }
        }
        if let (Some(c), Some(fresh)) = (carrier, fresh) {
            match map.get_mut(&pos) {
                Some(e) if e.kind == c.kind => {
                    e.state = new;
                    e.sub = c.sub;
                    e.wall = c.wall;
                    e.props = props;
                }
                _ => {
                    map.insert(pos, fresh);
                }
            }
            self.version += 1;
        } else if map.is_empty() {
            self.chunks.remove(&key);
        }
    }

    fn create_with(&self, state: u32, c: Carrier, props: Props) -> BlockEntity {
        let mut e = self.create(state, c);
        e.props = props;
        e
    }

    pub fn get(&self, pos: [i32; 3]) -> Option<&BlockEntity> {
        self.chunks.get(&(pos[0] >> 4, pos[2] >> 4))?.get(&pos)
    }

    /// Text of a sign face replaced locally (editor, `special-screens.md` § 5).
    pub fn set_sign_text(&mut self, pos: [i32; 3], front: bool, lines: &[String; 4]) {
        let Some(e) = self.get_mut(pos) else { return };
        let Data::Sign(faces) = &mut e.data else { return };
        let f = &mut faces[usize::from(!front)];
        for (i, l) in lines.iter().enumerate() {
            f.messages[i] = Some(Nbt::String(l.clone()));
            f.filtered[i] = Some(Nbt::String(l.clone()));
        }
        e.rev = e.rev.wrapping_add(1);
    }

    fn get_mut(&mut self, pos: [i32; 3]) -> Option<&mut BlockEntity> {
        self.chunks.get_mut(&(pos[0] >> 4, pos[2] >> 4))?.get_mut(&pos)
    }

    /// `block_entity_data` (§ 1.2): applied only to an existing block entity of the same type.
    pub fn on_data(&mut self, pos: [i32; 3], kind: i32, nbt: Option<&Nbt>) {
        let typed = usize::try_from(kind).ok().and_then(|k| self.tables.types.get(k)).copied().flatten();
        let Some(typed) = typed else { return };
        let empty = Nbt::Compound(Vec::new());
        let n = nbt.unwrap_or(&empty);
        // Data read before the mutable borrow.
        let Some(e) = self.get(pos) else { return };
        if e.kind != typed {
            return;
        }
        let data = self.parse(e.kind, n);
        self.note_head(&data);
        if let Some(e) = self.get_mut(pos) {
            e.data = data;
            e.rev = e.rev.wrapping_add(1);
            if e.kind == Kind::Vault {
                (e.anim.vault_players, e.anim.vault_range) = vault_shared(n);
            }
            if e.kind == Kind::EndGateway
                && let Data::Gateway(age) = e.data
            {
                e.anim.age = age;
            }
        }
    }

    /// Player head with a skin in the profile: request to make.
    fn note_head(&mut self, data: &Data) {
        if let Data::Skull(Some(p)) = data
            && let (Some(key), Some(t)) = (head_key(p), &p.textures)
        {
            self.heads.push((key, t.clone()));
        }
    }

    fn load(&self, e: &mut BlockEntity, n: &Nbt) {
        e.data = self.parse(e.kind, n);
        e.rev = e.rev.wrapping_add(1);
        if e.kind == Kind::Vault {
            (e.anim.vault_players, e.anim.vault_range) = vault_shared(n);
        }
        if let Data::Gateway(age) = e.data {
            e.anim.age = age;
        }
        if let Data::Piston { progress, .. } = e.data {
            (e.anim.open, e.anim.open_o) = (progress, progress);
        }
    }

    /// `block_event` (§ 1.4).
    pub fn on_event(&mut self, pos: [i32; 3], b0: u8, b1: u8, game_time: i64) {
        let Some(e) = self.get_mut(pos) else { return };
        if b0 != 1 {
            return;
        }
        match e.kind {
            Kind::Chest | Kind::TrappedChest | Kind::EnderChest | Kind::ShulkerBox => e.anim.target = b1 > 0,
            Kind::Bell => {
                e.anim.ticks = 0;
                e.anim.shaking = true;
                e.anim.dir = b1;
            }
            Kind::DecoratedPot => e.anim.wobble = Some((game_time, if b1 == 0 { 7 } else { 10 })),
            Kind::EndGateway => e.anim.cooldown = 40,
            _ => {}
        }
    }

    fn stack(&self, n: &Nbt) -> Option<Stack> {
        let (id, count) = match n {
            Nbt::String(s) => (s.as_str(), 1),
            _ => (get(n, "id")?.as_str()?, get(n, "count").and_then(Nbt::as_i64).unwrap_or(1) as i32),
        };
        let id = if id.contains(':') { id.to_owned() } else { format!("minecraft:{id}") };
        let item = *self.tables.items.get(&id)?;
        (count > 0 && id != "minecraft:air").then_some(Stack { item, count })
    }

    /// NBT → kept data (§ 1.3), field by field, default values if absent.
    fn parse(&self, kind: Kind, n: &Nbt) -> Data {
        match kind {
            Kind::Sign | Kind::HangingSign => {
                let face = |k: &str| {
                    let mut f = SignFace::default();
                    let Some(t) = get(n, k) else { return f };
                    let lines = |k: &str| {
                        let mut out: [Option<Nbt>; 4] = Default::default();
                        if let Some(Nbt::List(l)) = get(t, k) {
                            for (i, m) in l.iter().take(4).enumerate() {
                                out[i] = Some(m.clone());
                            }
                        }
                        out
                    };
                    f.messages = lines("messages");
                    f.filtered = if get(t, "filtered_messages").is_some() {
                        lines("filtered_messages")
                    } else {
                        f.messages.clone()
                    };
                    f.color = get(t, "color").and_then(Nbt::as_str).and_then(dye_index).unwrap_or(15);
                    f.glowing = as_bool(get(t, "has_glowing_text"));
                    f
                };
                Data::Sign(Box::new([face("front_text"), face("back_text")]))
            }
            Kind::Banner => {
                let mut out = Vec::new();
                if let Some(Nbt::List(l)) = get(n, "patterns") {
                    for p in l {
                        let color = get(p, "color").and_then(Nbt::as_str).and_then(dye_index).unwrap_or(0);
                        let asset = match get(p, "pattern") {
                            Some(Nbt::String(s)) => Some(s.clone()),
                            Some(def) => get(def, "asset_id").and_then(Nbt::as_str).map(str::to_owned),
                            None => None,
                        };
                        if let Some(a) = asset {
                            let path = a.split_once(':').map_or(a.as_str(), |(_, p)| p);
                            out.push((format!("entity/banner/{path}"), color));
                        }
                    }
                }
                Data::Banner(out)
            }
            Kind::Skull => {
                let profile = get(n, "profile").map(|p| {
                    let mut out = Profile::default();
                    match p {
                        Nbt::String(s) => out.name = Some(s.clone()),
                        _ => {
                            out.name = get(p, "name").and_then(Nbt::as_str).map(str::to_owned);
                            out.uuid = get(p, "id").and_then(uuid_of);
                            if let Some(Nbt::List(props)) = get(p, "properties") {
                                out.textures = props
                                    .iter()
                                    .find(|q| get(q, "name").and_then(Nbt::as_str) == Some("textures"))
                                    .and_then(|q| get(q, "value")?.as_str())
                                    .map(str::to_owned);
                            }
                        }
                    }
                    Box::new(out)
                });
                Data::Skull(profile)
            }
            Kind::DecoratedPot => {
                let mut faces = [None; 4];
                if let Some(s) = get(n, "sherds") {
                    match s {
                        Nbt::List(l) => {
                            for (i, it) in l.iter().take(4).enumerate() {
                                faces[i] = self.stack(it).map(|s| s.item);
                            }
                        }
                        _ => {
                            for (i, k) in ["back", "left", "right", "front"].into_iter().enumerate() {
                                faces[i] = get(s, k).and_then(|it| self.stack(it)).map(|s| s.item);
                            }
                        }
                    }
                }
                Data::Pot(faces)
            }
            Kind::Campfire | Kind::Shelf => {
                let size = if kind == Kind::Campfire { 4 } else { 3 };
                let mut slots = vec![None; size];
                if let Some(Nbt::List(l)) = get(n, "Items") {
                    for it in l {
                        let slot = get(it, "Slot").and_then(Nbt::as_i64).unwrap_or(-1);
                        if let Some(s) = usize::try_from(slot).ok().and_then(|i| slots.get_mut(i)) {
                            *s = self.stack(it);
                        }
                    }
                }
                Data::Items(slots, as_bool(get(n, "align_items_to_bottom")))
            }
            Kind::Brushable => Data::Brushable(
                get(n, "item").and_then(|i| self.stack(i)),
                get(n, "hit_direction").and_then(Nbt::as_i64).map(|d| d.clamp(0, 5) as u8),
            ),
            Kind::Spawner => {
                let id = get(n, "SpawnData")
                    .and_then(|d| get(d, "entity"))
                    .and_then(|e| get(e, "id"))
                    .and_then(Nbt::as_str)
                    .map(str::to_owned);
                let range = get(n, "RequiredPlayerRange").and_then(Nbt::as_i64).unwrap_or(16) as i32;
                let delay = get(n, "Delay").and_then(Nbt::as_i64).unwrap_or(20) as i32;
                Data::Spawner(id, range, delay)
            }
            Kind::TrialSpawner => {
                let id = get(n, "spawn_data")
                    .and_then(|d| get(d, "entity"))
                    .and_then(|e| get(e, "id"))
                    .and_then(Nbt::as_str)
                    .map(str::to_owned);
                Data::Trial(id, get(n, "next_mob_spawns_at").and_then(Nbt::as_i64).unwrap_or(0))
            }
            Kind::Vault => {
                Data::Vault(get(n, "shared_data").and_then(|d| get(d, "display_item")).and_then(|i| self.stack(i)))
            }
            Kind::EndGateway => Data::Gateway(get(n, "Age").and_then(Nbt::as_i64).unwrap_or(0)),
            Kind::TestInstance => Data::TestInstance(Box::new(crate::tech_blocks::TestInstance::from_nbt(n))),
            Kind::Piston => {
                let state = get(n, "blockState").map_or(0, |b| self.state_of(b));
                Data::Piston {
                    state,
                    facing: get(n, "facing").and_then(Nbt::as_i64).unwrap_or(0).clamp(0, 5) as u8,
                    progress: match get(n, "progress") {
                        Some(Nbt::Float(f)) => *f,
                        _ => 0.0,
                    },
                    extending: as_bool(get(n, "extending")),
                    source: as_bool(get(n, "source")),
                }
            }
            _ => Data::None,
        }
    }

    /// Block state of a `{Name, Properties}` compound.
    fn state_of(&self, n: &Nbt) -> u32 {
        let Some(name) = get(n, "Name").and_then(Nbt::as_str) else { return 0 };
        let name = if name.contains(':') { name.to_owned() } else { format!("minecraft:{name}") };
        let Some(b) = self.data.blocks.iter().find(|b| b.name == name) else { return 0 };
        let props: Vec<(&str, &str)> = match get(n, "Properties") {
            Some(Nbt::Compound(f)) => f.iter().filter_map(|(k, v)| Some((k.as_str(), v.as_str()?))).collect(),
            _ => Vec::new(),
        };
        if props.is_empty() {
            return b.default_state;
        }
        (b.first_state..b.first_state + b.state_count)
            .find(|&s| {
                self.data
                    .state_properties(s)
                    .all(|(k, v)| props.iter().find(|(pk, _)| *pk == k).is_none_or(|(_, pv)| *pv == v))
            })
            .unwrap_or(b.default_state)
    }

    /// Client tick (§ 1.5); `out` receives the particles emitted by the blocks. Moving
    /// blocks are only counted: they advance in `tick_pistons`, after the local
    /// player's tick (the game ticks entities before block entities).
    pub fn tick(&mut self, w: &TickWorld<'_>, out: &mut Vec<Emit>) {
        let tables = &self.tables;
        let data = &self.data;
        let mut moving = 0;
        for map in self.chunks.values_mut() {
            for (pos, e) in map.iter_mut() {
                if e.kind == Kind::Piston {
                    moving += 1;
                    continue;
                }
                tick_one(e, *pos, w, tables, data);
                if matches!(e.kind, Kind::Vault | Kind::TrialSpawner) {
                    emit_particles(e, *pos, w, data, out);
                } else if e.kind == Kind::Campfire {
                    campfire_particles(e, *pos, w, data, out);
                } else if e.kind == Kind::PotentSulfur && w.game_time % 20 == 0 {
                    sulfur_gas(*pos, w, tables, data, out);
                }
            }
        }
        self.moving = moving;
    }

    /// Step of the moving blocks; `out` receives their pushes and their ends. Nothing to walk
    /// without a moving block.
    pub fn tick_pistons(&mut self, out: &mut Vec<PistonTick>) {
        if self.moving == 0 {
            return;
        }
        for map in self.chunks.values_mut() {
            for (pos, e) in map.iter_mut().filter(|(_, e)| e.kind == Kind::Piston) {
                piston_tick(e, *pos, out);
            }
        }
    }
}

/// Client tick of a moving block: step of 0.5 (entities pushed before the step), then
/// 5 ticks of waiting once arrived before placing the block itself (the server, master of the
/// final block, has usually sent it before).
fn piston_tick(e: &mut BlockEntity, pos: [i32; 3], out: &mut Vec<PistonTick>) {
    let Data::Piston { state, facing, extending, source, .. } = e.data else { return };
    let a = &mut e.anim;
    a.open_o = a.open;
    if a.open_o >= 1.0 {
        if a.ticks < 5 {
            a.ticks += 1;
        } else {
            out.push(PistonTick::Finish(pos, state));
        }
    } else {
        let next = a.open + 0.5;
        let m = rc_physics::piston::Moving { moved: state, dir: facing, extending, source, progress: a.open };
        out.push(PistonTick::Push(pos, m, next));
        a.open = next.min(1.0);
    }
}

fn wrap_pi(mut a: f32) -> f32 {
    use std::f32::consts::PI;
    while a >= PI {
        a -= 2.0 * PI;
    }
    while a < -PI {
        a += 2.0 * PI;
    }
    a
}

fn next_int(seed: &mut u64, bound: u32) -> i32 {
    *seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
    ((*seed >> 33) % u64::from(bound)) as i32
}

fn tick_one(e: &mut BlockEntity, pos: [i32; 3], w: &TickWorld<'_>, t: &Tables, data: &VersionData) {
    let a = &mut e.anim;
    let center = [f64::from(pos[0]) + 0.5, f64::from(pos[1]) + 0.5, f64::from(pos[2]) + 0.5];
    let nearest = |r: f64| {
        w.players
            .iter()
            .map(|p| (p, (0..3).map(|i| (p[i] - center[i]).powi(2)).sum::<f64>()))
            .filter(|(_, d)| *d <= r * r)
            .min_by(|x, y| x.1.total_cmp(&y.1))
            .map(|(p, _)| *p)
    };
    match e.kind {
        Kind::Chest | Kind::TrappedChest | Kind::EnderChest => {
            a.open_o = a.open;
            if !a.target && a.open > 0.0 {
                a.open = (a.open - 0.1).max(0.0);
            } else if a.target && a.open < 1.0 {
                a.open = (a.open + 0.1).min(1.0);
            }
        }
        Kind::ShulkerBox => {
            a.open_o = a.open;
            if a.target {
                a.open = (a.open + 0.1).min(1.0);
            } else {
                a.open = (a.open - 0.1).max(0.0);
            }
        }
        Kind::Bell => {
            if a.shaking {
                a.ticks += 1;
                if a.ticks >= 50 {
                    a.shaking = false;
                    a.ticks = 0;
                }
            }
        }
        Kind::Skull => {
            if e.props.powered {
                a.ticks += 1;
            }
        }
        Kind::EndGateway => {
            a.age += 1;
            if a.cooldown > 0 {
                a.cooldown -= 1;
            }
        }
        Kind::EnchantingTable => {
            let b = &mut a.book;
            a.open_o = a.open;
            b[1] = b[0];
            if let Some(p) = nearest(3.0) {
                let (xd, zd) = (p[0] - center[0], p[2] - center[2]);
                b[2] = zd.atan2(xd) as f32;
                a.open += 0.1;
                if a.open < 0.5 || next_int(&mut a.seed, 40) == 0 {
                    let old = b[5];
                    loop {
                        b[5] += (next_int(&mut a.seed, 4) - next_int(&mut a.seed, 4)) as f32;
                        if b[5] != old {
                            break;
                        }
                    }
                }
            } else {
                b[2] += 0.02;
                a.open -= 0.1;
            }
            b[0] = wrap_pi(b[0]);
            b[2] = wrap_pi(b[2]);
            let dir = wrap_pi(b[2] - b[0]);
            b[0] += dir * 0.4;
            a.open = a.open.clamp(0.0, 1.0);
            a.ticks += 1;
            b[4] = b[3];
            let diff = ((b[5] - b[3]) * 0.4).clamp(-0.2, 0.2);
            b[6] += (diff - b[6]) * 0.9;
            b[3] += b[6];
        }
        Kind::Spawner => {
            if let Data::Spawner(_, range, delay) = &mut e.data
                && nearest(f64::from(*range)).is_some()
            {
                if *delay > 0 {
                    *delay -= 1;
                }
                a.spin_o = a.spin;
                a.spin = (a.spin + 1000.0 / (f64::from(*delay) + 200.0)) % 360.0;
            } else {
                a.spin_o = a.spin;
            }
        }
        Kind::TrialSpawner => {
            let next = match e.data {
                Data::Trial(_, n) => n,
                _ => 0,
            };
            let v = if e.props.active && matches!(e.data, Data::Trial(..)) { 1000.0 } else { 200.0 };
            a.spin_o = a.spin;
            a.spin = (a.spin + v / ((next - w.game_time).max(0) as f64 + 200.0)) % 360.0;
        }
        Kind::Vault => {
            a.spin_o = a.spin;
            a.spin = (a.spin + 10.0) % 360.0;
            if a.spin > 180.0 {
                a.spin -= 360.0;
                a.spin_o -= 360.0;
            }
        }
        Kind::Conduit => {
            a.ticks += 1;
            if w.game_time % 40 == 0 {
                let (active, count) = conduit_shape(pos, w, t, data);
                a.active = active;
                a.hunting = count >= 42;
            }
            if a.active {
                a.spin += 1.0;
            }
        }
        Kind::Beacon => beacon_tick(a, pos, w, t, data),
        _ => {}
    }
}

fn next_f(seed: &mut u64) -> f64 {
    f64::from(next_int(seed, 1 << 24)) / f64::from(1 << 24)
}

/// Particles emitted by the client tick of the vault and the trial spawner (states read
/// from the block: `ominous`, `facing`, `trial_spawner_state`).
fn emit_particles(e: &mut BlockEntity, pos: [i32; 3], w: &TickWorld<'_>, data: &VersionData, out: &mut Vec<Emit>) {
    use crate::particles::Kind as P;
    let state = (w.state)(pos[0], pos[1], pos[2]);
    let props: Vec<(&str, &str)> = data.state_properties(state).collect();
    let prop = |k: &str| props.iter().find(|p| p.0 == k).map(|p| p.1);
    let ominous = prop("ominous") == Some("true");
    let p = [f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2])];
    let seed = &mut e.anim.seed;
    let none = [0.0; 3];
    match e.kind {
        Kind::Vault => {
            if w.game_time % 20 == 0 && !e.anim.vault_players.is_empty() {
                let (dx, dz) = match prop("facing") {
                    Some("north") => (0.0, -0.5),
                    Some("south") => (0.0, 0.5),
                    Some("west") => (-0.5, 0.0),
                    Some("east") => (0.5, 0.0),
                    _ => (0.0, 0.0),
                };
                let keyhole = [p[0] + 0.5 + dx, p[1] + 1.75, p[2] + 0.5 + dz];
                for id in &e.anim.vault_players {
                    let Some((_, pp)) = w.named.iter().find(|(u, _)| u == id) else { continue };
                    let b = [pp[0].floor() - p[0], pp[1].floor() - p[1], pp[2].floor() - p[2]];
                    if b.iter().map(|v| v * v).sum::<f64>() > e.anim.vault_range * e.anim.vault_range {
                        continue;
                    }
                    let to = [pp[0] - keyhole[0], pp[1] + 0.9 - keyhole[1], pp[2] - keyhole[2]];
                    for _ in 0..2 + next_int(seed, 4) {
                        let v = to.map(|c| c + (next_f(seed) - 0.5));
                        out.push((P::VaultConnection, keyhole, v));
                    }
                }
            }
            if next_f(seed) <= 0.5 {
                let at = [
                    p[0] + 0.1 + next_f(seed) * 0.8,
                    p[1] + 0.25 + next_f(seed) * 0.5,
                    p[2] + 0.1 + next_f(seed) * 0.8,
                ];
                out.push((P::Smoke, at, none));
                if matches!(e.data, Data::Vault(Some(_))) {
                    out.push((if ominous { P::SoulFireFlame } else { P::SmallFlame }, at, none));
                }
            }
        }
        Kind::TrialSpawner => {
            let c = [p[0] + 0.5, p[1] + 0.5, p[2] + 0.5];
            let around = |s: &mut u64, k: f64| c.map(|v| v + (next_f(s) - 0.5) * k);
            match prop("trial_spawner_state") {
                Some("waiting_for_players" | "waiting_for_reward_ejection" | "ejecting_reward") => {
                    if next_int(seed, 2) == 0 {
                        out.push((if ominous { P::SoulFireFlame } else { P::SmallFlame }, around(seed, 0.9), none));
                    }
                }
                Some("active") => {
                    let at = around(seed, 1.0);
                    out.push((P::Smoke, at, none));
                    out.push((if ominous { P::SoulFireFlame } else { P::Flame }, at, none));
                }
                Some("cooldown") => {
                    let at = around(seed, 0.9);
                    if next_int(seed, 3) == 0 {
                        out.push((P::Smoke, at, none));
                    }
                    if w.game_time % 20 == 0 {
                        let top = [c[0], c[1] + 0.5, c[2]];
                        for _ in 0..20 + next_int(seed, 4) {
                            out.push((P::Smoke, top, none));
                        }
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }
}

/// Connected players and range of a vault (`shared_data`).
fn vault_shared(n: &Nbt) -> (Vec<u128>, f64) {
    let shared = get(n, "shared_data");
    let players = shared
        .and_then(|d| get(d, "connected_players"))
        .and_then(|l| match l {
            Nbt::List(items) => Some(
                items
                    .iter()
                    .filter_map(|i| match i {
                        Nbt::IntArray(a) if a.len() == 4 => {
                            Some(a.iter().fold(0u128, |acc, &x| (acc << 32) | u128::from(x as u32)))
                        }
                        _ => None,
                    })
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default();
    let range = match shared.and_then(|d| get(d, "connected_particles_range")) {
        Some(Nbt::Double(v)) => *v,
        _ => 4.5,
    };
    (players, range)
}

/// Conduit frame (§ 4.12): 27 water cells then effect blocks at the distance-2 crosses.
fn conduit_shape(pos: [i32; 3], w: &TickWorld<'_>, t: &Tables, data: &VersionData) -> (bool, usize) {
    let water = |s: u32| {
        data.states
            .get(s as usize)
            .and_then(|i| data.fluids.get(usize::from(i.fluid)))
            .is_some_and(|f| f.contains("water"))
    };
    for ox in -1..=1 {
        for oy in -1..=1 {
            for oz in -1..=1 {
                if !water((w.state)(pos[0] + ox, pos[1] + oy, pos[2] + oz)) {
                    return (false, 0);
                }
            }
        }
    }
    let mut count = 0;
    for ox in -2i32..=2 {
        for oy in -2i32..=2 {
            for oz in -2i32..=2 {
                let (ax, ay, az) = (ox.abs(), oy.abs(), oz.abs());
                if (ax > 1 || ay > 1 || az > 1)
                    && ((ox == 0 && (ay == 2 || az == 2))
                        || (oy == 0 && (ax == 2 || az == 2))
                        || (oz == 0 && (ax == 2 || ay == 2)))
                {
                    let s = (w.state)(pos[0] + ox, pos[1] + oy, pos[2] + oz);
                    let b = data.states.get(s as usize).map_or(0, |i| usize::from(i.block));
                    if t.conduit_frame.get(b).copied().unwrap_or(false) {
                        count += 1;
                    }
                }
            }
        }
    }
    (count >= 16, count)
}

/// Beacon beam (§ 4.9): sweep of 10 blocks per tick, levels every 80 ticks.
fn beacon_tick(a: &mut Anim, pos: [i32; 3], w: &TickWorld<'_>, t: &Tables, data: &VersionData) {
    let surface = (w.surface)(pos[0], pos[2]);
    let mut y = if a.last_y < pos[1] {
        a.checking.clear();
        a.last_y = pos[1] - 1;
        pos[1]
    } else {
        a.last_y + 1
    };
    let mut i = 0;
    while i < 10 && y <= surface {
        let s = (w.state)(pos[0], y, pos[2]);
        let info = data.states.get(s as usize);
        let block = info.map_or(0, |i| i.block);
        match t.beam_color.get(usize::from(block)).copied().flatten() {
            Some(d) => {
                let c = dye_rgb(d);
                if a.checking.len() <= 1 {
                    a.checking.push((c, 1));
                } else if let Some(last) = a.checking.last_mut() {
                    if last.0 == c {
                        last.1 += 1;
                    } else {
                        let avg = [0, 1, 2].map(|k| ((u16::from(last.0[k]) + u16::from(c[k])) / 2) as u8);
                        a.checking.push((avg, 1));
                    }
                }
            }
            None => {
                let opaque = info.is_some_and(|i| i.light_dampening >= 15) && block != t.bedrock;
                match a.checking.last_mut() {
                    Some(last) if !opaque => last.1 += 1,
                    _ => {
                        a.checking.clear();
                        a.last_y = surface;
                        break;
                    }
                }
            }
        }
        y += 1;
        a.last_y += 1;
        i += 1;
    }
    if w.game_time % 80 == 0 && !a.beam.is_empty() {
        a.levels = beacon_levels(pos, w, t, data);
    }
    if a.last_y >= surface {
        a.last_y = w.min_y - 1;
        a.beam = std::mem::take(&mut a.checking);
    }
}

fn beacon_levels(pos: [i32; 3], w: &TickWorld<'_>, t: &Tables, data: &VersionData) -> i32 {
    let mut levels = 0;
    for step in 1..=4 {
        let y = pos[1] - step;
        if y < w.min_y {
            break;
        }
        let ok = (pos[0] - step..=pos[0] + step).all(|x| {
            (pos[2] - step..=pos[2] + step).all(|z| {
                let s = (w.state)(x, y, z);
                let b = data.states.get(s as usize).map_or(0, |i| usize::from(i.block));
                t.beacon_base.get(b).copied().unwrap_or(false)
            })
        });
        if !ok {
            break;
        }
        levels = step;
    }
    levels
}

/// Diffusion colour of a dye (`items.md` § 1.4).
pub fn dye_rgb(d: u8) -> [u8; 3] {
    const C: [u32; 16] = [
        0xF9FFFE, 0xF9801D, 0xC74EBD, 0x3AB3DA, 0xFED83D, 0x80C71F, 0xF38BAA, 0x474F52, 0x9D9D97, 0x169C9C, 0x8932B8,
        0x3C44AA, 0x835432, 0x5E7C16, 0xB02E26, 0x1D1D21,
    ];
    let c = C[usize::from(d & 15)];
    [(c >> 16) as u8, (c >> 8) as u8, c as u8]
}

/// Client tick of wet or dormant potent sulfur (every 20 game ticks): cloud of
/// noxious gas at the centre of the water outlet above, if there is one.
fn sulfur_gas(pos: [i32; 3], w: &TickWorld<'_>, t: &Tables, data: &VersionData, out: &mut Vec<Emit>) {
    let state = (w.state)(pos[0], pos[1], pos[2]);
    let wet = data
        .state_properties(state)
        .any(|(k, v)| k == "potent_sulfur_state" && matches!(v, "wet" | "dormant"));
    if !wet {
        return;
    }
    if let Some(s) = t.gas.source(w.state, pos) {
        out.push((crate::particles::Kind::NoxiousGasCloud, s.map(|v| f64::from(v) + 0.5), [0.0; 3]));
    }
}

/// Client tick of a lit campfire: 11% chance per tick of 2 or 3 smoke columns
/// (signal smoke on a hay bale), and 4 smokes above each cooking food item
/// (20% per cell and per tick), at 0.3125 from the centre towards its corner.
fn campfire_particles(e: &mut BlockEntity, pos: [i32; 3], w: &TickWorld<'_>, data: &VersionData, out: &mut Vec<Emit>) {
    use crate::particles::Kind as P;
    let state = (w.state)(pos[0], pos[1], pos[2]);
    let prop = |k: &str| data.state_properties(state).find(|p| p.0 == k).map(|p| p.1);
    if prop("lit") != Some("true") {
        return;
    }
    let seed = &mut e.anim.seed;
    let p = [f64::from(pos[0]), f64::from(pos[1]), f64::from(pos[2])];
    if next_f(seed) < 0.11 {
        let smoke = if prop("signal_fire") == Some("true") { P::CampfireSignalSmoke } else { P::CampfireCosySmoke };
        for _ in 0..next_int(seed, 2) + 2 {
            let side = |s: &mut u64| next_f(s) / 3.0 * if next_int(s, 2) == 0 { 1.0 } else { -1.0 };
            let x = p[0] + 0.5 + side(seed);
            let y = p[1] + next_f(seed) + next_f(seed);
            let z = p[2] + 0.5 + side(seed);
            out.push((smoke, [x, y, z], [0.0, 0.07, 0.0]));
        }
    }
    // 2D rotation: south 0, west 1, north 2, east 3; step (x, z) and clockwise.
    const STEP: [[f64; 2]; 4] = [[0.0, 1.0], [-1.0, 0.0], [0.0, -1.0], [1.0, 0.0]];
    let rot = match prop("facing") {
        Some("west") => 1,
        Some("north") => 2,
        Some("east") => 3,
        _ => 0,
    };
    let Data::Items(slots, _) = &e.data else { return };
    for (slot, item) in slots.iter().enumerate() {
        if item.is_none() || next_f(seed) >= 0.2 {
            continue;
        }
        let d = (slot + rot) % 4;
        let (dir, cw) = (STEP[d], STEP[(d + 1) % 4]);
        let k = f64::from(0.3125f32);
        let x = p[0] + 0.5 - dir[0] * k + cw[0] * k;
        let z = p[2] + 0.5 - dir[1] * k + cw[1] * k;
        for _ in 0..4 {
            out.push((P::Smoke, [x, p[1] + 0.5, z], [0.0, 5.0e-4, 0.0]));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Wet potent sulfur (26.3 data): gas cloud at the centre of the first cell out
    /// of the water, every 20 ticks; dry: nothing.
    #[test]
    fn potent_sulfur_emits_gas_cloud() {
        let d = std::sync::Arc::new(rc_data::load_cached("26.3").expect("26.3 data in cache"));
        assert_eq!(carrier("potent_sulfur").map(|c| c.kind), Some(Kind::PotentSulfur));
        let block = |n: &str| d.blocks.iter().find(|b| b.name == n).expect(n).clone();
        let sulfur = block("minecraft:potent_sulfur");
        let water = block("minecraft:water").default_state;
        let with = |v: &str| {
            (sulfur.first_state..sulfur.first_state + sulfur.state_count)
                .find(|&s| d.state_properties(s).any(|p| p == ("potent_sulfur_state", v)))
                .expect(v)
        };
        let mut bes = BlockEntities::new(d.clone());
        let run = |bes: &mut BlockEntities, s: u32, game_time: i64| {
            let state = move |x: i32, y: i32, z: i32| match (x, y, z) {
                (5, 0, 5) => s,
                (5, 1..=2, 5) => water,
                _ => 0,
            };
            let e = bes.create(s, carrier("potent_sulfur").unwrap());
            bes.chunks.clear();
            bes.chunks.entry((0, 0)).or_default().insert([5, 0, 5], e);
            let w = TickWorld {
                state: &state,
                surface: &|_, _| 0,
                players: &[],
                named: &[],
                game_time,
                min_y: -64,
            };
            let mut out = Vec::new();
            bes.tick(&w, &mut out);
            out
        };
        let out = run(&mut bes, with("wet"), 40);
        assert_eq!(out, [(crate::particles::Kind::NoxiousGasCloud, [5.5, 3.5, 5.5], [0.0; 3])]);
        assert!(run(&mut bes, with("dormant"), 20).len() == 1);
        assert!(run(&mut bes, with("wet"), 41).is_empty());
        assert!(run(&mut bes, with("dry"), 40).is_empty());
    }

    #[test]
    fn carriers_by_name() {
        assert_eq!(carrier("oak_wall_sign").map(|c| (c.kind, c.wall)), Some((Kind::Sign, true)));
        assert_eq!(carrier("oak_wall_hanging_sign").map(|c| (c.kind, c.wall)), Some((Kind::HangingSign, true)));
        assert_eq!(carrier("red_wall_banner").map(|c| (c.kind, c.sub, c.wall)), Some((Kind::Banner, 14, true)));
        assert_eq!(carrier("light_gray_banner").map(|c| (c.kind, c.sub)), Some((Kind::Banner, 8)));
        assert_eq!(carrier("piglin_wall_head").map(|c| (c.kind, c.sub, c.wall)), Some((Kind::Skull, 5, true)));
        assert_eq!(carrier("waxed_weathered_copper_chest").map(|c| (c.kind, c.sub)), Some((Kind::Chest, 3)));
        assert_eq!(carrier("lime_shulker_box").map(|c| c.sub), Some(5));
        assert_eq!(carrier("stone"), None);
        assert_eq!(carrier("oak_shelf").map(|c| c.kind), Some(Kind::Shelf));
    }

    /// State `name` with the properties `props` (the others arbitrary).
    fn state(d: &VersionData, name: &str, props: &[(&str, &str)]) -> u32 {
        let b = d.blocks.iter().find(|b| b.name == name).unwrap_or_else(|| panic!("block {name}"));
        (b.first_state..b.first_state + b.state_count)
            .find(|&s| props.iter().all(|p| d.state_properties(s).any(|q| q == *p)))
            .unwrap_or_else(|| panic!("state {name} {props:?}"))
    }

    /// G5-F14: extend event of a piston facing east in front of stone, replayed by the
    /// client: the stone and the head become moving blocks at progress 0, which
    /// arrive in 2 ticks, then wait 5 ticks before placing their block.
    #[test]
    fn piston_event_replays_and_ticks() {
        let data = std::sync::Arc::new(rc_testdata::version_data().expect("version data missing (extraction)"));
        let replay = rc_sim::PistonReplay::new(data.clone());
        let mut bes = BlockEntities::new(data.clone());
        let air = state(&data, "minecraft:air", &[]);
        let stone = state(&data, "minecraft:stone", &[]);
        let piston = state(&data, "minecraft:piston", &[("facing", "east"), ("extended", "false")]);
        let mut world: HashMap<[i32; 3], u32> = HashMap::new();
        world.insert([0, 64, 0], piston);
        world.insert([1, 64, 0], stone);
        struct W<'a>(&'a HashMap<[i32; 3], u32>, u32, &'a BlockEntities);
        impl rc_sim::PistonWorld for W<'_> {
            fn state(&self, p: [i32; 3]) -> u32 {
                self.0.get(&p).copied().unwrap_or(self.1)
            }
            fn min_y(&self) -> i32 {
                -64
            }
            fn max_y(&self) -> i32 {
                319
            }
            fn in_border(&self, _p: [i32; 3]) -> bool {
                true
            }
        }
        impl rc_sim::ReplayWorld for W<'_> {
            fn mover(&self, p: [i32; 3]) -> Option<rc_sim::MovingBlock> {
                let m = self.2.mover(p)?;
                Some(rc_sim::MovingBlock { moved: m.moved, dir: m.dir, extending: m.extending, source: m.source })
            }
        }
        let ops = replay.replay(&W(&world, air, &bes), [0, 64, 0], 0, 5);
        for op in ops {
            match op {
                rc_sim::ReplayOp::Set(p, s) => {
                    let old = world.get(&p).copied().unwrap_or(air);
                    bes.on_block_change(p, old, s);
                    world.insert(p, s);
                }
                rc_sim::ReplayOp::Mover(p, m) => bes.set_mover(p, m),
                rc_sim::ReplayOp::Destroyed(..) => panic!("nothing crushed"),
            }
        }
        let stone_m = bes.mover([2, 64, 0]).expect("moving stone");
        assert_eq!((stone_m.moved, stone_m.dir, stone_m.extending, stone_m.source, stone_m.progress), (stone, 5, true, false, 0.0));
        let head_m = bes.mover([1, 64, 0]).expect("moving head");
        let head = state(&data, "minecraft:piston_head", &[("facing", "east"), ("type", "normal"), ("short", "false")]);
        assert_eq!((head_m.moved, head_m.extending, head_m.source), (head, true, true));
        assert_eq!(world[&[0, 64, 0]], state(&data, "minecraft:piston", &[("facing", "east"), ("extended", "true")]));
        let w = TickWorld {
            state: &|_, _, _| 0,
            surface: &|_, _| 0,
            players: &[],
            named: &[],
            game_time: 0,
            min_y: -64,
        };
        let mut pistons = Vec::new();
        let mut out = Vec::new();
        bes.tick(&w, &mut out);
        bes.tick_pistons(&mut pistons);
        assert_eq!(bes.mover([2, 64, 0]).map(|m| m.progress), Some(0.5));
        assert!(pistons.contains(&PistonTick::Push([2, 64, 0], stone_m, 0.5)));
        bes.tick(&w, &mut out);
        bes.tick_pistons(&mut pistons);
        assert_eq!(bes.mover([2, 64, 0]).map(|m| m.progress), Some(1.0));
        for _ in 0..5 {
            pistons.clear();
            bes.tick(&w, &mut out);
            bes.tick_pistons(&mut pistons);
            assert!(pistons.is_empty(), "{pistons:?}");
        }
        bes.tick(&w, &mut out);
        bes.tick_pistons(&mut pistons);
        assert!(pistons.contains(&PistonTick::Finish([2, 64, 0], stone)), "{pistons:?}");
        assert!(pistons.contains(&PistonTick::Finish([1, 64, 0], head)), "{pistons:?}");
    }

    #[test]
    fn uuid_from_ints() {
        let n = Nbt::IntArray(vec![1, 2, 3, -1]);
        assert_eq!(uuid_of(&n), Some((1u128 << 96) | (2u128 << 64) | (3u128 << 32) | 0xffff_ffff));
    }
}
