//! Advancements on the integrated server (mission S9, `docs/specs/advancements-stats.md` § 7): advancement
//! tree read from the jar and the world's packs, positions computed like the game, player
//! progress (criteria obtained, visibility), full then incremental `update_advancements` packet,
//! rewards, chat announcement, `advancements/<uuid>.json` save.

pub mod hooks;
pub mod layout;
pub mod pred;
pub mod trig;

#[cfg(test)]
mod tests;

use hashbrown::HashMap;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use rc_client::Event;
use rc_proto::nbt::Nbt;
use rc_proto_v777::advancements as adv;
use serde_json::Value;

use crate::player::Stack;
use crate::server::Server;

/// Display of an advancement (`display`).
#[derive(Debug, Clone)]
pub struct Display {
    pub title: Value,
    pub description: Value,
    pub icon: Value,
    /// 0 task, 1 challenge, 2 goal.
    pub frame: u8,
    pub background: Option<String>,
    pub toast: bool,
    pub announce: bool,
    pub hidden: bool,
}

/// Criterion: name, trigger, raw conditions.
#[derive(Debug, Clone)]
pub struct Crit {
    pub name: String,
    pub trigger: String,
    pub cond: Value,
}

#[derive(Debug, Clone)]
pub struct Adv {
    pub id: String,
    pub parent: Option<usize>,
    pub display: Option<Display>,
    pub rewards: Value,
    pub criteria: Vec<Crit>,
    /// Requirements: AND of ORs, as criterion indices.
    pub req: Vec<Vec<usize>>,
    /// Requirements by name (packet).
    pub req_names: Vec<Vec<String>>,
    pub telemetry: bool,
    pub children: Vec<usize>,
    pub root: usize,
    pub x: f32,
    pub y: f32,
}

/// Advancement tree of the world (shared by the players).
#[derive(Debug, Default)]
pub struct Tree {
    pub list: Vec<Adv>,
    pub index: HashMap<String, usize>,
    /// Trigger → (advancement, criterion).
    pub by_trigger: HashMap<String, Vec<(usize, usize)>>,
}

fn id_of(s: &str) -> String {
    crate::commands::args::id(s)
}

impl Tree {
    /// Shared tree: with the base pack alone, the jar's is read once per
    /// process (successive openings, parallel tests).
    pub(crate) fn shared(jar: &crate::registries::Jar, vanilla_only: bool) -> Arc<Tree> {
        static JAR_ONLY: std::sync::OnceLock<Arc<Tree>> = std::sync::OnceLock::new();
        if vanilla_only {
            return JAR_ONLY.get_or_init(|| Arc::new(Tree::load(jar))).clone();
        }
        Arc::new(Tree::load(jar))
    }

    /// Reads `data/<ns>/advancement/**.json` from the stack of chosen packs (`packs.rs`: the
    /// last pack wins), links parents and children, computes positions.
    pub(crate) fn load(jar: &crate::registries::Jar) -> Tree {
        let mut raw: std::collections::BTreeMap<String, Value> = std::collections::BTreeMap::new();
        for (name, bytes) in &jar.files {
            let Some(rest) = name.strip_prefix("data/") else { continue };
            let Some((ns, path)) = rest.split_once('/') else { continue };
            let Some(p) = path.strip_prefix("advancement/").and_then(|p| p.strip_suffix(".json")) else { continue };
            if let Ok(v) = serde_json::from_slice::<Value>(bytes) {
                raw.insert(format!("{ns}:{p}"), v);
            }
        }
        Tree::build(raw)
    }

    pub fn build(raw: std::collections::BTreeMap<String, Value>) -> Tree {
        let mut t = Tree::default();
        // Insertion in passes: an advancement enters when its parent is known.
        let mut pending: Vec<(String, Value)> = raw.into_iter().collect();
        loop {
            let before = pending.len();
            let mut rest = Vec::new();
            for (id, v) in pending {
                let parent = v.get("parent").and_then(Value::as_str).map(id_of);
                let pidx = match &parent {
                    Some(p) => match t.index.get(p) {
                        Some(&i) => Some(i),
                        None => {
                            rest.push((id, v));
                            continue;
                        }
                    },
                    None => None,
                };
                if let Some(a) = parse_adv(&id, &v, pidx) {
                    let i = t.list.len();
                    t.index.insert(id.clone(), i);
                    t.list.push(a);
                    if let Some(p) = pidx {
                        t.list[p].children.push(i);
                    }
                }
            }
            pending = rest;
            if pending.is_empty() || pending.len() == before {
                if !pending.is_empty() {
                    eprintln!("singleplayer: advancements without a parent ignored: {}", pending.len());
                }
                break;
            }
        }
        for i in 0..t.list.len() {
            let mut r = i;
            while let Some(p) = t.list[r].parent {
                r = p;
            }
            t.list[i].root = r;
            let mut ch = std::mem::take(&mut t.list[i].children);
            ch.sort_by(|a, b| t.list[*a].id.cmp(&t.list[*b].id));
            t.list[i].children = ch;
            for (c, crit) in t.list[i].criteria.iter().enumerate() {
                t.by_trigger.entry(crit.trigger.clone()).or_default().push((i, c));
            }
        }
        layout::run(&mut t);
        t
    }

    pub fn get(&self, id: &str) -> Option<usize> {
        self.index.get(id).copied()
    }
}

fn parse_adv(id: &str, v: &Value, parent: Option<usize>) -> Option<Adv> {
    let crit_obj = v.get("criteria")?.as_object()?;
    if crit_obj.is_empty() {
        return None;
    }
    let mut criteria: Vec<Crit> = crit_obj
        .iter()
        .map(|(name, c)| Crit {
            name: name.clone(),
            trigger: id_of(c.get("trigger").and_then(Value::as_str).unwrap_or("minecraft:impossible")),
            cond: c.get("conditions").cloned().unwrap_or(Value::Null),
        })
        .collect();
    criteria.sort_by(|a, b| a.name.cmp(&b.name));
    let req_names: Vec<Vec<String>> = match v.get("requirements").and_then(Value::as_array) {
        Some(groups) => groups
            .iter()
            .map(|g| {
                g.as_array()
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(str::to_owned)).collect())
                    .unwrap_or_default()
            })
            .collect(),
        None => criteria.iter().map(|c| vec![c.name.clone()]).collect(),
    };
    // Game validation at load: requirements = exactly the criteria; background reserved
    // for roots; displayed root with a background. A rejected advancement is ignored (warning).
    let named = |n: &String| criteria.iter().any(|c| &c.name == n);
    if !req_names.iter().flatten().all(named) || !criteria.iter().all(|c| req_names.iter().flatten().any(|n| n == &c.name))
    {
        eprintln!("singleplayer: advancement {id} rejected: requirements differ from the criteria");
        return None;
    }
    let background = v.get("display").is_some_and(|d| d.get("background").is_some());
    if v.get("parent").is_some() && background {
        eprintln!("singleplayer: advancement {id} rejected: only a root has a background");
        return None;
    }
    if v.get("parent").is_none() && v.get("display").is_some() && !background {
        eprintln!("singleplayer: advancement {id} rejected: displayed root without a background");
        return None;
    }
    let req = req_names
        .iter()
        .map(|g| g.iter().filter_map(|n| criteria.iter().position(|c| &c.name == n)).collect())
        .collect();
    let display = v.get("display").map(|d| Display {
        title: d.get("title").cloned().unwrap_or(Value::Null),
        description: d.get("description").cloned().unwrap_or(Value::Null),
        icon: d.get("icon").cloned().unwrap_or(Value::Null),
        frame: match d.get("frame").and_then(Value::as_str) {
            Some("challenge") => 1,
            Some("goal") => 2,
            _ => 0,
        },
        background: d.get("background").and_then(Value::as_str).map(id_of),
        toast: d.get("show_toast").and_then(Value::as_bool).unwrap_or(true),
        announce: d.get("announce_to_chat").and_then(Value::as_bool).unwrap_or(true),
        hidden: d.get("hidden").and_then(Value::as_bool).unwrap_or(false),
    });
    Some(Adv {
        id: id.to_owned(),
        parent,
        display,
        rewards: v.get("rewards").cloned().unwrap_or(Value::Null),
        criteria,
        req,
        req_names,
        telemetry: v.get("sends_telemetry_event").and_then(Value::as_bool).unwrap_or(false),
        children: Vec::new(),
        root: 0,
        x: 0.0,
        y: 0.0,
    })
}

/// Progress of a player.
pub struct Progress {
    pub tree: Arc<Tree>,
    /// Per advancement, per criterion: obtained instant (Unix ms).
    pub got: Vec<Vec<Option<i64>>>,
    pub visible: Vec<bool>,
    pub changed: BTreeSet<usize>,
    pub roots: BTreeSet<usize>,
    pub first: bool,
    pub tab: Option<usize>,
    /// Write order of the file (started advancements).
    pub order: Vec<usize>,
    pub path: PathBuf,
    pub dirty: bool,
    /// Inventory snapshot (detection of changed slots).
    pub inv_seen: Vec<Option<Stack>>,
    /// Start of the fall in progress (distance trigger).
    pub fall_start: Option<[f64; 3]>,
    /// Keys of the last `player_input` (`input` predicate).
    pub keys: u8,
    /// Statistics of the player.
    pub stats: crate::stats::Stats,
    /// Mob currently hitting the player (damage source).
    pub attacker: Option<Box<crate::mobs::mob::Mob>>,
    /// Other player currently hitting the player (local network): display name, name of
    /// the item in hand if renamed.
    pub attacker_player: Option<(rc_text::Component, Option<rc_text::Component>)>,
    /// Retaliation of a parried hit that changed the attacker (stunned ravager): identifier,
    /// stun ticks, random draw; taken back by the caller that holds the mob.
    pub riposte: Option<(i32, i32, crate::mobs::jrand::JRand)>,
    /// W8c: damage source in progress (shield), consumed by `hurt`.
    pub dmg_src: crate::item_use::DmgSrc,
    /// W8c: start of the levitation in progress (position, tick) — `levitation` trigger.
    pub lev_start: Option<([f64; 3], u64)>,
    /// W8c: mount to report (`started_riding`, at the player's next tick).
    pub ride_started: bool,
    /// W8c: entering lava on a vehicle (`ride_entity_in_lava`).
    pub lava_ride: Option<[f64; 3]>,
    /// W8c: last impact point of an explosion and type of its cause (`fall_after_explosion`).
    pub expl_impact: Option<([f64; 3], Option<String>)>,
    /// W8c: type of the exploding entity when it is not a mob (fireball,
    /// skull, crystal), set for the duration of the explosion.
    pub expl_cause_next: Option<&'static str>,
    /// S9 commands specific to the player: camera (`spectate`); stopwatches and waypoints in `cmd.s9`.
    pub cmds: crate::commands::cmds_s9a::State,
    /// `place`: generation data kept.
    pub place: Option<Box<crate::commands::cmds_s9d::PlaceData>>,
}

impl Progress {
    pub(crate) fn new(tree: Arc<Tree>, world: &Path, uuid: u128) -> Progress {
        let n = tree.list.len();
        let got = tree.list.iter().map(|a| vec![None; a.criteria.len()]).collect();
        // 26.3: `players/advancements/` (like `players/data/`).
        let path = world.join("players").join("advancements").join(format!("{}.json", crate::nbtio::uuid_text(uuid)));
        let mut p = Progress {
            tree,
            got,
            visible: vec![false; n],
            changed: BTreeSet::new(),
            roots: BTreeSet::new(),
            first: true,
            tab: None,
            order: Vec::new(),
            path,
            dirty: false,
            inv_seen: Vec::new(),
            fall_start: None,
            keys: 0,
            stats: crate::stats::Stats::read(world, uuid),
            attacker: None,
            attacker_player: None,
            riposte: None,
            dmg_src: Default::default(),
            lev_start: None,
            ride_started: false,
            lava_ride: None,
            expl_impact: None,
            expl_cause_next: None,
            cmds: Default::default(),
            place: None,
        };
        p.load();
        p
    }

    fn load(&mut self) {
        let Ok(text) = std::fs::read(&self.path) else { return };
        let Ok(Value::Object(map)) = serde_json::from_slice::<Value>(&text) else {
            eprintln!("singleplayer: advancements unreadable: {}", self.path.display());
            return;
        };
        let raw = String::from_utf8_lossy(&text);
        let mut entries: Vec<(usize, i64)> = Vec::new();
        for (id, v) in &map {
            if id == "DataVersion" {
                continue;
            }
            let Some(i) = self.tree.get(&id_of(id)) else {
                eprintln!("singleplayer: unknown advancement ignored: {id}");
                continue;
            };
            let mut first = i64::MAX;
            if let Some(Value::Object(c)) = v.get("criteria") {
                for (name, date) in c {
                    let Some(ci) = self.tree.list[i].criteria.iter().position(|x| &x.name == name) else { continue };
                    let ms = date.as_str().and_then(parse_date).unwrap_or(0);
                    first = first.min(ms);
                    self.got[i][ci] = Some(ms);
                }
            }
            // File order (the game's ordered table): position of the key in the text.
            let at = raw.find(&format!("{}:", json_str(id))).map_or(first, |p| p as i64);
            entries.push((i, at));
        }
        entries.sort_by_key(|e| e.1);
        for (i, _) in entries {
            self.order.push(i);
            self.changed.insert(i);
            self.roots.insert(self.tree.list[i].root);
        }
    }

    /// Data reload (`reload`): progress written then re-read with the new tree.
    pub fn reload(&mut self, tree: Arc<Tree>) {
        let _ = std::fs::create_dir_all(self.path.parent().unwrap_or_else(|| Path::new(".")));
        let _ = std::fs::write(&self.path, self.to_json(crate::chunk::DATA_VERSION));
        let n = tree.list.len();
        self.got = tree.list.iter().map(|a| vec![None; a.criteria.len()]).collect();
        self.visible = vec![false; n];
        self.changed.clear();
        self.roots.clear();
        self.order.clear();
        self.first = true;
        self.tab = None;
        self.tree = tree;
        self.load();
    }

    pub fn crit_done(&self, a: usize, c: usize) -> bool {
        self.got[a][c].is_some()
    }

    /// Requirements satisfied; no requirement: never.
    pub fn done(&self, a: usize) -> bool {
        let adv = &self.tree.list[a];
        !adv.req.is_empty() && adv.req.iter().all(|g| g.iter().any(|&c| self.got[a][c].is_some()))
    }

    pub fn has_progress(&self, a: usize) -> bool {
        self.got[a].iter().any(Option::is_some)
    }

    fn mark_root(&mut self, a: usize) {
        self.roots.insert(self.tree.list[a].root);
    }

    /// Visibility of a tree: added and removed.
    fn update_visibility(&mut self, root: usize, added: &mut Vec<usize>, removed: &mut Vec<usize>) {
        #[derive(Clone, Copy, PartialEq)]
        enum Rule {
            Show,
            Hide,
            Keep,
        }
        fn walk(
            p: &mut Progress,
            n: usize,
            stack: &mut Vec<Rule>,
            added: &mut Vec<usize>,
            removed: &mut Vec<usize>,
        ) -> bool {
            let done = p.done(n);
            let rule = match &p.tree.list[n].display {
                None => Rule::Hide,
                Some(_) if done => Rule::Show,
                Some(d) if d.hidden => Rule::Hide,
                Some(_) => Rule::Keep,
            };
            let mut any = done;
            stack.push(rule);
            let children = p.tree.list[n].children.clone();
            for c in children {
                any |= walk(p, c, stack, added, removed);
            }
            let visible = any || {
                let mut v = false;
                for i in 0..=2 {
                    match stack[stack.len() - 1 - i] {
                        Rule::Show => {
                            v = true;
                            break;
                        }
                        Rule::Hide => break,
                        Rule::Keep => {}
                    }
                }
                v
            };
            stack.pop();
            if visible {
                if !p.visible[n] {
                    p.visible[n] = true;
                    added.push(n);
                    p.changed.insert(n);
                }
            } else if p.visible[n] {
                p.visible[n] = false;
                removed.push(n);
            }
            any
        }
        let mut stack = vec![Rule::Keep; 3];
        walk(self, root, &mut stack, added, removed);
    }

    /// Pending packet; `None` if nothing to send.
    #[allow(clippy::type_complexity)]
    pub fn flush(&mut self) -> Option<(bool, Vec<usize>, Vec<usize>, Vec<usize>)> {
        let first = self.first;
        self.first = false;
        if !first && self.roots.is_empty() && self.changed.is_empty() {
            return None;
        }
        let mut added = Vec::new();
        let mut removed = Vec::new();
        for r in std::mem::take(&mut self.roots) {
            self.update_visibility(r, &mut added, &mut removed);
        }
        let progress: Vec<usize> = std::mem::take(&mut self.changed).into_iter().filter(|&a| self.visible[a]).collect();
        if progress.is_empty() && added.is_empty() && removed.is_empty() {
            return None;
        }
        Some((first, added, removed, progress))
    }

    /// Obtains a criterion; returns (changed, became done).
    pub fn grant(&mut self, a: usize, c: usize) -> (bool, bool) {
        let was = self.done(a);
        if self.got[a][c].is_some() {
            return (false, false);
        }
        self.got[a][c] = Some(now_ms());
        if !self.order.contains(&a) {
            self.order.push(a);
        }
        self.changed.insert(a);
        self.dirty = true;
        let now = self.done(a);
        if !was && now {
            self.mark_root(a);
        }
        (true, !was && now)
    }

    /// Revokes a criterion; returns true if it was obtained.
    pub fn revoke(&mut self, a: usize, c: usize) -> bool {
        let was = self.done(a);
        if self.got[a][c].is_none() {
            return false;
        }
        self.got[a][c] = None;
        self.changed.insert(a);
        self.dirty = true;
        if was && !self.done(a) {
            self.mark_root(a);
        }
        true
    }

    /// JSON file (game form: started advancements, criteria obtained with their date, `done`).
    pub fn to_json(&self, data_version: i32) -> String {
        let mut out = String::from("{\n");
        let mut first = true;
        let mut order: Vec<usize> = self.order.iter().copied().filter(|&a| self.has_progress(a)).collect();
        order.dedup();
        for a in order {
            if !first {
                out.push_str(",\n");
            }
            first = false;
            let adv = &self.tree.list[a];
            out.push_str(&format!("  {}: {{\n    \"criteria\": {{\n", json_str(&adv.id)));
            let mut firstc = true;
            let done: Vec<usize> = (0..adv.criteria.len()).filter(|&c| self.got[a][c].is_some()).collect();
            for c in java_hash_order(&done, |c| adv.criteria[c].name.as_str()) {
                let (crit, Some(ms)) = (&adv.criteria[c], self.got[a][c]) else { continue };
                if !firstc {
                    out.push_str(",\n");
                }
                firstc = false;
                out.push_str(&format!("      {}: {}", json_str(&crit.name), json_str(&format_date(ms))));
            }
            out.push_str(&format!("\n    }},\n    \"done\": {}\n  }}", self.done(a)));
        }
        if !first {
            out.push_str(",\n");
        }
        out.push_str(&format!("  \"DataVersion\": {data_version}\n}}"));
        out
    }
}

/// Iteration order of a Java hash table filled with these keys (capacity 16 doubled
/// beyond 3/4; slot = hash of the mixed string, then insertion order): this is
/// the order of the criteria written by the game.
fn java_hash_order<'a, T: Copy>(items: &[T], key: impl Fn(T) -> &'a str) -> Vec<T> {
    let mut cap = 16usize;
    while items.len() * 4 > cap * 3 {
        cap *= 2;
    }
    let bucket = |s: &str| {
        let h = s.encode_utf16().fold(0i32, |h, u| h.wrapping_mul(31).wrapping_add(i32::from(u))) as u32;
        ((h ^ (h >> 16)) as usize) & (cap - 1)
    };
    let mut v: Vec<(usize, T)> = items.iter().map(|&t| (bucket(key(t)), t)).collect();
    v.sort_by_key(|x| x.0);
    v.into_iter().map(|x| x.1).collect()
}

/// JSON string in Gson style (HTML characters escaped).
fn json_str(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for ch in s.chars() {
        match ch {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\t' => o.push_str("\\t"),
            '\r' => o.push_str("\\r"),
            '<' | '>' | '&' | '=' | '\'' => o.push_str(&format!("\\u{:04x}", ch as u32)),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::UNIX_EPOCH.elapsed().map_or(0, |d| d.as_millis() as i64)
}

/// `yyyy-MM-dd HH:mm:ss Z` in the local time zone.
pub fn format_date(ms: i64) -> String {
    let (y, mo, d, h, mi, s) = crate::storage::local_datetime(ms);
    let off = crate::storage::local_offset_secs() / 60;
    let sign = if off < 0 { '-' } else { '+' };
    let off = off.abs();
    format!("{y:04}-{mo:02}-{d:02} {h:02}:{mi:02}:{s:02} {sign}{:02}{:02}", off / 60, off % 60)
}

/// Inverse of `format_date` (time zone read from the string).
pub fn parse_date(s: &str) -> Option<i64> {
    let (date, rest) = s.split_once(' ')?;
    let (time, zone) = rest.split_once(' ').unwrap_or((rest, "+0000"));
    let mut d = date.split('-').map(|x| x.parse::<i64>());
    let (y, mo, da) = (d.next()?.ok()?, d.next()?.ok()?, d.next()?.ok()?);
    let mut t = time.split(':').map(|x| x.parse::<i64>());
    let (h, mi, se) = (t.next()?.ok()?, t.next()?.ok()?, t.next()?.ok()?);
    let sign = if zone.starts_with('-') { -1 } else { 1 };
    let z = zone.trim_start_matches(['+', '-']);
    let off = if z.len() == 4 { z[..2].parse::<i64>().ok()? * 60 + z[2..].parse::<i64>().ok()? } else { 0 } * sign;
    // Days since the epoch (inverse civil algorithm).
    let yy = if mo <= 2 { y - 1 } else { y };
    let era = yy.div_euclid(400);
    let yoe = yy - era * 400;
    let mp = (mo + 9) % 12;
    let doy = (153 * mp + 2) / 5 + da - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(((days * 86_400 + h * 3600 + mi * 60 + se) - off * 60) * 1000)
}

// ---------------------------------------------------------------------------------------------

impl Server {
    /// Client packets specific to advancements and statistics; true if consumed.
    pub(crate) fn progress_packet(&mut self, p: &[u8]) -> bool {
        use rc_proto::varint::read_varint;
        let ids = self.peer.ids();
        let mut r = rc_proto::Reader::new(p);
        let Ok(id) = read_varint(&mut r) else { return false };
        if id == ids.seen_advancements_out {
            // 0 tab open (+ identifier), 1 screen closed.
            if read_varint(&mut r).ok() == Some(0)
                && let Ok(tab) = rc_proto::read_str(&mut r, 32767)
            {
                let tab = id_of(tab);
                if let Some(a) = self.prog.tree.get(&tab) {
                    self.adv_select_tab(Some(a));
                }
            }
            return true;
        }
        if id == ids.client_command_out {
            let mut r2 = r;
            if read_varint(&mut r2).ok() == Some(1) {
                self.send_stats(); // statistics request
                return true;
            }
        }
        false
    }

    /// Selected tab: displayed root only, send if it changes.
    pub(crate) fn adv_select_tab(&mut self, a: Option<usize>) {
        let new = a.filter(|&a| self.prog.tree.list[a].parent.is_none() && self.prog.tree.list[a].display.is_some());
        if new != self.prog.tab {
            self.prog.tab = new;
            let id = new.map(|a| self.prog.tree.list[a].id.clone());
            let _ = self.send(Event::Progress(rc_client::progress::Progress::SelectTab(id)));
        }
    }

    /// Sending of changes (each tick, the sending of modified data).
    pub(crate) fn adv_flush(&mut self, show: bool) {
        let Some((reset, added, removed, progress)) = self.prog.flush() else { return };
        let tree = self.prog.tree.clone();
        let added = added
            .iter()
            .map(|&a| {
                let x = &tree.list[a];
                adv::Added {
                    id: x.id.clone(),
                    parent: x.parent.map(|p| tree.list[p].id.clone()),
                    display: x.display.as_ref().map(|d| self.adv_display(d)),
                    requirements: x.req_names.clone(),
                    x: x.x,
                    y: x.y,
                }
            })
            .collect();
        let removed = removed.iter().map(|&a| tree.list[a].id.clone()).collect();
        let progress = progress
            .iter()
            .map(|&a| {
                let x = &tree.list[a];
                let crits =
                    x.criteria.iter().enumerate().map(|(c, cr)| (cr.name.clone(), self.prog.got[a][c])).collect();
                (x.id.clone(), crits)
            })
            .collect();
        let u = adv::UpdateAdvancements { reset, added, removed, progress, show };
        let _ = self.send(Event::Progress(rc_client::progress::Progress::Advancements(Box::new(u))));
    }

    fn adv_display(&self, d: &Display) -> adv::Display {
        let icon_stack = Stack {
            id: id_of(d.icon.get("id").and_then(Value::as_str).unwrap_or("minecraft:air")),
            count: d.icon.get("count").and_then(Value::as_i64).unwrap_or(1) as i32,
            components: d.icon.get("components").filter(|c| c.is_object()).map(crate::recipes::json_to_nbt),
        };
        let icon = self.net(Some(&icon_stack)).unwrap_or(rc_proto_v777::item::ItemStack {
            item: 0,
            count: 1,
            added: Vec::new(),
            removed: Vec::new(),
        });
        adv::Display {
            title: Some(text_nbt(&d.title)),
            description: Some(text_nbt(&d.description)),
            icon,
            frame: d.frame,
            background: d.background.clone(),
            toast: d.toast,
            hidden: d.hidden,
        }
    }

    /// Obtains a criterion (`award`): rewards and announcement if the advancement becomes done.
    pub(crate) fn adv_award(&mut self, a: usize, c: usize) -> bool {
        let (changed, completed) = self.prog.grant(a, c);
        if completed {
            let tree = self.prog.tree.clone();
            self.adv_rewards(&tree.list[a].rewards);
            if let Some(d) = &tree.list[a].display
                && d.announce
                && self.rule("minecraft:show_advancement_messages", true)
            {
                let msg = self.adv_announcement(a);
                self.adv_broadcast(msg);
            }
        }
        changed
    }

    pub(crate) fn adv_revoke(&mut self, a: usize, c: usize) -> bool {
        self.prog.revoke(a, c)
    }

    /// Message `chat.type.advancement.<type>` (player name, decorated name of the advancement).
    fn adv_announcement(&self, a: usize) -> rc_text::Component {
        let kind = match self.prog.tree.list[a].display.as_ref().map_or(0, |d| d.frame) {
            1 => "challenge",
            2 => "goal",
            _ => "task",
        };
        let name = self.adv_name(a);
        crate::commands::text::tr(
            &format!("chat.type.advancement.{kind}"),
            vec![self.player_display_name().into(), name.into()],
        )
    }

    /// Decorated name: colored title in brackets, hover title + description.
    pub(crate) fn adv_name(&self, a: usize) -> rc_text::Component {
        let adv = &self.prog.tree.list[a];
        let Some(d) = &adv.display else { return crate::commands::text::lit(adv.id.clone()) };
        let color = if d.frame == 1 { 0xAA00AA } else { 0x55FF55 };
        let title = rc_text::from_nbt(&text_nbt(&d.title)).unwrap_or_else(|| crate::commands::text::lit(""));
        let desc = rc_text::from_nbt(&text_nbt(&d.description)).unwrap_or_else(|| crate::commands::text::lit(""));
        let mut tip = title.clone();
        tip.style.color = Some(color);
        tip.extra.push(crate::commands::text::lit("\n"));
        tip.extra.push(desc);
        let mut t = title;
        crate::commands::text::hover_text(&mut t, tip);
        let mut out = crate::commands::text::brackets(t);
        out.style.color = Some(color);
        out
    }

    /// Broadcast of a system message (all players).
    pub(crate) fn adv_broadcast(&self, c: rc_text::Component) {
        self.chat_out(Event::Chat { component: c, tag: rc_client::ChatTag::System });
    }

    /// Rewards: experience, loot, recipes, function.
    fn adv_rewards(&mut self, r: &Value) {
        if r.is_null() {
            return;
        }
        let xp = r.get("experience").and_then(Value::as_i64).unwrap_or(0) as i32;
        if xp != 0 {
            // W11: level-up of the game (delay of 100 ticks between two sounds).
            self.give_experience_points(xp);
        }
        let tables: Vec<String> = match r.get("loot") {
            Some(Value::String(s)) => vec![id_of(s)],
            Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str().map(id_of)).collect(),
            _ => Vec::new(),
        };
        let mut changes = false;
        for t in tables {
            for st in self.roll_loot(&t) {
                let creative = self.game_mode == 1;
                let left =
                    crate::menus::inventory_add(&mut self.slots, &self.life.items, self.held as usize, st, creative);
                match left {
                    None => {
                        let r1 = self.life.rng.next_float();
                        let r2 = self.life.rng.next_float();
                        self.play_sound(
                            "minecraft:entity.item.pickup",
                            7,
                            self.pos,
                            0.2,
                            ((r1 - r2) * 0.7 + 1.0) * 2.0,
                        );
                        changes = true;
                    }
                    Some(rest) => self.drop_reward(rest),
                }
            }
        }
        if changes {
            self.broadcast_menus();
        }
        let recipes: Vec<String> = r
            .get("recipes")
            .and_then(Value::as_array)
            .map(|a| a.iter().filter_map(|x| x.as_str().map(id_of)).collect())
            .unwrap_or_default();
        if !recipes.is_empty() {
            self.award_recipes(&recipes);
        }
        if let Some(f) = r.get("function").and_then(Value::as_str) {
            let mut src = self.player_source();
            src.silent = true;
            src.level = 2;
            let _ = self.run_functions(&src, &id_of(f), false);
        }
    }

    /// Reward item that does not fit in the inventory: placed in front of the player like a
    /// dropped item (not thrown by hand), pickable at once.
    fn drop_reward(&mut self, stack: Stack) {
        let n = self.life.drops.len();
        self.drop_from_player_with(stack, false);
        if self.life.drops.len() > n
            && let Some(d) = self.life.drops.last_mut()
        {
            d.pickup_delay = 0;
            d.thrower = None;
        }
    }

    /// Draw from a loot table in the "reward" context (`this` = player, origin =
    /// their position).
    pub(crate) fn roll_loot(&mut self, table: &str) -> Vec<Stack> {
        let seq =
            self.life.pack.get("loot_table", table).and_then(|t| t["random_sequence"].as_str().map(str::to_owned));
        // Generator taken out for the time of the draw (the world is lent to the conditions).
        let mut rng = match &seq {
            Some(s) => *self.life.seqs.get(s),
            None => self.life.rng,
        };
        let out = {
            let tags = &self.block_tags;
            let data = &self.data;
            let bt = |tag: &str, name: &str| {
                let b = data.blocks.iter().position(|x| x.name == name).map_or(u32::MAX, |i| i as u32);
                tags.get(tag.trim_start_matches('#')).is_some_and(|v| v.binary_search(&b).is_ok())
            };
            let world = self.loot_world(pred::Ctx {
                this: Some(pred::Who::E(crate::commands::ents::E::Player)),
                origin: Some(self.pos),
                ..Default::default()
            });
            let loot = crate::loot::Loot { pack: &self.life.pack, items: &self.life.items, block_tag: &bt };
            let ctx = crate::loot::Ctx {
                this_type: Some("minecraft:player".to_owned()),
                world: Some(&world),
                ..Default::default()
            };
            loot.drops(table, &ctx, &mut rng)
        };
        match &seq {
            Some(s) => *self.life.seqs.get(s) = rng,
            None => self.life.rng = rng,
        }
        out
    }

    /// "Advancement" predicate of a player: boolean (done) or criteria → boolean.
    pub(crate) fn adv_predicate(&self, id: &str, want: &Value) -> bool {
        let Some(a) = self.prog.tree.get(id) else { return false };
        match want {
            Value::Bool(b) => self.prog.done(a) == *b,
            Value::Object(m) => m.iter().all(|(name, b)| {
                let c = self.prog.tree.list[a].criteria.iter().position(|c| &c.name == name);
                c.is_some_and(|c| self.prog.crit_done(a, c) == b.as_bool().unwrap_or(true))
            }),
            _ => false,
        }
    }

    /// Advancements file to write with the save.
    pub(crate) fn progress_save_jobs(&mut self, jobs: &mut Vec<crate::save::Job>) {
        jobs.push(crate::save::Job::Raw {
            path: self.prog.path.clone(),
            bytes: self.prog.to_json(crate::chunk::DATA_VERSION).into_bytes(),
        });
        self.prog.dirty = false;
        self.stats_save_jobs(jobs);
        if let Some(j) = self.stopwatches_job() {
            jobs.push(j);
        }
    }

    /// Entering the game: first full packet, statistics marked to be sent.
    pub(crate) fn progress_join(&mut self) {
        self.prog.inv_seen = self.slots.clone();
        self.prog.stats.mark_all();
    }

    /// Player tick: periodic triggers, inventory, time statistics, send.
    pub(crate) fn progress_tick(&mut self) {
        self.stats_tick();
        self.trig_tick();
        self.trig_inventory_scan();
        self.adv_flush(true);
    }
}

/// Data pack format of 26.3 (`pack.mcmeta` of `datapack create`).
pub const DATA_PACK_FORMAT: i32 = 121;

/// JSON text component → NBT (plain string if simple text).
pub fn text_nbt(v: &Value) -> Nbt {
    match v {
        Value::String(s) => Nbt::String(s.clone()),
        Value::Null => Nbt::String(String::new()),
        o => crate::recipes::json_to_nbt(o),
    }
}
