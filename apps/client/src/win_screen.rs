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
    /// Width of a centered line, for the fonts the lines were last measured with (0 otherwise).
    width: i32,
}

impl Line {
    fn new(chars: Vec<StyledChar>, centered: bool) -> Line {
        Line { chars, centered, width: 0 }
    }
}

/// Lines `first..end` that can be on a screen `height` high when the first line sits at `first_y`
/// and the scroll moves everything by `offset`: a line shows while its position is above the
/// bottom edge and less than 20 above the top one. One line of margin on each side; the drawing
/// loop applies the exact test to each.
fn lines_near_screen(first_y: i32, offset: f32, height: i32, lines: usize) -> std::ops::Range<usize> {
    let start = f64::from(first_y) + f64::from(offset);
    let clamp = |line: f64| line.clamp(0.0, lines as f64) as usize;
    let first = clamp(((-20.0 - start) / 12.0).floor() - 1.0);
    let end = clamp(((f64::from(height) - start) / 12.0).ceil() + 2.0);
    first..end.max(first)
}

pub struct WinScreen {
    poem: bool,
    /// Raw texts: poem, credits (JSON), post-credits.
    sources: [Option<String>; 3],
    player: String,
    lines: Vec<Line>,
    /// Font identity the widths of the centered lines were measured with.
    measured_fonts: Option<usize>,
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
            measured_fonts: None,
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
        let empty = || Line::new(Vec::new(), false);
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
                        lines.push(Line::new(l, false));
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
                lines.push(Line::new(colored(SECTION_HEADING, WHITE), true));
                lines.push(Line::new(colored(&str_of(section, "section"), YELLOW), true));
                lines.push(Line::new(colored(SECTION_HEADING, WHITE), true));
                lines.push(empty());
                lines.push(empty());
                for discipline in arr(section, "disciplines") {
                    let name = str_of(&discipline, "discipline");
                    if !name.is_empty() {
                        lines.push(Line::new(colored(&name, YELLOW), true));
                        lines.push(empty());
                        lines.push(empty());
                    }
                    for title in arr(&discipline, "titles") {
                        lines.push(Line::new(colored(&str_of(&title, "title"), GRAY), false));
                        for n in arr(&title, "names") {
                            let n = n.as_str().unwrap_or("");
                            lines.push(Line::new(colored(&format!("{NAME_PREFIX}{n}"), WHITE), false));
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
        let now = Instant::now();
        let dt = self.last.map_or(0.0, |l| (now - l).as_secs_f64() / tick_len.max(1e-3)) as f32;
        self.last = Some(now);
        self.scroll = (self.scroll + dt * self.speed()).max(0.0);
        // `=0`: every line tested and measured on every frame (old path). The credits of the
        // options, the other implementation of this roll, read the same switch.
        let only_visible = rc_data::opt_switch!("RUSTCRAFT_OPT_CREDITS_VISIBLE_LINES");
        self.draw_scrolled(gui, queue, only_visible);
    }

    /// Widths of the centered lines, measured again when the fonts changed since.
    fn measure_centered_lines(&mut self, gui: &Gui) {
        if self.measured_fonts == Some(gui.font_generation()) {
            return;
        }
        self.measured_fonts = Some(gui.font_generation());
        for l in self.lines.iter_mut().filter(|l| l.centered) {
            l.width = rc_text::width(gui, &l.chars);
        }
    }

    /// Frame at the current scroll. `only_visible`: only the lines that can be on screen are
    /// tested and the widths of the centered lines are kept; otherwise every line is walked and
    /// measured on every frame (old path).
    fn draw_scrolled(&mut self, gui: &mut Gui, queue: &wgpu::Queue, only_visible: bool) {
        self.build(gui);
        if only_visible {
            self.measure_centered_lines(gui);
        }
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
        let first_y = logo_y + 100;
        let mut extra = 0.0f32;
        let n = self.lines.len();
        // Line `i` sits at `first_y + 12 i` (whole numbers) plus the scroll: the lines that can
        // show are found by a division instead of a walk through thousands of lines.
        let candidates = if only_visible { lines_near_screen(first_y, off, h, n) } else { 0..n };
        for i in candidates {
            let l = &self.lines[i];
            let y = first_y + 12 * i as i32;
            // Last line: stopped in the middle of the screen.
            if i + 1 == n {
                let diff = y as f32 + off - (h / 2 - 6) as f32;
                if diff < 0.0 {
                    extra = -diff;
                }
            }
            let sy = y as f32 + off;
            if sy + 12.0 + 8.0 > 0.0 && sy < h as f32 && !l.chars.is_empty() {
                let x = if l.centered {
                    let width = if only_visible { l.width } else { rc_text::width(gui, &l.chars) };
                    logo_x + 128 - width / 2
                } else {
                    logo_x
                };
                gui.text(queue, &l.chars, x as f32, sy + extra, 0xffff_ffff, true, 0);
            }
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

    /// The lines kept as candidates contain every line the exact test lets through, for scrolls
    /// before, across and past the text.
    #[test]
    fn lines_near_the_screen_include_every_visible_line() {
        let (first_y, height, lines) = (390, 240, 500usize);
        for step in 0..4_000 {
            let offset = -(step as f32) * 1.9 + 33.3;
            let near = lines_near_screen(first_y, offset, height, lines);
            assert!(near.end <= lines && near.len() <= (height as usize + 20) / 12 + 5, "{near:?}");
            for i in 0..lines {
                let sy = (first_y + 12 * i as i32) as f32 + offset;
                let visible = sy + 12.0 + 8.0 > 0.0 && sy < height as f32;
                assert!(!visible || near.contains(&i), "line {i} at {sy} left out of {near:?}");
            }
        }
        assert_eq!(lines_near_screen(390, 0.0, 240, 500), 0..0, "text still below the screen");
        assert_eq!(lines_near_screen(390, -20_000.0, 240, 500), 500..500, "text past the screen");
    }

    /// The whole end screen (poem with scrambled words, credits, last lines), 7.3 units a frame
    /// from the top to past the end, drawn by testing only the lines near the screen against
    /// walking every line: layers equal to the byte at two interface scales, fonts replaced
    /// half-way (kept widths measured again). Without an installed game or a GPU: skipped
    /// (`RUSTCRAFT_OPT_REQUIRE=1`: failure).
    #[test]
    fn lines_near_the_screen_draw_the_layers_of_the_full_walk() {
        use crate::gui::tests_opt;
        let Some(e) = tests_opt::env() else { return };
        for (scale, step) in [(3, 7.3f32), (1, 83.1)] {
            let mut paths = [false, true].map(|only_visible| {
                let mut screen = WinScreen::new(true, "Tester", |p| e.assets.read(p));
                screen.minceraft = false;
                (only_visible, screen, tests_opt::gui(&e, tests_opt::ALL))
            });
            let mut seen = hashbrown::HashSet::new();
            let mut frame = 0;
            // Until the screen asks for the respawn, and a little further.
            let mut frames_after_end = 0;
            while frames_after_end < 20 {
                let mut layers = Vec::new();
                for (only_visible, screen, gui) in &mut paths {
                    if frame == 2_000 {
                        let opts = rc_assets::font::FontOptions { uniform: true, jp: false };
                        let load = |id: &&str| ((*id).to_owned(), rc_assets::font::Font::load(&e.assets, id, opts));
                        gui.set_fonts(crate::resources::FONT_IDS.iter().map(load).collect());
                    }
                    gui.scale_option = scale;
                    gui.begin(1600, 900);
                    screen.scroll = frame as f32 * step;
                    screen.draw_scrolled(gui, &e.queue, *only_visible);
                    layers.push(tests_opt::layer_bytes(gui));
                }
                let first = layers[0].iter().zip(&layers[1]).position(|(a, b)| a != b);
                assert!(
                    layers[0].len() == layers[1].len() && first.is_none(),
                    "scale {scale}, frame {frame}: layers differ (byte {first:?})"
                );
                seen.insert(layers.swap_remove(0));
                assert_eq!(paths[0].1.finished, paths[1].1.finished);
                frames_after_end += usize::from(paths[0].1.finished);
                frame += 1;
                assert!(frame < 100_000, "the end screen never finishes");
            }
            let lines = paths[0].1.lines.len();
            assert!(lines > 1_000, "{lines} lines: poem and credits of the game");
            assert!(seen.len() * 10 > frame * 9, "scale {scale}: {} distinct frames out of {frame}", seen.len());
        }
    }

    /// Micro-bench of a frame of the end screen at three moments of the scroll (loaded machine
    /// possible): every line walked against the lines near the screen only. Time and bytes
    /// allocated per frame. Main thread by nature. Without a GPU: skipped.
    /// `cargo test --release -p rc-app bench_end_screen -- --ignored --nocapture`
    #[test]
    #[ignore = "micro-bench"]
    fn bench_end_screen() {
        use crate::gui::tests_opt;
        let Some(e) = tests_opt::env() else { return };
        let mut gui = tests_opt::gui(&e, tests_opt::ALL);
        for pass in 0..2 {
            for (label, part) in [("poem", 0.02f32), ("credits", 0.5), ("end", 0.98)] {
                for (name, only_visible) in [("every line walked", false), ("lines near the screen", true)] {
                    let mut screen = WinScreen::new(true, "Tester", |p| e.assets.read(p));
                    screen.minceraft = false;
                    screen.build(&gui);
                    screen.scroll = (screen.lines.len() * 12) as f32 * part;
                    let mut once = |gui: &mut Gui| {
                        gui.begin(1600, 900);
                        screen.draw_scrolled(gui, &e.queue, only_visible);
                    };
                    (0..200).for_each(|_| once(&mut gui));
                    let frames = 10_000;
                    let t = Instant::now();
                    (0..frames).for_each(|_| once(&mut gui));
                    let per_frame = t.elapsed().as_secs_f64() * 1e6 / f64::from(frames);
                    let bytes = crate::debug_sys::allocated_bytes(|| (0..100).for_each(|_| once(&mut gui))) / 100;
                    println!(
                        "end screen, pass {pass}, {label:<7} {name:<21}: {per_frame:>6.2} µs, {bytes:>5} bytes allocated per frame"
                    );
                }
            }
        }
    }

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
