//! Books (`docs/specs/special-screens.md` § 4): reading (`open_book`, links), book and quill
//! (editing, signing, `edit_book`) and lectern (menu buttons, forced page).

use rc_proto::Reader;
use rc_proto::varint::read_varint;
use rc_proto_v777::item::ItemStack;
use rc_text::{Component, StyledChar};
use winit::keyboard::{KeyCode, ModifiersState};

use crate::gui::Gui;
use crate::special::inside;
use crate::widgets::TextField;

/// What the screen asks of the application.
#[derive(Debug, Clone, PartialEq)]
pub enum BookAction {
    Close,
    /// Page-turn sound (reading, editing).
    PageTurn,
    /// Lectern button click (`container_button_click`).
    Lectern(i32),
    /// End of editing: pages, title (signing).
    Save {
        pages: Vec<String>,
        title: Option<String>,
    },
    /// Command of a `run_command` link.
    Command(String),
    /// Other clicked-text action (link, copy, dialog…), handled by `links.rs`.
    Click(rc_text::ClickEvent),
}

enum Mode {
    /// Reading: pages as components; lectern if `lectern`.
    Read { pages: Vec<Component>, lectern: bool, can_take: bool },
    /// Book and quill: pages as text, send box, page area, signing screen
    /// (title field) and title kept when coming back from signing with "Cancel".
    Edit {
        pages: Vec<String>,
        slot: i32,
        area: Box<Area>,
        signing: Option<Box<TextField>>,
        kept_title: String,
        author: String,
    },
}

pub struct Book {
    mode: Mode,
    page: usize,
    /// Split lines of the page read (page, lines).
    cache: Option<(usize, Vec<Vec<StyledChar>>)>,
    focus_at: std::time::Instant,
    /// Last click handled (double-click: under 250 ms, same button).
    last_click: Option<(std::time::Instant, u8)>,
}

const PAGE_W: f32 = 114.0;
const MAX_LINES: usize = 14;

/// Page input area: 122 × 134 at (w/2 − 57 − 8, 28), inner margin 4.
const AREA_PAD: i32 = 4;
const AREA_W: i32 = 122;
const AREA_H: i32 = 134;
const AREA_Y: i32 = 28;
/// Page limits: 1024 UTF-16 units, 126 / 9 = 14 lines.
const CHAR_LIMIT: usize = 1024;

fn area_x(screen_w: i32) -> i32 {
    (screen_w - 114) / 2 - 8
}

/// Length in UTF-16 units (the game's limits count this way).
pub(crate) fn u16_len(t: &[char]) -> usize {
    t.iter().map(|c| c.len_utf16()).sum()
}

/// Cuts to at most `max` UTF-16 units (at a character boundary).
pub(crate) fn truncate_u16(t: &mut Vec<char>, max: usize) {
    let mut n = 0;
    let keep = t
        .iter()
        .take_while(|c| {
            n += c.len_utf16();
            n <= max
        })
        .count();
    t.truncate(keep);
}

/// Character allowed in an input (no `§`, no control, no DEL; `\n` in multiline).
fn allowed(c: char, multiline: bool) -> bool {
    (c != '§' && c >= ' ' && c != '\u{7f}') || (multiline && c == '\n')
}

fn advance(m: &dyn rc_text::Metrics, c: char) -> f32 {
    rc_text::Metrics::advance(m, "", c).0
}

/// Width of a plain text (rounded-up sum).
pub(crate) fn plain_width(m: &dyn rc_text::Metrics, t: &[char]) -> i32 {
    t.iter().map(|&c| advance(m, c)).sum::<f32>().ceil() as i32
}

/// Longest prefix whose cumulative width stays ≤ `w`: number of characters.
fn prefix_by_width(m: &dyn rc_text::Metrics, t: &[char], w: f32) -> usize {
    let mut left = w;
    let mut n = 0;
    for &c in t {
        left -= advance(m, c);
        if left < 0.0 {
            break;
        }
        n += 1;
    }
    n
}

/// Splitting of a plain text into lines (start, end) without the consumed breaks: break
/// at the last space (or at the character that overflows), `\n` and break space skipped; an
/// empty text has no line.
fn split_ranges(m: &dyn rc_text::Metrics, t: &[char], max: f32) -> Vec<(usize, usize)> {
    let max = max.max(1.0);
    let mut out = Vec::new();
    let mut start = 0;
    while start < t.len() {
        let (mut width, mut had, mut last_space, mut brk) = (0.0f32, false, None, None);
        for (i, &c) in t.iter().enumerate().skip(start) {
            if c == '\n' {
                brk = Some(i);
                break;
            }
            if c == ' ' {
                last_space = Some(i);
            }
            let a = advance(m, c);
            width += a;
            if had && width > max {
                brk = Some(last_space.unwrap_or(i));
                break;
            }
            had |= a != 0.0;
        }
        let Some(b) = brk else {
            out.push((start, t.len()));
            break;
        };
        let next = if matches!(t[b], '\n' | ' ') { b + 1 } else { b };
        out.push((start, b));
        start = next;
    }
    out
}

/// Multiline text area (book-and-quill page, multiline dialog input): cursor,
/// selection anchor, displayed lines recomputed on every value change. Wrap width,
/// character limit (UTF-16 units) and line limit specific to each use.
pub(crate) struct Area {
    pub(crate) text: Vec<char>,
    pub(crate) cursor: usize,
    anchor: usize,
    pub(crate) selecting: bool,
    pub(crate) lines: Vec<(usize, usize)>,
    dirty: bool,
    wrap: f32,
    char_limit: usize,
    line_limit: Option<usize>,
}

impl Default for Area {
    /// Book page: 114 wide, 1024 units, 14 lines.
    fn default() -> Self {
        Area::new(PAGE_W, CHAR_LIMIT, Some(MAX_LINES))
    }
}

impl Area {
    pub(crate) fn new(wrap: f32, char_limit: usize, line_limit: Option<usize>) -> Area {
        Area {
            text: Vec::new(),
            cursor: 0,
            anchor: 0,
            selecting: false,
            lines: Vec::new(),
            dirty: false,
            wrap,
            char_limit,
            line_limit,
        }
    }

    pub(crate) fn value(&self) -> String {
        self.text.iter().collect()
    }

    /// Forced value (page change): truncated to the limit, cursor at the end; line
    /// overflow is allowed.
    pub(crate) fn set_value(&mut self, s: &str) {
        let mut t: Vec<char> = s.chars().collect();
        truncate_u16(&mut t, self.char_limit);
        self.text = t;
        self.cursor = self.text.len();
        self.anchor = self.cursor;
        self.dirty = true;
    }

    /// Forced value without line overflow (dialog input): if it overflows, it
    /// is refused entirely and the old value stays, like the game.
    pub(crate) fn set_value_within(&mut self, m: &dyn rc_text::Metrics, s: &str) {
        let mut t: Vec<char> = s.chars().collect();
        truncate_u16(&mut t, self.char_limit);
        if !self.overflows(m, &t) {
            self.set_value(s);
        }
    }

    pub(crate) fn reflow(&mut self, m: &dyn rc_text::Metrics) {
        if !self.dirty && !self.lines.is_empty() {
            return;
        }
        self.dirty = false;
        self.lines = if self.text.is_empty() { vec![(0, 0)] } else { split_ranges(m, &self.text, self.wrap) };
        if self.text.last() == Some(&'\n') {
            self.lines.push((self.text.len(), self.text.len()));
        }
    }

    fn overflows(&self, m: &dyn rc_text::Metrics, t: &[char]) -> bool {
        self.line_limit.is_some_and(|max| {
            split_ranges(m, t, self.wrap).len() + usize::from(t.last() == Some(&'\n')) > max
        })
    }

    pub(crate) fn selection(&self) -> (usize, usize) {
        (self.cursor.min(self.anchor), self.cursor.max(self.anchor))
    }

    /// Replaces the selection with `s` (filtered, cut to the remaining room); refused entirely if
    /// the page would overflow 14 lines. True if the value changed.
    pub(crate) fn insert(&mut self, m: &dyn rc_text::Metrics, s: &str) -> bool {
        let (a, b) = self.selection();
        if s.is_empty() && a == b {
            return false;
        }
        let mut add: Vec<char> = s.chars().filter(|&c| allowed(c, true)).collect();
        truncate_u16(&mut add, self.char_limit.saturating_sub(u16_len(&self.text)));
        let mut t = self.text[..a].to_vec();
        t.extend_from_slice(&add);
        t.extend_from_slice(&self.text[b..]);
        if self.overflows(m, &t) {
            return false;
        }
        self.text = t;
        self.cursor = a + add.len();
        self.anchor = self.cursor;
        self.dirty = true;
        true
    }

    fn delete(&mut self, m: &dyn rc_text::Metrics, dir: i64) -> bool {
        if self.cursor == self.anchor {
            self.anchor = (self.cursor as i64 + dir).clamp(0, self.text.len() as i64) as usize;
        }
        self.insert(m, "")
    }

    pub(crate) fn seek(&mut self, to: usize) {
        self.cursor = to.min(self.text.len());
        if !self.selecting {
            self.anchor = self.cursor;
        }
    }

    fn line_at_cursor(&self) -> Option<usize> {
        self.lines.iter().position(|&(a, b)| self.cursor >= a && self.cursor <= b)
    }

    fn cursor_line(&self, offset: i64) -> (usize, usize) {
        match self.line_at_cursor() {
            Some(i) => {
                let j = (i as i64 + offset).clamp(0, self.lines.len() as i64 - 1) as usize;
                self.lines[j]
            }
            None => self.lines.last().copied().unwrap_or((0, 0)),
        }
    }

    /// Neighboring line: same abscissa (+ 2 px) in the targeted line.
    fn seek_line(&mut self, m: &dyn rc_text::Metrics, offset: i64) {
        let (a, _) = self.cursor_line(0);
        let left = plain_width(m, &self.text[a..self.cursor]) + 2;
        let (a, b) = self.cursor_line(offset);
        let n = prefix_by_width(m, &self.text[a..b], left as f32);
        self.seek(a + n);
    }

    /// Click in the area (coordinates relative to the inner corner).
    pub(crate) fn seek_point(&mut self, m: &dyn rc_text::Metrics, x: f32, y: f32) {
        let row = ((y / 9.0).floor() as i64).clamp(0, self.lines.len() as i64 - 1) as usize;
        let (a, b) = self.lines[row];
        let n = prefix_by_width(m, &self.text[a..b], x.floor());
        self.seek(a + n);
    }

    fn is_space(&self, i: usize) -> bool {
        self.text[i].is_whitespace()
    }

    fn word_end(&self, from: usize) -> usize {
        let mut e = from;
        while e < self.text.len() && !self.is_space(e) {
            e += 1;
        }
        e
    }

    /// Previous word (start, end); the start is clamped to the last character.
    fn previous_word(&self) -> (usize, usize) {
        if self.text.is_empty() {
            return (0, 0);
        }
        let mut p = self.cursor.min(self.text.len() - 1);
        while p > 0 && self.is_space(p - 1) {
            p -= 1;
        }
        while p > 0 && !self.is_space(p - 1) {
            p -= 1;
        }
        (p, self.word_end(p))
    }

    fn next_word(&self) -> (usize, usize) {
        if self.text.is_empty() {
            return (0, 0);
        }
        let mut p = self.cursor.min(self.text.len() - 1);
        while p < self.text.len() && !self.is_space(p) {
            p += 1;
        }
        while p < self.text.len() && self.is_space(p) {
            p += 1;
        }
        (p, self.word_end(p))
    }

    pub(crate) fn select_word(&mut self) {
        let (a, b) = self.previous_word();
        self.seek(a);
        self.selecting = true;
        self.seek(b);
    }

    /// Key; true if the value changed.
    pub(crate) fn key(&mut self, m: &dyn rc_text::Metrics, k: KeyCode, mods: ModifiersState) -> bool {
        self.reflow(m);
        let (ctrl, shift, alt) = (mods.control_key(), mods.shift_key(), mods.alt_key());
        self.selecting = shift;
        let shortcut = ctrl && !shift && !alt;
        let n = self.text.len();
        let changed = match k {
            KeyCode::KeyA if shortcut => {
                self.cursor = n;
                self.anchor = 0;
                false
            }
            KeyCode::KeyC if shortcut => {
                let (a, b) = self.selection();
                crate::widgets::copy_to_clipboard(self.text[a..b].iter().collect());
                false
            }
            KeyCode::KeyV if shortcut => self.insert(m, &crate::links::get_clipboard()),
            KeyCode::KeyX if shortcut => {
                let (a, b) = self.selection();
                crate::widgets::copy_to_clipboard(self.text[a..b].iter().collect());
                self.insert(m, "")
            }
            KeyCode::Backspace => {
                let d = if ctrl { self.previous_word().0 as i64 - self.cursor as i64 } else { -1 };
                self.delete(m, d)
            }
            KeyCode::Enter | KeyCode::NumpadEnter => self.insert(m, "\n"),
            KeyCode::Delete => {
                let d = if ctrl { self.next_word().0 as i64 - self.cursor as i64 } else { 1 };
                self.delete(m, d)
            }
            KeyCode::Home => {
                let to = if ctrl { 0 } else { self.cursor_line(0).0 };
                self.seek(to);
                false
            }
            KeyCode::End => {
                let to = if ctrl { n } else { self.cursor_line(0).1 };
                self.seek(to);
                false
            }
            KeyCode::ArrowRight => {
                let to = if ctrl { self.next_word().0 } else { self.cursor + 1 };
                self.seek(to);
                false
            }
            KeyCode::ArrowLeft => {
                let to = if ctrl { self.previous_word().0 } else { self.cursor.saturating_sub(1) };
                self.seek(to);
                false
            }
            KeyCode::ArrowDown if !ctrl => {
                self.seek_line(m, 1);
                false
            }
            KeyCode::ArrowUp if !ctrl => {
                self.seek_line(m, -1);
                false
            }
            _ => false,
        };
        self.reflow(m);
        changed
    }
}

/// Pages of a written book (`written_book_content`, raw version) or of a book and quill
/// (`writable_book_content`); `None` if the stack is neither.
pub fn pages_of(st: &ItemStack, ids: &crate::stack::ComponentIds) -> Option<Vec<Component>> {
    if let Some(b) = ids.of("minecraft:written_book_content").and_then(|i| st.component(i)) {
        let mut r = Reader::new(b);
        let skip_opt_str = |r: &mut Reader<'_>| -> Option<()> {
            rc_proto::read_str(r, 32767).ok()?;
            if r.u8().ok()? == 1 {
                rc_proto::read_str(r, 32767).ok()?;
            }
            Some(())
        };
        skip_opt_str(&mut r)?;
        rc_proto::read_str(&mut r, 32767).ok()?;
        read_varint(&mut r).ok()?;
        let n = read_varint(&mut r).ok()?.clamp(0, 100);
        let mut pages = Vec::new();
        for _ in 0..n {
            let raw = rc_proto::nbt::read(&mut r).ok()?;
            if r.u8().ok()? == 1 {
                rc_proto::nbt::read(&mut r).ok()?;
            }
            pages.push(raw.as_ref().and_then(rc_text::from_nbt).unwrap_or_else(|| Component::text("")));
        }
        return Some(pages);
    }
    let text = writable_pages(st, ids)?;
    Some(text.into_iter().map(Component::text).collect())
}

/// Text pages of a book and quill.
pub fn writable_pages(st: &ItemStack, ids: &crate::stack::ComponentIds) -> Option<Vec<String>> {
    let b = ids.of("minecraft:writable_book_content").and_then(|i| st.component(i))?;
    let mut r = Reader::new(b);
    let n = read_varint(&mut r).ok()?.clamp(0, 100);
    let mut pages = Vec::new();
    for _ in 0..n {
        let raw = rc_proto::read_str(&mut r, 32767).ok()?.to_owned();
        if r.u8().ok()? == 1 {
            rc_proto::read_str(&mut r, 32767).ok()?;
        }
        pages.push(raw);
    }
    Some(pages)
}

/// Bytes of the `writable_book_content` component for these pages (no filtered version).
pub fn writable_bytes(pages: &[String]) -> Vec<u8> {
    let mut w = Vec::new();
    rc_proto::varint::write_varint(&mut w, pages.len() as i32);
    for p in pages {
        rc_proto::write_str(&mut w, p);
        w.push(0);
    }
    w
}

/// Edited page: lines in black with no shadow, insertion cursor (10 px bar) or end cursor
/// ("_"), highlighted selection (up to 118 px from the inner edge when it continues on the
/// next line, like the game). Only lines entirely inside the area are drawn.
fn draw_area(gui: &mut Gui, q: &wgpu::Queue, a: &Area, ax: i32, blink: bool) {
    let black = 0xff00_0000;
    let (left, top0) = (ax + AREA_PAD, AREA_Y + AREA_PAD);
    let visible = |y: i32| y >= AREA_Y && y + 9 <= AREA_Y + AREA_H;
    let s = |r: &[char]| crate::gui::plain(&r.iter().collect::<String>());
    let insert = a.cursor < a.text.len();
    let (mut cx, mut cy, mut drawn) = (0, 0, false);
    let mut top = top0;
    for &(b, e) in &a.lines {
        if !drawn && insert && a.cursor >= b && a.cursor <= e {
            cx = left + plain_width(gui, &a.text[b..a.cursor]);
            cy = top;
            if visible(top) {
                gui.text_string(q, &s(&a.text[b..a.cursor]), left as f32, top as f32, black, false);
                gui.text_string(q, &s(&a.text[a.cursor..e]), cx as f32, top as f32, black, false);
                if blink {
                    gui.fill(cx as f32, (top - 1) as f32, (cx + 1) as f32, (top + 10) as f32, black);
                }
            }
            drawn = true;
        } else {
            if visible(top) {
                gui.text_string(q, &s(&a.text[b..e]), left as f32, top as f32, black, false);
            }
            if !insert {
                cx = left + plain_width(gui, &a.text[b..e]);
                cy = top;
            }
        }
        top += 9;
    }
    if blink && !insert && visible(cy) {
        gui.text(q, &crate::gui::plain("_"), cx as f32, cy as f32, black, false, 0);
    }
    // IME: the book area always has focus during editing.
    crate::ime::field_cursor(cx, cy, 10);
    let (sb, se) = a.selection();
    if sb != se {
        let mut top = top0;
        for &(b, e) in &a.lines {
            if sb > e {
                top += 9;
                continue;
            }
            if b > se {
                break;
            }
            if visible(top) {
                let x0 = plain_width(gui, &a.text[b..sb.max(b)]);
                let x1 = if se > e { AREA_W - AREA_PAD } else { plain_width(gui, &a.text[b..se]) };
                gui.text_highlight((left + x0) as f32, top as f32, (left + x1) as f32, (top + 9) as f32, true);
            }
            top += 9;
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn button(gui: &mut Gui, q: &wgpu::Queue, x: i32, y: i32, w: i32, label: &[StyledChar], hover: bool, active: bool) {
    let s = if !active {
        "widget/button_disabled"
    } else if hover {
        "widget/button_highlighted"
    } else {
        "widget/button"
    };
    gui.sprite(s, x as f32, y as f32, w as f32, 20.0, 0xffff_ffff);
    let color = if active { 0xffff_ffff } else { 0xffa0_a0a0 };
    crate::widgets::scrolling_text(gui, q, label, x + 2, y, x + w - 2, y + 20, color, 0.0);
}

fn tr(gui: &Gui, key: &str, args: &[String]) -> Vec<StyledChar> {
    crate::widgets::chars(&crate::widgets::tr(&gui.lang, key, args))
}

impl Book {
    pub fn read(pages: Vec<Component>) -> Book {
        Book::new(Mode::Read { pages, lectern: false, can_take: false })
    }

    pub fn lectern(pages: Vec<Component>, page: usize, can_take: bool) -> Book {
        let mut b = Book::new(Mode::Read { pages, lectern: true, can_take });
        b.page = page;
        b
    }

    pub fn edit(mut pages: Vec<String>, slot: i32, author: String) -> Book {
        if pages.is_empty() {
            pages.push(String::new());
        }
        let mut area = Box::<Area>::default();
        area.set_value(&pages[0]);
        Book::new(Mode::Edit { pages, slot, area, signing: None, kept_title: String::new(), author })
    }

    fn new(mode: Mode) -> Book {
        Book { mode, page: 0, cache: None, focus_at: std::time::Instant::now(), last_click: None }
    }

    /// Signing title field: 15 characters, no frame, centered, black with no shadow.
    fn title_field(value: &str) -> TextField {
        let mut t = TextField::new(Vec::new());
        t.max_len = 15;
        t.bordered = false;
        t.centered = true;
        t.shadow = false;
        t.text_color = 0xff00_0000;
        t.set_value(value);
        t
    }

    pub fn is_lectern(&self) -> bool {
        matches!(self.mode, Mode::Read { lectern: true, .. })
    }

    /// Lectern: new pages (book from slot 0) and forced page.
    pub fn set_lectern(&mut self, pages: Vec<Component>, page: usize) {
        if let Mode::Read { pages: p, .. } = &mut self.mode
            && *p != pages
        {
            *p = pages;
            self.cache = None;
        }
        if self.page != page {
            self.page = page;
            self.cache = None;
        }
    }

    pub fn slot(&self) -> Option<i32> {
        match &self.mode {
            Mode::Edit { slot, .. } => Some(*slot),
            Mode::Read { .. } => None,
        }
    }

    fn count(&self) -> usize {
        match &self.mode {
            Mode::Read { pages, .. } => pages.len(),
            Mode::Edit { pages, .. } => pages.len(),
        }
    }

    /// Previous / next page (except lectern: menu button). The page sound comes only
    /// from the click on the arrow (`press`), never from the Page Up/Down keys.
    fn turn(&mut self, dir: i32, out: &mut Vec<BookAction>) {
        let n = self.count();
        if self.is_lectern() {
            out.push(BookAction::Lectern(if dir < 0 { 1 } else { 2 }));
            return;
        }
        // Going back is impossible on page 1: nothing moves (the cursor stays where it is).
        if dir < 0 && self.page == 0 {
            return;
        }
        if dir < 0 {
            if self.page > 0 {
                self.page -= 1;
            }
        } else if self.page + 1 < n {
            self.page += 1;
        } else if let Mode::Edit { pages, .. } = &mut self.mode {
            if pages.len() < 100 {
                pages.push(String::new());
            }
            if self.page + 1 < pages.len() {
                self.page += 1;
            }
        }
        self.cache = None;
        let page = self.page;
        if let Mode::Edit { pages, area, .. } = &mut self.mode {
            area.set_value(&pages[page]);
        }
    }

    fn has_next(&self) -> bool {
        match &self.mode {
            Mode::Edit { .. } => true,
            Mode::Read { .. } => self.page + 1 < self.count(),
        }
    }

    /// Lines of the page read (cut at 114, 14 at most).
    fn read_lines(&mut self, gui: &Gui) -> &[Vec<StyledChar>] {
        let fresh = self.cache.as_ref().is_some_and(|c| c.0 == self.page);
        if !fresh {
            let lines = match &self.mode {
                Mode::Read { pages, .. } => pages.get(self.page).map_or_else(Vec::new, |p| {
                    let chars = rc_text::styled_chars(&rc_text::flatten(p, &gui.lang));
                    let mut l = rc_text::wrap(gui, &chars, PAGE_W);
                    l.truncate(MAX_LINES);
                    l
                }),
                Mode::Edit { .. } => Vec::new(),
            };
            self.cache = Some((self.page, lines));
        }
        self.cache.as_ref().map_or(&[], |c| &c.1)
    }

    /// Style under the mouse (reading): link, tooltip (`links.rs`).
    pub fn style_at(&mut self, gui: &Gui, m: (f32, f32)) -> Option<rc_text::CharStyle> {
        if !matches!(self.mode, Mode::Read { .. }) {
            return None;
        }
        let l = (gui.width as i32 - 192) / 2;
        let row = ((m.1 - 32.0) / 9.0).floor();
        if row < 0.0 {
            return None;
        }
        let line = self.read_lines(gui).get(row as usize)?;
        crate::links::style_at(gui, line, m.0 - (l + 36) as f32).cloned()
    }

    /// Link under the mouse (reading).
    fn link_at(&mut self, gui: &Gui, m: (f32, f32)) -> Option<rc_text::ClickEvent> {
        self.style_at(gui, m)?.click.as_deref().cloned()
    }

    pub fn press(&mut self, gui: &Gui, m: (f32, f32), b: u8, shift: bool) -> Vec<BookAction> {
        let mut out = Vec::new();
        if b != 0 {
            return out;
        }
        let now = std::time::Instant::now();
        let double = self.last_click.is_some_and(|(t, lb)| lb == b && now.duration_since(t).as_millis() < 250);
        if self.press_at(gui, m, shift, double, &mut out) {
            self.last_click = Some((now, b));
        }
        out
    }

    /// Left click; true if an element took it (basis of the double-click).
    fn press_at(&mut self, gui: &Gui, m: (f32, f32), shift: bool, double: bool, out: &mut Vec<BookAction>) -> bool {
        let (w, l) = (gui.width as i32, (gui.width as i32 - 192) / 2);
        if let Mode::Edit { signing: Some(t), kept_title, .. } = &mut self.mode {
            let blank = t.value().trim().is_empty();
            if inside(Some(m), w / 2 - 100, 196, 98, 20) {
                if !blank {
                    self.finish(true, out);
                }
                return !blank;
            }
            if inside(Some(m), w / 2 + 2, 196, 98, 20) {
                // "Cancel": back to the editor (rebuilt: cursor at the end of the page,
                // blink resumed), the typed title is kept for later.
                *kept_title = t.value();
                let page = self.page;
                if let Mode::Edit { signing, area, pages, .. } = &mut self.mode {
                    *signing = None;
                    area.set_value(&pages[page]);
                }
                self.focus_at = std::time::Instant::now();
                return true;
            }
            return false;
        }
        // The page area comes before the arrows (it covers them by 3 px at the top).
        let ax = area_x(w);
        if let Mode::Edit { area, .. } = &mut self.mode
            && inside(Some(m), ax, AREA_Y, AREA_W, AREA_H)
        {
            area.reflow(gui);
            if double {
                area.select_word();
            } else {
                area.selecting = shift;
                area.seek_point(gui, m.0 - (ax + AREA_PAD) as f32, m.1 - (AREA_Y + AREA_PAD) as f32);
            }
            return true;
        }
        let sound = !self.is_lectern();
        if self.page > 0 && inside(Some(m), l + 43, 159, 23, 13) {
            self.turn(-1, out);
            if sound {
                out.push(BookAction::PageTurn);
            }
            return true;
        }
        if self.has_next() && inside(Some(m), l + 116, 159, 23, 13) {
            self.turn(1, out);
            if sound {
                out.push(BookAction::PageTurn);
            }
            return true;
        }
        match &self.mode {
            Mode::Edit { .. } => {
                if inside(Some(m), w / 2 - 100, 196, 98, 20) {
                    if let Mode::Edit { signing, kept_title, .. } = &mut self.mode {
                        *signing = Some(Box::new(Book::title_field(kept_title)));
                    }
                    self.focus_at = std::time::Instant::now();
                    return true;
                } else if inside(Some(m), w / 2 + 2, 196, 98, 20) {
                    self.finish(false, out);
                    return true;
                }
                false
            }
            Mode::Read { lectern, can_take, .. } => {
                let (lectern, can_take) = (*lectern, *can_take);
                let done = if lectern && can_take {
                    inside(Some(m), w / 2 - 100, 196, 98, 20)
                } else {
                    inside(Some(m), (w - 200) / 2, 196, 200, 20)
                };
                if done {
                    out.push(BookAction::Close);
                    true
                } else if lectern && can_take && inside(Some(m), w / 2 + 2, 196, 98, 20) {
                    out.push(BookAction::Lectern(3));
                    true
                } else if let Some(c) = self.link_at(gui, m) {
                    match c.action.as_str() {
                        "change_page" => {
                            let n = c.value.parse::<usize>().unwrap_or(1).max(1) - 1;
                            if lectern {
                                // Lectern: requested page sent as is (the server clamps).
                                if n != self.page {
                                    out.push(BookAction::Lectern(100 + n as i32));
                                }
                            } else {
                                self.page = n.min(self.count().saturating_sub(1));
                                self.cache = None;
                            }
                        }
                        "run_command" => out.push(BookAction::Command(c.value.trim_start_matches('/').to_owned())),
                        _ => out.push(BookAction::Click(c)),
                    }
                    true
                } else {
                    false
                }
            }
        }
    }

    /// End of editing: pages (trailing empty ones removed except when signing) and title.
    fn finish(&mut self, sign: bool, out: &mut Vec<BookAction>) {
        let Mode::Edit { pages, signing, .. } = &self.mode else { return };
        let mut pages = pages.clone();
        let title = if sign { signing.as_ref().map(|t| t.value().trim().to_owned()) } else { None };
        if !sign {
            while pages.last().is_some_and(String::is_empty) {
                pages.pop();
            }
        }
        out.push(BookAction::Save { pages, title });
        out.push(BookAction::Close);
    }

    /// Key (and typed text). Esc closes without saving, from the editor as from the
    /// signing screen; Page Up/Down turn the page silently.
    pub fn key(&mut self, gui: Option<&Gui>, k: KeyCode, mods: ModifiersState, text: Option<&str>) -> Vec<BookAction> {
        let mut out = Vec::new();
        if k == KeyCode::Escape {
            out.push(BookAction::Close);
            return out;
        }
        // Signing: title field; Enter signs as soon as the field is not empty (even
        // if it only has spaces: only the button requires a non-blank title).
        if let Mode::Edit { signing: Some(t), .. } = &mut self.mode {
            if matches!(k, KeyCode::Enter | KeyCode::NumpadEnter) {
                if !t.text.is_empty() {
                    self.finish(true, &mut out);
                }
                return out;
            }
            let (used, _) = t.key(k, mods);
            if !used && let Some(s) = text {
                t.insert(s);
            }
            return out;
        }
        match k {
            KeyCode::PageUp => {
                self.turn(-1, &mut out);
                return out;
            }
            KeyCode::PageDown => {
                self.turn(1, &mut out);
                return out;
            }
            _ => {}
        }
        let page = self.page;
        let (Mode::Edit { pages, area, .. }, Some(gui)) = (&mut self.mode, gui) else { return out };
        let mut changed = area.key(gui, k, mods);
        if let Some(s) = text {
            changed |= area.insert(gui, s);
        }
        if changed {
            pages[page] = area.value();
        }
        area.reflow(gui);
        out
    }

    pub fn draw(
        &mut self,
        gui: &mut Gui,
        q: &wgpu::Queue,
        _it: &crate::hud::HotbarItems<'_>,
        mouse: Option<(f32, f32)>,
        _time: f64,
    ) {
        gui.next_layer();
        crate::widgets::ingame_ui_background(gui);
        let w = gui.width as i32;
        let l = (w - 192) / 2;
        let f = |v: i32| v as f32;
        gui.sprite_region("bg/book", 256.0, 256.0, 0.0, 0.0, f(l), 2.0, 192.0, 192.0);
        let black = 0xff00_0000;
        if let Mode::Edit { signing: Some(field), author, .. } = &self.mode {
            let author = author.clone();
            let centered = |gui: &mut Gui, s: &[StyledChar], y: i32, color: u32| {
                let tw = rc_text::width(gui, s);
                gui.text(q, s, f(l + 36 + (114 - tw) / 2), f(y), color, false, 0);
            };
            // Game order: buttons and field (widgets) first, screen texts on top.
            let blink = (self.focus_at.elapsed().as_millis() / 300).is_multiple_of(2);
            crate::widgets::draw_field_blink(gui, q, field, [(w - 114) / 2 - 3, 50, 114, 20], true, blink);
            let blank = field.value().trim().is_empty();
            let t = tr(gui, "book.editTitle", &[]);
            centered(gui, &t, 34, black);
            // Author name in dark gray (component style, takes priority over black).
            let a = tr(gui, "book.byAuthor", &[author]);
            centered(gui, &a, 60, 0xff55_5555);
            let warn = tr(gui, "book.finalizeWarning", &[]);
            for (k, line) in rc_text::wrap(gui, &warn, PAGE_W).iter().enumerate() {
                gui.text(q, line, f(l + 36), f(82 + 9 * k as i32), black, false, 0);
            }
            let sign = tr(gui, "book.finalizeButton", &[]);
            button(gui, q, w / 2 - 100, 196, 98, &sign, inside(mouse, w / 2 - 100, 196, 98, 20), !blank);
            let cancel = tr(gui, "gui.cancel", &[]);
            button(gui, q, w / 2 + 2, 196, 98, &cancel, inside(mouse, w / 2 + 2, 196, 98, 20), true);
            return;
        }
        // Page indicator.
        let n = self.count().max(1);
        let ind = tr(gui, "book.pageIndicator", &[(self.page + 1).to_string(), n.to_string()]);
        let iw = rc_text::width(gui, &ind);
        gui.text(q, &ind, f(l + 148 - iw), 18.0, black, false, 0);
        let blink = (self.focus_at.elapsed().as_millis() / 300).is_multiple_of(2);
        if let Mode::Edit { area, .. } = &mut self.mode {
            area.reflow(gui);
            draw_area(gui, q, area, area_x(w), blink);
        } else {
            let lines = self.read_lines(gui).to_vec();
            for (k, line) in lines.iter().enumerate() {
                gui.text(q, line, f(l + 36), f(32 + 9 * k as i32), black, false, 0);
            }
        }
        // Page buttons.
        if self.page > 0 {
            let h = inside(mouse, l + 43, 159, 23, 13);
            let s = if h { "widget/page_backward_highlighted" } else { "widget/page_backward" };
            gui.sprite(s, f(l + 43), 159.0, 23.0, 13.0, 0xffff_ffff);
        }
        if self.has_next() {
            let h = inside(mouse, l + 116, 159, 23, 13);
            let s = if h { "widget/page_forward_highlighted" } else { "widget/page_forward" };
            gui.sprite(s, f(l + 116), 159.0, 23.0, 13.0, 0xffff_ffff);
        }
        let done = tr(gui, "gui.done", &[]);
        match &self.mode {
            Mode::Edit { .. } => {
                let sign = tr(gui, "book.signButton", &[]);
                button(gui, q, w / 2 - 100, 196, 98, &sign, inside(mouse, w / 2 - 100, 196, 98, 20), true);
                button(gui, q, w / 2 + 2, 196, 98, &done, inside(mouse, w / 2 + 2, 196, 98, 20), true);
            }
            Mode::Read { lectern: true, can_take: true, .. } => {
                button(gui, q, w / 2 - 100, 196, 98, &done, inside(mouse, w / 2 - 100, 196, 98, 20), true);
                let take = tr(gui, "lectern.take_book", &[]);
                button(gui, q, w / 2 + 2, 196, 98, &take, inside(mouse, w / 2 + 2, 196, 98, 20), true);
            }
            Mode::Read { .. } => {
                button(gui, q, (w - 200) / 2, 196, 200, &done, inside(mouse, (w - 200) / 2, 196, 200, 20), true);
            }
        }
    }
}

impl crate::App {
    /// Stack held in hand (0 main, 1 secondary) and send slot (bar or 40).
    fn hand_book(&self, hand: i32) -> Option<(ItemStack, i32)> {
        if hand == 1 {
            return self.inventory.offhand().cloned().map(|s| (s, 40));
        }
        let i = self.hud.selected;
        self.inventory.hotbar(i).cloned().map(|s| (s, i as i32))
    }

    /// `open_book` (§ 4.1).
    pub(crate) fn open_book_read(&mut self, hand: i32) {
        let Some((st, _)) = self.hand_book(hand) else { return };
        let Some(pages) = pages_of(&st, &self.component_ids) else { return };
        self.open_overlay(crate::special::Overlay::Book(Box::new(Book::read(pages))));
    }

    /// Book and quill used (§ 4.2): local opening of the editor.
    pub(crate) fn open_book_edit(&mut self, hand: i32) {
        let Some((st, slot)) = self.hand_book(hand) else { return };
        let pages = writable_pages(&st, &self.component_ids).unwrap_or_default();
        let author = self.local_name.clone();
        self.open_overlay(crate::special::Overlay::Book(Box::new(Book::edit(pages, slot, author))));
    }

    pub(crate) fn book_action(&mut self, actions: Vec<BookAction>) {
        for a in actions {
            match a {
                BookAction::Close => self.close_overlay(),
                BookAction::PageTurn => {
                    if let Some(au) = self.audio.get() {
                        au.play("minecraft:item.book.page_turn", rc_audio::Category::Ui, None, 1.0, 1.0, None);
                    }
                }
                BookAction::Lectern(b) => {
                    if let Some(c) = &self.inventory.container {
                        let w = c.window;
                        let link = &self.session.link;
                        let _ = link.send_raw(link.ids.container_button_click, |o| {
                            rc_proto_v777::special::write_button_click(o, w, b)
                        });
                    }
                }
                BookAction::Save { pages, title } => self.book_save(&pages, title.as_deref()),
                BookAction::Command(c) => {
                    let lectern =
                        matches!(&self.special.overlay, Some(crate::special::Overlay::Book(b)) if b.is_lectern());
                    if lectern {
                        self.close_overlay();
                    }
                    // Checked (confirmation if needed); sent: the book closes.
                    self.run_unattended(&c, crate::links::Origin::Book);
                }
                BookAction::Click(ev) => self.click_event(&ev, crate::links::Origin::Book),
            }
        }
    }

    /// Saving: local stack updated, `edit_book` sent.
    fn book_save(&mut self, pages: &[String], title: Option<&str>) {
        let Some(slot) = (match &self.special.overlay {
            Some(crate::special::Overlay::Book(b)) => b.slot(),
            _ => None,
        }) else {
            return;
        };
        if title.is_none()
            && let Some(id) = self.component_ids.of("minecraft:writable_book_content")
        {
            let menu = if slot == 40 { 45 } else { 36 + slot as usize };
            if let Some(Some(st)) = self.inventory.menu.get_mut(menu) {
                st.added.retain(|c| c.0 != id);
                st.added.push((id, writable_bytes(pages).into_boxed_slice()));
            }
        }
        let link = &self.session.link;
        let _ = link.send_raw(link.ids.edit_book, |w| rc_proto_v777::special::write_edit_book(w, slot, pages, title));
    }

    /// Lectern (§ 4.3): reading screen taken from slot 0, page = property 0.
    pub(crate) fn lectern_tick(&mut self) {
        let Some(c) = &self.inventory.container else { return };
        let pages = c.slots.first().and_then(Option::as_ref).and_then(|s| pages_of(s, &self.component_ids));
        let page = c.props[0].max(0) as usize;
        let can_take = matches!(self.env.game_mode, 0 | 1);
        match &mut self.special.overlay {
            Some(crate::special::Overlay::Book(b)) if b.is_lectern() => b.set_lectern(pages.unwrap_or_default(), page),
            None if self.screens.open => {
                let b = Book::lectern(pages.unwrap_or_default(), page, can_take);
                self.special.overlay = Some(crate::special::Overlay::Book(Box::new(b)));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Dummy metrics: 6 px per character, 4 px for the space.
    struct Fixed;
    impl rc_text::Metrics for Fixed {
        fn advance(&self, _font: &str, c: char) -> (f32, f32) {
            (if c == ' ' { 4.0 } else { 6.0 }, 1.0)
        }
    }

    fn area(s: &str) -> Area {
        let mut a = Area::default();
        a.set_value(s);
        a.reflow(&Fixed);
        a
    }

    #[test]
    fn split_skips_the_breaking_space_and_newline() {
        // 19 letters = 114 px fit; the 20th overflows.
        let t: Vec<char> = "aaaaaaaaaa bbbbbbbbbb\ncc".chars().collect();
        assert_eq!(split_ranges(&Fixed, &t, 114.0), vec![(0, 10), (11, 21), (22, 24)]);
        let t: Vec<char> = "a".repeat(20).chars().collect();
        assert_eq!(split_ranges(&Fixed, &t, 114.0), vec![(0, 19), (19, 20)]);
        // Final newline: extra empty line only in the displayed lines.
        let t: Vec<char> = "ab\n".chars().collect();
        assert_eq!(split_ranges(&Fixed, &t, 114.0), vec![(0, 2)]);
        assert_eq!(area("ab\n").lines, vec![(0, 2), (3, 3)]);
        assert_eq!(area("").lines, vec![(0, 0)]);
    }

    #[test]
    fn insertion_that_overflows_fourteen_lines_is_refused_whole() {
        let full = "x\n".repeat(13) + "x";
        let mut a = area(&full);
        a.cursor = 0;
        a.anchor = 0;
        // A 15th line: refused, the text and the end of the page stay intact.
        assert!(!a.insert(&Fixed, "\n"));
        assert_eq!(a.value(), full);
        assert!(a.insert(&Fixed, "y"));
        assert_eq!(a.cursor, 1);
    }

    #[test]
    fn imposed_value_over_the_line_limit_is_refused() {
        // Single-line dialog input: "l1\nl2" refused, the field stays empty.
        let mut a = Area::new(PAGE_W, 100, Some(1));
        a.set_value_within(&Fixed, "l1\nl2");
        assert_eq!(a.value(), "");
        a.set_value_within(&Fixed, "l1");
        assert_eq!(a.value(), "l1");
    }

    #[test]
    fn words_and_lines_move_like_the_game() {
        let mut a = area("me this  three");
        a.cursor = 9;
        a.anchor = 9;
        assert_eq!(a.previous_word(), (3, 7));
        // Next word from the start of a word: that word is skipped (end of text here).
        assert_eq!(a.next_word(), (14, 14));
        // Ctrl+Backspace: to the start of the previous word.
        a.key(&Fixed, KeyCode::Backspace, ModifiersState::CONTROL);
        assert_eq!(a.value(), "me three");
        // Down arrow: same abscissa + 2 px in the next line.
        let mut a = area("abcdef\nabcdef");
        a.cursor = 2;
        a.anchor = 2;
        a.key(&Fixed, KeyCode::ArrowDown, ModifiersState::empty());
        assert_eq!(a.cursor, 9);
        // Shift + arrow: the selection extends; Ctrl+Shift+A does not select everything.
        a.key(&Fixed, KeyCode::ArrowLeft, ModifiersState::SHIFT);
        assert_eq!(a.selection(), (8, 9));
        a.key(&Fixed, KeyCode::KeyA, ModifiersState::CONTROL | ModifiersState::SHIFT);
        assert_eq!(a.selection(), (8, 9));
    }

    #[test]
    fn page_keys_turn_silently_and_escape_never_saves() {
        let mut b = Book::edit(vec!["a".into(), "b".into()], 0, "Me".into());
        let out = b.key(None, KeyCode::PageDown, ModifiersState::empty(), None);
        assert_eq!((b.page, out.len()), (1, 0));
        let out = b.key(None, KeyCode::Escape, ModifiersState::empty(), None);
        assert_eq!(out, vec![BookAction::Close]);
    }

    #[test]
    fn writable_round_trip() {
        let pages = vec!["Hello".to_owned(), String::new()];
        let b = writable_bytes(&pages);
        let st = ItemStack { item: 1, count: 1, added: vec![(0, b.into_boxed_slice())], removed: vec![] };
        let mut r = Reader::new(st.component(0).unwrap());
        assert_eq!(read_varint(&mut r).unwrap(), 2);
        assert_eq!(rc_proto::read_str(&mut r, 1024).unwrap(), "Hello");
    }
}
