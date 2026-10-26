//! Subtitles (`docs/specs/sounds.md` § 7, `miscellaneous.md` § 3): list fed by the
//! audio engine when the `showSubtitles` option is on, displayed at the bottom right with direction
//! arrows and fade.

use std::time::Instant;

use glam::Vec3;
use rc_audio::SubtitleEvent;

use crate::gui::Gui;

/// One instance: position, instant, range.
struct Instance {
    pos: [f64; 3],
    at: Instant,
    range: f32,
}

struct Line {
    key: String,
    repeated: bool,
    instances: Vec<Instance>,
}

#[derive(Default)]
pub struct Subtitles {
    lines: Vec<Line>,
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
        match self.lines.iter_mut().find(|l| l.key == e.key) {
            Some(l) => {
                l.instances.retain(|i| i.pos != e.pos);
                l.instances.push(inst);
            }
            None => self.lines.push(Line { key: e.key, repeated: e.repeated, instances: vec![inst] }),
        }
    }

    /// Lines visible from `cam` with their best instance (the only one, otherwise the most recent
    /// then the farthest strictly within range; the first one on a tie). Expired
    /// instances are purged; a line is never removed and keeps its rank of first
    /// appearance.
    fn visible(&mut self, now: Instant, cam: [f64; 3], life: f64) -> Vec<(&Line, &Instance)> {
        for l in &mut self.lines {
            l.instances.retain(|i| (now - i.at).as_millis() as f64 <= life);
        }
        let d2 = |p: [f64; 3]| (p[0] - cam[0]).powi(2) + (p[1] - cam[1]).powi(2) + (p[2] - cam[2]).powi(2);
        let near = |i: &Instance| d2(i.pos) < f64::from(i.range) * f64::from(i.range);
        self.lines
            .iter()
            .filter_map(|l| {
                let best = if l.instances.len() == 1 {
                    &l.instances[0]
                } else {
                    l.instances.iter().filter(|i| near(i)).fold(None, |acc: Option<&Instance>, i| match acc {
                        Some(a) if a.at.cmp(&i.at).then(d2(a.pos).total_cmp(&d2(i.pos))).is_ge() => Some(a),
                        _ => Some(i),
                    })?
                };
                (best.range.is_infinite() || near(best)).then_some((l, best))
            })
            .collect()
    }

    /// Drawing (last element of the HUD, separate layer). `cam`: camera position.
    pub fn draw(&mut self, gui: &mut Gui, queue: &wgpu::Queue, cam: [f64; 3], yaw: f32, pitch: f32, style: &Style) {
        let now = Instant::now();
        let visible = self.visible(now, cam, 3000.0 * style.duration);
        if visible.is_empty() {
            return;
        }
        gui.next_layer();
        let texts: Vec<_> = visible
            .iter()
            .map(|(l, _)| {
                let c = rc_text::Component::translate(l.key.clone(), vec![]);
                rc_text::styled_chars(&rc_text::flatten(&c, &gui.lang))
            })
            .collect();
        let glyph = |gui: &Gui, s: &str| rc_text::width(gui, &crate::gui::plain(s));
        let (lt, gt, sp) = (glyph(gui, "<"), glyph(gui, ">"), glyph(gui, " "));
        let max = texts.iter().map(|t| rc_text::width(gui, t)).max().unwrap_or(0);
        let w = (max + lt + sp + gt + sp) / 2;
        let h = 4;
        let (gw, gh) = (gui.width.floor() as i32, gui.height.floor() as i32);
        let forward = crate::camera::forward(yaw, pitch);
        let yr = yaw.to_radians();
        let right = Vec3::new(-yr.cos(), 0.0, -yr.sin());
        for (i, ((line, inst), text)) in visible.iter().zip(&texts).enumerate() {
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
            let tw = rc_text::width(gui, text);
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
        let keys: Vec<&str> = v.iter().map(|(l, _)| l.key.as_str()).collect();
        assert_eq!(keys, ["a", "b"]);
        assert!(v[0].0.repeated, "loop flag of the first appearance kept");
    }

    #[test]
    fn range_is_strict() {
        let mut s = Subtitles::default();
        let t0 = Instant::now();
        s.notify(SubtitleEvent { key: "x".into(), pos: [16.0, 0.0, 0.0], range: 16.0, repeated: false }, t0);
        assert!(s.visible(t0, [0.0; 3], 3000.0).is_empty());
    }
}
