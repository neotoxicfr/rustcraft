//! Subtitles (`docs/specs/sounds.md` § 7, `miscellaneous.md` § 3): list fed by the
//! audio engine when the `showSubtitles` option is on, displayed at the bottom right with direction
//! arrows and fade.

use std::sync::Arc;
use std::time::Instant;

use glam::Vec3;
use rc_audio::SubtitleEvent;
use rc_text::{Lang, Metrics, StyledChar};

use crate::gui::Gui;

/// One instance: position, instant, range.
struct Instance {
    pos: [f64; 3],
    at: Instant,
    range: f32,
}

struct Line {
    key: Arc<str>,
    repeated: bool,
    instances: Vec<Instance>,
    /// Drawn text and width kept between frames (`RUSTCRAFT_OPT_SUBTITLE_INTERNED`).
    text: Option<LineText>,
}

/// Font set a width was measured with: generation of the GUI fonts and the "fishy glyphs"
/// filter of the chat (off outside the chat drawing).
type FontKey = (usize, bool);

/// Text of a line as last drawn, with what it was computed from: the translation template (a
/// translation without arguments depends on nothing else) and the font set.
struct LineText {
    template: String,
    fonts: FontKey,
    chars: Vec<StyledChar>,
    width: i32,
}

#[derive(Default)]
pub struct Subtitles {
    lines: Vec<Line>,
    /// Widths of "<", ">" and " " and the font set they were measured with.
    marks: Option<(FontKey, [i32; 3])>,
}

/// Text of a subtitle line: its key translated (no arguments), then the § codes.
fn line_chars(key: &str, lang: &Lang) -> Vec<StyledChar> {
    let c = rc_text::Component::translate(key, vec![]);
    rc_text::styled_chars(&rc_text::flatten(&c, lang))
}

impl Line {
    /// Keeps the drawn text and width up to date; recomputed only when the translation of the
    /// key or the fonts changed (language or font options), not at every frame.
    fn refresh_text(&mut self, lang: &Lang, metrics: &dyn Metrics, fonts: FontKey) {
        let template = lang.map.get(&*self.key).map_or(&*self.key, String::as_str);
        if self.text.as_ref().is_some_and(|t| t.fonts == fonts && t.template == template) {
            return;
        }
        let chars = line_chars(&self.key, lang);
        let width = rc_text::width(metrics, &chars);
        self.text = Some(LineText { template: template.to_owned(), fonts, chars, width });
    }
}

/// Display parameters read from the options.
pub struct Style {
    /// Display duration of the notifications (factor).
    pub duration: f64,
    /// Background color (ARGB).
    pub background: u32,
}

impl Subtitles {
    /// Sound started: same text refreshed (instance with the same position replaced, original loop
    /// flag kept), otherwise a new line at the end.
    pub fn notify(&mut self, e: SubtitleEvent, now: Instant) {
        let inst = Instance { pos: e.pos, at: now, range: e.range };
        // Keys shared by the sound table: the same pointer is the same key (content compared
        // otherwise, after a resource reload for instance).
        let shared = rc_audio::subtitle_interned_enabled();
        match self.lines.iter_mut().find(|l| (shared && Arc::ptr_eq(&l.key, &e.key)) || l.key == e.key) {
            Some(l) => {
                l.instances.retain(|i| i.pos != e.pos);
                l.instances.push(inst);
            }
            None => self.lines.push(Line { key: e.key, repeated: e.repeated, instances: vec![inst], text: None }),
        }
    }

    /// Lines visible from `cam` with their best instance (the only one, otherwise the most recent
    /// then the farthest strictly within range; the first one on a tie). Expired
    /// instances are purged; a line is never removed and keeps its rank of first
    /// appearance.
    /// Returned as (line, instance) indices.
    fn visible(&mut self, now: Instant, cam: [f64; 3], life: f64) -> Vec<(usize, usize)> {
        for l in &mut self.lines {
            l.instances.retain(|i| (now - i.at).as_millis() as f64 <= life);
        }
        let d2 = |p: [f64; 3]| (p[0] - cam[0]).powi(2) + (p[1] - cam[1]).powi(2) + (p[2] - cam[2]).powi(2);
        let near = |i: &Instance| d2(i.pos) < f64::from(i.range) * f64::from(i.range);
        self.lines
            .iter()
            .enumerate()
            .filter_map(|(n, l)| {
                let (k, best) = if l.instances.len() == 1 {
                    (0, &l.instances[0])
                } else {
                    l.instances.iter().enumerate().filter(|(_, i)| near(i)).fold(
                        None,
                        |acc: Option<(usize, &Instance)>, (k, i)| match acc {
                            Some((ka, a)) if a.at.cmp(&i.at).then(d2(a.pos).total_cmp(&d2(i.pos))).is_ge() => {
                                Some((ka, a))
                            }
                            _ => Some((k, i)),
                        },
                    )?
                };
                (best.range.is_infinite() || near(best)).then_some((n, k))
            })
            .collect()
    }

    /// Widths of "<", ">" and " " (kept per font set with `SUBTITLE_INTERNED`).
    fn mark_widths(&mut self, gui: &Gui, kept: bool, fonts: FontKey) -> [i32; 3] {
        if kept && let Some((k, m)) = self.marks && k == fonts {
            return m;
        }
        let glyph = |s: &str| rc_text::width(gui, &crate::gui::plain(s));
        let m = [glyph("<"), glyph(">"), glyph(" ")];
        if kept {
            self.marks = Some((fonts, m));
        }
        m
    }

    /// Drawing (last element of the HUD, separate layer). `cam`: camera position.
    pub fn draw(&mut self, gui: &mut Gui, queue: &wgpu::Queue, cam: [f64; 3], yaw: f32, pitch: f32, style: &Style) {
        let now = Instant::now();
        let visible = self.visible(now, cam, 3000.0 * style.duration);
        if visible.is_empty() {
            return;
        }
        gui.next_layer();
        let kept = rc_audio::subtitle_interned_enabled();
        let fonts = (gui.font_generation(), gui.filter_fishy.get());
        let [lt, gt, sp] = self.mark_widths(gui, kept, fonts);
        // Old path: translated, flattened and measured at every frame.
        let fresh: Vec<(Vec<StyledChar>, i32)> = if kept {
            for &(l, _) in &visible {
                self.lines[l].refresh_text(&gui.lang, &*gui, fonts);
            }
            Vec::new()
        } else {
            visible
                .iter()
                .map(|&(l, _)| {
                    let chars = line_chars(&self.lines[l].key, &gui.lang);
                    let width = rc_text::width(&*gui, &chars);
                    (chars, width)
                })
                .collect()
        };
        let texts: Vec<(&[StyledChar], i32)> = if kept {
            visible
                .iter()
                .map(|&(l, _)| self.lines[l].text.as_ref().map_or((&[][..], 0), |t| (&t.chars[..], t.width)))
                .collect()
        } else {
            fresh.iter().map(|(c, w)| (&c[..], *w)).collect()
        };
        let max = texts.iter().map(|t| t.1).max().unwrap_or(0);
        let w = (max + lt + sp + gt + sp) / 2;
        let h = 4;
        let (gw, gh) = (gui.width.floor() as i32, gui.height.floor() as i32);
        let forward = crate::camera::forward(yaw, pitch);
        let yr = yaw.to_radians();
        let right = Vec3::new(-yr.cos(), 0.0, -yr.sin());
        for (i, (&(l, k), &(text, tw))) in visible.iter().zip(&texts).enumerate() {
            let (line, inst) = (&self.lines[l], &self.lines[l].instances[k]);
            let ox = (gw - w - 2) as f32;
            let oy = (gh - 35 - i as i32 * 10) as f32;
            gui.fill(
                ox - w as f32 - 1.0,
                oy - h as f32 - 1.0,
                ox + w as f32 + 1.0,
                oy + h as f32 + 1.0,
                style.background,
            );
            let p = if line.repeated { 500.0 } else { 0.0 };
            let age = (now - inst.at).as_secs_f64() * 1000.0;
            let t = ((age - p).max(0.0) / ((3000.0 - p) * style.duration)).clamp(0.0, 1.0);
            let l = (255.0 + t * (75.0 - 255.0)).floor() as u32;
            let color = 0xff00_0000 | l << 16 | l << 8 | l;
            let u =
                Vec3::new((inst.pos[0] - cam[0]) as f32, (inst.pos[1] - cam[1]) as f32, (inst.pos[2] - cam[2]) as f32)
                    .normalize_or_zero();
            if forward.dot(u) <= 0.5 {
                let r = right.dot(u);
                if r > 0.0 {
                    gui.text(queue, &crate::gui::plain(">"), ox + (w - gt) as f32, oy - h as f32, color, true, 0);
                } else if r < 0.0 {
                    gui.text(queue, &crate::gui::plain("<"), ox - w as f32, oy - h as f32, color, true, 0);
                }
            }
            gui.text(queue, text, ox - (tw / 2) as f32, oy - h as f32, color, true, 0);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn ev(key: &str, repeated: bool) -> SubtitleEvent {
        SubtitleEvent { key: key.into(), pos: [1.0, 0.0, 0.0], range: 16.0, repeated }
    }

    #[test]
    fn lines_keep_first_appearance_rank_and_loop_flag() {
        let t0 = Instant::now();
        let mut s = Subtitles::default();
        s.notify(ev("a", true), t0);
        s.notify(ev("b", false), t0);
        // "a" expires, then comes back: it stays first.
        let later = t0 + Duration::from_secs(4);
        assert_eq!(s.visible(later, [0.0; 3], 3000.0).len(), 0);
        s.notify(ev("b", false), later);
        s.notify(ev("a", false), later);
        let v = s.visible(later, [0.0; 3], 3000.0);
        let keys: Vec<&str> = v.iter().map(|&(l, _)| &*s.lines[l].key).collect();
        assert_eq!(keys, ["a", "b"]);
        assert!(s.lines[v[0].0].repeated, "loop flag of the first appearance kept");
    }

    /// Advances that depend on the character and on a font set number (a font change moves
    /// every width).
    struct Fonts(u32);
    impl Metrics for Fonts {
        fn advance(&self, _: &str, c: char) -> (f32, f32) {
            ((u32::from(c) % 7 + self.0) as f32, 1.0)
        }
    }

    /// The game's language files, `None` (test skipped with a message) without a 26.3 install.
    fn languages() -> Option<(Lang, Lang)> {
        let install = rc_testdata::need(rc_data::locate("26.3").ok(), "installed 26.3 game")?;
        let index = rc_testdata::need(install.asset_index(), "26.3 asset index (languages)")?;
        let assets = rc_testdata::need(
            rc_assets::Assets::for_install(&install.client_jar, &[], Some((index.0.as_path(), index.1.as_path()))).ok(),
            "26.3 game resources",
        )?;
        let read = |p: &str| assets.read(p);
        let deprecated = read("assets/minecraft/lang/deprecated.json");
        let en = read("assets/minecraft/lang/en_us.json")?;
        let fr = read("assets/minecraft/lang/fr_fr.json")?;
        Some((Lang::load(&en, None, deprecated.as_deref()), Lang::load(&en, Some(&fr), deprecated.as_deref())))
    }

    /// Kept text and width equal to the ones computed at every frame, for every key of `en_us`
    /// (and a missing one), after a language change and after a font change; nothing recomputed
    /// while nothing changed.
    #[test]
    fn kept_line_text_matches_fresh_text() {
        let Some((en, fr)) = languages() else { return };
        let mut keys: Vec<&String> = en.map.keys().collect();
        keys.sort_unstable();
        let mut lines: Vec<Line> = keys
            .iter()
            .map(|k| Line { key: Arc::from(k.as_str()), repeated: false, instances: Vec::new(), text: None })
            .chain([Line { key: "subtitles.nowhere".into(), repeated: false, instances: Vec::new(), text: None }])
            .collect();
        let (base, other) = (Fonts(0), Fonts(3));
        let mut changed = 0;
        for (lang, metrics, fonts) in [(&en, &base, (0, false)), (&fr, &base, (0, false)), (&fr, &other, (1, false))] {
            for line in &mut lines {
                let before = line.text.as_ref().map(|t| (t.chars.clone(), t.width));
                line.refresh_text(lang, metrics, fonts);
                let chars = line_chars(&line.key, lang);
                let width = rc_text::width(metrics, &chars);
                let kept = line.text.as_ref().expect("text kept");
                assert_eq!((&kept.chars, kept.width), (&chars, width), "{}", line.key);
                changed += usize::from(before.is_some_and(|b| b != (chars, width)));
                let at = kept.chars.as_ptr();
                line.refresh_text(lang, metrics, fonts);
                assert_eq!(line.text.as_ref().map(|t| t.chars.as_ptr()), Some(at), "recomputed without a change");
            }
        }
        // The language and the font changes did change texts: the proof is not empty.
        assert!(changed > keys.len(), "{changed} lines changed");
    }

    #[test]
    fn range_is_strict() {
        let mut s = Subtitles::default();
        let t0 = Instant::now();
        s.notify(SubtitleEvent { key: "x".into(), pos: [16.0, 0.0, 0.0], range: 16.0, repeated: false }, t0);
        assert!(s.visible(t0, [0.0; 3], 3000.0).is_empty());
    }
}
