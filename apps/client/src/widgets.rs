//! Common interface components (`docs/specs/option-menus.md` § 1): atlas nine-slices,
//! button, cycle button, slider, checkbox, text field, section title, tooltip,
//! scrolling list, scrolling centred text, focus and keyboard navigation, screen backgrounds.
//!
//! A [`Panel`] carries the components of a screen: it receives mouse and keyboard, hands
//! [`Event`]s back to the owning screen and counts the clicks to sound.

use std::time::Instant;

use rc_text::StyledChar;
use winit::keyboard::{KeyCode, ModifiersState};

use crate::gui::Gui;

/// Interface click sound ("ui" category, volume 0.25, unpositioned).
pub const CLICK_SOUND: &str = "minecraft:ui.button.click";

/// Translated text, arguments in plain text.
pub fn tr(lang: &rc_text::Lang, key: &str, args: &[String]) -> String {
    let c = rc_text::Component::translate(key, args.iter().map(|a| rc_text::Arg::Text(a.clone())).collect());
    rc_text::flatten(&c, lang).iter().map(|s| s.text.as_str()).collect()
}

/// Styled characters of a text (§ codes included).
pub fn chars(s: &str) -> Vec<StyledChar> {
    crate::gui::plain(s)
}

/// Bold underlined text (section titles).
fn underlined_bold(s: &[StyledChar]) -> Vec<StyledChar> {
    s.iter()
        .map(|c| {
            let mut c = c.clone();
            c.style.bold = true;
            c.style.underlined = true;
            c
        })
        .collect()
}

/// Text field (§ 1.7).
#[derive(Debug, Clone, PartialEq)]
pub struct TextField {
    pub text: Vec<char>,
    pub cursor: usize,
    /// Other end of the selection (= cursor if none).
    pub anchor: usize,
    /// First visible character.
    pub offset: usize,
    pub max_len: usize,
    pub hint: Vec<StyledChar>,
    pub editable: bool,
    pub bordered: bool,
    /// Colour of the editable text (red for an invalid input).
    pub text_color: u32,
    /// Colour of the text when the field is not editable.
    pub uneditable_color: u32,
    /// Selection highlight with colour inversion (false: blue addition only).
    pub invert_highlight: bool,
    focus_at: Option<Instant>,
    /// RGB colour per character (parsed command) and ghost suggestion text.
    pub colors: Option<Vec<u32>>,
    pub ghost: Option<String>,
    /// Text centred in the frame (signing title of a book).
    pub centered: bool,
    /// Drop shadow of the text (true by default).
    pub shadow: bool,
    /// Part displayed at the last draw: first character, abscissa of the text from the edge
    /// of the frame, cumulated widths (unrounded) of the following characters; used for the click.
    drawn: std::cell::RefCell<(usize, i32, Vec<f32>)>,
}

impl Default for TextField {
    fn default() -> Self {
        TextField::new(Vec::new())
    }
}

impl TextField {
    pub fn new(hint: Vec<StyledChar>) -> TextField {
        TextField {
            text: Vec::new(),
            cursor: 0,
            anchor: 0,
            offset: 0,
            max_len: 32,
            hint,
            editable: true,
            bordered: true,
            text_color: 0xffe0_e0e0,
            uneditable_color: 0xff70_7070,
            invert_highlight: true,
            focus_at: None,
            colors: None,
            ghost: None,
            centered: false,
            shadow: true,
            drawn: Default::default(),
        }
    }

    /// Character aimed at by a click at `dx` pixels from the left edge of the frame, like the game: number
    /// of displayed characters whose cumulated width fits in the gap to the start of the text
    /// (bounded by the useful width `inner`), after the first displayed character.
    fn index_at(&self, dx: i32, inner: i32) -> usize {
        let d = self.drawn.borrow();
        let room = (dx - d.1).min(inner) as f32;
        (d.0 + d.2.iter().take_while(|&&w| w <= room).count()).min(self.text.len())
    }

    /// Click in the field (`dx` from the left edge of the frame): cursor placed, Shift extends the
    /// selection, double click selects the word.
    fn click(&mut self, dx: i32, inner: i32, shift: bool, double: bool) {
        let k = self.index_at(dx, inner);
        if double {
            let (a, b) = (word_pos(&self.text, k, -1), word_pos(&self.text, k, 1));
            self.move_to(a, false);
            self.move_to(b, true);
        } else {
            self.move_to(k, shift);
        }
    }

    pub fn value(&self) -> String {
        self.text.iter().collect()
    }

    pub(crate) fn selection(&self) -> (usize, usize) {
        (self.cursor.min(self.anchor), self.cursor.max(self.anchor))
    }

    fn delete_selection(&mut self) -> bool {
        let (a, b) = self.selection();
        if a == b {
            return false;
        }
        self.text.drain(a..b);
        self.cursor = a;
        self.anchor = a;
        true
    }

    /// Value imposed like the game: no filter, cut at `max_len` UTF-16 units, cursor at
    /// the end (a surrogate pair cut in two is removed whole).
    pub(crate) fn set_value(&mut self, s: &str) {
        let mut t: Vec<char> = s.chars().collect();
        crate::book::truncate_u16(&mut t, self.max_len);
        self.cursor = t.len();
        self.anchor = self.cursor;
        self.text = t;
    }

    /// Input like the game: `§`, characters below space and DEL removed; length counted
    /// in UTF-16 units, without cutting a character outside the basic plane.
    pub(crate) fn insert(&mut self, s: &str) {
        self.delete_selection();
        let mut len = crate::book::u16_len(&self.text);
        for c in s.chars().filter(|&c| c != '§' && c >= ' ' && c != '\u{7f}') {
            len += c.len_utf16();
            if len > self.max_len {
                break;
            }
            self.text.insert(self.cursor, c);
            self.cursor += 1;
        }
        self.anchor = self.cursor;
    }

    /// Start of the word before / after `from` (Ctrl + arrows, Ctrl + delete).
    fn word(&self, from: usize, dir: i32) -> usize {
        word_pos(&self.text, from, dir)
    }

    fn move_to(&mut self, to: usize, extend: bool) {
        self.cursor = to.min(self.text.len());
        if !extend {
            self.anchor = self.cursor;
        }
    }

    /// Key with focus; true if the text changed.
    pub(crate) fn key(&mut self, k: KeyCode, mods: ModifiersState) -> (bool, bool) {
        let (ctrl, shift) = (mods.control_key(), mods.shift_key());
        // Select all, copy, paste, cut: Ctrl alone (neither Shift nor Alt), like the game.
        let shortcut = ctrl && !shift && !mods.alt_key();
        let n = self.text.len();
        let before = self.text.clone();
        let mut used = true;
        match k {
            KeyCode::Backspace | KeyCode::Delete if self.editable => {
                if !self.delete_selection() {
                    let dir = if k == KeyCode::Backspace { -1 } else { 1 };
                    let to = if ctrl {
                        self.word(self.cursor, dir)
                    } else if dir < 0 {
                        self.cursor.saturating_sub(1)
                    } else {
                        (self.cursor + 1).min(n)
                    };
                    let (a, b) = (to.min(self.cursor), to.max(self.cursor));
                    self.text.drain(a..b);
                    self.cursor = a;
                    self.anchor = a;
                }
            }
            KeyCode::Home => self.move_to(0, shift),
            KeyCode::End => self.move_to(n, shift),
            KeyCode::ArrowLeft => {
                let to = if ctrl { self.word(self.cursor, -1) } else { self.cursor.saturating_sub(1) };
                self.move_to(to, shift);
            }
            KeyCode::ArrowRight => {
                let to = if ctrl { self.word(self.cursor, 1) } else { self.cursor + 1 };
                self.move_to(to, shift);
            }
            KeyCode::KeyA if shortcut => {
                self.anchor = 0;
                self.cursor = n;
            }
            KeyCode::KeyC | KeyCode::KeyX if shortcut => {
                let (a, b) = self.selection();
                set_clipboard(self.text[a..b].iter().collect());
                if k == KeyCode::KeyX && self.editable {
                    self.delete_selection();
                }
            }
            KeyCode::KeyV if shortcut && self.editable => self.insert(&clipboard()),
            _ => used = false,
        }
        (used, self.text != before)
    }
}

/// Start of the word before (`dir` < 0) or after `from`, spaces skipped like the game (simple fields
/// and panel lines, where the text has no line break).
pub(crate) fn word_pos(text: &[char], from: usize, dir: i32) -> usize {
    let mut i = from.min(text.len());
    if dir < 0 {
        while i > 0 && text[i - 1] == ' ' {
            i -= 1;
        }
        while i > 0 && text[i - 1] != ' ' {
            i -= 1;
        }
    } else {
        let n = text.len();
        while i < n && text[i] != ' ' {
            i += 1;
        }
        while i < n && text[i] == ' ' {
            i += 1;
        }
    }
    i
}

/// Advance of a styled character (bold included), like the measurement of text widths.
fn advance(m: &dyn rc_text::Metrics, c: &StyledChar) -> f32 {
    let (a, bold) = m.advance(&c.style.font, c.c);
    if c.style.bold { a + bold } else { a }
}

/// Physical key (position code): the one the game's shortcuts compare
/// (reassignable keys), unlike the shortcuts of the text fields.
pub fn physical_key(event: &winit::event::KeyEvent) -> Option<KeyCode> {
    match event.physical_key {
        winit::keyboard::PhysicalKey::Code(c) => Some(c),
        _ => None,
    }
}

/// Key passed to the input fields ONLY: with Ctrl, the letter of the keyboard layout (the game
/// reads key codes according to the layout: Ctrl+A on AZERTY is the key
/// marked A), otherwise the physical key. The game's shortcuts use [`physical_key`].
pub fn edit_key(event: &winit::event::KeyEvent, mods: ModifiersState) -> Option<KeyCode> {
    use winit::keyboard::{Key, PhysicalKey};
    const LETTERS: [KeyCode; 26] = [
        KeyCode::KeyA,
        KeyCode::KeyB,
        KeyCode::KeyC,
        KeyCode::KeyD,
        KeyCode::KeyE,
        KeyCode::KeyF,
        KeyCode::KeyG,
        KeyCode::KeyH,
        KeyCode::KeyI,
        KeyCode::KeyJ,
        KeyCode::KeyK,
        KeyCode::KeyL,
        KeyCode::KeyM,
        KeyCode::KeyN,
        KeyCode::KeyO,
        KeyCode::KeyP,
        KeyCode::KeyQ,
        KeyCode::KeyR,
        KeyCode::KeyS,
        KeyCode::KeyT,
        KeyCode::KeyU,
        KeyCode::KeyV,
        KeyCode::KeyW,
        KeyCode::KeyX,
        KeyCode::KeyY,
        KeyCode::KeyZ,
    ];
    let PhysicalKey::Code(physical) = event.physical_key else { return None };
    if mods.control_key()
        && let Key::Character(s) = &event.logical_key
        && let Some(c) = s.chars().next().map(|c| c.to_ascii_lowercase())
        && c.is_ascii_lowercase()
    {
        return Some(LETTERS[(c as u8 - b'a') as usize]);
    }
    Some(physical)
}

/// Text of a keystroke for the input fields, to be taken here and nowhere else. With Ctrl,
/// winit returns on Windows the letter computed WITHOUT Ctrl (Ctrl+A gives "a"), whereas the game
/// only receives a control character, which it discards: so we read the text computed with all
/// the modifiers (Ctrl+A gives U+0001, discarded; AltGr, which arrives as Ctrl + Alt, gives its
/// real character, accepted).
pub fn typed_text(event: &winit::event::KeyEvent, mods: ModifiersState) -> Option<&str> {
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    let all = winit::platform::modifier_supplement::KeyEventExtModifierSupplement::text_with_all_modifiers(event);
    // Other platforms: no "all modifiers" text, Ctrl alone types nothing.
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    let all = event.text.as_deref().filter(|_| mods.alt_key());
    keep_typed(event.text.as_deref(), all, mods)
}

/// Rule of [`typed_text`]: with Ctrl, the "all modifiers" text; never a control character
/// (DEL included).
pub fn keep_typed<'a>(text: Option<&'a str>, all_mods: Option<&'a str>, mods: ModifiersState) -> Option<&'a str> {
    let t = if mods.control_key() { all_mods } else { text };
    t.filter(|t| !t.is_empty() && !t.chars().any(char::is_control))
}

/// Internal fallback copy (system clipboard unreadable or empty).
#[cfg(not(test))]
static CLIPBOARD: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

/// Text copied to the system clipboard.
pub fn copy_to_clipboard(s: String) {
    set_clipboard(s);
}

#[cfg(test)]
thread_local! {
    /// Tests: clipboard specific to each thread (neither the system's nor shared between tests).
    static TEST_CLIPBOARD: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

pub(crate) fn set_clipboard(s: String) {
    #[cfg(test)]
    {
        TEST_CLIPBOARD.with_borrow_mut(|c| *c = s);
    }
    #[cfg(not(test))]
    {
        crate::links::set_clipboard(&s);
        *CLIPBOARD.lock().unwrap_or_else(|p| p.into_inner()) = s;
    }
}

/// System clipboard, otherwise the last internal copy.
pub(crate) fn clipboard() -> String {
    #[cfg(test)]
    {
        TEST_CLIPBOARD.with_borrow(Clone::clone)
    }
    #[cfg(not(test))]
    {
        let s = crate::links::get_clipboard();
        if !s.is_empty() {
            return s;
        }
        CLIPBOARD.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

/// Kind of a component.
#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Button,
    /// Cycle button "name: value" (§ 1.4): returns [`Event::Cycle`].
    Cycle,
    /// Icon button (15 × 15 sprite centred; tooltip only if given).
    Icon(&'static str),
    /// Text and icon button: scrolling message on the left, sprite (width, height) at 2 px from the
    /// right edge.
    IconText(&'static str, i32, i32),
    /// Button drawn by a sprite at its size (`<name>`, `<name>_highlighted` on hover or
    /// focus, `<name>_disabled` inactive): difficulty lock, social interaction buttons.
    Sprite(&'static str),
    /// Slider (§ 1.5): position in [0, 1], number of notches (0 = continuous), adjust mode.
    Slider {
        value: f64,
        steps: u32,
        adjusting: bool,
    },
    Checkbox {
        checked: bool,
    },
    Field(Box<TextField>),
    /// Section title (bold underlined, focusable).
    Title,
    /// Simple text, not focusable: colour, centred.
    Label {
        color: u32,
        centered: bool,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Widget {
    pub id: u32,
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub msg: Vec<StyledChar>,
    /// Tooltip lines (already cut at 170).
    pub tooltip: Vec<Vec<StyledChar>>,
    pub active: bool,
    pub visible: bool,
    pub alpha: f32,
    pub kind: Kind,
    /// In the scrolling list: `y` in content coordinates.
    pub in_list: bool,
}

impl Widget {
    pub fn new(id: u32, kind: Kind, x: i32, y: i32, w: i32, h: i32, msg: Vec<StyledChar>) -> Widget {
        Widget {
            id,
            x,
            y,
            w,
            h,
            msg,
            tooltip: Vec::new(),
            active: true,
            visible: true,
            alpha: 1.0,
            kind,
            in_list: false,
        }
    }

    pub fn button(id: u32, x: i32, y: i32, w: i32, msg: &str) -> Widget {
        Widget::new(id, Kind::Button, x, y, w, 20, chars(msg))
    }

    pub fn inactive(mut self) -> Widget {
        self.active = false;
        self
    }

    /// Tooltip cut at 170 wide (§ 1.8).
    pub fn with_tooltip(mut self, gui: &Gui, text: &str) -> Widget {
        self.set_tooltip(gui, text);
        self
    }

    pub fn set_tooltip(&mut self, gui: &Gui, text: &str) {
        self.tooltip = if text.is_empty() { Vec::new() } else { rc_text::wrap(gui, &chars(text), 170.0) };
    }

    /// Lines of the tooltip displayed on hover or focus: the one given by the screen only.
    fn shown_tooltip(&self) -> &[Vec<StyledChar>] {
        &self.tooltip
    }

    fn focusable(&self) -> bool {
        self.visible && self.active && !matches!(self.kind, Kind::Label { .. })
    }
}

/// Entry of a selection list (languages): height and centred label.
#[derive(Debug, Clone, PartialEq)]
pub struct Entry {
    pub top: i32,
    pub h: i32,
    pub label: Vec<StyledChar>,
}

/// Scrolling list (§ 1.9).
#[derive(Debug, Clone, PartialEq)]
pub struct List {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub row_width: i32,
    pub scroll: f64,
    /// Height of the content (sum of the entries + 4).
    pub content: i32,
    /// Wheel step (default height / 2).
    pub step: i32,
    /// Selectable entries (language list); empty for the component lists.
    pub entries: Vec<Entry>,
    pub selected: Option<usize>,
    pub inworld: bool,
    /// Entries drawn by the screen (`Panel::draw_with`): neither label nor selection here.
    pub custom_rows: bool,
    /// The list has focus (entry clicked or chosen with the arrows, no component focused):
    /// outline of the chosen entry white instead of grey.
    pub focused: bool,
    /// List background and separators drawn (false for the statistics lists).
    pub background: bool,
    pub separators: bool,
}

impl List {
    pub fn new(x: i32, y: i32, w: i32, h: i32, row_width: i32, item_h: i32) -> List {
        List {
            x,
            y,
            w,
            h,
            row_width,
            scroll: 0.0,
            content: 4,
            step: item_h / 2,
            entries: Vec::new(),
            selected: None,
            inworld: true,
            custom_rows: false,
            focused: false,
            background: true,
            separators: true,
        }
    }

    /// Rectangle of an entry on screen (scrolling applied): x, y, width, height.
    pub fn entry_rect(&self, i: usize) -> Option<(i32, i32, i32, i32)> {
        let e = self.entries.get(i)?;
        Some((self.row_left(), self.screen_y(2 + e.top), self.row_width, e.h))
    }

    /// Entry under the mouse (full rectangle of the row).
    pub fn entry_under(&self, m: (f32, f32)) -> Option<usize> {
        self.entry_at(m)
    }

    pub fn row_left(&self) -> i32 {
        self.x + self.w / 2 - self.row_width / 2
    }

    pub fn max_scroll(&self) -> f64 {
        f64::from((self.content - self.h).max(0))
    }

    pub fn set_scroll(&mut self, s: f64) {
        self.scroll = s.clamp(0.0, self.max_scroll());
    }

    fn bar_x(&self) -> i32 {
        self.row_left() + self.row_width + 6 + 2
    }

    fn bar_height(&self) -> i32 {
        ((self.h * self.h) / self.content.max(1)).clamp(32, (self.h - 8).max(32))
    }

    fn contains(&self, m: (f32, f32)) -> bool {
        m.0 >= self.x as f32 && m.0 < (self.x + self.w) as f32 && m.1 >= self.y as f32 && m.1 < (self.y + self.h) as f32
    }

    /// Screen ordinate of a content ordinate.
    fn screen_y(&self, y: i32) -> i32 {
        self.y - self.scroll.floor() as i32 + y
    }

    /// Brings `[top, bottom]` (content) into the visible area, margin 2.
    fn reveal(&mut self, top: i32, bottom: i32) {
        let (vt, vb) = (self.scroll as i32, self.scroll as i32 + self.h);
        if top - 2 < vt {
            self.set_scroll(f64::from(top - 2));
        } else if bottom + 2 > vb {
            self.set_scroll(f64::from(bottom + 2 - self.h));
        }
    }

    /// Entry under the mouse (useful area: entry shrunk by 2).
    fn entry_at(&self, m: (f32, f32)) -> Option<usize> {
        if !self.contains(m) {
            return None;
        }
        let left = self.row_left() as f32;
        if m.0 < left || m.0 >= left + self.row_width as f32 {
            return None;
        }
        let cy = m.1 - self.y as f32 + self.scroll.floor() as f32;
        self.entries.iter().position(|e| cy >= (2 + e.top) as f32 && cy < (2 + e.top + e.h) as f32)
    }
}

/// Return of a component to the owning screen.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Button (mouse or validation).
    Press(u32),
    /// Cycle button: +1 next, −1 previous (Shift or wheel up).
    Cycle(u32, i32),
    /// New position of a slider (mouse or arrows).
    Slide(u32, f64),
    /// Slider released.
    SlideEnd(u32),
    /// Discrete slider in adjust mode: previous (−1) or next (+1) value.
    Step(u32, i32),
    Toggle(u32, bool),
    /// Text modified.
    Edit(u32),
    /// List entry chosen (click), and validated (double click or validation).
    Select(usize),
    Activate(usize),
}

/// Components of a screen, focus, list and tooltip.
#[derive(Default)]
pub struct Panel {
    pub widgets: Vec<Widget>,
    pub list: Option<List>,
    pub focus: Option<usize>,
    hover: Option<usize>,
    /// Slider held by the mouse.
    dragging: Option<usize>,
    bar_drag: bool,
    /// Last input = keyboard (tooltip on focus, initial focus).
    pub keyboard: bool,
    /// Clicks to sound.
    pub clicks: u32,
    last_select: Option<(Instant, usize)>,
    /// Screen size at the last draw (starting edge of the arrows without focus).
    pub screen: (i32, i32),
    /// Last left click (double click: under 250 ms, wherever it is on the screen).
    last_click: Option<Instant>,
    /// Text field held by the mouse (the selection follows the drag).
    drag_field: Option<usize>,
}

impl Panel {
    pub fn new(widgets: Vec<Widget>) -> Panel {
        Panel { widgets, ..Default::default() }
    }

    pub fn get(&self, id: u32) -> Option<&Widget> {
        self.widgets.iter().find(|w| w.id == id)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut Widget> {
        self.widgets.iter_mut().find(|w| w.id == id)
    }

    /// Focus imposed by the screen (input field on opening).
    pub fn focus_widget(&mut self, id: u32) {
        let i = self.widgets.iter().position(|w| w.id == id);
        self.set_focus(i, true);
    }

    /// Initial focus (§ 1.11): first component if the last input is the keyboard.
    pub fn initial_focus(&mut self, keyboard: bool) {
        self.keyboard = keyboard;
        if keyboard {
            self.tab(false);
        }
    }

    /// Screen rectangle of a component (list: scrolling applied).
    pub fn rect(&self, w: &Widget) -> (i32, i32, i32, i32) {
        match (&self.list, w.in_list) {
            (Some(l), true) => (w.x, l.screen_y(w.y), w.w, w.h),
            _ => (w.x, w.y, w.w, w.h),
        }
    }

    fn hit(&self, i: usize, m: (f32, f32)) -> bool {
        let w = &self.widgets[i];
        if !w.visible {
            return false;
        }
        if w.in_list && !self.list.as_ref().is_some_and(|l| l.contains(m)) {
            return false;
        }
        let (x, y, ww, hh) = self.rect(w);
        let (x, y, ww, hh) = (x as f32, y as f32, ww as f32, hh as f32);
        m.0 >= x && m.0 < x + ww && m.1 >= y && m.1 < y + hh
    }

    fn at(&self, m: (f32, f32)) -> Option<usize> {
        (0..self.widgets.len()).rev().find(|&i| self.hit(i, m))
    }

    fn set_focus(&mut self, f: Option<usize>, by_mouse_or_tab: bool) {
        if self.focus == f {
            return;
        }
        if let Some(old) = self.focus.and_then(|i| self.widgets.get_mut(i)) {
            if let Kind::Slider { adjusting, .. } = &mut old.kind {
                *adjusting = false;
            }
            if let Kind::Field(t) = &mut old.kind {
                t.focus_at = None;
            }
        }
        self.focus = f;
        let Some(i) = f else { return };
        if let Some(l) = &mut self.list {
            l.focused = false;
        }
        match &mut self.widgets[i].kind {
            Kind::Slider { adjusting, .. } => *adjusting = by_mouse_or_tab,
            Kind::Field(t) => t.focus_at = Some(Instant::now()),
            _ => {}
        }
        if self.widgets[i].in_list {
            let (top, bottom) = (self.widgets[i].y, self.widgets[i].y + self.widgets[i].h);
            if let Some(l) = &mut self.list {
                l.reveal(top, bottom);
            }
        }
    }

    /// Tab / Shift+Tab (§ 1.11): at the end of the order, the focus restarts from the other end in the same
    /// keystroke, like the game (focus removed then new search).
    fn tab(&mut self, back: bool) {
        let order: Vec<usize> = (0..self.widgets.len()).filter(|&i| self.widgets[i].focusable()).collect();
        if order.is_empty() {
            return;
        }
        let pos = self.focus.and_then(|f| order.iter().position(|&i| i == f));
        let next = match (pos, back) {
            (Some(p), false) => order.get(p + 1).copied(),
            (Some(p), true) => p.checked_sub(1).map(|p| order[p]),
            (None, _) => None,
        };
        let wrap = if back { order.last().copied() } else { Some(order[0]) };
        self.set_focus(next.or(wrap), true);
    }

    /// Spatial navigation with the arrows (§ 1.11) like the game; `d` = unit (dx, dy). The starting point
    /// is the one-pixel band right after the edge of the focused component (without focus:
    /// just before the opposite edge of the screen). Candidates first among those that overlap
    /// this band on the other axis, the nearest then the topmost / leftmost; otherwise the
    /// nearest by the distance between the middle of the starting edge and the middle of the
    /// candidate's entry edge.
    fn arrow(&mut self, d: (i32, i32)) {
        let horizontal = d.0 != 0;
        let positive = d.0 + d.1 > 0;
        // Rectangle with inclusive bounds (left, top, right, bottom).
        let bounds = |(x, y, w, h): (i32, i32, i32, i32)| (x, y, x + w - 1, y + h - 1);
        // Bound of a rectangle in a direction (true: direction of the movement) on the movement axis.
        let bound = |b: (i32, i32, i32, i32), forward: bool| {
            let (lo, hi) = if horizontal { (b.0, b.2) } else { (b.1, b.3) };
            if forward == positive { hi } else { lo }
        };
        let other = |b: (i32, i32, i32, i32)| if horizontal { (b.1, b.3) } else { (b.0, b.2) };
        let after = |a: i32, b: i32| if positive { a > b } else { b > a };
        let step = if positive { 1 } else { -1 };
        // Starting band: coordinate on the axis, extent on the other axis.
        let (start, span) = match self.focus {
            Some(f) => {
                let b = bounds(self.rect(&self.widgets[f]));
                (bound(b, true) + step, other(b))
            }
            None => {
                let (w, h) = self.screen;
                let b = (0, 0, w - 1, h - 1);
                (bound(b, false) - step, other(b))
            }
        };
        let mut primary: Option<((i32, i32), usize)> = None;
        let mut fallback: Option<(i64, usize)> = None;
        let centre = (span.0 + span.1) / 2;
        for (i, w) in self.widgets.iter().enumerate() {
            if Some(i) == self.focus || !w.focusable() {
                continue;
            }
            let b = bounds(self.rect(w));
            let (lo, hi) = other(b);
            let entry = bound(b, false);
            if lo.max(span.0) <= hi.min(span.1) {
                let ahead = after(entry, start) || (entry == start && after(bound(b, true), start));
                // Nearest on the axis, then the smallest lower edge of the other axis.
                let key = (if positive { entry } else { -entry }, lo);
                if ahead && primary.is_none_or(|p| key < p.0) {
                    primary = Some((key, i));
                }
            }
            if after(entry, start) {
                let c = (lo + hi) / 2;
                let (da, db) = (i64::from(entry - start), i64::from(c - centre));
                let dist = da * da + db * db;
                if fallback.is_none_or(|v| dist < v.0) {
                    fallback = Some((dist, i));
                }
            }
        }
        if let Some(i) = primary.map(|p| p.1).or_else(|| fallback.map(|v| v.1)) {
            self.set_focus(Some(i), false);
        }
    }

    /// Mouse moved (GUI units).
    pub fn motion(&mut self, m: (f32, f32), out: &mut Vec<Event>) {
        self.keyboard = false;
        if let Some(i) = self.dragging {
            self.slide_to(i, m.0, out);
        }
        if let Some(i) = self.drag_field
            && self.focus == Some(i)
        {
            let (wx, ww) = (self.rect(&self.widgets[i]).0, self.widgets[i].w);
            if let Kind::Field(t) = &mut self.widgets[i].kind {
                let k = t.index_at(m.0.floor() as i32 - wx, inner_width(t, ww));
                t.move_to(k, true);
            }
        }
        if self.bar_drag
            && let Some(l) = &mut self.list
        {
            let max = l.max_scroll();
            if m.1 < l.y as f32 {
                l.set_scroll(0.0);
            } else if m.1 > (l.y + l.h) as f32 {
                l.set_scroll(max);
            } else {
                let ha = l.bar_height();
                let ya = (l.scroll * f64::from(l.h - ha) / max.max(1.0)) as f32 + l.y as f32;
                let dy = m.1 - (ya + ha as f32 / 2.0);
                let k = (max.max(1.0) / f64::from((l.h - ha).max(1))).max(1.0);
                l.set_scroll(l.scroll + f64::from(dy) * k);
            }
        }
    }

    fn slide_to(&mut self, i: usize, mx: f32, out: &mut Vec<Event>) {
        let (x, w) = (f64::from(self.widgets[i].x), f64::from(self.widgets[i].w));
        let v = ((f64::from(mx) - (x + 4.0)) / (w - 8.0)).clamp(0.0, 1.0);
        let id = self.widgets[i].id;
        if let Kind::Slider { value, .. } = &mut self.widgets[i].kind
            && *value != v
        {
            *value = v;
            out.push(Event::Slide(id, v));
        }
    }

    /// Press of a mouse button (0 left); only the left acts.
    pub fn press(&mut self, m: (f32, f32), button: u8, shift: bool, out: &mut Vec<Event>) {
        self.keyboard = false;
        if button != 0 {
            return;
        }
        // Scroll bar of the list.
        if let Some(l) = &self.list
            && l.max_scroll() > 0.0
            && l.contains(m)
            && m.0 >= l.bar_x() as f32
            && m.0 < (l.bar_x() + 6) as f32
        {
            self.bar_drag = true;
            return;
        }
        let Some(i) = self.at(m) else {
            self.set_focus(None, false);
            if let Some(l) = &mut self.list
                && let Some(e) = l.entry_at(m)
            {
                // Selection in a list: silent (the game plays no sound for an entry;
                // a screen that wants one, like the world list, adds it itself).
                l.selected = Some(e);
                l.focused = true;
                let double = self.last_select.is_some_and(|(t, k)| k == e && t.elapsed().as_millis() < 250);
                self.last_select = Some((Instant::now(), e));
                out.push(if double { Event::Activate(e) } else { Event::Select(e) });
            }
            return;
        };
        if !self.widgets[i].active {
            return;
        }
        self.set_focus(Some(i), true);
        let double = self.last_click.is_some_and(|t| t.elapsed().as_millis() < 250);
        self.last_click = Some(Instant::now());
        let (id, wx, ww) = (self.widgets[i].id, self.rect(&self.widgets[i]).0, self.widgets[i].w);
        match &mut self.widgets[i].kind {
            Kind::Button | Kind::Icon(_) | Kind::IconText(..) | Kind::Sprite(_) => {
                self.clicks += 1;
                out.push(Event::Press(id));
            }
            Kind::Cycle => {
                self.clicks += 1;
                out.push(Event::Cycle(id, if shift { -1 } else { 1 }));
            }
            Kind::Checkbox { checked } => {
                *checked = !*checked;
                self.clicks += 1;
                out.push(Event::Toggle(id, *checked));
            }
            Kind::Slider { .. } => {
                self.dragging = Some(i);
                self.slide_to(i, m.0, out);
            }
            Kind::Field(t) => {
                // Character under the mouse (real widths of the displayed glyphs), Shift extends the
                // selection, double click selects the word; then the selection follows the drag.
                t.click(m.0.floor() as i32 - wx, inner_width(t, ww), shift, double);
                self.drag_field = Some(i);
            }
            Kind::Title | Kind::Label { .. } => {}
        }
    }

    /// Release: end of a slider drag (sound on release).
    pub fn release(&mut self, out: &mut Vec<Event>) {
        self.bar_drag = false;
        self.drag_field = None;
        if let Some(i) = self.dragging.take() {
            self.clicks += 1;
            out.push(Event::SlideEnd(self.widgets[i].id));
        }
    }

    /// Wheel (notches, positive = upwards).
    pub fn wheel(&mut self, m: (f32, f32), dy: f64, out: &mut Vec<Event>) {
        if let Some(l) = &mut self.list
            && l.contains(m)
        {
            l.set_scroll(l.scroll - dy * f64::from(l.step));
            return;
        }
        if let Some(i) = self.at(m)
            && self.widgets[i].active
            && self.widgets[i].kind == Kind::Cycle
            && dy != 0.0
        {
            out.push(Event::Cycle(self.widgets[i].id, if dy > 0.0 { -1 } else { 1 }));
        }
    }

    /// Key pressed; returns false if it was not used (Escape, screen shortcuts).
    pub fn key(&mut self, k: KeyCode, mods: ModifiersState, out: &mut Vec<Event>) -> bool {
        self.keyboard = true;
        // Text field with focus: editing first.
        if let Some(i) = self.focus
            && let Kind::Field(t) = &mut self.widgets[i].kind
        {
            let (used, changed) = t.key(k, mods);
            if changed {
                out.push(Event::Edit(self.widgets[i].id));
            }
            if used {
                return true;
            }
        }
        let select = matches!(k, KeyCode::Enter | KeyCode::Space | KeyCode::NumpadEnter);
        match k {
            KeyCode::Tab => self.tab(mods.shift_key()),
            _ if select => {
                if let Some(i) = self.focus {
                    let id = self.widgets[i].id;
                    match &mut self.widgets[i].kind {
                        Kind::Button | Kind::Icon(_) | Kind::IconText(..) | Kind::Sprite(_) => {
                            self.clicks += 1;
                            out.push(Event::Press(id));
                        }
                        Kind::Cycle => {
                            self.clicks += 1;
                            out.push(Event::Cycle(id, if mods.shift_key() { -1 } else { 1 }));
                        }
                        Kind::Checkbox { checked } => {
                            *checked = !*checked;
                            self.clicks += 1;
                            out.push(Event::Toggle(id, *checked));
                        }
                        Kind::Slider { adjusting, .. } => *adjusting = !*adjusting,
                        _ => {}
                    }
                } else if let Some(e) = self.list.as_ref().and_then(|l| l.selected) {
                    out.push(Event::Activate(e));
                } else {
                    return false;
                }
            }
            KeyCode::ArrowLeft | KeyCode::ArrowRight => {
                let dir = if k == KeyCode::ArrowLeft { -1 } else { 1 };
                let meta = self.focus.map(|i| (self.widgets[i].id, self.widgets[i].w));
                if let (Some(i), Some((id, w))) = (self.focus, meta)
                    && let Kind::Slider { value, steps, adjusting: true } = &mut self.widgets[i].kind
                {
                    // Discrete slider: previous / next value, computed by the screen.
                    if *steps > 0 {
                        out.push(Event::Step(id, dir));
                        return true;
                    }
                    let v = (*value + f64::from(dir) / f64::from(w - 8)).clamp(0.0, 1.0);
                    if v != *value {
                        *value = v;
                        out.push(Event::Slide(id, v));
                    }
                } else {
                    self.arrow((dir, 0));
                }
            }
            KeyCode::ArrowUp | KeyCode::ArrowDown => {
                let dir = if k == KeyCode::ArrowUp { -1 } else { 1 };
                if self.focus.is_none()
                    && let Some(l) = &mut self.list
                    && !l.entries.is_empty()
                {
                    let n = l.entries.len();
                    let s = match l.selected {
                        Some(s) => (s as i32 + dir).clamp(0, n as i32 - 1) as usize,
                        None => 0,
                    };
                    l.selected = Some(s);
                    l.focused = true;
                    let e = &l.entries[s];
                    let (top, bottom) = (2 + e.top, 2 + e.top + e.h);
                    l.reveal(top, bottom);
                    out.push(Event::Select(s));
                } else {
                    self.arrow((0, dir));
                }
            }
            _ => return false,
        }
        true
    }

    /// Character typed (text field with focus).
    pub fn typed(&mut self, s: &str, out: &mut Vec<Event>) {
        if let Some(i) = self.focus
            && let Kind::Field(t) = &mut self.widgets[i].kind
            && t.editable
        {
            let before = t.text.len();
            t.insert(s);
            if t.text.len() != before {
                out.push(Event::Edit(self.widgets[i].id));
            }
        }
    }

    /// Drawing: list, components, then tooltip (last layer). `time` in seconds.
    pub fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, mouse: Option<(f32, f32)>, time: f64) {
        self.draw_with(gui, q, mouse, time, &mut |_, _, _| {});
    }

    /// Drawing, with the list entries drawn by `rows` (clipping of the active list).
    pub fn draw_with(
        &mut self,
        gui: &mut Gui,
        q: &wgpu::Queue,
        mouse: Option<(f32, f32)>,
        time: f64,
        rows: &mut dyn FnMut(&mut Gui, &wgpu::Queue, &List),
    ) {
        self.hover = mouse.and_then(|m| self.at(m));
        self.screen = (gui.width as i32, gui.height as i32);
        self.request_cursor(mouse);
        if let Some(l) = &self.list {
            draw_list_back(gui, q, l);
            gui.set_clip(Some([l.x as f32, l.y as f32, (l.x + l.w) as f32, (l.y + l.h) as f32]));
            rows(gui, q, l);
            for i in 0..self.widgets.len() {
                if self.widgets[i].in_list && self.widgets[i].visible {
                    self.draw_widget(gui, q, i, time);
                }
            }
            gui.set_clip(None);
            draw_list_front(gui, l);
        }
        for i in 0..self.widgets.len() {
            if !self.widgets[i].in_list && self.widgets[i].visible {
                self.draw_widget(gui, q, i, time);
            }
        }
        // Tooltip: hover, or focus if the last input is the keyboard.
        let (target, by_mouse) = match (self.hover, self.focus) {
            (Some(h), _) => (Some(h), true),
            (None, Some(f)) if self.keyboard => (Some(f), false),
            _ => (None, false),
        };
        if let Some(i) = target {
            // Tooltip given by the screen only (an icon button without a tooltip has
            // none: the game shows its message only if the screen asks for it).
            let w = &self.widgets[i];
            if !w.shown_tooltip().is_empty() {
                let rect = self.rect(w);
                let m = if by_mouse { mouse } else { None };
                draw_tooltip(gui, q, w.shown_tooltip(), rect, m);
            }
        }
    }

    /// Cursor shape wanted by the hovered component (scroll bar included).
    fn request_cursor(&self, mouse: Option<(f32, f32)>) {
        let Some(m) = mouse else { return };
        if let Some(l) = &self.list
            && l.max_scroll() > 0.0
            && l.contains(m)
            && m.0 >= l.bar_x() as f32
            && m.0 < (l.bar_x() + 6) as f32
        {
            request_cursor(if self.bar_drag { Cursor::ResizeNs } else { Cursor::Hand });
            return;
        }
        if self.bar_drag {
            request_cursor(Cursor::ResizeNs);
            return;
        }
        let Some(i) = self.dragging.or(self.hover) else { return };
        let w = &self.widgets[i];
        let c = match &w.kind {
            Kind::Label { .. } | Kind::Title => return,
            _ if !w.active => Cursor::NotAllowed,
            Kind::Slider { .. } if self.dragging == Some(i) => Cursor::ResizeEw,
            Kind::Field(t) if !t.editable => Cursor::NotAllowed,
            Kind::Field(_) => Cursor::IBeam,
            _ => Cursor::Hand,
        };
        request_cursor(c);
    }

    fn draw_widget(&self, gui: &mut Gui, q: &wgpu::Queue, i: usize, time: f64) {
        let w = &self.widgets[i];
        let (x, y, ww, hh) = self.rect(w);
        let (xf, yf, wf, hf) = (x as f32, y as f32, ww as f32, hh as f32);
        let hovered = self.hover == Some(i);
        let focused = self.focus == Some(i);
        let a = ((w.alpha.clamp(0.0, 1.0) * 255.0) as u32) << 24;
        let tint = a | 0x00ff_ffff;
        let text_color = if w.active { tint } else { a | 0x00a0_a0a0 };
        match &w.kind {
            Kind::Button | Kind::Cycle | Kind::Icon(_) | Kind::IconText(..) => {
                let sprite = if !w.active {
                    crate::gui::Pre::BUTTON_DISABLED
                } else if hovered || focused {
                    crate::gui::Pre::BUTTON_HIGHLIGHTED
                } else {
                    crate::gui::Pre::BUTTON
                };
                gui.sprite_pre(sprite, xf, yf, wf, hf, tint);
                match w.kind {
                    Kind::Icon(icon) => {
                        let (ix, iy) = centered_icon((x, y, ww, hh), (15, 15));
                        gui.sprite(icon, ix as f32, iy as f32, 15.0, 15.0, tint);
                    }
                    Kind::IconText(icon, sw, sh) => {
                        let (mid, zone, (ix, iy)) = icon_text_layout((x, y, ww, hh), (sw, sh));
                        scrolling_text_at(gui, q, &w.msg, mid, zone, text_color, time);
                        gui.sprite(icon, ix as f32, iy as f32, sw as f32, sh as f32, tint);
                    }
                    _ => scrolling_text(gui, q, &w.msg, x + 2, y, x + ww - 2, y + hh, text_color, time),
                }
            }
            Kind::Sprite(name) => {
                let s = if !w.active {
                    format!("{name}_disabled")
                } else if hovered || focused {
                    format!("{name}_highlighted")
                } else {
                    (*name).to_owned()
                };
                gui.sprite(&s, xf, yf, wf, hf, tint);
            }
            Kind::Slider { value, adjusting, .. } => {
                let bg = if w.active && focused && !adjusting { "widget/slider_highlighted" } else { "widget/slider" };
                gui.sprite(bg, xf, yf, wf, 20.0, tint);
                let handle = if w.active && (hovered || *adjusting) {
                    "widget/slider_handle_highlighted"
                } else {
                    "widget/slider_handle"
                };
                let hx = x + (value * f64::from(ww - 8)).floor() as i32;
                gui.sprite(handle, hx as f32, yf, 8.0, hf, tint);
                scrolling_text(gui, q, &w.msg, x + 2, y, x + ww - 2, y + hh, text_color, time);
            }
            Kind::Checkbox { checked } => {
                let s = match (checked, focused) {
                    (true, true) => "widget/checkbox_selected_highlighted",
                    (true, false) => "widget/checkbox_selected",
                    (false, true) => "widget/checkbox_highlighted",
                    (false, false) => "widget/checkbox",
                };
                gui.sprite(s, xf, yf, 17.0, 17.0, tint);
                gui.text(q, &w.msg, xf + 21.0, yf + 4.0, text_color, true, 0);
            }
            Kind::Field(t) => draw_field(gui, q, t, x, y, ww, hh, focused),
            Kind::Title => {
                let msg = underlined_bold(&w.msg);
                let tw = rc_text::width(gui, &msg);
                let cx = x + ww / 2 - tw / 2;
                // Box of `width + 8` × 17 (margin 4), black background and white outline on focus.
                if focused {
                    let bx = cx - 4;
                    gui.fill(bx as f32, yf, (bx + tw + 8) as f32, (y + 17) as f32, 0xffff_ffff);
                    gui.fill((bx + 1) as f32, yf + 1.0, (bx + tw + 7) as f32, (y + 16) as f32, 0xff00_0000);
                }
                gui.text(q, &msg, cx as f32, (y + 4) as f32, tint, true, 0);
            }
            Kind::Label { color, centered } => {
                let tx = if *centered { x + ww / 2 - gui.visual_width(&w.msg) / 2 } else { x };
                gui.text(q, &w.msg, tx as f32, yf, *color, true, 0);
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_field(gui: &mut Gui, q: &wgpu::Queue, t: &TextField, x: i32, y: i32, w: i32, h: i32, focused: bool) {
    let blink = t.focus_at.is_some_and(|f| (f.elapsed().as_millis() / 300) % 2 == 0);
    draw_field_blink(gui, q, t, [x, y, w, h], focused, blink);
}

/// Cumulated widths (unrounded) of the characters of `chars` up to the first one whose rounded-up
/// cumulation exceeds `inner` (included).
fn visible_widths(m: &dyn rc_text::Metrics, chars: &[StyledChar], inner: i32) -> Vec<f32> {
    let mut sums = Vec::with_capacity(chars.len().min(64));
    let mut acc = 0.0f32;
    for c in chars {
        acc += advance(m, c);
        sums.push(acc);
        if acc.ceil() as i32 > inner {
            break;
        }
    }
    sums
}

/// Icon centred in a button: middle of the button minus half the icon (separate
/// integer divisions, like the game).
fn centered_icon((x, y, w, h): (i32, i32, i32, i32), (sw, sh): (i32, i32)) -> (i32, i32) {
    (x + w / 2 - sw / 2, y + h / 2 - sh / 2)
}

/// Text and icon button: centre of the text, text area (bounded before the icon) and position
/// of the icon, at 2 px from the right edge and vertically centred.
fn icon_text_layout((x, y, w, h): (i32, i32, i32, i32), (sw, sh): (i32, i32)) -> (i32, [i32; 4], (i32, i32)) {
    (x + w / 2, [x + 2, y, x + w - sw - 4, y + h], (x + w - sw - 2, y + h / 2 - sh / 2))
}

/// Useful width of the text of a field `w` wide (margin of 4 on each side if it has a frame).
fn inner_width(t: &TextField, w: i32) -> i32 {
    if t.bordered { w - 8 } else { w }
}

/// Base colour of a field's hint, like the game: a hint without any style takes the
/// dark grey (0x555555); a styled hint keeps its colours, the rest follows the text colour.
fn hint_color(hint: &[StyledChar], text_color: u32) -> u32 {
    if hint.iter().all(|c| c.style == rc_text::CharStyle::default()) { 0xff55_5555 } else { text_color }
}

/// Outline of the chosen entry of a list: white if the list has focus, grey otherwise.
pub fn selection_outline(list_focused: bool) -> u32 {
    if list_focused { 0xffff_ffff } else { 0xff80_8080 }
}

/// Input field like the game (cursor, selection, scrolling), with a blink supplied
/// by the caller; `[x, y, w, h]` is the field's frame.
pub(crate) fn draw_field_blink(
    gui: &mut Gui,
    q: &wgpu::Queue,
    t: &TextField,
    [x, y, w, h]: [i32; 4],
    focused: bool,
    blink: bool,
) {
    if t.bordered {
        let s = if focused { "widget/text_field_highlighted" } else { "widget/text_field" };
        gui.sprite(s, x as f32, y as f32, w as f32, h as f32, 0xffff_ffff);
    }
    let (tx, ty) = if t.bordered { (x + 4, y + (h - 8) / 2) } else { (x, y) };
    let inner = if t.bordered { w - 8 } else { w };
    let color = if t.editable { t.text_color } else { t.uneditable_color };
    if t.text.is_empty() && !focused {
        *t.drawn.borrow_mut() = (0, tx - x, Vec::new());
        gui.text(q, &t.hint, tx as f32, ty as f32, hint_color(&t.hint, color), true, 0);
        return;
    }
    // Visible part from the scrolling, brought back to keep the cursor visible.
    let mut all = chars(&t.value());
    if let Some(cs) = &t.colors {
        for (c, &rgb) in all.iter_mut().zip(cs) {
            c.style.color = Some(rgb);
        }
    }
    let width_of = |gui: &Gui, a: usize, b: usize| rc_text::width(gui, &all[a.min(all.len())..b.min(all.len())]);
    let mut offset = t.offset.min(t.cursor);
    while offset < t.cursor && width_of(gui, offset, t.cursor) > inner {
        offset += 1;
    }
    // End of the visible part: cumulated widths from `offset` in the same order as the
    // measurement of a text (width = rounded-up cumulation), a single pass; the next cumulation
    // (first character that overflows) is kept for the click.
    let sums = visible_widths(gui, &all[offset.min(all.len())..], inner);
    let end = offset + sums.iter().take_while(|s| s.ceil() as i32 <= inner).count();
    let prefix = |k: usize| match k.min(all.len()).saturating_sub(offset) {
        0 => 0,
        n => sums.get(n - 1).map_or_else(|| width_of(gui, offset, k), |s| s.ceil() as i32),
    };
    let (shown_w, cursor_w, anchor_w) = (prefix(end), prefix(t.cursor), prefix(t.anchor.clamp(offset, end)));
    let shown = &all[offset..end];
    // Centred: the visible part is centred in the whole width of the frame.
    let tx = if t.centered { x + (w - shown_w) / 2 } else { tx };
    *t.drawn.borrow_mut() = (offset, tx - x, sums);
    let sh = t.shadow;
    gui.text_forward(q, shown, tx as f32, ty as f32, color, sh);
    // Cursor position like the game: one pixel after the text that precedes it (nothing if the
    // visible part is empty), one less in insertion (cursor before the end, or full field).
    let insert = t.cursor < all.len() || crate::book::u16_len(&t.text) >= t.max_len;
    let cx = if end > offset { tx + cursor_w + 1 } else { tx } - insert as i32;
    if focused && t.editable {
        crate::ime::field_cursor(cx, ty, 10);
    }
    if let Some(g) = &t.ghost
        && !insert
    {
        gui.text_string(q, &chars(g), (cx - 1) as f32, ty as f32, 0xff80_8080, sh);
    }
    // Selection: inversion then addition of blue (blue text on white), from the cursor to one
    // pixel before the anchor, bounded by the right edge of the field.
    if t.anchor != t.cursor {
        let hl_x = tx + anchor_w - 1;
        let (x0, x1) = (cx.min(x + w), hl_x.min(x + w));
        gui.text_highlight(x0.min(x1) as f32, (ty - 1) as f32, x0.max(x1) as f32, (ty + 10) as f32, t.invert_highlight);
    }
    if focused && blink {
        if insert {
            gui.fill(cx as f32, (ty - 1) as f32, (cx + 1) as f32, (ty + 10) as f32, color);
        } else {
            gui.text(q, &chars("_"), cx as f32, ty as f32, color, sh, 0);
        }
    }
}

/// Scrolling centred text (§ 1.1) in `[left, right] × [top, bottom]`.
#[allow(clippy::too_many_arguments)]
pub fn scrolling_text(
    gui: &mut Gui,
    q: &wgpu::Queue,
    msg: &[StyledChar],
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    color: u32,
    time: f64,
) {
    scrolling_text_at(gui, q, msg, (left + right) / 2, [left, top, right, bottom], color, time);
}

/// Scrolling text in `[left, top, right, bottom]`, centred on `mid` when it fits (brought back
/// into the area).
pub fn scrolling_text_at(
    gui: &mut Gui,
    q: &wgpu::Queue,
    msg: &[StyledChar],
    mid: i32,
    [left, top, right, bottom]: [i32; 4],
    color: u32,
    time: f64,
) {
    let tw = rc_text::width(gui, msg);
    let ty = (top + bottom - 9) / 2 + 1;
    let zone = right - left;
    if tw <= zone {
        let cx = mid.clamp(left + tw / 2, right - tw / 2);
        gui.text(q, msg, (cx - tw / 2) as f32, ty as f32, color, true, 0);
        return;
    }
    let over = f64::from(tw - zone);
    let period = (over * 0.5).max(3.0);
    let s = (std::f64::consts::FRAC_PI_2 * (std::f64::consts::TAU * time / period).cos()).sin() / 2.0 + 0.5;
    let shift = (s * over).floor() as i32;
    let old = gui.clip();
    let r = [left as f32, top as f32, right as f32, bottom as f32];
    gui.set_clip(Some(match old {
        Some(c) => [c[0].max(r[0]), c[1].max(r[1]), c[2].min(r[2]), c[3].min(r[3])],
        None => r,
    }));
    gui.text(q, msg, (left - shift) as f32, ty as f32, color, true, 0);
    gui.set_clip(old);
}

/// Background and selectable entries of a list (§ 1.9, steps 1–2).
fn draw_list_back(gui: &mut Gui, q: &wgpu::Queue, l: &List) {
    let bg = if l.inworld { "bg/inworld_menu_list_background" } else { "bg/menu_list_background" };
    let (x0, y0, x1, y1) = (l.x as f32, l.y as f32, (l.x + l.w) as f32, (l.y + l.h) as f32);
    if l.background {
        gui.sprite_tiled(bg, x0, y0, x1, y1, 32.0, 32.0, x1, y1 + l.scroll.floor() as f32);
    }
    gui.set_clip(Some([x0, y0, x1, y1]));
    let left = l.row_left();
    for (i, e) in l.entries.iter().enumerate().filter(|_| !l.custom_rows) {
        let top = l.screen_y(2 + e.top);
        if top + e.h < l.y || top > l.y + l.h {
            continue;
        }
        if l.selected == Some(i) {
            let (a, b) = (left + 2, left + l.row_width - 2);
            let (t, bt) = (top - 2, top + e.h - 2);
            gui.fill(a as f32, t as f32, b as f32, bt as f32, selection_outline(l.focused));
            gui.fill((a + 1) as f32, (t + 1) as f32, (b - 1) as f32, (bt - 1) as f32, 0xff00_0000);
        }
        let tw = rc_text::width(gui, &e.label);
        let cx = l.x + l.w / 2 - tw / 2;
        gui.text(q, &e.label, cx as f32, (top + e.h / 2 - 9 / 2 - 1) as f32, 0xffff_ffff, true, 0);
    }
    gui.set_clip(None);
}

/// Separators and scroll bar (§ 1.9, steps 3–4).
fn draw_list_front(gui: &mut Gui, l: &List) {
    let (head, foot) = if l.inworld {
        ("bg/inworld_header_separator", "bg/inworld_footer_separator")
    } else {
        ("bg/header_separator", "bg/footer_separator")
    };
    let (x0, x1) = (l.x as f32, (l.x + l.w) as f32);
    if l.separators {
        gui.sprite_tiled(head, x0, (l.y - 2) as f32, x1, l.y as f32, 32.0, 2.0, 0.0, 0.0);
        gui.sprite_tiled(foot, x0, (l.y + l.h) as f32, x1, (l.y + l.h + 2) as f32, 32.0, 2.0, 0.0, 0.0);
    }
    let max = l.max_scroll();
    if max > 0.0 {
        let bx = l.bar_x() as f32;
        gui.sprite("widget/scroller_background", bx, l.y as f32, 6.0, l.h as f32, 0xffff_ffff);
        let ha = l.bar_height();
        let ya = ((l.scroll.floor() * f64::from(l.h - ha) / max) as i32 + l.y).max(l.y);
        gui.sprite("widget/scroller", bx, ya as f32, 6.0, ha as f32, 0xffff_ffff);
    }
}

/// Component tooltip (§ 1.8): box of `inventories.md` § 4.7, placed near the mouse
/// (or under the component with the keyboard).
pub fn draw_tooltip(
    gui: &mut Gui,
    q: &wgpu::Queue,
    lines: &[Vec<StyledChar>],
    r: (i32, i32, i32, i32),
    mouse: Option<(f32, f32)>,
) {
    let w = lines.iter().map(|l| rc_text::width(gui, l)).max().unwrap_or(0);
    let h = 10 * lines.len() as i32 - 2;
    let (sw, sh) = (gui.width as i32, gui.height as i32);
    let (rx, ry, rw, rh) = r;
    let (x, y) = match mouse {
        Some((mx, my)) => {
            let (sx, sy) = (mx.floor() as i32, my.floor() as i32);
            let mut x = sx + 12;
            if x + w > sw - 5 {
                x = (sx - 12 - w).max(9);
            }
            let mut y = sy + 3;
            let hr = rh.max(1) as f32;
            let ofs = |a: i32, b: i32| {
                let t = ((a - b).abs() as f32).min(hr) / hr;
                ((hr - 3.0) + (5.0 - (hr - 3.0)) * t).round() as i32
            };
            if ry + rh + 3 + ofs(0, 0) + h + 6 <= sh - 5 {
                y += ofs(y, ry);
            } else {
                y -= h + 6 + ofs(y, ry + rh);
            }
            (x, y)
        }
        None => {
            let (mut x, mut y) = (rx + 3, ry + rh + 4);
            if y + h + 3 > sh {
                y = ry - h - 4;
            }
            if x + w > sw {
                x = (rx + rw - w - 3).max(4);
            }
            (x, y)
        }
    };
    gui.tooltip_layer();
    let f = |v: i32| v as f32;
    gui.sprite_pre(crate::gui::Pre::TOOLTIP_BACKGROUND, f(x - 12), f(y - 12), f(w + 24), f(h + 24), 0xffff_ffff);
    gui.sprite_pre(crate::gui::Pre::TOOLTIP_FRAME, f(x - 12), f(y - 12), f(w + 24), f(h + 24), 0xffff_ffff);
    for (k, l) in lines.iter().enumerate() {
        gui.text(q, l, f(x), f(y + 10 * k as i32), 0xffff_ffff, true, 0);
    }
}

/// Background of the "in game" screens (§ 1.12): vertical gradient, no blur.
pub fn ingame_background(gui: &mut Gui) {
    gui.fill_gradient(0.0, 0.0, gui.width, gui.height, 0xC010_1010, 0xD010_1010);
}

/// Background of an "in game" screen (container, panel, book, technical block editor): the veil,
/// then the place of the subtitles, under the panel that follows (`Gui::draw_after_veil`).
pub fn ingame_ui_background(gui: &mut Gui) {
    ingame_background(gui);
    gui.veil_here();
}

/// Background of the menu screens (§ 1.12, steps 2–3): blur of what precedes (`menuBackgroundBlurriness`),
/// then repeated background texture (in a world or out of a world, over the panorama).
pub fn menu_background(gui: &mut Gui, blur: i32, inworld: bool) {
    gui.blur(blur);
    let (w, h) = (gui.width, gui.height);
    let bg = if inworld { "bg/inworld_menu_background" } else { "bg/menu_background" };
    gui.sprite_tiled(bg, 0.0, 0.0, w, h, 32.0, 32.0, 0.0, 0.0);
}

/// Centred text (screen titles): shadowed white.
pub fn centered(gui: &mut Gui, q: &wgpu::Queue, msg: &[StyledChar], cx: i32, y: i32, color: u32) {
    // Width of the drawn line (Arabic forms included), like the game for a component.
    let tw = gui.visual_width(msg);
    gui.text(q, msg, (cx - tw / 2) as f32, y as f32, color, true, 0);
}

/// Mouse cursor shape requested by the interface during a frame (`allowCursorChanges`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum Cursor {
    #[default]
    Arrow = 0,
    Hand = 1,
    IBeam = 2,
    NotAllowed = 3,
    ResizeEw = 4,
    ResizeNs = 5,
    ResizeAll = 6,
}

static CURSOR: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(0);

/// Request of a cursor shape for the current frame (the last request wins).
pub fn request_cursor(c: Cursor) {
    CURSOR.store(c as u8, std::sync::atomic::Ordering::Relaxed);
}

/// Shape requested since the last call (reset to the arrow).
pub fn take_cursor() -> Cursor {
    match CURSOR.swap(0, std::sync::atomic::Ordering::Relaxed) {
        1 => Cursor::Hand,
        2 => Cursor::IBeam,
        3 => Cursor::NotAllowed,
        4 => Cursor::ResizeEw,
        5 => Cursor::ResizeNs,
        6 => Cursor::ResizeAll,
        _ => Cursor::Arrow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ctrl + letter types nothing (winit returns "a" without Ctrl, U+0001 with); AltGr (Ctrl + Alt)
    /// types its character; Ctrl + Alt without a character (layout without AltGr) types nothing.
    #[test]
    fn ctrl_letters_type_nothing_but_altgr_does() {
        let (none, ctrl, alt) = (ModifiersState::empty(), ModifiersState::CONTROL, ModifiersState::ALT);
        assert_eq!(keep_typed(Some("a"), Some("a"), none), Some("a"));
        assert_eq!(keep_typed(Some("a"), Some("\u{1}"), ctrl), None);
        assert_eq!(keep_typed(Some("v"), Some("\u{16}"), ctrl), None);
        assert_eq!(keep_typed(Some("V"), Some("\u{16}"), ctrl | ModifiersState::SHIFT), None);
        assert_eq!(keep_typed(Some("@"), Some("@"), ctrl | alt), Some("@"));
        assert_eq!(keep_typed(Some("a"), None, ctrl | alt), None);
        assert_eq!(keep_typed(Some("\u{7f}"), Some("\u{7f}"), none), None);
        assert_eq!(keep_typed(Some("\u{8}"), Some("\u{8}"), none), None);
    }

    #[test]
    fn text_field_editing() {
        let mut t = TextField::new(Vec::new());
        t.insert("hello world");
        assert_eq!(t.cursor, 11);
        let none = ModifiersState::empty();
        t.key(KeyCode::Backspace, ModifiersState::CONTROL);
        assert_eq!(t.value(), "hello ");
        t.key(KeyCode::Home, none);
        t.key(KeyCode::ArrowRight, ModifiersState::SHIFT);
        t.key(KeyCode::ArrowRight, ModifiersState::SHIFT);
        t.key(KeyCode::KeyX, ModifiersState::CONTROL);
        assert_eq!(t.value(), "llo ");
        t.key(KeyCode::End, none);
        t.key(KeyCode::KeyV, ModifiersState::CONTROL);
        assert_eq!(t.value(), "llo he");
        t.max_len = 7;
        t.insert("xyz");
        assert_eq!(t.value(), "llo hex");
    }

    #[test]
    fn text_field_counts_utf16_and_sets_raw_values() {
        // Input: 4 UTF-16 units = two emojis; C1 characters allowed, `§` and DEL removed.
        let mut t = TextField::new(Vec::new());
        t.max_len = 4;
        t.insert("😀😀😀");
        assert_eq!(t.value(), "😀😀");
        let mut t = TextField::new(Vec::new());
        t.insert("a\u{85}§\u{7f}\u{1}b");
        assert_eq!(t.value(), "a\u{85}b");
        // Imposed value: nothing filtered, cut in UTF-16, cursor at the end.
        t.max_len = 9;
        t.set_value("§aHello!!\u{1}xyz");
        assert_eq!((t.value().as_str(), t.cursor, t.anchor), ("§aHello!!", 9, 9));
    }

    #[test]
    fn tab_cycles_and_skips_inactive() {
        let mut p = Panel::new(vec![
            Widget::button(1, 0, 0, 100, "a"),
            Widget::button(2, 0, 24, 100, "b").inactive(),
            Widget::button(3, 0, 48, 100, "c"),
        ]);
        let mut ev = Vec::new();
        let none = ModifiersState::empty();
        p.key(KeyCode::Tab, none, &mut ev);
        assert_eq!(p.focus, Some(0));
        p.key(KeyCode::Tab, none, &mut ev);
        assert_eq!(p.focus, Some(2));
        // At the end: back to the first in the same keystroke (and to the last with Shift).
        p.key(KeyCode::Tab, none, &mut ev);
        assert_eq!(p.focus, Some(0));
        p.key(KeyCode::Tab, ModifiersState::SHIFT, &mut ev);
        assert_eq!(p.focus, Some(2));
        p.key(KeyCode::Tab, none, &mut ev);
        p.key(KeyCode::ArrowDown, none, &mut ev);
        p.key(KeyCode::ArrowDown, none, &mut ev);
        assert_eq!(p.focus, Some(2));
        p.key(KeyCode::Enter, none, &mut ev);
        assert_eq!(ev, vec![Event::Press(3)]);
        assert_eq!(p.clicks, 1);
    }

    /// Arrows without focus: start from the opposite edge of the screen (Up takes the lowest);
    /// "loose" fallback measured between midpoints of the facing edges, not between centres.
    #[test]
    fn arrows_start_from_the_opposite_edge_and_measure_side_midpoints() {
        let none = ModifiersState::empty();
        let mut ev = Vec::new();
        let mut p = Panel::new(vec![Widget::button(1, 0, 0, 100, "top"), Widget::button(2, 0, 100, 100, "bottom")]);
        p.screen = (200, 200);
        p.key(KeyCode::ArrowUp, none, &mut ev);
        assert_eq!(p.focus, Some(1));
        let mut p = Panel::new(vec![
            Widget::new(1, Kind::Button, 0, 0, 20, 20, Vec::new()),
            Widget::new(2, Kind::Button, 100, 30, 10, 10, Vec::new()),
            Widget::new(3, Kind::Button, 25, 30, 400, 10, Vec::new()),
        ]);
        p.screen = (500, 300);
        p.focus_widget(1);
        p.key(KeyCode::ArrowRight, none, &mut ev);
        assert_eq!(p.focus, Some(2));
    }

    /// Click in a field: character from the real widths displayed, Shift extends, double
    /// click takes the word, drag extends the selection.
    #[test]
    fn field_click_uses_glyph_widths_shift_double_click_and_drag() {
        let mut t = TextField::new(Vec::new());
        t.set_value("ii ww ab");
        // Frame at x = 10 (text at x = 14): i = 2, space = 4, w = 6, a/b = 6.
        *t.drawn.borrow_mut() = (0, 4, vec![2.0, 4.0, 8.0, 14.0, 20.0, 24.0, 30.0, 36.0]);
        let mut p = Panel::new(vec![Widget::new(1, Kind::Field(Box::new(t)), 10, 10, 100, 20, Vec::new())]);
        let field = |p: &Panel| match &p.widgets[0].kind {
            Kind::Field(t) => (t.cursor, t.anchor),
            _ => unreachable!(),
        };
        let mut ev = Vec::new();
        p.press((19.5, 15.0), 0, false, &mut ev);
        assert_eq!(field(&p), (2, 2));
        p.last_click = None;
        p.press((14.0 + 20.0, 15.0), 0, true, &mut ev);
        assert_eq!(field(&p), (5, 2));
        p.release(&mut ev);
        p.last_click = None;
        p.press((14.0 + 17.0, 15.0), 0, false, &mut ev);
        p.press((14.0 + 17.0, 15.0), 0, false, &mut ev);
        assert_eq!(field(&p), (6, 3));
        p.release(&mut ev);
        p.last_click = None;
        p.press((14.0, 15.0), 0, false, &mut ev);
        p.motion((14.0 + 31.0, 15.0), &mut ev);
        assert_eq!(field(&p), (7, 0));
        p.release(&mut ev);
        p.motion((14.0, 15.0), &mut ev);
        assert_eq!(field(&p), (7, 0));
    }

    /// Hint: dark grey without style, its own colours if it has any; selection outline
    /// white with focus; cumulated widths up to the first character that overflows.
    #[test]
    fn hint_colour_outline_and_visible_widths() {
        assert_eq!(hint_color(&chars("Seed"), 0xffe0_e0e0), 0xff55_5555);
        assert_eq!(hint_color(&chars("§7§oSearch"), 0xffe0_e0e0), 0xffe0_e0e0);
        assert_eq!((selection_outline(true), selection_outline(false)), (0xffff_ffff, 0xff80_8080));
        struct Half;
        impl rc_text::Metrics for Half {
            fn advance(&self, _font: &str, _c: char) -> (f32, f32) {
                (2.5, 1.0)
            }
        }
        // 2.5; 5; 7.5 (rounded 8 > 7: last kept); the useful width is 7.
        assert_eq!(visible_widths(&Half, &chars("abcdef"), 7), vec![2.5, 5.0, 7.5]);
    }

    /// Bench (`--ignored`): end of the visible part of a 256-character field, old
    /// search (a full measurement per character added) against a single cumulation; identical
    /// result verified.
    #[test]
    #[ignore = "bench"]
    fn bench_visible_widths() {
        struct M;
        impl rc_text::Metrics for M {
            fn advance(&self, _font: &str, c: char) -> (f32, f32) {
                (if c == 'i' { 2.0 } else { 6.0 }, 1.0)
            }
        }
        let all = chars(&"ai".repeat(128));
        let inner = 1000;
        let n = 2000;
        let t = Instant::now();
        let mut old_end = 0;
        for _ in 0..n {
            let mut end = 0;
            while end < all.len() && rc_text::width(&M, &all[..end + 1]) <= inner {
                end += 1;
            }
            old_end = std::hint::black_box(end);
        }
        let old = t.elapsed();
        let t = Instant::now();
        let mut new_end = 0;
        for _ in 0..n {
            let s = visible_widths(&M, &all, inner);
            new_end = std::hint::black_box(s.iter().take_while(|w| w.ceil() as i32 <= inner).count());
        }
        let new = t.elapsed();
        assert_eq!(old_end, new_end);
        eprintln!("visible end ({old_end} chars): old {:?} / call, cumulative {:?} / call", old / n, new / n);
    }

    /// List: focus taken by a click on an entry or the arrows, handed to a component.
    #[test]
    fn list_takes_focus_from_entry_clicks_and_arrows() {
        let mut p = Panel::new(vec![Widget::button(1, 0, 180, 100, "ok")]);
        let mut l = List::new(0, 0, 200, 100, 100, 20);
        l.entries = vec![Entry { top: 0, h: 20, label: chars("a") }, Entry { top: 20, h: 20, label: chars("b") }];
        p.list = Some(l);
        let mut ev = Vec::new();
        p.press((100.0, 25.0), 0, false, &mut ev);
        assert!(p.list.as_ref().unwrap().focused);
        p.key(KeyCode::Tab, ModifiersState::empty(), &mut ev);
        assert!(!p.list.as_ref().unwrap().focused);
        p.focus = None;
        p.key(KeyCode::ArrowDown, ModifiersState::empty(), &mut ev);
        assert!(p.list.as_ref().unwrap().focused);
    }

    /// Icon buttons: icon centred (20 × 20: 3 px from the edge, not 2); text and icon button
    /// (accessibility welcome): icon at 2 px from the right edge, text bounded before it; no
    /// tooltip without the one given by the screen.
    #[test]
    fn icon_buttons_layout_and_tooltip() {
        assert_eq!(centered_icon((10, 10, 20, 20), (15, 15)), (13, 13));
        assert_eq!(icon_text_layout((0, 50, 150, 20), (16, 16)), (75, [2, 50, 130, 70], (132, 52)));
        assert_eq!(icon_text_layout((0, 50, 150, 20), (15, 15)).2, (133, 53));
        let w = Widget::new(1, Kind::Icon("icon/language"), 0, 0, 20, 20, chars("Language"));
        assert!(w.shown_tooltip().is_empty());
    }

    #[test]
    fn slider_follows_mouse_and_sounds_on_release() {
        let mut p = Panel::new(vec![Widget::new(
            7,
            Kind::Slider { value: 0.0, steps: 0, adjusting: false },
            10,
            10,
            108,
            20,
            Vec::new(),
        )]);
        let mut ev = Vec::new();
        p.press((64.0, 15.0), 0, false, &mut ev);
        assert_eq!(ev, vec![Event::Slide(7, 0.5)]);
        assert_eq!(p.clicks, 0);
        p.motion((500.0, 15.0), &mut ev);
        p.release(&mut ev);
        assert_eq!(ev[1..], [Event::Slide(7, 1.0), Event::SlideEnd(7)]);
        assert_eq!(p.clicks, 1);
    }
}
