//! On-screen chat (`docs/specs/chat.md` § 3–4): history, split lines, fade, sidebar,
//! action bar, input field (selection, clipboard, drafts), settings
//! (scale, width, heights, line spacing, delay, colours, filters), clickable and
//! hoverable text (`links.rs`).

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::{Duration, Instant};

use rc_client::ChatTag;
use rc_text::{CharStyle, Component, HoverEvent, StyledChar};

use crate::gui::Gui;

const MAX: usize = 100;
pub const MAX_INPUT: usize = 256;
/// Id of the internal action "next message of the queue" (`chat.queue` line).
pub const QUEUE_EXPAND: &str = "minecraft:internal/expand_chat_queue";
/// Id of the internal action "restrictions screen" (prompt of the restricted chat).
pub const GO_TO_RESTRICTIONS: &str = "minecraft:internal/go_to_restrictions_screen";

/// Origin of a message: visibility filter (`chatVisibility`) reapplied to the messages
/// already received when the option changes.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Source {
    /// Message from the client itself: always visible.
    Client,
    /// Player message (or disguised): visible only with "Shown".
    Player,
    /// Server system message: hidden only with "Hidden".
    Server,
}

impl Source {
    fn of(tag: ChatTag) -> Source {
        if tag == ChatTag::System { Source::Server } else { Source::Player }
    }

    fn visible(self, visibility: i32) -> bool {
        match self {
            Source::Client => true,
            Source::Player => visibility == 0,
            Source::Server => visibility != 2,
        }
    }
}

struct Message {
    component: Component,
    tick: i64,
    tag: ChatTag,
    source: Source,
    /// Signature of a signed player message (for `delete_chat`).
    signature: Option<rc_client::chat::Signature>,
    /// Original signed text of an "edited" message (tag tooltip).
    original: Option<Arc<str>>,
}

struct Line {
    chars: Vec<StyledChar>,
    tick: i64,
    tag: ChatTag,
    original: Option<Arc<str>>,
    /// Last line of the message (carries the tag icon).
    end: bool,
    /// Continuation line: `chars[0]` is the indent (one space). Like the game, the rest is
    /// reordered alone and the indent stays on the left, even in a right-to-left paragraph.
    indent: bool,
}

/// Visual order of a line (`None`: as is).
type OrderOf<'g> = dyn Fn(&[StyledChar]) -> Option<Arc<[(char, u32)]>> + 'g;

impl Line {
    /// Indent (empty or one space), then the rest of the line.
    fn parts(&self) -> (&[StyledChar], &[StyledChar]) {
        self.chars.split_at(usize::from(self.indent))
    }

    /// Drawing: indent then rest, each in its visual order.
    fn draw(&self, gui: &mut Gui, q: &wgpu::Queue, y: f32, color: u32) {
        let (head, rest) = self.parts();
        let x = gui.text(q, head, 0.0, y, color, true, 0);
        gui.text(q, rest, x, y, color, true, 0);
    }

    fn style_at<'a>(&'a self, gui: &Gui, x: f32) -> Option<&'a CharStyle> {
        line_style_in(gui, self, &|c| gui.visual_order_of(c), x)
    }

    /// Drawn width (indent included).
    fn width(&self, gui: &Gui) -> i32 {
        let (head, rest) = self.parts();
        gui.visual_width(head) + gui.visual_width(rest)
    }
}

/// Style under the abscissa `x` of a line laid out like [`Line::draw`].
fn line_style_in<'a>(m: &dyn rc_text::Metrics, l: &'a Line, order_of: &OrderOf<'_>, x: f32) -> Option<&'a CharStyle> {
    let (head, rest) = l.parts();
    let w: f32 = head.iter().map(|c| m.advance(&c.style.font, c.c).0).sum();
    if x < w {
        return crate::links::style_in(m, head, order_of(head).as_deref(), x);
    }
    crate::links::style_in(m, rest, order_of(rest).as_deref(), x - w)
}

/// Queued message (chat delay): component, tag, signature, original text.
type Queued = (Component, ChatTag, Option<rc_client::chat::Signature>, Option<Arc<str>>);

/// Chat settings (`options.txt`), applied by [`Chat::apply_options`].
#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub scale: f64,
    pub width: f64,
    pub height_focused: f64,
    pub height_unfocused: f64,
    pub spacing: f64,
    /// Delay between player messages, in seconds.
    pub delay: f64,
    pub colors: bool,
    pub links: bool,
    pub links_prompt: bool,
    pub hide_matched_names: bool,
    pub only_secure: bool,
    pub save_drafts: bool,
    /// `chatVisibility`: 0 all, 1 commands only, 2 hidden.
    pub visibility: i32,
    /// Opacity of the text (`chatOpacity × 0.9 + 0.1`) and of the background.
    pub text_opacity: f32,
    pub background: f32,
    /// Background of the input field: `backgroundForChatOnly` (otherwise the text background opacity).
    pub background_chat_only: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            scale: 1.0,
            width: 1.0,
            height_focused: 1.0,
            height_unfocused: 0.4375,
            spacing: 0.0,
            delay: 0.0,
            colors: true,
            links: true,
            links_prompt: true,
            hide_matched_names: true,
            only_secure: false,
            save_drafts: false,
            visibility: 0,
            text_opacity: 1.0,
            background: 0.5,
            background_chat_only: true,
        }
    }
}

/// Chat width in interface pixels (`floor(pct × 280 + 40)`).
pub fn width_px(pct: f64) -> i32 {
    (pct * 280.0 + 40.0).floor() as i32
}

/// Chat height in interface pixels (`floor(pct × 160 + 20)`).
pub fn height_px(pct: f64) -> i32 {
    (pct * 160.0 + 20.0).floor() as i32
}

/// Input in progress (borderless field, `chat.md` § 4).
pub struct Input {
    pub text: Vec<char>,
    pub cursor: usize,
    /// Other end of the selection (= cursor if none).
    pub anchor: usize,
    /// First character displayed (horizontal scrolling).
    display: usize,
    /// Scrolling to recompute at the next draw (cursor moved).
    rescroll: bool,
    focus: Instant,
    history_pos: usize,
    history_buffer: String,
    /// Draft restored, not yet modified: grey italic, Backspace erases everything.
    pub is_draft: bool,
}

impl Input {
    fn new(text: &str, is_draft: bool) -> Input {
        let text: Vec<char> = text.chars().collect();
        let n = text.len();
        Input {
            text,
            cursor: n,
            anchor: n,
            display: 0,
            rescroll: true,
            focus: Instant::now(),
            history_pos: 0,
            history_buffer: String::new(),
            is_draft,
        }
    }

    pub fn value(&self) -> String {
        self.text.iter().collect()
    }

    /// Chat in bed on waking: closed if it is empty or holds only a resumed draft; typed
    /// text stays, in ordinary chat.
    pub fn closes_on_wake(&self) -> bool {
        self.is_draft || self.text.is_empty()
    }

    pub fn selection(&self) -> (usize, usize) {
        (self.cursor.min(self.anchor), self.cursor.max(self.anchor))
    }

    /// Text replaced, cursor placed (no selection).
    pub fn set(&mut self, text: Vec<char>, cursor: usize) {
        self.text = text;
        self.move_to(cursor, false);
    }

    pub fn move_to(&mut self, to: usize, extend: bool) {
        self.cursor = to.min(self.text.len());
        if !extend {
            self.anchor = self.cursor;
        }
        self.rescroll = true;
    }

    /// Insertion in place of the selection (disallowed characters removed, length bounded);
    /// true if the text changed.
    pub fn insert(&mut self, s: &str) -> bool {
        let (a, b) = self.selection();
        let room = MAX_INPUT as i64 - self.text.len() as i64 + (b - a) as i64;
        if room <= 0 {
            return false;
        }
        let add: Vec<char> = s.chars().filter(|&c| rc_text::allowed_chat_char(c)).take(room as usize).collect();
        if add.is_empty() && a == b {
            return false;
        }
        let n = add.len();
        self.text.splice(a..b, add);
        self.move_to(a + n, false);
        true
    }

    /// Position of the word (`dir` < 0: start of the previous word; > 0: after the word and its spaces).
    pub fn word(&self, from: usize, dir: i32) -> usize {
        let t = &self.text;
        let mut i = from.min(t.len());
        if dir > 0 {
            match t[i..].iter().position(|&c| c == ' ') {
                None => i = t.len(),
                Some(p) => {
                    i += p;
                    while i < t.len() && t[i] == ' ' {
                        i += 1;
                    }
                }
            }
        } else {
            while i > 0 && t[i - 1] == ' ' {
                i -= 1;
            }
            while i > 0 && t[i - 1] != ' ' {
                i -= 1;
            }
        }
        i
    }

    /// Deletion towards `dir` (character or word); a selection is deleted first.
    pub fn delete(&mut self, dir: i32, word: bool) -> bool {
        if self.text.is_empty() {
            return false;
        }
        if self.anchor != self.cursor {
            return self.insert("");
        }
        let to = if word {
            self.word(self.cursor, dir)
        } else if dir < 0 {
            self.cursor.saturating_sub(1)
        } else {
            (self.cursor + 1).min(self.text.len())
        };
        let (a, b) = (to.min(self.cursor), to.max(self.cursor));
        if a == b {
            return false;
        }
        self.text.drain(a..b);
        self.move_to(a, false);
        true
    }

    pub fn selected_text(&self) -> String {
        let (a, b) = self.selection();
        self.text[a..b].iter().collect()
    }

    /// Number of characters from `from` that fit in `w` pixels.
    fn fit(gui: &Gui, chars: &[char], w: f32) -> usize {
        let mut acc = 0.0;
        for (i, &c) in chars.iter().enumerate() {
            acc += rc_text::Metrics::advance(gui, "", c).0;
            if acc > w {
                return i;
            }
        }
        chars.len()
    }

    /// Number of characters from the end that fit in `w` pixels.
    fn fit_rev(gui: &Gui, chars: &[char], w: f32) -> usize {
        let mut acc = 0.0;
        for (i, &c) in chars.iter().rev().enumerate() {
            acc += rc_text::Metrics::advance(gui, "", c).0;
            if acc > w {
                return i;
            }
        }
        chars.len()
    }

    /// Horizontal scrolling to keep `pos` visible.
    fn scroll_to(&mut self, gui: &Gui, pos: usize, inner: f32) {
        let n = self.text.len();
        self.display = self.display.min(n);
        let shown = Self::fit(gui, &self.text[self.display..], inner);
        let last = self.display + shown;
        if pos == self.display {
            self.display = self.display.saturating_sub(Self::fit_rev(gui, &self.text, inner));
        }
        if pos > last {
            self.display += pos - last;
        } else if pos <= self.display {
            self.display = pos;
        }
        self.display = self.display.min(n);
    }

    /// Index of the character under `x` (interface units; field at x = 4).
    /// Measures like the field's drawing: without "suspect" glyphs.
    pub fn index_at(&self, gui: &Gui, x: f32, inner: f32) -> usize {
        gui.filter_fishy.set(true);
        let k = self.index_at_inner(gui, x, inner);
        gui.filter_fishy.set(false);
        k
    }

    fn index_at_inner(&self, gui: &Gui, x: f32, inner: f32) -> usize {
        let pos = ((x.floor() - 4.0).min(inner)).max(0.0);
        self.display + Self::fit(gui, &self.text[self.display.min(self.text.len())..], pos)
    }
}

/// Persistent command history (`command_history.txt` in the game folder, UTF-8, one
/// command per line): last 50, no consecutive duplicate, rewritten on every addition. Reading
/// keeps all the lines of the file (the game only truncates on the following additions).
#[derive(Default)]
pub(crate) struct CommandHistory {
    /// File; `None` = `command_history.txt` in the game folder.
    path: Option<std::path::PathBuf>,
    loaded: bool,
    list: VecDeque<String>,
}

const MAX_COMMANDS: usize = 50;

impl CommandHistory {
    fn file(&self) -> std::path::PathBuf {
        self.path.clone().unwrap_or_else(|| crate::servers::game_dir().join("command_history.txt"))
    }

    /// Read on first use; returns the commands read (empty afterwards).
    fn load(&mut self) -> Vec<String> {
        if std::mem::replace(&mut self.loaded, true) {
            return Vec::new();
        }
        match std::fs::read(self.file()) {
            Ok(b) => {
                // Read by lines (endings `\n`, `\r\n` or `\r`, like the game's reader).
                let text = String::from_utf8_lossy(&b).replace("\r\n", "\n").replace('\r', "\n");
                let mut lines: Vec<String> = text.split('\n').map(str::to_owned).collect();
                if lines.last().is_some_and(String::is_empty) {
                    lines.pop();
                }
                self.list = lines.iter().cloned().collect();
                lines
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(e) => {
                eprintln!("command_history.txt: cannot read, history missing: {e}");
                Vec::new()
            }
        }
    }

    fn add(&mut self, command: &str) {
        if self.list.back().map(String::as_str) == Some(command) {
            return;
        }
        if self.list.len() >= MAX_COMMANDS {
            self.list.pop_front();
        }
        self.list.push_back(command.to_owned());
        let mut out = String::new();
        for c in &self.list {
            out.push_str(c);
            out.push_str(if cfg!(windows) { "\r\n" } else { "\n" });
        }
        if let Err(e) = std::fs::write(self.file(), out) {
            eprintln!("command_history.txt: cannot write: {e}");
        }
    }
}

impl Chat {
    /// Input history completed with the commands of previous launches (once).
    fn load_command_history(&mut self) {
        let lines = self.commands.load();
        if !lines.is_empty() {
            // The game fills the input history with these commands before any input.
            let mut all = lines;
            all.append(&mut self.history);
            self.history = all;
        }
    }
}

/// Reason the input was closed (decides the draft).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    /// Escape: draft kept if `saveChatDrafts`.
    Intentional,
    /// Another screen on top (death, link…): draft kept.
    Interrupted,
}

#[derive(Default)]
pub struct Chat {
    messages: VecDeque<Message>,
    lines: VecDeque<Line>,
    dirty: bool,
    scroll: usize,
    new_message: bool,
    /// Messages added since the last split (scrolling kept at the top).
    added: usize,
    pub hud_tick: i64,
    pub input: Option<Input>,
    history: Vec<String>,
    /// Commands kept between two launches (`command_history.txt`, last 50); read on
    /// first opening, like the game which fills the input history with them.
    commands: CommandHistory,
    /// Action bar: text, remaining ticks, animated colour ("Now Playing").
    action_bar: Option<(Component, i32, bool)>,
    pub settings: Settings,
    /// RGB colour of each character of the input (parsed command, `commands.md` § 4.3).
    pub format: Option<Vec<u32>>,
    /// Ghost text of the selected suggestion, drawn after the cursor at the end of the text.
    pub ghost: Option<String>,
    /// Pending deletions (less than 60 ticks since the addition, `chat.md` § 1.5).
    deletions: Vec<rc_client::chat::Signature>,
    /// Queue of delayed player messages (`chatDelay`) and time of the last one displayed.
    queue: VecDeque<Queued>,
    last_player_message: Option<Instant>,
    /// Draft kept on closing (`chat.md` § 4.6).
    draft: Option<String>,
    /// Rank in the field's history at the last send (chat in bed, see `reopen_after_send`).
    sent_history_pos: usize,
    /// Hover tooltip found at the last draw (chat open).
    pub hover: Option<Arc<HoverEvent>>,
    /// Confirmation screen (link, command) above everything (`links.rs`).
    pub confirm: Option<crate::links::Confirm>,
    /// Selection drag in progress in the field; last click (double click).
    pub dragging: bool,
    pub last_click: Option<(Instant, (f32, f32))>,
    /// Mouse forced by the test script (`RUSTCRAFT_CMDTEST`, step `mX,Y`).
    pub test_mouse: Option<(f32, f32)>,
    /// Player message added last (signature to attach), even if delayed.
    last_was_queued: bool,
    /// Restricted chat (`chatVisibility` setting, launcher, account): red underlined prompt
    /// `chat_screen.restricted` above the messages, clickable towards the restrictions screen.
    pub restricted: bool,
    /// Menus open on top of the chat (restrictions screen), set on every frame: the chat
    /// is then drawn closed (no field, no suggestions, no hover, no IME cursor), like the
    /// game whose chat screen is replaced.
    pub covered: bool,
}

/// Colour of the sidebar; `None`: secure message, no bar.
fn tag_color(t: ChatTag) -> Option<u32> {
    match t {
        ChatTag::System | ChatTag::NotSecure | ChatTag::Disguised => Some(0xD0D0D0),
        ChatTag::Error => Some(0xFF5555),
        ChatTag::Modified => Some(0x606060),
        ChatTag::Secure => None,
    }
}

fn tag_text(t: ChatTag) -> &'static str {
    match t {
        ChatTag::System => "chat.tag.system_single_player",
        ChatTag::Disguised => "chat.tag.system",
        ChatTag::NotSecure => "chat.tag.not_secure",
        ChatTag::Error => "chat.tag.error",
        ChatTag::Modified | ChatTag::Secure => "chat.tag.modified",
    }
}

/// Tooltip of the tag: its text; "edited": followed by the original signed text in grey.
fn tag_tooltip(line: &Line) -> Component {
    let mut t = Component::translate(tag_text(line.tag), Vec::new());
    if line.tag == ChatTag::Modified {
        let mut original = Component::text(line.original.as_deref().unwrap_or_default());
        original.style.color = Some(0xAAAAAA);
        t = Component {
            content: rc_text::Content::Text(String::new()),
            style: Default::default(),
            extra: vec![t, Component::text("\n"), original],
        };
    }
    t
}

/// Chat geometry (local frame: scale `s`, offset of 4 on the left).
struct Geo {
    s: f32,
    max_w: f32,
    bottom: f32,
    entry_h: f32,
    to_msg: f32,
}

impl Chat {
    /// Settings read from `options.txt`; re-split and scrolling reset if they change.
    pub fn apply_options(&mut self, o: &crate::options::Options) {
        let s = Settings {
            scale: o.f("chatScale"),
            width: o.f("chatWidth"),
            height_focused: o.f("chatHeightFocused"),
            height_unfocused: o.f("chatHeightUnfocused"),
            spacing: o.f("chatLineSpacing"),
            delay: o.f("chatDelay"),
            colors: o.b("chatColors"),
            links: o.b("chatLinks"),
            links_prompt: o.b("chatLinksPrompt"),
            hide_matched_names: o.b("hideMatchedNames"),
            only_secure: o.b("onlyShowSecureChat"),
            save_drafts: o.b("saveChatDrafts"),
            visibility: o.i("chatVisibility"),
            text_opacity: (o.f("chatOpacity") * 0.9 + 0.1) as f32,
            background: o.f("textBackgroundOpacity") as f32,
            background_chat_only: o.b("backgroundForChatOnly"),
        };
        let rewrap =
            s.scale != self.settings.scale || s.width != self.settings.width || s.colors != self.settings.colors;
        if s.delay == 0.0 && self.settings.delay > 0.0 {
            self.flush_queue();
        }
        // Visibility changed: the messages already received are refiltered by origin, without
        // touching the scrolling.
        if s.visibility != self.settings.visibility {
            self.dirty = true;
        }
        self.settings = s;
        if rewrap {
            self.dirty = true;
            self.scroll = 0;
            self.new_message = false;
        }
    }

    /// Message received: player messages go through the queue if a delay is set.
    pub fn add(&mut self, component: Component, tag: ChatTag) {
        self.last_was_queued = false;
        if tag.delayed() && self.settings.delay > 0.0 {
            self.queue.push_back((component, tag, None, None));
            self.last_was_queued = true;
            return;
        }
        self.show(component, tag, Source::of(tag), None, None);
    }

    /// Message from the client itself (bed, captures, debug keys, hidden players…):
    /// never filtered by the chat visibility.
    pub fn add_client(&mut self, component: Component) {
        self.last_was_queued = false;
        self.show(component, ChatTag::System, Source::Client, None, None);
    }

    /// Effective display; false if the message is filtered.
    fn show(
        &mut self,
        component: Component,
        tag: ChatTag,
        source: Source,
        sig: Option<rc_client::chat::Signature>,
        original: Option<Arc<str>>,
    ) -> bool {
        match tag {
            // Secure chat only: unsecured messages are hidden.
            ChatTag::NotSecure if self.settings.only_secure => return false,
            ChatTag::NotSecure | ChatTag::Error | ChatTag::Secure | ChatTag::Modified | ChatTag::Disguised => {
                self.last_player_message = Some(Instant::now())
            }
            ChatTag::System => {}
        }
        self.messages.push_front(Message { component, tick: self.hud_tick, tag, source, signature: sig, original });
        self.messages.truncate(MAX);
        self.dirty = true;
        self.added += 1;
        true
    }

    /// Next message of the queue (click on the `chat.queue` line).
    pub fn accept_next(&mut self) {
        if let Some((c, t, s, o)) = self.queue.pop_front() {
            self.show(c, t, Source::of(t), s, o);
        }
    }

    fn flush_queue(&mut self) {
        while let Some((c, t, s, o)) = self.queue.pop_front() {
            self.show(c, t, Source::of(t), s, o);
        }
        self.last_player_message = None;
    }

    /// Signature of the player message added just before.
    pub fn attach_signature(&mut self, sig: rc_client::chat::Signature) {
        if self.last_was_queued {
            if let Some(q) = self.queue.back_mut() {
                q.2 = Some(sig);
            }
        } else if let Some(m) = self.messages.front_mut() {
            m.signature = Some(sig);
        }
    }

    /// Original text of the "edited" message added just before.
    pub fn attach_original(&mut self, text: String) {
        let text: Arc<str> = text.into();
        if self.last_was_queued {
            if let Some(q) = self.queue.back_mut() {
                q.3 = Some(text);
            }
        } else if let Some(m) = self.messages.front_mut() {
            m.original = Some(text);
            self.dirty = true;
        }
    }

    /// `delete_chat` (`chat.md` § 1.5): grey italic marker in place of the message, after
    /// at least 60 HUD ticks since its addition (otherwise retried on every tick); a message still
    /// in the queue is simply removed.
    pub fn delete(&mut self, sig: rc_client::chat::Signature) {
        let before = self.queue.len();
        self.queue.retain(|q| q.2.as_ref() != Some(&sig));
        if self.queue.len() != before {
            return;
        }
        if !self.try_delete(&sig) {
            self.deletions.push(sig);
        }
    }

    fn try_delete(&mut self, sig: &rc_client::chat::Signature) -> bool {
        let now = self.hud_tick;
        let Some(m) = self.messages.iter_mut().find(|m| m.signature.as_ref() == Some(sig)) else { return true };
        if now - m.tick < 60 {
            return false;
        }
        let mut c = Component::translate("chat.deleted_marker", Vec::new());
        c.style.color = Some(0xAAAAAA);
        c.style.italic = Some(true);
        m.component = c;
        // "System" tag (`chat.tag.system`), like the game's marker.
        m.tag = ChatTag::Disguised;
        m.source = Source::Server;
        m.original = None;
        m.signature = None;
        self.dirty = true;
        true
    }

    /// Displayed messages cleared (F3+D); the send history is kept.
    pub fn clear_messages(&mut self) {
        self.messages.clear();
        self.lines.clear();
        self.queue.clear();
        self.scroll = 0;
        self.dirty = true;
    }

    /// End of a chat session (`reset_chat` of the configuration, disconnection): messages, delayed
    /// queue and pending deletions dropped; the input history restarts from the kept
    /// commands only (`command_history.txt`). Settings and draft stay, like the game.
    pub fn clear_session(&mut self) {
        self.clear_messages();
        self.deletions.clear();
        self.added = 0;
        self.new_message = false;
        self.last_was_queued = false;
        self.hover = None;
        // File not read yet: it will be on the next opening, ahead of the empty history.
        self.history = if self.commands.loaded { self.commands.list.iter().cloned().collect() } else { Vec::new() };
    }

    /// Disconnection: the whole chat display starts afresh (input, action bar, scrolling),
    /// but settings, draft and kept commands survive, like the game's chat which stays
    /// the same from one server to the next.
    pub fn disconnected(&mut self) {
        let old = std::mem::take(self);
        self.settings = old.settings;
        self.draft = old.draft;
        self.commands = old.commands;
        self.clear_session();
    }

    pub fn action_bar(&mut self, c: Component) {
        self.action_bar = Some((c, 60, false));
    }

    /// Jukebox message: same place, colour that cycles around the hue wheel.
    pub fn now_playing(&mut self, description: Component) {
        let c = Component::translate("record.nowPlaying", vec![rc_text::Arg::Component(description)]);
        self.action_bar = Some((c, 60, true));
    }

    /// HUD tick: action bar counted down, queue of delayed messages.
    pub fn tick(&mut self) {
        self.hud_tick += 1;
        if !self.deletions.is_empty() {
            let pending = std::mem::take(&mut self.deletions);
            for sig in pending {
                if !self.try_delete(&sig) {
                    self.deletions.push(sig);
                }
            }
        }
        if let Some((_, left, _)) = &mut self.action_bar {
            *left -= 1;
            if *left <= 0 {
                self.action_bar = None;
            }
        }
        self.tick_queue();
    }

    /// Queue of delayed messages ("chat delay" setting): the only part of the tick that also runs
    /// during the singleplayer pause (fade, deletions and action bar are frozen there).
    pub fn tick_queue(&mut self) {
        if self.settings.delay <= 0.0 {
            if !self.queue.is_empty() {
                self.flush_queue();
            }
        } else {
            let delay = Duration::from_millis((self.settings.delay * 1000.0) as u64);
            if self.last_player_message.is_none_or(|t| t.elapsed() >= delay) {
                while let Some((c, t, s, o)) = self.queue.pop_front() {
                    if self.show(c, t, Source::of(t), s, o) {
                        break;
                    }
                }
            }
        }
    }

    /// Input open (game keys are then inactive).
    pub fn is_open(&self) -> bool {
        self.input.is_some()
    }

    /// Input open AND visible: no confirmation screen or menus on top.
    pub fn shown(&self) -> bool {
        self.input.is_some() && self.confirm.is_none() && !self.covered
    }

    /// Scrolling reset to the bottom, like the game when it leaves the chat screen.
    pub fn reset_scroll(&mut self) {
        self.scroll = 0;
        self.new_message = false;
    }

    /// Opening (`""` message, `"/"` command): the draft is resumed if it fits.
    pub fn open(&mut self, initial: &str) {
        let restore = self.draft.as_ref().is_some_and(|d| initial.is_empty() || d.starts_with('/'));
        let mut i = match (&self.draft, restore) {
            (Some(d), true) => Input::new(d, true),
            _ => Input::new(initial, false),
        };
        self.load_command_history();
        i.history_pos = self.history.len();
        self.input = Some(i);
        self.dragging = false;
    }

    /// Text inserted at the cursor (Shift + click on an insertion) or replaced (`suggest_command`).
    pub fn insert_text(&mut self, text: &str, replace: bool) -> bool {
        let Some(i) = &mut self.input else { return false };
        if replace {
            i.text.clear();
            i.move_to(0, false);
        }
        let changed = i.insert(text) || replace;
        if changed {
            i.is_draft = false;
        }
        changed
    }

    /// Character(s) typed; true if the text changed.
    pub fn type_text(&mut self, t: &str) -> bool {
        let Some(i) = &mut self.input else { return false };
        let changed = i.insert(t);
        if changed {
            i.is_draft = false;
        }
        changed
    }

    /// Backspace / Delete (whole word with Ctrl); an intact draft is erased entirely.
    pub fn delete_text(&mut self, dir: i32, word: bool) -> bool {
        let Some(i) = &mut self.input else { return false };
        if i.is_draft && dir < 0 {
            i.text.clear();
            i.move_to(0, false);
            i.is_draft = false;
            return true;
        }
        let changed = i.delete(dir, word);
        if changed {
            i.is_draft = false;
        }
        changed
    }

    /// Cursor moved by `d` characters (or one word), selection extended with Shift.
    pub fn move_cursor_by(&mut self, d: i32, word: bool, extend: bool) {
        if let Some(i) = &mut self.input {
            let to =
                if word { i.word(i.cursor, d) } else { (i.cursor as i32 + d).clamp(0, i.text.len() as i32) as usize };
            i.move_to(to, extend);
        }
    }

    pub fn move_cursor(&mut self, d: i32) {
        self.move_cursor_by(d, false, false);
    }

    pub fn home_end(&mut self, end: bool, extend: bool) {
        if let Some(i) = &mut self.input {
            let to = if end { i.text.len() } else { 0 };
            i.move_to(to, extend);
        }
    }

    pub fn select_all(&mut self) {
        if let Some(i) = &mut self.input {
            i.move_to(i.text.len(), false);
            i.anchor = 0;
        }
    }

    /// Selected text (Ctrl+C / Ctrl+X).
    pub fn selected(&self) -> String {
        self.input.as_ref().map(Input::selected_text).unwrap_or_default()
    }

    /// Up/down arrows: send history (the position after the end gives the current text).
    pub fn history_step(&mut self, d: i32) -> bool {
        let n = self.history.len();
        let Some(i) = &mut self.input else { return false };
        let pos = (i.history_pos as i32 + d).clamp(0, n as i32) as usize;
        if pos == i.history_pos {
            return false;
        }
        let t = if pos == n {
            i.history_buffer.clone()
        } else {
            if i.history_pos == n {
                i.history_buffer = i.value();
            }
            self.history[pos].clone()
        };
        i.history_pos = pos;
        let chars: Vec<char> = t.chars().collect();
        let n = chars.len();
        i.set(chars, n);
        true
    }

    /// Lines per page (height of the open or closed chat / line height).
    pub fn lines_per_page(&self) -> usize {
        let pct = if self.input.is_some() { self.settings.height_focused } else { self.settings.height_unfocused };
        let lh = (9.0 * (self.settings.spacing + 1.0)) as i32;
        (height_px(pct) / lh.max(1)).max(0) as usize
    }

    pub fn scroll(&mut self, d: i32) {
        let max = self.lines.len() as i32 - self.lines_per_page() as i32;
        let mut s = self.scroll as i32 + d;
        if s > max {
            s = max;
        }
        if s <= 0 {
            s = 0;
            self.new_message = false;
        }
        self.scroll = s as usize;
    }

    /// Enter: cleaned text (spaces collapsed, truncated); returns `Some((command?, text))`.
    pub fn submit(&mut self) -> Option<(bool, String)> {
        let i = self.input.take()?;
        self.sent_history_pos = i.history_pos;
        self.scroll = 0;
        self.new_message = false;
        self.draft = None;
        let raw: String = i.text.iter().collect();
        let mut text = raw.split_whitespace().collect::<Vec<_>>().join(" ");
        text = text.chars().take(MAX_INPUT).collect();
        if text.is_empty() {
            return None;
        }
        self.load_command_history();
        if self.history.last() != Some(&text) {
            self.history.push(text.clone());
            if self.history.len() > MAX {
                self.history.remove(0);
            }
        }
        if text.starts_with('/') {
            self.commands.add(&text);
        }
        match text.strip_prefix('/') {
            Some(cmd) => Some((true, cmd.to_owned())),
            None => Some((false, text)),
        }
    }

    /// Chat in bed after a send: the screen stays, field emptied, scrolling reset to the bottom. The rank
    /// in the history stays the one from before the send (like the game: the up arrow then skips
    /// the message just added).
    pub fn reopen_after_send(&mut self) {
        let mut i = Input::new("", false);
        i.history_pos = self.sent_history_pos;
        self.input = Some(i);
        self.reset_scroll();
    }

    /// Closing without sending (`removed`): draft kept or dropped depending on the reason.
    pub fn close_with(&mut self, why: Exit) {
        if let Some(i) = self.input.take() {
            let text = i.value();
            let keep = why == Exit::Interrupted || self.settings.save_drafts;
            if !keep || text.trim().is_empty() {
                self.draft = None;
            } else if !i.is_draft {
                self.draft = Some(text);
            }
        }
        self.reset_scroll();
        self.dragging = false;
    }

    /// Forced closing (screen on top, death, disconnection): draft kept.
    pub fn close(&mut self) {
        self.close_with(Exit::Interrupted);
    }

    fn rewrap(&mut self, metrics: &dyn rc_text::Metrics, lang: &rc_text::Lang) {
        self.lines.clear();
        let w = (f64::from(width_px(self.settings.width)) / self.settings.scale.max(0.01)).floor() as f32;
        let mut fresh = 0;
        // From oldest to newest, each line inserted at the head (the newest at 0).
        let visibility = self.settings.visibility;
        for (k_msg, m) in self.messages.iter().enumerate().rev() {
            if !m.source.visible(visibility) {
                continue;
            }
            let mut segments = rc_text::flatten(&m.component, lang);
            if !self.settings.colors {
                for s in &mut segments {
                    s.text = rc_text::strip_codes(&s.text);
                }
            }
            let chars = rc_text::styled_chars(&segments);
            // Edited message: room for the icon (9 px) and its margins (4 + 2) removed.
            let wrapped = rc_text::wrap(metrics, &chars, if m.tag == ChatTag::Modified { w - 15.0 } else { w });
            let last = wrapped.len().saturating_sub(1);
            for (k, mut l) in wrapped.into_iter().enumerate() {
                if k > 0 {
                    l.insert(0, StyledChar { c: ' ', style: Default::default() });
                }
                if k_msg < self.added {
                    fresh += 1;
                }
                self.lines.push_front(Line {
                    chars: l,
                    tick: m.tick,
                    tag: m.tag,
                    original: m.original.clone(),
                    end: k == last,
                    indent: k > 0,
                });
            }
        }
        self.lines.truncate(MAX);
        // Chat open and scrolled: the view stays on the same lines ("message received since scrolling" flag).
        if self.input.is_some() && self.scroll > 0 && fresh > 0 {
            self.new_message = true;
            self.scroll(fresh);
        }
        self.added = 0;
        self.dirty = false;
    }

    fn geo(&self, h: f32) -> Geo {
        let s = self.settings.scale as f32;
        let sp = self.settings.spacing;
        Geo {
            s,
            max_w: (width_px(self.settings.width) as f32 / s.max(0.01)).ceil(),
            bottom: ((h - 40.0) / s.max(0.01)).floor(),
            entry_h: (9.0 * (sp + 1.0)) as i32 as f32,
            to_msg: (8.0 * (sp + 1.0) - 4.0 * sp).round() as f32,
        }
    }

    fn alpha(&self, open: bool, line: &Line) -> f32 {
        if open {
            return 1.0;
        }
        let age = (self.hud_tick - line.tick) as f32;
        let t = ((1.0 - age / 200.0) * 10.0).clamp(0.0, 1.0);
        t * t
    }

    /// Visible lines: (index, rank from the bottom, opacity).
    fn visible(&self, open: bool) -> Vec<(usize, usize, f32)> {
        let n = self.lines.len().saturating_sub(self.scroll).min(self.lines_per_page());
        (0..n)
            .rev()
            .filter_map(|i| {
                let a = self.alpha(open, &self.lines[i + self.scroll]);
                (a > 1.0e-5).then_some((i + self.scroll, i, a))
            })
            .collect()
    }

    /// `chat.queue` line (queue of delayed messages), with click and tooltip.
    fn queue_line(&self) -> Option<Component> {
        if self.queue.is_empty() {
            return None;
        }
        let mut c = Component::translate("chat.queue", vec![rc_text::Arg::Text(self.queue.len().to_string())]);
        c.style.click =
            Some(Arc::new(rc_text::ClickEvent { action: "custom".into(), value: QUEUE_EXPAND.into(), nbt: None }));
        c.style.hover = Some(Arc::new(HoverEvent::Text(Component::translate("chat.queue.tooltip", Vec::new()))));
        Some(c)
    }

    /// Prompt of the restricted chat (chat open): red underlined `chat_screen.restricted`, click towards
    /// the restrictions screen; wider than the chat, cut with "..." and the tooltip of the
    /// whole text, like the game.
    fn restricted_line(&self, gui: &Gui, max_w: f32) -> Option<Vec<StyledChar>> {
        if !self.restricted || self.input.is_none() {
            return None;
        }
        Some(restricted_prompt(gui, &gui.lang, max_w))
    }

    /// Top of the restricted chat prompt text: line above the `count` visible lines.
    fn restricted_top(g: &Geo, count: usize) -> f32 {
        let above = g.bottom - (count as f32 + 1.0) * g.entry_h;
        above + g.entry_h - g.to_msg - 1.0
    }
}

/// Text of the restricted chat prompt for a chat `max_w` wide.
fn restricted_prompt(m: &dyn rc_text::Metrics, lang: &rc_text::Lang, max_w: f32) -> Vec<StyledChar> {
    let mut c = Component::translate("chat_screen.restricted", Vec::new());
    c.style.color = Some(0xFF5555);
    c.style.underlined = Some(true);
    c.style.click =
        Some(Arc::new(rc_text::ClickEvent { action: "custom".into(), value: GO_TO_RESTRICTIONS.into(), nbt: None }));
    let mut chars = rc_text::styled_chars(&rc_text::flatten(&c, lang));
    let max = max_w as i32;
    if rc_text::width(m, &chars) > max {
        let hover = Arc::new(HoverEvent::Text(Component::translate("chat_screen.restricted", Vec::new())));
        let dots = crate::gui::plain("...");
        let room = (max - rc_text::width(m, &dots)) as f32;
        let (mut acc, mut keep) = (0.0, 0);
        for (i, ch) in chars.iter().enumerate() {
            let (a, bold) = m.advance(&ch.style.font, ch.c);
            acc += if ch.style.bold { a + bold } else { a };
            if acc > room {
                break;
            }
            keep = i + 1;
        }
        chars.truncate(keep);
        for ch in &mut chars {
            ch.style.hover = Some(hover.clone());
        }
        chars.extend(dots);
    }
    chars
}

impl Chat {
    /// Style under the mouse (chat open): message text or queue line.
    pub fn style_at(&mut self, gui: &Gui, m: (f32, f32)) -> Option<CharStyle> {
        self.input.as_ref()?;
        if self.dirty {
            self.rewrap(gui, &gui.lang);
        }
        let g = self.geo(gui.height);
        let (lx, ly) = (m.0 / g.s - 4.0, m.1 / g.s);
        for (idx, row, _) in self.visible(true) {
            let entry_bottom = g.bottom - row as f32 * g.entry_h;
            let top = entry_bottom - g.to_msg;
            if ly >= top && ly < top + 9.0 {
                return self.lines[idx].style_at(gui, lx).cloned();
            }
        }
        if let Some(c) = self.queue_line() {
            let top = g.bottom + 1.0;
            if ly >= top && ly < top + 9.0 {
                let chars = rc_text::styled_chars(&rc_text::flatten(&c, &gui.lang));
                return crate::links::style_at(gui, &chars, lx).cloned();
            }
        }
        if let Some(chars) = self.restricted_line(gui, g.max_w) {
            let top = Self::restricted_top(&g, self.visible(true).len());
            if ly >= top && ly < top + 9.0 {
                return crate::links::style_at(gui, &chars, lx).cloned();
            }
        }
        None
    }

    /// Action bar (HUD, before the title): 60 ticks, fade over the last 20, new layer,
    /// text background `background` (alpha only, 0 if reserved for the chat).
    pub fn draw_action_bar(&self, gui: &mut Gui, queue: &wgpu::Queue, pt: f32, background: u32) {
        let Some((c, left, rainbow)) = &self.action_bar else { return };
        let t = *left as f32 - pt;
        let a = ((t * 255.0 / 20.0) as i32).min(255);
        if a <= 0 {
            return;
        }
        gui.next_layer();
        let chars = rc_text::styled_chars(&rc_text::flatten(c, &gui.lang));
        let lw = rc_text::width(gui, &chars);
        // Integer centre of the screen, then −width/2 truncated towards zero.
        let x = (gui.width / 2.0).floor() - (lw / 2) as f32;
        let rgb = if *rainbow { crate::world_fx::hsv_rgb(t / 50.0, 0.7, 0.6) } else { 0xFF_FFFF };
        let y = (gui.height as i32 - 68 - 4) as f32;
        crate::hud::text_with_backdrop(gui, queue, &chars, x, y, lw, (a as u32) << 24 | rgb, background);
    }

    /// Drawing: chat (closed or open, scaled), input field; keeps
    /// the hovered tooltip in `hover` (open chat only).
    pub fn draw(&mut self, gui: &mut Gui, queue: &wgpu::Queue, _pt: f32, mouse: Option<(f32, f32)>) {
        if self.dirty {
            self.rewrap(gui, &gui.lang);
        }
        let mouse = self.test_mouse.or(mouse);
        self.hover = None;
        let h = gui.height;
        let open = self.shown();
        let g = self.geo(h);
        let set = &self.settings;
        let vis = self.visible(open);
        let ql = self.queue_line();
        let rl = if open { self.restricted_line(gui, g.max_w) } else { None };
        if !vis.is_empty() || ql.is_some() || rl.is_some() {
            let mark = gui.mark();
            let local = mouse.map(|m| (m.0 / g.s - 4.0, m.1 / g.s));
            // Backgrounds first, then texts and sidebars.
            for &(_, row, a) in &vis {
                let b = g.bottom - row as f32 * g.entry_h;
                gui.fill(-4.0, b - g.entry_h, g.max_w + 8.0, b, ((a * set.background * 255.0) as u32) << 24);
            }
            if ql.is_some() {
                let bg = ((set.background * 255.0) as u32) << 24;
                gui.fill(-2.0, g.bottom, g.max_w + 4.0, g.bottom + 9.0, bg);
            }
            let restricted_top = Self::restricted_top(&g, vis.len());
            if rl.is_some() {
                let bg = ((set.background * 255.0) as u32) << 24;
                let above = g.bottom - (vis.len() as f32 + 1.0) * g.entry_h;
                gui.fill(-2.0, above, g.max_w + 8.0, above + g.entry_h, bg);
            }
            // Hover on a message (one of its lines, chat open): its tag icon
            // shown on its last line.
            let mut hovered_message = false;
            for &(idx, row, a) in &vis {
                let line = &self.lines[idx];
                let b = g.bottom - row as f32 * g.entry_h;
                let top = b - g.to_msg;
                let over_text = open
                    && local.is_some_and(|(lx, ly)| {
                        ly >= top && ly < top + 9.0 && line.style_at(gui, lx).is_some()
                    });
                hovered_message |= over_text;
                let force_icon = line.end && std::mem::take(&mut hovered_message);
                let ta = (a * set.text_opacity * 255.0) as u32;
                if ta == 0 {
                    continue;
                }
                line.draw(gui, queue, top, ta << 24 | 0xFF_FFFF);
                let color = tag_color(line.tag);
                if let Some(c) = color {
                    gui.fill(-4.0, b - g.entry_h, -2.0, b, ta << 24 | c);
                }
                if open && let Some((lx, ly)) = local {
                    if over_text {
                        self.hover = line.style_at(gui, lx).and_then(|s| s.hover.clone());
                    }
                    if color.is_some() && (-4.0..-2.0).contains(&lx) && (b - g.entry_h..b).contains(&ly) {
                        self.hover = Some(Arc::new(HoverEvent::Text(tag_tooltip(line))));
                    }
                }
                // "Edited" icon (9 × 9) after the text of the last line, chat open:
                // visible when hovering the message or the icon.
                if open && line.end && line.tag == ChatTag::Modified {
                    let left = line.width(gui) as f32 + 4.0;
                    let icon_top = top + 9.0 - 9.0 - 1.0;
                    let over_icon = local.is_some_and(|(lx, ly)| {
                        lx >= left && lx < left + 9.0 && ly >= icon_top && ly < icon_top + 9.0 + 1.0
                    });
                    if over_icon {
                        self.hover = Some(Arc::new(HoverEvent::Text(tag_tooltip(line))));
                    }
                    if force_icon || over_icon {
                        gui.sprite("icon/chat_modified", left, icon_top, 9.0, 9.0, 0xFFFF_FFFF);
                    }
                }
            }
            if let Some(c) = &ql {
                let chars = rc_text::styled_chars(&rc_text::flatten(c, &gui.lang));
                let ta = (0.5 * set.text_opacity * 255.0) as u32;
                gui.text(queue, &chars, 0.0, g.bottom + 1.0, ta << 24 | 0xFF_FFFF, true, 0);
                if open
                    && let Some((lx, ly)) = local
                    && ly >= g.bottom + 1.0
                    && ly < g.bottom + 10.0
                {
                    self.hover = crate::links::style_at(gui, &chars, lx).and_then(|s| s.hover.clone());
                }
            }
            if let Some(chars) = &rl {
                let ta = (set.text_opacity * 255.0) as u32;
                gui.text(queue, chars, 0.0, restricted_top, ta << 24 | 0xFF_FFFF, true, 0);
                if let Some((lx, ly)) = local
                    && ly >= restricted_top
                    && ly < restricted_top + 9.0
                {
                    self.hover = crate::links::style_at(gui, chars, lx).and_then(|s| s.hover.clone());
                }
            }
            // Scroll bar (chat open, history longer than the page).
            let total = self.lines.len();
            if open && total > 0 {
                let count = vis.len() as f32;
                let chat_h = count * g.entry_h;
                let virt = total as f32 * g.entry_h;
                if virt != chat_h {
                    let y = (self.scroll as f32 * chat_h / total as f32).floor() - g.bottom;
                    let bh = (chat_h * chat_h / virt).floor();
                    let alpha: u32 = if y > 0.0 { 170 } else { 96 };
                    let color = if self.new_message { 0xCC3333 } else { 0x3333AA };
                    let x = g.max_w + 4.0;
                    gui.fill(x, -y - bh, x + 2.0, -y, alpha << 24 | color);
                    gui.fill(x + 1.0, -y - bh, x + 2.0, -y, alpha << 24 | 0xCCCCCC);
                }
            }
            gui.transform_since(mark, [g.s, 0.0, 0.0, g.s, 4.0 * g.s, 0.0]);
        }
        // Field (and IME cursor, `ime::field_cursor`) only if the input is visible.
        if self.shown() {
            self.draw_input(gui, queue);
        }
    }

    /// Input field (`chat.md` § 4, borderless at x = 4, y = h − 12), drawn and measured without
    /// "suspect" glyphs like the game.
    fn draw_input(&mut self, gui: &mut Gui, queue: &wgpu::Queue) {
        gui.filter_fishy.set(true);
        self.draw_input_text(gui, queue);
        gui.filter_fishy.set(false);
    }

    fn draw_input_text(&mut self, gui: &mut Gui, queue: &wgpu::Queue) {
        let (w, h) = (gui.width, gui.height);
        let bg = if self.settings.background_chat_only {
            0x8000_0000
        } else {
            ((self.settings.background * 255.0) as u32) << 24
        };
        let format = self.format.clone();
        let ghost = self.ghost.clone();
        let Some(i) = &mut self.input else { return };
        gui.fill(2.0, h - 14.0, w - 2.0, h - 2.0, bg);
        let inner = w - 4.0;
        if i.rescroll {
            let c = i.cursor;
            i.scroll_to(gui, c, inner);
            i.rescroll = false;
        }
        let (tx, ty) = (4.0f32, h - 12.0);
        let color = 0xFFE0_E0E0;
        let n = i.text.len();
        let start = i.display.min(n);
        let shown = Input::fit(gui, &i.text[start..], inner);
        let style = |k: usize| {
            let mut st = CharStyle::default();
            if i.is_draft {
                st.color = Some(0xAAAAAA);
                st.italic = true;
            } else if let Some(rgb) = format.as_ref().and_then(|f| f.get(k)) {
                st.color = Some(*rgb);
            }
            st
        };
        let chars: Vec<StyledChar> =
            (start..start + shown).map(|k| StyledChar { c: i.text[k], style: style(k) }).collect();
        let rel = i.cursor as i64 - start as i64;
        let on_screen = rel >= 0 && rel as usize <= chars.len();
        let width = |gui: &Gui, cs: &[StyledChar]| rc_text::width(gui, cs) as f32;
        let mut draw_x = tx;
        if !chars.is_empty() {
            let half = if on_screen { &chars[..rel as usize] } else { &chars[..] };
            gui.text_forward(queue, half, draw_x, ty, color, true);
            draw_x += width(gui, half) + 1.0;
        }
        let insert = i.cursor < n || n >= MAX_INPUT;
        let mut cursor_x = draw_x;
        if !on_screen {
            cursor_x = if rel > 0 { tx + inner } else { tx };
        } else if insert {
            cursor_x -= 1.0;
            draw_x -= 1.0;
        }
        if !chars.is_empty() && on_screen && (rel as usize) < chars.len() {
            gui.text_forward(queue, &chars[rel as usize..], draw_x, ty, color, true);
        }
        if !insert && let Some(g) = &ghost {
            gui.text_string(queue, &crate::gui::plain(g), cursor_x - 1.0, ty, 0xFF80_8080, true);
        }
        // IME: cursor of the field (composition drawn at the end of the frame, `ime.rs`).
        crate::ime::field_cursor(cursor_x as i32, ty as i32, 10);
        let rel_anchor = (i.anchor as i64 - start as i64).clamp(0, chars.len() as i64) as usize;
        if on_screen && rel_anchor != rel as usize || !on_screen && i.anchor != i.cursor {
            let hx = tx + width(gui, &chars[..rel_anchor]);
            let (x0, x1) = (cursor_x.min(4.0 + inner), (hx - 1.0).min(4.0 + inner));
            gui.text_highlight(x0.min(x1), ty - 1.0, x0.max(x1), ty + 10.0, true);
        }
        let blink = (i.focus.elapsed().as_millis() / 300) % 2 == 0;
        if blink && on_screen {
            if insert {
                gui.fill(cursor_x, ty - 1.0, cursor_x + 1.0, ty + 10.0, color);
            } else {
                gui.text(queue, &crate::gui::plain("_"), cursor_x, ty, color, true, 0);
            }
        }
    }

    /// Click in the field (`x`, `y` in interface units): cursor placed, Shift extends,
    /// double click selects the word; true if the click fell in the field.
    pub fn field_click(&mut self, gui: &Gui, m: (f32, f32), shift: bool, double: bool) -> bool {
        let (w, h) = (gui.width, gui.height);
        let Some(i) = &mut self.input else { return false };
        if !(m.0 >= 4.0 && m.0 < w && m.1 >= h - 12.0 && m.1 < h) {
            return false;
        }
        let k = i.index_at(gui, m.0, w - 4.0);
        if double {
            let (a, b) = (i.word(k, -1), i.word(k, 1));
            i.move_to(a, false);
            i.move_to(b, true);
        } else {
            i.move_to(k, shift);
        }
        self.dragging = true;
        true
    }

    /// Drag in the field: the selection follows the mouse.
    pub fn field_drag(&mut self, gui: &Gui, m: (f32, f32)) {
        let w = gui.width;
        if let Some(i) = &mut self.input
            && self.dragging
        {
            let k = i.index_at(gui, m.0, w - 4.0);
            i.move_to(k, true);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Menus on top of the open chat (restrictions screen): input not visible, hence no
    /// field, hover or IME cursor; on return: chat visible again, scrolling reset to 0.
    #[test]
    fn chat_under_menus_is_drawn_closed() {
        let mut c = Chat::default();
        c.open("hello");
        assert!(c.shown());
        c.scroll = 5;
        c.new_message = true;
        c.covered = true;
        c.reset_scroll();
        assert!(c.is_open() && !c.shown());
        assert_eq!((c.scroll, c.new_message), (0, false));
        c.covered = false;
        assert!(c.shown());
    }

    struct Fixed;
    impl rc_text::Metrics for Fixed {
        fn advance(&self, _: &str, c: char) -> (f32, f32) {
            (if c == ' ' { 4.0 } else { 6.0 }, 1.0)
        }
    }

    /// Continuation line of a Hebrew message (right-to-left language): the indent stays on the left,
    /// the line is reordered without it (the game sticks the indent in front of the reordered line).
    #[test]
    fn continuation_indent_stays_left_in_rtl_paragraph() {
        let link = CharStyle { insertion: Some("x".into()), ..CharStyle::default() };
        let mut chars = vec![StyledChar { c: ' ', style: CharStyle::default() }];
        chars.extend("שלום".chars().map(|c| StyledChar { c, style: link.clone() }));
        let line = Line { chars, tick: 0, tag: ChatTag::Secure, original: None, end: true, indent: true };
        let order_of = |c: &[StyledChar]| rc_text::bidi::visual(c, true).map(Arc::from);
        let at = |x: f32| line_style_in(&Fixed, &line, &order_of, x).map(|s| s.insertion.is_some());
        // Indent 0..4, Hebrew 4..28; reordered as a block, the space used to end up on the right (24..28).
        assert_eq!(at(1.0), Some(false));
        assert_eq!(at(5.0), Some(true));
        assert_eq!(at(27.0), Some(true));
        assert_eq!(at(29.0), None);
    }

    /// Commands kept from one launch to the next (`command_history.txt`): re-read into
    /// the input history, 50 at most, no consecutive duplicate; messages do not go there.
    #[test]
    fn command_history_persists_between_launches() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-cmdhist");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("command_history.txt");
        let _ = std::fs::remove_file(&path);
        let chat_at = || {
            let mut c = Chat::default();
            c.commands.path = Some(path.clone());
            c
        };
        let send = |c: &mut Chat, s: &str| {
            c.open(s);
            c.submit()
        };
        let mut c = chat_at();
        for i in 0..55 {
            send(&mut c, &format!("/say {i}"));
        }
        send(&mut c, "/say 54");
        send(&mut c, "hello");
        let saved = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = saved.lines().collect();
        assert_eq!(lines.len(), 50);
        assert_eq!(lines[0], "/say 5");
        assert_eq!(lines[49], "/say 54");
        // New launch: up arrow = last command of the previous launch.
        let mut c = chat_at();
        c.open("");
        assert!(c.history_step(-1));
        assert_eq!(c.input.as_ref().unwrap().value(), "/say 54");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Edited message: split 15 px shorter (icon 9 + margins 4 and 2), the others not.
    #[test]
    fn modified_message_wraps_short_of_its_icon() {
        let lines_of = |tag| {
            let mut c = Chat::default();
            c.settings.scale = 1.0;
            c.settings.width = 0.0; // 40 px wide
            c.add(Component::text("aaaaaa"), tag); // 36 px
            c.rewrap(&Fixed, &rc_text::Lang::default());
            c.lines.len()
        };
        assert_eq!(lines_of(ChatTag::Secure), 1);
        assert_eq!(lines_of(ChatTag::Modified), 2);
    }

    /// Chat in bed: a send keeps the screen with an empty field; the history rank from before
    /// the send is kept (the up arrow skips the message just sent, like the game). On
    /// waking, a resumed draft closes the chat like an empty field.
    #[test]
    fn bed_chat_send_keeps_the_screen_and_wake_closes_drafts() {
        let mut c = Chat::default();
        for s in ["one", "two"] {
            c.open(s);
            c.submit();
        }
        c.open("three");
        assert_eq!(c.submit(), Some((false, "three".into())));
        c.reopen_after_send();
        let i = c.input.as_ref().expect("screen kept");
        assert!(i.text.is_empty() && !i.is_draft && i.closes_on_wake());
        assert!(c.history_step(-1));
        assert_eq!(c.input.as_ref().unwrap().value(), "two");
        c.input = Some(Input::new("draft", true));
        assert!(c.input.as_ref().unwrap().closes_on_wake(), "draft kept on wake");
        c.input = Some(Input::new("typed", false));
        assert!(!c.input.as_ref().unwrap().closes_on_wake());
    }

    /// `reset_chat` and disconnection: only the messages go; settings, draft and
    /// kept commands stay, the input history restarts from the kept commands.
    #[test]
    fn reset_chat_keeps_settings_draft_and_saved_commands() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/test-resetchat");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("command_history.txt");
        let _ = std::fs::remove_file(&path);
        let mut c = Chat::default();
        c.commands.path = Some(path);
        c.settings.width = 0.25;
        for s in ["/say a", "hello"] {
            c.open(s);
            c.submit();
        }
        c.add(Component::text("x"), ChatTag::System);
        c.draft = Some("draft".into());
        for disconnect in [false, true] {
            if disconnect {
                c.add(Component::text("y"), ChatTag::System);
                c.disconnected();
            } else {
                c.clear_session();
            }
            assert!(c.messages.is_empty() && c.lines.is_empty() && c.deletions.is_empty());
            assert_eq!(c.settings.width, 0.25);
            assert_eq!(c.draft.as_deref(), Some("draft"));
            assert_eq!(c.history, ["/say a"]);
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn visibility_filters_by_source() {
        // Shown: everything is visible; commands only: no more player messages;
        // hidden: only the client's messages remain.
        for (vis, client, player, server) in [(0, true, true, true), (1, true, false, true), (2, true, false, false)] {
            assert_eq!(Source::Client.visible(vis), client);
            assert_eq!(Source::Player.visible(vis), player);
            assert_eq!(Source::Server.visible(vis), server);
        }
    }

    /// Restricted chat prompt: red underlined, click towards the restrictions screen; cut
    /// with "..." and the tooltip of the whole text when the chat is too narrow; placed on the
    /// line above the visible messages.
    #[test]
    fn restricted_prompt_is_red_underlined_clickable_and_clipped() {
        struct Fixed;
        impl rc_text::Metrics for Fixed {
            fn advance(&self, _font: &str, _c: char) -> (f32, f32) {
                (6.0, 1.0)
            }
        }
        let lang = rc_text::Lang::default();
        let full = restricted_prompt(&Fixed, &lang, 320.0);
        let text: String = full.iter().map(|c| c.c).collect();
        assert_eq!(text, "chat_screen.restricted");
        assert!(full.iter().all(|c| c.style.color == Some(0xFF5555) && c.style.underlined && c.style.hover.is_none()));
        assert!(full.iter().all(|c| c.style.click.as_ref().is_some_and(|e| e.value == GO_TO_RESTRICTIONS)));
        let cut = restricted_prompt(&Fixed, &lang, 60.0);
        let text: String = cut.iter().map(|c| c.c).collect();
        assert_eq!(text, "chat_sc...");
        assert!(cut[..7].iter().all(|c| c.style.hover.is_some()) && cut[7..].iter().all(|c| c.style.click.is_none()));
        let g = Geo { s: 1.0, max_w: 320.0, bottom: 180.0, entry_h: 9.0, to_msg: 8.0 };
        assert_eq!(Chat::restricted_top(&g, 3), 180.0 - 4.0 * 9.0 + 9.0 - 8.0 - 1.0);
    }

    #[test]
    fn source_follows_the_tag() {
        assert_eq!(Source::of(ChatTag::System), Source::Server);
        for t in [ChatTag::Secure, ChatTag::NotSecure, ChatTag::Modified, ChatTag::Error, ChatTag::Disguised] {
            assert_eq!(Source::of(t), Source::Player);
        }
    }
}
