//! Command tree (`docs/specs/commands.md` § 1): `commands` packet and the 62 argument types
//! with their properties.

use rc_proto::varint::read_varint;
use rc_proto::{Decode, Error, Reader, Result, read_count, read_str};

/// Kind of a node (`flags & 0x03`, 3 treated as root).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Root,
    Literal,
    Argument,
}

/// Bounds of a numeric argument (absent = type defaults).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds<T> {
    pub min: Option<T>,
    pub max: Option<T>,
}

/// Argument type and its properties (§ 1.3, fixed registry indices).
#[derive(Debug, Clone, PartialEq)]
pub enum ArgType {
    Bool,
    Float(Bounds<f32>),
    Double(Bounds<f64>),
    Integer(Bounds<i32>),
    Long(Bounds<i64>),
    /// 0 single word, 1 phrase, 2 greedy.
    String(u8),
    /// (a single entity, players only).
    Entity {
        single: bool,
        players: bool,
    },
    GameProfile,
    BlockPos,
    ColumnPos,
    Vec3,
    Vec2,
    BlockState,
    BlockPredicate,
    ItemStack,
    ItemPredicate,
    TeamColor,
    HexColor,
    Component,
    Style,
    Message,
    NbtCompoundTag,
    NbtTag,
    NbtPath,
    Objective,
    ObjectiveCriteria,
    Operation,
    Particle,
    Angle,
    Rotation,
    ScoreboardSlot,
    ScoreHolder {
        multiple: bool,
    },
    Swizzle,
    Team,
    ItemSlot,
    ItemSlots,
    ResourceLocation,
    Function,
    EntityAnchor,
    IntRange,
    FloatRange,
    Dimension,
    Gamemode,
    /// Minimum in ticks.
    Time(i32),
    ResourceOrTag(String),
    ResourceOrTagKey(String),
    Resource(String),
    ResourceKey(String),
    ResourceSelector(String),
    TemplateMirror,
    TemplateRotation,
    Heightmap,
    LootTable,
    LootPredicate,
    LootModifier,
    ContextFloatProvider,
    ContextIntProvider,
    SlotSource,
    Dialog,
    Feature,
    SwingAnimation,
    Uuid,
}

/// Identifiers of the `command_argument_type` registry, in index order.
pub const ARG_TYPE_NAMES: [&str; 62] = [
    "brigadier:bool",
    "brigadier:float",
    "brigadier:double",
    "brigadier:integer",
    "brigadier:long",
    "brigadier:string",
    "entity",
    "game_profile",
    "block_pos",
    "column_pos",
    "vec3",
    "vec2",
    "block_state",
    "block_predicate",
    "item_stack",
    "item_predicate",
    "team_color",
    "hex_color",
    "component",
    "style",
    "message",
    "nbt_compound_tag",
    "nbt_tag",
    "nbt_path",
    "objective",
    "objective_criteria",
    "operation",
    "particle",
    "angle",
    "rotation",
    "scoreboard_slot",
    "score_holder",
    "swizzle",
    "team",
    "item_slot",
    "item_slots",
    "resource_location",
    "function",
    "entity_anchor",
    "int_range",
    "float_range",
    "dimension",
    "gamemode",
    "time",
    "resource_or_tag",
    "resource_or_tag_key",
    "resource",
    "resource_key",
    "resource_selector",
    "template_mirror",
    "template_rotation",
    "heightmap",
    "loot_table",
    "loot_predicate",
    "loot_modifier",
    "context_float_provider",
    "context_int_provider",
    "slot_source",
    "dialog",
    "feature",
    "swing_animation",
    "uuid",
];

fn bounds<'a, T>(r: &mut Reader<'a>, read: impl Fn(&mut Reader<'a>) -> Result<T>) -> Result<Bounds<T>> {
    let flags = r.u8()?;
    let min = if flags & 1 != 0 { Some(read(r)?) } else { None };
    let max = if flags & 2 != 0 { Some(read(r)?) } else { None };
    Ok(Bounds { min, max })
}

fn ident(r: &mut Reader<'_>) -> Result<String> {
    Ok(read_str(r, rc_proto::STRING_MAX_DEFAULT)?.to_owned())
}

impl ArgType {
    /// Index in the registry then properties.
    pub fn read(id: i32, r: &mut Reader<'_>) -> Result<Self> {
        use ArgType as A;
        Ok(match id {
            0 => A::Bool,
            1 => A::Float(bounds(r, f32::decode)?),
            2 => A::Double(bounds(r, f64::decode)?),
            3 => A::Integer(bounds(r, i32::decode)?),
            4 => A::Long(bounds(r, i64::decode)?),
            5 => A::String(rc_proto::check_enum("string type", read_varint(r)?, 3)? as u8),
            6 => {
                let f = r.u8()?;
                A::Entity { single: f & 1 != 0, players: f & 2 != 0 }
            }
            7 => A::GameProfile,
            8 => A::BlockPos,
            9 => A::ColumnPos,
            10 => A::Vec3,
            11 => A::Vec2,
            12 => A::BlockState,
            13 => A::BlockPredicate,
            14 => A::ItemStack,
            15 => A::ItemPredicate,
            16 => A::TeamColor,
            17 => A::HexColor,
            18 => A::Component,
            19 => A::Style,
            20 => A::Message,
            21 => A::NbtCompoundTag,
            22 => A::NbtTag,
            23 => A::NbtPath,
            24 => A::Objective,
            25 => A::ObjectiveCriteria,
            26 => A::Operation,
            27 => A::Particle,
            28 => A::Angle,
            29 => A::Rotation,
            30 => A::ScoreboardSlot,
            31 => A::ScoreHolder { multiple: r.u8()? & 1 != 0 },
            32 => A::Swizzle,
            33 => A::Team,
            34 => A::ItemSlot,
            35 => A::ItemSlots,
            36 => A::ResourceLocation,
            37 => A::Function,
            38 => A::EntityAnchor,
            39 => A::IntRange,
            40 => A::FloatRange,
            41 => A::Dimension,
            42 => A::Gamemode,
            43 => A::Time(i32::decode(r)?),
            44 => A::ResourceOrTag(ident(r)?),
            45 => A::ResourceOrTagKey(ident(r)?),
            46 => A::Resource(ident(r)?),
            47 => A::ResourceKey(ident(r)?),
            48 => A::ResourceSelector(ident(r)?),
            49 => A::TemplateMirror,
            50 => A::TemplateRotation,
            51 => A::Heightmap,
            52 => A::LootTable,
            53 => A::LootPredicate,
            54 => A::LootModifier,
            55 => A::ContextFloatProvider,
            56 => A::ContextIntProvider,
            57 => A::SlotSource,
            58 => A::Dialog,
            59 => A::Feature,
            60 => A::SwingAnimation,
            61 => A::Uuid,
            _ => return Err(Error::InvalidValue { what: "command argument type", value: id.into() }),
        })
    }

    /// Index in the registry.
    pub fn id(&self) -> usize {
        use ArgType as A;
        match self {
            A::Bool => 0,
            A::Float(_) => 1,
            A::Double(_) => 2,
            A::Integer(_) => 3,
            A::Long(_) => 4,
            A::String(_) => 5,
            A::Entity { .. } => 6,
            A::GameProfile => 7,
            A::BlockPos => 8,
            A::ColumnPos => 9,
            A::Vec3 => 10,
            A::Vec2 => 11,
            A::BlockState => 12,
            A::BlockPredicate => 13,
            A::ItemStack => 14,
            A::ItemPredicate => 15,
            A::TeamColor => 16,
            A::HexColor => 17,
            A::Component => 18,
            A::Style => 19,
            A::Message => 20,
            A::NbtCompoundTag => 21,
            A::NbtTag => 22,
            A::NbtPath => 23,
            A::Objective => 24,
            A::ObjectiveCriteria => 25,
            A::Operation => 26,
            A::Particle => 27,
            A::Angle => 28,
            A::Rotation => 29,
            A::ScoreboardSlot => 30,
            A::ScoreHolder { .. } => 31,
            A::Swizzle => 32,
            A::Team => 33,
            A::ItemSlot => 34,
            A::ItemSlots => 35,
            A::ResourceLocation => 36,
            A::Function => 37,
            A::EntityAnchor => 38,
            A::IntRange => 39,
            A::FloatRange => 40,
            A::Dimension => 41,
            A::Gamemode => 42,
            A::Time(_) => 43,
            A::ResourceOrTag(_) => 44,
            A::ResourceOrTagKey(_) => 45,
            A::Resource(_) => 46,
            A::ResourceKey(_) => 47,
            A::ResourceSelector(_) => 48,
            A::TemplateMirror => 49,
            A::TemplateRotation => 50,
            A::Heightmap => 51,
            A::LootTable => 52,
            A::LootPredicate => 53,
            A::LootModifier => 54,
            A::ContextFloatProvider => 55,
            A::ContextIntProvider => 56,
            A::SlotSource => 57,
            A::Dialog => 58,
            A::Feature => 59,
            A::SwingAnimation => 60,
            A::Uuid => 61,
        }
    }

    pub fn name(&self) -> &'static str {
        ARG_TYPE_NAMES[self.id()]
    }
}

/// Node as received (indices into the packet's list).
#[derive(Debug, Clone, PartialEq)]
pub struct Node {
    pub kind: Kind,
    pub executable: bool,
    /// Flag 0x20: requires a permission (§ 4.5).
    pub restricted: bool,
    pub children: Vec<u32>,
    pub redirect: Option<u32>,
    /// Empty for the root.
    pub name: String,
    pub arg: Option<ArgType>,
    /// Custom suggestions identifier (flag 0x10).
    pub suggestions: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Commands {
    pub nodes: Vec<Node>,
    pub root: u32,
}

fn read_node(r: &mut Reader<'_>) -> Result<Node> {
    let flags = r.u8()?;
    let n = read_count(r, 1)?;
    let mut children = Vec::with_capacity(n);
    for _ in 0..n {
        children.push(read_varint(r)? as u32);
    }
    let redirect = if flags & 0x08 != 0 { Some(read_varint(r)? as u32) } else { None };
    let kind = match flags & 3 {
        1 => Kind::Literal,
        2 => Kind::Argument,
        _ => Kind::Root,
    };
    let mut node = Node {
        kind,
        executable: flags & 0x04 != 0,
        restricted: flags & 0x20 != 0,
        children,
        redirect,
        name: String::new(),
        arg: None,
        suggestions: None,
    };
    match kind {
        Kind::Root => {}
        Kind::Literal => node.name = ident(r)?,
        Kind::Argument => {
            node.name = ident(r)?;
            let id = read_varint(r)?;
            node.arg = Some(ArgType::read(id, r)?);
            if flags & 0x10 != 0 {
                node.suggestions = Some(ident(r)?);
            }
        }
    }
    Ok(node)
}

/// Reads the packet and validates the tree (§ 1.2): indices within bounds, redirects resolvable.
pub fn read_commands(r: &mut Reader<'_>) -> Result<Commands> {
    let n = read_count(r, 3)?;
    let mut nodes = Vec::with_capacity(n);
    for _ in 0..n {
        nodes.push(read_node(r)?);
    }
    let root = read_varint(r)? as u32;
    let len = nodes.len() as u32;
    if root >= len || nodes.iter().any(|n| n.children.iter().any(|&c| c >= len) || n.redirect.is_some_and(|t| t >= len))
    {
        return Err(Error::InvalidValue { what: "command node index", value: i64::from(len) });
    }
    Ok(Commands { nodes, root })
}

/// `command_suggestions` response (§ 3.2): range in the sent text, `/` included.
#[derive(Debug, Clone, PartialEq)]
pub struct ServerSuggestions {
    pub id: i32,
    pub start: i32,
    pub length: i32,
    /// Text and tooltip (NBT Component).
    pub list: Vec<(String, Option<rc_proto::nbt::Nbt>)>,
}

pub fn read_command_suggestions(r: &mut Reader<'_>) -> Result<ServerSuggestions> {
    let id = read_varint(r)?;
    let start = read_varint(r)?;
    let length = read_varint(r)?;
    let n = read_count(r, 2)?;
    let mut list = Vec::with_capacity(n);
    for _ in 0..n {
        let text = ident(r)?;
        let tip = if bool::decode(r)? { rc_proto::nbt::read(r)? } else { None };
        list.push((text, tip));
    }
    Ok(ServerSuggestions { id, start, length, list })
}

/// `custom_chat_completions` (§ 3.5): action (0 add, 1 remove, 2 set) and entries.
pub fn read_chat_completions(r: &mut Reader<'_>) -> Result<(i32, Vec<String>)> {
    let action = rc_proto::check_enum("completions action", read_varint(r)?, 3)?;
    let n = read_count(r, 1)?;
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        v.push(ident(r)?);
    }
    Ok((action, v))
}

/// `command_suggestion` (sent): transaction identifier and input truncated at the cursor.
pub fn write_command_suggestion(w: &mut Vec<u8>, id: i32, text: &str) {
    rc_proto::varint::write_varint(w, id);
    rc_proto::write_str(w, text);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decode(body: &[u8]) -> Commands {
        let mut r = Reader::new(body);
        let t = read_commands(&mut r).expect("tree decoding");
        assert!(r.is_empty(), "extra bytes: {}", r.remaining());
        t
    }

    /// Tree of an operator on the test server, recorded by `RUSTCRAFT_CMDDUMP` (outside the repository).
    fn tree() -> Option<Commands> {
        let dir = std::path::PathBuf::from(rc_data::env_os!("LOCALAPPDATA")?).join("rustcraft-re").join("captures");
        Some(decode(&std::fs::read(dir.join("commands-op.bin")).ok()?))
    }

    #[test]
    fn official_capture_tree() {
        // Official client session (non-operator player): reduced tree, decoded in full.
        let Some(data) = rc_testdata::version_data() else { return };
        let Some(cap) = rc_testdata::reference_capture() else { return };
        for body in rc_testdata::play_packets(&cap, &data, "minecraft:commands") {
            let t = decode(&body);
            assert_eq!(t.nodes[t.root as usize].kind, Kind::Root);
            assert!(!t.nodes[t.root as usize].children.is_empty());
        }
    }

    fn child<'a>(t: &'a Commands, of: u32, name: &str) -> &'a Node {
        let n = &t.nodes[of as usize];
        let i = n.children.iter().find(|&&c| t.nodes[c as usize].name == name).expect(name);
        &t.nodes[*i as usize]
    }

    #[test]
    fn full_registry() {
        // Each re-read index gives the same index (62 types, no gaps).
        for (i, name) in ARG_TYPE_NAMES.iter().enumerate() {
            let props: &[u8] = match i {
                1 | 2 | 3 | 4 | 6 | 31 => &[0],
                5 => &[2],
                43 => &[0, 0, 0, 0],
                44..=48 => &[1, b'x'],
                _ => &[],
            };
            let t = ArgType::read(i as i32, &mut Reader::new(props)).expect(name);
            assert_eq!(t.id(), i);
            assert_eq!(t.name(), *name);
        }
        assert!(ArgType::read(62, &mut Reader::new(&[])).is_err());
    }

    #[test]
    fn numeric_bounds() {
        let mut b = vec![3u8];
        b.extend_from_slice(&1i32.to_be_bytes());
        b.extend_from_slice(&64i32.to_be_bytes());
        let t = ArgType::read(3, &mut Reader::new(&b)).unwrap();
        assert_eq!(t, ArgType::Integer(Bounds { min: Some(1), max: Some(64) }));
        let mut b = vec![2u8];
        b.extend_from_slice(&1.5f64.to_be_bytes());
        assert_eq!(
            ArgType::read(2, &mut Reader::new(&b)).unwrap(),
            ArgType::Double(Bounds { min: None, max: Some(1.5) })
        );
    }

    #[test]
    fn real_tree() {
        let Some(t) = tree() else { return };
        let root = &t.nodes[t.root as usize];
        assert_eq!(root.kind, Kind::Root);
        // Basic commands of an operator.
        let give = child(&t, t.root, "give");
        assert_eq!(give.kind, Kind::Literal);
        let tgt = &t.nodes[give.children[0] as usize];
        assert_eq!(tgt.arg, Some(ArgType::Entity { single: false, players: true }));
        let item = &t.nodes[tgt.children[0] as usize];
        assert_eq!(item.arg, Some(ArgType::ItemStack));
        assert!(item.executable);
        // `execute run` redirects to the root.
        let execute = t.nodes[t.root as usize]
            .children
            .iter()
            .map(|&c| c as usize)
            .find(|&c| t.nodes[c].name == "execute")
            .unwrap();
        let run = child(&t, execute as u32, "run");
        assert_eq!(run.redirect, Some(t.root));
        // `message` arguments (say) and various types present.
        let say = child(&t, t.root, "say");
        assert_eq!(t.nodes[say.children[0] as usize].arg, Some(ArgType::Message));
        let used: std::collections::HashSet<usize> =
            t.nodes.iter().filter_map(|n| n.arg.as_ref()).map(ArgType::id).collect();
        assert!(used.len() > 40, "{} types used", used.len());
    }
}
