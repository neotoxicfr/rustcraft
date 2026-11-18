//! Notifications ("toasts", `docs/specs/creative-inventory.md` § 13): generic manager
//! (5 slots of 32 at the top right, waiting queue, 600 ms slide, sounds) and the
//! "recipe unlocked" notification.
//!
//! Clock of a notification: the "fully visible" time only runs once the entry
//! slide is over; each notification decides its wanted visibility from it.

use std::collections::VecDeque;

use rc_proto_v777::item::ItemStack;
use rc_text::{Component, StyledChar};

use crate::advancements::Frame;
use crate::gui::{Blend, Gui};
use crate::hud::HotbarItems;
use crate::screens::{Cached, refresh};

const SLOTS: usize = 5;
const SLIDE_MS: i64 = 600;
const CHALLENGE: &str = "minecraft:ui.toast.challenge_complete";

/// Content of a notification.
pub enum Toast {
    /// Recipes unlocked: (station stack, result stack); additions since the last
    /// update, visible time at the last addition, recipe shown.
    Recipe { entries: Vec<(Option<ItemStack>, Option<ItemStack>)>, changed: bool, last_changed: i64, index: usize },
    /// Advancement made (`advancements-stats.md` § 3).
    Advancement { title: Box<Component>, frame: Frame, icon: ItemStack },
    /// System notification: title lines (yellow) and message lines (white) cut at 200 px,
    /// width, display duration (ms, before the multiplier), update id.
    System(Box<SystemToast>),
    /// Tutorial: icon, lines (purple title then description, cut at 126 px), optional progress
    /// bar (target, smoothed value, last smoothing), hidden.
    Tutorial(Box<TutorialToast>),
}

/// Id of an updatable system notification: name and number (one download batch per number,
/// like a new id per batch in the game).
pub type ToastId = (&'static str, u64);

pub struct SystemToast {
    title: Vec<Vec<StyledChar>>,
    message: Vec<Vec<StyledChar>>,
    width: i32,
    display_ms: f64,
    /// Id: name and number (`None`: never updated, always stacked).
    id: Option<ToastId>,
    changed: bool,
    last_changed: i64,
    force_hide: bool,
}

pub struct TutorialToast {
    pub id: u32,
    icon: &'static str,
    lines: Vec<Vec<StyledChar>>,
    progressable: bool,
    progress: f32,
    smoothed: f32,
    last_smooth: i64,
    hidden: bool,
    /// Display duration (ms, 0 = until removal): the bar follows the elapsed time, then the
    /// notification is removed (not multiplied by the notification duration, like the game).
    display_ms: i64,
}

impl Toast {
    fn width(&self) -> i32 {
        match self {
            Toast::System(s) => s.width,
            _ => 160,
        }
    }

    fn height(&self) -> i32 {
        match self {
            Toast::System(s) => 20 + (s.title.len() as i32 - 1).max(0) * 12 + (s.message.len() as i32).max(1) * 12,
            Toast::Tutorial(t) => 7 + (t.lines.len() as i32).max(2) * 11 + 3,
            _ => 32,
        }
    }

    /// Slots occupied (height rounded up to a multiple of 32).
    fn slots(&self) -> usize {
        (self.height() as usize).div_ceil(32)
    }

    /// Update with the fully visible time `fv` (ms); returns the wanted visibility.
    fn update(&mut self, fv: i64, mult: f64) -> bool {
        match self {
            Toast::Recipe { entries, changed, last_changed, index } => {
                if *changed {
                    *last_changed = fv;
                    *changed = false;
                }
                let n = entries.len();
                if n > 0 {
                    let per = (5000.0 * mult / n as f64).max(1.0);
                    *index = (fv as f64 / per % n as f64) as usize;
                }
                n > 0 && ((fv - *last_changed) as f64) < 5000.0 * mult
            }
            Toast::Advancement { .. } => (fv as f64) < 5000.0 * mult,
            Toast::System(s) => {
                if s.changed {
                    s.last_changed = fv;
                    s.changed = false;
                }
                !s.force_hide && ((fv - s.last_changed) as f64) < s.display_ms * mult
            }
            Toast::Tutorial(t) => {
                if t.display_ms > 0 {
                    t.progress = (fv as f32 / t.display_ms as f32).min(1.0);
                    t.smoothed = t.progress;
                    t.last_smooth = fv;
                    if fv > t.display_ms {
                        t.hidden = true;
                    }
                } else if t.progressable {
                    let k = ((fv - t.last_smooth) as f32 / 100.0).clamp(0.0, 1.0);
                    t.smoothed += (t.progress - t.smoothed) * k;
                    t.last_smooth = fv;
                }
                !t.hidden
            }
        }
    }
}

struct Instance {
    toast: Toast,
    slot: usize,
    /// Slots reserved for placement.
    count: usize,
    /// Start of the animation (−1: not updated yet), instant when it became fully
    /// visible, visibility (true: shown), fully visible time, visible portion.
    anim_start: i64,
    became_visible: i64,
    shown: bool,
    fully_visible_for: i64,
    portion: f32,
    finished: bool,
    cache: Vec<Option<Cached>>,
}

impl Instance {
    fn new(toast: Toast, slot: usize, count: usize) -> Self {
        Instance {
            toast,
            slot,
            count,
            anim_start: -1,
            became_visible: -1,
            shown: false,
            fully_visible_for: 0,
            portion: 0.0,
            finished: false,
            cache: Vec::new(),
        }
    }

    /// Update at instant `now` (whole ms): clock, portion, wanted visibility, end.
    fn update(&mut self, now: i64, mult: f64) {
        if self.anim_start == -1 {
            self.anim_start = now;
            self.shown = true;
        }
        if self.shown && now - self.anim_start <= SLIDE_MS {
            self.became_visible = now;
        }
        self.fully_visible_for = now - self.became_visible;
        let x = ((now - self.anim_start) as f32 / SLIDE_MS as f32).clamp(0.0, 1.0);
        let x = x * x;
        self.portion = if self.shown { x } else { 1.0 - x };
        let want = self.toast.update(self.fully_visible_for, mult);
        if want != self.shown {
            self.anim_start = now - ((1.0 - self.portion) * SLIDE_MS as f32) as i64;
            self.shown = want;
        }
        self.finished = !self.shown && now - self.anim_start > SLIDE_MS;
    }
}

/// Periodic reminder (`regional_compliancies.json`): delay and period in minutes, title and
/// message keys (argument: number of elapsed periods).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notice {
    delay: i64,
    period: i64,
    title: String,
    message: String,
}

/// Periodic reminders for the user's country: a timer started when the resources load,
/// which rings at the smallest delay then at every GCD of the periods; at each ring,
/// the first reminder whose period has just ended is shown (5 s, system notification).
#[derive(Debug, Default)]
pub struct Periodic {
    list: Vec<Notice>,
    /// Timer step (min); minute reached by the next ring.
    step: i64,
    minute: i64,
    /// Time (ms) of the next ring.
    next_ms: f64,
}

fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 { a.abs() } else { gcd(b, a % b) }
}

impl Periodic {
    /// Reminders of country `country` (three-letter code) read from `json`, timer started at
    /// `now_ms`; empty (never a reminder) with no reminder, or if a period is zero.
    pub fn new(json: &[u8], country: Option<&str>, now_ms: f64) -> Periodic {
        let v: serde_json::Value = serde_json::from_slice(json).unwrap_or_default();
        let list: Vec<Notice> = v
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(k, _)| Some(k.as_str()) == country)
            .flat_map(|(_, l)| l.as_array().cloned().unwrap_or_default())
            .filter_map(|n| {
                Some(Notice {
                    delay: n.get("delay").map_or(Some(0), serde_json::Value::as_i64)?,
                    period: n.get("period")?.as_i64()?,
                    title: n.get("title")?.as_str()?.to_owned(),
                    message: n.get("message")?.as_str()?.to_owned(),
                })
            })
            .collect();
        if list.is_empty() || list.iter().any(|n| n.period == 0) {
            return Periodic::default();
        }
        let delay = list.iter().map(|n| n.delay).min().unwrap_or(0);
        let step = list.iter().map(|n| gcd(n.delay - delay, n.period)).reduce(gcd).unwrap_or(1).max(1);
        Periodic { list, step, minute: delay, next_ms: now_ms + delay as f64 * 60_000.0 }
    }

    /// Ring due at `now_ms`: returns the reminder to show (title, message, elapsed periods).
    pub fn poll(&mut self, now_ms: f64) -> Option<(String, String, i64)> {
        if self.list.is_empty() || now_ms < self.next_ms {
            return None;
        }
        self.next_ms += self.step as f64 * 60_000.0;
        let (cur, next) = (self.minute, self.minute + self.step);
        self.minute = next;
        self.list
            .iter()
            .find(|n| cur >= n.delay && cur / n.period != next / n.period)
            .map(|n| (n.title.clone(), n.message.clone(), cur / n.period))
    }
}

#[derive(Default)]
pub struct Toasts {
    active: Vec<Instance>,
    queue: VecDeque<Toast>,
    /// Sounds to play (`ui.toast.in` / `ui.toast.out`).
    pub sounds: Vec<&'static str>,
    /// Periodic reminders of the country (`regional_compliancies.json`).
    pub periodic: Periodic,
}

impl Toasts {
    /// Periodic reminder due at `now` (ms): 5 s system notification.
    pub fn periodic_tick(&mut self, gui: &Gui, now: f64) {
        if let Some((title, message, n)) = self.periodic.poll(now) {
            let arg = || vec![rc_text::Arg::Text(n.to_string())];
            self.system(gui, &Component::translate(title, arg()), Some(&Component::translate(message, arg())), 5000.0);
        }
    }

    /// Recipe unlocked: added to the existing notification (visible or queued), otherwise a
    /// new one (§ 13.2). The waiting time in the queue does not count.
    pub fn recipe(&mut self, station: Option<ItemStack>, result: Option<ItemStack>, _now: f64) {
        let add = |t: &mut Toast| {
            let Toast::Recipe { entries, changed, .. } = t else { return };
            entries.push((station.clone(), result.clone()));
            *changed = true;
        };
        if let Some(i) = self.active.iter_mut().find(|i| matches!(i.toast, Toast::Recipe { .. })) {
            add(&mut i.toast);
            return;
        }
        if let Some(t) = self.queue.iter_mut().find(|t| matches!(t, Toast::Recipe { .. })) {
            add(t);
            return;
        }
        let entries = vec![(station, result)];
        self.queue.push_back(Toast::Recipe { entries, changed: true, last_changed: 0, index: 0 });
    }

    /// Advancement made: one notification per advancement, queued (§ 3.2).
    pub fn advancement(&mut self, title: Component, frame: Frame, icon: ItemStack) {
        self.queue.push_back(Toast::Advancement { title: Box::new(title), frame, icon });
    }

    /// System notification (`SystemToast`): title, optional message, duration (5 s, 10 s for
    /// the insecure server warning), always stacked.
    pub fn system(&mut self, gui: &Gui, title: &Component, message: Option<&Component>, display_ms: f64) {
        self.system_keyed(gui, None, title, message, display_ms);
    }

    /// System notification with id `id`: a notification with the same id (displayed
    /// or queued) is updated (texts, width, timer restarted) instead of stacking one.
    pub fn system_keyed(
        &mut self,
        gui: &Gui,
        id: Option<ToastId>,
        title: &Component,
        message: Option<&Component>,
        display_ms: f64,
    ) {
        let split = |c: &Component| rc_text::wrap(gui, &rc_text::styled_chars(&rc_text::flatten(c, &gui.lang)), 200.0);
        let title = split(title);
        let message = message.map(split).unwrap_or_default();
        let widest = title.iter().chain(&message).map(|l| rc_text::width(gui, l)).max().unwrap_or(200);
        let width = widest.max(160) + 30;
        if let Some(id) = id {
            let active = self.active.iter_mut().map(|i| &mut i.toast);
            let found = active.chain(self.queue.iter_mut()).find_map(|t| match t {
                Toast::System(s) if s.id == Some(id) => Some(s),
                _ => None,
            });
            if let Some(s) = found {
                s.title = title;
                s.message = message;
                s.width = width;
                s.changed = true;
                return;
            }
        }
        let s =
            SystemToast { title, message, width, display_ms, id, changed: false, last_changed: 0, force_hide: false };
        self.queue.push_back(Toast::System(Box::new(s)));
    }

    /// System notification `id` hidden right away (exit slide), displayed or queued;
    /// false if none exists.
    pub fn system_hide(&mut self, id: ToastId) -> bool {
        let mut found = false;
        let active = self.active.iter_mut().map(|i| &mut i.toast);
        for t in active.chain(self.queue.iter_mut()) {
            if let Toast::System(s) = t
                && s.id == Some(id)
            {
                s.force_hide = true;
                found = true;
            }
        }
        found
    }

    /// Tutorial notification `id` (`tutorial.rs`): stays displayed until `tutorial_hide`, or
    /// `display_ms` milliseconds if non-zero.
    #[allow(clippy::too_many_arguments)]
    pub fn tutorial(
        &mut self,
        gui: &Gui,
        id: u32,
        icon: &'static str,
        title: &Component,
        description: &Component,
        progressable: bool,
        display_ms: i64,
    ) {
        let mut t = title.clone();
        t.style.color = Some(0x50_0050);
        let split = |c: &Component| rc_text::wrap(gui, &rc_text::styled_chars(&rc_text::flatten(c, &gui.lang)), 126.0);
        let mut lines = split(&t);
        lines.extend(split(description));
        let toast = TutorialToast {
            id,
            icon,
            lines,
            progressable,
            progress: 0.0,
            smoothed: 0.0,
            last_smooth: 0,
            hidden: false,
            display_ms,
        };
        self.queue.push_back(Toast::Tutorial(Box::new(toast)));
    }

    fn tutorial_mut(&mut self, id: u32) -> Option<&mut TutorialToast> {
        let active = self.active.iter_mut().map(|i| &mut i.toast);
        active.chain(self.queue.iter_mut()).find_map(|t| match t {
            Toast::Tutorial(t) if t.id == id => Some(&mut **t),
            _ => None,
        })
    }

    pub fn tutorial_hide(&mut self, id: u32) {
        if let Some(t) = self.tutorial_mut(id) {
            t.hidden = true;
        }
    }

    pub fn tutorial_progress(&mut self, id: u32, p: f32) {
        if let Some(t) = self.tutorial_mut(id) {
            t.progress = p;
        }
    }

    /// Update on every frame (`now` in ms), even with the HUD hidden: displayed notifications (a
    /// single visibility sound per update, slots freed at the end of the exit
    /// slide), then the whole queue walked in order, each notification that fits placed.
    pub fn update(&mut self, now: f64, mult: f64) {
        let now = now as i64;
        let mut sound_played = false;
        for i in &mut self.active {
            let before = i.shown;
            i.update(now, mult);
            if i.shown != before && !sound_played {
                sound_played = true;
                self.sounds.push(if i.shown { "minecraft:ui.toast.in" } else { "minecraft:ui.toast.out" });
            }
        }
        self.active.retain(|i| !i.finished);
        let mut occupied = [false; SLOTS];
        for i in &self.active {
            for s in occupied.iter_mut().skip(i.slot).take(i.count) {
                *s = true;
            }
        }
        if occupied.iter().all(|&o| o) {
            return;
        }
        let mut challenge = false;
        let mut kept = VecDeque::with_capacity(self.queue.len());
        for toast in self.queue.drain(..) {
            let need = toast.slots();
            let Some(slot) = free_run(&occupied, need) else {
                kept.push_back(toast);
                continue;
            };
            for s in occupied.iter_mut().skip(slot).take(need) {
                *s = true;
            }
            // Challenge sound: when the notification takes a slot, once per update.
            if matches!(&toast, Toast::Advancement { frame: Frame::Challenge, .. }) && !challenge {
                challenge = true;
                self.sounds.push(CHALLENGE);
            }
            self.active.push(Instance::new(toast, slot, need));
        }
        self.queue = kept;
    }

    /// Drawing, new stratum above the rest of the interface.
    /// Position: `screen width − width × portion`, y `slot × height` (a tall
    /// notification placed lower goes down by as much, like in the game).
    pub fn draw(&mut self, gui: &mut Gui, queue: &wgpu::Queue, it: &HotbarItems<'_>, _now: f64, _mult: f64) {
        if self.active.is_empty() {
            return;
        }
        let f = |v: i32| v as f32;
        let screen = gui.width.floor();
        for inst in &mut self.active {
            if inst.finished {
                continue;
            }
            let w = inst.toast.width();
            let x = screen - w as f32 * inst.portion;
            let y = f(inst.slot as i32 * inst.toast.height());
            let fv = inst.fully_visible_for as f64;
            gui.next_layer();
            match &inst.toast {
                Toast::Recipe { entries, index, .. } => {
                    gui.sprite("toast/recipe", x, y, 160.0, 32.0, 0xffff_ffff);
                    let title = rc_text::styled_chars(&rc_text::flatten(
                        &Component::translate("recipe.toast.title", vec![]),
                        &gui.lang,
                    ));
                    let desc = rc_text::styled_chars(&rc_text::flatten(
                        &Component::translate("recipe.toast.description", vec![]),
                        &gui.lang,
                    ));
                    gui.text(queue, &title, x + 30.0, y + 7.0, 0xff50_0050, false, 0);
                    gui.text(queue, &desc, x + 30.0, y + 18.0, 0xff00_0000, false, 0);
                    let Some((station, result)) = entries.get(*index).or_else(|| entries.last()) else { continue };
                    inst.cache.resize_with(2, || None);
                    if let Some(models) = gui.items.as_ref().map(|a| &a.models) {
                        for (c, s) in inst.cache.iter_mut().zip([station, result]) {
                            match s {
                                Some(s) => refresh(c, s, it, models, 3000),
                                None => *c = None,
                            }
                        }
                    }
                    gui.stratum(Blend::Premultiplied);
                    if let Some(c) = &inst.cache[0]
                        && !c.layers.is_empty()
                    {
                        gui.item_scaled(c.key, &c.layers, x + 3.0 * 0.6, y + 3.0 * 0.6, 0.6);
                    }
                    if let Some(c) = &inst.cache[1]
                        && !c.layers.is_empty()
                    {
                        gui.item(c.key, &c.layers, x + 8.0, y + 8.0, 0.0);
                    }
                    gui.stratum(Blend::Normal);
                }
                Toast::Tutorial(t) => {
                    let h = inst.toast.height();
                    gui.sprite("toast/tutorial", x, y, 160.0, f(h), 0xffff_ffff);
                    gui.sprite(t.icon, x + 6.0, y + 6.0, 20.0, 20.0, 0xffff_ffff);
                    let content = (t.lines.len() as i32).max(2) * 11;
                    let top = 7 + (content - t.lines.len() as i32 * 11) / 2;
                    for (k, l) in t.lines.iter().enumerate() {
                        gui.text(queue, l, x + 30.0, y + f(top + 11 * k as i32), 0xff00_0000, false, 0);
                    }
                    if t.progressable {
                        let by = y + f(h - 4);
                        gui.fill(x + 3.0, by, x + 157.0, by + 1.0, 0xffff_ffff);
                        let col = if t.progress >= t.smoothed { 0xff00_7000 } else { 0xff55_0000 };
                        gui.fill(x + 3.0, by, x + (3.0 + 154.0 * t.smoothed).trunc(), by + 1.0, col);
                    }
                }
                Toast::System(s) => {
                    let (title, message) = (&s.title, &s.message);
                    let h = inst.toast.height();
                    gui.sprite("toast/system", x, y, f(s.width), f(h), 0xffff_ffff);
                    let top = if message.is_empty() { 12 } else { 7 };
                    for (k, l) in title.iter().enumerate() {
                        gui.text(queue, l, x + 18.0, y + f(top + 12 * k as i32), 0xffff_ff00, false, 0);
                    }
                    let top = 7 + title.len() as i32 * 12;
                    for (k, l) in message.iter().enumerate() {
                        gui.text(queue, l, x + 18.0, y + f(top + 12 * k as i32), 0xffff_ffff, false, 0);
                    }
                }
                Toast::Advancement { title, frame, icon } => {
                    gui.sprite("toast/advancement", x, y, 160.0, 32.0, 0xffff_ffff);
                    let flat = |gui: &Gui, c: &Component| rc_text::styled_chars(&rc_text::flatten(c, &gui.lang));
                    let title = flat(gui, title);
                    let key = format!("advancements.toast.{}", frame.name());
                    let label = flat(gui, &Component::translate(key, vec![]));
                    let lines = rc_text::wrap(&*gui, &title, 125.0);
                    let color = frame.toast_color();
                    if lines.len() == 1 {
                        gui.text(queue, &label, x + 30.0, y + 7.0, color, false, 0);
                        gui.text(queue, &title, x + 30.0, y + 18.0, 0xffff_ffff, false, 0);
                    } else {
                        // Title on several lines: label alone, then lines fading in (§ 3.1).
                        let t = fv;
                        if t < 1500.0 {
                            let a = (((1500.0 - t) / 300.0).clamp(0.0, 1.0) * 255.0) as u32;
                            gui.text(queue, &label, x + 30.0, y + 11.0, a << 24 | (color & 0xff_ffff), false, 0);
                        } else {
                            let a = (((t - 1500.0) / 300.0).clamp(0.0, 1.0) * 252.0) as u32;
                            let top = 16 - (lines.len() as i32 * 9) / 2;
                            for (k, l) in lines.iter().enumerate() {
                                let ly = y + f(top + 9 * k as i32);
                                gui.text(queue, l, x + 30.0, ly, a << 24 | 0xff_ffff, false, 0);
                            }
                        }
                    }
                    inst.cache.resize_with(1, || None);
                    if let Some(models) = gui.items.as_ref().map(|a| &a.models) {
                        refresh(&mut inst.cache[0], icon, it, models, 0);
                    }
                    if let Some(c) = &inst.cache[0]
                        && !c.layers.is_empty()
                    {
                        gui.stratum(Blend::Premultiplied);
                        gui.item(c.key, &c.layers, x + 8.0, y + 8.0, 0.0);
                        gui.stratum(Blend::Normal);
                    }
                }
            }
        }
    }
}

/// First block of `need` consecutive free slots, if enough free slots remain.
fn free_run(occupied: &[bool; SLOTS], need: usize) -> Option<usize> {
    if occupied.iter().filter(|&&o| !o).count() < need {
        return None;
    }
    let mut run = 0;
    for (i, &o) in occupied.iter().enumerate() {
        if o {
            run = 0;
        } else {
            run += 1;
            if run == need {
                return Some(i + 1 - run);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn system(lines: usize) -> Toast {
        let line = vec![rc_text::StyledChar { c: 'a', style: Default::default() }];
        Toast::System(Box::new(SystemToast {
            title: vec![line.clone()],
            message: vec![line; lines],
            width: 190,
            display_ms: 5000.0,
            id: None,
            changed: false,
            last_changed: 0,
            force_hide: false,
        }))
    }

    fn adv() -> Toast {
        Toast::Advancement {
            title: Box::new(Component::text("x")),
            frame: Frame::Task,
            icon: ItemStack { item: 1, count: 1, added: Vec::new(), removed: Vec::new() },
        }
    }

    /// Playtime reminders (game file, Korea): every hour the number of hours, after
    /// 24 h the "more than 24 hours" reminder (first of the list); nothing for another country.
    #[test]
    fn periodic_playtime_reminders_follow_the_timer() {
        let json = br#"{"KOR":[{"delay":1440,"period":60,"title":"compliance.playtime.greaterThan24Hours","message":"compliance.playtime.message"},{"period":60,"title":"compliance.playtime.hours","message":"compliance.playtime.message"}]}"#;
        let mut other = Periodic::new(json, Some("FRA"), 0.0);
        assert_eq!(other.poll(1e12), None);
        let mut p = Periodic::new(json, Some("KOR"), 1000.0);
        let hour = 3_600_000.0;
        let s = |t: &str, n: i64| Some((format!("compliance.playtime.{t}"), "compliance.playtime.message".to_owned(), n));
        // Zero minimum delay: first ring at load, with 0 periods (like the game).
        assert_eq!(p.poll(1000.0), s("hours", 0));
        assert_eq!(p.poll(1000.0 + hour), s("hours", 1));
        assert_eq!(p.poll(1000.0 + hour), None);
        for h in 2..24i32 {
            assert_eq!(p.poll(1000.0 + hour * f64::from(h)).map(|x| x.2), Some(i64::from(h)));
        }
        assert_eq!(p.poll(1000.0 + hour * 24.0), s("greaterThan24Hours", 24));
    }

    /// Timed tutorial notification (social interactions, 8 s): bar proportional to the time,
    /// removal after the delay, whatever the configured notification duration.
    #[test]
    fn timed_tutorial_toast_hides_after_its_time() {
        let mut t = Toast::Tutorial(Box::new(TutorialToast {
            id: 1,
            icon: "toast/social_interactions",
            lines: Vec::new(),
            progressable: true,
            progress: 0.0,
            smoothed: 0.0,
            last_smooth: 0,
            hidden: false,
            display_ms: 8000,
        }));
        assert!(t.update(4000, 10.0));
        let Toast::Tutorial(tt) = &t else { unreachable!() };
        assert_eq!((tt.progress, tt.smoothed), (0.5, 0.5));
        assert!(t.update(8000, 10.0));
        assert!(!t.update(8001, 10.0));
    }

    #[test]
    fn display_clock_starts_after_slide_in() {
        let mut t = Toasts::default();
        t.advancement(
            Component::text("x"),
            Frame::Task,
            ItemStack { item: 1, count: 1, added: vec![], removed: vec![] },
        );
        t.update(1000.0, 1.0);
        assert_eq!(t.active.len(), 1);
        // First update: entry slide, "in" sound.
        t.update(1000.0, 1.0);
        assert_eq!(t.sounds, ["minecraft:ui.toast.in"]);
        // Frames every 10 ms: 5 s after the entry, still 600 ms to hold (the clock starts at
        // the end of the slide).
        let mut now = 1000.0;
        while now < 6590.0 {
            now += 10.0;
            t.update(now, 1.0);
        }
        assert!(t.active[0].shown);
        while now < 6610.0 {
            now += 10.0;
            t.update(now, 1.0);
        }
        assert!(!t.active[0].shown, "hidden at 5 s fully visible");
        assert_eq!(t.sounds.last(), Some(&"minecraft:ui.toast.out"));
        while now < 7220.0 {
            now += 10.0;
            t.update(now, 1.0);
        }
        assert!(t.active.is_empty(), "slot freed after the exit slide");
    }

    #[test]
    fn queued_recipe_toast_waits_with_its_own_clock() {
        let mut t = Toasts::default();
        for _ in 0..5 {
            t.queue.push_back(adv());
        }
        t.recipe(None, None, 0.0);
        t.update(0.0, 1.0);
        assert_eq!(t.queue.len(), 1, "recipe queued, the 5 slots are taken");
        // The advancements leave (≈ 5.6 s + 0.6 s); the recipe then arrives, with a full 5 s.
        let mut now = 0.0;
        while !t.queue.is_empty() {
            now += 50.0;
            t.update(now, 1.0);
        }
        let start = now;
        let recipe = |t: &Toasts| t.active.iter().find(|i| matches!(i.toast, Toast::Recipe { .. })).map(|i| i.shown);
        while now < start + 600.0 + 4900.0 {
            now += 10.0;
            t.update(now, 1.0);
        }
        assert_eq!(recipe(&t), Some(true), "the waiting time in the queue does not count");
        while now < start + 600.0 + 5100.0 {
            now += 10.0;
            t.update(now, 1.0);
        }
        assert_eq!(recipe(&t), Some(false));
    }

    #[test]
    fn whole_queue_is_scanned_and_tall_toasts_sit_lower() {
        let mut t = Toasts::default();
        for _ in 0..4 {
            t.queue.push_back(adv());
        }
        t.update(0.0, 1.0);
        // A large one (2 slots) does not fit, the next small one goes ahead.
        t.queue.push_back(system(3));
        t.queue.push_back(adv());
        t.update(10.0, 1.0);
        assert_eq!(t.active.len(), 5);
        assert!(matches!(t.queue.front(), Some(Toast::System(_))));
        assert_eq!(system(3).height(), 20 + 3 * 12);
        assert_eq!(system(3).slots(), 2);
        assert_eq!(free_run(&[true, false, true, false, false], 2), Some(3));
        assert_eq!(free_run(&[true, false, true, false, true], 2), None);
    }
}
