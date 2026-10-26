//! Commands of the integrated server (mission S5, `docs/specs/commands.md` § 6): tree sent according to
//! the permission level, parsing of the input (same tree, `rc-commands`), error messages
//! of the game, execution (`execute` and its subcommands, vanilla commands), suggestions computed
//! by the server, command blocks.
//!
//! The tree comes from the extractor (`tools/extractor/CommandsExtractor.java`): built by the
//! game itself ("integrated" selection) then filtered for each level, in packet format.

// Failures carry a component (rare, off the hot path).
#![allow(clippy::result_large_err)]

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
pub mod cmds_scores;
pub mod cmds_world;
pub mod ents;
pub mod exec;
pub mod hash_order;
pub mod itempred;
pub mod scores;
pub mod select;
pub mod snbt;
pub mod src;
pub mod suggest;
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
    pub storage_dirty: bool,
    /// Force-loaded chunks (`chunk_tickets.dat`, `forceload`).
    pub forced: BTreeSet<(i32, i32)>,
    pub forced_dirty: bool,
    /// Changed rules (rewrite `game_rules.dat`).
    pub rules_dirty: bool,
    /// Functions of the data packs: identifier → lines; function tags.
    pub functions: BTreeMap<String, Vec<String>>,
    pub function_tags: BTreeMap<String, Vec<String>>,
    /// Data (re)loaded: the `#minecraft:load` tag goes out at the next tick.
    pub post_reload: bool,
    /// Server-side registries not synchronized (structures, advancements, templates…).
    pub server_regs: BTreeMap<String, Vec<String>>,
    /// Most recent suggestions request (processed at the next tick).
    pub pending_suggest: Option<(i32, String)>,
    /// Scheduled functions (`schedule`): game tick → (function, tag).
    pub scheduled: Vec<(i64, String, bool)>,
    /// Execution depth (nested functions, safeguard).
    pub depth: u32,
    /// Return in progress (`return`) in a function: (success, value).
    pub returned: Option<(bool, i32)>,
    /// Execution context (command quota, fork limit, return mode).
    pub flow: exec::Flow,
    /// Successful executions of the current command (multiple sources counted).
    pub successes: i32,
    /// Stopwatches and waypoints (world state, shared by all players; the player's
    /// camera stays in their advancements).
    pub s9: cmds_s9a::State,
    /// Game speed and freeze (`tick`).
    pub ticking: crate::tickrate::TickRate,
    /// Effects of non-player entities (network identifier); the player: `effects.rs` (S7).
    pub mob_effects: BTreeMap<i32, Vec<ents::Effect>>,
    /// Paused clocks, clock rates (`time pause`, `time rate`).
    pub clock_paused: BTreeSet<String>,
    pub clock_rate: BTreeMap<String, f32>,
    pub clock_acc: BTreeMap<String, f32>,
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
            warn_time: f("warning_time", 15.0) as i32,
            warn_blocks: f("warning_blocks", 5.0) as i32,
        }
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

fn load_levels(path: &std::path::Path) -> Vec<Level> {
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
            Ok(c) => {
                let tree = Tree::new(&c);
                out.push(Level { packet: c, tree });
            }
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

impl Cmds {
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
        for (dir, reg) in [
            ("worldgen/structure", "minecraft:worldgen/structure"),
            ("worldgen/configured_feature", "minecraft:worldgen/configured_feature"),
            ("worldgen/template_pool", "minecraft:worldgen/template_pool"),
            ("worldgen/structure_set", "minecraft:worldgen/structure_set"),
            ("advancement", "minecraft:advancement"),
            ("loot_table", "minecraft:loot_table"),
            ("predicate", "minecraft:predicate"),
            ("item_modifier", "minecraft:item_modifier"),
            ("recipe", "minecraft:recipe"),
            ("structure", "minecraft:structure"),
        ] {
            let mut v: Vec<String> = jar
                .files
                .keys()
                .filter_map(|k| {
                    let rest = k.strip_prefix("data/")?;
                    let (ns, path) = rest.split_once('/')?;
                    let p = path.strip_prefix(dir)?.strip_prefix('/')?;
                    let p = p.strip_suffix(".json").or_else(|| p.strip_suffix(".nbt"))?;
                    Some(format!("{ns}:{p}"))
                })
                .collect();
            v.sort();
            v.dedup();
            if !v.is_empty() {
                server_regs.insert(reg.to_owned(), v.clone());
                // Loot tables and predicates are also read inline: known registry.
                d.set_registry(reg, v);
            }
        }
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
        let (functions, function_tags) = exec::load_functions(jar, &levels, &d);
        let mut c = Cmds {
            levels,
            data: d,
            jar_tags,
            builtin_packs: Vec::new(),
            op,
            scores: scores::Board::read(world),
            storage: cmds_data::read_storage(world),
            storage_dirty: false,
            forced: cmds_world::read_forced(dim_dir),
            forced_dirty: false,
            rules_dirty: false,
            functions,
            function_tags,
            post_reload: true,
            server_regs,
            pending_suggest: None,
            scheduled: Vec::new(),
            depth: 0,
            returned: None,
            flow: exec::Flow::default(),
            successes: 0,
            ticking: crate::tickrate::TickRate::default(),
            s9: Default::default(),
            mob_effects: BTreeMap::new(),
            clock_paused: BTreeSet::new(),
            clock_rate: BTreeMap::new(),
            clock_acc: BTreeMap::new(),
            time_markers: load_markers(jar),
            border: Border::read(dim_dir),
            border_dirty: false,
            locgen: None,
            locate: cmds_locate::LocateData::read(jar),
            bars: cmds_more::read_bars(world),
            bars_dirty: false,
            deferred: std::cell::RefCell::new(Vec::new()),
        };
        c.data.teams = c.scores.teams.iter().map(|t| t.name.clone()).collect();
        c.data.players = vec![player.to_owned()];
        c
    }

    pub fn level(&self, op: u8) -> Option<&Level> {
        self.levels.get(usize::from(op.min(4)))
    }
}

/// Parse segment: nodes read up to a redirect, readable path.
#[derive(Debug, Clone)]
pub struct Seg {
    /// Path: node names (arguments between `<>`), preceded by the name of the root node of the
    /// segment if it is not the root (`execute` subcommands, aliases).
    pub path: Vec<String>,
    /// Arguments: name → range in the input.
    pub args: Vec<(String, (usize, usize))>,
    /// Last node read (possible redirect).
    pub last: NodeId,
}

impl Seg {
    pub fn path_str(&self) -> String {
        self.path.join(" ")
    }
}

/// Parsed and split input.
pub struct Parsed {
    pub input: Vec<char>,
    pub segs: Vec<Seg>,
}

impl Parsed {
    pub fn arg(&self, seg: &Seg, name: &str) -> Option<String> {
        seg.args.iter().find(|(n, _)| n == name).map(|(_, (a, b))| self.input[*a..*b].iter().collect())
    }

    pub fn range(&self, seg: &Seg, name: &str) -> Option<(usize, usize)> {
        seg.args.iter().find(|(n, _)| n == name).map(|(_, r)| *r)
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

/// Parse error text as the game writes it in its logs and macro messages
/// (error message: message, then "at position N: …<--[HERE]"); `None` if the command is
/// valid.
pub fn syntax_message(lvl: &Level, data: &rc_commands::Data, text: &str, op: u8) -> Option<String> {
    let input: Vec<char> = text.chars().collect();
    let src = rc_commands::parse::Source { restricted: true, selectors: op >= 2 };
    let p = rc_commands::parse(&lvl.tree, &input, data, src);
    let e = parse_error(&p, input.len())?;
    let msg = cmds_ents::plain(&tr(e.key, e.args.iter().map(|a| text::A::S(a.clone())).collect()));
    let Some(c) = e.cursor else { return Some(msg) };
    let c = c.min(input.len());
    let dots = if c > 10 { "..." } else { "" };
    let before: String = input[c.saturating_sub(10)..c].iter().collect();
    Some(format!("{msg} at position {c}: {dots}{before}<--[HERE]"))
}

pub fn analyse(lvl: &Level, data: &rc_commands::Data, text: &str, op: u8) -> Result<Parsed, Vec<Component>> {
    let input: Vec<char> = text.chars().collect();
    let src = rc_commands::parse::Source { restricted: true, selectors: op >= 2 };
    let p = rc_commands::parse(&lvl.tree, &input, data, src);
    let start = usize::from(input.first() == Some(&'/'));
    let body: String = input[start..].iter().collect();
    let shift = |c: usize| c.saturating_sub(start);
    if let Some(e) = parse_error(&p, input.len()) {
        let mut out = vec![tr(e.key, e.args.iter().map(|a| text::A::S(a.clone())).collect())];
        if let Some(c) = e.cursor {
            let c = shift(c).min(body.chars().count());
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
        return Err(out);
    }
    let mut segs = Vec::new();
    let mut ctx = Some(&p.ctx);
    while let Some(c) = ctx {
        let mut path = Vec::new();
        if c.root != lvl.tree.root {
            path.push(lvl.tree.node(c.root).name.clone());
        }
        let mut args = Vec::new();
        let mut last = c.root;
        for &(id, r) in &c.nodes {
            let n = lvl.tree.node(id);
            if n.kind == Kind::Argument {
                path.push(format!("<{}>", n.name));
                args.push((n.name.clone(), r));
            } else {
                path.push(n.name.clone());
            }
            last = id;
        }
        segs.push(Seg { path, args, last });
        ctx = c.child.as_deref();
    }
    Ok(Parsed { input, segs })
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
        self.chat_out(Event::Chat { component: c, tag: ChatTag::NotSecure });
    }

    /// Command typed by the player.
    pub(crate) fn player_command(&mut self, text: &str) {
        let src = self.player_source();
        self.perform(src, text);
    }

    /// Executes a complete command for a source; returns (success, result).
    pub fn perform(&mut self, src: Src, text: &str) -> (bool, i32) {
        // Texts rendered on the server side (conversion to a plain string): server language, en_us (S7).
        cmds_ents::SERVER_LANG.get_or_init(|| crate::stations::en_us(&self.life.items));
        if self.exhausted() {
            return (false, 0);
        }
        let op = src.level;
        let Some(lvl) = self.cmd.level(op) else {
            self.fail(&src, tr("command.unknown.command", Vec::new()));
            return (false, 0);
        };
        let text = text.strip_prefix('/').unwrap_or(text);
        let parsed = match analyse(lvl, &self.cmd.data, text, op) {
            Ok(p) => p,
            Err(msgs) => {
                for m in msgs {
                    self.fail(&src, m);
                }
                return (false, 0);
            }
        };
        self.in_context(|sv| sv.run_parsed(src, &parsed))
    }

    /// Success message: to the player if the rule allows it, to the output
    /// of a command block otherwise.
    pub fn ok(&mut self, src: &Src, msg: Component) {
        // Scoreboard packets first: the game broadcasts them during the operation, before the return.
        self.flush_scores();
        if cmds_ents::trace_message(&msg) {
            return; // `/debug function`: the trace replaces the output
        }
        if src.silent {
            return;
        }
        match src.origin {
            Origin::Player => {
                if self.rule("minecraft:send_command_feedback", true) {
                    self.chat_out(Event::Chat { component: msg, tag: ChatTag::System });
                }
            }
            Origin::Block(p) => self.cb_output(p, msg, true),
            Origin::Minecart(_) | Origin::Server => {}
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
            Origin::Block(p) => self.cb_output(p, msg, false),
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
                self.chat_out(Event::Chat { component: c, tag: ChatTag::System });
            }
            Origin::Block(p) => self.cb_output(p, c, false),
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
        self.chat_out(Event::Chat { component: msg, tag: ChatTag::System });
    }

    /// Tick: pending suggestions, scheduled functions, command blocks.
    pub(crate) fn commands_tick(&mut self) {
        if let Some((tx, text)) = self.cmd.pending_suggest.take() {
            self.answer_suggestions(tx, &text);
        }
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
        if self.cmd.function_tags.get(tag).is_none_or(Vec::is_empty) {
            return;
        }
        let src = self.server_source();
        let _ = self.run_functions(&src, &format!("#{tag}"), false);
    }

    /// Speed of a clock sent to the client (0 when paused or with no time advance).
    pub(crate) fn clock_net_rate(&self, clock: &str, base: f32) -> f32 {
        if base == 0.0 || self.cmd.clock_paused.contains(clock) {
            return 0.0;
        }
        self.cmd.clock_rate.get(clock).copied().unwrap_or(1.0)
    }

    /// Advance of the clocks (pause and speed of `time`); replaces the fixed one-tick advance.
    pub(crate) fn advance_clocks(&mut self) {
        for c in &mut self.clocks {
            if self.cmd.clock_paused.contains(&c.0) {
                continue;
            }
            let rate = self.cmd.clock_rate.get(&c.0).copied().unwrap_or(1.0);
            if rate == 1.0 {
                c.1 += 1;
            } else {
                let acc = self.cmd.clock_acc.entry(c.0.clone()).or_insert(0.0);
                *acc += rate;
                let whole = acc.floor();
                *acc -= whole;
                c.1 += whole as i64;
            }
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
        if self.cmd.storage_dirty {
            self.cmd.storage_dirty = false;
            jobs.extend(cmds_data::storage_jobs(&self.world, &self.cmd.storage));
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
