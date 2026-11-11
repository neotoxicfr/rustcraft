//! Credits roll without the poem ("Credits and Attributions" → "Credits",
//! `docs/specs/option-menus.md` § 3.14): sections of `texts/credits.json`, logo at the top, menu
//! background that follows the scroll, credits vignette. Esc or the end of the scroll closes
//! the screen. Same keys as the end screen: Space speeds up (×5), each Ctrl adds 15,
//! up arrow reverses.

use std::time::Instant;

use rc_text::StyledChar;
use winit::keyboard::KeyCode;

use crate::gui::{Blend, Gui};
use crate::kept_text::misc_screen_caches_on;
use crate::widgets::chars;

/// Speed without the poem: 0.75 unit per tick.
const SPEED: f32 = 0.75;

/// Width of a centred line not measured yet.
const NOT_MEASURED: i32 = i32::MIN;

/// Lines above the screen that the drawing loop may skip in one step when the first line sits
/// at `first_y`: all of them end at least a line above the top edge (y + 20 ≤ 0), with one more
/// line of margin.
fn lines_above_screen(first_y: f32) -> usize {
    let above = ((-20.0 - f64::from(first_y)) / 12.0).floor() - 1.0;
    if above > 0.0 { above as usize } else { 0 }
}

pub struct CreditsRoll {
    /// Lines and centering.
    lines: Vec<(Vec<StyledChar>, bool)>,
    /// Width of each centred line, measured when it first comes on screen, for the fonts
    /// `widths_fonts`.
    widths: Vec<i32>,
    widths_fonts: Option<usize>,
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
        CreditsRoll {
            widths: vec![NOT_MEASURED; lines.len()],
            widths_fonts: None,
            lines,
            scroll: 0.0,
            last: None,
            speedup: false,
            ctrl: [false; 2],
            direction: 1.0,
        }
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
        self.draw_scrolled(gui, q, blur, misc_screen_caches_on());
    }

    /// Frame at the current scroll. `only_visible`: the lines above the screen are skipped in
    /// one step, the loop stops below the screen and the widths of the centred lines are kept;
    /// otherwise every line is walked and measured on every frame (old path).
    fn draw_scrolled(&mut self, gui: &mut Gui, q: &wgpu::Queue, blur: i32, only_visible: bool) {
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
        if only_visible {
            self.draw_visible_lines(gui, q, logo_x, logo_y + 100.0);
        } else {
            self.draw_lines_walking_all(gui, q, logo_x, logo_y + 100.0);
        }
    }

    /// Lines from `first_y` down, 12 apart, every line tested (old path).
    fn draw_lines_walking_all(&self, gui: &mut Gui, q: &wgpu::Queue, logo_x: f32, first_y: f32) {
        let h = gui.height;
        let mut y = first_y;
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

    /// Same lines at the same places as [`CreditsRoll::draw_lines_walking_all`], reaching the
    /// first line near the screen in one step and stopping below the screen.
    fn draw_visible_lines(&mut self, gui: &mut Gui, q: &wgpu::Queue, logo_x: f32, first_y: f32) {
        if self.widths_fonts != Some(gui.font_generation()) {
            self.widths_fonts = Some(gui.font_generation());
            self.widths.fill(NOT_MEASURED);
        }
        let h = gui.height;
        let n = self.lines.len();
        // The last line is never skipped: it stops in the middle of the screen. While y stays
        // below zero each `y += 12` of the walk is exact (the sum only loses magnitude), so one
        // multiplication lands on the very value the walk reaches.
        let skipped = lines_above_screen(first_y).min(n.saturating_sub(1));
        let mut y = first_y + 12.0 * skipped as f32;
        for i in skipped..n {
            if i + 1 == n {
                y = y.max(h / 2.0 - 6.0);
            }
            // Below the screen: so is every following line (the last one included).
            if y >= h {
                break;
            }
            let (line, centered) = &self.lines[i];
            if y + 20.0 > 0.0 && !line.is_empty() {
                let x = if *centered {
                    if self.widths[i] == NOT_MEASURED {
                        self.widths[i] = rc_text::width(gui, line);
                    }
                    logo_x + 128.0 - (self.widths[i] / 2) as f32
                } else {
                    logo_x
                };
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

    /// The lines skipped above the screen are reached in one step on the very value the walk
    /// reaches by adding 12 line after line (bit for bit, for twenty thousand scroll
    /// positions), and none of them could show: each ends at least a line above the top edge.
    #[test]
    fn skipped_lines_land_on_the_walked_position() {
        assert_eq!([100.0, -31.0, -44.0, -1000.0].map(lines_above_screen), [0, 0, 1, 80]);
        // Fixed sequence (64-bit LCG): first lines between −70,000 and +600, any fraction.
        let mut state = 0x9E37_79B9_7F4A_7C15u64;
        let mut skipped_some = 0;
        for _ in 0..20_000 {
            state = state.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
            let first_y = ((state >> 11) as f64 / (1u64 << 53) as f64 * 70_600.0 - 70_000.0) as f32;
            let skipped = lines_above_screen(first_y);
            let mut walked = first_y;
            for _ in 0..skipped {
                assert!(walked + 20.0 <= -12.0, "line at {walked} skipped from {first_y}");
                walked += 12.0;
            }
            let jumped = first_y + 12.0 * skipped as f32;
            assert_eq!(jumped.to_bits(), walked.to_bits(), "from {first_y}, {skipped} lines");
            skipped_some += usize::from(skipped > 0);
        }
        assert!(skipped_some > 18_000);
    }

    /// The whole roll of the game's credits, 7.3 units a frame from the top to past the end
    /// (last line stopped in the middle of the screen), drawn by reaching the visible lines
    /// directly against walking every line: layers equal to the byte at two interface scales,
    /// fonts replaced half-way (kept widths measured again). Without an installed game or a GPU:
    /// skipped (`RUSTCRAFT_OPT_REQUIRE=1`: failure).
    #[test]
    fn visible_lines_draw_the_layers_of_the_full_walk() {
        use crate::gui::tests_opt;
        let Some(e) = tests_opt::env() else { return };
        let json = e.assets.read("assets/minecraft/texts/credits.json").expect("credits of the game");
        for (scale, step) in [(3, 7.3f32), (1, 61.7)] {
            let mut paths = [false, true].map(|only_visible| (only_visible, CreditsRoll::new(&json), tests_opt::gui(&e, tests_opt::ALL)));
            let lines = paths[0].1.lines.len();
            assert!(lines > 1_000, "{lines} lines of credits");
            let end = (lines * 12) as f32 + 2.0 * 900.0 + 200.0;
            let frames = (end / step) as usize;
            let mut seen = hashbrown::HashSet::new();
            let mut finished_at = None;
            for frame in 0..frames {
                let mut layers = Vec::new();
                for (only_visible, roll, gui) in &mut paths {
                    if frame == frames / 2 {
                        let opts = rc_assets::font::FontOptions { uniform: true, jp: false };
                        let load = |id: &&str| ((*id).to_owned(), rc_assets::font::Font::load(&e.assets, id, opts));
                        gui.set_fonts(crate::resources::FONT_IDS.iter().map(load).collect());
                    }
                    gui.scale_option = scale;
                    gui.begin(1600, 900);
                    roll.scroll = frame as f32 * step;
                    roll.draw_scrolled(gui, &e.queue, 0, *only_visible);
                    layers.push(tests_opt::layer_bytes(gui));
                    if finished_at.is_none() && roll.finished(gui.height) {
                        finished_at = Some(frame);
                    }
                }
                let first = layers[0].iter().zip(&layers[1]).position(|(a, b)| a != b);
                assert!(
                    layers[0].len() == layers[1].len() && first.is_none(),
                    "scale {scale}, frame {frame}: layers differ (byte {first:?})"
                );
                seen.insert(layers.swap_remove(0));
            }
            assert!(seen.len() * 10 > frames * 9, "scale {scale}: {} distinct frames out of {frames}", seen.len());
            assert!(finished_at.is_some_and(|f| f > frames * 8 / 10), "scale {scale}: roll finished at {finished_at:?}");
        }
    }

    /// Micro-bench of a frame of the credits roll in the middle of the scroll (loaded machine
    /// possible): every line walked against the visible lines reached directly; time and bytes
    /// allocated per frame. Main thread by nature. Without a GPU: skipped.
    /// `cargo test --release -p rc-app bench_credits_roll -- --ignored --nocapture`
    #[test]
    #[ignore = "micro-bench"]
    fn bench_credits_roll() {
        use crate::gui::tests_opt;
        let Some(e) = tests_opt::env() else { return };
        let json = e.assets.read("assets/minecraft/texts/credits.json").expect("credits of the game");
        let mut gui = tests_opt::gui(&e, tests_opt::ALL);
        for pass in 0..2 {
            for (label, part) in [("start", 0.0f32), ("middle", 0.5), ("end", 0.98)] {
                for (name, only_visible) in [("every line walked", false), ("visible lines only", true)] {
                    let mut roll = CreditsRoll::new(&json);
                    roll.scroll = (roll.lines.len() * 12) as f32 * part;
                    let mut once = |gui: &mut Gui| {
                        gui.begin(1600, 900);
                        roll.draw_scrolled(gui, &e.queue, 0, only_visible);
                    };
                    (0..200).for_each(|_| once(&mut gui));
                    let frames = 10_000;
                    let t = std::time::Instant::now();
                    (0..frames).for_each(|_| once(&mut gui));
                    let per_frame = t.elapsed().as_secs_f64() * 1e6 / f64::from(frames);
                    let bytes = crate::debug_sys::allocated_bytes(|| (0..100).for_each(|_| once(&mut gui))) / 100;
                    println!(
                        "credits roll, pass {pass}, {label:<6} {name:<18}: {per_frame:>6.2} µs, {bytes:>5} bytes allocated per frame"
                    );
                }
            }
        }
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
