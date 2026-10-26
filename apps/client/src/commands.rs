//! Commands in the chat screen (`docs/specs/commands.md` § 2–4): parsing on every keystroke,
//! suggestions (local and server), list above the field, ghost text, coloring,
//! usage and error lines, choice of the packet to send.

use rc_commands::parse::{Completion, Source, completions, signable};
use rc_commands::suggest::{Builder, Suggestion, Suggestions, Tooltip};
use rc_commands::{CmdError, Data, LITERAL_INCORRECT, Parse, Tree, parse};
use rc_text::{Arg, Component};

use crate::chat::Chat;
use crate::gui::Gui;

/// Input field key (independent of winit, for scripted tests).
#[derive(Debug, Clone, PartialEq)]
pub enum Key {
    Enter,
    Escape,
    Tab {
        shift: bool,
    },
    Up,
    Down,
    Left,
    Right,
    Backspace,
    PageUp,
    PageDown,
    Text(String),
    /// Delete, Backspace / Delete by word (Ctrl).
    Delete,
    BackspaceWord,
    DeleteWord,
    /// Home / End (Shift: extended selection).
    Home {
        extend: bool,
    },
    End {
        extend: bool,
    },
    /// Left (−1) / right (+1) arrow, by word with Ctrl, extended selection with Shift.
    Move {
        dir: i32,
        word: bool,
        extend: bool,
    },
    SelectAll,
    Copy,
    Cut,
    Paste,
}

/// What the caller must do after a key.
#[derive(Debug, PartialEq)]
pub enum Outcome {
    None,
    /// Close the chat screen (Esc).
    Close,
    /// Enter on an empty or whitespace-only field: nothing is sent (the ordinary chat
    /// closes, the in-bed one only empties its field).
    Empty,
    /// Send: `Some(command without /)` or message.
    Send {
        command: bool,
        text: String,
    },
}

const LINES: usize = 10;
const FILL: u32 = 0xD000_0000;
const ARG_COLORS: [u32; 5] = [0x55FFFF, 0xFFFF55, 0x55FF55, 0xFF55FF, 0xFFAA00];
const GRAY: u32 = 0xAAAAAA;
const RED: u32 = 0xFF5555;

struct List {
    items: Vec<Suggestion>,
    original: Vec<char>,
    offset: usize,
    current: usize,
    tab_cycles: bool,
    last_mouse: (f32, f32),
    /// Rectangle (x, y, width, height) at the last draw.
    rect: (f32, f32, f32, f32),
}

struct Waiting {
    id: i32,
    input: Vec<char>,
    local: Vec<Suggestions>,
}

#[derive(Default)]
pub struct CommandUi {
    pub tree: Tree,
    pub data: Data,
    pub auto_suggest: bool,
    /// (messages allowed, commands allowed) according to the chat visibility.
    pub allowed: (bool, bool),
    parsed: Option<(Vec<char>, Parse)>,
    is_command: bool,
    is_message: bool,
    waiting: Option<Waiting>,
    ready: Option<Suggestions>,
    list: Option<List>,
    allow: bool,
    keep: bool,
    usage: Vec<Component>,
    usage_at: usize,
    next_id: i32,
    custom: Vec<String>,
    tips: Vec<Option<Component>>,
    /// Request to send to the server: (id, text).
    pub outgoing: Option<(i32, String)>,
    /// Test script (`RUSTCRAFT_CMDTEST`): remaining steps and wait.
    script: Option<(std::collections::VecDeque<String>, i32)>,
    /// Input in a screen field (command block) instead of the chat.
    pub field: Option<FieldAnchor>,
}

/// Screen field that carries the input (field text = input without the leading "/"): list and
/// usage under the field, at `top`, on `lines` lines at most.
#[derive(Debug, Clone, Default)]
pub struct FieldAnchor {
    /// Screen x of each position of the field text (length + 1).
    pub xs: Vec<f32>,
    /// Inner width of the field.
    pub inner: f32,
    pub top: f32,
    pub lines: usize,
    pub fill: u32,
}

/// `RUSTCRAFT_CMDTEST="w40|/|tgi|tab"`: steps separated by `|` played after entering the game.
/// `wN` wait N ticks, `/` open the chat with `/`, `o` open it empty, `tTEXT` type,
/// `tab`, `stab`, `up`, `down`, `left`, `esc`, `enter`, `bs`, `meshflip`, `cap:NAME`.
fn script() -> Option<(std::collections::VecDeque<String>, i32)> {
    let v = rc_data::env_str!("RUSTCRAFT_CMDTEST")?;
    Some((v.split('|').map(str::to_owned).collect(), 0))
}

impl crate::App {
    /// Tick: current data (players, target, settings), request to the server, test script.
    pub(crate) fn commands_tick(&mut self) {
        if self.chat.is_open() {
            self.commands_refresh();
        }
        self.commands_send();
        let Some((steps, wait)) = &mut self.cmd.script else { return };
        if !self.session.link.is_loaded() {
            return;
        }
        if *wait > 0 {
            *wait -= 1;
            return;
        }
        let Some(step) = steps.pop_front() else { return };
        let k = match step.as_str() {
            "/" | "o" => {
                self.chat.open(if step == "/" { "/" } else { "" });
                self.commands_refresh();
                self.cmd.open(&mut self.chat);
                self.keys.clear();
                self.grab(false);
                None
            }
            "tab" => Some(Key::Tab { shift: false }),
            "stab" => Some(Key::Tab { shift: true }),
            "up" => Some(Key::Up),
            "down" => Some(Key::Down),
            "left" => Some(Key::Left),
            "right" => Some(Key::Right),
            "esc" => Some(Key::Escape),
            "enter" => Some(Key::Enter),
            "bs" => Some(Key::Backspace),
            s if s.starts_with('w') => {
                if let Some((_, w)) = &mut self.cmd.script {
                    *w = s[1..].parse().unwrap_or(0);
                }
                None
            }
            s if s.starts_with('t') => Some(Key::Text(s[1..].to_owned())),
            // Clickable text (`links.rs`): `mX,Y` places the mouse (interface units),
            // `click` / `sclick` (Shift) click, `cbtnN` presses button N of the confirmation,
            // `home`, `sall`, `copy`, `paste` edit the field.
            s if s.starts_with('m') && s.contains(',') => {
                let (x, y) = s[1..].split_once(',').unwrap_or_default();
                self.chat.test_mouse = x.parse().ok().zip(y.parse().ok());
                None
            }
            "click" | "sclick" => {
                if let Some(m) = self.chat.test_mouse {
                    if self.chat.confirm.is_some() {
                        let hit = self.chat.confirm.as_ref().and_then(|c| c.hit(m));
                        if let Some(i) = hit {
                            self.confirm_press(i);
                        }
                    } else if self.chat.is_open() {
                        self.chat_click_at(m, step == "sclick");
                    }
                }
                None
            }
            s if s.starts_with("cbtn") => {
                self.confirm_press(s[4..].parse().unwrap_or(0));
                None
            }
            // Frame comparison within a single launch: `meshflip` flips the meshing formats
            // (`remesh::flip_formats`) and re-meshes everything; `cap:NAME` captures the frame
            // (`NAME.bmp` next to the `RUSTCRAFT_CAPTURE` file) without quitting.
            "meshflip" => {
                let keep = self.renderer.as_ref().is_some_and(|r| r.fluid96());
                crate::remesh::flip_formats(keep);
                self.apply_video_options();
                None
            }
            s if s.starts_with("cap:") => {
                let dir = rc_data::env_str!("RUSTCRAFT_CAPTURE").and_then(|v| {
                    std::path::Path::new(v.split_once(';').map_or(v, |x| x.0)).parent().map(std::path::Path::to_path_buf)
                });
                if let (Some(d), Some(r)) = (dir, &mut self.renderer) {
                    r.request_capture(d.join(format!("{}.bmp", &s[4..])));
                }
                None
            }
            "home" => Some(Key::Home { extend: false }),
            "sall" => Some(Key::SelectAll),
            "copy" => Some(Key::Copy),
            "paste" => Some(Key::Paste),
            _ => None,
        };
        if let Some(k) = k {
            self.chat_input_scripted(k);
        }
    }

    fn chat_input_scripted(&mut self, k: Key) {
        if self.chat.is_open() {
            self.commands_refresh();
            self.chat_input(k);
            self.commands_send();
        }
    }

    /// Suggestion request waiting to be sent.
    pub(crate) fn commands_send(&mut self) {
        if let Some((id, text)) = self.cmd.outgoing.take() {
            let _ = self.session.link.send_command_suggestion(id, &text);
        }
    }

    /// Online players, crosshair target, settings (read on opening and on every tick).
    pub(crate) fn commands_refresh(&mut self) {
        let mut names: Vec<String> = self.players.map.values().map(|p| p.name.clone()).collect();
        names.sort();
        self.cmd.data.players = names;
        let (mut block, mut point, mut entity) = (None, None, None);
        match self.pick() {
            crate::pick::Hit::Block { pos, at, .. } => {
                block = Some(pos);
                point = Some(at);
            }
            crate::pick::Hit::Entity { id, .. } => {
                entity = self.entities.map.get(&id).map(|e| uuid_text(e.uuid));
            }
            crate::pick::Hit::Miss => {}
        }
        self.cmd.data.target_block = block;
        self.cmd.data.target_point = point;
        self.cmd.data.target_entity = entity;
        self.cmd.auto_suggest = self.options.b("autoSuggestions");
        let vis = self.options.i("chatVisibility");
        // Multiplayer server: `--disableChat` and chat refused by the account cut the messages.
        let account_ok = self.solo.is_some() || rc_client::session::player_chat_allowed();
        self.cmd.allowed = (vis == 0 && account_ok, vis != 2);
        // Restricted chat prompt: at least one restriction (setting, launcher, account).
        self.chat.restricted = vis != 0 || !account_ok;
    }
}

/// UUID in the game's text format (lowercase, 8-4-4-4-12).
fn uuid_text(u: u128) -> String {
    let h = format!("{u:032x}");
    format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32])
}

fn source(data: &Data) -> Source {
    Source { restricted: true, selectors: data.selectors() }
}

/// Error message (with `… <--[HERE]` context if the position is known).
pub fn error_component(e: &CmdError, input: &[char]) -> Component {
    let msg = Component::translate(e.key, e.args.iter().map(|a| Arg::Text(a.clone())).collect());
    match (e.cursor, e.context(input)) {
        (Some(c), Some(ctx)) => Component::translate(
            "command.context.parse_error",
            vec![Arg::Component(msg), Arg::Text(c.to_string()), Arg::Text(ctx)],
        ),
        _ => msg,
    }
}

/// Start of the last word (after the last run of blanks).
fn last_word(s: &[char]) -> usize {
    let mut r = 0;
    for (i, c) in s.iter().enumerate() {
        if c.is_whitespace() {
            r = i + 1;
        }
    }
    r
}

impl CommandUi {
    /// Built-in registries and blocks known from startup.
    pub fn with_data(d: &rc_data::VersionData) -> Self {
        let mut data = Data::from_version(d);
        // Summonable entity types (approximation: all except player and fishing bobber).
        data.summonable = data
            .registry("minecraft:entity_type")
            .unwrap_or(&[])
            .iter()
            .filter(|e| !matches!(e.as_str(), "minecraft:player" | "minecraft:fishing_bobber"))
            .map(|e| (e.clone(), format!("entity.{}", e.replacen(':', ".", 1))))
            .collect();
        data.sounds = data.registry("minecraft:sound_event").map(<[String]>::to_vec).unwrap_or_default();
        CommandUi {
            data,
            auto_suggest: true,
            allowed: (true, true),
            next_id: -1,
            script: script(),
            ..Default::default()
        }
    }

    pub fn set_tree(&mut self, c: &rc_proto_v777::commands::Commands) {
        self.tree = Tree::new(c);
        self.parsed = None;
    }

    /// Synchronized registry received in configuration (replaces the built-in one of the same name).
    pub fn registry(&mut self, name: &str, entries: &[(String, Option<rc_proto::nbt::Nbt>)]) {
        let full = |e: &str| if e.contains(':') { e.to_owned() } else { format!("minecraft:{e}") };
        self.data.set_registry(name, entries.iter().map(|(e, _)| full(e)).collect());
    }

    /// Tags received: indices resolved in the known registries.
    pub fn tags(&mut self, tags: &rc_proto_v777::play::Tags) {
        for (reg, list) in tags {
            let entries = self.data.registry(reg).map(<[String]>::to_vec).unwrap_or_default();
            let map = list
                .iter()
                .map(|(t, ids)| {
                    let name = if t.contains(':') { t.clone() } else { format!("minecraft:{t}") };
                    (name, ids.iter().filter_map(|&i| entries.get(i as usize).cloned()).collect())
                })
                .collect();
            self.data.set_tags(reg, map);
        }
    }

    pub fn set_completions(&mut self, action: i32, entries: Vec<String>) {
        match action {
            0 => {
                for e in entries {
                    if !self.custom.contains(&e) {
                        self.custom.push(e);
                    }
                }
            }
            1 => self.custom.retain(|c| !entries.contains(c)),
            _ => {
                self.custom.clear();
                for e in entries {
                    if !self.custom.contains(&e) {
                        self.custom.push(e);
                    }
                }
            }
        }
    }

    /// Screen opening: parsing and usage, but no automatic list (§ 4.1).
    pub fn open(&mut self, chat: &mut Chat) {
        self.allow = false;
        self.list = None;
        self.waiting = None;
        self.ready = None;
        self.parsed = None;
        self.update(chat);
    }

    pub fn close(&mut self, chat: &mut Chat) {
        self.list = None;
        self.waiting = None;
        self.usage.clear();
        chat.ghost = None;
        chat.format = None;
    }

    /// Text changed outside the keyboard (click on an insertion, suggestion, IME).
    pub fn text_changed(&mut self, chat: &mut Chat) {
        self.edited(chat);
    }

    /// Text changed: suggestions allowed, parsing restarted.
    fn edited(&mut self, chat: &mut Chat) {
        self.allow = true;
        self.update(chat);
    }

    /// The command information update (§ 3.1).
    pub fn update(&mut self, chat: &mut Chat) {
        let Some(input) = &chat.input else { return };
        let text = input.text.clone();
        let cursor = input.cursor;
        if self.parsed.as_ref().is_some_and(|(t, _)| *t != text) {
            self.parsed = None;
            self.is_command = false;
            self.is_message = false;
        }
        if !self.keep {
            chat.ghost = None;
            self.list = None;
        }
        self.usage.clear();
        // Screen field (command block): always a command, leading "/" optional.
        let slash = self.field.is_some() || text.first() == Some(&'/');
        let min = if self.field.is_some() { usize::from(text.first() == Some(&'/')) } else { 1 };
        if slash {
            let src = source(&self.data);
            if self.parsed.is_none() {
                let p = parse(&self.tree, &text, &self.data, src);
                self.is_command = true;
                self.is_message = !signable(&self.tree, &p, &text).is_empty();
                self.parsed = Some((text.clone(), p));
            }
            if cursor >= min && (self.list.is_none() || !self.keep) {
                let (_, p) = self.parsed.as_ref().expect("parsing");
                let c: Completion = completions(&self.tree, p, &text, cursor, &self.data, src);
                self.ready = None;
                self.waiting = None;
                if c.server {
                    // Pre-incremented id; a new request cancels the previous one.
                    self.next_id += 1;
                    let id = self.next_id;
                    let sent: String = text[..cursor.min(text.len())].iter().collect();
                    self.outgoing = Some((id, sent));
                    self.waiting = Some(Waiting { id, input: text.clone(), local: c.local });
                } else {
                    let s = Suggestions::merge(&text, c.local);
                    self.ready = Some(s.clone());
                    self.usage_info(chat, &s);
                }
            }
        } else if text.iter().any(|c| !c.is_whitespace()) {
            self.parsed = None;
            self.is_message = true;
            let partial = &text[..cursor.min(text.len())];
            let mut b = Builder::new(partial, last_word(partial));
            let mut names: Vec<String> = self.data.players.clone();
            for c in &self.custom {
                if !names.contains(c) {
                    names.push(c.clone());
                }
            }
            b.suggest_matching(names);
            self.ready = Some(b.build());
            self.waiting = None;
            if !self.allowed.0 {
                self.usage.push(chat_key_red("chat_screen.messages_not_allowed"));
            }
            self.usage_at = 0;
        } else {
            self.parsed = None;
            self.ready = None;
            self.waiting = None;
        }
        chat.format = self.format(&text);
    }

    /// Server response (§ 3.2): accepted only for the pending request.
    pub fn server_suggestions(&mut self, chat: &mut Chat, s: rc_proto_v777::commands::ServerSuggestions) {
        let Some(w) = &self.waiting else { return };
        if w.id != s.id {
            return;
        }
        let w = self.waiting.take().expect("pending");
        self.next_id = -1;
        if chat.input.as_ref().is_none_or(|i| i.text != w.input) {
            return;
        }
        let (start, end) = (s.start.max(0) as usize, (s.start + s.length).max(0) as usize);
        self.tips = s.list.iter().map(|(_, t)| t.as_ref().and_then(rc_text::from_nbt)).collect();
        let list: Vec<Suggestion> = s
            .list
            .into_iter()
            .enumerate()
            .map(|(i, (text, tip))| Suggestion { start, end, text, tooltip: tip.map(|_| Tooltip::Server(i)) })
            .collect();
        let server = Suggestions { start, end, list };
        let mut all = w.local;
        all.push(server);
        let merged = Suggestions::merge(&w.input, all);
        self.ready = Some(merged.clone());
        self.usage_info(chat, &merged);
    }

    /// The usage help update (§ 4.4): error or usage lines, then automatic list.
    fn usage_info(&mut self, chat: &mut Chat, sugg: &Suggestions) {
        let Some(input) = &chat.input else { return };
        let Some((text, p)) = &self.parsed else { return };
        let cursor = input.cursor;
        let mut trailing = false;
        if cursor == text.len() {
            if sugg.is_empty() && !p.errors.is_empty() {
                let mut literals = 0;
                for (_, e) in &p.errors {
                    if e.key == LITERAL_INCORRECT {
                        literals += 1;
                    } else {
                        self.usage.push(error_component(e, text));
                    }
                }
                if literals > 0 {
                    let e = CmdError::new("command.unknown.argument", Vec::new(), p.cursor);
                    self.usage.push(error_component(&e, text));
                }
            } else if p.cursor < text.len() {
                trailing = true;
            }
        }
        let (_, parent, start) = p.ctx.suggestion_context(cursor);
        if self.usage.is_empty() {
            let lines: Vec<String> = rc_commands::usage::smart_usage(&self.tree, parent, source(&self.data))
                .into_iter()
                .filter(|(_, lit, _)| !lit)
                .map(|(_, _, u)| u)
                .collect();
            if lines.is_empty() && trailing {
                let e = parse_exception(p);
                self.usage.push(error_component(&e, text));
            }
            self.usage.extend(lines.iter().map(|l| gray(l)));
        }
        if self.is_command && !self.allowed.1 {
            self.usage.push(chat_key_red("chat_screen.commands_not_allowed"));
        }
        if self.is_message && !self.allowed.0 {
            self.usage.push(chat_key_red("chat_screen.messages_not_allowed"));
        }
        self.usage_at = start;
        self.list = None;
        if self.allow && self.auto_suggest {
            self.show(chat);
        }
    }

    /// Opens the list if suggestions are ready.
    fn show(&mut self, chat: &mut Chat) {
        let Some(s) = &self.ready else { return };
        if s.is_empty() {
            return;
        }
        let Some(input) = &chat.input else { return };
        let partial = &input.text[..input.cursor.min(input.text.len())];
        let lw: String = partial[last_word(partial)..].iter().collect::<String>().to_lowercase();
        let (mut first, mut rest): (Vec<Suggestion>, Vec<Suggestion>) = (Vec::new(), Vec::new());
        for x in &s.list {
            if x.text.starts_with(&lw) || rc_data::ids::with_key(&["minecraft:", &lw], |k| x.text.starts_with(k)) {
                first.push(x.clone());
            } else {
                rest.push(x.clone());
            }
        }
        first.extend(rest);
        self.list = Some(List {
            items: first,
            original: input.text.clone(),
            offset: 0,
            current: 0,
            tab_cycles: false,
            last_mouse: (f32::NAN, f32::NAN),
            rect: (0.0, 0.0, 0.0, 0.0),
        });
        self.select(chat, 0);
    }

    fn select(&mut self, chat: &mut Chat, index: i64) {
        let Some(l) = &mut self.list else { return };
        let n = l.items.len() as i64;
        let i = ((index % n) + n) % n;
        l.current = i as usize;
        let applied = l.items[l.current].apply(&l.original);
        let value: String = chat.input.as_ref().map(|i| i.text.iter().collect()).unwrap_or_default();
        chat.ghost = applied.strip_prefix(value.as_str()).map(str::to_owned);
    }

    fn cycle(&mut self, chat: &mut Chat, d: i64) {
        let Some(l) = &self.list else { return };
        let cur = l.current as i64 + d;
        self.select(chat, cur);
        let lines = self.lines();
        let Some(l) = &mut self.list else { return };
        let max_off = l.items.len().saturating_sub(lines);
        let last = l.offset + lines - 1;
        if l.current < l.offset {
            l.offset = l.current.min(max_off);
        } else if l.current > last {
            l.offset = (l.current + 1).saturating_sub(lines).min(max_off);
        }
    }

    /// Applies the selection: range replaced, cursor at the end of the suggestion, list kept.
    fn use_suggestion(&mut self, chat: &mut Chat) {
        let Some(l) = &self.list else { return };
        let s = l.items[l.current].clone();
        let applied: Vec<char> = s.apply(&l.original).chars().collect();
        self.keep = true;
        if let Some(i) = &mut chat.input {
            let cursor = s.start + s.text.chars().count();
            i.set(applied, cursor);
            i.is_draft = false;
        }
        self.edited(chat);
        let cur = self.list.as_ref().map_or(0, |l| l.current as i64);
        self.select(chat, cur);
        self.keep = false;
        if let Some(l) = &mut self.list {
            l.tab_cycles = true;
        }
    }

    /// Key during input.
    pub fn key(&mut self, chat: &mut Chat, key: Key) -> Outcome {
        if self.list.is_some() {
            match key {
                Key::Up => {
                    self.cycle(chat, -1);
                    if let Some(l) = &mut self.list {
                        l.tab_cycles = false;
                    }
                    return Outcome::None;
                }
                Key::Down => {
                    self.cycle(chat, 1);
                    if let Some(l) = &mut self.list {
                        l.tab_cycles = false;
                    }
                    return Outcome::None;
                }
                Key::Tab { shift } => {
                    if self.list.as_ref().is_some_and(|l| l.tab_cycles) {
                        self.cycle(chat, if shift { -1 } else { 1 });
                    }
                    self.use_suggestion(chat);
                    return Outcome::None;
                }
                Key::Escape => {
                    self.list = None;
                    chat.ghost = None;
                    return Outcome::None;
                }
                _ => {}
            }
        } else if let Key::Tab { .. } = key {
            // No "hideable" mode in the chat: Tab opens the list.
            self.show(chat);
            return Outcome::None;
        }
        match key {
            Key::Escape => {
                chat.close_with(crate::chat::Exit::Intentional);
                return Outcome::Close;
            }
            Key::Enter => {
                let ok = !(self.is_message && !self.allowed.0) && (!self.is_command || self.allowed.1);
                if !ok {
                    return Outcome::None;
                }
                return match chat.submit() {
                    Some((command, text)) => Outcome::Send { command, text },
                    None => Outcome::Empty,
                };
            }
            Key::Backspace | Key::BackspaceWord | Key::Delete | Key::DeleteWord => {
                let dir = if matches!(key, Key::Backspace | Key::BackspaceWord) { -1 } else { 1 };
                if chat.delete_text(dir, matches!(key, Key::BackspaceWord | Key::DeleteWord)) {
                    self.edited(chat);
                }
            }
            Key::Left => chat.move_cursor(-1),
            Key::Right => chat.move_cursor(1),
            Key::Move { dir, word, extend } => chat.move_cursor_by(dir, word, extend),
            Key::Home { extend } => chat.home_end(false, extend),
            Key::End { extend } => chat.home_end(true, extend),
            Key::SelectAll => chat.select_all(),
            Key::Copy | Key::Cut => {
                crate::links::set_clipboard(&chat.selected());
                if key == Key::Cut && chat.type_text("") {
                    self.edited(chat);
                }
            }
            Key::Paste => {
                if chat.type_text(&crate::links::get_clipboard()) {
                    self.edited(chat);
                }
            }
            Key::Up | Key::Down => {
                let d = if key == Key::Up { -1 } else { 1 };
                if chat.history_step(d) {
                    self.edited(chat);
                    self.allow = false;
                    self.list = None;
                }
            }
            Key::PageUp => chat.scroll(chat.lines_per_page() as i32 - 1),
            Key::PageDown => chat.scroll(-(chat.lines_per_page() as i32) + 1),
            Key::Text(t) => {
                if chat.type_text(&t) {
                    self.edited(chat);
                }
            }
            Key::Tab { .. } => {}
        }
        Outcome::None
    }

    /// Signable arguments of the command to send (§ 2.6): `chat_command_signed` if there are any.
    pub fn signable_args(&self, command: &str) -> Vec<(String, String)> {
        let input: Vec<char> = std::iter::once('/').chain(command.chars()).collect();
        let p = parse(&self.tree, &input, &self.data, source(&self.data));
        signable(&self.tree, &p, &input)
    }

    /// Coloring (§ 4.3): color per character of the input, `None` for a message.
    fn format(&self, text: &[char]) -> Option<Vec<u32>> {
        let (t, p) = self.parsed.as_ref()?;
        if t != text {
            return None;
        }
        let mut colors = vec![GRAY; text.len()];
        let ctx = p.ctx.last_child();
        let mut k = 0;
        let mut unformatted = 0;
        for &(id, (a, b)) in &ctx.nodes {
            if self.tree.node(id).arg.is_none() {
                continue;
            }
            let color = ARG_COLORS[k % ARG_COLORS.len()];
            k += 1;
            if a >= text.len() {
                break;
            }
            for c in &mut colors[a..b.min(text.len())] {
                *c = color;
            }
            unformatted = b;
        }
        if p.cursor < text.len() {
            let start = p.cursor.max(unformatted.min(p.cursor));
            for c in &mut colors[start..] {
                *c = RED;
            }
        }
        Some(colors)
    }

    /// Click in the list: selects and applies the line.
    pub fn click(&mut self, chat: &mut Chat, m: (f32, f32)) -> bool {
        let Some(l) = &self.list else { return false };
        let (x, y, w, h) = l.rect;
        if !(m.0 >= x && m.0 < x + w && m.1 >= y && m.1 < y + h) {
            return false;
        }
        let line = ((m.1 - y) / 12.0).floor() as usize + l.offset;
        if line < l.items.len() {
            self.select(chat, line as i64);
            self.use_suggestion(chat);
        }
        true
    }

    /// Wheel over the list.
    pub fn wheel(&mut self, m: (f32, f32), d: f32) -> bool {
        let lines = self.lines();
        let Some(l) = &mut self.list else { return false };
        let (x, y, w, h) = l.rect;
        if !(m.0 >= x && m.0 < x + w && m.1 >= y && m.1 < y + h) {
            return false;
        }
        let max = l.items.len().saturating_sub(lines) as i64;
        l.offset = (l.offset as i64 - d.clamp(-1.0, 1.0) as i64).clamp(0, max) as usize;
        true
    }

    /// Drawing: list or usage lines, above the field (§ 4.2, § 4.4).
    /// Suggestion list displayed.
    pub fn has_list(&self) -> bool {
        self.list.is_some()
    }

    /// List lines at most (10 in the chat, adjustable in a screen field).
    fn lines(&self) -> usize {
        self.field.as_ref().map_or(LINES, |f| f.lines.max(1))
    }

    pub fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, chat: &mut Chat, mouse: Option<(f32, f32)>) {
        let Some(input) = &chat.input else { return };
        let h = gui.height;
        let field = self.field.clone();
        let inner = field.as_ref().map_or(gui.width - 4.0, |f| f.inner);
        let fill = field.as_ref().map_or(FILL, |f| f.fill);
        let lines = self.lines();
        let xcar = |gui: &Gui, i: usize| {
            if let Some(f) = &field {
                return f.xs.get(i).or_else(|| f.xs.last()).copied().unwrap_or(0.0);
            }
            4.0 + rc_text::width(
                gui,
                &crate::gui::plain(&input.text[..i.min(input.text.len())].iter().collect::<String>()),
            ) as f32
        };
        gui.next_layer();
        if let Some(l) = &mut self.list {
            let w = l.items.iter().map(|s| rc_text::width(gui, &crate::gui::plain(&s.text))).max().unwrap_or(0) as f32;
            let start = l.items.first().map_or(0, |s| s.start);
            let n = l.items.len().min(lines);
            let (x, y) = match &field {
                // Bordered field: list stuck under `top` (one line higher).
                Some(f) => (xcar(gui, start).clamp(0.0, (xcar(gui, 0) + inner - w).max(0.0)), f.top - 1.0),
                None => (
                    xcar(gui, start).clamp(0.0, (xcar(gui, 0) + inner - w).max(0.0)) - 1.0,
                    h - 12.0 - 3.0 - 12.0 * n as f32,
                ),
            };
            l.rect = (x, y, w + 1.0, 12.0 * n as f32);
            let moved = mouse.is_some_and(|m| m != l.last_mouse);
            if let Some(m) = mouse {
                l.last_mouse = m;
            }
            let (prev, next) = (l.offset > 0, l.items.len() > l.offset + n);
            if prev || next {
                gui.fill(x, y - 1.0, x + w + 1.0, y, fill);
                gui.fill(x, y + 12.0 * n as f32, x + w + 1.0, y + 12.0 * n as f32 + 1.0, fill);
                let dots = |gui: &mut Gui, yy: f32| {
                    let mut i = 0.0;
                    while i < w + 1.0 {
                        gui.fill(x + i, yy, x + i + 1.0, yy + 1.0, 0xFFFF_FFFF);
                        i += 2.0;
                    }
                };
                if prev {
                    dots(gui, y - 1.0);
                }
                if next {
                    dots(gui, y + 12.0 * n as f32);
                }
            }
            let mut hovered = false;
            let mut select = None;
            for i in 0..n {
                let s = &l.items[i + l.offset];
                let ly = y + 12.0 * i as f32;
                gui.fill(x, ly, x + w + 1.0, ly + 12.0, fill);
                if let Some(m) = mouse
                    && m.0 > x
                    && m.0 < x + w + 1.0
                    && m.1 > ly
                    && m.1 < ly + 12.0
                {
                    if moved {
                        select = Some(i + l.offset);
                    }
                    hovered = true;
                }
                let color = if i + l.offset == l.current { 0xFFFF_FF00 } else { 0xFFAA_AAAA };
                gui.text_string(q, &crate::gui::plain(&s.text), x + 1.0, ly + 2.0, color, true);
            }
            if let Some(i) = select {
                self.select(chat, i as i64);
            }
            let Some(l) = &self.list else { return };
            if hovered
                && let Some(m) = mouse
                && let Some(tip) = &l.items[l.current].tooltip
            {
                let c = match tip {
                    Tooltip::Key(k) => Some(Component::translate(k.clone(), Vec::new())),
                    Tooltip::Server(i) => self.tips.get(*i).cloned().flatten(),
                };
                if let Some(c) = c {
                    gui.next_layer();
                    let chars = rc_text::styled_chars(&rc_text::flatten(&c, &gui.lang));
                    crate::widgets::draw_tooltip(gui, q, &[chars], (m.0 as i32, m.1 as i32, 0, 0), Some(m));
                }
            }
            return;
        }
        if self.usage.is_empty() {
            return;
        }
        let lines: Vec<Vec<rc_text::StyledChar>> =
            self.usage.iter().map(|c| rc_text::styled_chars(&rc_text::flatten(c, &gui.lang))).collect();
        let wu = lines.iter().map(|l| rc_text::width(gui, l)).max().unwrap_or(0) as f32;
        let x = xcar(gui, self.usage_at).clamp(0.0, (xcar(gui, 0) + inner - wu).max(0.0));
        for (i, line) in lines.iter().enumerate() {
            let y = field.as_ref().map_or(h - 27.0 - 12.0 * i as f32, |f| f.top + 12.0 * i as f32);
            gui.fill(x - 1.0, y, x + wu + 1.0, y + 12.0, fill);
            gui.text(q, line, x, y + 2.0, 0xFFFF_FFFF, true, 0);
        }
    }
}

/// General error: the single error, otherwise unknown command or argument.
fn parse_exception(p: &Parse) -> CmdError {
    match p.errors.first() {
        Some((_, e)) if p.errors.len() == 1 => e.clone(),
        _ if p.ctx.range.0 == p.ctx.range.1 => CmdError::new("command.unknown.command", Vec::new(), p.cursor),
        _ => CmdError::new("command.unknown.argument", Vec::new(), p.cursor),
    }
}

fn chat_key_red(key: &str) -> Component {
    let mut c = Component::translate(key, Vec::new());
    c.style.color = Some(RED);
    c
}

fn gray(text: &str) -> Component {
    let mut c = Component::text(text);
    c.style.color = Some(GRAY);
    c
}

/// Enter on an empty or whitespace-only field: outcome distinct from Esc (the in-bed chat does not
/// close); a text is sent.
#[test]
fn empty_enter_is_not_escape() {
    let mut ui = CommandUi::default();
    let mut chat = Chat::default();
    for empty in ["", "   "] {
        chat.open(empty);
        assert_eq!(ui.key(&mut chat, Key::Enter), Outcome::Empty, "{empty:?}");
    }
    chat.open("hello");
    assert_eq!(ui.key(&mut chat, Key::Escape), Outcome::Close);
    chat.open("hello");
    assert_eq!(ui.key(&mut chat, Key::Enter), Outcome::Send { command: false, text: "hello".into() });
}
