//! Input-method (IME) input common to all text fields, like the game: IME input
//! is allowed as long as an editable field has focus (simple fields, book area, sign,
//! chat), the system's candidate window is placed on the field's cursor, and the composition
//! in progress is drawn in a floating frame (sprite `widget/preedit`) 4 px below the field, or
//! above if it would leave the screen, in the last stratum of the interface.
//!
//! Each field that has focus declares its cursor position while it draws
//! ([`field_cursor`]); the end of the frame ([`finish_frame`]) draws the composition and returns the area
//! to give to the system (`links.rs`, `sync_ime`). The committed text arrives as
//! `WindowEvent::Ime(Ime::Commit)`: each screen handles it like a keystroke ([`commit`]).

use std::cell::RefCell;
use std::time::Instant;

use rc_text::StyledChar;
use winit::event::{Ime, WindowEvent};
use winit::keyboard::KeyCode;

use crate::gui::Gui;

/// Neutral key accompanying a text committed by the IME (no screen handles it): the screens
/// whose input goes through "key + text" thus receive the text alone.
pub const TEXT_ONLY: KeyCode = KeyCode::F35;

/// Composition in progress: text, cursor position (characters) and split into blocks, the targeted
/// block underlined.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preedit {
    pub text: Vec<char>,
    pub caret: usize,
    /// Blocks (start, end) in characters, covering the whole text.
    pub blocks: Vec<(usize, usize)>,
    pub focused: usize,
}

impl Preedit {
    /// Composition described by the system: `range` = start and end (bytes) of the targeted part;
    /// `None` puts the cursor at the end. Same split as the game: text before the cursor,
    /// targeted part (underlined), rest; an empty part gives a single underlined block.
    pub fn from_system(text: &str, range: Option<(usize, usize)>) -> Option<Preedit> {
        if text.is_empty() {
            return None;
        }
        let chars: Vec<char> = text.chars().collect();
        let n = chars.len();
        let at = |b: usize| text.get(..b.min(text.len())).map_or(n, |s| s.chars().count());
        let (caret, len) = match range {
            Some((a, b)) => (at(a).min(n), at(b.max(a)).saturating_sub(at(a))),
            None => (n, 0),
        };
        let whole = Preedit { text: chars, caret, blocks: vec![(0, n)], focused: 0 };
        if len == 0 {
            return Some(whole);
        }
        let end = (caret + len).min(n);
        if end <= caret {
            return Some(whole);
        }
        let mut blocks = Vec::with_capacity(3);
        let mut focused = 0;
        if caret > 0 {
            blocks.push((0, caret));
            focused = 1;
        }
        blocks.push((caret, end));
        if end < n {
            blocks.push((end, n));
        }
        Some(Preedit { blocks, focused, ..whole })
    }

    /// Black text, targeted block underlined.
    pub fn styled(&self) -> Vec<StyledChar> {
        let mut out = crate::gui::plain(&self.text.iter().collect::<String>());
        let (a, b) = self.blocks.get(self.focused).copied().unwrap_or((0, 0));
        for (i, c) in out.iter_mut().enumerate() {
            c.style.color = Some(0);
            c.style.underlined = (a..b).contains(&i);
        }
        out
    }
}

/// Composition frame (left, top, right, bottom) for a field whose cursor is at
/// `(x, y)` and the line `h` high, on a screen `w` × `sh`; `pw` = text width.
pub fn overlay_rect(x: i32, y: i32, h: i32, pw: i32, w: i32, sh: i32) -> [i32; 4] {
    let mut left = x;
    if left + pw > w {
        left = w - pw;
    }
    let mut bottom = y + h + 4 + 9;
    if bottom > sh {
        bottom = y - 4 - 9;
    }
    [left, bottom - 9, left + pw, bottom]
}

#[derive(Default)]
struct State {
    /// Composition received and instant of reception (the cursor blinks from that instant).
    preedit: Option<(Preedit, Instant)>,
    /// Cursor of the field that has focus, declared during the frame: x, y, line height.
    field: Option<(i32, i32, i32)>,
    /// Input area of the last frame (interface units), `None`: IME input off.
    area: Option<[i32; 4]>,
    /// State applied to the window: IME input allowed, candidate area.
    applied: (bool, Option<[i32; 4]>),
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

/// Composition received (or ended: `None`).
pub fn set_preedit(p: Option<Preedit>) {
    STATE.with_borrow_mut(|s| s.preedit = p.map(|p| (p, Instant::now())));
}

/// Composition in progress (tests).
#[cfg(test)]
pub fn preedit() -> Option<Preedit> {
    STATE.with_borrow(|s| s.preedit.as_ref().map(|p| p.0.clone()))
}

/// An editable field that has focus is drawn, cursor at `(x, y)`, line `h` high.
pub fn field_cursor(x: i32, y: i32, h: i32) {
    STATE.with_borrow_mut(|s| s.field = Some((x, y, h)));
}

/// End of the interface: composition drawn over everything except tooltips (if a field has
/// focus); input area retained for the system. Without a field, the composition is abandoned.
pub fn finish_frame(gui: &mut Gui, q: &wgpu::Queue) {
    let (field, pre) = STATE.with_borrow_mut(|s| {
        let field = s.field.take();
        if field.is_none() {
            s.preedit = None;
        }
        (field, s.preedit.clone())
    });
    let styled = pre.as_ref().map(|(p, _)| p.styled());
    let pw = styled.as_ref().map(|s| rc_text::width(gui, s));
    let area = resolve(field, pw, gui.width as i32, gui.height as i32);
    if let (Some(r), Some((p, since)), Some(styled)) = (area, pre, styled) {
        let f = |v: i32| v as f32;
        // New stratum above the interface, but below the tooltips, like the game.
        gui.draw_before_tooltips(|gui| {
            gui.next_layer();
            let (w, h) = (f(r[2] - r[0] + 10), f(r[3] - r[1] + 10));
            gui.sprite("widget/preedit", f(r[0] - 5), f(r[1] - 5), w, h, 0xffff_ffff);
            gui.text(q, &styled, f(r[0]), f(r[1]), 0xff00_0000, false, 0);
            if (since.elapsed().as_millis() / 300).is_multiple_of(2) {
                let cx = r[0] + rc_text::width(gui, &styled[..p.caret.min(styled.len())]);
                gui.fill(f(cx), f(r[1] - 1), f(cx + 1), f(r[1] + 10), 0xff00_0000);
            }
        });
    }
    STATE.with_borrow_mut(|s| s.area = area);
}

/// Input area of a frame: none without a field that has focus, the cursor (1 × line
/// height) without composition, otherwise the composition frame (`pw` = width of its text).
fn resolve(field: Option<(i32, i32, i32)>, pw: Option<i32>, w: i32, sh: i32) -> Option<[i32; 4]> {
    let (x, y, h) = field?;
    Some(match pw {
        None => [x, y, x + 1, y + h],
        Some(pw) => overlay_rect(x, y, h, pw, w, sh),
    })
}

/// Changes to make to the window since the last call: IME input allowed or not (an
/// editable field has focus), then candidate area (interface units) if it moved.
pub fn window_update() -> (Option<bool>, Option<[i32; 4]>) {
    STATE.with_borrow_mut(|s| {
        let want = s.area.is_some();
        let allow = (want != s.applied.0).then(|| {
            s.applied = (want, None);
            want
        });
        let area = (want && s.area != s.applied.1).then(|| {
            s.applied.1 = s.area;
            s.area
        });
        (allow, area.flatten())
    })
}

/// Text committed by the IME carried by an event (handled like a keystroke by the screen).
pub fn commit(event: &WindowEvent) -> Option<&str> {
    match event {
        WindowEvent::Ime(Ime::Commit(t)) => Some(t.as_str()).filter(|t| t.chars().all(|c| !c.is_control())),
        _ => None,
    }
}

/// IME events other than committed text: composition updated or ended. True if handled.
pub fn observe(event: &WindowEvent) -> bool {
    match event {
        WindowEvent::Ime(Ime::Preedit(t, range)) => {
            set_preedit(Preedit::from_system(t, *range));
            true
        }
        WindowEvent::Ime(Ime::Enabled | Ime::Disabled) => {
            set_preedit(None);
            true
        }
        WindowEvent::Ime(Ime::Commit(_)) => {
            set_preedit(None);
            false
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Split of the composition like the game: before the cursor, targeted part underlined, rest;
    /// with no targeted part, all the text underlined and the cursor where the system put it.
    #[test]
    fn preedit_blocks_follow_the_system_selection() {
        assert_eq!(Preedit::from_system("", None), None);
        let p = Preedit::from_system("にほんご", Some((3, 9))).unwrap();
        assert_eq!((p.caret, p.blocks.clone(), p.focused), (1, vec![(0, 1), (1, 3), (3, 4)], 1));
        let u: Vec<bool> = p.styled().iter().map(|c| c.style.underlined).collect();
        assert_eq!(u, [false, true, true, false]);
        assert!(p.styled().iter().all(|c| c.style.color == Some(0)));
        let p = Preedit::from_system("abc", Some((0, 3))).unwrap();
        assert_eq!((p.caret, p.blocks, p.focused), (0, vec![(0, 3)], 0));
        let p = Preedit::from_system("abc", Some((2, 2))).unwrap();
        assert_eq!((p.caret, p.blocks, p.focused), (2, vec![(0, 3)], 0));
        let p = Preedit::from_system("abc", None).unwrap();
        assert_eq!((p.caret, p.blocks), (3, vec![(0, 3)]));
    }

    /// Frame 4 px below the line (10 high), above if it leaves the screen, brought back to the left
    /// if it overflows on the right.
    #[test]
    fn overlay_sits_under_the_line_or_above_near_the_bottom() {
        assert_eq!(overlay_rect(10, 100, 10, 30, 400, 300), [10, 114, 40, 123]);
        assert_eq!(overlay_rect(380, 100, 10, 30, 400, 300), [370, 114, 400, 123]);
        assert_eq!(overlay_rect(10, 288, 10, 30, 400, 300), [10, 266, 40, 275]);
    }

    /// With no field that has focus, no area: IME input is off and the composition
    /// abandoned; a declared field gives the area of its cursor (1 × 10).
    #[test]
    fn area_follows_the_focused_field() {
        assert_eq!(resolve(None, Some(20), 400, 300), None);
        assert_eq!(resolve(Some((5, 6, 10)), None, 400, 300), Some([5, 6, 6, 16]));
        assert_eq!(resolve(Some((5, 6, 9)), Some(20), 400, 300), Some([5, 19, 25, 28]));
        // Window: input allowed then area, once each; off without a field.
        STATE.with_borrow_mut(|s| s.area = Some([5, 6, 6, 16]));
        assert_eq!(window_update(), (Some(true), Some([5, 6, 6, 16])));
        assert_eq!(window_update(), (None, None));
        STATE.with_borrow_mut(|s| s.area = None);
        assert_eq!(window_update(), (Some(false), None));
        assert!(observe(&WindowEvent::Ime(Ime::Preedit("ab".into(), None))));
        assert_eq!(preedit().map(|p| p.caret), Some(2));
        assert!(observe(&WindowEvent::Ime(Ime::Disabled)));
        assert_eq!(preedit(), None);
        // Committed text: composition ended, text returned to the screen like a keystroke.
        set_preedit(Preedit::from_system("a", None));
        let c = WindowEvent::Ime(Ime::Commit("日本".into()));
        assert!(!observe(&c));
        assert_eq!((preedit(), commit(&c)), (None, Some("日本")));
    }
}
