//! Basic commands: dispatch, game modes, difficulty, time, weather, rules, seed,
//! messages (`say`, `me`, `msg`, `tellraw`, `title`), players (`list`, `help`), `kill`,
//! experience, `give`, `clear`, tags, teleportation, spawn points, sounds,
//! particles, `random`.

use rc_client::Event;
use rc_proto::nbt::Nbt;
use rc_text::{Component, HoverEvent};

use super::ents::E;
use super::src::{Fail, Src};
use super::text::{self, GRAY, jf, lit};
use super::{Parsed, R, Seg, args};
use crate::player::Stack;
use crate::server::Server;
use crate::tr;

/// Tracking of the elements touched ("single" / "multiple" replies of the game).
pub struct Track<T> {
    pub total: i32,
    only: Option<T>,
    count: i32,
    only_nz: Option<T>,
    nz: i32,
}

impl<T: Clone> Default for Track<T> {
    fn default() -> Self {
        Track { total: 0, only: None, count: 0, only_nz: None, nz: 0 }
    }
}

impl<T: Clone> Track<T> {
    pub fn track(&mut self, e: T, v: i32) {
        self.total += v;
        self.count += 1;
        self.only = if self.count == 1 { Some(e.clone()) } else { None };
        if v != 0 {
            self.nz += 1;
            self.only_nz = if self.nz == 1 { Some(e) } else { None };
        }
    }

    /// (single element, count) for the wanted element type (`any`: all, otherwise non-zero ones).
    pub fn pick(&self, any: bool) -> (Option<&T>, i32) {
        if any { (self.only.as_ref(), self.count) } else { (self.only_nz.as_ref(), self.nz) }
    }
}

pub const SOUND_SOURCES: [&str; 11] =
    ["master", "music", "record", "weather", "block", "hostile", "neutral", "player", "ambient", "voice", "ui"];

pub fn mode_id(name: &str) -> i32 {
    match name {
        "creative" => 1,
        "adventure" => 2,
        "spectator" => 3,
        _ => 0,
    }
}

pub fn mode_name(id: i32) -> &'static str {
    match id {
        1 => "creative",
        2 => "adventure",
        3 => "spectator",
        _ => "survival",
    }
}

/// Duration in ticks of a `time` argument (`1d`, `20s`, `5t`).
pub fn time_ticks(s: &str) -> i32 {
    let (num, f) = if let Some(n) = s.strip_suffix('d') {
        (n, 24000.0)
    } else if let Some(n) = s.strip_suffix('s') {
        (n, 20.0)
    } else if let Some(n) = s.strip_suffix('t') {
        (n, 1.0)
    } else {
        (s, 1.0)
    };
    (num.parse::<f32>().unwrap_or(0.0) * f).round() as i32
}

impl Server {
    /// Reply of a tracker: error if no element (if `zero`), single or multiple message.
    pub fn track_feedback<T: Clone>(
        &mut self,
        s: &Src,
        t: &Track<T>,
        any: bool,
        zero: Option<Fail>,
        single: impl Fn(&Server, &T) -> Component,
        multiple: impl Fn(i32) -> Component,
    ) -> R {
        let (first, n) = t.pick(any);
        if n == 0
            && let Some(z) = zero
        {
            return Err(z);
        }
        let msg = match first {
            Some(e) => single(self, e),
            None => multiple(n),
        };
        self.ok(s, msg);
        Ok(t.total)
    }

    /// Players of a `players` argument (singleplayer: the player or nothing).
    pub fn arg_players(&mut self, s: &Src, text: &str) -> Result<Vec<E>, Fail> {
        let v = self.arg_entities(s, text, true)?;
        Ok(v.into_iter().filter(|e| *e == E::Player).collect())
    }

    /// Executing player.
    pub fn source_player(&self, s: &Src) -> Result<E, Fail> {
        match s.entity {
            Some(E::Player) => Ok(E::Player),
            _ => Err(Fail(tr!("permissions.requires.player"))),
        }
    }

    pub fn source_entity(&self, s: &Src) -> Result<E, Fail> {
        s.entity.ok_or_else(|| Fail(tr!("permissions.requires.entity")))
    }

    /// Dispatch of the final commands.
    pub(crate) fn dispatch(&mut self, s: &Src, p: &Parsed, seg: &Seg, path: &str) -> R {
        let head = seg.path.first().map(String::as_str).unwrap_or("");
        let a = |n: &str| p.arg(seg, n).unwrap_or_default();
        match head {
            "gamemode" => {
                let mode = mode_id(&a("gamemode"));
                let targets = match seg.path.get(2) {
                    Some(_) => self.arg_players(s, &a("target"))?,
                    None => vec![self.source_player(s)?],
                };
                let mut n = 0;
                for t in targets {
                    if t == E::Player && self.game_mode != mode {
                        self.set_game_mode(mode);
                        self.gamemode_message(s, mode);
                        n += 1;
                    }
                }
                Ok(n)
            }
            "defaultgamemode" => {
                let mode = mode_id(&a("gamemode"));
                self.level.game_type = mode;
                if let Some(d) = crate::nbtio::get_mut(&mut self.level.root, "Data") {
                    crate::nbtio::set(d, "GameType", Nbt::Int(mode));
                }
                self.ok(s, tr!("commands.defaultgamemode.success", tr!(&format!("gameMode.{}", mode_name(mode)))));
                Ok(0)
            }
            "difficulty" => self.cmd_difficulty(s, seg.path.get(1).map(String::as_str)),
            "seed" => {
                let seed = self.world_seed();
                self.ok(s, tr!("commands.seed.success", text::copy_text(&seed.to_string())));
                Ok(seed as i32)
            }
            "time" => self.cmd_time(s, p, seg, path),
            "weather" => self.cmd_weather(s, p, seg),
            "gamerule" => self.cmd_gamerule(s, p, seg),
            "say" => {
                let msg = self.arg_message(s, &a("message"));
                let c = tr!("chat.type.announcement", s.display.clone(), msg);
                self.chat_broadcast(c);
                Ok(1)
            }
            "me" => {
                let msg = self.arg_message(s, &a("action"));
                let c = tr!("chat.type.emote", s.display.clone(), msg);
                self.chat_broadcast(c);
                Ok(1)
            }
            "msg" => {
                let targets = self.arg_players(s, &a("targets"))?;
                let msg = self.arg_message(s, &a("message"));
                for t in &targets {
                    let mut out = tr!("commands.message.display.outgoing", self.ent_display(*t), msg.clone());
                    out.style.color = Some(GRAY);
                    out.style.italic = Some(true);
                    if s.origin == super::Origin::Player {
                        self.chat_broadcast(out);
                    }
                    let mut inc = tr!("commands.message.display.incoming", s.display.clone(), msg.clone());
                    inc.style.color = Some(GRAY);
                    inc.style.italic = Some(true);
                    self.chat_broadcast(inc);
                }
                Ok(targets.len() as i32)
            }
            "teammsg" => self.cmd_teammsg(s, &a("message")),
            "tellraw" => {
                let targets = self.arg_players(s, &a("targets"))?;
                let mut n = 0;
                for t in targets {
                    let c = self.arg_component(s, &a("message"), Some(t))?;
                    self.system(c);
                    n += 1;
                }
                Ok(n)
            }
            "title" => self.cmd_title(s, p, seg),
            "list" => {
                let name = if seg.path.get(1).map(String::as_str) == Some("uuids") {
                    tr!(
                        "commands.list.nameAndId",
                        Component::text(self.name.clone()),
                        crate::nbtio::uuid_text(self.uuid)
                    )
                } else {
                    self.player_display_name()
                };
                self.ok(s, tr!("commands.list.players", 1, 8, name));
                Ok(1)
            }
            "help" => self.cmd_help(s, p, seg),
            "kill" => {
                let targets = match seg.path.get(1) {
                    Some(_) => self.arg_entities(s, &a("targets"), false)?,
                    None => vec![self.source_entity(s)?],
                };
                let mut t = Track::default();
                for e in targets {
                    let name = self.ent_display(e);
                    self.ent_kill(e);
                    t.track(name, 1);
                }
                self.track_feedback(
                    s,
                    &t,
                    false,
                    None,
                    |_, n| tr!("commands.kill.success.single", n.clone()),
                    |c| tr!("commands.kill.success.multiple", c),
                )
            }
            "experience" => self.cmd_xp(s, p, seg),
            "give" => {
                let targets = self.arg_players(s, &a("targets"))?;
                let count = p.arg(seg, "count").and_then(|c| c.parse().ok()).unwrap_or(1);
                self.cmd_give(s, &a("item"), &targets, count)
            }
            "clear" => self.cmd_clear(s, p, seg),
            "tag" => self.cmd_tag(s, p, seg),
            "teleport" => self.cmd_teleport(s, p, seg),
            "spawnpoint" => self.cmd_spawnpoint(s, p, seg),
            "setworldspawn" => self.cmd_setworldspawn(s, p, seg),
            "playsound" => self.cmd_playsound(s, p, seg),
            "stopsound" => {
                let targets = self.arg_players(s, &a("targets"))?;
                let cat = seg.path.get(2).map(String::as_str).filter(|c| *c != "<sound>");
                let sound = p.arg(seg, "sound").map(|x| args::id(&x));
                let category = match cat {
                    Some("*") | None => None,
                    Some(c) => SOUND_SOURCES.iter().position(|x| x == &c).map(|i| i as i32),
                };
                for _ in &targets {
                    let _ = self.send(Event::StopSound { category, name: sound.clone() });
                }
                let msg = match (category, &sound) {
                    (Some(c), Some(snd)) => {
                        tr!("commands.stopsound.success.source.sound", snd.clone(), SOUND_SOURCES[c as usize])
                    }
                    (Some(c), None) => tr!("commands.stopsound.success.source.any", SOUND_SOURCES[c as usize]),
                    (None, Some(snd)) => tr!("commands.stopsound.success.sourceless.sound", snd.clone()),
                    (None, None) => tr!("commands.stopsound.success.sourceless.any"),
                };
                self.ok(s, msg);
                Ok(targets.len() as i32)
            }
            "particle" => self.cmd_particle(s, p, seg),
            "random" => self.cmd_random(s, p, seg),
            "version" => {
                for c in self.version_lines() {
                    self.tell_source(s, c);
                }
                Ok(1)
            }
            "publish" => self.cmd_publish(s, p, seg), // S9: LAN (`lan/`)
            "unpublish" => {
                if self.lan.is_none() {
                    return Err(Fail(tr!("commands.unpublish.notPublished")));
                }
                // Only the host has the level 4 it asks for (guests: 2 at most): no guest
                // is installed here. Guests sent back ("server stopped") before the success.
                self.lan_unpublish();
                self.ok(s, tr!("commands.unpublish.success"));
                Ok(1)
            }
            "kick" => self.cmd_kick(s, p, seg),
            _ => self.dispatch_more(s, p, seg, path),
        }
    }

    /// Chat message broadcast (singleplayer: to the player), "player" tag.
    pub fn chat_broadcast(&self, c: Component) {
        self.chat_out(Event::Chat { component: c, tag: rc_client::ChatTag::NotSecure });
    }

    fn gamemode_message(&mut self, s: &Src, mode: i32) {
        let m = tr!(&format!("gameMode.{}", mode_name(mode)));
        if s.entity == Some(E::Player) {
            self.ok(s, tr!("commands.gamemode.success.self", m));
        } else {
            if self.rule("minecraft:send_command_feedback", true) {
                self.system(tr!("gameMode.changed", m.clone()));
            }
            self.ok(s, tr!("commands.gamemode.success.other", self.player_display_name(), m));
        }
    }

    /// Mode changed outside a command (F3+F4): "Your game mode…" reply.
    pub(crate) fn change_mode_feedback(&mut self, mode: i32, _: bool) {
        if !(0..4).contains(&mode) || mode == self.game_mode {
            return;
        }
        self.set_game_mode(mode);
        let src = self.player_source();
        self.gamemode_message(&src, mode);
    }

    fn cmd_difficulty(&mut self, s: &Src, which: Option<&str>) -> R {
        let names = ["peaceful", "easy", "normal", "hard"];
        let cur = self.level.difficulty_id();
        let Some(w) = which else {
            self.ok(s, tr!("commands.difficulty.query", tr!(&format!("options.difficulty.{}", names[cur as usize]))));
            return Ok(cur);
        };
        let id = names.iter().position(|n| *n == w).unwrap_or(2) as i32;
        let disp = tr!(&format!("options.difficulty.{w}"));
        if id == cur {
            return Err(Fail(tr!("commands.difficulty.failure", disp)));
        }
        // Lock ignored, but extreme always hard: `/difficulty easy` succeeds there (message
        // included) without changing anything, like the game.
        self.set_difficulty(id, true);
        self.ok(s, tr!("commands.difficulty.success", disp));
        Ok(0)
    }

    /// `/teammsg`: the source entity must be on a team; recipients = itself if it is
    /// a player ("sent") and the players of the same team ("received"); returns their number.
    fn cmd_teammsg(&mut self, s: &Src, text: &str) -> R {
        let e = self.source_entity(s)?;
        let holder = self.ent_score_name(e);
        let Some(team) = self.cmd.scores.team_of(&holder).cloned() else {
            return Err(Fail(tr!("commands.teammsg.failed.noteam")));
        };
        let same = |sv: &Server, name: &str| sv.cmd.scores.team_of(name).is_some_and(|t| t.name == team.name);
        let me = e == E::Player || same(self, &self.name);
        let others: Vec<Option<usize>> =
            self.lan_other_players().into_iter().filter(|(_, n)| same(self, n)).map(|(w, _)| w).collect();
        let n = i32::from(me) + others.len() as i32;
        if n == 0 {
            return Ok(0);
        }
        let msg = self.arg_message(s, text);
        let mut tname = rc_text::from_nbt(&team.display).unwrap_or_else(|| lit(team.name.clone()));
        tname = text::brackets(tname);
        if let Some(c) = &team.color {
            tname.style.color = rc_text::parse_color(c);
        }
        text::hover_text(&mut tname, tr!("chat.type.team.hover"));
        text::suggest_click(&mut tname, "/teammsg ".into());
        let line = |key: &str| tr!(key, tname.clone(), s.display.clone(), msg.clone());
        let chat = |c: Component| Event::Chat { component: c, tag: rc_client::ChatTag::NotSecure };
        if me {
            let key = if e == E::Player { "chat.type.team.sent" } else { "chat.type.team.text" };
            let _ = self.send_to_current(chat(line(key)));
        }
        for who in others {
            let ev = chat(line("chat.type.team.text"));
            self.as_player(who, |sv| sv.send_to_current(ev));
        }
        Ok(n)
    }

    /// Players of a `/kick` argument: `None` the host, `Some(k)` guest `k`. A name (case
    /// ignored) or `@a` also reach the guests; the other selectors, the current player.
    fn kick_targets(&mut self, s: &Src, text: &str) -> Result<Vec<Option<usize>>, Fail> {
        let me = self.lan.as_ref().and_then(|l| l.active);
        let others = self.lan_other_players();
        if text == "@a" {
            let mut all: Vec<Option<usize>> = others.into_iter().map(|(w, _)| w).chain([me]).collect();
            all.sort_unstable(); // host first, then guests by arrival
            return Ok(all);
        }
        if !text.starts_with('@') {
            if super::select::java_eq_ignore_case(text, &self.name) {
                return Ok(vec![me]);
            }
            if let Some((w, _)) = others.iter().find(|(_, n)| super::select::java_eq_ignore_case(text, n)) {
                return Ok(vec![*w]);
            }
        }
        Ok(self.arg_players(s, text)?.into_iter().map(|_| me).collect())
    }

    /// `/kick <targets> [reason]`: open world only; the host is never kicked.
    fn cmd_kick(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let targets = self.kick_targets(s, &p.arg(seg, "targets").unwrap_or_default())?;
        let reason = match p.arg(seg, "reason") {
            Some(r) => self.arg_message(s, &r),
            None => tr!("multiplayer.disconnect.kicked"),
        };
        if self.lan.is_none() {
            return Err(Fail(tr!("commands.kick.singleplayer.failed")));
        }
        let uuids: std::collections::HashMap<usize, u128> =
            self.lan.as_ref().map(|l| l.guests.iter().enumerate().map(|(k, g)| (k, g.uuid)).collect()).unwrap_or_default();
        let mut n = 0;
        for who in targets {
            let Some(k) = who else { continue };
            // Seats move on every departure: guest found again by their profile.
            let Some(uuid) = uuids.get(&k).copied() else { continue };
            let Some(i) = self.lan.as_ref().and_then(|l| l.guests.iter().position(|g| g.uuid == uuid)) else {
                continue;
            };
            let shown = self.as_player(Some(i), |sv| sv.player_display_name());
            self.lan_kick_with(i, reason.clone());
            // Leaves right away ("left the game" before the success, like the game), except
            // for an installed guest, who leaves at the next poll.
            if self.lan.as_ref().is_some_and(|l| l.active.is_none()) {
                self.lan_leave(i);
            }
            self.ok(s, tr!("commands.kick.success", shown, reason.clone()));
            n += 1;
        }
        if n == 0 { Err(Fail(tr!("commands.kick.owner.failed"))) } else { Ok(n) }
    }

    /// Lines of `/version`: header then the game's identity read from the jar (`version.json`).
    fn version_lines(&self) -> Vec<Component> {
        let b = crate::version_info::get(&self.data);
        let b = b.as_ref();
        let protocol = b.map_or(self.data.protocol, |b| b.protocol);
        let or_q = |f: fn(&rc_data::BuildInfo) -> String| b.map_or_else(|| "?".to_owned(), f);
        vec![
            tr!("commands.version.header"),
            tr!("commands.version.id", or_q(|b| b.id.clone())),
            tr!("commands.version.name", or_q(|b| b.name.clone())),
            tr!("commands.version.data", b.map_or(crate::chunk::DATA_VERSION, |b| b.world_version)),
            tr!("commands.version.series", or_q(|b| b.series.clone())),
            tr!("commands.version.protocol", protocol, format!("0x{protocol:x}")),
            // Date shown like the game: text of the Java date, local time zone.
            tr!("commands.version.build_time", lit(or_q(|b| crate::version_info::java_date_local(&b.build_time)))),
            tr!("commands.version.pack.resource", or_q(|b| format!("{}.{}", b.resource_pack.0, b.resource_pack.1))),
            tr!("commands.version.pack.data", or_q(|b| format!("{}.{}", b.data_pack.0, b.data_pack.1))),
            if b.is_none_or(|b| b.stable) {
                tr!("commands.version.stable.yes")
            } else {
                tr!("commands.version.stable.no")
            },
        ]
    }

    fn cmd_weather(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let kind = seg.path.get(1).map(String::as_str).unwrap_or("clear");
        let given = p.arg(seg, "duration").map(|d| time_ticks(&d));
        let dur = |me: &mut Server, lo: i32, hi: i32| match given {
            Some(d) => d,
            None => me.life.rng.int_between(lo, hi),
        };
        let w = match kind {
            "rain" => {
                let d = dur(self, 12000, 24000);
                (0, d, true, false)
            }
            "thunder" => {
                let d = dur(self, 3600, 15600);
                (0, d, true, true)
            }
            _ => {
                let d = dur(self, 12000, 180000);
                (d, 0, false, false)
            }
        };
        let wt = &mut self.life.weather;
        wt.clear_time = w.0;
        wt.rain_time = w.1;
        wt.thunder_time = w.1;
        wt.raining = w.2;
        wt.thundering = w.3;
        self.ok(s, tr!(&format!("commands.weather.set.{kind}")));
        Ok(given.unwrap_or(-1))
    }

    fn cmd_gamerule(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let raw = seg.path.get(1).cloned().unwrap_or_default();
        let id = raw.strip_prefix("minecraft:").unwrap_or(&raw).to_owned();
        let key = format!("minecraft:{id}");
        let is_bool = self.rule_is_bool(&key);
        let current = self.rule_value(&key, is_bool);
        match p.arg(seg, "value") {
            None => {
                self.ok(s, tr!("commands.gamerule.query", id.clone(), current.clone()));
                Ok(if is_bool { i32::from(current == "true") } else { current.parse().unwrap_or(0) })
            }
            Some(v) => {
                // Value read then rewritten (`03` → `3`), compared by value; short name in
                // the message, like the game.
                let v = if is_bool { v } else { v.parse::<i32>().map_or(v, |n| n.to_string()) };
                if v == current {
                    return Err(Fail(tr!("commands.gamerule.not_set", id.clone(), v)));
                }
                let n = if is_bool { Nbt::Byte(i8::from(v == "true")) } else { Nbt::Int(v.parse().unwrap_or(0)) };
                crate::nbtio::set(&mut self.rules, &key, n);
                self.cmd.rules_dirty = true;
                self.after_rule_change(&key);
                self.ok(s, tr!("commands.gamerule.set", id, v.clone()));
                Ok(if is_bool { i32::from(v == "true") } else { v.parse().unwrap_or(0) })
            }
        }
    }

    /// Boolean rule according to the tree (argument `brigadier:bool`).
    fn rule_is_bool(&self, key: &str) -> bool {
        let Some(l) = self.cmd.level(4) else { return true };
        let t = &l.tree;
        let Some(&g) = t.node(t.root).literals.get("gamerule") else { return true };
        let Some(&r) = t.node(g).literals.get(key) else { return true };
        t.node(r)
            .arguments
            .first()
            .is_none_or(|&a| matches!(t.node(a).arg, Some(rc_proto_v777::commands::ArgType::Bool)))
    }

    /// Current value of a rule (default: the game's value).
    pub(crate) fn rule_value(&self, key: &str, is_bool: bool) -> String {
        if let Some(v) = self.rules.get(key) {
            return if is_bool {
                (v.as_i64().unwrap_or(0) != 0 || v.as_str() == Some("true")).to_string()
            } else {
                v.as_i64().map_or_else(|| v.as_str().unwrap_or("0").to_owned(), |i| i.to_string())
            };
        }
        rule_default(key).to_owned()
    }

    /// Immediate effects of a changed rule (like the game's callbacks).
    pub(crate) fn after_rule_change(&mut self, key: &str) {
        match key {
            "minecraft:advance_time" => self.send_time(true),
            "minecraft:reduced_debug_info" => {
                // Each player receives the event on their own entity (guests included).
                let ev = if self.rule(key, false) { 22 } else { 23 };
                self.send_each_player(|id| Event::Entity(rc_client::EntityEvent::Event { id, event: ev }));
            }
            "minecraft:immediate_respawn" => {
                let v = if self.rule(key, false) { 1.0 } else { 0.0 };
                let _ = self.send(Event::GameEvent { kind: 11, param: v });
            }
            "minecraft:limited_crafting" => {
                let v = if self.rule(key, false) { 1.0 } else { 0.0 };
                let _ = self.send(Event::GameEvent { kind: 12, param: v });
            }
            _ => {}
        }
    }

    fn cmd_title(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let targets = self.arg_players(s, &p.arg(seg, "targets").unwrap_or_default())?;
        let what = seg.path.get(2).cloned().unwrap_or_default();
        let mut t: Track<Component> = Track::default();
        for e in &targets {
            match what.as_str() {
                "clear" => {
                    let _ = self.send(Event::ClearTitles(false));
                }
                "reset" => {
                    let _ = self.send(Event::ClearTitles(true));
                }
                "times" => {
                    let g = |n: &str| p.arg(seg, n).map_or(0, |v| time_ticks(&v));
                    let _ = self.send(Event::TitleTimes([g("fadeIn"), g("stay"), g("fadeOut")]));
                }
                _ => {
                    let c = self.arg_component(s, &p.arg(seg, "title").unwrap_or_default(), Some(*e))?;
                    let n = text::to_nbt(&c);
                    let _ = self.send(match what.as_str() {
                        "subtitle" => Event::Subtitle(Some(n)),
                        "actionbar" => Event::ActionBar(c),
                        _ => Event::Title(Some(n)),
                    });
                }
            }
            t.track(self.ent_display(*e), 1);
        }
        let k = match what.as_str() {
            "clear" => "commands.title.cleared",
            "reset" => "commands.title.reset",
            "times" => "commands.title.times",
            "subtitle" => "commands.title.show.subtitle",
            "actionbar" => "commands.title.show.actionbar",
            _ => "commands.title.show.title",
        };
        self.track_feedback(
            s,
            &t,
            false,
            None,
            |_, n| tr!(&format!("{k}.single"), n.clone()),
            |c| tr!(&format!("{k}.multiple"), c),
        )
    }

    fn cmd_help(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let Some(l) = self.cmd.level(s.level) else { return Ok(0) };
        let src = rc_commands::parse::Source { restricted: true, selectors: s.level >= 2 };
        let lines: Vec<String> = match p.arg(seg, "command") {
            None => rc_commands::usage::smart_usage(&l.tree, l.tree.root, src).into_iter().map(|(_, _, u)| u).collect(),
            Some(cmd) => {
                let input: Vec<char> = cmd.chars().collect();
                let pr = rc_commands::parse(&l.tree, &input, &self.cmd.data, src);
                let Some(&(last, _)) = pr.ctx.last_child().nodes.last() else {
                    return Err(Fail(tr!("commands.help.failed")));
                };
                let prefix = cmd.clone();
                rc_commands::usage::smart_usage(&l.tree, last, src)
                    .into_iter()
                    .map(|(_, _, u)| format!("{prefix} {u}"))
                    .collect()
            }
        };
        let n = lines.len() as i32;
        for line in lines {
            self.ok(s, lit(format!("/{line}")));
        }
        Ok(n)
    }

    fn cmd_xp(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let op = seg.path.get(1).cloned().unwrap_or_default();
        let kind = if seg.path.get(4).map(String::as_str) == Some("levels")
            || (op == "query" && seg.path.get(3).map(String::as_str) == Some("levels"))
        {
            "levels"
        } else {
            "points"
        };
        let target = p.arg(seg, "target").unwrap_or_default();
        if op == "query" {
            let e = self.arg_entity(s, &target, true)?;
            let v = &self.life.vit;
            let r = if kind == "levels" {
                v.xp_level
            } else {
                (v.xp_progress * crate::vitals::xp_needed(v.xp_level) as f32).floor() as i32
            };
            self.ok(s, tr!(&format!("commands.experience.query.{kind}"), self.ent_display(e), r));
            return Ok(r);
        }
        let targets = self.arg_players(s, &target)?;
        let amount: i32 = p.arg(seg, "amount").and_then(|v| v.parse().ok()).unwrap_or(0);
        let mut t: Track<Component> = Track::default();
        for e in targets {
            let ok = if op == "add" {
                if kind == "levels" {
                    self.give_experience_levels(amount);
                } else {
                    self.give_experience_points(amount);
                }
                true
            } else if kind == "levels" {
                self.set_levels(amount);
                true
            } else if amount >= crate::vitals::xp_needed(self.life.vit.xp_level) {
                false
            } else {
                self.set_points(amount);
                true
            };
            t.track(self.ent_display(e), i32::from(ok));
        }
        let zero = (op == "set").then(|| Fail(tr!("commands.experience.set.points.invalid")));
        self.track_feedback(
            s,
            &t,
            false,
            zero,
            |_, n| tr!(&format!("commands.experience.{op}.{kind}.success.single"), amount, n.clone()),
            |c| tr!(&format!("commands.experience.{op}.{kind}.success.multiple"), amount, c),
        )
    }

    fn set_levels(&mut self, n: i32) {
        self.life.vit.xp_level = n;
        self.life.vit.sent_xp = None;
        self.send_health();
    }

    /// Progress bounded to (need − 1)/need, total unchanged.
    fn set_points(&mut self, n: i32) {
        let v = &mut self.life.vit;
        let limit = crate::vitals::xp_needed(v.xp_level) as f32;
        v.xp_progress = (n as f32 / limit).clamp(0.0, (limit - 1.0) / limit);
        self.life.vit.sent_xp = None;
        self.send_health();
    }

    /// Displayed name of a stack: brackets, rarity, "item" hover.
    pub fn stack_display(&self, st: &Stack) -> Component {
        let mut inner = lit("");
        let mut name = self.stack_name(st);
        let custom = crate::items::component(st, "minecraft:custom_name").is_some();
        if custom {
            name.style.italic = Some(true);
        }
        inner.extra.push(name);
        let mut c = text::brackets(inner);
        let rarity = self
            .data
            .registry("minecraft:item")
            .and_then(|r| r.entries.iter().position(|e| *e == st.id))
            .and_then(|i| self.data.items.get(i))
            .map_or(0, |i| i.rarity);
        let enchanted = crate::items::component(st, "minecraft:enchantments")
            .is_some_and(|e| matches!(e, Nbt::Compound(f) if !f.is_empty()));
        let rarity = match (rarity, enchanted) {
            (0 | 1, true) => 2,
            (2, true) => 3,
            (r, _) => r,
        };
        c.style.color = Some(match rarity {
            1 => 0xFFFF55,
            2 => 0x55FFFF,
            3 => 0xFF55FF,
            _ => 0xFFFFFF,
        });
        c.style.hover = Some(std::sync::Arc::new(HoverEvent::Item {
            id: st.id.clone(),
            count: st.count,
            components: st.components.clone(),
        }));
        c
    }

    fn cmd_give(&mut self, s: &Src, item: &str, targets: &[E], count: i32) -> R {
        let Some(proto) = self.item_input(item, 1) else { return Err(Fail(tr!("argument.item.id.invalid", item))) };
        let max = self.life.items.max_stack(&proto);
        let allowed = max * 100;
        if count > allowed {
            return Err(Fail(tr!("commands.give.failed.toomanyitems", allowed, self.stack_display(&proto))));
        }
        let mut t: Track<Component> = Track::default();
        for e in targets {
            let mut remaining = count;
            while remaining > 0 {
                let size = max.min(remaining);
                remaining -= size;
                let mut st = proto.clone();
                st.count = size;
                let creative = self.game_mode == 1;
                // Remainder not stored (the game drops what found no room, even after a
                // partial store); in creative everything is absorbed.
                let like = |sv: &Self| -> i32 {
                    sv.slots
                        .iter()
                        .flatten()
                        .filter(|t| t.id == st.id && t.components == st.components)
                        .map(|t| t.count)
                        .sum()
                };
                let before = like(self);
                let _ = crate::menus::inventory_add(
                    &mut self.slots,
                    &self.life.items,
                    self.held as usize,
                    st.clone(),
                    creative,
                );
                let left = if creative { 0 } else { size - (like(self) - before) };
                let rest = (left > 0).then(|| {
                    let mut r = st.clone();
                    r.count = left;
                    r
                });
                match rest {
                    None => {
                        // Dummy item (never pickable, age 5999: disappears on the next
                        // tick) in front of the player, like the game, then the pickup sound.
                        let mut one = proto.clone();
                        one.count = 1;
                        self.give_drop(one, 32767, Some(5999));
                        let pitch = ((self.life.rng.next_float() - self.life.rng.next_float()) * 0.7 + 1.0) * 2.0;
                        self.play_sound("minecraft:entity.item.pickup", 7, self.pos, 0.2, pitch);
                    }
                    // Surplus dropped without pickup delay.
                    Some(r) => self.give_drop(r, 0, None),
                }
            }
            self.broadcast_menus();
            t.track(self.ent_display(*e), 1);
        }
        let disp = self.stack_display(&proto);
        self.track_feedback(
            s,
            &t,
            false,
            None,
            |_, n| tr!("commands.give.success.single", count, disp.clone(), n.clone()),
            |c| tr!("commands.give.success.multiple", count, disp.clone(), c),
        )
    }

    fn cmd_clear(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let targets = match p.arg(seg, "targets") {
            Some(t) => self.arg_players(s, &t)?,
            None => vec![self.source_player(s)?],
        };
        let pred = p.arg(seg, "item").map(|t| super::itempred::ItemPred::parse(&t));
        let max: i32 = p.arg(seg, "maxCount").and_then(|v| v.parse().ok()).unwrap_or(-1);
        let counting = max == 0;
        let mut t: Track<Component> = Track::default();
        for e in targets {
            // Game order: inventory in its internal order (hotbar, storage, armor from the feet
            // to the head, offhand), then crafting grid, then the item on the cursor.
            // The remainder to remove is `maxCount − already removed` (−1 − n stays unlimited).
            let mut n = 0;
            let order = (36..45).chain(9..36).chain([8, 7, 6, 5, 45]).chain(1..5);
            for i in order {
                let Some(st) = self.slots[i].clone() else { continue };
                let r = self.clear_stack(&st, pred.as_ref(), max - n, counting);
                if r > 0 && !counting {
                    let mut ns = st;
                    ns.count -= r;
                    self.slots[i] = (ns.count > 0).then_some(ns);
                }
                n += r;
            }
            let carried = match self.life.menu.as_ref() {
                Some(m) => m.carried.clone(),
                None => self.life.inv_menu.carried.clone(),
            };
            if let Some(st) = carried {
                let r = self.clear_stack(&st, pred.as_ref(), max - n, counting);
                if r > 0 && !counting {
                    let mut ns = st;
                    ns.count -= r;
                    let ns = (ns.count > 0).then_some(ns);
                    match self.life.menu.as_mut() {
                        Some(m) => m.carried = ns,
                        None => self.life.inv_menu.carried = ns,
                    }
                }
                n += r;
            }
            if !counting {
                self.broadcast_menus();
            }
            t.track(self.ent_display(e), n);
        }
        if t.total == 0 {
            let (first, c) = t.pick(true);
            return Err(Fail(match first {
                Some(n) => tr!("clear.failed.single", n.clone()),
                None => tr!("clear.failed.multiple", c),
            }));
        }
        let k = if counting { "commands.clear.test" } else { "commands.clear.success" };
        let total = t.total;
        self.track_feedback(
            s,
            &t,
            false,
            None,
            |_, n| tr!(&format!("{k}.single"), total, n.clone()),
            |c| tr!(&format!("{k}.multiple"), total, c),
        )
    }

    /// Item dropped by `give`: in front of the player,
    /// without a thrower, pickup delay and age imposed.
    fn give_drop(&mut self, stack: Stack, pickup_delay: i32, age: Option<i32>) {
        let before = self.life.drops.len();
        self.drop_from_player_with(stack, false);
        if self.life.drops.len() > before
            && let Some(d) = self.life.drops.last_mut()
        {
            d.thrower = None;
            d.pickup_delay = pickup_delay;
            if let Some(a) = age {
                d.age = a;
            }
        }
    }

    /// Amount removed (or counted) from a stack by `clear`: 0 if it does not match;
    /// `amount` < 0 = the whole stack.
    fn clear_stack(
        &self,
        st: &Stack,
        pred: Option<&super::itempred::ItemPred>,
        amount: i32,
        counting: bool,
    ) -> i32 {
        if st.count <= 0 || pred.is_some_and(|pr| !self.item_pred_test(pr, st)) {
            return 0;
        }
        if counting {
            return st.count;
        }
        if amount < 0 { st.count } else { amount.min(st.count) }
    }

    fn cmd_tag(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let targets = self.arg_entities(s, &p.arg(seg, "targets").unwrap_or_default(), false)?;
        let op = seg.path.get(2).cloned().unwrap_or_default();
        let name = p.arg(seg, "name").unwrap_or_default();
        let mut t: Track<Component> = Track::default();
        match op.as_str() {
            "add" | "remove" => {
                for e in targets {
                    let mut tags = self.ent_tags(e);
                    let changed = if op == "add" {
                        if tags.contains(&name) || tags.len() >= 1024 {
                            false
                        } else {
                            tags.push(name.clone());
                            true
                        }
                    } else if let Some(i) = tags.iter().position(|x| *x == name) {
                        tags.remove(i);
                        true
                    } else {
                        false
                    };
                    if changed {
                        self.ent_set_tags(e, &tags);
                    }
                    t.track(self.ent_display(e), i32::from(changed));
                }
                let zero = Fail(tr!(&format!("commands.tag.{op}.failed")));
                self.track_feedback(
                    s,
                    &t,
                    false,
                    Some(zero),
                    |_, n| tr!(&format!("commands.tag.{op}.success.single"), name.clone(), n.clone()),
                    |c| tr!(&format!("commands.tag.{op}.success.multiple"), name.clone(), c),
                )
            }
            _ => {
                let mut all: Vec<String> = Vec::new();
                for e in targets {
                    let tags = self.ent_tags(e);
                    for x in &tags {
                        if !all.contains(x) {
                            all.push(x.clone());
                        }
                    }
                    t.track(self.ent_display(e), tags.len() as i32);
                }
                let n = all.len() as i32;
                if all.is_empty() {
                    self.track_feedback(
                        s,
                        &t,
                        false,
                        None,
                        |_, d| tr!("commands.tag.list.single.empty", d.clone()),
                        |c| tr!("commands.tag.list.multiple.empty", c),
                    )?;
                } else {
                    // Java set (`HashSet`) then sorted list (trie).
                    all.sort();
                    let list = text::list(all.iter().map(|x| text::colored(lit(x.clone()), text::GREEN)).collect());
                    self.track_feedback(
                        s,
                        &t,
                        false,
                        None,
                        |_, d| tr!("commands.tag.list.single.success", d.clone(), n, list.clone()),
                        |c| tr!("commands.tag.list.multiple.success", c, n, list.clone()),
                    )?;
                }
                Ok(n)
            }
        }
    }

    fn cmd_teleport(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let a = |n: &str| p.arg(seg, n);
        let targets = match a("targets") {
            Some(t) => self.arg_entities(s, &t, false)?,
            None => vec![self.source_entity(s)?],
        };
        if let Some(dest) = a("destination") {
            let d = self.arg_entity(s, &dest, false)?;
            let pos = self.e_pos(d);
            let rot = self.ent_rot(d);
            let mut t: Track<Component> = Track::default();
            for e in targets {
                // Towards an entity: absolute position and rotation (no relative flag).
                self.command_teleport(e, pos, [false; 3], Some(rot), [false; 2]);
                self.after_command_teleport(e, [false; 3]);
                t.track(self.ent_display(e), 1);
            }
            let dn = self.ent_display(d);
            return self.track_feedback(
                s,
                &t,
                false,
                None,
                |_, n| tr!("commands.teleport.success.entity.single", n.clone(), dn.clone()),
                |c| tr!("commands.teleport.success.entity.multiple", c, dn.clone()),
            );
        }
        let loc = a("location").unwrap_or_default();
        let pos = args::vec3(self, s, &loc);
        let bp = [pos[0].floor(), pos[1].floor(), pos[2].floor()];
        // World limits, half-open intervals like the game.
        let inside = |v: f64, m: f64| (-m..m).contains(&v);
        if !inside(bp[0], 30_000_000.0) || !inside(bp[2], 30_000_000.0) || !inside(bp[1], 20_000_000.0) {
            return Err(Fail(tr!("commands.teleport.invalidPosition")));
        }
        let rot_text = a("rotation");
        // Relative axes: `~` per axis, `^` on all three; absent rotation = relative (+0).
        let rel = if args::is_local(&loc) { [true; 3] } else { args::relative_flags(&loc) };
        let rot_rel = rot_text.as_deref().map_or([true; 2], args::rotation_relative);
        let rot_src = rot_text.as_deref().map(|r| args::rotation(s, r));
        // Facing (`facing`): point, or entity and its anchor.
        let look = if seg.path.contains(&"facing".to_owned()) {
            Some(if let Some(fe) = a("facingEntity") {
                let fe = self.arg_entity(s, &fe, false)?;
                let eyes = a("facingAnchor").as_deref() == Some("eyes");
                let mut tp = self.e_pos(fe);
                if eyes {
                    tp[1] += self.ent_eye_height(fe);
                }
                (tp, Some((fe, eyes)))
            } else {
                (args::vec3(self, s, &a("facingLocation").unwrap_or_default()), None)
            })
        } else {
            None
        };
        let mut t: Track<Component> = Track::default();
        for e in targets {
            let rot = rot_src.map(|[y, pi]| [crate::mobs::mth::wrap(y), crate::mobs::mth::wrap(pi)]);
            if e == E::Player && s.dim != self.dim.name {
                // `execute in … run tp`: change of world (S6), the player arrives in the
                // dimension of the source.
                let [yaw, pitch] = rot.unwrap_or([self.yaw, self.pitch]);
                self.teleport_player(crate::dims::Transition {
                    dim: s.dim.clone(),
                    pos,
                    yaw,
                    pitch,
                    relative: 0,
                    sound: false,
                    ticket: false,
                    missing_respawn: false,
                });
            } else {
                self.command_teleport(e, pos, rel, rot_src, rot_rel);
            }
            // Facing computed from the source's anchor (feet except `anchored eyes`).
            if let Some((target, ent)) = look {
                self.command_look_at(e, s.eyes, target, ent);
            }
            self.after_command_teleport(e, rel);
            t.track(self.ent_display(e), 1);
        }
        let f = |v: f64| format!("{v:.6}");
        self.track_feedback(
            s,
            &t,
            false,
            None,
            |_, n| tr!("commands.teleport.success.location.single", n.clone(), f(pos[0]), f(pos[1]), f(pos[2])),
            |c| tr!("commands.teleport.success.location.multiple", c, f(pos[0]), f(pos[1]), f(pos[2])),
        )
    }

    fn cmd_spawnpoint(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let targets = match p.arg(seg, "targets") {
            Some(t) => self.arg_players(s, &t)?,
            None => vec![self.source_player(s)?],
        };
        let pos = match p.arg(seg, "pos") {
            Some(t) => self.spawnable_pos(s, &t)?,
            None => [s.pos[0].floor() as i32, s.pos[1].floor() as i32, s.pos[2].floor() as i32],
        };
        let [y, pi] = p.arg(seg, "rotation").map_or([0.0, 0.0], |r| args::rotation(s, &r));
        let yaw = crate::mobs::mth::wrap(y);
        let pitch = pi.clamp(-90.0, 90.0);
        let dim = s.dim.clone();
        let mut t: Track<Component> = Track::default();
        for e in targets {
            self.life.vit.respawn = Some((dim.clone(), pos, yaw, pitch, true));
            t.track(self.ent_display(e), 1);
        }
        self.track_feedback(
            s,
            &t,
            false,
            None,
            |_, n| {
                tr!(
                    "commands.spawnpoint.success.single",
                    pos[0],
                    pos[1],
                    pos[2],
                    jf(yaw),
                    jf(pitch),
                    dim.clone(),
                    n.clone()
                )
            },
            |c| tr!("commands.spawnpoint.success.multiple", pos[0], pos[1], pos[2], jf(yaw), jf(pitch), dim.clone(), c),
        )
    }

    pub fn spawnable_pos(&self, s: &Src, text: &str) -> Result<[i32; 3], Fail> {
        let p = args::block_pos(self, s, text);
        if p[0].abs() >= 30_000_000 || p[2].abs() >= 30_000_000 || p[1].abs() > 20_000_000 {
            return Err(Fail(tr!("argument.pos.outofbounds")));
        }
        Ok(p)
    }

    fn cmd_setworldspawn(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let pos = match p.arg(seg, "pos") {
            Some(t) => self.spawnable_pos(s, &t)?,
            None => [s.pos[0].floor() as i32, s.pos[1].floor() as i32, s.pos[2].floor() as i32],
        };
        let [yaw, pitch] = p.arg(seg, "rotation").map_or([0.0, 0.0], |r| args::rotation(s, &r));
        // Yaw wrapped, pitch clamped (the game's respawn data).
        let (yaw, pitch) = (crate::mobs::mth::wrap(yaw), pitch.clamp(-90.0, 90.0));
        self.level.spawn_pos = pos;
        self.level.spawn_yaw = yaw;
        self.level.spawn_pitch = pitch;
        self.level.spawn_dimension = s.dim.clone();
        if let Some(d) = crate::nbtio::get_mut(&mut self.level.root, "Data") {
            let sp = Nbt::Compound(vec![
                ("dimension".into(), Nbt::String(s.dim.clone())),
                ("pos".into(), Nbt::IntArray(pos.to_vec())),
                ("yaw".into(), Nbt::Float(yaw)),
                ("pitch".into(), Nbt::Float(pitch)),
            ]);
            crate::nbtio::set(d, "spawn", sp);
        }
        let _ = self.send(Event::SpawnPosition { dimension: s.dim.clone(), pos });
        self.ok(s, tr!("commands.setworldspawn.success", pos[0], pos[1], pos[2], jf(yaw), jf(pitch), s.dim.clone()));
        Ok(1)
    }

    fn cmd_playsound(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let sound = args::id(&p.arg(seg, "sound").unwrap_or_default());
        let cat_name = seg.path.get(2).cloned().unwrap_or_else(|| "master".into());
        let cat = SOUND_SOURCES.iter().position(|x| *x == cat_name).unwrap_or(0) as i32;
        let targets = match p.arg(seg, "targets") {
            Some(t) => self.arg_players(s, &t)?,
            None => s.entity.filter(|e| *e == E::Player).into_iter().collect(),
        };
        let pos = p.arg(seg, "pos").map_or(s.pos, |t| args::vec3(self, s, &t));
        let f = |n: &str, d: f32| p.arg(seg, n).and_then(|v| v.parse::<f32>().ok()).unwrap_or(d);
        let (volume, pitch, min_volume) = (f("volume", 1.0), f("pitch", 1.0), f("minVolume", 0.0));
        let range = if volume > 1.0 { 16.0 * volume } else { 16.0 };
        let max_d2 = f64::from(range * range);
        let seed = self.life.rng.next_long();
        let mut t: Track<Component> = Track::default();
        for e in targets {
            // Player in a different dimension than the source: nothing. Sending cut off = execution in
            // a parked dimension, hence not the player's one.
            if self.muted || s.dim != self.dim.name {
                continue;
            }
            let pp = self.pos;
            let d = [pos[0] - pp[0], pos[1] - pp[1], pos[2] - pp[2]];
            let d2 = d[0] * d[0] + d[1] * d[1] + d[2] * d[2];
            let (mut lp, mut lv) = (pos, volume);
            if d2 > max_d2 {
                if min_volume <= 0.0 {
                    continue;
                }
                let dist = d2.sqrt();
                lp = [pp[0] + d[0] / dist * 2.0, pp[1] + d[1] / dist * 2.0, pp[2] + d[2] / dist * 2.0];
                lv = min_volume;
            }
            // Always a direct sound with variable range, even when the registry knows it (the
            // game does not take the fixed range of the registry entry). To the targeted player only.
            let _ = self.send_to_current(Event::Sound(rc_proto_v777::play::SoundPacket {
                sound: rc_proto_v777::play::SoundRef::Named(sound.clone()),
                category: cat,
                pos: Some(lp),
                entity: None,
                volume: lv,
                pitch,
                seed,
            }));
            t.track(self.ent_display(e), 1);
        }
        self.track_feedback(
            s,
            &t,
            false,
            Some(Fail(tr!("commands.playsound.failed"))),
            |_, n| tr!("commands.playsound.success.single", sound.clone(), n.clone()),
            |c| tr!("commands.playsound.success.multiple", sound.clone(), c),
        )
    }

    fn cmd_particle(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let name_text = p.arg(seg, "name").unwrap_or_default();
        let brace = name_text.find('{').unwrap_or(name_text.len());
        let name: String = args::id(&name_text[..brace]);
        let opts = super::snbt::parse(&name_text[brace..]).unwrap_or(Nbt::Compound(Vec::new()));
        let particle = self.particle_options(&name, &opts)?;
        let pos = p.arg(seg, "pos").map_or(s.pos, |t| args::vec3(self, s, &t));
        // `delta`: world coordinates without recentering, resolved against the source like the
        // game (`~1 ~1 ~1` = source position + 1).
        let delta = p.arg(seg, "delta").map_or([0.0; 3], |t| args::vec3_exact(self, s, &t));
        let speed: f32 = p.arg(seg, "speed").and_then(|v| v.parse().ok()).unwrap_or(0.0);
        let count: i32 = p.arg(seg, "count").and_then(|v| v.parse().ok()).unwrap_or(0);
        let force = seg.path.iter().any(|w| w == "force");
        let viewers = match p.arg(seg, "viewers") {
            Some(v) => self.arg_players(s, &v)?,
            None => vec![E::Player],
        };
        let mut n = 0;
        for _ in viewers {
            // Distance measured from the center of the player's block (like the game).
            let d2 = (0..3).map(|k| (self.pos[k].floor() + 0.5 - pos[k]).powi(2)).sum::<f64>();
            let limit = if force { 512.0 * 512.0 } else { 32.0 * 32.0 };
            if d2 >= limit {
                continue;
            }
            let _ = self.send(Event::Particles(rc_proto_v777::entity::LevelParticles {
                particle: particle.clone(),
                force,
                always_visible: false,
                pos,
                delta: [delta[0] as f32, delta[1] as f32, delta[2] as f32],
                speed: [speed, 0.0, 0.0],
                count,
                mode: 0,
            }));
            n += 1;
        }
        if n == 0 {
            return Err(Fail(tr!("commands.particle.failed")));
        }
        self.ok(s, tr!("commands.particle.success", name));
        Ok(n)
    }

    /// Particle and its options (`name{…}`), decoded like the game's codecs (see
    /// `decode_particle`).
    fn particle_options(&self, name: &str, o: &Nbt) -> Result<rc_proto_v777::entity::Particle, Fail> {
        let kind = self
            .data
            .registry("minecraft:particle_type")
            .and_then(|r| r.entries.iter().position(|e| e == name))
            .map_or(0, |i| i as i32);
        decode_particle(kind, name, o, &|v| self.states.state_of(&self.data, v).map(|s| s as i32), &|id| {
            self.data
                .registry("minecraft:item")
                .and_then(|r| r.entries.iter().position(|e| e == id))
                .map(|i| i as i32)
                .filter(|&i| i != 0)
        })
        .map_err(|m| Fail(tr!("particle.invalidOptions", m)))
    }

    fn cmd_random(&mut self, s: &Src, p: &Parsed, seg: &Seg) -> R {
        let op = seg.path.get(1).cloned().unwrap_or_default();
        if op == "reset" {
            // Optional salt and flags (absent flags count as true).
            let flag = |k: &str| p.arg(seg, k).is_none_or(|v| v != "false");
            let custom = p
                .arg(seg, "seed")
                .and_then(|v| v.parse::<i32>().ok())
                .map(|salt| (salt, flag("includeWorldSeed"), flag("includeSequenceId")));
            let Some(seq) = p.arg(seg, "sequence") else {
                // `reset *`: forget everything (and change the defaults if a salt is given).
                let n = self.life.seqs.clear(custom);
                self.ok(s, tr!("commands.random.reset.all.success", n as i32));
                return Ok(n as i32);
            };
            let seq = args::id(&seq);
            self.life.seqs.reset(&seq, custom);
            self.ok(s, tr!("commands.random.reset.success", seq));
            return Ok(1);
        }
        let r = super::select::read_range(&p.arg(seg, "range").unwrap_or_default()).unwrap_or_default();
        let min = r.min.map_or(i32::MIN, |v| v as i32);
        let max = r.max.map_or(i32::MAX, |v| v as i32);
        let span = i64::from(max) - i64::from(min);
        if span == 0 {
            return Err(Fail(tr!("commands.random.error.range_too_small")));
        }
        if span >= 2_147_483_647 {
            return Err(Fail(tr!("commands.random.error.range_too_large")));
        }
        let v = match p.arg(seg, "sequence") {
            Some(seq) => self.life.seqs.get(&args::id(&seq)).int_between(min, max),
            None => self.life.rng.int_between(min, max),
        };
        if op == "roll" {
            self.system(tr!("commands.random.roll", s.display.clone(), v, min, max));
        } else {
            self.ok(s, tr!("commands.random.sample.success", v));
        }
        Ok(v)
    }

    fn cmd_time(&mut self, s: &Src, p: &Parsed, seg: &Seg, path: &str) -> R {
        let w: Vec<&str> = path.split(' ').collect();
        if w.get(1) == Some(&"query") && w.get(2) == Some(&"gametime") {
            self.ok(s, tr!("commands.time.query.gametime", self.time));
            return Ok((self.time % 2_147_483_647) as i32);
        }
        // Clock: `time of <clock> …` or the dimension's default clock.
        let (clock, rest) = if w.get(1) == Some(&"of") {
            (args::id(&p.arg(seg, "clock").unwrap_or_default()), &w[3..])
        } else {
            let def = self.data.dimension_types.iter().find(|t| t.name == s.dim).map(|t| t.default_clock.clone());
            match def.filter(|c| !c.is_empty()) {
                Some(c) => (c, &w[1..]),
                None => return Err(Fail(tr!("commands.time.no_default_clock", s.dim.clone()))),
            }
        };
        let ticks = self.clocks.iter().find(|c| c.0 == clock).map_or(0, |c| c.1);
        match rest {
            ["set", "<time>"] => {
                let v = time_ticks(&p.arg(seg, "time").unwrap_or_default());
                if ticks == i64::from(v) {
                    return Err(Fail(tr!("commands.time.set.already_at_time", clock, v)));
                }
                self.set_clock(&clock, i64::from(v));
                self.ok(s, tr!("commands.time.set.absolute", clock, v));
                Ok(v)
            }
            ["set", "<timemarker>"] => {
                let marker = args::id(&p.arg(seg, "timemarker").unwrap_or_default());
                let Some((mt, period)) = self.cmd_time_marker(&clock, &marker) else {
                    return Err(Fail(tr!("commands.time.no_time_marker_found", marker, clock)));
                };
                let occurs = match period {
                    Some(per) => ticks % i64::from(per) == i64::from(mt),
                    None => ticks == i64::from(mt),
                };
                if occurs {
                    return Err(Fail(tr!("commands.time.set.already_at_time_marker", clock, marker)));
                }
                let to = match period {
                    Some(per) => {
                        let from = ticks % i64::from(per);
                        let d = i64::from(mt) - from;
                        ticks + if d > 0 { d } else { i64::from(per) + d }
                    }
                    None => i64::from(mt),
                };
                self.set_clock(&clock, to);
                self.ok(s, tr!("commands.time.set.time_marker", clock, marker));
                Ok((to % 2_147_483_647) as i32)
            }
            ["add", _] => {
                let v = time_ticks(&p.arg(seg, "time").unwrap_or_default());
                let to = (ticks + i64::from(v)).max(0);
                self.set_clock(&clock, to);
                self.ok(s, tr!("commands.time.set.absolute", clock, to));
                Ok((to % 2_147_483_647) as i32)
            }
            ["query", "time"] => {
                self.ok(s, tr!("commands.time.query.absolute", clock, ticks));
                Ok((ticks % 2_147_483_647) as i32)
            }
            ["query", "<timeline>", tail @ ..] => {
                let tl = args::id(&p.arg(seg, "timeline").unwrap_or_default());
                let Some(def) = self.data.timelines.iter().find(|t| t.name == tl) else {
                    return Err(Fail(tr!("argument.resource.not_found", tl, "minecraft:timeline")));
                };
                if def.clock != clock {
                    return Err(Fail(tr!("commands.time.wrong_timeline_for_clock", tl, clock)));
                }
                let per = i64::from(def.period.max(1));
                if tail.first() == Some(&"repetition") {
                    // Timeline without period: 0 repetitions (the game), quotient truncated to int.
                    let r = if def.period > 0 { i64::from((ticks / per) as i32) } else { 0 };
                    self.ok(s, tr!("commands.time.query.timeline.repetitions", tl, r));
                    Ok((r % 2_147_483_647) as i32)
                } else {
                    let v = if def.period > 0 { ticks % per } else { ticks };
                    self.ok(s, tr!("commands.time.query.timeline", tl, v));
                    Ok((v % 2_147_483_647) as i32)
                }
            }
            ["pause"] | ["resume"] => {
                let pause = rest[0] == "pause";
                let paused = self.cmd.clock_paused.contains(&clock);
                if paused == pause {
                    let k = if pause {
                        "commands.time.pause.already_paused"
                    } else {
                        "commands.time.pause.already_running"
                    };
                    return Err(Fail(tr!(k, clock)));
                }
                if pause {
                    self.cmd.clock_paused.insert(clock.clone());
                } else {
                    self.cmd.clock_paused.remove(&clock);
                }
                self.send_time(true);
                self.ok(s, tr!(if pause { "commands.time.pause" } else { "commands.time.resume" }, clock));
                Ok(1)
            }
            ["rate", _] => {
                let r: f32 = p.arg(seg, "rate").and_then(|v| v.parse().ok()).unwrap_or(1.0);
                let cur = self.cmd.clock_rate.get(&clock).copied().unwrap_or(1.0);
                if cur == r {
                    return Err(Fail(tr!("commands.time.rate.already_same", clock, jf(r))));
                }
                self.cmd.clock_rate.insert(clock.clone(), r);
                self.send_time(true);
                self.ok(s, tr!("commands.time.rate", clock, jf(r)));
                Ok(1)
            }
            _ => Err(Fail(tr!("command.unknown.command"))),
        }
    }

    fn set_clock(&mut self, clock: &str, v: i64) {
        if let Some(c) = self.clocks.iter_mut().find(|c| c.0 == clock) {
            c.1 = v;
        } else {
            self.clocks.push((clock.to_owned(), v));
        }
        self.send_time(true);
    }

    /// Time marker of a clock: (ticks, period of its timeline).
    fn cmd_time_marker(&self, clock: &str, marker: &str) -> Option<(i32, Option<u32>)> {
        self.cmd.time_markers.iter().find(|m| m.clock == clock && m.id == marker).map(|m| (m.ticks, m.period))
    }
}

/// Default values of the game's rules (26.3), used when the world does not write them.
pub fn rule_default(key: &str) -> &'static str {
    match key.trim_start_matches("minecraft:") {
        "advance_time"
        | "advance_weather"
        | "allow_entering_nether_using_portals"
        | "block_drops"
        | "block_explosion_drop_decay"
        | "command_block_output"
        | "command_blocks_work"
        | "drowning_damage"
        | "elytra_movement_check"
        | "ender_pearls_vanish_on_death"
        | "entity_drops"
        | "fall_damage"
        | "fire_damage"
        | "forgive_dead_players"
        | "freeze_damage"
        | "global_sound_events"
        | "locator_bar"
        | "log_admin_commands"
        | "mob_drops"
        | "mob_explosion_drop_decay"
        | "mob_griefing"
        | "natural_health_regeneration"
        | "player_movement_check"
        | "projectiles_can_break_blocks"
        | "pvp"
        | "raids"
        | "send_command_feedback"
        | "show_advancement_messages"
        | "show_death_messages"
        | "spawn_mobs"
        | "spawn_monsters"
        | "spawn_patrols"
        | "spawn_phantoms"
        | "spawn_wandering_traders"
        | "spawn_wardens"
        | "spawner_blocks_work"
        | "spectators_generate_chunks"
        | "spread_vines"
        | "tnt_explodes"
        | "water_source_conversion" => "true",
        "immediate_respawn"
        | "keep_inventory"
        | "limited_crafting"
        | "reduced_debug_info"
        | "lava_source_conversion"
        | "tnt_explosion_drop_decay"
        | "universal_anger" => "false",
        "max_entity_cramming" => "24",
        "max_command_sequence_length" => "65536",
        "max_command_forks" => "65536",
        "max_block_modifications" => "32768",
        "max_snow_accumulation_height" => "1",
        "players_nether_portal_creative_delay" => "0",
        "players_nether_portal_default_delay" => "80",
        "players_sleeping_percentage" => "100",
        "random_tick_speed" => "3",
        "respawn_radius" => "10",
        "fire_spread_radius_around_player" => "128",
        _ => "false",
    }
}

/// Particle options decoded like the game's codecs: integer color or floats 0–1
/// (`floor(x × 255)`), block state (compound `Name`/`Properties` or identifier), item
/// (identifier or compound `id`), bounded scale, power, delay. Shared by execution
/// (`particle_options`) and parsing (`check_particle`): the game refuses at parse time.
fn decode_particle(
    kind: i32,
    name: &str,
    o: &Nbt,
    block_state: &dyn Fn(&Nbt) -> Option<i32>,
    item_index: &dyn Fn(&str) -> Option<i32>,
) -> Result<rc_proto_v777::entity::Particle, String> {
    let mut part =
        rc_proto_v777::entity::Particle { kind, color: 0, state: 0, item: -1, scale: 1.0, color2: 0, vib: None };
    let invalid = |m: String| m;
    let field =
        |k: &str| o.get(k).ok_or_else(|| invalid(format!("No key {k} in MapLike[{}]", super::snbt::to_snbt(o))));
    let float = |k: &str| -> Result<f32, String> {
        crate::mobs::mob::f64_of(field(k)?).map(|v| v as f32).ok_or_else(|| {
            invalid(format!("Not a number: {}", super::snbt::to_snbt(&o.get(k).cloned().unwrap_or(Nbt::Byte(0)))))
        })
    };
    let color = |k: &str, alpha: bool| -> Result<u32, String> {
        let v = field(k)?;
        if let Some(i) = v.as_i64().filter(|_| !matches!(v, Nbt::Float(_) | Nbt::Double(_))) {
            return Ok(i as i32 as u32);
        }
        let n = if alpha { 4 } else { 3 };
        let f: Vec<f32> = match v {
            Nbt::List(l) => l.iter().filter_map(|x| crate::mobs::mob::f64_of(x).map(|x| x as f32)).collect(),
            _ => Vec::new(),
        };
        if f.len() != n {
            return Err(invalid(format!("Input is not a list of {n} elements")));
        }
        let ch = |x: f32| ((x * 255.0).floor() as i32 & 0xFF) as u32;
        let a = if alpha { ch(f[3]) } else { 255 };
        Ok(a << 24 | ch(f[0]) << 16 | ch(f[1]) << 8 | ch(f[2]))
    };
    let scale = |k: &str| -> Result<f32, String> {
        let v = float(k)?;
        if !(0.01..=4.0).contains(&v) {
            return Err(invalid(format!("Value must be within range [0.01;4.0]: {v}")));
        }
        Ok(v)
    };
    match name.trim_start_matches("minecraft:") {
        "block" | "block_marker" | "falling_dust" | "dust_pillar" | "block_crumble" => {
            let v = field("block_state")?;
            part.state = block_state(v).ok_or_else(|| invalid("Unknown block".into()))?;
        }
        "dust" => {
            part.color = color("color", false)?;
            part.scale = scale("scale")?;
        }
        "dust_color_transition" => {
            part.color = color("from_color", false)?;
            part.color2 = color("to_color", false)?;
            part.scale = scale("scale")?;
        }
        "entity_effect" | "tinted_leaves" | "flash" => part.color = color("color", true)?,
        "effect" | "instant_effect" => {
            part.color = if o.get("color").is_some() { color("color", false)? } else { u32::MAX };
            part.scale = if o.get("power").is_some() { float("power")? } else { 1.0 };
        }
        "dragon_breath" => part.scale = if o.get("power").is_some() { float("power")? } else { 1.0 },
        "sculk_charge" => part.scale = float("roll")?,
        "shriek" => part.state = field("delay")?.as_i64().ok_or_else(|| invalid("Not a number".into()))? as i32,
        "item" => {
            let v = field("item")?;
            let id = v.as_str().map(str::to_owned).or_else(|| v.get("id").and_then(Nbt::as_str).map(str::to_owned));
            let id = id.map(|i| args::id(&i)).ok_or_else(|| invalid("Not a string".into()))?;
            part.item = item_index(&id).ok_or_else(|| {
                invalid(format!("Unknown registry key in ResourceKey[minecraft:root / minecraft:item]: {id}"))
            })?;
        }
        _ => {}
    }
    Ok(part)
}

/// Parse-time check of particle options (`name{…}` or `name` alone): the game decodes them
/// during parsing, so an invalid input never executes (not even in a function).
pub fn check_particle(d: &rc_commands::Data, text: &str) -> Result<(), String> {
    let brace = text.find('{').unwrap_or(text.len());
    let name = args::id(&text[..brace]);
    let opts = super::snbt::parse(&text[brace..]).unwrap_or(Nbt::Compound(Vec::new()));
    let kind =
        d.registry("minecraft:particle_type").and_then(|r| r.iter().position(|e| *e == name)).map_or(0, |i| i as i32);
    decode_particle(
        kind,
        &name,
        &opts,
        &|v| {
            let v = crate::nbtio::unwrap_hetero(v);
            let id = match v {
                Nbt::String(s) => s.as_str(),
                Nbt::Compound(_) => v.get("id").or_else(|| v.get("Name")).and_then(Nbt::as_str)?,
                _ => return None,
            };
            d.blocks.contains_key(id).then_some(0)
        },
        &|id| {
            d.registry("minecraft:item")
                .and_then(|r| r.iter().position(|e| e == id))
                .map(|i| i as i32)
                .filter(|&i| i != 0)
        },
    )
    .map(|_| ())
}
