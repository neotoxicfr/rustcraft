//! Execution: chain of segments (`execute` subcommands then the final command), multiple
//! sources (`as`, `at`…), conditions, result storage (`store`), data-pack
//! functions (`function`, `schedule`, `return`), dispatch to the commands.

use std::collections::BTreeMap;
use rc_proto::nbt::Nbt;

use super::ents::E;
use super::src::{Fail, Src, Store};
use super::{Parsed, R, Seg, args, text};
use crate::registries::Jar;
use crate::server::Server;
use crate::tr;

/// Functions and function tags of the stack of chosen packs
/// (`data/<namespace>/function/**.mcfunction`, `data/<namespace>/tags/function`, `packs.rs`). Each
/// function is compiled like the game does at load time (`compile_function`): one invalid line makes
/// the whole of it refused (server log). The tags are then resolved against the
/// loaded functions: nested tags flattened, refused as a whole if a required entry
/// is missing or loops (`packs::resolve_tags`).
pub(crate) fn load_functions(
    jar: &Jar,
    levels: &[super::Level],
    cmd_data: &rc_commands::Data,
) -> (BTreeMap<String, Vec<String>>, BTreeMap<String, Vec<String>>) {
    let mut f = BTreeMap::new();
    for (name, bytes) in &jar.files {
        let Some(rest) = name.strip_prefix("data/") else { continue };
        let Some((ns, path)) = rest.split_once('/') else { continue };
        let Some(p) = path.strip_prefix("function/").and_then(|p| p.strip_suffix(".mcfunction")) else { continue };
        let id = format!("{ns}:{p}");
        let Ok(text) = std::str::from_utf8(bytes) else {
            eprintln!("Failed to load function {id}: not UTF-8");
            continue;
        };
        let raw: Vec<&str> = text.lines().collect();
        match compile_function(&raw, levels.get(2), cmd_data) {
            Ok(lines) => {
                f.insert(id, lines);
            }
            Err(e) => eprintln!("Failed to load function {id}: {e}"),
        }
    }
    let raw = crate::packs::raw_tags(jar, "function");
    let map: BTreeMap<String, &[serde_json::Value]> =
        raw.iter().map(|(n, v)| (n.clone(), v.as_array().map_or(&[][..], Vec::as_slice))).collect();
    let tags = crate::packs::resolve_tags(&map, &|id| f.contains_key(id).then(|| id.to_owned()));
    (f, tags)
}

/// Tags per registry (`minecraft:block` → name without `#` → members), like `Data::tags`.
pub type TagSets = hashbrown::HashMap<String, hashbrown::HashMap<String, Vec<String>>>;

/// Adds to the parsing tags those of the registries that `base` does not cover (tags of the
/// synced and built-in registries, already resolved on the pack stack), read from the stack
/// (`data/<ns>/tags/<registry>/…`), like the game which loads them before the functions: without them,
/// a line citing `#pack:tag` would make the whole function refused. The members of these registries
/// are not known here: only references between tags are checked.
pub(crate) fn apply_pack_tags(jar: &Jar, data: &mut rc_commands::Data, base: &TagSets) {
    // registry → tag → stacked values
    let mut found: BTreeMap<String, Vec<(String, serde_json::Value)>> = BTreeMap::new();
    for (name, bytes) in &jar.files {
        let Some(rest) = name.strip_prefix("data/") else { continue };
        let Some((ns, path)) = rest.split_once('/') else { continue };
        let Some(full) = path.strip_prefix("tags/").and_then(|p| p.strip_suffix(".json")) else { continue };
        // Registry folder: everything before the last level is not known in advance
        // (`worldgen/biome`); the known registries are tested.
        let Some((reg, tag)) = split_tag_path(data, base, full) else { continue };
        if reg == "minecraft:function" || base.contains_key(&reg) {
            continue;
        }
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(bytes) else { continue };
        let values = v.get("values").cloned().unwrap_or(serde_json::Value::Array(Vec::new()));
        found.entry(reg).or_default().push((format!("{ns}:{tag}"), values));
    }
    for (reg, tags) in &found {
        let map: BTreeMap<String, &[serde_json::Value]> =
            tags.iter().map(|(n, v)| (n.clone(), v.as_array().map_or(&[][..], Vec::as_slice))).collect();
        let resolved = crate::packs::resolve_tags(&map, &|id| Some(id.to_owned()));
        data.set_tags(reg, resolved.into_iter().collect());
    }
}

/// Data registries of the `worldgen` folder neither synced nor known to the parser in advance:
/// their tags (`tags/worldgen/feature/…`) are filed under their full name.
const WORLDGEN_DATA_REGISTRIES: [&str; 15] = [
    "worldgen/carver",
    "worldgen/density_function",
    "worldgen/feature",
    "worldgen/flat_level_generator_preset",
    "worldgen/material_condition",
    "worldgen/material_rule",
    "worldgen/multi_noise_biome_source_parameter_list",
    "worldgen/noise",
    "worldgen/noise_settings",
    "worldgen/placed_feature",
    "worldgen/processor_list",
    "worldgen/structure",
    "worldgen/structure_set",
    "worldgen/template_pool",
    "worldgen/world_preset",
];

/// Splits `block/foo/bar` into (registry `minecraft:block`, tag `foo/bar`): the registry is
/// the longest folder prefix that is one (`worldgen/biome`), otherwise the first folder
/// (data registry not known in advance, `villager_trade`), never `worldgen` alone, which is
/// not a registry.
fn split_tag_path(data: &rc_commands::Data, base: &TagSets, full: &str) -> Option<(String, String)> {
    let parts: Vec<&str> = full.split('/').collect();
    for n in (1..parts.len()).rev() {
        let dir = parts[..n].join("/");
        let reg = format!("minecraft:{dir}");
        let known = data.has_registry(&reg) || base.contains_key(&reg) || WORLDGEN_DATA_REGISTRIES.contains(&dir.as_str());
        if known || (n == 1 && dir != "worldgen") {
            return Some((reg, parts[n..].join("/")));
        }
    }
    None
}

/// Failure without a message (`return fail`, function call without a result).
fn is_silent(c: &rc_text::Component) -> bool {
    matches!(&c.content, rc_text::Content::Text(t) if t.is_empty()) && c.extra.is_empty()
}

/// The segment is a "fork" subcommand (multiple sources, errors silenced).
fn is_fork(path: &str) -> bool {
    path.starts_with("execute as ")
        || path.starts_with("execute at ")
        || path.starts_with("execute positioned as ")
        || path.starts_with("execute rotated as ")
        || path.starts_with("execute facing entity ")
        || path.starts_with("execute if ")
        || path.starts_with("execute unless ")
        || path.starts_with("execute on ")
}

/// Execution context of a top-level command (input, command block, scheduled
/// function…): like the game, a command quota (`max_command_sequence_length`) shared
/// by everything it triggers (nested functions included) and a source limit
/// (`max_command_forks`).
#[derive(Debug, Clone, Default)]
pub struct Flow {
    pub active: bool,
    /// Remaining cost: one subcommand step, one final command per source, one function
    /// call. At 0, the whole execution stops (server log, no message).
    pub quota: i64,
    pub limit: i64,
    pub fork_limit: i64,
    /// The running final command follows a `return run` (a called function returns its
    /// result to the calling function).
    pub ret_mode: bool,
    /// The final command returned no result (function without `return`): neither storage
    /// (`store`) nor counted success.
    pub no_callback: bool,
    /// Function call depth: stack safeguard only (the game has just a queue).
    pub depth: u32,
}

/// Depth of nested calls beyond which execution stops as with an exhausted quota
/// (integrated server thread stack; the game, without recursion, only has the quota).
pub const MAX_NESTING: u32 = 65536;

#[cfg(test)]
thread_local! {
    /// Trials: (depth, address of a local variable) at each function call.
    pub(crate) static STACK_PROBE: std::cell::RefCell<Vec<(u32, usize)>> = const { std::cell::RefCell::new(Vec::new()) };
}

impl Server {
    /// Runs `f` in the current execution context, or opens one (top level).
    pub(crate) fn in_context<T>(&mut self, f: impl FnOnce(&mut Self) -> T) -> T {
        if self.cmd.flow.active {
            return f(self);
        }
        let int = |sv: &Self, k: &str| sv.rules.get(k).and_then(Nbt::as_i64).unwrap_or(65536);
        let limit = int(self, "minecraft:max_command_sequence_length").max(1);
        let forks = int(self, "minecraft:max_command_forks");
        self.cmd.flow = Flow { active: true, quota: limit, limit, fork_limit: forks, ..Flow::default() };
        let r = f(self);
        if self.cmd.flow.quota <= 0 {
            eprintln!("Command execution stopped due to limit (executed {} commands)", self.cmd.flow.limit);
        }
        self.cmd.flow = Flow::default();
        self.cmd.returned = None;
        r
    }

    /// Quota exhausted: nothing runs any more in this context.
    pub(crate) fn exhausted(&self) -> bool {
        self.cmd.flow.active && self.cmd.flow.quota <= 0
    }

    /// Runs a parsed input: subcommands then the final command for each source.
    pub fn run_parsed(&mut self, src: Src, p: &Parsed) -> (bool, i32) {
        let n = p.segs.len();
        let origin = src.clone();
        let mut sources = vec![src];
        let mut forked = false;
        // `return run`: the running function stops from this step, the rest returns its
        // result (failure if no source remains).
        let mut ret = false;
        for seg in &p.segs[..n - 1] {
            let path = seg.path_str();
            if path == "return run" {
                ret = true;
                continue;
            }
            // Pure redirections (alias, `execute run`): no step, no cost.
            if matches!(path.as_str(), "tp" | "xp" | "tell" | "w" | "tm" | "execute run") {
                continue;
            }
            forked |= is_fork(&path);
            if !path.ends_with(" function <name>") {
                self.cmd.flow.quota -= 1;
            }
            let mut next = Vec::new();
            for s in sources {
                match self.modifier(&s, p, seg, &path) {
                    Ok(v) => {
                        if self.cmd.flow.active && (next.len() + v.len()) as i64 >= self.cmd.flow.fork_limit {
                            let limit = self.cmd.flow.fork_limit;
                            self.fail_error(&origin, tr!("command.forkLimit", limit), forked);
                            if ret {
                                self.cmd.returned = Some((false, 0));
                            }
                            return (false, 0);
                        }
                        next.extend(v);
                    }
                    Err(f) => {
                        self.fail_error(&s, f.0, forked);
                        if !forked {
                            // `return run` has already abandoned the function's frame: it
                            // stops here, without running its following lines.
                            if ret {
                                self.cmd.returned = Some((false, 0));
                            }
                            return (false, 0);
                        }
                    }
                }
            }
            sources = next;
            if sources.is_empty() {
                if ret {
                    self.cmd.returned = Some((false, 0));
                }
                return (false, 0);
            }
        }
        let last = &p.segs[n - 1];
        let path = last.path_str();
        // `function` and `return` run directly (no queue entry, no cost).
        let custom = matches!(last.path.first().map(String::as_str), Some("function" | "return"));
        if ret && !custom {
            // Return mode: only the first source runs the command.
            sources.truncate(1);
        }
        let mut any = false;
        let mut total = 0i32;
        for s in sources {
            if !custom {
                if self.exhausted() {
                    break;
                }
                self.cmd.flow.quota -= 1;
            }
            self.cmd.flow.ret_mode = ret;
            self.cmd.flow.no_callback = false;
            let r = self.command(&s, p, last, &path);
            self.cmd.flow.ret_mode = false;
            let no_callback = std::mem::take(&mut self.cmd.flow.no_callback);
            let (ok, v) = match r {
                Ok(v) => (true, v),
                Err(f) => {
                    if !is_silent(&f.0) {
                        self.fail_error(&s, f.0, forked);
                    }
                    (false, 0)
                }
            };
            if ret && !custom {
                self.cmd.returned = Some((ok, v));
            }
            // `return run function`: the first source that returns a result ends the
            // calling function, the following ones never run.
            let stop = ret && custom && self.cmd.returned.is_some();
            if no_callback {
                continue;
            }
            any |= ok;
            self.cmd.successes += i32::from(ok);
            total = total.wrapping_add(v);
            self.apply_stores(&s, ok, v);
            if stop {
                break;
            }
        }
        (any, total)
    }

    fn apply_stores(&mut self, s: &Src, ok: bool, v: i32) {
        for (result, st) in &s.stores {
            let value = if *result { v } else { i32::from(ok) };
            match st {
                Store::Score { targets, objective } => {
                    for t in targets {
                        self.score_set(t, objective, value);
                    }
                }
                Store::Bossbar { id, max } => {
                    self.cmd.bars_dirty = true;
                    let shown = self.cmd.bars.iter().find(|b| b.id == *id).map(|b| b.shown_to(self.uuid));
                    if let Some(b) = self.cmd.bars.iter_mut().find(|b| b.id == *id) {
                        if *max {
                            b.max = value;
                        } else {
                            b.value = value;
                        }
                        let progress = if b.max <= 0 { 0.0 } else { (b.value as f32 / b.max as f32).clamp(0.0, 1.0) };
                        if shown == Some(true) {
                            let _ = self.peer.send(rc_client::Event::Boss(rc_proto_v777::play::BossEvent {
                                id: b.uuid,
                                op: rc_proto_v777::play::BossOp::Progress(progress),
                            }));
                        }
                    }
                }
                Store::Block { pos, path, kind, scale } => {
                    if let Some(mut n) = self.data_of_block(*pos) {
                        let tag = super::cmds_data::numeric(kind, f64::from(value) * scale);
                        if super::cmds_data::path_set(&mut n, path, tag).is_ok() {
                            self.set_block_data(*pos, n);
                        }
                    }
                }
                Store::Entity { ent, path, kind, scale } => {
                    if *ent == E::Player {
                        continue;
                    }
                    let mut n = self.ent_nbt(*ent);
                    let tag = super::cmds_data::numeric(kind, f64::from(value) * scale);
                    if super::cmds_data::path_set(&mut n, path, tag).is_ok() {
                        self.ent_set_nbt(*ent, &n);
                    }
                }
                Store::Storage { id, path, kind, scale } => {
                    let mut n = self.cmd.storage.get(id).cloned().unwrap_or(Nbt::Compound(Vec::new()));
                    let tag = super::cmds_data::numeric(kind, f64::from(value) * scale);
                    if super::cmds_data::path_set(&mut n, path, tag).is_ok() {
                        self.cmd.storage.insert(id.clone(), n);
                        self.cmd.storage_dirty = true;
                    }
                }
            }
        }
        self.flush_scores();
    }

    /// Subcommand (redirection): following sources.
    fn modifier(&mut self, s: &Src, p: &Parsed, seg: &Seg, path: &str) -> Result<Vec<Src>, Fail> {
        let a = |n: &str| p.arg(seg, n).unwrap_or_default();
        let one = |x: Src| Ok(vec![x]);
        // Alias (`tp`, `xp`, `tell`, `w`, `tm`) and `execute run`: source unchanged.
        if matches!(path, "tp" | "xp" | "tell" | "w" | "tm" | "execute run") {
            return one(s.clone());
        }
        let p_ = path.strip_prefix("execute ").unwrap_or(path);
        let w: Vec<&str> = p_.split(' ').collect();
        match w.as_slice() {
            ["as", _] => {
                let ents = self.arg_entities_opt(s, &a("targets"));
                Ok(ents.into_iter().map(|e| self.with_entity(s, e)).collect())
            }
            ["at", _] => {
                let ents = self.arg_entities_opt(s, &a("targets"));
                Ok(ents
                    .into_iter()
                    .map(|e| {
                        let mut x = s.clone();
                        x.pos = self.e_pos(e);
                        let [y, pi] = self.ent_rot(e);
                        x.rot = [pi, y];
                        x
                    })
                    .collect())
            }
            ["positioned", "<pos>"] => {
                let mut x = s.clone();
                x.pos = args::vec3(self, s, &a("pos"));
                x.eyes = false;
                one(x)
            }
            ["positioned", "as", _] => {
                let ents = self.arg_entities_opt(s, &a("targets"));
                Ok(ents
                    .into_iter()
                    .map(|e| {
                        let mut x = s.clone();
                        x.pos = self.e_pos(e);
                        x
                    })
                    .collect())
            }
            ["positioned", "over", _] => {
                let (bx, bz) = (s.pos[0].floor() as i32, s.pos[2].floor() as i32);
                if !self.is_loaded([bx, 0, bz]) {
                    return Err(Fail(tr!("argument.pos.unloaded")));
                }
                let h = self.surface_height(&a("heightmap"), bx, bz);
                let mut x = s.clone();
                x.pos[1] = f64::from(h);
                one(x)
            }
            ["rotated", "<rot>"] => {
                let mut x = s.clone();
                let [y, pi] = args::rotation(s, &a("rot"));
                x.rot = [pi, y];
                one(x)
            }
            ["rotated", "as", _] => {
                let ents = self.arg_entities_opt(s, &a("targets"));
                Ok(ents
                    .into_iter()
                    .map(|e| {
                        let mut x = s.clone();
                        let [y, pi] = self.ent_rot(e);
                        x.rot = [pi, y];
                        x
                    })
                    .collect())
            }
            ["facing", "entity", _, _] => {
                let eyes = a("anchor") == "eyes";
                let ents = self.arg_entities_opt(s, &a("targets"));
                Ok(ents
                    .into_iter()
                    .map(|e| {
                        let mut t = self.e_pos(e);
                        if eyes {
                            t[1] += self.ent_eye_height(e);
                        }
                        self.facing(s, t)
                    })
                    .collect())
            }
            ["facing", "<pos>"] => {
                let t = args::vec3(self, s, &a("pos"));
                one(self.facing(s, t))
            }
            ["align", _] => {
                let mut x = s.clone();
                for c in a("axes").chars() {
                    let k = match c {
                        'x' => 0,
                        'y' => 1,
                        _ => 2,
                    };
                    x.pos[k] = x.pos[k].floor();
                }
                one(x)
            }
            ["anchored", _] => {
                let mut x = s.clone();
                x.eyes = a("anchor") == "eyes";
                one(x)
            }
            ["in", _] => {
                let d = args::id(&a("dimension"));
                if !self.world_dimensions().contains(&d) {
                    return Err(Fail(tr!("argument.dimension.invalid", d)));
                }
                // Interface S6: without parked worlds, only the open dimension exists; the
                // source changes dimension (coordinates unchanged, like the game).
                let mut x = s.clone();
                x.dim = d;
                one(x)
            }
            ["summon", _] => {
                let e = self.summon_entity(s, &args::id(&a("entity")), s.pos, None)?;
                one(self.with_entity(s, e))
            }
            ["on", rel] => {
                let Some(me) = s.entity else { return Ok(Vec::new()) };
                let found: Vec<E> = match *rel {
                    "attacker" => self.relation_attacker(me).into_iter().collect(),
                    "target" => self.relation_target(me).into_iter().collect(),
                    "owner" => self.relation_owner(me).into_iter().collect(),
                    "vehicle" => self.vehicle_of(me).into_iter().collect(),
                    "passengers" => self.relation_passengers(me),
                    "controller" => self.relation_controller(me).into_iter().collect(),
                    "origin" => self.relation_origin(me).into_iter().collect(),
                    // Leashes not simulated by singleplayer: no held entity.
                    _ => Vec::new(),
                };
                Ok(found.into_iter().map(|e| self.with_entity(s, e)).collect())
            }
            ["store", kind @ ("result" | "success"), rest @ ..] => {
                let result = *kind == "result";
                let st = match rest {
                    ["score", _, _] => {
                        let targets = self.arg_score_holders(s, &a("targets"))?;
                        let objective = a("objective");
                        if self.cmd.scores.objective(&objective).is_none() {
                            return Err(Fail(tr!("arguments.objective.notFound", objective)));
                        }
                        Store::Score { targets, objective }
                    }
                    // Targets checked as soon as redirected, like the game: unknown bar or block
                    // without a block entity stop the command.
                    ["bossbar", _, which] => {
                        let id = args::id(&a("id"));
                        self.bar(&id)?;
                        Store::Bossbar { id, max: *which == "max" }
                    }
                    ["block", _, _, ty, _] => {
                        let pos = self.loaded_pos(s, &a("target"))?;
                        if self.be_get(pos).is_none() {
                            return Err(Fail(tr!("commands.data.block.invalid")));
                        }
                        Store::Block { pos, path: a("path"), kind: (*ty).to_owned(), scale: a("scale").parse().unwrap_or(1.0) }
                    }
                    ["entity", _, _, ty, _] => Store::Entity {
                        ent: self.arg_entity(s, &a("target"), false)?,
                        path: a("path"),
                        kind: (*ty).to_owned(),
                        scale: a("scale").parse().unwrap_or(1.0),
                    },
                    ["storage", _, _, ty, _] => Store::Storage {
                        id: args::id(&a("target")),
                        path: a("path"),
                        kind: (*ty).to_owned(),
                        scale: a("scale").parse().unwrap_or(1.0),
                    },
                    _ => return Err(Fail(tr!("command.failed"))),
                };
                let mut x = s.clone();
                x.stores.push((result, st));
                one(x)
            }
            [cond @ ("if" | "unless"), rest @ ..] => {
                let expected = *cond == "if";
                let n = self.condition(s, p, seg, rest)?;
                Ok(if (n > 0) == expected { vec![s.clone()] } else { Vec::new() })
            }
            _ => Err(Fail(tr!("command.unknown.command"))),
        }
    }

    /// `execute on passengers`: direct passengers, in order.
    fn relation_passengers(&self, e: E) -> Vec<E> {
        use crate::mobs::mob::Ent;
        // G5-F06: passengers of a minecart.
        if let E::Other(i) = e
            && let Some(c) = self.cart_ref(i)
        {
            return c
                .passengers
                .iter()
                .map(|p| match *p {
                    Ent::Player => E::Player,
                    Ent::Mob(x) => E::Mob(x),
                })
                .collect();
        }
        let E::Mob(i) = e else { return Vec::new() };
        let Some(m) = self.mobs.mob_ref_any(i) else { return Vec::new() };
        m.mount
            .passengers
            .iter()
            .map(|p| match *p {
                Ent::Player => E::Player,
                Ent::Mob(x) => E::Mob(x),
            })
            .collect()
    }

    /// `execute on controller`: the passenger that steers (player on a saddled mount, mob
    /// leading a mount without its AI turned off).
    fn relation_controller(&self, e: E) -> Option<E> {
        let E::Mob(i) = e else { return None };
        let m = self.mobs.mob_ref_any(i)?;
        if Self::player_controls(m) {
            return Some(E::Player);
        }
        Self::mob_controller(m).map(E::Mob)
    }

    /// `execute on origin`: thrower of a thrown item, summoner of a vex, registered
    /// owner (`Owner`) of a projectile, a cloud or a TNT.
    fn relation_origin(&self, e: E) -> Option<E> {
        use crate::mobs::mob::Ent;
        match e {
            E::Item(i) => match self.life.drops.iter().find(|d| d.id == i)?.thrower? {
                Ent::Player => Some(E::Player),
                Ent::Mob(x) => Some(E::Mob(x)).filter(|m| self.ent_exists(*m)),
            },
            E::Mob(i) => {
                let m = self.mobs.mob_ref_any(i)?;
                self.ent_by_uuid(m.ill.owner?)
            }
            E::Other(_) => {
                let n = self.ent_nbt(e);
                self.ent_by_uuid(crate::nbtio::uuid(n.get("Owner"))?)
            }
            _ => None,
        }
    }

    pub(crate) fn facing(&self, s: &Src, t: [f64; 3]) -> Src {
        let from = s.anchor(self);
        let (dx, dy, dz) = (t[0] - from[0], t[1] - from[1], t[2] - from[2]);
        let sd = (dx * dx + dz * dz).sqrt();
        // Like the game: fast arctangent (table), π rounded to a 32-bit float, removal of the
        // 90° done in 32 bits.
        use crate::mobs::mth::{atan2, wrap};
        let pi = f64::from(std::f32::consts::PI);
        let pitch = wrap((-(atan2(dy, sd) * 180.0 / pi)) as f32);
        let yaw = wrap((atan2(dz, dx) * 180.0 / pi) as f32 - 90.0);
        let mut x = s.clone();
        x.rot = [pitch, yaw];
        x
    }

    fn surface_height(&self, map: &str, x: i32, z: i32) -> i32 {
        let kind = match map {
            "world_surface" => 1,
            "ocean_floor" => 3,
            "motion_blocking" => 4,
            _ => 5,
        };
        self.surface_y(x, z, kind)
    }

    /// Condition of `execute if|unless`: number of matches (0 = false).
    fn condition(&mut self, s: &Src, p: &Parsed, seg: &Seg, rest: &[&str]) -> Result<i32, Fail> {
        let a = |n: &str| p.arg(seg, n).unwrap_or_default();
        Ok(match rest {
            ["block", _, _] => {
                let pos = self.loaded_pos(s, &a("pos"))?;
                i32::from(self.block_matches(&a("block"), pos))
            }
            ["blocks", _, _, _, mode] => {
                let start = self.loaded_pos(s, &a("start"))?;
                let end = self.loaded_pos(s, &a("end"))?;
                let dest = self.loaded_pos(s, &a("destination"))?;
                self.compare_regions(start, end, dest, *mode == "masked")?.unwrap_or(0)
            }
            ["entity", _] => self.arg_entities_opt(s, &a("entities")).len() as i32,
            ["score", _, _, op, _, _] => {
                let t = self.arg_score_holders(s, &a("target"))?;
                let src2 = self.arg_score_holders(s, &a("source"))?;
                let to = a("targetObjective");
                let so = a("sourceObjective");
                for o in [&to, &so] {
                    if self.cmd.scores.objective(o).is_none() {
                        return Err(Fail(tr!("arguments.objective.notFound", o)));
                    }
                }
                let (Some(x), Some(y)) = (self.cmd.scores.get(&t[0], &to), self.cmd.scores.get(&src2[0], &so)) else {
                    return Ok(0);
                };
                i32::from(match *op {
                    "=" => x == y,
                    "<" => x < y,
                    "<=" => x <= y,
                    ">" => x > y,
                    _ => x >= y,
                })
            }
            ["score", _, _, "matches", _] => {
                let t = self.arg_score_holders(s, &a("target"))?;
                let to = a("targetObjective");
                if self.cmd.scores.objective(&to).is_none() {
                    return Err(Fail(tr!("arguments.objective.notFound", to)));
                }
                let r = super::select::read_range(&a("range")).unwrap_or_default();
                self.cmd.scores.get(&t[0], &to).map_or(0, |v| i32::from(r.test(f64::from(v))))
            }
            ["data", kind, _, _] => {
                let n = self.data_source(s, p, seg, kind)?;
                super::cmds_data::path_get(&n, &a("path")).len() as i32
            }
            ["dimension", _] => i32::from(args::id(&a("dimension")) == s.dim),
            ["loaded", _] => {
                let pos = args::block_pos(self, s, &a("pos"));
                i32::from(self.is_loaded(pos))
            }
            ["biome", _, _] => {
                let pos = self.loaded_pos(s, &a("pos"))?;
                let b = self.biome_name(pos);
                let want = a("biome");
                i32::from(match want.strip_prefix('#') {
                    Some(t) => self.biome_tag(pos, &args::id(t)),
                    None => args::id(&want) == b,
                })
            }
            ["function", _] => {
                let r = self.call_functions(s, &a("name"))?;
                i32::from(r.0 && r.1 != 0)
            }
            ["predicate", _] => i32::from(self.command_predicate(s, &a("predicate"))?), // S9
            ["items", ..] => self.count_items(s, p, seg)?,
            // Number of slots the target owns in the range (empty ones included).
            ["slots", ..] => self.count_slots(s, p, seg)?,
            // Elapsed time of the stopwatch, in seconds, in the range.
            ["stopwatch", _, _] => {
                let id = args::id(&a("id"));
                let Some(secs) = self.stopwatch_seconds(&id) else {
                    return Err(Fail(tr!("commands.stopwatch.does_not_exist", id)));
                };
                let r = super::select::read_range(&a("range")).unwrap_or_default();
                i32::from(r.test(secs))
            }
            _ => 0,
        })
    }

    /// Loaded position.
    pub fn loaded_pos(&self, s: &Src, text: &str) -> Result<[i32; 3], Fail> {
        let pos = args::block_pos(self, s, text);
        if !self.is_loaded(pos) {
            return Err(Fail(tr!("argument.pos.unloaded")));
        }
        if !self.in_build_height(pos[1]) {
            return Err(Fail(tr!("argument.pos.outofworld")));
        }
        Ok(pos)
    }

    fn compare_regions(&self, a: [i32; 3], b: [i32; 3], d: [i32; 3], masked: bool) -> Result<Option<i32>, Fail> {
        let min = [a[0].min(b[0]), a[1].min(b[1]), a[2].min(b[2])];
        let max = [a[0].max(b[0]), a[1].max(b[1]), a[2].max(b[2])];
        let area = (0..3).map(|k| i64::from(max[k] - min[k] + 1)).product::<i64>();
        if area > 32768 {
            return Err(Fail(tr!("commands.execute.blocks.toobig", 32768, area)));
        }
        let air = self.tb().block_id("air");
        let mut count = 0;
        for z in min[2]..=max[2] {
            for y in min[1]..=max[1] {
                for x in min[0]..=max[0] {
                    let sp = [x, y, z];
                    let dp = [d[0] + x - min[0], d[1] + y - min[1], d[2] + z - min[2]];
                    let st = self.get(sp);
                    if masked && self.v().block(st) == air {
                        continue;
                    }
                    if st != self.get(dp) {
                        return Ok(None);
                    }
                    let strip = |n: Option<&Nbt>| {
                        n.map(|n| {
                            let mut n = n.clone();
                            for k in ["x", "y", "z", "id"] {
                                crate::nbtio::remove(&mut n, k);
                            }
                            n
                        })
                    };
                    let (sb, db) = (strip(self.be_get(sp)), strip(self.be_get(dp)));
                    if let Some(sb) = sb {
                        match db {
                            Some(db) if crate::items::nbt_eq(&sb, &db) => {}
                            _ => return Ok(None),
                        }
                    }
                    count += 1;
                }
            }
        }
        Ok(Some(count))
    }

    /// Final command.
    fn command(&mut self, s: &Src, p: &Parsed, seg: &Seg, path: &str) -> R {
        // `execute in`: the command runs in the source's world (S6); `teleport`
        // does the dimension change itself.
        if s.dim != self.dim.name && seg.path.first().map(String::as_str) != Some("teleport") {
            let dim = s.dim.clone();
            return self.in_dimension(&dim, |sv| sv.command_here(s, p, seg, path));
        }
        self.command_here(s, p, seg, path)
    }

    fn command_here(&mut self, s: &Src, p: &Parsed, seg: &Seg, path: &str) -> R {
        let head = seg.path.first().map(String::as_str).unwrap_or("");
        match head {
            "execute" => {
                // Terminal condition (`execute if … ` without `run`).
                let w: Vec<&str> = path.split(' ').skip(1).collect();
                let expected = w.first() == Some(&"if");
                let numeric = matches!(w.get(1), Some(&"entity" | &"data" | &"items" | &"slots" | &"blocks"));
                let n = self.condition(s, p, seg, &w[1..])?;
                if numeric {
                    if expected {
                        if n > 0 {
                            self.ok(s, tr!("commands.execute.conditional.pass_count", n));
                            return Ok(n);
                        }
                        return Err(Fail(tr!("commands.execute.conditional.fail")));
                    }
                    if n == 0 {
                        self.ok(s, tr!("commands.execute.conditional.pass"));
                        return Ok(1);
                    }
                    return Err(Fail(tr!("commands.execute.conditional.fail_count", n)));
                }
                if (n > 0) == expected {
                    self.ok(s, tr!("commands.execute.conditional.pass"));
                    Ok(1)
                } else {
                    Err(Fail(tr!("commands.execute.conditional.fail")))
                }
            }
            "function" => self.cmd_function(s, p, seg),
            "return" => {
                if seg.path.get(1).map(String::as_str) == Some("fail") {
                    // Failure without a message; the function stops.
                    self.cmd.returned = Some((false, 0));
                    return Err(Fail(rc_text::Component::text("")));
                }
                let v = p.arg(seg, "value").and_then(|v| v.parse().ok()).unwrap_or(0);
                self.cmd.returned = Some((true, v));
                Ok(v)
            }
            "schedule" => self.cmd_schedule(s, p, seg),
            _ => self.dispatch(s, p, seg, path),
        }
    }

    /// Runs a function or a function tag (`execute if function`): the first one
    /// that returns a result stops the following ones; (found, value returned, 0 without `return`).
    pub fn call_functions(&mut self, s: &Src, name: &str) -> Result<(bool, i32), Fail> {
        let ids = self.function_ids(name)?;
        let mut fs = s.clone();
        fs.stores.clear();
        for id in &ids {
            let lines = match self.instantiate_function(id, None) {
                Ok(l) => l,
                Err(f) => return Err(Fail(tr!("commands.execute.function.instantiationFailure", id.clone(), f.0))),
            };
            if let Some((_, v)) = self.call_function(&fs, id, &lines) {
                return Ok((true, v));
            }
            if self.exhausted() {
                break;
            }
        }
        Ok((!ids.is_empty(), 0))
    }

    /// Functions launched by the server (`tick`/`load` tags, `schedule`, advancement
    /// rewards, `/debug function`): each in its own execution context, like the game;
    /// (found, last value returned).
    pub fn run_functions(&mut self, s: &Src, name: &str, _announce: bool) -> Result<(bool, i32), Fail> {
        let ids = self.function_ids(name)?;
        let mut last = 0;
        for id in &ids {
            let Ok(lines) = self.instantiate_function(id, None) else { continue };
            if let Some((_, v)) = self.in_context(|sv| sv.call_function(s, id, &lines)) {
                last = v;
            }
        }
        Ok((!ids.is_empty(), last))
    }

    /// Ids designated by `name` (`#tag` or function).
    fn function_ids(&self, name: &str) -> Result<Vec<String>, Fail> {
        if let Some(t) = name.strip_prefix('#') {
            let t = args::id(t);
            return match self.cmd.function_tags.get(&t) {
                Some(v) => Ok(v.clone()),
                None => Err(Fail(tr!("arguments.function.tag.unknown", t))),
            };
        }
        let id = args::id(name);
        if !self.cmd.functions.contains_key(&id) {
            return Err(Fail(tr!("arguments.function.unknown", id)));
        }
        Ok(vec![id])
    }

    /// Lines to run of a function: macros substituted (`$…$(key)…`) then parsed,
    /// like the game's instantiation; error = reason (`commands.function.error.*`).
    fn instantiate_function(&self, id: &str, args_nbt: Option<&Nbt>) -> Result<Vec<String>, Fail> {
        let Some(lines) = self.cmd.functions.get(id) else { return Ok(Vec::new()) };
        if !lines.iter().any(|l| l.starts_with('$')) {
            return Ok(lines.clone());
        }
        let Some(args_nbt) = args_nbt else {
            return Err(Fail(tr!("commands.function.error.missing_arguments", id.to_owned())));
        };
        // Parameters in order of first appearance (the game reports the first one missing).
        let mut params: Vec<String> = Vec::new();
        let templates: Vec<Option<Template>> =
            lines.iter().map(|l| l.strip_prefix('$').and_then(|m| Template::parse(m).ok())).collect();
        for t in templates.iter().flatten() {
            for v in &t.vars {
                if !params.contains(v) {
                    params.push(v.clone());
                }
            }
        }
        let mut values = BTreeMap::new();
        for k in &params {
            let Some(v) = args_nbt.get(k) else {
                return Err(Fail(tr!("commands.function.error.missing_argument", id.to_owned(), k.clone())));
            };
            values.insert(k.clone(), macro_string(v));
        }
        let lvl = self.cmd.level(2);
        let mut out = Vec::with_capacity(lines.len());
        for (line, t) in lines.iter().zip(&templates) {
            let Some(t) = t else {
                out.push(line.clone());
                continue;
            };
            let cmd = t.substitute(&values);
            if let Some(lvl) = lvl
                && let Some(msg) = super::syntax_message(lvl, &self.cmd.data, &cmd, 2)
            {
                return Err(Fail(tr!("commands.function.error.parse", id.to_owned(), cmd, msg)));
            }
            out.push(cmd);
        }
        Ok(out)
    }

    /// Call of an instantiated function (queue entry of the game: one cost); `Some((success,
    /// value))` if it returned a result (`return`).
    fn call_function(&mut self, s: &Src, id: &str, lines: &[String]) -> Option<(bool, i32)> {
        if !self.cmd.flow.active {
            // Top-level call: its own context (the lines must not each
            // open one, which would reset the call state to zero).
            return self.in_context(|sv| sv.call_function(s, id, lines));
        }
        if self.exhausted() {
            return None;
        }
        self.cmd.flow.quota -= 1;
        if self.cmd.flow.depth >= MAX_NESTING {
            // Stack safeguard: complete stop, like an exhausted quota.
            self.cmd.flow.quota = 0;
            return None;
        }
        if super::cmds_ents::tracing() {
            // `/debug function`: traced call with the function's command count.
            super::cmds_ents::trace_call(self.cmd.depth as usize, id, lines.len());
        }
        #[cfg(test)]
        {
            let here = 0u8;
            let a = std::ptr::from_ref(&here) as usize;
            STACK_PROBE.with(|p| p.borrow_mut().push((self.cmd.flow.depth, a)));
        }
        self.cmd.depth += 1;
        self.cmd.flow.depth += 1;
        let mut fs = s.clone();
        fs.silent = true;
        fs.level = fs.level.max(2);
        fs.stores.clear();
        let saved = self.cmd.returned.take();
        for l in lines {
            if self.exhausted() {
                break;
            }
            super::cmds_ents::trace_command(self.cmd.depth as usize, l);
            let (ok, v) = self.perform(fs.clone(), l);
            if ok {
                super::cmds_ents::trace_return(self.cmd.depth as usize, l, v);
            }
            if self.cmd.returned.is_some() {
                break;
            }
        }
        let result = std::mem::replace(&mut self.cmd.returned, saved);
        self.cmd.depth -= 1;
        self.cmd.flow.depth -= 1;
        result
    }

    /// Command `function <name> [arguments | with …]`.
    fn cmd_function(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let ret = std::mem::take(&mut self.cmd.flow.ret_mode);
        let name = p.arg(seg, "name").unwrap_or_default();
        let ids = self.function_ids(&name)?;
        if ids.is_empty() {
            return Err(Fail(tr!("commands.function.scheduled.no_functions", args::id(name.trim_start_matches('#')))));
        }
        let fargs = self.function_arguments(s, p, seg)?;
        if ids.len() == 1 {
            self.ok(s, tr!("commands.function.scheduled.single", ids[0].clone()));
        } else {
            let list = text::list(ids.iter().map(|i| rc_text::Component::text(i.clone())).collect());
            self.ok(s, tr!("commands.function.scheduled.multiple", list));
        }
        let single = ids.len() == 1;
        let mut got: Option<(bool, i32)> = None;
        let mut sum = 0i32;
        for id in &ids {
            let lines = match self.instantiate_function(id, fargs.as_ref()) {
                Ok(l) => l,
                Err(f) => {
                    if ret {
                        self.cmd.returned = Some((false, 0));
                    }
                    return Err(Fail(tr!("commands.function.instantiationFailure", id.clone(), f.0)));
                }
            };
            let r = self.call_function(s, id, &lines);
            if let Some((ok, v)) = r {
                // Result announced to the source (nothing if it is muted).
                self.ok(s, tr!("commands.function.result", id.clone(), v));
                if ret {
                    // `return run function`: the first function that returns a result returns it
                    // to the caller and stops the following ones.
                    self.cmd.returned = Some((ok, v));
                    return if ok { Ok(v) } else { Err(Fail(rc_text::Component::text(""))) };
                }
                sum = sum.wrapping_add(v);
                got = Some(if single { (ok, v) } else { (true, sum) });
            }
            if self.exhausted() {
                break;
            }
        }
        if ret {
            // No function returned a result: the caller returns a failure.
            self.cmd.returned = Some((false, 0));
            return Err(Fail(rc_text::Component::text("")));
        }
        match got {
            Some((true, v)) => Ok(v),
            Some((false, _)) => Err(Fail(rc_text::Component::text(""))),
            None => {
                self.cmd.flow.no_callback = true;
                Ok(0)
            }
        }
    }

    /// Macro arguments: SNBT compound, or data of a block, an entity, a storage
    /// (optional path designating a compound).
    fn function_arguments(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> Result<Option<Nbt>, Fail> {
        match seg.path.get(2).map(String::as_str) {
            Some("<arguments>") => {
                let text = p.arg(seg, "arguments").unwrap_or_default();
                Ok(Some(super::snbt::parse(&text).unwrap_or(Nbt::Compound(Vec::new()))))
            }
            Some("with") => {
                let kind = seg.path.get(3).map(String::as_str).unwrap_or("storage");
                let data = self.data_source(s, p, seg, kind)?;
                let Some(path) = p.arg(seg, "path") else { return Ok(Some(data)) };
                let found = super::cmds_data::path_find(&data, &path)?;
                if found.len() > 1 {
                    return Err(Fail(tr!("commands.data.get.multiple")));
                }
                match &found[0] {
                    c @ Nbt::Compound(_) => Ok(Some(c.clone())),
                    other => Err(Fail(tr!("commands.function.error.argument_not_compound", tag_type_name(other)))),
                }
            }
            _ => Ok(None),
        }
    }

    fn cmd_schedule(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let a = |n: &str| p.arg(seg, n).unwrap_or_default();
        match seg.path.get(1).map(String::as_str) {
            Some("clear") => {
                let f = a("function");
                let before = self.cmd.scheduled.len();
                self.cmd.scheduled.retain(|(_, n, _)| *n != f);
                let n = before - self.cmd.scheduled.len();
                if n == 0 {
                    return Err(Fail(tr!("commands.schedule.cleared.failure", f)));
                }
                self.ok(s, tr!("commands.schedule.cleared.success", n, f));
                Ok(n as i32)
            }
            _ => {
                let f = a("function");
                let t = super::cmds_basic::time_ticks(&a("time"));
                if t == 0 {
                    return Err(Fail(tr!("commands.schedule.same_tick")));
                }
                let replace = seg.path.get(4).is_none_or(|m| m == "replace");
                let at = self.time + i64::from(t);
                if replace {
                    self.cmd.scheduled.retain(|(_, n, _)| *n != f);
                }
                self.cmd.scheduled.push((at, f.clone(), f.starts_with('#')));
                let key = if f.starts_with('#') {
                    "commands.schedule.created.tag"
                } else {
                    "commands.schedule.created.function"
                };
                self.ok(s, tr!(key, f.trim_start_matches('#').to_owned(), t, at));
                Ok(at as i32)
            }
        }
    }

    pub(crate) fn run_scheduled(&mut self) {
        if self.cmd.scheduled.is_empty() {
            return;
        }
        let now = self.time;
        let (due, rest): (Vec<_>, Vec<_>) =
            std::mem::take(&mut self.cmd.scheduled).into_iter().partition(|(t, _, _)| *t <= now);
        self.cmd.scheduled = rest;
        for (_, f, _) in due {
            let src = self.server_source();
            let _ = self.run_functions(&src, &f, false);
        }
    }
}

/// Spaces removed like Java's `String.trim` (characters ≤ U+0020).
fn jtrim(s: &str) -> &str {
    s.trim_matches(|c: char| c <= ' ')
}

/// Compilation of a function at load time, like the game: `\` continuations joined,
/// comments and empty lines removed, macro lines (`$`) kept with their `$`; any
/// invalid line (command that does not parse at level 2, leading `/`, macro without a
/// variable…) makes the whole function refused (the game's log text).
pub fn compile_function(
    raw: &[&str],
    lvl: Option<&super::Level>,
    data: &rc_commands::Data,
) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        let number = i + 1;
        let mut line = jtrim(raw[i]).to_owned();
        while line.ends_with('\\') {
            i += 1;
            if i == raw.len() {
                return Err("Line continuation at end of file".into());
            }
            line.pop();
            line.push_str(jtrim(raw[i]));
        }
        i += 1;
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(rest) = line.strip_prefix('/') {
            if rest.starts_with('/') {
                return Err(format!(
                    "Unknown or invalid command '{line}' on line {number} (if you intended to make a comment, use '#' not '//')"
                ));
            }
            let name: String = rest.chars().take_while(|&c| rc_commands::reader::is_unquoted(c)).collect();
            return Err(format!(
                "Unknown or invalid command '{line}' on line {number} (did you mean '{name}'? Do not use a preceding forwards slash.)"
            ));
        }
        if let Some(m) = line.strip_prefix('$') {
            if Template::parse(m).is_err() {
                return Err(format!("Can't parse function line {number}: '{m}'"));
            }
        } else if let Some(lvl) = lvl
            && let Some(msg) = super::syntax_message(lvl, data, &line, 2)
        {
            return Err(format!("Whilst parsing command on line {number}: {msg}"));
        }
        out.push(line);
    }
    Ok(out)
}

/// Template of a macro line: pieces of text and interspersed `$(name)` variables.
struct Template {
    segments: Vec<String>,
    vars: Vec<String>,
}

impl Template {
    fn parse(input: &str) -> Result<Template, &'static str> {
        let b = input.as_bytes();
        let (mut segments, mut vars) = (Vec::new(), Vec::new());
        let mut start = 0;
        let mut index = input.find('$');
        while let Some(at) = index {
            if at != b.len() - 1 && b[at + 1] == b'(' {
                segments.push(input[start..at].to_owned());
                let Some(end) = input[at + 1..].find(')').map(|e| e + at + 1) else {
                    return Err("Unterminated macro variable");
                };
                let var = &input[at + 2..end];
                if !var.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    return Err("Invalid macro variable name");
                }
                vars.push(var.to_owned());
                start = end + 1;
                index = input[start..].find('$').map(|e| e + start);
            } else {
                index = input[at + 1..].find('$').map(|e| e + at + 1);
            }
        }
        if start == 0 {
            return Err("No variables in macro");
        }
        if start != input.len() {
            segments.push(input[start..].to_owned());
        }
        Ok(Template { segments, vars })
    }

    fn substitute(&self, values: &BTreeMap<String, String>) -> String {
        let mut s = String::new();
        for (i, v) in self.vars.iter().enumerate() {
            s.push_str(&self.segments[i]);
            s.push_str(values.get(v).map_or("", String::as_str));
        }
        if self.segments.len() > self.vars.len()
            && let Some(last) = self.segments.last()
        {
            s.push_str(last);
        }
        s
    }
}

/// Value of a macro argument as text: numbers without suffix (floats in decimal, at most 15
/// digits after the decimal point, never an exponent), raw string, otherwise SNBT.
fn macro_string(v: &Nbt) -> String {
    match v {
        Nbt::Float(f) => java_decimal(f64::from(*f)),
        Nbt::Double(d) => java_decimal(*d),
        Nbt::Byte(x) => x.to_string(),
        Nbt::Short(x) => x.to_string(),
        Nbt::Long(x) => x.to_string(),
        Nbt::String(s) => s.clone(),
        other => super::snbt::to_snbt(other),
    }
}

/// The game's decimal format for macros (pattern "#", at most 15 decimals, round half even):
/// starts from the digits of the shortest representation of the double, like the Java formatter.
fn java_decimal(x: f64) -> String {
    if x.is_nan() {
        return "NaN".into();
    }
    if x.is_infinite() {
        return if x > 0.0 { "\u{221e}".into() } else { "-\u{221e}".into() };
    }
    let neg = x.is_sign_negative();
    let e = format!("{:e}", x.abs());
    let (mant, exp) = e.split_once('e').unwrap_or((&e, "0"));
    let exp: i64 = exp.parse().unwrap_or(0);
    let digits: Vec<u8> = mant.bytes().filter(u8::is_ascii_digit).map(|c| c - b'0').collect();
    // Value = 0.d1d2… × 10^(exp+1); position of the decimal point in `digits`.
    let point = exp + 1;
    // Digits kept: up to 15 after the decimal point.
    let keep = (point + 15).max(0) as usize;
    let mut kept: Vec<u8> = digits.iter().copied().take(keep).collect();
    let mut point = point;
    if digits.len() > keep {
        let rest = &digits[keep..];
        let first = rest[0];
        let tail_nonzero = rest[1..].iter().any(|&d| d != 0);
        let last_odd = kept.last().is_some_and(|d| d % 2 == 1);
        let up = first > 5 || (first == 5 && (tail_nonzero || last_odd));
        if up {
            let mut k = kept.len();
            loop {
                if k == 0 {
                    kept.insert(0, 1);
                    point += 1;
                    break;
                }
                k -= 1;
                if kept[k] == 9 {
                    kept[k] = 0;
                } else {
                    kept[k] += 1;
                    break;
                }
            }
        }
    }
    let int: String = if point <= 0 {
        "0".into()
    } else {
        (0..point as usize).map(|j| char::from(b'0' + kept.get(j).copied().unwrap_or(0))).collect()
    };
    let frac: String =
        (point..kept.len() as i64).map(|j| if j < 0 { '0' } else { char::from(b'0' + kept[j as usize]) }).collect();
    let frac = frac.trim_end_matches('0');
    let mut s = String::new();
    if neg {
        s.push('-');
    }
    s.push_str(&int);
    if !frac.is_empty() {
        s.push('.');
        s.push_str(frac);
    }
    s
}

/// Name of an NBT tag type as the game displays it (`TAG_Compound`…).
fn tag_type_name(v: &Nbt) -> &'static str {
    match v {
        Nbt::Byte(_) => "TAG_Byte",
        Nbt::Short(_) => "TAG_Short",
        Nbt::Int(_) => "TAG_Int",
        Nbt::Long(_) => "TAG_Long",
        Nbt::Float(_) => "TAG_Float",
        Nbt::Double(_) => "TAG_Double",
        Nbt::ByteArray(_) => "TAG_Byte_Array",
        Nbt::String(_) => "TAG_String",
        Nbt::List(_) => "TAG_List",
        Nbt::Compound(_) => "TAG_Compound",
        Nbt::IntArray(_) => "TAG_Int_Array",
        Nbt::LongArray(_) => "TAG_Long_Array",
    }
}

#[cfg(test)]
mod macro_tests {
    use super::*;

    #[test]
    fn decimal_like_java() {
        assert_eq!(java_decimal(1.0), "1");
        assert_eq!(java_decimal(-2.5), "-2.5");
        assert_eq!(java_decimal(0.0), "0");
        assert_eq!(java_decimal(-0.0), "-0");
        assert_eq!(java_decimal(f64::from(0.1f32)), "0.100000001490116");
        assert_eq!(java_decimal(1e20), "100000000000000000000");
        assert_eq!(java_decimal(2f64.powi(70)), "1180591620717411300000");
        assert_eq!(java_decimal(1e-16), "0");
        assert_eq!(java_decimal(-1e-16), "-0");
        assert_eq!(java_decimal(0.000123), "0.000123");
        assert_eq!(java_decimal(0.9999999999999999), "1");
        assert_eq!(java_decimal(123.456), "123.456");
    }

    #[test]
    fn template_parse_and_substitute() {
        let t = Template::parse("say $(a) and $(b_2)!").unwrap();
        assert_eq!(t.vars, ["a", "b_2"]);
        let v: BTreeMap<String, String> = [("a".into(), "1".into()), ("b_2".into(), "x".into())].into_iter().collect();
        assert_eq!(t.substitute(&v), "say 1 and x!");
        assert!(Template::parse("say nothing").is_err());
        assert!(Template::parse("say $(a").is_err());
        assert!(Template::parse("say $(a-b)").is_err());
        assert_eq!(
            Template::parse("$(x)").unwrap().substitute(&[("x".into(), "kill".into())].into_iter().collect()),
            "kill"
        );
        // `$` alone: text kept.
        assert_eq!(
            Template::parse("say 5$ $(x)").unwrap().substitute(&[("x".into(), "y".into())].into_iter().collect()),
            "say 5$ y"
        );
    }

    #[test]
    fn macro_values() {
        assert_eq!(macro_string(&Nbt::Byte(3)), "3");
        assert_eq!(macro_string(&Nbt::Long(-7)), "-7");
        assert_eq!(macro_string(&Nbt::String("a b".into())), "a b");
        assert_eq!(macro_string(&Nbt::Int(12)), "12");
        assert_eq!(macro_string(&Nbt::Compound(vec![("k".into(), Nbt::Int(1))])), "{k:1}");
    }
}
