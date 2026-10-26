//! Warning screens outside a world (`docs/specs/option-menus.md` § 3.16): online game warning
//! (first time), server code of conduct, accessibility welcome (first
//! launch), server reconfiguration. They stack in [`crate::front::Front`].

use std::time::Instant;

use rc_text::StyledChar;

use crate::gui::Gui;
use crate::options::Options;
use crate::widgets::{self, Event, Kind, Panel, Widget, chars, tr};

pub const ACCEPT: u32 = 1;
pub const REFUSE: u32 = 2;
const CHECK: u32 = 3;

/// Centered "title, message, checkbox, buttons" warning (§ 3.16).
#[derive(Debug, Clone, PartialEq)]
pub enum NoticeKind {
    /// Online play with third-party servers: "Proceed" / "Back".
    Multiplayer,
    /// Code of conduct (server text): "Acknowledge" / "Disconnect".
    Conduct(String),
}

pub struct Notice {
    pub kind: NoticeKind,
    pub panel: Panel,
    size: (i32, i32),
    lines: Vec<Vec<StyledChar>>,
    title_y: i32,
    text_x: i32,
    text_y: i32,
}

/// Reply to a warning: accepted, checkbox ticked.
pub type Answer = (bool, bool);

impl Notice {
    pub fn new(kind: NoticeKind) -> Notice {
        Notice { kind, panel: Panel::default(), size: (-1, -1), lines: Vec::new(), title_y: 0, text_x: 0, text_y: 0 }
    }

    /// Layout: centered column, spacing 8; message cut at `width − 100` (margin 12).
    pub fn layout(&mut self, gui: &Gui) {
        let size = (gui.width as i32, gui.height as i32);
        if self.size == size {
            return;
        }
        self.size = size;
        let (w, h) = size;
        let t = |k: &str| tr(&gui.lang, k, &[]);
        let (message, check, yes, no) = match &self.kind {
            NoticeKind::Multiplayer => {
                (t("multiplayerWarning.message"), t("multiplayerWarning.check"), t("gui.proceed"), t("gui.back"))
            }
            NoticeKind::Conduct(text) => {
                (text.clone(), t("multiplayer.codeOfConduct.check"), t("gui.acknowledge"), t("menu.disconnect"))
            }
        };
        let text_w = w - 100 - 8;
        let max_lines = ((h - 100 - 8) / 9).max(1) as usize;
        self.lines = rc_text::wrap(gui, &chars(&message), text_w as f32);
        self.lines.truncate(max_lines);
        let msg_h = 9 * self.lines.len() as i32 + 8;
        let total = 9 + 8 + (msg_h + 24) + 8 + 17 + 8 + 20;
        let top = (h - total) / 2;
        self.title_y = top;
        self.text_x = 50 + 4;
        self.text_y = top + 9 + 8 + 12 + 4;
        let check_y = top + 9 + 8 + msg_h + 24 + 8;
        let cw = 17 + 4 + rc_text::width(gui, &chars(&check));
        let checked = self.panel.get(CHECK).is_some_and(|c| matches!(c.kind, Kind::Checkbox { checked: true }));
        let by = check_y + 17 + 8;
        let keyboard = self.panel.keyboard;
        self.panel = Panel::new(vec![
            Widget::new(CHECK, Kind::Checkbox { checked }, (w - cw) / 2, check_y, cw, 17, chars(&format!("§7{check}"))),
            Widget::button(ACCEPT, w / 2 - 154, by, 150, &yes),
            Widget::button(REFUSE, w / 2 + 4, by, 150, &no),
        ]);
        self.panel.keyboard = keyboard;
    }

    /// Button pressed: (accepted, checkbox ticked).
    pub fn handle(&mut self, events: &[Event]) -> Option<Answer> {
        let checked = self.panel.get(CHECK).is_some_and(|c| matches!(c.kind, Kind::Checkbox { checked: true }));
        events.iter().find_map(|e| match e {
            Event::Press(ACCEPT) => Some((true, checked)),
            Event::Press(REFUSE) => Some((false, checked)),
            _ => None,
        })
    }

    /// Esc: "Back" for the online game warning; no effect on the code of conduct.
    pub fn escape(&self) -> Option<Answer> {
        (self.kind == NoticeKind::Multiplayer).then_some((false, false))
    }

    pub fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, o: &Options, mouse: Option<(f32, f32)>, time: f64) {
        self.layout(gui);
        gui.next_layer();
        widgets::menu_background(gui, o.i("menuBackgroundBlurriness"), false);
        gui.next_layer();
        let key = match self.kind {
            NoticeKind::Multiplayer => "multiplayerWarning.header",
            NoticeKind::Conduct(_) => "multiplayer.codeOfConduct.title",
        };
        let title = chars(&format!("§l{}", tr(&gui.lang, key, &[])));
        widgets::centered(gui, q, &title, gui.width as i32 / 2, self.title_y, 0xffff_ffff);
        for (k, l) in self.lines.iter().enumerate() {
            gui.text(q, l, self.text_x as f32, (self.text_y + 9 * k as i32) as f32, 0xffff_ffff, true, 0);
        }
        self.panel.draw(gui, q, mouse, time);
    }
}

// ---------- Accessibility welcome (first launch) ----------

const NARRATOR: u32 = 1;
const SOUNDS: u32 = 2;
const ACCESSIBILITY: u32 = 3;
const LANGUAGE: u32 = 4;
const CONTINUE: u32 = 5;

/// Request of the welcome screen.
#[derive(Debug, Clone, PartialEq)]
pub enum OnboardOut {
    /// Options screen to open on top (the welcome screen stays underneath); the end of the exit fade
    /// is seen at draw time (`finished`).
    Menu(Vec<crate::menus::Id>),
}

pub struct Onboarding {
    pub panel: Panel,
    size: (i32, i32),
    lines: Vec<Vec<StyledChar>>,
    text_y: i32,
    fade_in: Option<Instant>,
    fade_out: Option<Instant>,
}

impl Default for Onboarding {
    fn default() -> Self {
        Onboarding {
            panel: Panel::default(),
            size: (-1, -1),
            lines: Vec::new(),
            text_y: 0,
            fade_in: None,
            fade_out: None,
        }
    }
}

impl Onboarding {
    /// Layout: header of 90 (logo), text of at most 374 (margin 8), 2 × 2 grid of 150
    /// (margin 4), "Continue" in a footer of 33.
    pub fn layout(&mut self, gui: &Gui) {
        let size = (gui.width as i32, gui.height as i32);
        if self.size == size {
            return;
        }
        self.size = size;
        let (w, h) = size;
        let t = |k: &str| tr(&gui.lang, k, &[]);
        self.lines = rc_text::wrap(gui, &chars(&t("accessibility.onboarding.screen.title")), 374.0);
        let text_h = 9 * self.lines.len() as i32 + 8;
        let content_h = 4 + 8 + text_h + 8 + 4 + 4 + 2 * 28;
        let top = (90 + 30).min(h - 33 - content_h);
        self.text_y = top + 4 + 8 + 4;
        let gy = top + 4 + 8 + text_h + 8 + 4 + 4;
        let (x0, x1) = (w / 2 - 154, w / 2 + 4);
        let mut narrator = Widget::new(
            NARRATOR,
            Kind::Cycle,
            x0,
            gy,
            150,
            20,
            chars(&tr(
                &gui.lang,
                "options.generic_value",
                &[t("options.narrator"), t("options.narrator.notavailable")],
            )),
        );
        narrator.active = false;
        let keyboard = self.panel.keyboard;
        self.panel = Panel::new(vec![
            narrator,
            // Text and icon buttons of the game; tooltips of the language and accessibility.
            Widget::new(SOUNDS, Kind::IconText("icon/music_notes", 16, 16), x1, gy, 150, 20, chars(&t("options.sounds"))),
            Widget::new(
                ACCESSIBILITY,
                Kind::IconText("icon/accessibility", 15, 15),
                x0,
                gy + 28,
                150,
                20,
                chars(&t("accessibility.onboarding.accessibility.button")),
            )
            .with_tooltip(gui, &t("options.accessibility.tooltip")),
            Widget::new(LANGUAGE, Kind::IconText("icon/language", 15, 15), x1, gy + 28, 150, 20, chars(&t("options.language")))
                .with_tooltip(gui, &t("options.language.tooltip")),
            Widget::button(CONTINUE, w / 2 - 75, h - 26, 150, &t("gui.continue")),
        ]);
        self.panel.keyboard = keyboard;
    }

    pub fn handle(&mut self, events: &[Event], escape: bool) -> Option<OnboardOut> {
        use crate::menus::Id;
        if self.fade_out.is_some() {
            return None;
        }
        for e in events {
            match e {
                Event::Press(SOUNDS) => return Some(OnboardOut::Menu(vec![Id::Options, Id::Sounds])),
                Event::Press(ACCESSIBILITY) => return Some(OnboardOut::Menu(vec![Id::Options, Id::Accessibility])),
                Event::Press(LANGUAGE) => return Some(OnboardOut::Menu(vec![Id::Options, Id::Language])),
                Event::Press(CONTINUE) => self.fade_out = Some(Instant::now()),
                _ => {}
            }
        }
        if escape {
            self.fade_out = Some(Instant::now());
        }
        None
    }

    /// Exit fade (1 s) finished.
    pub fn finished(&self) -> bool {
        self.fade_out.is_some_and(|t| t.elapsed().as_secs_f32() >= 1.0)
    }

    pub fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, o: &Options, mouse: Option<(f32, f32)>, time: f64) {
        self.layout(gui);
        // Appearance: components invisible the first second, then fade until 2 s;
        // exit: fade in 1 s (components only).
        let start = *self.fade_in.get_or_insert_with(Instant::now);
        let k = (start.elapsed().as_secs_f32() / 2.0).clamp(0.0, 1.0);
        let mut alpha = ((k - 0.5) * 2.0).clamp(0.0, 1.0);
        if let Some(t) = self.fade_out {
            let f = 1.0 - t.elapsed().as_secs_f32();
            alpha = ((f - 0.5) * 2.0).clamp(0.0, 1.0);
        }
        gui.next_layer();
        widgets::menu_background(gui, o.i("menuBackgroundBlurriness"), false);
        gui.next_layer();
        let w = gui.width as i32;
        gui.sprite_region_color(
            "tex/gui/title/minecraft",
            (256.0, 64.0),
            (0.0, 0.0),
            [(w / 2 - 128) as f32, 30.0, 256.0, 44.0],
            0xffff_ffff,
        );
        gui.sprite_region_color(
            "tex/gui/title/edition",
            (128.0, 16.0),
            (0.0, 0.0),
            [(w / 2 - 64) as f32, 67.0, 128.0, 14.0],
            0xffff_ffff,
        );
        let a = ((alpha * 255.0) as u32) << 24;
        if a != 0 {
            for (k, l) in self.lines.iter().enumerate() {
                widgets::centered(gui, q, l, w / 2, self.text_y + 9 * k as i32, a | 0xff_ffff);
            }
        }
        for wd in &mut self.panel.widgets {
            wd.alpha = alpha;
        }
        if alpha > 0.0 {
            self.panel.draw(gui, q, mouse, time);
        }
    }
}

// ---------- Reconfiguration ----------

const DISCONNECT: u32 = 1;
/// Delay before "Disconnect" becomes active (600 ticks).
const DISCONNECT_DELAY: f32 = 30.0;

pub struct Reconfig {
    pub panel: Panel,
    size: (i32, i32),
    since: Instant,
    title_y: i32,
}

impl Default for Reconfig {
    fn default() -> Self {
        Reconfig { panel: Panel::default(), size: (-1, -1), since: Instant::now(), title_y: 0 }
    }
}

impl Reconfig {
    /// Centered column: title and "Disconnect" (margins of 10).
    pub fn layout(&mut self, gui: &Gui) {
        let size = (gui.width as i32, gui.height as i32);
        let active = self.since.elapsed().as_secs_f32() >= DISCONNECT_DELAY;
        if self.size == size {
            if let Some(b) = self.panel.get_mut(DISCONNECT) {
                b.active = active;
            }
            return;
        }
        self.size = size;
        let (w, h) = size;
        let top = (h - (9 + 20 + 20 + 20)) / 2;
        self.title_y = top + 10;
        let mut b = Widget::button(DISCONNECT, w / 2 - 75, top + 29 + 10, 150, &tr(&gui.lang, "menu.disconnect", &[]));
        b.active = active;
        self.panel = Panel::new(vec![b]);
    }

    pub fn wants_disconnect(&self, events: &[Event]) -> bool {
        events.contains(&Event::Press(DISCONNECT))
    }

    pub fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, o: &Options, mouse: Option<(f32, f32)>, time: f64) {
        self.layout(gui);
        gui.next_layer();
        widgets::menu_background(gui, o.i("menuBackgroundBlurriness"), false);
        gui.next_layer();
        let title = chars(&tr(&gui.lang, "connect.reconfiguring", &[]));
        widgets::centered(gui, q, &title, gui.width as i32 / 2, self.title_y, 0xffff_ffff);
        self.panel.draw(gui, q, mouse, time);
    }
}

// ---------- Application ----------

impl crate::App {
    /// `code_of_conduct` (configuration): already accepted for this `servers.dat` entry
    /// (same fingerprint) → immediate acceptance; otherwise screen.
    pub(crate) fn on_code_of_conduct(&mut self, text: String) {
        let hash = crate::servers::java_string_hash(&text);
        if let Some((name, ip)) = &self.menus.world.server {
            let list = crate::servers::ServerList::load(&crate::servers::game_dir());
            let known = list
                .visible
                .iter()
                .chain(&list.hidden)
                .any(|e| &e.name == name && &e.ip == ip && e.accepted_code_of_conduct == hash);
            if known {
                eprintln!("code of conduct already accepted for {name}: accepted without screen");
                let _ = self.session.link.send_accept_code_of_conduct();
                return;
            }
        }
        self.front.open_notice(NoticeKind::Conduct(text));
        self.keys.clear();
        self.grab(false);
    }

    /// Reply to the code of conduct screen: stored in `servers.dat` (fingerprint if accepted
    /// with the box ticked, otherwise erased), then acceptance or disconnection.
    pub(crate) fn conduct_answer(&mut self, text: &str, accepted: bool, remember: bool) {
        if let Some((name, ip)) = self.menus.world.server.clone() {
            let dir = crate::servers::game_dir();
            let mut list = crate::servers::ServerList::load(&dir);
            if let Some(e) =
                list.visible.iter_mut().chain(list.hidden.iter_mut()).find(|e| e.name == name && e.ip == ip)
            {
                e.accepted_code_of_conduct =
                    if accepted && remember { crate::servers::java_string_hash(text) } else { 0 };
                eprintln!("code of conduct: accepted {accepted}, remembered {}", e.accepted_code_of_conduct != 0);
                if let Err(e) = list.save(&dir) {
                    eprintln!("servers.dat: {e}");
                }
            }
        }
        if accepted {
            let _ = self.session.link.send_accept_code_of_conduct();
            if self.front.in_world && !self.front.is_open() && !self.menus.is_open() {
                self.grab(true);
            }
        } else {
            self.front.kicked = Some(rc_text::Component::translate("multiplayer.disconnect.code_of_conduct", vec![]));
            self.session_end("code of conduct refused");
        }
    }
}
