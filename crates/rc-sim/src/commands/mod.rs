//! Commands of the integrated server (mission S5, `docs/specs/commands.md` § 6): tree sent according to
//! the permission level, parsing of the input (same tree, `rc-commands`), error messages
//! of the game, execution (`execute` and its subcommands, vanilla commands), suggestions computed
//! by the server, command blocks.
//!
//! The tree comes from the extractor (`tools/extractor/CommandsExtractor.java`): built by the
//! game itself ("integrated" selection) then filtered for each level, in packet format.

// Failures carry a component (rare, off the hot path).
#![allow(clippy::result_large_err)]

pub mod admin;
pub mod args;
pub mod cmdblock;
pub mod cmds_basic;
pub mod cmds_data;
pub mod cmds_ents;
pub mod cmds_locate;
pub mod cmds_more;
pub mod cmds_s9a;
pub mod cmds_s9b;
pub mod cmds_s9c;
pub mod cmds_s9d;
pub mod cmds_s9e;
pub mod cmds_scores;
pub mod cmds_world;
pub mod ents;
pub mod exec;
pub mod functions;
pub mod hash_order;
pub mod itempred;
pub mod kept_entity_index;
pub mod parse_cache;
pub mod paths;
pub mod schedule;
pub mod score_holders;
pub mod scores;
pub mod select;
pub mod select_candidates;
pub mod team_members;
pub mod snbt;
pub mod src;
pub mod suggest;
pub mod test;
#[macro_use]
pub mod text;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod world_tests;
#[cfg(test)]
mod w11_tests;
#[cfg(test)]
mod g3f10_tests;
#[cfg(test)]
mod scores_tests;
#[cfg(test)]
mod scores_equivalence_tests;
#[cfg(test)]
mod scores_world_equivalence_tests;
#[cfg(test)]
mod team_by_uuid_tests;
#[cfg(test)]
mod function_cache_tests;
#[cfg(test)]
mod command_block_equivalence_tests;
#[cfg(test)]
mod parser_equivalence_tests;
#[cfg(test)]
mod container_commands_tests;
#[cfg(test)]
mod selector_equivalence_tests;
#[cfg(test)]
mod lazy_feedback_tests;
#[cfg(test)]
mod data_world_slot_equivalence_tests;

use std::collections::{BTreeMap, BTreeSet};

use rc_client::{ChatTag, Event};
use rc_commands::tree::{NodeId, Tree};
use rc_proto::nbt::Nbt;
use rc_proto::varint::read_varint;
use rc_proto::{Reader, read_str};
use rc_proto_v777::commands::{Commands, Kind};
use rc_text::Component;

use crate::server::Server;
pub use src::{Fail, Origin, Src};
use text::{GRAY, RED, colored, lit};

/// Result of a command: returned value, or failure (red message).
pub type R = Result<i32, Fail>;

/// Tree of a level: decoded packet and traversal tree.
pub struct Level {
    pub packet: Commands,
    pub tree: Tree,
    /// Name of each node, shared by the kept parses (their argument names).
    node_names: Vec<std::sync::Arc<str>>,
}

impl Level {
    fn new(packet: Commands) -> Level {
        let tree = Tree::new(&packet);
        let node_names = tree.nodes.iter().map(|n| std::sync::Arc::from(n.name.as_str())).collect();
        Level { packet, tree, node_names }
    }
}

/// State of the commands of the open world.
pub struct Cmds {
    /// Levels 0 to 4 (empty if the extractor produced nothing).
    pub levels: Vec<Level>,
    /// Parsing data (registries, tags, blocks, players, teams).
    pub data: rc_commands::Data,
    /// Jar tags only (before those of the world's packs, rebuilt at each reload).
    pub jar_tags: exec::TagSets,
    /// Built-in packs (base and experimental functions), read from the jar at opening.
    pub builtin_packs: Vec<crate::packs::Pack>,
    /// Player level (singleplayer: 4 if cheats, otherwise 0).
    pub op: u8,
    pub scores: scores::Board,
    /// Command storage (`command_storage_*.dat`): identifier → compound.
    pub storage: BTreeMap<String, Nbt>,
    /// Namespaces of the storage to save (the game keeps one file and one dirty mark per namespace).
    pub storage_dirty: BTreeSet<String>,
    /// Force-loaded chunks (`chunk_tickets.dat`, `forceload`).
    pub forced: BTreeSet<(i32, i32)>,
    pub forced_dirty: bool,
    /// Changed rules (rewrite `game_rules.dat`).
    pub rules_dirty: bool,
    /// Functions of the data packs: identifier → compiled function; function tags (member lists
    /// shared with the calls that run them).
    pub functions: BTreeMap<String, functions::Function>,
    pub function_tags: BTreeMap<String, std::sync::Arc<[String]>>,
    /// Data (re)loaded: the `#minecraft:load` tag goes out at the next tick.
    pub post_reload: bool,
    /// Server-side registries not synchronized (structures, advancements, templates…).
    pub server_regs: BTreeMap<String, Vec<String>>,
    /// Most recent suggestions request (processed at the next tick).
    pub pending_suggest: Option<(i32, String)>,
    /// Scheduled functions (`schedule`).
    pub scheduled: schedule::ScheduledFunctions,
    /// Optimised command paths, read from their switches when the world opens.
    pub paths: paths::CommandPaths,
    /// Parses of the texts run by command blocks, minecarts and the player (`PARSECACHE`).
    pub parse_cache: parse_cache::ParseCache,
    /// Execution depth (nested functions, safeguard).
    pub depth: u32,
    /// Return in progress (`return`) in a function: (success, value).
    pub returned: Option<(bool, i32)>,
    /// Execution context (command quota, fork limit, return mode).
    pub flow: exec::Flow,
    /// Successful executions of the current command (multiple sources counted).
    pub successes: i32,
    /// Stopwatches (world state, shared by all players; the player's
    /// camera stays in their advancements).
    pub s9: cmds_s9a::State,
    /// Game speed and freeze (`tick`).
    pub ticking: crate::tickrate::TickRate,
    /// Effects of non-player entities (network identifier); the player: `effects.rs` (S7).
    pub mob_effects: BTreeMap<i32, Vec<ents::Effect>>,
    /// Time markers of the timelines (`time set day`…).
    pub time_markers: Vec<Marker>,
    /// World border of the open dimension.
    pub border: Border,
    pub border_dirty: bool,
    /// Generator for `locate` (built on demand) and structure data from the jar.
    pub locgen: Option<(String, std::sync::Arc<rc_worldgen::chunkgen::DimGen>)>,
    pub locate: cmds_locate::LocateData,
    /// Custom boss bars.
    pub bars: Vec<cmds_more::Bar>,
    pub bars_dirty: bool,
    /// Messages emitted in a parked dimension (sends cut off), rendered on return.
    pub deferred: std::cell::RefCell<Vec<Event>>,
    /// G6-F14: templates known to the server (world, game, structure block captures).
    pub templates: crate::tech_blocks::Templates,
    /// G3-F06: in-game tests (registries frozen at opening, runner).
    pub tests: crate::gametest::GameTests,
}

/// Time marker: clock, identifier, ticks, timeline period, suggested?
#[derive(Debug, Clone)]
pub struct Marker {
    pub clock: String,
    pub id: String,
    pub ticks: i32,
    pub period: Option<u32>,
    pub show: bool,
}

/// Border (`world_border.dat`).
#[derive(Debug, Clone)]
pub struct Border {
    pub center: [f64; 2],
    pub size: f64,
    pub damage: f32,
    pub buffer: f32,
    pub warn_time: i32,
    pub warn_blocks: i32,
    /// Transition in progress: start size, end size, duration, ticks remaining.
    pub lerp: Option<(f64, f64, i64, i64)>,
}

impl Border {
    fn read(dim_dir: &std::path::Path) -> Border {
        let b = crate::nbtio::read_dat(&dim_dir.join("data").join("minecraft").join("world_border.dat"))
            .ok()
            .and_then(|v| v.get("data").cloned());
        let f = |k: &str, d: f64| b.as_ref().and_then(|b| b.get(k)).and_then(crate::mobs::mob::f64_of).unwrap_or(d);
        let size = f("size", 59_999_968.0);
        let lerp_time = f("lerp_time", 0.0) as i64;
        Border {
            lerp: (lerp_time > 0).then(|| (size, f("lerp_target", size), lerp_time, lerp_time)),
            center: [f("center_x", 0.0), f("center_z", 0.0)],
            size,
            damage: f("damage_per_block", 0.2) as f32,
            buffer: f("safe_zone", 5.0) as f32,
            warn_time: f("warning_time", 300.0) as i32,
            warn_blocks: f("warning_blocks", 5.0) as i32,
        }
    }

    /// Border as the physics sees it (current size, centre, warnings): the server's entities
    /// collide with it like the game's.
    pub(crate) fn physics(&self) -> rc_physics::WorldBorder {
        let mut w = rc_physics::WorldBorder::default();
        w.set_center(self.center[0], self.center[1]);
        w.set_size(self.size);
        w.warn_blocks = self.warn_blocks;
        w.warn_ticks = self.warn_time;
        w
    }

    fn to_nbt(&self) -> Nbt {
        Nbt::Compound(vec![
            ("center_x".into(), Nbt::Double(self.center[0])),
            ("center_z".into(), Nbt::Double(self.center[1])),
            ("size".into(), Nbt::Double(self.size)),
            ("lerp_target".into(), Nbt::Double(self.lerp.map_or(self.size, |l| l.1))),
            ("lerp_time".into(), Nbt::Long(self.lerp.map_or(0, |l| l.3))),
            ("damage_per_block".into(), Nbt::Double(f64::from(self.damage))),
            ("safe_zone".into(), Nbt::Double(f64::from(self.buffer))),
            ("warning_time".into(), Nbt::Int(self.warn_time)),
            ("warning_blocks".into(), Nbt::Int(self.warn_blocks)),
        ])
    }
}

/// Time markers of the jar timelines (`data/*/timeline/*.json`).
fn load_markers(jar: &crate::registries::Jar) -> Vec<Marker> {
    let mut out = Vec::new();
    for (k, bytes) in &jar.files {
        let Some(rest) = k.strip_prefix("data/") else { continue };
        let Some((_, path)) = rest.split_once('/') else { continue };
        if !path.starts_with("timeline/") {
            continue;
        }
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) else { continue };
        let clock = v.get("clock").and_then(serde_json::Value::as_str).unwrap_or("").to_owned();
        let period = v.get("period_ticks").and_then(serde_json::Value::as_u64).map(|p| p as u32);
        let Some(m) = v.get("time_markers").and_then(serde_json::Value::as_object) else { continue };
        for (id, mv) in m {
            let (ticks, show) = match mv {
                serde_json::Value::Number(n) => (n.as_i64().unwrap_or(0) as i32, false),
                o => (
                    o.get("ticks").and_then(serde_json::Value::as_i64).unwrap_or(0) as i32,
                    o.get("show_in_commands").and_then(serde_json::Value::as_bool).unwrap_or(false),
                ),
            };
            out.push(Marker { clock: clock.clone(), id: args::id(id), ticks, period, show });
        }
    }
    out
}

pub(crate) fn load_levels(path: &std::path::Path) -> Vec<Level> {
    let Ok(bytes) = std::fs::read(path) else {
        eprintln!("singleplayer: command tree missing ({}): redo the data preparation", path.display());
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 4 <= bytes.len() {
        let n = u32::from_be_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]) as usize;
        i += 4;
        let Some(body) = bytes.get(i..i + n) else { break };
        i += n;
        match rc_proto_v777::commands::read_commands(&mut Reader::new(body)) {
            Ok(c) => out.push(Level::new(c)),
            Err(e) => {
                eprintln!("singleplayer: command tree unreadable: {e}");
                return Vec::new();
            }
        }
    }
    out
}

/// Tags of the synchronized and built-in registries (`regs.tags`) as member names.
pub(crate) fn registry_tags(data: &rc_data::VersionData, regs: &crate::registries::Registries) -> exec::TagSets {
    let mut out = exec::TagSets::new();
    for (tags_reg, entries) in &regs.tags {
        let mut map = hashbrown::HashMap::new();
        let reg_entries: Vec<String> =
            data.registry(tags_reg).map(|r| r.entries.clone()).unwrap_or_else(|| regs.entries(tags_reg).to_vec());
        for (tag, ids) in entries {
            let members = ids.iter().filter_map(|&i| reg_entries.get(i as usize).cloned()).collect::<Vec<String>>();
            map.insert(tag.clone(), members);
        }
        out.insert(tags_reg.clone(), map);
    }
    out
}

/// Data folders whose file ids the commands know as a registry (parsing, `*`, suggestions),
/// with that registry's name. These four are world generation data, frozen when the world opens.
const IDS_FROZEN_AT_OPENING: [(&str, &str); 4] = [
    ("worldgen/structure", "minecraft:worldgen/structure"),
    ("worldgen/configured_feature", "minecraft:worldgen/configured_feature"),
    ("worldgen/template_pool", "minecraft:worldgen/template_pool"),
    ("worldgen/structure_set", "minecraft:worldgen/structure_set"),
];

/// Same, for the folders the game reads again on a data reload (`/reload`, `/datapack`).
const IDS_LISTED_AGAIN_ON_RELOAD: [(&str, &str); 8] = [
    ("advancement", "minecraft:advancement"),
    ("loot_table", "minecraft:loot_table"),
    ("predicate", "minecraft:predicate"),
    ("item_modifier", "minecraft:item_modifier"),
    ("context_int_provider", "minecraft:context_int_provider"),
    ("context_float_provider", "minecraft:context_float_provider"),
    ("recipe", "minecraft:recipe"),
    ("structure", "minecraft:structure"),
];

/// Lists for the commands the ids (`ns:path`, sorted) of the files each of `folders` holds in the
/// pack stack `jar`, in one pass over the stack. An empty folder is not listed, so its ids are
/// not checked when a command is read; a folder listed before and now empty gets an empty list
/// (every id refused) instead of keeping the ids of the stack before.
fn list_pack_ids(
    jar: &crate::registries::Jar,
    folders: &[(&str, &str)],
    command_data: &mut rc_commands::Data,
    server_regs: &mut BTreeMap<String, Vec<String>>,
) {
    let mut ids_by_folder: Vec<Vec<String>> = vec![Vec::new(); folders.len()];
    for file in jar.files.keys() {
        let Some((namespace, path)) = file.strip_prefix("data/").and_then(|rest| rest.split_once('/')) else {
            continue;
        };
        for ((folder, _), ids) in folders.iter().zip(&mut ids_by_folder) {
            let in_folder = path.strip_prefix(folder).and_then(|rest| rest.strip_prefix('/'));
            if let Some(id_path) = in_folder.and_then(|p| p.strip_suffix(".json").or_else(|| p.strip_suffix(".nbt"))) {
                ids.push(format!("{namespace}:{id_path}"));
                break;
            }
        }
    }
    for ((_, registry), mut ids) in folders.iter().zip(ids_by_folder) {
        ids.sort();
        ids.dedup();
        if ids.is_empty() {
            if server_regs.remove(*registry).is_some() {
                command_data.set_registry(registry, Vec::new());
            }
            continue;
        }
        server_regs.insert((*registry).to_owned(), ids.clone());
        // Loot tables and predicates are also read inline: known registry.
        command_data.set_registry(registry, ids);
    }
}

impl Cmds {
    /// After a data reload: the ids of the folders the game reads again, listed from the new
    /// pack stack `jar`. Before the functions are compiled: their lines are read with these lists.
    pub(crate) fn list_reloaded_pack_ids(&mut self, jar: &crate::registries::Jar) {
        list_pack_ids(jar, &IDS_LISTED_AGAIN_ON_RELOAD, &mut self.data, &mut self.server_regs);
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) fn new(
        jar: &crate::registries::Jar,
        data: &rc_data::VersionData,
        regs: &crate::registries::Registries,
        world: &std::path::Path,
        dim_dir: &std::path::Path,
        player: &str,
        allow_commands: bool,
    ) -> Cmds {
        let mut d = rc_commands::Data::from_version(data);
        for (name, entries) in &regs.synced {
            d.set_registry(name, entries.clone());
        }
        let mut server_regs: BTreeMap<String, Vec<String>> = BTreeMap::new();
        let every_listed_folder = [&IDS_FROZEN_AT_OPENING[..], &IDS_LISTED_AGAIN_ON_RELOAD[..]].concat();
        list_pack_ids(jar, &every_listed_folder, &mut d, &mut server_regs);
        // "Named" registries (`resource_or_id`) expected under these names by the parser.
        let jar_tags = registry_tags(data, regs);
        for (tags_reg, map) in &jar_tags {
            d.set_tags(tags_reg, map.clone());
        }
        d.particle_check = Some(cmds_basic::check_particle);
        d.dimensions = crate::server::DIMENSIONS.iter().map(|s| (*s).to_owned()).collect();
        if let Some(r) = data.registry("minecraft:sound_event") {
            d.sounds = r.entries.clone();
        }
        d.summonable = data
            .registry("minecraft:entity_type")
            .map(|r| {
                r.entries
                    .iter()
                    .filter(|e| !matches!(e.as_str(), "minecraft:player" | "minecraft:fishing_bobber"))
                    .map(|e| (e.clone(), format!("entity.{}", e.replace(':', "."))))
                    .collect()
            })
            .unwrap_or_default();
        let op = if allow_commands { 4 } else { 0 };
        d.op_level = i32::from(op);
        let levels = load_levels(&rc_data::commands_path(&data.version));
        exec::apply_pack_tags(jar, &mut d, &jar_tags);
        let paths = paths::CommandPaths::from_switches();
        let (functions, function_tags) = exec::load_functions(jar, &levels, &d, &paths);
        let mut c = Cmds {
            levels,
            data: d,
            jar_tags,
            builtin_packs: Vec::new(),
            op,
            scores: scores::Board::read(world),
            storage: cmds_data::read_storage(world),
            storage_dirty: BTreeSet::new(),
            forced: cmds_world::read_forced(dim_dir),
            forced_dirty: false,
            rules_dirty: false,
            functions,
            function_tags,
            post_reload: true,
            server_regs,
            pending_suggest: None,
            scheduled: schedule::ScheduledFunctions::new(paths.schedule_queue),
            paths,
            parse_cache: parse_cache::ParseCache::default(),
            depth: 0,
            returned: None,
            flow: exec::Flow::default(),
            successes: 0,
            ticking: crate::tickrate::TickRate::default(),
            s9: Default::default(),
            mob_effects: BTreeMap::new(),
            time_markers: load_markers(jar),
            border: Border::read(dim_dir),
            border_dirty: false,
            locgen: None,
            locate: cmds_locate::LocateData::read(jar),
            bars: cmds_more::read_bars(world),
            bars_dirty: false,
            deferred: std::cell::RefCell::new(Vec::new()),
            templates: Default::default(),
            tests: crate::gametest::GameTests::read(jar, regs),
        };
        c.data.teams = c.scores.teams.iter().map(|t| t.name.clone()).collect();
        c.data.players = vec![player.to_owned()];
        c
    }

    pub fn level(&self, op: u8) -> Option<&Level> {
        self.levels.get(usize::from(op.min(4)))
    }

    /// Data reloaded (`/reload`, `/datapack`): new functions and tags; the parses kept for the
    /// former tags and registries are forgotten.
    pub(crate) fn functions_reloaded(
        &mut self,
        functions: BTreeMap<String, functions::Function>,
        tags: BTreeMap<String, std::sync::Arc<[String]>>,
    ) {
        self.functions = functions;
        self.function_tags = tags;
        self.parse_cache.clear();
    }
}

/// Parse segment: nodes read up to a redirect, readable path.
#[derive(Debug, Clone)]
pub struct Seg {
    /// Path: node names (arguments between `<>`), preceded by the name of the root node of the
    /// segment if it is not the root (`execute` subcommands, aliases).
    pub path: Vec<String>,
    /// Arguments: name (shared with the tree's level) → byte range in the parsed text.
    pub args: Vec<(std::sync::Arc<str>, (u32, u32))>,
    /// Last node read (possible redirect).
    pub last: NodeId,
    /// `path` joined by spaces, built once (every run of the command matches on it).
    path_text: String,
}

impl Seg {
    pub fn path_str(&self) -> &str {
        &self.path_text
    }
}

/// Parsed and split input. Kept by the functions and the parse cache: the text is stored once,
/// as UTF-8 (a `Vec<char>` would take 4 bytes per character for every kept line).
#[derive(Debug)]
pub struct Parsed {
    pub text: String,
    pub segs: Vec<Seg>,
}

impl Parsed {
    /// Bytes held: the structure and everything it owns (memory of the kept parses, benches).
    #[cfg(test)]
    pub(crate) fn held_bytes(&self) -> usize {
        let seg = |s: &Seg| {
            size_of::<Seg>()
                + s.path.capacity() * size_of::<String>()
                + s.path.iter().map(String::capacity).sum::<usize>()
                + s.args.capacity() * size_of::<(std::sync::Arc<str>, (u32, u32))>()
                + s.path_text.capacity()
        };
        size_of::<Parsed>() + self.text.capacity() + self.segs.iter().map(seg).sum::<usize>()
    }

    pub fn arg(&self, seg: &Seg, name: &str) -> Option<String> {
        self.range(seg, name).map(|(a, b)| self.text[a..b].to_owned())
    }

    /// Byte range of an argument in `text`.
    pub fn range(&self, seg: &Seg, name: &str) -> Option<(usize, usize)> {
        seg.args.iter().find(|(n, _)| **n == *name).map(|&(_, (a, b))| (a as usize, b as usize))
    }
}

/// Raw parse of an input (no mandatory `/`) with the tree of a level; error =
/// message(s) already formed like the game.
/// Parse error (§ 4.4 server side: the parse error).
fn parse_error(p: &rc_commands::Parse, len: usize) -> Option<rc_commands::CmdError> {
    if p.cursor < len {
        if p.errors.len() == 1 {
            Some(p.errors[0].1.clone())
        } else if p.ctx.nodes.is_empty() {
            Some(rc_commands::CmdError::new("command.unknown.command", Vec::new(), p.cursor))
        } else {
            Some(rc_commands::CmdError::new("command.unknown.argument", Vec::new(), p.cursor))
        }
    } else if !p.ctx.last_child().executable {
        Some(rc_commands::CmdError::new("command.unknown.command", Vec::new(), p.cursor))
    } else {
        None
    }
}

/// Parse of `text` with the tree of a level (the walk chosen by `no_alloc`); the input as
/// characters, for the error texts.
fn parse_text(lvl: &Level, data: &rc_commands::Data, text: &str, op: u8, no_alloc: bool) -> (Vec<char>, rc_commands::Parse) {
    let input: Vec<char> = text.chars().collect();
    let src = rc_commands::parse::Source { restricted: true, selectors: op >= 2 };
    let p = rc_commands::parse::parse_with(&lvl.tree, &input, data, src, no_alloc);
    (input, p)
}

/// Parse error text as the game writes it in its logs and macro messages: message, then "at
/// position N: …<--[HERE]".
fn log_text(e: &rc_commands::CmdError, input: &[char]) -> String {
    let msg = cmds_ents::plain(&tr(e.key, e.args.iter().map(|a| text::A::S(a.clone())).collect()));
    let Some(c) = e.cursor else { return msg };
    let c = c.min(input.len());
    let dots = if c > 10 { "..." } else { "" };
    let before: String = input[c.saturating_sub(10)..c].iter().collect();
    format!("{msg} at position {c}: {dots}{before}<--[HERE]")
}

/// Parse error lines shown to the source: the message, then the context (command offered on
/// click, rest underlined in red, "<--[HERE]").
fn chat_lines(e: &rc_commands::CmdError, input: &[char]) -> Vec<Component> {
    let start = usize::from(input.first() == Some(&'/'));
    let body: String = input[start..].iter().collect();
    let mut out = vec![tr(e.key, e.args.iter().map(|a| text::A::S(a.clone())).collect())];
    if let Some(c) = e.cursor {
        let c = c.saturating_sub(start).min(body.chars().count());
        let chars: Vec<char> = body.chars().collect();
        let mut ctx = colored(lit(""), GRAY);
        suggest_cmd(&mut ctx, &body);
        let mut extra = Vec::new();
        if c > 10 {
            extra.push(lit("..."));
        }
        extra.push(lit(chars[c.saturating_sub(10)..c].iter().collect::<String>()));
        if c < chars.len() {
            let mut rest = colored(lit(chars[c..].iter().collect::<String>()), RED);
            rest.style.underlined = Some(true);
            extra.push(rest);
        }
        let mut here = colored(tr("command.context.here", Vec::new()), RED);
        here.style.italic = Some(true);
        extra.push(here);
        ctx.extra = extra;
        out.push(ctx);
    }
    out
}

/// Segments of a parse that succeeded; argument ranges turned into byte ranges of `text`.
fn parsed_from(lvl: &Level, p: &rc_commands::Parse, text: &str) -> Parsed {
    // Byte offset of each character index (and of the end), for the ranges of the parser; a
    // command fits in `u32` bytes (the protocol caps it at 32,767 characters).
    let mut offsets: Vec<u32> = text.char_indices().map(|(b, _)| b as u32).collect();
    offsets.push(text.len() as u32);
    let bytes = |(a, b): (usize, usize)| (offsets[a.min(offsets.len() - 1)], offsets[b.min(offsets.len() - 1)]);
    let mut segs = Vec::new();
    let mut ctx = Some(&p.ctx);
    while let Some(c) = ctx {
        // Exact capacities: a kept parse lives as long as its function.
        let mut path = Vec::with_capacity(c.nodes.len() + 1);
        if c.root != lvl.tree.root {
            path.push(lvl.tree.node(c.root).name.clone());
        }
        let arguments = c.nodes.iter().filter(|(id, _)| lvl.tree.node(*id).kind == Kind::Argument).count();
        let mut args = Vec::with_capacity(arguments);
        let mut last = c.root;
        for &(id, r) in &c.nodes {
            let n = lvl.tree.node(id);
            if n.kind == Kind::Argument {
                path.push(format!("<{}>", n.name));
                args.push((lvl.node_names[id as usize].clone(), bytes(r)));
            } else {
                path.push(n.name.clone());
            }
            last = id;
        }
        let path_text = path.join(" ");
        segs.push(Seg { path, args, last, path_text });
        ctx = c.child.as_deref();
    }
    segs.shrink_to_fit();
    Parsed { text: text.to_owned(), segs }
}

/// Parse error text as the game writes it in its logs and macro messages; `None` if the command
/// is valid.
pub fn syntax_message(lvl: &Level, data: &rc_commands::Data, text: &str, op: u8, no_alloc: bool) -> Option<String> {
    let (input, p) = parse_text(lvl, data, text, op, no_alloc);
    parse_error(&p, input.len()).map(|e| log_text(&e, &input))
}

/// A function line (level 2, the game's function permission level) parsed once: the parsed
/// command, or its error in the log's words (one parse gives both).
pub fn parse_function_line(lvl: &Level, data: &rc_commands::Data, text: &str, no_alloc: bool) -> Result<Parsed, String> {
    let (input, p) = parse_text(lvl, data, text, 2, no_alloc);
    match parse_error(&p, input.len()) {
        Some(e) => Err(log_text(&e, &input)),
        None => Ok(parsed_from(lvl, &p, text)),
    }
}

pub fn analyse(lvl: &Level, data: &rc_commands::Data, text: &str, op: u8) -> Result<Parsed, Vec<Component>> {
    analyse_with(lvl, data, text, op, rc_commands::parse::no_alloc_switch())
}

/// `analyse` with the parser walk chosen by the caller.
pub fn analyse_with(lvl: &Level, data: &rc_commands::Data, text: &str, op: u8, no_alloc: bool) -> Result<Parsed, Vec<Component>> {
    let (input, p) = parse_text(lvl, data, text, op, no_alloc);
    match parse_error(&p, input.len()) {
        Some(e) => Err(chat_lines(&e, &input)),
        None => Ok(parsed_from(lvl, &p, text)),
    }
}

fn tr(key: &str, args: Vec<text::A>) -> Component {
    text::tr(key, args)
}

fn suggest_cmd(c: &mut Component, body: &str) {
    text::suggest_click(c, format!("/{body}"));
}

impl Server {
    /// Tree of the current level sent to the client (entering the game, permission change).
    pub(crate) fn send_commands(&self) {
        if let Some(l) = self.cmd.level(self.cmd.op) {
            let _ = self.send(Event::Commands(Box::new(l.packet.clone())));
        }
    }

    /// Client packets specific to commands; true if consumed.
    pub(crate) fn commands_packet(&mut self, p: &[u8]) -> bool {
        let ids = self.peer.ids();
        let mut r = Reader::new(p);
        let Ok(id) = read_varint(&mut r) else { return false };
        let r = &mut r;
        if id == ids.chat_command_out || id == ids.chat_command_signed {
            let Ok(text) = read_str(r, 32767) else { return true };
            let text = text.to_owned();
            self.player_command(&text);
            true
        } else if id == ids.chat_out {
            let Ok(text) = read_str(r, 256) else { return true };
            let text = text.to_owned();
            self.player_chat(&text);
            true
        } else if id == ids.command_suggestion_out {
            let (Ok(tx), Ok(text)) = (read_varint(r), read_str(r, 32500).map(str::to_owned)) else { return true };
            self.cmd.pending_suggest = Some((tx, text));
            true
        } else if id == ids.set_command_block {
            self.on_set_command_block(r);
            true
        } else if id == ids.set_command_minecart {
            self.on_set_command_minecart(r);
            true
        } else if id == ids.change_game_mode {
            // Mode selector (F3+F4): allowed for operators (`gamemode`).
            if let Ok(m) = read_varint(r)
                && self.cmd.op >= 2
            {
                self.change_mode_feedback(m, true);
            }
            true
        } else {
            false
        }
    }

    /// Chat message: `chat.type.text` "<name> message".
    pub(crate) fn player_chat(&mut self, text: &str) {
        if text.chars().any(|c| !rc_text::allowed_chat_char(c)) {
            return;
        }
        let name = self.player_display_name();
        let c = tr("chat.type.text", vec![name.into(), lit(text).into()]);
        self.chat_out(Event::Chat { component: Box::new(c), tag: ChatTag::NotSecure });
    }

    /// Command typed by the player.
    pub(crate) fn player_command(&mut self, text: &str) {
        let src = self.player_source();
        self.perform(src, text);
    }

    /// Executes a complete command for a source; returns (success, result).
    pub fn perform(&mut self, src: Src, text: &str) -> (bool, i32) {
        self.perform_as(&src, text)
    }

    /// `perform` with a borrowed source.
    pub fn perform_as(&mut self, src: &Src, text: &str) -> (bool, i32) {
        // Texts rendered on the server side (conversion to a plain string): server language, en_us (S7).
        cmds_ents::SERVER_LANG.get_or_init(|| crate::stations::en_us(&self.life.items));
        if self.exhausted() {
            return (false, 0);
        }
        let text = text.strip_prefix('/').unwrap_or(text);
        match self.parse_to_run(src.level, text) {
            None => {
                self.fail(src, tr("command.unknown.command", Vec::new()));
                (false, 0)
            }
            Some(Ok(parsed)) => self.in_context(|sv| sv.run_parsed(src, &parsed)),
            Some(Err(msgs)) => {
                // Moved out when not shared with the cache.
                let msgs = std::sync::Arc::try_unwrap(msgs).unwrap_or_else(|shared| (*shared).clone());
                for m in msgs {
                    self.fail(src, m);
                }
                (false, 0)
            }
        }
    }

    /// Parse of a command about to run at level `op` (`None`: no tree for that level). With
    /// `PARSECACHE`, a text already run comes back from the cache: the parse is a pure function
    /// of the level, the text and the pack data (emptied at `/reload`).
    pub(crate) fn parse_to_run(&mut self, op: u8, text: &str) -> Option<parse_cache::CachedParse> {
        let cmd = &mut self.cmd;
        let lvl = cmd.levels.get(usize::from(op.min(4)))?;
        let (data, paths) = (&cmd.data, cmd.paths);
        let parse = || match analyse_with(lvl, data, text, op, paths.parser_no_alloc) {
            Ok(p) => Ok(std::sync::Arc::new(p)),
            Err(msgs) => Err(std::sync::Arc::new(msgs)),
        };
        if !paths.parse_cache {
            return Some(parse());
        }
        if let Some(hit) = cmd.parse_cache.get(op, text) {
            return Some(hit);
        }
        let fresh = parse();
        cmd.parse_cache.insert(op, text, fresh.clone());
        Some(fresh)
    }

    /// Success message: to the player if the rule allows it, to the output of a command block
    /// otherwise; then relayed to the other operators unless the game keeps this message for its
    /// source (`admin::quiet`).
    pub fn ok(&mut self, src: &Src, msg: Component) {
        self.success(src, msg, None);
    }

    /// `ok` with a message built only when someone receives it (`success_would_emit`): a
    /// function's lines and the commands of a silent source then build no text at all. `build`
    /// must be pure (no draw, no write): the former path (`RUSTCRAFT_OPT_LAZYFEEDBACK=0`) always
    /// calls it.
    pub fn ok_with(&mut self, src: &Src, build: impl FnOnce(&Server) -> Component) {
        let msg = self.feedback_wanted(src).then(|| build(self));
        self.ok_built(src, msg);
    }

    /// Is the success message of `src` worth building (`ok_with`)? Always on the former path.
    pub(crate) fn feedback_wanted(&self, src: &Src) -> bool {
        !self.cmd.paths.lazy_feedback || self.success_would_emit(src, None)
    }

    /// `ok` of a message built only if `feedback_wanted` said so (`None`: nobody receives it, and
    /// only the scoreboard packets every success sends first go out).
    pub(crate) fn ok_built(&mut self, src: &Src, msg: Option<Component>) {
        match msg {
            Some(m) => self.success(src, m, None),
            None => self.flush_scores(),
        }
    }

    /// Success message the game never relays to the other operators (its call says so, for a
    /// message that other calls do relay: tick status of `tick query`).
    pub fn ok_quiet(&mut self, src: &Src, msg: Component) {
        self.success(src, msg, Some(false));
    }

    /// Does a success of `src` reach anyone? The branches of `success`, in its order: a
    /// `/debug function` trace takes every message; otherwise a source with output, while
    /// `send_command_feedback` is on, informs its player, its command block's tracked output or
    /// the operators. `relay`: the call's own choice (`None`: the message decides, so it is
    /// needed whenever an operator could receive it).
    pub(crate) fn success_would_emit(&self, src: &Src, relay: Option<bool>) -> bool {
        if cmds_ents::tracing() {
            return true;
        }
        if src.silent || !self.rule("minecraft:send_command_feedback", true) {
            return false;
        }
        let own_output = match src.origin {
            Origin::Player => true,
            Origin::Block(p) => self.command_block_tracks_output(p),
            Origin::Minecart(_) | Origin::Server => false,
        };
        own_output || (relay != Some(false) && self.admin_recipient(src))
    }

    /// `relay`: the game's choice for this call, `None` to read it from the message.
    fn success(&mut self, src: &Src, msg: Component, relay: Option<bool>) {
        // Scoreboard packets first: the game broadcasts them during the operation, before the return.
        self.flush_scores();
        if !self.success_would_emit(src, relay) || cmds_ents::trace_message(&msg) {
            return; // nobody receives it, or `/debug function` traces it instead of the output
        }
        // From here the source has an output and `send_command_feedback` is on. The relay is
        // built before the message moves to the source; the table is read only when an operator
        // could receive it (never for the host's own commands in singleplayer).
        let admin = (self.admin_recipient(src) && relay.unwrap_or_else(|| !admin::quiet(&msg))).then(|| {
            let mut c = tr!("chat.type.admin", src.display.clone(), msg.clone());
            c.style.color = Some(GRAY);
            c.style.italic = Some(true);
            c
        });
        match src.origin {
            Origin::Player => self.chat_out(Event::Chat { component: Box::new(msg), tag: ChatTag::System }),
            Origin::Block(p) => self.command_block_output(p, msg, true),
            Origin::Minecart(_) | Origin::Server => {}
        }
        if let Some(c) = admin {
            self.to_host(c);
        }
    }

    /// Does an operator other than the source receive its relayed successes? The operators of the
    /// integrated server: the host when the world allows commands (the guests are never on the
    /// operator list, whatever their permission level). The source must also inform them: a
    /// player or the server always, a command block (or minecart) with `command_block_output`
    /// while it tracks its output (without tracking it has no output at all).
    fn admin_recipient(&self, src: &Src) -> bool {
        if !self.level.allow_commands {
            return false;
        }
        let host_current = self.lan.as_ref().is_none_or(|l| l.active.is_none());
        match src.origin {
            Origin::Player => !host_current,
            Origin::Server => true,
            Origin::Block(p) => self.rule("minecraft:command_block_output", true) && self.command_block_tracks_output(p),
            Origin::Minecart(id) => {
                self.rule("minecraft:command_block_output", true)
                    && self.cart_ref(id).is_none_or(|c| crate::nbtio::int(&c.nbt, "TrackOutput").is_none_or(|v| v != 0))
            }
        }
    }

    /// System message to the host, whoever the current player is.
    pub(crate) fn to_host(&self, c: Component) {
        if self.lan.as_ref().is_some_and(|l| l.active.is_some()) {
            let _ = self.peer.send(Event::Chat { component: Box::new(c), tag: ChatTag::System });
        } else {
            self.system(c);
        }
    }

    /// System message to the source, outside `send_command_feedback`: to the executing player, otherwise
    /// tracked output of the command block (no broadcast to operators).
    pub fn tell_source(&mut self, src: &Src, msg: Component) {
        self.flush_scores();
        if src.silent {
            return;
        }
        if src.entity == Some(ents::E::Player) {
            self.system(msg);
            return;
        }
        if cmds_ents::trace_message(&msg) {
            return;
        }
        // Source without a player (mob via `execute as`, block…): original output of the source,
        // which stays the player when they are the one who typed the command.
        match src.origin {
            Origin::Player => self.system(msg),
            Origin::Block(p) => self.command_block_output(p, msg, false),
            Origin::Minecart(_) | Origin::Server => {}
        }
    }

    /// Command exception: traced `[E]` even in forked mode, then failure message to the
    /// source except in forked mode (like the game).
    pub fn fail_error(&mut self, src: &Src, msg: Component, forked: bool) {
        cmds_ents::trace_error(&msg);
        if !forked {
            self.fail(src, msg);
        }
    }

    /// Failure message: red.
    pub fn fail(&mut self, src: &Src, msg: Component) {
        self.flush_scores(); // like `ok`
        if cmds_ents::trace_message(&msg) {
            return; // `/debug function`: the trace replaces the output
        }
        if src.silent {
            return;
        }
        let mut c = lit("");
        c.style.color = Some(RED);
        c.extra.push(msg);
        match src.origin {
            Origin::Player => {
                self.chat_out(Event::Chat { component: Box::new(c), tag: ChatTag::System });
            }
            Origin::Block(p) => self.command_block_output(p, c, false),
            Origin::Minecart(_) | Origin::Server => {}
        }
    }

    /// System message to the player (unconditional).
    /// Chat line: sent, or kept if the execution takes place in a parked dimension.
    pub(crate) fn chat_out(&self, e: Event) {
        if self.muted {
            self.cmd.deferred.borrow_mut().push(e);
        } else {
            let _ = self.send(e);
        }
    }

    /// Executes `f` in dimension `dim` (S6) then returns the kept messages.
    pub(crate) fn in_dimension<T>(&mut self, dim: &str, f: impl FnOnce(&mut Server) -> T) -> T {
        if dim == self.dim.name {
            return f(self);
        }
        let r = self.in_world(dim, f);
        if !self.muted {
            let msgs = std::mem::take(&mut *self.cmd.deferred.borrow_mut());
            for e in msgs {
                let _ = self.send(e);
            }
        }
        r
    }

    pub fn system(&self, msg: Component) {
        self.chat_out(Event::Chat { component: Box::new(msg), tag: ChatTag::System });
    }

    /// Tick: pending suggestions, scheduled functions, command blocks.
    pub(crate) fn commands_tick(&mut self) {
        if let Some((tx, text)) = self.cmd.pending_suggest.take() {
            self.answer_suggestions(tx, &text);
        }
        self.fetchprofile_poll(); // G3-F08: profile lookups answered on the server thread
        self.flush_scores();
    }

    /// Server functions, at the head of the tick (before clocks and world), if the game runs
    /// normally: once `#minecraft:load` after a (re)load, then `#minecraft:tick`.
    pub(crate) fn functions_tick(&mut self) {
        if !self.cmd.ticking.runs() {
            return;
        }
        if std::mem::take(&mut self.cmd.post_reload) {
            self.run_tag_functions("minecraft:load");
        }
        self.run_tag_functions("minecraft:tick");
    }

    fn run_tag_functions(&mut self, tag: &str) {
        let Some(members) = self.cmd.function_tags.get(tag).filter(|m| !m.is_empty()) else { return };
        let src = self.server_source();
        if self.cmd.paths.function_cache {
            // The tag key is already complete: its members are shared, not looked up by name again.
            let ids = exec::FunctionIds::Tag(members.clone());
            self.run_function_ids(&src, &ids);
        } else {
            let _ = self.run_functions(&src, &format!("#{tag}"), false);
        }
    }

    /// Command files to write (rules, scores, storage, forced chunks).
    pub(crate) fn commands_save_jobs(&mut self, jobs: &mut Vec<crate::save::Job>) {
        let dir = self.world.join("data").join("minecraft");
        let dat = |name: &str, data: Nbt| {
            let root = Nbt::Compound(vec![
                ("data".into(), data),
                ("DataVersion".into(), Nbt::Int(crate::chunk::DATA_VERSION)),
            ]);
            crate::save::Job::Dat { path: dir.join(name), nbt: crate::nbtio::encode_root(&root), backup: false }
        };
        if self.cmd.rules_dirty {
            self.cmd.rules_dirty = false;
            jobs.push(dat("game_rules.dat", self.rules.clone()));
        }
        if self.cmd.scores.dirty {
            self.cmd.scores.dirty = false;
            jobs.push(dat("scoreboard.dat", self.cmd.scores.to_nbt()));
        }
        if !self.cmd.storage_dirty.is_empty() {
            let dirty = std::mem::take(&mut self.cmd.storage_dirty);
            jobs.extend(cmds_data::storage_jobs(&self.world, &self.cmd.storage, &dirty));
        }
        if self.cmd.forced_dirty {
            self.cmd.forced_dirty = false;
            jobs.push(cmds_world::forced_job(&self.dim.dir, &self.cmd.forced));
        }
        if self.cmd.bars_dirty {
            self.cmd.bars_dirty = false;
            jobs.push(cmds_more::bars_job(&self.world, &self.cmd.bars));
        }
        if self.cmd.border_dirty {
            self.cmd.border_dirty = false;
            let root = Nbt::Compound(vec![
                ("data".into(), self.cmd.border.to_nbt()),
                ("DataVersion".into(), Nbt::Int(crate::chunk::DATA_VERSION)),
            ]);
            let path = self.dim.dir.join("data").join("minecraft").join("world_border.dat");
            jobs.push(crate::save::Job::Dat { path, nbt: crate::nbtio::encode_root(&root), backup: false });
        }
    }
}
