//! Advanced HUD (`docs/specs/hud-advanced.md`): boss bars, titles, status effects of the local
//! player.

use std::time::Instant;

use crate::gui::{BOSS_COLORS, BOSS_NOTCHES, Gui, Pre};

struct BossBar {
    id: u128,
    name: Option<rc_proto::nbt::Nbt>,
    /// Progress: start and target of the animation, timestamp.
    from: f32,
    to: f32,
    since: Instant,
    color: usize,
    style: usize,
    flags: u8,
}

impl BossBar {
    fn progress(&self) -> f32 {
        let a = (self.since.elapsed().as_secs_f32() * 10.0).clamp(0.0, 1.0);
        self.from + a * (self.to - self.from)
    }
}

/// Status effect of the local player.
pub struct Effect {
    pub id: i32,
    /// Amplifier (level displayed next to the inventory, `inv_effects.rs`).
    pub amplifier: i32,
    /// Ticks left (−1: infinite).
    pub duration: i32,
    pub flags: i8,
}

/// Categories (§ 2.4): true = beneficial (top row).
const BENEFICIAL: [bool; 40] = [
    true, false, true, false, true, true, false, true, false, true, true, true, true, true, false, true, false, false,
    false, false, true, true, true, false, false, true, false, true, true, true, false, true, false, false, false,
    false, false, false, false, true,
];

#[derive(Default)]
pub struct HudExtra {
    bars: Vec<BossBar>,
    title: Option<rc_proto::nbt::Nbt>,
    subtitle: Option<rc_proto::nbt::Nbt>,
    timer: i32,
    times: [i32; 3],
    pub effects: Vec<Effect>,
    /// Effect names (`mob_effect` registry), for the sprites.
    pub effect_names: Vec<String>,
    /// Effect colors (tie-break of the icon order).
    pub effect_colors: Vec<u32>,
}

impl HudExtra {
    pub fn new() -> Self {
        HudExtra { times: [10, 70, 20], ..Default::default() }
    }

    /// `boss_event` packet (§ 1.2); operation on an unknown id: ignored.
    pub fn boss_event(&mut self, ev: rc_proto_v777::play::BossEvent) {
        use rc_proto_v777::play::BossOp;
        let pos = self.bars.iter().position(|b| b.id == ev.id);
        match (ev.op, pos) {
            (BossOp::Add { name, progress, color, style, flags }, p) => {
                let bar = BossBar {
                    id: ev.id,
                    name,
                    from: progress,
                    to: progress,
                    since: Instant::now(),
                    color: color.clamp(0, 6) as usize,
                    style: style.clamp(0, 4) as usize,
                    flags,
                };
                match p {
                    Some(i) => self.bars[i] = bar,
                    None => self.bars.push(bar),
                }
            }
            (BossOp::Remove, Some(i)) => {
                self.bars.remove(i);
            }
            (BossOp::Progress(v), Some(i)) => {
                let b = &mut self.bars[i];
                b.from = b.progress();
                b.to = v;
                b.since = Instant::now();
            }
            (BossOp::Name(n), Some(i)) => self.bars[i].name = n,
            (BossOp::Style { color, style }, Some(i)) => {
                self.bars[i].color = color.clamp(0, 6) as usize;
                self.bars[i].style = style.clamp(0, 4) as usize;
            }
            (BossOp::Flags(f), Some(i)) => self.bars[i].flags = f,
            _ => {}
        }
    }

    /// Combined flags of all the bars (1 dark sky, 2 music, 4 fog); to
    /// be wired to the environment (darkening, fog) and the music.
    pub fn boss_flags(&self) -> u8 {
        self.bars.iter().fold(0, |a, b| a | b.flags)
    }

    pub fn set_title(&mut self, t: Option<rc_proto::nbt::Nbt>) {
        self.title = t;
        self.timer = self.times.iter().sum();
    }

    pub fn set_subtitle(&mut self, t: Option<rc_proto::nbt::Nbt>) {
        self.subtitle = t;
    }

    pub fn set_times(&mut self, t: [i32; 3]) {
        for (cur, new) in self.times.iter_mut().zip(t) {
            if new >= 0 {
                *cur = new;
            }
        }
        if self.timer > 0 {
            self.timer = self.times.iter().sum();
        }
    }

    pub fn clear_titles(&mut self, reset: bool) {
        self.title = None;
        self.subtitle = None;
        self.timer = 0;
        if reset {
            self.times = [10, 70, 20];
        }
    }

    /// Disconnection or new connection: everything is cleared.
    pub fn reset(&mut self) {
        *self = HudExtra {
            effect_names: std::mem::take(&mut self.effect_names),
            effect_colors: std::mem::take(&mut self.effect_colors),
            ..HudExtra::new()
        };
    }

    /// Active named effect (`minecraft:poison`…).
    pub fn has_effect(&self, name: &str) -> bool {
        self.effects
            .iter()
            .any(|e| usize::try_from(e.id).ok().and_then(|i| self.effect_names.get(i)).is_some_and(|n| n == name))
    }

    pub fn effect(&mut self, id: i32, amplifier: i32, duration: i32, flags: i8) {
        let e = Effect { id, amplifier, duration, flags };
        match self.effects.iter_mut().find(|x| x.id == id) {
            Some(x) => *x = e,
            None => self.effects.push(e),
        }
    }

    pub fn remove_effect(&mut self, id: i32) {
        self.effects.retain(|e| e.id != id);
    }

    /// Tick: title timer, effect durations (never removed by the client).
    pub fn tick(&mut self) {
        if self.timer > 0 {
            self.timer -= 1;
            if self.timer == 0 {
                self.title = None;
                self.subtitle = None;
            }
        }
        for e in &mut self.effects {
            if e.duration > 0 {
                e.duration -= 1;
            }
        }
    }

    /// Effects at the top right (`hud.md` § 5.4), in the hotbar's stratum.
    pub fn draw_effects(&self, gui: &mut Gui) {
        let mut list: Vec<&Effect> = self.effects.iter().filter(|e| e.flags & 4 != 0).collect();
        // Natural order reversed: ambient, infinite, decreasing durations, then color.
        let color = |e: &Effect| usize::try_from(e.id).ok().and_then(|i| self.effect_colors.get(i)).copied();
        list.sort_by(|a, b| effect_order(b, a, color(b), color(a)));
        let (mut good, mut bad) = (0, 0);
        for e in list {
            let beneficial = usize::try_from(e.id).ok().and_then(|i| BENEFICIAL.get(i)).copied().unwrap_or(false);
            let (n, y) = if beneficial {
                good += 1;
                (good, 1.0)
            } else {
                bad += 1;
                (bad, 27.0)
            };
            let x = gui.width.floor() - 25.0 * n as f32;
            let ambient = e.flags & 1 != 0;
            gui.sprite(
                if ambient { "hud/effect_background_ambient" } else { "hud/effect_background" },
                x,
                y,
                24.0,
                24.0,
                0xffff_ffff,
            );
            let mut a = 1.0f32;
            if !ambient && e.duration >= 0 && e.duration <= 200 {
                let d = e.duration as f32;
                let u = 10.0 - d / 20.0;
                a = ((d / 10.0 / 5.0 * 0.5).clamp(0.0, 0.5)
                    + (d * std::f32::consts::PI / 5.0).cos() * (u / 10.0 * 0.25).clamp(0.0, 0.25))
                .clamp(0.0, 1.0);
            }
            if let Some(name) = usize::try_from(e.id).ok().and_then(|i| self.effect_names.get(i)) {
                let alpha = (a * 255.0) as u32;
                let path = format!("mob_effect/{}", name.strip_prefix("minecraft:").unwrap_or(name));
                gui.sprite(&path, x + 3.0, y + 3.0, 18.0, 18.0, alpha << 24 | 0xff_ffff);
            }
        }
    }

    /// Boss bars (§ 1.4), new stratum.
    pub fn draw_bosses(&self, gui: &mut Gui, queue: &wgpu::Queue) {
        if self.bars.is_empty() {
            return;
        }
        gui.next_layer();
        let (l, h) = (gui.width.floor() as i32, gui.height.floor() as i32);
        let mut y = 12;
        for b in &self.bars {
            let x = (l / 2 - 91) as f32;
            // Sprites `boss_bar/<color>_background` / `_progress` and those of the notches, resolved once.
            let c = b.color.min(BOSS_COLORS.len() - 1);
            let notched = b.style > 0 && b.style < BOSS_NOTCHES.len();
            gui.sprite_pre(Pre::boss_color(c, false), x, y as f32, 182.0, 5.0, 0xffff_ffff);
            if notched {
                gui.sprite_pre(Pre::boss_notches(b.style, false), x, y as f32, 182.0, 5.0, 0xffff_ffff);
            }
            let p = b.progress();
            let w = ((p * 181.0).floor() as i32 + i32::from(p > 0.0)).clamp(0, 182);
            if w > 0 {
                let wf = w as f32;
                gui.sprite_region_pre(Pre::boss_color(c, true), 182.0, 5.0, 0.0, 0.0, x, y as f32, wf, 5.0);
                if notched {
                    gui.sprite_region_pre(Pre::boss_notches(b.style, true), 182.0, 5.0, 0.0, 0.0, x, y as f32, wf, 5.0);
                }
            }
            if let Some(c) = b.name.as_ref().and_then(rc_text::from_nbt) {
                let chars = rc_text::styled_chars(&rc_text::flatten(&c, &gui.lang));
                let width = rc_text::width(gui, &chars);
                gui.text(queue, &chars, (l / 2 - width / 2) as f32, (y - 9) as f32, 0xffff_ffff, true, 0);
            }
            y += 19;
            if y >= h / 3 {
                break;
            }
        }
    }

    /// Title and subtitle (§ 3.4), new stratum.
    /// `background`: text background (alpha only, 0 if reserved for the chat).
    pub fn draw_title(&self, gui: &mut Gui, queue: &wgpu::Queue, pt: f32, background: u32) {
        let Some(title) = self.title.as_ref().and_then(rc_text::from_nbt) else { return };
        if self.timer <= 0 {
            return;
        }
        let [a, m, d] = self.times.map(|v| v as f32);
        let f = self.timer as f32 - pt;
        let mut alpha = 255.0;
        if self.timer as f32 > d + m {
            alpha = ((a + m + d - f) * 255.0 / a).trunc();
        }
        if self.timer as f32 <= d {
            alpha = (f * 255.0 / d).trunc();
        }
        let alpha = if alpha.is_nan() { 0 } else { alpha.clamp(0.0, 255.0) as u32 };
        if alpha == 0 {
            return;
        }
        gui.next_layer();
        let color = alpha << 24 | 0xff_ffff;
        let (l, h) = (gui.width.floor() as i32, gui.height.floor() as i32);
        let pose = gui.pose;
        let line = |gui: &mut Gui, c: &rc_text::Component, k: f32, y: f32| {
            let chars = rc_text::styled_chars(&rc_text::flatten(c, &gui.lang));
            let w = rc_text::width(gui, &chars);
            gui.pose = pose * k;
            // Position on screen divided by the scale of the enlarged frame.
            let x = ((l / 2) as f32 - k * (w / 2) as f32) / k;
            crate::hud::text_with_backdrop(gui, queue, &chars, x, ((h / 2) as f32 + y) / k, w, color, background);
        };
        line(gui, &title, 4.0, -40.0);
        if let Some(sub) = self.subtitle.as_ref().and_then(rc_text::from_nbt) {
            line(gui, &sub, 2.0, 10.0);
        }
        gui.pose = pose;
    }
}

/// Natural order of two effects (`hud-advanced.md` § 2.4): if either duration ≤ 32147 and
/// they are not both ambient, non-ambient, finite, increasing duration then color;
/// otherwise ambient then color only.
fn effect_order(a: &Effect, b: &Effect, ca: Option<u32>, cb: Option<u32>) -> std::cmp::Ordering {
    const CUT: i32 = 32147;
    let (aa, ab) = (a.flags & 1 != 0, b.flags & 1 != 0);
    let color = ca.unwrap_or(0).cmp(&cb.unwrap_or(0));
    if (a.duration <= CUT || b.duration <= CUT) && !(aa && ab) {
        aa.cmp(&ab).then((a.duration == -1).cmp(&(b.duration == -1))).then(a.duration.cmp(&b.duration)).then(color)
    } else {
        aa.cmp(&ab).then(color)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fx(id: i32, duration: i32, flags: i8) -> Effect {
        Effect { id, amplifier: 0, duration, flags }
    }

    #[test]
    fn equal_durations_break_ties_by_color() {
        let (a, b) = (fx(0, 100, 4), fx(1, 100, 4));
        assert_eq!(effect_order(&a, &b, Some(0x10), Some(0x20)), std::cmp::Ordering::Less);
        // Durations both beyond the threshold: only the color counts.
        let (a, b) = (fx(0, 40_000, 4), fx(1, 50_000, 4));
        assert_eq!(effect_order(&a, &b, Some(0x30), Some(0x20)), std::cmp::Ordering::Greater);
        // Infinite after the finite durations.
        let (a, b) = (fx(0, -1, 4), fx(1, 20, 4));
        assert_eq!(effect_order(&a, &b, None, None), std::cmp::Ordering::Greater);
    }

    #[test]
    fn has_effect_by_registry_name() {
        let mut h = HudExtra::new();
        h.effect_names = vec!["minecraft:speed".into(), "minecraft:poison".into()];
        h.effect(1, 0, 100, 4);
        assert!(h.has_effect("minecraft:poison"));
        assert!(!h.has_effect("minecraft:speed"));
    }
}
