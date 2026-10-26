//! Sign editor (`docs/specs/special-screens.md` § 5): `open_sign_editor`, input of the
//! 4 lines bounded by the font width, live update of the world's sign, sending
//! `sign_update` (lines then face) on close.

use winit::keyboard::{KeyCode, ModifiersState};

use rc_text::StyledChar;

use crate::gui::Gui;
use crate::special::inside;

pub struct SignEdit {
    pub pos: [i32; 3],
    pub front: bool,
    lines: [Vec<char>; 4],
    line: usize,
    cursor: usize,
    /// Other end of the selection in the current line (= cursor if none).
    anchor: usize,
    /// Hanging sign (background, scale, max width 60).
    hanging: bool,
    /// Wall sign (background cut to 12 high).
    wall: bool,
    /// Wood (`signs/<wood>`, `hanging_signs/<wood>`).
    wood: String,
    color: u32,
    edited: bool,
    focus_at: std::time::Instant,
}

/// Wood of a sign from the block name (`oak_wall_hanging_sign` → `oak`).
fn wood_of(name: &str) -> String {
    let n = name.strip_prefix("minecraft:").unwrap_or(name);
    ["_wall_hanging_sign", "_hanging_sign", "_wall_sign", "_sign"]
        .iter()
        .find_map(|s| n.strip_suffix(s))
        .unwrap_or("oak")
        .to_owned()
}

impl SignEdit {
    fn max_width(&self) -> i32 {
        if self.hanging { 60 } else { 90 }
    }

    pub fn lines(&self) -> [String; 4] {
        std::array::from_fn(|i| self.lines[i].iter().collect())
    }

    /// True once if the text changed since the last call.
    pub fn take_edit(&mut self) -> bool {
        std::mem::take(&mut self.edited)
    }

    /// Click: true if "Done" was pressed.
    pub fn press(&mut self, gui: &Gui, m: (f32, f32), b: u8) -> bool {
        let (w, h) = (gui.width as i32, gui.height as i32);
        b == 0 && inside(Some(m), w / 2 - 100, h / 4 + 144, 200, 20)
    }

    /// Current line replaced (text of the world's sign updated at the next call).
    fn set_line(&mut self, l: Vec<char>) {
        if self.lines[self.line] != l {
            self.lines[self.line] = l;
            self.edited = true;
        }
    }

    /// Selection removed from `l` (cursor and anchor at the start); `l` unchanged without a selection.
    fn delete_selection(&mut self, l: &mut Vec<char>) {
        let (a, b) = (self.cursor.min(self.anchor).min(l.len()), self.cursor.max(self.anchor).min(l.len()));
        if a != b {
            l.drain(a..b);
        }
        self.cursor = a;
        self.anchor = a;
    }

    fn selected(&self) -> String {
        let l = &self.lines[self.line];
        let (a, b) = (self.cursor.min(self.anchor).min(l.len()), self.cursor.max(self.anchor).min(l.len()));
        l[a..b].iter().collect()
    }

    /// Insertion like the game: the selection removed first, then the whole refused if the line
    /// exceeds the sign's width; the line then stays intact but the selection is
    /// abandoned, cursor and anchor at the start of the old selection.
    fn insert(&mut self, gui: Option<&Gui>, s: &str) {
        let mut l = self.lines[self.line].clone();
        if self.cursor != self.anchor {
            self.delete_selection(&mut l);
        }
        self.cursor = self.cursor.min(l.len());
        let add: Vec<char> = s.chars().collect();
        l.splice(self.cursor..self.cursor, add.iter().copied());
        let width = match gui {
            Some(g) => rc_text::width(g, &crate::gui::plain(&l.iter().collect::<String>())),
            None => approx_width(&l),
        };
        // 384 characters: limit of the `sign_update` packet (never reached under the width).
        if l.len() <= 384 && width <= self.max_width() {
            self.cursor = (self.cursor + add.len()).min(l.len());
            self.anchor = self.cursor;
            self.set_line(l);
        }
    }

    /// Deletion of `count` characters (or up to the word) from the cursor, or of the selection.
    fn remove(&mut self, count: i32, word: bool) {
        let mut l = self.lines[self.line].clone();
        if l.is_empty() {
            return;
        }
        if self.cursor != self.anchor {
            self.delete_selection(&mut l);
        } else {
            let other = if word {
                crate::widgets::word_pos(&l, self.cursor, count)
            } else {
                (self.cursor as i64 + i64::from(count)).clamp(0, l.len() as i64) as usize
            };
            let (a, b) = (other.min(self.cursor), other.max(self.cursor).min(l.len()));
            l.drain(a..b);
            if count < 0 {
                self.cursor = a;
                self.anchor = a;
            }
        }
        self.set_line(l);
    }

    fn move_to(&mut self, to: usize, selecting: bool) {
        self.cursor = to.min(self.lines[self.line].len());
        if !selecting {
            self.anchor = self.cursor;
        }
    }

    /// Key: true if the screen must close (Esc). Up / Down / Enter change line;
    /// the rest follows the game's input helper: select all, copy, paste, cut
    /// (Ctrl alone), deletion and movement by character or by word (Ctrl), Shift selects.
    pub fn key(&mut self, gui: Option<&Gui>, k: KeyCode, mods: ModifiersState, text: Option<&str>) -> bool {
        let (ctrl, shift) = (mods.control_key(), mods.shift_key());
        let shortcut = ctrl && !shift && !mods.alt_key();
        let n = self.lines[self.line].len();
        match k {
            KeyCode::Escape => return true,
            KeyCode::ArrowUp => {
                self.line = (self.line + 3) % 4;
                self.move_to(usize::MAX, false);
            }
            KeyCode::ArrowDown | KeyCode::Enter | KeyCode::NumpadEnter => {
                self.line = (self.line + 1) % 4;
                self.move_to(usize::MAX, false);
            }
            KeyCode::KeyA if shortcut => {
                self.anchor = 0;
                self.cursor = n;
            }
            KeyCode::KeyC if shortcut => crate::widgets::set_clipboard(self.selected()),
            KeyCode::KeyV if shortcut => {
                let clip = rc_text::strip_codes(&crate::widgets::clipboard()).replace('\r', "");
                self.insert(gui, &clip);
                self.anchor = self.cursor;
            }
            KeyCode::KeyX if shortcut => {
                crate::widgets::set_clipboard(self.selected());
                let mut l = self.lines[self.line].clone();
                self.delete_selection(&mut l);
                self.set_line(l);
            }
            KeyCode::Backspace => self.remove(-1, ctrl),
            KeyCode::Delete => self.remove(1, ctrl),
            KeyCode::ArrowLeft | KeyCode::ArrowRight => {
                let dir = if k == KeyCode::ArrowLeft { -1 } else { 1 };
                let to = if ctrl {
                    crate::widgets::word_pos(&self.lines[self.line], self.cursor, dir)
                } else {
                    (self.cursor as i64 + i64::from(dir)).clamp(0, n as i64) as usize
                };
                self.move_to(to, shift);
            }
            KeyCode::Home => self.move_to(0, shift),
            KeyCode::End => self.move_to(n, shift),
            // Ctrl alone types nothing (winit nonetheless returns the letter without Ctrl: Ctrl+B gives "b").
            _ if ctrl && !mods.alt_key() => {}
            _ => {
                // Character by character, like the game's keystrokes (`§`, controls and DEL
                // refused); AltGr (Ctrl + Alt) gives real characters, allowed.
                for c in text.unwrap_or("").chars().filter(|&c| c != '§' && c >= ' ' && c != '\u{7f}') {
                    self.insert(gui, &c.to_string());
                }
            }
        }
        false
    }

    pub fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, _time: f64) {
        gui.next_layer();
        crate::widgets::ingame_ui_background(gui);
        let (w, h) = (gui.width as i32, gui.height as i32);
        let f = |v: f32| v.round();
        let key = if self.hanging { "hanging_sign.edit" } else { "sign.edit" };
        let title = crate::widgets::chars(&crate::widgets::tr(&gui.lang, key, &[]));
        crate::widgets::centered(gui, q, &title, w / 2, 40, 0xffff_ffff);
        let cx = w as f32 / 2.0;
        let (y0, line_h, top) = if self.hanging {
            let s = 4.5;
            let name = format!("bg/hanging_signs/{}", self.wood);
            gui.sprite_stretch(
                &name,
                (16.0, 16.0),
                [0.0, 0.0, 16.0, 16.0],
                [cx - 8.0 * s, 125.0 - 13.0 - 8.0 * s, 16.0 * s, 16.0 * s],
                0xffff_ffff,
            );
            (125.0, 9.0, -18.0)
        } else {
            let s = 3.9;
            let hh = if self.wall { 12.0 } else { 26.0 };
            let name = format!("bg/signs/{}", self.wood);
            gui.sprite_stretch(
                &name,
                (24.0, 26.0),
                [0.0, 0.0, 24.0, hh],
                [cx - 12.0 * s, 90.0 + 27.0 - 13.0 * s, 24.0 * s, hh * s],
                0xffff_ffff,
            );
            (90.0, 10.0, -20.0)
        };
        let blink = (self.focus_at.elapsed().as_millis() / 300).is_multiple_of(2);
        for i in 0..4 {
            let s: String = self.lines[i].iter().collect();
            let chars = shown_line(crate::gui::plain(&s), gui.lang.bidirectional);
            let lw = rc_text::width(gui, &chars);
            let x = cx - lw as f32 / 2.0;
            let y = y0 + top + line_h * i as f32;
            gui.text_string(q, &chars, f(x), f(y), self.color, false);
            if i != self.line {
                continue;
            }
            // Cursor and selection: width of the first `k` characters of the shown line.
            let width_to = |gui: &Gui, k: usize| rc_text::width(gui, &chars[..k.min(chars.len())]) as f32;
            let bx = x + width_to(gui, self.cursor);
            if blink {
                if self.cursor >= chars.len() {
                    gui.text(q, &crate::gui::plain("_"), f(bx), f(y), self.color, false, 0);
                } else {
                    gui.fill(f(bx), f(y - 1.0), f(bx) + 1.0, f(y + line_h), self.color);
                }
            }
            // Selection highlighted over the whole height of the line, like the game.
            if self.anchor != self.cursor {
                let (a, b) = (x + width_to(gui, self.cursor.min(self.anchor)), x + width_to(gui, self.cursor.max(self.anchor)));
                gui.text_highlight(f(a), f(y), f(b), f(y + line_h), true);
            }
            crate::ime::field_cursor(f(bx) as i32, f(y) as i32, line_h as i32);
        }
        // "Done" button.
        let done = crate::widgets::chars(&crate::widgets::tr(&gui.lang, "gui.done", &[]));
        let (bx, by) = (w / 2 - 100, h / 4 + 144);
        gui.sprite("widget/button", bx as f32, by as f32, 200.0, 20.0, 0xffff_ffff);
        crate::widgets::scrolling_text(gui, q, &done, bx + 2, by, bx + 198, by + 20, 0xffff_ffff, 0.0);
    }
}

/// Sign line shown by the editor: in a bidirectional language, the game replaces it
/// by its formatted and reordered form (a string), which it centers and measures as is;
/// drawing a string then reorders it once more (`text_string`), like the game.
fn shown_line(chars: Vec<StyledChar>, bidirectional: bool) -> Vec<StyledChar> {
    if !bidirectional {
        return chars;
    }
    match rc_text::bidi::visual(&chars, true) {
        Some(order) => order.iter().map(|&(c, _)| StyledChar { c, style: Default::default() }).collect(),
        None => chars,
    }
}

/// Approximate width of a line (no font loaded).
fn approx_width(l: &[char]) -> i32 {
    l.iter()
        .map(|c| match c {
            'i' | '!' | ',' | '.' | ':' | ';' | '|' | '\'' => 2,
            'l' | '`' => 3,
            't' | 'I' | '[' | ']' | ' ' => 4,
            'f' | 'k' | '<' | '>' | '(' | ')' | '{' | '}' | '"' | '*' => 5,
            '@' | '~' => 7,
            _ => 6,
        })
        .sum()
}

impl crate::App {
    /// `open_sign_editor` (§ 5): editor if there is a sign at this position.
    pub(crate) fn open_sign_editor(&mut self, pos: [i32; 3], front: bool) {
        let Some(e) = self.block_entities.get(pos) else {
            eprintln!("sign editor: no sign at {pos:?}");
            return;
        };
        let crate::block_entities::Data::Sign(faces) = &e.data else { return };
        let hanging = e.kind == crate::block_entities::Kind::HangingSign;
        let face = &faces[usize::from(!front)];
        let lang = self.renderer.as_ref().map(|r| r.gui.lang.clone()).unwrap_or_default();
        let lines: [Vec<char>; 4] = std::array::from_fn(|i| {
            face.messages[i]
                .as_ref()
                .and_then(rc_text::from_nbt)
                .map(|c| rc_text::plain_string(&c, &lang))
                .unwrap_or_default()
                .chars()
                .collect()
        });
        let color = 0xff00_0000
            | if face.glowing {
                crate::block_text::TEXT_COLORS[usize::from(face.color & 15)]
            } else {
                crate::block_text::dark_color(face.color, false)
            };
        let name = self.data.block_of(e.state).map(|b| b.name.clone()).unwrap_or_default();
        let s = SignEdit {
            pos,
            front,
            cursor: lines[0].len(),
            anchor: lines[0].len(),
            lines,
            line: 0,
            hanging,
            wall: e.wall,
            wood: wood_of(&name),
            color,
            edited: false,
            focus_at: std::time::Instant::now(),
        };
        self.open_overlay(crate::special::Overlay::Sign(Box::new(s)));
    }

    /// Closing (Esc, Done, forced): `sign_update`.
    pub(crate) fn sign_close(&mut self, s: SignEdit) {
        let lines = s.lines();
        let pos = rc_proto::BlockPos::new(s.pos[0], s.pos[1], s.pos[2]);
        let link = &self.session.link;
        let _ =
            link.send_raw(link.ids.sign_update, |w| rc_proto_v777::special::write_sign_update(w, pos, &lines, s.front));
    }

    /// Input: text of the world's sign updated right away.
    pub(crate) fn sign_live(&mut self, pos: [i32; 3], front: bool, lines: &[String; 4]) {
        self.block_entities.set_sign_text(pos, front, lines);
    }

    /// Closing if the sign disappeared or if the player is too far: like the game, distance
    /// from the eye to the block's cube, not less than the block interaction range + 4.
    pub(crate) fn sign_tick(&mut self) {
        let Some(crate::special::Overlay::Sign(s)) = &self.special.overlay else { return };
        let gone =
            !self.block_entities.get(s.pos).is_some_and(|e| matches!(e.data, crate::block_entities::Data::Sign(_)));
        let mut eye = self.player.pos;
        eye[1] += self.player.eye_height();
        let range = self.player.attribute("minecraft:block_interaction_range").unwrap_or(4.5);
        if gone || too_far(s.pos, eye, range) {
            self.close_overlay();
        }
    }
}

/// Eye out of reach of block `pos`: squared distance from the eye to the block's unit cube at
/// least equal to (range + 4)².
fn too_far(pos: [i32; 3], eye: [f64; 3], range: f64) -> bool {
    let d2: f64 = (0..3)
        .map(|i| {
            let lo = f64::from(pos[i]);
            (lo - eye[i]).max(eye[i] - (lo + 1.0)).max(0.0).powi(2)
        })
        .sum();
    let max = range + 4.0;
    d2 >= max * max
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sign(first: &str) -> SignEdit {
        let lines: [Vec<char>; 4] = std::array::from_fn(|i| if i == 0 { first.chars().collect() } else { Vec::new() });
        let n = lines[0].len();
        SignEdit {
            pos: [0; 3],
            front: true,
            lines,
            line: 0,
            cursor: n,
            anchor: n,
            hanging: false,
            wall: false,
            wood: "oak".into(),
            color: 0,
            edited: false,
            focus_at: std::time::Instant::now(),
        }
    }

    /// Game input helper: deletion by word, selection with Shift, cut / paste, AltGr allowed,
    /// pasting that is too wide refused entirely.
    #[test]
    fn sign_line_edits_like_the_game_text_helper() {
        let (none, ctrl, shift) = (ModifiersState::empty(), ModifiersState::CONTROL, ModifiersState::SHIFT);
        let mut s = sign("hello world");
        s.key(None, KeyCode::Backspace, ctrl, None);
        assert_eq!(s.lines()[0], "hello ");
        s.key(None, KeyCode::Home, none, None);
        s.key(None, KeyCode::ArrowRight, shift, None);
        s.key(None, KeyCode::ArrowRight, shift, None);
        assert_eq!((s.cursor, s.anchor), (2, 0));
        s.key(None, KeyCode::KeyX, ctrl, None);
        assert_eq!((s.lines()[0].as_str(), s.cursor), ("llo ", 0));
        s.key(None, KeyCode::End, none, None);
        s.key(None, KeyCode::KeyV, ctrl, None);
        assert_eq!(s.lines()[0], "llo he");
        // AltGr = Ctrl + Alt: the produced character is typed.
        s.key(None, KeyCode::Digit0, ctrl | ModifiersState::ALT, Some("@"));
        assert_eq!(s.lines()[0], "llo he@");
        // Ctrl + letter outside a shortcut, and Ctrl + Shift + V: nothing is typed.
        s.key(None, KeyCode::KeyB, ctrl, Some("b"));
        s.key(None, KeyCode::KeyV, ctrl | shift, Some("V"));
        assert_eq!(s.lines()[0], "llo he@");
        s.key(None, KeyCode::KeyA, ctrl, Some("a"));
        assert_eq!((s.cursor, s.anchor), (7, 0));
        // Pasting that is too wide (90 px) in place of the selection: nothing is changed, the selection
        // is brought back to its start.
        crate::widgets::set_clipboard("W".repeat(20));
        s.key(None, KeyCode::KeyV, ctrl, None);
        assert_eq!((s.lines()[0].as_str(), s.cursor, s.anchor), ("llo he@", 0, 0));
        s.key(None, KeyCode::ArrowDown, none, None);
        assert_eq!((s.line, s.cursor, s.anchor), (1, 0, 0));
        assert!(s.take_edit());
    }

    /// Closing: eye at (range + 4) from the block's cube or more.
    #[test]
    fn sign_closes_at_reach_plus_four_from_the_block_box() {
        assert!(!too_far([0, 0, 0], [9.49, 0.5, 0.5], 4.5));
        assert!(too_far([0, 0, 0], [9.5, 0.5, 0.5], 4.5));
        assert!(!too_far([0, 0, 0], [0.5, 9.4, 0.5], 4.5));
        assert!(too_far([0, 0, 0], [-8.5, 0.5, 0.5], 4.5));
    }

    #[test]
    fn bidirectional_language_shows_the_shaped_line() {
        // "لا" (lam, alef): ligature of a single character, measured and targeted by the cursor
        // like the string shown; elsewhere the line stays as is.
        let line = crate::gui::plain("لا");
        let shown: String = shown_line(line.clone(), true).iter().map(|c| c.c).collect();
        assert_eq!(shown, "\u{FEFB}");
        assert_eq!(shown_line(line, false).len(), 2);
        let latin: String = shown_line(crate::gui::plain("abc"), true).iter().map(|c| c.c).collect();
        assert_eq!(latin, "abc");
    }

    #[test]
    fn wood_names() {
        assert_eq!(wood_of("minecraft:dark_oak_wall_hanging_sign"), "dark_oak");
        assert_eq!(wood_of("minecraft:pale_oak_sign"), "pale_oak");
        assert_eq!(wood_of("spruce_wall_sign"), "spruce");
    }
}
