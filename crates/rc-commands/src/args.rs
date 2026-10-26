//! Readers and suggestions for each argument type (§ 2.4, § 3.3).

use rc_proto_v777::commands::{ArgType, Bounds};

use crate::parse::Source;
use crate::reader::{Reader, is_id_char, java_split_space, read_id};
use crate::selector::{SelParser, is_uuid};
use crate::suggest::{Builder, Tooltip};
use crate::{CmdError, Data};

pub const BLOCK: &str = "minecraft:block";
pub const ITEM: &str = "minecraft:item";
pub const COMPONENT: &str = "minecraft:data_component_type";
pub const ITEM_PREDICATE: &str = "minecraft:data_component_predicate_type";
pub const PARTICLE: &str = "minecraft:particle_type";
pub const STAT_TYPE: &str = "minecraft:stat_type";

pub const TEAM_COLORS: [&str; 16] = [
    "black",
    "dark_blue",
    "dark_green",
    "dark_aqua",
    "dark_red",
    "dark_purple",
    "gold",
    "gray",
    "dark_gray",
    "blue",
    "green",
    "aqua",
    "red",
    "light_purple",
    "yellow",
    "white",
];
const GAMEMODES: [&str; 4] = ["survival", "creative", "adventure", "spectator"];
const ANCHORS: [&str; 2] = ["feet", "eyes"];
const MIRRORS: [&str; 3] = ["none", "left_right", "front_back"];
const ROTATIONS: [&str; 4] = ["none", "clockwise_90", "180", "counterclockwise_90"];
const HEIGHTMAPS: [&str; 4] = ["world_surface", "motion_blocking", "motion_blocking_no_leaves", "ocean_floor"];
const SWINGS: [&str; 3] = ["none", "whack", "stab"];
const OPERATIONS: [&str; 9] = ["=", "+=", "-=", "*=", "/=", "%=", "<", ">", "><"];
const CUSTOM_CRITERIA: [&str; 11] = [
    "dummy",
    "trigger",
    "deathCount",
    "playerKillCount",
    "totalKillCount",
    "health",
    "food",
    "air",
    "armor",
    "xp",
    "level",
];

/// Named slots (§ 2.4): (name, number of slots).
fn slot_names() -> Vec<(String, usize)> {
    let mut v: Vec<(String, usize)> = vec![("contents".into(), 1)];
    let mut range = |p: &str, n: usize| {
        for i in 0..n {
            v.push((format!("{p}{i}"), 1));
        }
        v.push((format!("{p}*"), n));
    };
    range("container.", 54);
    range("hotbar.", 9);
    range("inventory.", 27);
    range("enderchest.", 27);
    range("mob.inventory.", 8);
    range("horse.", 15);
    for (n, c) in [
        ("weapon", 1),
        ("weapon.mainhand", 1),
        ("weapon.offhand", 1),
        ("weapon.*", 2),
        ("armor.head", 1),
        ("armor.chest", 1),
        ("armor.legs", 1),
        ("armor.feet", 1),
        ("armor.body", 1),
        ("armor.*", 5),
        ("saddle", 1),
        ("horse.chest", 1),
        ("player.cursor", 1),
    ] {
        v.push((n.into(), c));
    }
    for i in 0..4 {
        v.push((format!("player.crafting.{i}"), 1));
    }
    v.push(("player.crafting.*".into(), 4));
    v
}

fn check_bounds<T: PartialOrd + std::fmt::Display + Copy>(
    r: &mut Reader<'_>,
    start: usize,
    v: T,
    b: &Bounds<T>,
    kind: &str,
) -> Result<(), CmdError> {
    let (low, big): (&'static str, &'static str) = match kind {
        "integer" => ("argument.integer.low", "argument.integer.big"),
        "long" => ("argument.long.low", "argument.long.big"),
        "float" => ("argument.float.low", "argument.float.big"),
        _ => ("argument.double.low", "argument.double.big"),
    };
    if let Some(min) = b.min
        && v < min
    {
        r.cursor = start;
        return Err(r.err(low, vec![min.to_string(), v.to_string()]));
    }
    if let Some(max) = b.max
        && v > max
    {
        r.cursor = start;
        return Err(r.err(big, vec![max.to_string(), v.to_string()]));
    }
    Ok(())
}

/// Reading an argument from the cursor.
pub fn parse(ty: &ArgType, r: &mut Reader<'_>, data: &Data, src: Source) -> Result<(), CmdError> {
    use ArgType as A;
    let start = r.cursor;
    match ty {
        A::Bool => {
            r.read_bool()?;
        }
        A::Float(b) => {
            let v = r.read_float()?;
            check_bounds(r, start, v, b, "float")?;
        }
        A::Double(b) => {
            let v = r.read_double()?;
            check_bounds(r, start, v, b, "double")?;
        }
        A::Integer(b) => {
            let v = r.read_int()?;
            check_bounds(r, start, v, b, "integer")?;
        }
        A::Long(b) => {
            let v = r.read_long()?;
            check_bounds(r, start, v, b, "long")?;
        }
        A::String(0) => {
            r.read_unquoted();
        }
        A::String(1) => {
            r.read_string()?;
        }
        A::String(_) => r.cursor = r.s.len(),
        A::Entity { single, players } => {
            let s = SelParser::new(r, src.selectors, data).parse()?;
            if s.max > 1 && *single {
                r.cursor = 0;
                return Err(r.err0(if *players { "argument.player.toomany" } else { "argument.entity.toomany" }));
            }
            if s.includes_entities && *players && !s.current {
                r.cursor = 0;
                return Err(r.err0("argument.player.entities"));
            }
        }
        A::GameProfile => {
            if r.next_is('@') {
                let s = SelParser::new(r, src.selectors, data).parse()?;
                if s.includes_entities {
                    return Err(r.err0("argument.player.entities"));
                }
            } else {
                r.read_until_space();
            }
        }
        A::ScoreHolder { multiple } => {
            if r.next_is('@') {
                let s = SelParser::new(r, src.selectors, data).parse()?;
                if !multiple && s.max > 1 {
                    return Err(r.err0("argument.entity.toomany"));
                }
            } else {
                r.read_until_space();
            }
        }
        A::BlockPos => coords(r, 3, true, false)?,
        A::Vec3 => coords(r, 3, false, false)?,
        A::ColumnPos => coords(r, 2, true, false)?,
        A::Vec2 => coords(r, 2, false, false)?,
        A::Rotation => coords(r, 2, false, true)?,
        A::Angle => {
            if !r.can_read() {
                return Err(r.err0("argument.angle.incomplete"));
            }
            if r.next_is('~') {
                r.skip();
            }
            if r.can_read() && r.peek() != ' ' {
                let v = r.read_float()?;
                if !v.is_finite() {
                    return Err(r.err0("argument.angle.invalid"));
                }
            }
        }
        A::Swizzle => {
            let mut seen = [false; 3];
            while r.can_read() && r.peek() != ' ' {
                let c = r.read();
                let i = match c {
                    'x' => 0,
                    'y' => 1,
                    'z' => 2,
                    _ => return Err(r.err0("arguments.swizzle.invalid")),
                };
                if seen[i] {
                    return Err(r.err0("arguments.swizzle.invalid"));
                }
                seen[i] = true;
            }
        }
        A::BlockState => BlockParser::new(r, data, false).parse()?,
        A::BlockPredicate => BlockParser::new(r, data, true).parse()?,
        A::ItemStack => ItemParser::new(r, data).parse()?,
        A::ItemPredicate => ItemPredParser::new(r, data).parse()?,
        A::TeamColor => enum_word(r, &TEAM_COLORS, "argument.color.invalid", false)?,
        A::HexColor => {
            let s = r.read_unquoted();
            let ok = (s.len() == 3 || s.len() == 6) && s.chars().all(|c| c.is_ascii_hexdigit());
            if !ok {
                return Err(r.err("argument.hexcolor.invalid", vec![s]));
            }
        }
        A::Component => {
            crate::snbt::parse_argument(r)?;
        }
        A::Style => {
            let at = r.cursor;
            match crate::snbt::parse_argument(r) {
                Ok(crate::snbt::Value::Compound(_)) => {}
                Ok(_) => {
                    r.cursor = at;
                    return Err(r.err("argument.style.invalid", vec![String::new()]));
                }
                Err(e) => return Err(e),
            }
        }
        A::Message => message(r, src.selectors, data)?,
        A::NbtCompoundTag => {
            crate::snbt::compound_argument(r)?;
        }
        A::NbtTag => {
            crate::snbt::parse_argument(r)?;
        }
        A::NbtPath => nbt_path(r)?,
        A::Objective | A::Team => {
            r.read_unquoted();
        }
        A::ObjectiveCriteria => {
            let s = r.read_until_space();
            if !criteria_ok(&s, data) {
                r.cursor = start;
                return Err(r.err("argument.criteria.invalid", vec![s]));
            }
        }
        A::Operation => {
            if !r.can_read() {
                return Err(r.err0("arguments.operation.invalid"));
            }
            let s = r.read_until_space();
            if !OPERATIONS.contains(&s.as_str()) {
                return Err(r.err0("arguments.operation.invalid"));
            }
        }
        A::Particle => {
            let start = r.cursor;
            let id = read_id(r)?;
            if data.has_registry(PARTICLE) && !data.contains(PARTICLE, &id) {
                return Err(r.err("particle.notFound", vec![id]));
            }
            if r.next_is('{') {
                crate::snbt::compound_argument(r)?;
            }
            if let Some(check) = data.particle_check
                && let Err(m) = check(data, &r.slice(start, r.cursor))
            {
                return Err(r.err("particle.invalidOptions", vec![m]));
            }
        }
        A::ScoreboardSlot => {
            let s = r.read_unquoted();
            let ok = matches!(s.as_str(), "list" | "sidebar" | "below_name")
                || s.strip_prefix("sidebar.team.").is_some_and(|c| TEAM_COLORS.contains(&c));
            if !ok {
                return Err(r.err("argument.scoreboardDisplaySlot.invalid", vec![s]));
            }
        }
        A::ItemSlot | A::ItemSlots => {
            let s = r.read_until_space();
            match slot_names().into_iter().find(|(n, _)| *n == s) {
                None => return Err(r.err("slot.unknown", vec![s])),
                Some((_, n)) if n != 1 && matches!(ty, A::ItemSlot) => {
                    return Err(r.err("slot.only_single_allowed", vec![s]));
                }
                _ => {}
            }
        }
        A::ResourceLocation | A::Dimension => {
            read_id(r)?;
        }
        A::Function => {
            if r.next_is('#') {
                r.skip();
            }
            read_id(r)?;
        }
        A::EntityAnchor => enum_word(r, &ANCHORS, "argument.anchor.invalid", true)?,
        A::IntRange => {
            crate::selector::read_range(r, true)?;
        }
        A::FloatRange => {
            crate::selector::read_range(r, false)?;
        }
        A::Gamemode => enum_word(r, &GAMEMODES, "argument.gamemode.invalid", false)?,
        A::Time(min) => {
            let v = r.read_float()?;
            let unit = r.read_unquoted();
            let f = match unit.as_str() {
                "d" => 24000.0,
                "s" => 20.0,
                "t" | "" => 1.0,
                _ => return Err(r.err0("argument.time.invalid_unit")),
            };
            let ticks = (v * f).round() as i32;
            if ticks < *min {
                return Err(r.err("argument.time.tick_count_too_low", vec![min.to_string(), ticks.to_string()]));
            }
        }
        A::Resource(reg) => {
            let id = read_id(r)?;
            if data.has_registry(reg) && !data.contains(reg, &id) {
                return Err(r.err("argument.resource.not_found", vec![id, reg.clone()]));
            }
        }
        A::ResourceKey(_) => {
            read_id(r)?;
        }
        A::ResourceOrTag(reg) => {
            if r.next_is('#') {
                r.skip();
                let id = read_id(r).inspect_err(|_| r.cursor = start)?;
                if data.has_registry(reg) && !data.has_tag(reg, &id) {
                    return Err(r.err("argument.resource_tag.not_found", vec![id, reg.clone()]));
                }
            } else {
                let id = read_id(r)?;
                if data.has_registry(reg) && !data.contains(reg, &id) {
                    return Err(r.err("argument.resource.not_found", vec![id, reg.clone()]));
                }
            }
        }
        A::ResourceOrTagKey(_) => {
            if r.next_is('#') {
                r.skip();
            }
            read_id(r).inspect_err(|_| r.cursor = start)?;
        }
        A::ResourceSelector(reg) => {
            let s0 = r.cursor;
            while r.can_read() && (is_id_char(r.peek()) || matches!(r.peek(), '*' | '?')) {
                r.skip();
            }
            let mut pat = r.slice(s0, r.cursor);
            if !pat.contains(':') {
                pat = format!("minecraft:{pat}");
            }
            if let Some(entries) = data.registry(reg)
                && !entries.iter().any(|e| glob(&pat, e))
            {
                return Err(r.err("argument.resource_selector.not_found", vec![pat, reg.clone()]));
            }
        }
        A::TemplateMirror => enum_word(r, &MIRRORS, "argument.enum.invalid", false)?,
        A::TemplateRotation => enum_word(r, &ROTATIONS, "argument.enum.invalid", false)?,
        A::Heightmap => enum_word(r, &HEIGHTMAPS, "argument.enum.invalid", false)?,
        A::LootTable
        | A::LootPredicate
        | A::LootModifier
        | A::ContextFloatProvider
        | A::ContextIntProvider
        | A::Dialog
        | A::Feature => resource_or_inline(r, resource_or_id_registry(ty), data)?,
        A::SlotSource => {
            let s = r.read_until_space();
            if !slot_names().iter().any(|(n, _)| *n == s) {
                r.cursor = start;
                resource_or_inline(r, "minecraft:slot_source", data)?;
            }
        }
        A::SwingAnimation => enum_word(r, &SWINGS, "argument.swing_animation.invalid", false)?,
        A::Uuid => {
            let n = r.remaining().iter().take_while(|c| c.is_ascii_hexdigit() || **c == '-').count();
            let s: String = r.remaining()[..n].iter().collect();
            if n == 0 || !is_uuid(&s) {
                return Err(r.err0("argument.uuid.invalid"));
            }
            r.cursor += n;
        }
    }
    Ok(())
}

fn enum_word(r: &mut Reader<'_>, values: &[&str], key: &'static str, rewind: bool) -> Result<(), CmdError> {
    let start = r.cursor;
    let s = r.read_unquoted();
    if !values.contains(&s.as_str()) {
        if rewind {
            r.cursor = start;
        }
        return Err(r.err(key, vec![s]));
    }
    Ok(())
}

/// Coordinates (§ 2.4): `n` components, integer or real; rotation without `^`.
fn coords(r: &mut Reader<'_>, n: usize, int: bool, rotation: bool) -> Result<(), CmdError> {
    let start = r.cursor;
    let incomplete = match (n, rotation) {
        (_, true) => "argument.rotation.incomplete",
        (2, _) => "argument.pos2d.incomplete",
        _ => "argument.pos3d.incomplete",
    };
    if n == 3 && r.next_is('^') {
        for i in 0..3 {
            if !r.can_read() {
                return Err(r.err0("argument.pos.missing.double"));
            }
            if r.peek() != '^' {
                r.cursor = start;
                return Err(r.err0("argument.pos.mixed"));
            }
            r.skip();
            if r.can_read() && r.peek() != ' ' {
                r.read_double()?;
            }
            if i < 2 {
                if !r.next_is(' ') {
                    r.cursor = start;
                    return Err(r.err0(incomplete));
                }
                r.skip();
            }
        }
        return Ok(());
    }
    if n == 2 && !r.can_read() {
        return Err(r.err0(incomplete));
    }
    for i in 0..n {
        if r.next_is('^') {
            return Err(r.err0("argument.pos.mixed"));
        }
        if !r.can_read() {
            return Err(r.err0(if int { "argument.pos.missing.int" } else { "argument.pos.missing.double" }));
        }
        let relative = r.next_is('~');
        if relative {
            r.skip();
        }
        if r.can_read() && r.peek() != ' ' {
            if relative || !int {
                r.read_double()?;
            } else {
                r.read_int()?;
            }
        }
        if i + 1 < n {
            if !r.next_is(' ') {
                r.cursor = start;
                return Err(r.err0(incomplete));
            }
            r.skip();
        }
    }
    Ok(())
}

/// Message (§ 2.4): rest of the input, selectors read if allowed.
fn message(r: &mut Reader<'_>, selectors: bool, data: &Data) -> Result<(), CmdError> {
    let rest = r.s.len() - r.cursor;
    if rest > 256 {
        return Err(CmdError {
            key: "argument.message.too_long",
            args: vec![rest.to_string(), "256".into()],
            cursor: None,
        });
    }
    if !selectors {
        r.cursor = r.s.len();
        return Ok(());
    }
    while r.can_read() {
        if r.peek() == '@' {
            let start = r.cursor;
            match SelParser::new(r, true, data).parse() {
                Ok(_) => {}
                Err(e) if matches!(e.key, "argument.entity.selector.missing" | "argument.entity.selector.unknown") => {
                    r.cursor = start + 1;
                }
                Err(e) => return Err(e),
            }
        } else {
            r.skip();
        }
    }
    Ok(())
}

fn nbt_path(r: &mut Reader<'_>) -> Result<(), CmdError> {
    let mut first = true;
    let mut depth = 0;
    let unquoted = |c: char| !matches!(c, ' ' | '"' | '\'' | '[' | ']' | '.' | '{' | '}');
    while r.can_read() && r.peek() != ' ' {
        depth += 1;
        if depth > 512 {
            return Err(r.err0("arguments.nbtpath.too_deep"));
        }
        match r.peek() {
            '"' | '\'' => {
                let name = r.read_string()?;
                object_node(r, &name)?;
            }
            '[' => {
                r.skip();
                match r.peek_at(0) {
                    Some('{') => {
                        crate::snbt::compound_argument(r)?;
                        r.expect(']')?;
                    }
                    Some(']') => r.skip(),
                    _ => {
                        r.read_int()?;
                        r.expect(']')?;
                    }
                }
            }
            '{' => {
                if !first {
                    return Err(r.err0("arguments.nbtpath.node.invalid"));
                }
                crate::snbt::compound_argument(r)?;
            }
            _ => {
                let s0 = r.cursor;
                while r.can_read() && unquoted(r.peek()) {
                    r.skip();
                }
                if r.cursor == s0 {
                    return Err(r.err0("arguments.nbtpath.node.invalid"));
                }
                let name = r.slice(s0, r.cursor);
                object_node(r, &name)?;
            }
        }
        first = false;
        if r.can_read() {
            let c = r.peek();
            if c != ' ' && c != '[' && c != '{' {
                r.expect('.')?;
            }
        }
    }
    Ok(())
}

fn object_node(r: &mut Reader<'_>, name: &str) -> Result<(), CmdError> {
    if name.is_empty() {
        return Err(r.err0("arguments.nbtpath.node.invalid"));
    }
    if r.next_is('{') {
        crate::snbt::compound_argument(r)?;
    }
    Ok(())
}

/// Known objective criterion: custom, by team colour or statistic.
fn criteria_ok(s: &str, data: &Data) -> bool {
    if CUSTOM_CRITERIA.contains(&s) {
        return true;
    }
    if let Some(c) = s.strip_prefix("teamkill.").or_else(|| s.strip_prefix("killedByTeam.")) {
        return TEAM_COLORS.contains(&c);
    }
    let Some((t, v)) = s.split_once(':') else { return false };
    let to_id = |x: &str| x.replacen('.', ":", 1);
    let ty = to_id(t);
    match stat_registry(&ty) {
        Some(reg) => data.contains(reg, &to_id(v)),
        None => !data.has_registry(STAT_TYPE),
    }
}

/// Registry of the values of a statistic type.
fn stat_registry(stat_type: &str) -> Option<&'static str> {
    Some(match stat_type {
        "minecraft:mined" => BLOCK,
        "minecraft:crafted" | "minecraft:used" | "minecraft:broken" | "minecraft:picked_up" | "minecraft:dropped" => {
            ITEM
        }
        "minecraft:killed" | "minecraft:killed_by" => "minecraft:entity_type",
        "minecraft:custom" => "minecraft:custom_stat",
        _ => return None,
    })
}

fn criteria_list(data: &Data) -> Vec<String> {
    let mut v: Vec<String> = CUSTOM_CRITERIA.iter().map(|s| (*s).to_owned()).collect();
    for c in TEAM_COLORS {
        v.push(format!("teamkill.{c}"));
        v.push(format!("killedByTeam.{c}"));
    }
    let key = |id: &str| id.replacen(':', ".", 1);
    for t in data.registry(STAT_TYPE).unwrap_or(&[]) {
        if let Some(reg) = stat_registry(t) {
            for e in data.registry(reg).unwrap_or(&[]) {
                v.push(format!("{}:{}", key(t), key(e)));
            }
        }
    }
    v
}

/// `*` / `?` pattern on a full identifier.
fn glob(pat: &str, s: &str) -> bool {
    let p: Vec<char> = pat.chars().collect();
    let t: Vec<char> = s.chars().collect();
    let (mut i, mut j, mut star, mut mark) = (0usize, 0usize, None, 0usize);
    while j < t.len() {
        if i < p.len() && (p[i] == '?' || p[i] == t[j]) {
            i += 1;
            j += 1;
        } else if i < p.len() && p[i] == '*' {
            star = Some(i);
            mark = j;
            i += 1;
        } else if let Some(s) = star {
            i = s + 1;
            mark += 1;
            j = mark;
        } else {
            return false;
        }
    }
    while i < p.len() && p[i] == '*' {
        i += 1;
    }
    i == p.len()
}

fn resource_or_id_registry(ty: &ArgType) -> &'static str {
    match ty {
        ArgType::LootTable => "minecraft:loot_table",
        ArgType::LootPredicate => "minecraft:predicate",
        ArgType::LootModifier => "minecraft:item_modifier",
        ArgType::ContextFloatProvider => "minecraft:context_float_provider",
        ArgType::ContextIntProvider => "minecraft:context_int_provider",
        ArgType::Dialog => "minecraft:dialog",
        _ => "minecraft:worldgen/configured_feature",
    }
}

/// Identifier of a registry element or inline SNBT value.
fn resource_or_inline(r: &mut Reader<'_>, reg: &str, data: &Data) -> Result<(), CmdError> {
    let start = r.cursor;
    let starts_id =
        r.can_read() && is_id_char(r.peek()) && !r.peek().is_ascii_digit() && r.peek() != '-' && r.peek() != '.';
    if starts_id {
        let id = read_id(r);
        if let Ok(id) = id
            && (!r.can_read() || r.peek() == ' ')
        {
            if data.has_registry(reg) && !data.contains(reg, &id) {
                r.cursor = start;
                return Err(r.err("argument.resource_or_id.no_such_element", vec![id, reg.to_owned()]));
            }
            return Ok(());
        }
        r.cursor = start;
    }
    crate::snbt::parse_argument(r).map(|_| ())
}

// ---------------------------------------------------------------------------------------------
// Blocks

#[derive(Clone, Copy, PartialEq)]
enum BlockSugg {
    IdOrTag,
    Id,
    Tag,
    OpenPropsOrNbt,
    OpenVagueOrNbt,
    PropNameOrEnd,
    VaguePropNameOrEnd,
    PropName,
    VaguePropName,
    Equals,
    Value(usize),
    VagueValue,
    NextOrEnd,
    OpenNbt,
    Nothing,
}

struct BlockParser<'a, 'r> {
    r: &'r mut Reader<'a>,
    data: &'r Data,
    testing: bool,
    sugg: BlockSugg,
    block: Option<String>,
    tag: Option<String>,
    props: Vec<String>,
    loose: Vec<String>,
    loose_key: String,
}

impl<'a, 'r> BlockParser<'a, 'r> {
    fn new(r: &'r mut Reader<'a>, data: &'r Data, testing: bool) -> Self {
        BlockParser {
            r,
            data,
            testing,
            sugg: BlockSugg::Nothing,
            block: None,
            tag: None,
            props: Vec::new(),
            loose: Vec::new(),
            loose_key: String::new(),
        }
    }

    fn def(&self) -> Option<&crate::data::BlockDef> {
        self.data.blocks.get(self.block.as_ref()?)
    }

    fn parse(&mut self) -> Result<(), CmdError> {
        self.sugg = if self.testing { BlockSugg::IdOrTag } else { BlockSugg::Id };
        if self.r.next_is('#') {
            if !self.testing {
                return Err(self.r.err0("argument.block.tag.disallowed"));
            }
            let start = self.r.cursor;
            self.r.skip();
            self.sugg = BlockSugg::Tag;
            let id = read_id(self.r)?;
            if !self.data.tags.is_empty() && !self.data.has_tag(BLOCK, &id) {
                self.r.cursor = start;
                return Err(self.r.err("arguments.block.tag.unknown", vec![id]));
            }
            self.tag = Some(id);
            self.sugg = BlockSugg::OpenVagueOrNbt;
            if self.r.next_is('[') {
                self.loose_props()?;
                self.sugg = BlockSugg::OpenNbt;
            }
        } else {
            let start = self.r.cursor;
            let id = read_id(self.r)?;
            if !self.data.blocks.is_empty() && !self.data.blocks.contains_key(&id) {
                self.r.cursor = start;
                return Err(self.r.err("argument.block.id.invalid", vec![id]));
            }
            self.block = Some(id);
            self.sugg = BlockSugg::OpenPropsOrNbt;
            if self.r.next_is('[') {
                self.props()?;
                self.sugg = BlockSugg::OpenNbt;
            }
        }
        if self.r.next_is('{') {
            self.sugg = BlockSugg::Nothing;
            crate::snbt::compound_argument(self.r)?;
        }
        Ok(())
    }

    fn props(&mut self) -> Result<(), CmdError> {
        self.r.skip();
        self.sugg = BlockSugg::PropNameOrEnd;
        self.r.skip_whitespace();
        let id = self.block.clone().unwrap_or_default();
        while self.r.can_read() && self.r.peek() != ']' {
            self.r.skip_whitespace();
            let ks = self.r.cursor;
            let key = self.r.read_string()?;
            let Some(pi) = self.def().and_then(|d| d.props.iter().position(|(n, _)| *n == key)) else {
                self.r.cursor = ks;
                return Err(self.r.err("argument.block.property.unknown", vec![id, key]));
            };
            if self.props.contains(&key) {
                self.r.cursor = ks;
                return Err(self.r.err("argument.block.property.duplicate", vec![key, id]));
            }
            self.r.skip_whitespace();
            self.sugg = BlockSugg::Equals;
            if !self.r.next_is('=') {
                return Err(self.r.err("argument.block.property.novalue", vec![key, id]));
            }
            self.r.skip();
            self.r.skip_whitespace();
            self.sugg = BlockSugg::Value(pi);
            let vs = self.r.cursor;
            let value = self.r.read_string()?;
            let ok = self.def().is_some_and(|d| d.props[pi].1.contains(&value));
            if !ok {
                self.r.cursor = vs;
                return Err(self.r.err("argument.block.property.invalid", vec![id, value, key]));
            }
            self.props.push(key);
            self.sugg = BlockSugg::NextOrEnd;
            self.r.skip_whitespace();
            if self.r.can_read() {
                if self.r.peek() != ',' {
                    if self.r.peek() != ']' {
                        return Err(self.r.err0("argument.block.property.unclosed"));
                    }
                    break;
                }
                self.r.skip();
                self.sugg = BlockSugg::PropName;
            }
        }
        if self.r.can_read() {
            self.r.skip();
            Ok(())
        } else {
            Err(self.r.err0("argument.block.property.unclosed"))
        }
    }

    fn loose_props(&mut self) -> Result<(), CmdError> {
        self.r.skip();
        self.sugg = BlockSugg::VaguePropNameOrEnd;
        let mut value_start: Option<usize> = None;
        self.r.skip_whitespace();
        let id = self.tag.clone().unwrap_or_default();
        while self.r.can_read() && self.r.peek() != ']' {
            self.r.skip_whitespace();
            let ks = self.r.cursor;
            let key = self.r.read_string()?;
            if self.loose.contains(&key) {
                self.r.cursor = ks;
                return Err(self.r.err("argument.block.property.duplicate", vec![key, id]));
            }
            self.r.skip_whitespace();
            if !self.r.next_is('=') {
                self.r.cursor = ks;
                return Err(self.r.err("argument.block.property.novalue", vec![key, id]));
            }
            self.r.skip();
            self.r.skip_whitespace();
            self.sugg = BlockSugg::VagueValue;
            self.loose_key = key.clone();
            value_start = Some(self.r.cursor);
            self.r.read_string()?;
            self.loose.push(key);
            self.r.skip_whitespace();
            if self.r.can_read() {
                value_start = None;
                if self.r.peek() != ',' {
                    if self.r.peek() != ']' {
                        return Err(self.r.err0("argument.block.property.unclosed"));
                    }
                    break;
                }
                self.r.skip();
                self.sugg = BlockSugg::VaguePropName;
            }
        }
        if self.r.can_read() {
            self.r.skip();
            Ok(())
        } else {
            if let Some(v) = value_start {
                self.r.cursor = v;
            }
            Err(self.r.err0("argument.block.property.unclosed"))
        }
    }

    fn tag_blocks(&self) -> Vec<&crate::data::BlockDef> {
        let Some(t) = &self.tag else { return Vec::new() };
        self.data.tag_members(BLOCK, t).iter().filter_map(|b| self.data.blocks.get(b)).collect()
    }

    fn suggest(&self, b: &mut Builder<'_>) {
        let mut s = b.offset(self.r.cursor);
        let empty = s.remaining.is_empty();
        let mut ids: Vec<&String> = self.data.blocks.keys().collect();
        ids.sort();
        let tags = || {
            let mut t = self.data.tag_names(BLOCK);
            t.sort();
            t
        };
        match self.sugg {
            BlockSugg::IdOrTag => {
                s.suggest_resources_with(tags(), "#", "", |_| None);
                s.suggest_resources(ids);
            }
            BlockSugg::Id => s.suggest_resources(ids),
            BlockSugg::Tag => s.suggest_resources_with(tags(), "#", "", |_| None),
            BlockSugg::OpenPropsOrNbt => {
                if empty {
                    if self.def().is_some_and(|d| !d.props.is_empty()) {
                        s.suggest("[");
                    }
                    if self.def().is_some_and(|d| d.entity) {
                        s.suggest("{");
                    }
                }
            }
            BlockSugg::OpenVagueOrNbt => {
                if empty {
                    let bl = self.tag_blocks();
                    if bl.iter().any(|d| !d.props.is_empty()) {
                        s.suggest("[");
                    }
                    if bl.iter().any(|d| d.entity) {
                        s.suggest("{");
                    }
                }
            }
            BlockSugg::PropNameOrEnd | BlockSugg::PropName => {
                if empty && self.sugg == BlockSugg::PropNameOrEnd {
                    s.suggest("]");
                }
                let p = s.remaining_lower.clone();
                if let Some(d) = self.def() {
                    for (n, _) in &d.props {
                        if !self.props.contains(n) && n.starts_with(&p) {
                            s.suggest(format!("{n}="));
                        }
                    }
                }
            }
            BlockSugg::VaguePropNameOrEnd | BlockSugg::VaguePropName => {
                if empty && self.sugg == BlockSugg::VaguePropNameOrEnd {
                    s.suggest("]");
                }
                let p = s.remaining_lower.clone();
                for d in self.tag_blocks() {
                    for (n, _) in &d.props {
                        if !self.loose.contains(n) && n.starts_with(&p) {
                            s.suggest(format!("{n}="));
                        }
                    }
                }
            }
            BlockSugg::Equals => {
                if empty {
                    s.suggest("=");
                }
            }
            BlockSugg::Value(pi) => {
                if let Some(d) = self.def() {
                    for v in &d.props[pi].1 {
                        s.suggest(v.clone());
                    }
                }
            }
            BlockSugg::VagueValue => {
                let mut more = false;
                for d in self.tag_blocks() {
                    if let Some((_, vals)) = d.props.iter().find(|(n, _)| *n == self.loose_key) {
                        for v in vals {
                            s.suggest(v.clone());
                        }
                    }
                    more |= d.props.iter().any(|(n, _)| !self.loose.contains(n));
                }
                if more {
                    s.suggest(",");
                }
                s.suggest("]");
            }
            BlockSugg::NextOrEnd => {
                if empty {
                    s.suggest("]");
                    if self.def().is_some_and(|d| self.props.len() < d.props.len()) {
                        s.suggest(",");
                    }
                }
            }
            BlockSugg::OpenNbt => {
                let ent = self.def().is_some_and(|d| d.entity) || self.tag_blocks().iter().any(|d| d.entity);
                if empty && ent {
                    s.suggest("{");
                }
            }
            BlockSugg::Nothing => {}
        }
        b.add(s);
    }
}

// ---------------------------------------------------------------------------------------------
// Items

#[derive(Clone, Copy, PartialEq)]
enum ItemSugg {
    Item,
    StartComponents,
    AssignOrRemove,
    Component,
    Assign,
    NextOrEnd,
    Nothing,
}

/// Component value checked by its format (error at the start of the value, like the game).
fn check_component(r: &mut Reader<'_>, at: usize, id: &str, v: &crate::snbt::Value) -> Result<(), CmdError> {
    let text = r.slice(at, r.cursor);
    crate::components::check(id, v, &text).map_err(|msg| {
        r.cursor = at;
        r.err("arguments.item.component.malformed", vec![id.to_owned(), msg])
    })
}

/// Written identifier → full identifier (empty or absent namespace = `minecraft`).
fn full_id(raw: &str) -> String {
    match raw.split_once(':') {
        Some(("", p)) => format!("minecraft:{p}"),
        Some(_) => raw.to_owned(),
        None => format!("minecraft:{raw}"),
    }
}

struct ItemParser<'a, 'r> {
    r: &'r mut Reader<'a>,
    data: &'r Data,
    sugg: ItemSugg,
}

impl<'a, 'r> ItemParser<'a, 'r> {
    fn new(r: &'r mut Reader<'a>, data: &'r Data) -> Self {
        ItemParser { r, data, sugg: ItemSugg::Nothing }
    }

    fn component_type(&mut self) -> Result<String, CmdError> {
        if !self.r.can_read() {
            return Err(self.r.err0("arguments.item.component.expected"));
        }
        let start = self.r.cursor;
        let id = read_id(self.r)?;
        // Components with no saved form: unknown to commands.
        if (self.data.has_registry(COMPONENT) && !self.data.contains(COMPONENT, &id))
            || crate::components::is_transient(&id)
        {
            self.r.cursor = start;
            return Err(self.r.err("arguments.item.component.unknown", vec![id]));
        }
        Ok(id)
    }

    fn parse(&mut self) -> Result<(), CmdError> {
        self.sugg = ItemSugg::Item;
        let start = self.r.cursor;
        let id = read_id(self.r)?;
        if self.data.has_registry(ITEM) && !self.data.contains(ITEM, &id) {
            self.r.cursor = start;
            return Err(self.r.err("argument.item.id.invalid", vec![id]));
        }
        self.sugg = ItemSugg::StartComponents;
        if !self.r.next_is('[') {
            return Ok(());
        }
        self.sugg = ItemSugg::Nothing;
        self.r.expect('[')?;
        self.sugg = ItemSugg::AssignOrRemove;
        let mut seen: Vec<String> = Vec::new();
        while self.r.can_read() && self.r.peek() != ']' {
            self.r.skip_whitespace();
            if self.r.next_is('!') {
                self.r.skip();
                self.sugg = ItemSugg::Component;
                let c = self.component_type()?;
                if seen.contains(&c) {
                    return Err(CmdError { key: "arguments.item.component.repeated", args: vec![c], cursor: None });
                }
                seen.push(c);
                self.sugg = ItemSugg::Nothing;
                self.r.skip_whitespace();
            } else {
                let c = self.component_type()?;
                if seen.contains(&c) {
                    return Err(CmdError { key: "arguments.item.component.repeated", args: vec![c], cursor: None });
                }
                seen.push(c.clone());
                self.sugg = ItemSugg::Assign;
                self.r.skip_whitespace();
                self.r.expect('=')?;
                self.sugg = ItemSugg::Nothing;
                self.r.skip_whitespace();
                let at = self.r.cursor;
                let v = crate::snbt::parse_argument(self.r)?;
                check_component(self.r, at, &c, &v)?;
                self.r.skip_whitespace();
            }
            self.sugg = ItemSugg::NextOrEnd;
            if !self.r.next_is(',') {
                break;
            }
            self.r.skip();
            self.r.skip_whitespace();
            self.sugg = ItemSugg::AssignOrRemove;
            if !self.r.can_read() {
                return Err(self.r.err0("arguments.item.component.expected"));
            }
        }
        self.r.expect(']')?;
        self.sugg = ItemSugg::Nothing;
        Ok(())
    }

    fn suggest(&self, b: &mut Builder<'_>) {
        let mut s = b.offset(self.r.cursor);
        let empty = s.remaining.is_empty();
        let comps = self.data.registry(COMPONENT).unwrap_or(&[]);
        match self.sugg {
            ItemSugg::Item => s.suggest_resources(self.data.registry(ITEM).unwrap_or(&[])),
            ItemSugg::StartComponents => {
                if empty {
                    s.suggest("[");
                }
            }
            ItemSugg::AssignOrRemove => {
                s.suggest("!");
                s.suggest_resources_with(comps, "", "=", |_| None);
            }
            ItemSugg::Component => s.suggest_resources(comps),
            ItemSugg::Assign => {
                if empty {
                    s.suggest("=");
                }
            }
            ItemSugg::NextOrEnd => {
                if empty {
                    s.suggest(",");
                    s.suggest("]");
                }
            }
            ItemSugg::Nothing => {}
        }
        b.add(s);
    }
}

// ---------------------------------------------------------------------------------------------
// Item predicate: `type[conditions]` grammar; suggestions at the furthest failure point.

struct ItemPredParser<'a, 'r> {
    r: &'r mut Reader<'a>,
    data: &'r Data,
    /// Furthest failure point and what was expected there.
    far: usize,
    expect: Vec<Expect>,
    err: Option<CmdError>,
}

#[derive(Clone, PartialEq)]
enum Expect {
    Char(char),
    Items,
    Tags,
    Components,
    Predicates,
}

impl<'a, 'r> ItemPredParser<'a, 'r> {
    fn new(r: &'r mut Reader<'a>, data: &'r Data) -> Self {
        ItemPredParser { r, data, far: 0, expect: Vec::new(), err: None }
    }

    fn fail(&mut self, at: usize, e: Expect, err: Option<CmdError>) {
        if at > self.far || (self.expect.is_empty() && self.err.is_none()) {
            self.far = at;
            self.expect.clear();
            self.err = None;
        }
        if at == self.far {
            if !self.expect.contains(&e) {
                self.expect.push(e);
            }
            if self.err.is_none() {
                self.err = err;
            }
        }
    }

    fn ch(&mut self, c: char) -> bool {
        self.r.skip_whitespace();
        if self.r.next_is(c) {
            self.r.skip();
            true
        } else {
            let at = self.r.cursor;
            let e = self.r.err(crate::LITERAL_INCORRECT, vec![c.to_string()]);
            self.fail(at, Expect::Char(c), Some(e));
            false
        }
    }

    fn id_in(&mut self, what: Expect, check: impl Fn(&Data, &str) -> bool, key: &'static str) -> bool {
        self.r.skip_whitespace();
        let at = self.r.cursor;
        match read_id(self.r) {
            Ok(id) if self.r.cursor > at && check(self.data, &id) => true,
            Ok(id) => {
                self.r.cursor = at;
                let e = (self.r.cursor > at || !id.ends_with(':')).then(|| self.r.err(key, vec![id]));
                self.fail(at, what, e);
                false
            }
            Err(e) => {
                self.fail(at, what, Some(e));
                false
            }
        }
    }

    fn parse(&mut self) -> Result<(), CmdError> {
        let start = self.r.cursor;
        self.far = start;
        if self.top() {
            return Ok(());
        }
        self.r.cursor = self.far;
        Err(self.err.clone().unwrap_or_else(|| self.r.err0("argument.id.invalid")))
    }

    fn top(&mut self) -> bool {
        let start = self.r.cursor;
        if !self.ty() {
            self.r.cursor = start;
            return false;
        }
        let after = self.r.cursor;
        if self.ch('[') {
            // Cut: after `[`, no more backtracking.
            let cs = self.r.cursor;
            if !self.conditions() {
                self.r.cursor = cs;
            }
            if self.ch(']') {
                return true;
            }
            return false;
        }
        self.r.cursor = after;
        true
    }

    fn ty(&mut self) -> bool {
        let s = self.r.cursor;
        if self.id_in(Expect::Items, |d, id| !d.has_registry(ITEM) || d.contains(ITEM, id), "argument.item.id.invalid")
        {
            return true;
        }
        self.r.cursor = s;
        if self.ch('#') {
            return self.id_in(
                Expect::Tags,
                |d, id| d.tags.is_empty() || d.has_tag(ITEM, id),
                "arguments.item.tag.unknown",
            );
        }
        self.r.cursor = s;
        if self.ch('*') {
            return true;
        }
        self.r.cursor = s;
        false
    }

    fn conditions(&mut self) -> bool {
        loop {
            if !self.alternatives() {
                return false;
            }
            let s = self.r.cursor;
            if !self.ch(',') {
                self.r.cursor = s;
                return true;
            }
        }
    }

    fn alternatives(&mut self) -> bool {
        loop {
            if !self.term() {
                return false;
            }
            let s = self.r.cursor;
            if !self.ch('|') {
                self.r.cursor = s;
                return true;
            }
        }
    }

    fn term(&mut self) -> bool {
        let s = self.r.cursor;
        if self.test() {
            return true;
        }
        self.r.cursor = s;
        if self.ch('!') {
            return self.test();
        }
        self.r.cursor = s;
        false
    }

    fn test(&mut self) -> bool {
        let s = self.r.cursor;
        // Pseudo-component `count`; components with no saved form refused.
        let comp = |d: &Data, id: &str| {
            id == "minecraft:count"
                || ((!d.has_registry(COMPONENT) || d.contains(COMPONENT, id)) && !crate::components::is_transient(id))
        };
        let pred = |d: &Data, id: &str| {
            id == "minecraft:count"
                || !d.has_registry(ITEM_PREDICATE)
                || d.contains(ITEM_PREDICATE, id)
                || d.contains(COMPONENT, id)
        };
        if self.id_in(Expect::Components, comp, "arguments.item.component.unknown") {
            let after = self.r.cursor;
            let id = full_id(self.r.slice(s, after).trim_start());
            if self.ch('=') {
                let at = self.r.cursor;
                return match crate::snbt::value(self.r) {
                    Ok(v) => {
                        let at = (at..self.r.cursor).find(|&i| self.r.slice(i, i + 1).trim() != "").unwrap_or(at);
                        match check_component(self.r, at, &id, &v) {
                            Ok(()) => true,
                            Err(e) => {
                                self.fail(at, Expect::Char('{'), Some(e));
                                false
                            }
                        }
                    }
                    Err(e) => {
                        let c = e.cursor.unwrap_or(at);
                        self.fail(c, Expect::Char('{'), Some(e));
                        false
                    }
                };
            }
            self.r.cursor = after;
        }
        self.r.cursor = s;
        if self.id_in(Expect::Predicates, pred, "arguments.item.predicate.unknown") && self.ch('~') {
            return self.tag();
        }
        self.r.cursor = s;
        if self.id_in(Expect::Components, comp, "arguments.item.component.unknown") {
            return true;
        }
        self.r.cursor = s;
        false
    }

    fn tag(&mut self) -> bool {
        let at = self.r.cursor;
        match crate::snbt::value(self.r) {
            Ok(_) => true,
            Err(e) => {
                let c = e.cursor.unwrap_or(at);
                self.fail(c, Expect::Char('{'), Some(e));
                false
            }
        }
    }

    fn suggest(&self, b: &mut Builder<'_>) {
        if self.expect.is_empty() {
            return;
        }
        let mut s = b.offset(self.far);
        for e in &self.expect {
            match e {
                Expect::Char(c) => s.suggest_matching([c.to_string()]),
                Expect::Items => s.suggest_resources(self.data.registry(ITEM).unwrap_or(&[])),
                Expect::Tags => s.suggest_resources(self.data.tag_names(ITEM)),
                Expect::Components => s.suggest_resources(self.data.registry(COMPONENT).unwrap_or(&[])),
                Expect::Predicates => s.suggest_resources(self.data.registry(ITEM_PREDICATE).unwrap_or(&[])),
            }
        }
        b.add(s);
    }
}

// ---------------------------------------------------------------------------------------------
// Suggestions

/// Coordinates suggested (§ 3.3, common rule): only those that parse are kept.
fn suggest_coords(b: &mut Builder<'_>, n: usize, int: bool, data: &Data) {
    let rem = b.remaining.clone();
    let local = rem.starts_with('^');
    let c: [String; 3] = if local {
        ["^".into(), "^".into(), "^".into()]
    } else if int && let Some(p) = data.target_block {
        [p[0].to_string(), p[1].to_string(), p[2].to_string()]
    } else if !int && let Some(p) = data.target_point {
        [format!("{:.2}", p[0]), format!("{:.2}", p[1]), format!("{:.2}", p[2])]
    } else {
        ["~".into(), "~".into(), "~".into()]
    };
    let valid = |s: &str| {
        let ch: Vec<char> = s.chars().collect();
        coords(&mut Reader::new(&ch, 0), n, int, false).is_ok()
    };
    let mut out: Vec<String> = Vec::new();
    if n == 3 {
        if rem.is_empty() {
            let full = format!("{} {} {}", c[0], c[1], c[2]);
            if valid(&full) {
                out.push(c[0].clone());
                out.push(format!("{} {}", c[0], c[1]));
                out.push(full);
            }
        } else {
            let f = java_split_space(&rem);
            if f.len() == 1 {
                let full = format!("{} {} {}", f[0], c[1], c[2]);
                if valid(&full) {
                    out.push(format!("{} {}", f[0], c[1]));
                    out.push(full);
                }
            } else if f.len() == 2 {
                let full = format!("{} {} {}", f[0], f[1], c[2]);
                if valid(&full) {
                    out.push(full);
                }
            }
        }
    } else if rem.is_empty() {
        let full = format!("{} {}", c[0], c[2]);
        if valid(&full) {
            out.push(c[0].clone());
            out.push(full);
        }
    } else {
        let f = java_split_space(&rem);
        if f.len() == 1 {
            let full = format!("{} {}", f[0], c[2]);
            if valid(&full) {
                out.push(full);
            }
        }
    }
    b.suggest_matching(out);
}

fn entity_like(b: &mut Builder<'_>, data: &Data, src: Source, players_only: bool) {
    let input = b.input;
    let mut r = Reader::new(input, b.start);
    let mut p = SelParser::new(&mut r, src.selectors, data);
    let _ = p.parse();
    let names = |nb: &mut Builder<'_>| {
        let mut all: Vec<String> = data.players.clone();
        if !players_only && let Some(u) = &data.target_entity {
            all.push(u.clone());
        }
        nb.suggest_matching(all);
    };
    p.suggest(b, &names);
}

/// Suggestions specific to the type; `true`: they must be asked of the server.
pub fn suggest(ty: &ArgType, b: &mut Builder<'_>, data: &Data, src: Source) -> bool {
    use ArgType as A;
    match ty {
        A::Bool => b.suggest_matching(["true", "false"]),
        A::Entity { players, .. } => entity_like(b, data, src, *players),
        A::GameProfile | A::ScoreHolder { .. } => entity_like(b, data, src, true),
        A::BlockPos => suggest_coords(b, 3, true, data),
        A::ColumnPos => suggest_coords(b, 2, true, data),
        A::Vec3 => suggest_coords(b, 3, false, data),
        A::Vec2 => suggest_coords(b, 2, false, data),
        A::BlockState | A::BlockPredicate => {
            let input = b.input;
            let mut r = Reader::new(input, b.start);
            let mut p = BlockParser::new(&mut r, data, matches!(ty, A::BlockPredicate));
            let _ = p.parse();
            p.suggest(b);
        }
        A::ItemStack => {
            let input = b.input;
            let mut r = Reader::new(input, b.start);
            let mut p = ItemParser::new(&mut r, data);
            let _ = p.parse();
            p.suggest(b);
        }
        A::ItemPredicate => {
            let input = b.input;
            let mut r = Reader::new(input, b.start);
            let mut p = ItemPredParser::new(&mut r, data);
            let _ = p.parse();
            p.suggest(b);
        }
        A::TeamColor => b.suggest_matching(TEAM_COLORS),
        A::Gamemode => b.suggest_matching(GAMEMODES),
        A::EntityAnchor => b.suggest_matching(ANCHORS),
        A::Heightmap => b.suggest_matching(HEIGHTMAPS),
        A::TemplateMirror => b.suggest_matching(MIRRORS),
        A::TemplateRotation => b.suggest_matching(ROTATIONS),
        A::SwingAnimation => b.suggest_matching(SWINGS),
        A::Operation => b.suggest_matching(OPERATIONS),
        A::HexColor => b.suggest_matching(["F00", "FF0000"]),
        A::ScoreboardSlot => {
            let mut v: Vec<String> = vec!["list".into(), "sidebar".into(), "below_name".into()];
            v.extend(TEAM_COLORS.iter().map(|c| format!("sidebar.team.{c}")));
            b.suggest_matching(v);
        }
        A::ItemSlot => b.suggest_matching(slot_names().into_iter().filter(|(_, n)| *n == 1).map(|(s, _)| s)),
        A::ItemSlots => b.suggest_matching(slot_names().into_iter().map(|(s, _)| s)),
        A::ObjectiveCriteria => b.suggest_matching(criteria_list(data)),
        A::Team => b.suggest_matching(&data.teams),
        A::Objective => return true,
        A::Dimension => b.suggest_resources(&data.dimensions),
        A::Particle => b.suggest_resources(data.registry(PARTICLE).unwrap_or(&[])),
        A::Time(_) => {
            let rem: Vec<char> = b.remaining.chars().collect();
            let mut r = Reader::new(&rem, 0);
            if r.read_float().is_ok() {
                let mut s = b.offset(b.start + r.cursor);
                s.suggest_matching(["d", "s", "t", ""]);
                b.add(s);
            }
        }
        A::Resource(reg) | A::ResourceKey(reg) | A::ResourceSelector(reg) => match data.registry(reg) {
            Some(e) => b.suggest_resources(e),
            None => return true,
        },
        A::ResourceOrTag(reg) | A::ResourceOrTagKey(reg) => match data.registry(reg) {
            Some(e) => {
                let tags = data.tag_names(reg);
                b.suggest_resources_with(tags, "#", "", |_| None);
                b.suggest_resources(e);
            }
            None => return true,
        },
        A::LootTable
        | A::LootPredicate
        | A::LootModifier
        | A::ContextFloatProvider
        | A::ContextIntProvider
        | A::Dialog
        | A::Feature => match data.registry(resource_or_id_registry(ty)) {
            Some(e) => b.suggest_resources(e),
            None => return true,
        },
        A::SlotSource => {
            let mut sub = b.offset(b.start);
            sub.suggest_matching(slot_names().into_iter().map(|(s, _)| s));
            b.add(sub);
            match data.registry("minecraft:slot_source") {
                Some(e) => b.suggest_resources(e),
                None => return true,
            }
        }
        _ => {}
    }
    false
}

/// Named provider (§ 1.4); `true`: asked of the server.
pub fn custom_suggestions(id: &str, b: &mut Builder<'_>, data: &Data) -> bool {
    match id {
        "minecraft:available_sounds" => b.suggest_resources(&data.sounds),
        "minecraft:summonable_entities" => {
            let ids: Vec<&String> = data.summonable.iter().map(|(i, _)| i).collect();
            b.suggest_resources_with(ids, "", "", |i| {
                data.summonable.iter().find(|(x, _)| x == i).map(|(_, k)| Tooltip::Key(k.clone()))
            });
        }
        "minecraft:post_effects" => b.suggest_resources(&data.post_effects),
        _ => return true,
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(ty: &ArgType, s: &str, data: &Data) -> Result<usize, &'static str> {
        let c: Vec<char> = s.chars().collect();
        let mut r = Reader::new(&c, 0);
        parse(ty, &mut r, data, Source { restricted: true, selectors: true }).map(|_| r.cursor).map_err(|e| e.key)
    }

    #[test]
    fn coordinate_args() {
        let d = Data::default();
        assert_eq!(run(&ArgType::BlockPos, "~ ~1 5", &d), Ok(6));
        assert_eq!(run(&ArgType::BlockPos, "^ ^ ^1", &d), Ok(6));
        assert_eq!(run(&ArgType::BlockPos, "1 2", &d), Err("argument.pos3d.incomplete"));
        assert_eq!(run(&ArgType::BlockPos, "1.5 2 3", &d), Err("parsing.int.invalid"));
        assert_eq!(run(&ArgType::BlockPos, "^ ~ ^", &d), Err("argument.pos.mixed"));
        assert_eq!(run(&ArgType::Vec3, "0.5 ~ .9", &d), Ok(8));
        assert_eq!(run(&ArgType::Rotation, "~ ~", &d), Ok(3));
        assert_eq!(run(&ArgType::ColumnPos, "1", &d), Err("argument.pos2d.incomplete"));
    }

    #[test]
    fn misc_args() {
        let d = Data::default();
        assert_eq!(run(&ArgType::Time(0), "1.5d", &d), Ok(4));
        assert_eq!(run(&ArgType::Time(0), "1x", &d), Err("argument.time.invalid_unit"));
        assert_eq!(run(&ArgType::Time(1), "0", &d), Err("argument.time.tick_count_too_low"));
        assert_eq!(run(&ArgType::HexColor, "F0a", &d), Ok(3));
        assert_eq!(run(&ArgType::HexColor, "ggg", &d), Err("argument.hexcolor.invalid"));
        assert_eq!(run(&ArgType::Swizzle, "xzy", &d), Ok(3));
        assert_eq!(run(&ArgType::Swizzle, "xx", &d), Err("arguments.swizzle.invalid"));
        assert_eq!(run(&ArgType::ItemSlot, "weapon.*", &d), Err("slot.only_single_allowed"));
        assert_eq!(run(&ArgType::ItemSlots, "weapon.*", &d), Ok(8));
        assert_eq!(run(&ArgType::NbtPath, "a.b[0].\"c d\"[]", &d), Ok(14));
        assert_eq!(run(&ArgType::NbtPath, "{x:1}.a", &d), Ok(7));
        assert_eq!(run(&ArgType::NbtPath, "a[]{x:1}", &d), Err("arguments.nbtpath.node.invalid"));
        assert_eq!(run(&ArgType::Uuid, "dd12be42-52a9-4a91-a8a1-11c01849e498 x", &d), Ok(36));
        assert_eq!(run(&ArgType::Operation, "+=", &d), Ok(2));
        assert_eq!(run(&ArgType::ObjectiveCriteria, "teamkill.red", &d), Ok(12));
        assert_eq!(
            run(&ArgType::Integer(Bounds { min: Some(1), max: Some(64) }), "65", &d),
            Err("argument.integer.big")
        );
        assert_eq!(run(&ArgType::ResourceSelector("x".into()), "foo*", &d), Ok(4));
        assert!(glob("minecraft:a*b?", "minecraft:axxbc"));
        assert!(!glob("minecraft:a*b", "minecraft:axxbc"));
        assert_eq!(run(&ArgType::Message, "hi @e[", &d), Err("argument.entity.options.unterminated"));
        assert_eq!(run(&ArgType::Message, "mail me @ home", &d), Ok(14));
    }

    #[test]
    fn item_predicate_args() {
        let d = Data::default();
        assert_eq!(run(&ArgType::ItemPredicate, "*[damage=1,!enchantments|custom_name]", &d), Ok(37));
        assert_eq!(run(&ArgType::ItemPredicate, "#swords", &d), Ok(7));
        assert_eq!(run(&ArgType::ItemPredicate, "stick[enchantments~[{levels:{min:1}}]]", &d), Ok(38));
        assert!(run(&ArgType::ItemPredicate, "stick[", &d).is_err());
    }

    #[test]
    fn items_and_blocks() {
        let mut d = Data::default();
        d.set_registry(ITEM, vec!["minecraft:diamond".into()]);
        d.blocks.insert(
            "minecraft:oak_stairs".into(),
            crate::data::BlockDef {
                props: vec![("facing".into(), vec!["north".into(), "east".into()])],
                entity: false,
            },
        );
        assert_eq!(run(&ArgType::ItemStack, "diamond[custom_name='x']", &d), Ok(24));
        assert_eq!(run(&ArgType::ItemStack, "dirt", &d), Err("argument.item.id.invalid"));
        assert_eq!(run(&ArgType::BlockState, "oak_stairs[facing=east]", &d), Ok(23));
        assert_eq!(run(&ArgType::BlockState, "oak_stairs[facing=up]", &d), Err("argument.block.property.invalid"));
        assert_eq!(run(&ArgType::BlockState, "oak_stairs[foo=up]", &d), Err("argument.block.property.unknown"));
        assert_eq!(run(&ArgType::BlockState, "#logs", &d), Err("argument.block.tag.disallowed"));
        assert_eq!(run(&ArgType::BlockPredicate, "#logs[axis=y]", &d), Ok(13));
    }
}
