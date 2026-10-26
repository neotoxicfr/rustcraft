//! End screen (`docs/specs/end-sleep-spectator.md` § 1): opened by game event 4
//! (leaving the End), poem then credits scrolling over the End portal background, credits
//! music; at the end (or Esc) the client requests its respawn (`client_command` 0).

use std::time::Instant;

use rc_text::StyledChar;

use crate::gui::{Blend, Gui};

/// Prefix of the names under a credits title (11 spaces).
const NAME_PREFIX: &str = "           ";
/// Poem token replaced by 3 to 6 scrambled characters.
const OBFUSCATE_TOKEN: &str = "§f§k§a§b";
/// Fixed seed of the draw of the scrambled lengths.
const POEM_SEED: i64 = 8_124_371;
const SECTION_HEADING: &str = "============";
/// Colors of the formatting codes used (yellow, gray, white).
const YELLOW: u32 = 0xFF_FF55;
const GRAY: u32 = 0xAA_AAAA;
const WHITE: u32 = 0xFF_FFFF;

/// Line ready to draw, centered or left-aligned in the column.
struct Line {
    chars: Vec<StyledChar>,
    centered: bool,
}

pub struct WinScreen {
    poem: bool,
    /// Raw texts: poem, credits (JSON), post-credits.
    sources: [Option<String>; 3],
    player: String,
    lines: Vec<Line>,
    /// "minceraft" logo (one chance in 10000).
    minceraft: bool,
    scroll: f32,
    base_speed: f32,
    speedup: bool,
    /// Left and right Ctrl keys held down (each adds 15 to the acceleration).
    ctrl: [bool; 2],
    direction: f32,
    last: Option<Instant>,
    /// End of the scroll reached: respawn requested at the next tick.
    pub finished: bool,
}

impl WinScreen {
    /// `read` reads a game resource (`assets/minecraft/texts/…`).
    pub fn new(poem: bool, player: &str, read: impl Fn(&str) -> Option<Vec<u8>>) -> Self {
        let text = |p: &str| read(p).and_then(|b| String::from_utf8(b).ok());
        let mut rng = rc_audio::JavaRandom::from_entropy();
        WinScreen {
            poem,
            sources: [
                text("assets/minecraft/texts/end.txt").filter(|_| poem),
                text("assets/minecraft/texts/credits.json"),
                text("assets/minecraft/texts/postcredits.txt").filter(|_| poem),
            ],
            player: player.to_owned(),
            lines: Vec::new(),
            minceraft: rng.next_float() < 1.0e-4,
            scroll: 0.0,
            base_speed: if poem { 0.5 } else { 0.75 },
            speedup: false,
            ctrl: [false; 2],
            direction: 1.0,
            last: None,
            finished: false,
        }
    }

    fn speed(&self) -> f32 {
        let n = self.ctrl.iter().filter(|&&c| c).count() as f32;
        let k = if self.speedup { 5.0 + n * 15.0 } else { 1.0 };
        self.base_speed * k * self.direction
    }

    /// Key: up arrow = back, Space = accelerate, Ctrl (left, right) = even more.
    /// Returns true for Esc (closing: respawn).
    pub fn key(&mut self, code: winit::keyboard::KeyCode, pressed: bool) -> bool {
        use winit::keyboard::KeyCode as K;
        match code {
            K::Escape => return pressed,
            K::ArrowUp => self.direction = if pressed { -1.0 } else { 1.0 },
            K::Space => self.speedup = pressed,
            K::ControlLeft => self.ctrl[0] = pressed,
            K::ControlRight => self.ctrl[1] = pressed,
            _ => {}
        }
        false
    }

    /// Builds the lines (once, with the font metrics).
    fn build(&mut self, gui: &Gui) {
        if !self.lines.is_empty() {
            return;
        }
        let plain = |s: &str| crate::gui::plain(s);
        let colored = |s: &str, color: u32| {
            let mut c = plain(s);
            for ch in &mut c {
                ch.style.color.get_or_insert(color);
            }
            c
        };
        let empty = || Line { chars: Vec::new(), centered: false };
        let mut lines = Vec::new();
        let mut rng = rc_audio::JavaRandom::new(POEM_SEED);
        let mut poem = |text: &str, lines: &mut Vec<Line>| {
            for raw in text.lines() {
                let mut line = raw.replace("PLAYERNAME", &self.player);
                while let Some(p) = line.find(OBFUSCATE_TOKEN) {
                    let n = rng.next_int_bounded(4) as usize + 3;
                    line = format!("{}§f§k{}{}", &line[..p], &"XXXXXXXX"[..n], &line[p + OBFUSCATE_TOKEN.len()..]);
                }
                let chars = plain(&line);
                // Empty line: no text line, only the line spacing that follows.
                if !chars.is_empty() {
                    for l in rc_text::wrap(gui, &chars, 256.0) {
                        lines.push(Line { chars: l, centered: false });
                    }
                }
                lines.push(empty());
            }
            for _ in 0..8 {
                lines.push(empty());
            }
        };
        if let Some(t) = &self.sources[0] {
            poem(t, &mut lines);
        }
        if let Some(json) = self.sources[1].as_ref().and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok()) {
            let str_of = |v: &serde_json::Value, k: &str| v.get(k).and_then(|s| s.as_str()).unwrap_or("").to_owned();
            let arr = |v: &serde_json::Value, k: &str| v.get(k).and_then(|a| a.as_array()).cloned().unwrap_or_default();
            for section in json.as_array().into_iter().flatten() {
                lines.push(Line { chars: colored(SECTION_HEADING, WHITE), centered: true });
                lines.push(Line { chars: colored(&str_of(section, "section"), YELLOW), centered: true });
                lines.push(Line { chars: colored(SECTION_HEADING, WHITE), centered: true });
                lines.push(empty());
                lines.push(empty());
                for discipline in arr(section, "disciplines") {
                    let name = str_of(&discipline, "discipline");
                    if !name.is_empty() {
                        lines.push(Line { chars: colored(&name, YELLOW), centered: true });
                        lines.push(empty());
                        lines.push(empty());
                    }
                    for title in arr(&discipline, "titles") {
                        lines.push(Line { chars: colored(&str_of(&title, "title"), GRAY), centered: false });
                        for n in arr(&title, "names") {
                            let n = n.as_str().unwrap_or("");
                            lines.push(Line { chars: colored(&format!("{NAME_PREFIX}{n}"), WHITE), centered: false });
                        }
                        lines.push(empty());
                        lines.push(empty());
                    }
                }
            }
        }
        if let Some(t) = &self.sources[2] {
            poem(t, &mut lines);
        }
        if lines.is_empty() {
            lines.push(empty());
        }
        self.lines = lines;
    }

    /// End portal background (poem only; otherwise menu background, not handled here).
    pub fn portal_background(&self) -> bool {
        self.poem
    }

    /// Full-screen drawing; the scroll advances by the elapsed time (in ticks, `tick_len` in s).
    pub fn draw(&mut self, gui: &mut Gui, queue: &wgpu::Queue, tick_len: f64) {
        self.build(gui);
        let now = Instant::now();
        let dt = self.last.map_or(0.0, |l| (now - l).as_secs_f64() / tick_len.max(1e-3)) as f32;
        self.last = Some(now);
        self.scroll = (self.scroll + dt * self.speed()).max(0.0);
        let (w, h) = (gui.width as i32, gui.height as i32);
        if self.scroll > (self.lines.len() as i32 * 12 + h + h + 24) as f32 {
            self.finished = true;
        }
        gui.next_layer();
        // Credits vignette: darkens the edges (dst·(1 − src)).
        gui.stratum(Blend::Vignette);
        gui.sprite("tex/misc/credits_vignette", 0.0, 0.0, w as f32, h as f32, 0xffff_ffff);
        gui.stratum(Blend::Normal);
        let off = -self.scroll;
        let logo_x = w / 2 - 128;
        let logo_y = h + 50;
        // 256 × 44 logo (256 × 64 image) and 128 × 14 edition (128 × 16 image) overlapping by 7.
        let logo = if self.minceraft { "tex/gui/title/minceraft" } else { "tex/gui/title/minecraft" };
        let ly = logo_y as f32 + off;
        gui.sprite_region_color(logo, (256.0, 64.0), (0.0, 0.0), [logo_x as f32, ly, 256.0, 44.0], 0xffff_ffff);
        gui.sprite_region_color(
            "tex/gui/title/edition",
            (128.0, 16.0),
            (0.0, 0.0),
            [(w / 2 - 64) as f32, ly + 37.0, 128.0, 14.0],
            0xffff_ffff,
        );
        let mut y = logo_y + 100;
        let mut extra = 0.0f32;
        let n = self.lines.len();
        for (i, l) in self.lines.iter().enumerate() {
            // Last line: stopped in the middle of the screen.
            if i + 1 == n {
                let diff = y as f32 + off - (h / 2 - 6) as f32;
                if diff < 0.0 {
                    extra = -diff;
                }
            }
            let sy = y as f32 + off;
            if sy + 12.0 + 8.0 > 0.0 && sy < h as f32 && !l.chars.is_empty() {
                let x = if l.centered { logo_x + 128 - rc_text::width(gui, &l.chars) / 2 } else { logo_x };
                gui.text(queue, &l.chars, x as f32, sy + extra, 0xffff_ffff, true, 0);
            }
            y += 12;
        }
    }
}

/// Poem background: a large End portal quad (15 layers, coordinates projected onto
/// the screen) 0.1 block in front of the camera, which covers the whole image and masks the world by
/// depth. Block-entity frame: x, z relative to the camera's integer origin.
pub fn push_backdrop(fx: &mut crate::block_fx::FxFrame, shot: &crate::camera::Shot) {
    let origin = [shot.pos[0].floor(), shot.pos[2].floor()];
    let eye = glam::Vec3::new((shot.pos[0] - origin[0]) as f32, shot.pos[1] as f32, (shot.pos[2] - origin[1]) as f32);
    let (right, up, fwd) = crate::camera::axes(shot.yaw, shot.pitch);
    let c = eye + fwd * 0.1;
    let v = |a: f32, b: f32| {
        let p = c + right * a + up * b;
        crate::block_fx::PortalVertex { pos: p.to_array(), layers: 15 }
    };
    let q = [v(-5.0, -5.0), v(5.0, -5.0), v(5.0, 5.0), v(-5.0, 5.0)];
    // Two faces (no face sorting assumed).
    fx.portals.extend([0, 1, 2, 0, 2, 3, 0, 2, 1, 0, 3, 2].map(|k| q[k]));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speed_rules() {
        let mut s = WinScreen::new(true, "Bench", |_| None);
        assert_eq!(s.speed(), 0.5);
        s.key(winit::keyboard::KeyCode::Space, true);
        assert_eq!(s.speed(), 2.5);
        s.key(winit::keyboard::KeyCode::ControlLeft, true);
        assert_eq!(s.speed(), 10.0);
        s.key(winit::keyboard::KeyCode::ArrowUp, true);
        assert_eq!(s.speed(), -10.0);
        assert!(s.key(winit::keyboard::KeyCode::Escape, true));
        let c = WinScreen::new(false, "Bench", |_| None);
        assert_eq!(c.speed(), 0.75);
    }
}
