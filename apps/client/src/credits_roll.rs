//! Credits roll without the poem ("Credits and Attributions" → "Credits",
//! `docs/specs/option-menus.md` § 3.14): sections of `texts/credits.json`, logo at the top, menu
//! background that follows the scroll, credits vignette. Esc or the end of the scroll closes
//! the screen. Same keys as the end screen: Space speeds up (×5), each Ctrl adds 15,
//! up arrow reverses.

use std::time::Instant;

use rc_text::StyledChar;
use winit::keyboard::KeyCode;

use crate::gui::{Blend, Gui};
use crate::widgets::chars;

/// Speed without the poem: 0.75 unit per tick.
const SPEED: f32 = 0.75;

pub struct CreditsRoll {
    /// Lines and centering.
    lines: Vec<(Vec<StyledChar>, bool)>,
    scroll: f32,
    last: Option<Instant>,
    speedup: bool,
    /// Left and right Ctrl held down.
    ctrl: [bool; 2],
    direction: f32,
}

impl CreditsRoll {
    /// Lines of the credits file: framed section header, disciplines (yellow), titles
    /// (gray) and offset names, empty lines between groups (12 high each).
    pub fn new(credits_json: &[u8]) -> CreditsRoll {
        let mut lines = Vec::new();
        let empty = |l: &mut Vec<(Vec<StyledChar>, bool)>| l.push((Vec::new(), false));
        let bytes = credits_json.strip_prefix(b"\xEF\xBB\xBF").unwrap_or(credits_json);
        let v: serde_json::Value = serde_json::from_slice(bytes).unwrap_or_else(|e| {
            eprintln!("unreadable credits: {e}");
            serde_json::Value::Null
        });
        let s = |v: &serde_json::Value, k: &str| v[k].as_str().unwrap_or("").to_owned();
        for section in v.as_array().into_iter().flatten() {
            lines.push((chars("§f============"), true));
            lines.push((chars(&format!("§e{}", s(section, "section"))), true));
            lines.push((chars("§f============"), true));
            empty(&mut lines);
            empty(&mut lines);
            for d in section["disciplines"].as_array().into_iter().flatten() {
                let name = s(d, "discipline");
                if !name.is_empty() {
                    lines.push((chars(&format!("§e{name}")), true));
                    empty(&mut lines);
                    empty(&mut lines);
                }
                for t in d["titles"].as_array().into_iter().flatten() {
                    lines.push((chars(&format!("§7{}", s(t, "title"))), false));
                    for n in t["names"].as_array().into_iter().flatten() {
                        lines.push((chars(&format!("§f           {}", n.as_str().unwrap_or(""))), false));
                    }
                    empty(&mut lines);
                    empty(&mut lines);
                }
            }
        }
        CreditsRoll { lines, scroll: 0.0, last: None, speedup: false, ctrl: [false; 2], direction: 1.0 }
    }

    /// Speed in units per tick: base × (5 + 15 per Ctrl) with Space, sign of the direction.
    fn speed(&self) -> f32 {
        let n = self.ctrl.iter().filter(|&&c| c).count() as f32;
        let k = if self.speedup { 5.0 + n * 15.0 } else { 1.0 };
        SPEED * k * self.direction
    }

    /// Key press or release; true for the Esc press (closing).
    pub fn key(&mut self, code: KeyCode, pressed: bool) -> bool {
        match code {
            KeyCode::Escape => return pressed,
            KeyCode::ArrowUp => self.direction = if pressed { -1.0 } else { 1.0 },
            KeyCode::Space => self.speedup = pressed,
            KeyCode::ControlLeft => self.ctrl[0] = pressed,
            KeyCode::ControlRight => self.ctrl[1] = pressed,
            _ => {}
        }
        false
    }

    /// End of the scroll: all the text has passed (total height + two screens + 24).
    pub fn finished(&self, height: f32) -> bool {
        self.scroll > (self.lines.len() * 12) as f32 + 2.0 * height + 24.0
    }

    pub fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, blur: i32) {
        let now = Instant::now();
        let dt = self.last.map_or(0.0, |t| now.duration_since(t).as_secs_f32().min(0.1));
        self.last = Some(now);
        self.scroll = (self.scroll + dt * 20.0 * self.speed()).max(0.0);
        let (w, h) = (gui.width, gui.height);
        gui.next_layer();
        gui.blur(blur);
        gui.sprite_tiled("bg/menu_background", 0.0, 0.0, w, h, 32.0, 32.0, 0.0, self.scroll * 0.5);
        // Credits vignette (texture `misc/credits_vignette`): darkens the edges.
        gui.stratum(Blend::Vignette);
        gui.sprite("tex/misc/credits_vignette", 0.0, 0.0, w, h, 0xffff_ffff);
        gui.stratum(Blend::Normal);
        gui.next_layer();
        let off = -self.scroll;
        let logo_x = (w as i32 / 2 - 128) as f32;
        let logo_y = h + 50.0 + off;
        gui.sprite_region_color(
            "tex/gui/title/minecraft",
            (256.0, 64.0),
            (0.0, 0.0),
            [logo_x, logo_y, 256.0, 44.0],
            0xffff_ffff,
        );
        gui.sprite_region_color(
            "tex/gui/title/edition",
            (128.0, 16.0),
            (0.0, 0.0),
            [(w as i32 / 2 - 64) as f32, logo_y + 37.0, 128.0, 14.0],
            0xffff_ffff,
        );
        let mut y = logo_y + 100.0;
        let n = self.lines.len();
        for (i, (line, centered)) in self.lines.iter().enumerate() {
            // Last line: stopped in the middle of the screen.
            if i + 1 == n {
                y = y.max(h / 2.0 - 6.0);
            }
            if y + 20.0 > 0.0 && y < h && !line.is_empty() {
                let x = if *centered { logo_x + 128.0 - (rc_text::width(gui, line) / 2) as f32 } else { logo_x };
                gui.text(q, line, x, y.floor(), 0xffff_ffff, true, 0);
            }
            y += 12.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credits_lines_follow_sections() {
        let json =
            br#"[{"section":"S","disciplines":[{"discipline":"D","titles":[{"title":"T","names":["A","B"]}]}]}]"#;
        let r = CreditsRoll::new(json);
        // Header (3) + 2 empty, discipline + 2 empty, title + 2 names + 2 empty.
        assert_eq!(r.lines.len(), 3 + 2 + 3 + 5);
        assert!(r.lines[1].1 && !r.lines[8].1);
    }

    #[test]
    fn speed_keys_match_end_screen() {
        let mut r = CreditsRoll::new(b"[]");
        assert_eq!(r.speed(), 0.75);
        r.key(KeyCode::Space, true);
        assert_eq!(r.speed(), 3.75);
        r.key(KeyCode::ControlLeft, true);
        r.key(KeyCode::ControlRight, true);
        assert_eq!(r.speed(), 0.75 * 35.0);
        r.key(KeyCode::ArrowUp, true);
        assert!(r.speed() < 0.0);
        r.key(KeyCode::ArrowUp, false);
        r.key(KeyCode::Space, false);
        assert_eq!(r.speed(), 0.75);
        assert!(r.key(KeyCode::Escape, true) && !r.key(KeyCode::Escape, false));
    }
}
