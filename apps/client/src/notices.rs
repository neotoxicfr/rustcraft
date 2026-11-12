//! Warning screens outside a world (`docs/specs/option-menus.md` § 3.16): online game warning
//! (first time), server code of conduct, accessibility welcome (first
//! launch), server reconfiguration. They stack in [`crate::front::Front`].

use std::time::Instant;

use rc_text::StyledChar;

use crate::gui::{Gui, TextContext};
use crate::kept_text::Kept;
use crate::options::Options;
use crate::widgets::{self, Event, Kind, Panel, Widget, chars, tr};

pub const ACCEPT: u32 = 1;
pub const REFUSE: u32 = 2;
const CHECK: u32 = 3;
const MESSAGE: u32 = 4;

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
    title_y: i32,
    /// Bold title, kept while the language stays (see [`Gui::keeps_translated_texts`]).
    title: Kept<Vec<StyledChar>>,
}

/// Reply to a warning: accepted, checkbox ticked.
pub type Answer = (bool, bool);

/// Colour of the online play checkbox label (the code of conduct keeps the default white).
const SAFETY_CHECK_COLOR: u32 = 0xE0E0E0;

impl Notice {
    pub fn new(kind: NoticeKind) -> Notice {
        Notice { kind, panel: Panel::default(), size: (-1, -1), title_y: 0, title: Kept::default() }
    }

    pub fn layout(&mut self, gui: &Gui) {
        let size = (gui.width as i32, gui.height as i32);
        if self.size != size {
            self.arrange(gui, &gui.lang, size);
        }
    }

    /// Centered column, spacing 8: title, message in a framed text area `width − 100` wide
    /// (margin 12) cut to its lines, or `height − 100` high and scrolled when longer;
    /// checkbox, buttons. Focus order: message, checkbox, buttons.
    fn arrange(&mut self, m: &dyn rc_text::Metrics, lang: &rc_text::Lang, size: (i32, i32)) {
        self.size = size;
        let (w, h) = size;
        let t = |k: &str| tr(lang, k, &[]);
        let (message, check, yes, no) = match &self.kind {
            NoticeKind::Multiplayer => {
                (t("multiplayerWarning.message"), t("multiplayerWarning.check"), t("gui.proceed"), t("gui.back"))
            }
            NoticeKind::Conduct(text) => {
                (text.clone(), t("multiplayer.codeOfConduct.check"), t("gui.acknowledge"), t("menu.disconnect"))
            }
        };
        let area_w = w - 100;
        let mut area = widgets::TextArea::new(rc_text::wrap(m, &chars(&message), (area_w - 8) as f32));
        let area_h = area.content().min(h - 100);
        let old = self.panel.get(MESSAGE).and_then(|w| match &w.kind {
            Kind::TextArea(a) => Some(a.scroll),
            _ => None,
        });
        area.set_scroll(old.unwrap_or(0.0), area_h);
        let mut label = chars(&check);
        let mut dots = rc_text::CharStyle::default();
        if self.kind == NoticeKind::Multiplayer {
            dots.color = Some(SAFETY_CHECK_COLOR);
            for c in &mut label {
                c.style.color.get_or_insert(SAFETY_CHECK_COLOR);
            }
        }
        let checked = self.panel.get(CHECK).is_some_and(|c| matches!(c.kind, Kind::Checkbox { checked: true, .. }));
        let mut cb = Widget::checkbox(m, CHECK, (0, 0), checked, label, None, &dots);
        let total = 9 + 8 + (area_h + 24) + 8 + cb.h + 8 + 20;
        let top = (h - total) / 2;
        self.title_y = top;
        let check_y = top + 9 + 8 + area_h + 24 + 8;
        (cb.x, cb.y) = ((w - cb.w) / 2, check_y);
        let by = check_y + cb.h + 8;
        let (keyboard, focus) = (self.panel.keyboard, self.panel.focus);
        self.panel = Panel::new(vec![
            Widget::new(MESSAGE, Kind::TextArea(Box::new(area)), (w - area_w) / 2, top + 9 + 8 + 12, area_w, area_h, Vec::new()),
            cb,
            Widget::button(ACCEPT, w / 2 - 154, by, 150, &yes),
            Widget::button(REFUSE, w / 2 + 4, by, 150, &no),
        ]);
        self.panel.keyboard = keyboard;
        self.panel.focus = focus;
    }

    /// Button pressed: (accepted, checkbox ticked).
    pub fn handle(&mut self, events: &[Event]) -> Option<Answer> {
        let checked = self.panel.get(CHECK).is_some_and(|c| matches!(c.kind, Kind::Checkbox { checked: true, .. }));
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
        let keep = gui.keeps_translated_texts();
        let title = self.title.get(gui, keep, |gui| chars(&format!("§l{}", tr(&gui.lang, key, &[]))));
        widgets::centered(gui, q, title, gui.width as i32 / 2, self.title_y, 0xffff_ffff);
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
    /// Text context of the layout: the language menu opens over this screen, its labels follow
    /// the language chosen there.
    built_for: Option<TextContext>,
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
            built_for: None,
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
        if self.size == size && self.built_for == Some(gui.text_context()) {
            return;
        }
        self.size = size;
        self.built_for = Some(gui.text_context());
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
        gui.next_layer();
        widgets::menu_background(gui, o.i("menuBackgroundBlurriness"), false);
        gui.next_layer();
        // Logo of the title screen that follows, drawn opaque through the fades: it stays in place
        // when this screen gives way to the title screen.
        crate::front::draw_logo(gui, false, 0xffff_ffff);
        self.draw_content(gui, q, mouse, time);
    }

    /// Opacity of the text and the buttons: invisible the first second, then fading in until 2 s;
    /// on exit, fading out in 1 s.
    fn content_alpha(&mut self) -> f32 {
        let start = *self.fade_in.get_or_insert_with(Instant::now);
        let k = (start.elapsed().as_secs_f32() / 2.0).clamp(0.0, 1.0);
        let mut alpha = ((k - 0.5) * 2.0).clamp(0.0, 1.0);
        if let Some(t) = self.fade_out {
            let f = 1.0 - t.elapsed().as_secs_f32();
            alpha = ((f - 0.5) * 2.0).clamp(0.0, 1.0);
        }
        alpha
    }

    /// Text and buttons under the logo.
    fn draw_content(&mut self, gui: &mut Gui, q: &wgpu::Queue, mouse: Option<(f32, f32)>, time: f64) {
        let alpha = self.content_alpha();
        let w = gui.width as i32;
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
        let title = gui.translated("connect.reconfiguring");
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Dummy metrics: 6 px per character, 4 px for the space.
    struct Fixed;
    impl rc_text::Metrics for Fixed {
        fn advance(&self, _font: &str, c: char) -> (f32, f32) {
            (if c == ' ' { 4.0 } else { 6.0 }, 1.0)
        }
    }

    fn lang() -> rc_text::Lang {
        rc_text::Lang::load(br#"{"multiplayerWarning.check":"Do not show this screen again"}"#, None, None)
    }

    /// Welcome screen with its fade-in over (text and buttons opaque).
    fn settled_onboarding() -> Onboarding {
        let long_ago = Instant::now().checked_sub(std::time::Duration::from_secs(5));
        Onboarding { fade_in: long_ago, ..Default::default() }
    }

    /// The welcome screen draws the logo of the title screen that follows it
    /// ([`crate::front::draw_logo`]): rendered at 1600 × 900 against the logo it drew before (the
    /// game's "Java Edition" subtitle, 128 × 14 at y 67), the frame changes, and only inside the
    /// subtitle's area. Without an installed game or a GPU: skipped.
    #[test]
    fn onboarding_logo_changes_only_the_subtitle() {
        use crate::gui::tests_opt;
        use crate::render::upload::Uploader;
        let Some(e) = tests_opt::env() else { return };
        let o = Options::default();
        let rendered = |logo_before: bool| {
            let mut gui = tests_opt::gui(&e, tests_opt::ALL);
            let mut screen = settled_onboarding();
            gui.scale_option = 3;
            gui.begin(tests_opt::W, tests_opt::H);
            gui.overlays_here();
            gui.models_here();
            if logo_before {
                screen.layout(&gui);
                gui.next_layer();
                widgets::menu_background(&mut gui, o.i("menuBackgroundBlurriness"), false);
                gui.next_layer();
                let x = (gui.width as i32 / 2) as f32;
                let logo = [x - 128.0, 30.0, 256.0, 44.0];
                gui.sprite_region_color("tex/gui/title/minecraft", (256.0, 64.0), (0.0, 0.0), logo, 0xffff_ffff);
                let subtitle = [x - 64.0, 67.0, 128.0, 14.0];
                gui.sprite_region_color("tex/gui/title/edition", (128.0, 16.0), (0.0, 0.0), subtitle, 0xffff_ffff);
                screen.draw_content(&mut gui, &e.queue, None, 0.0);
            } else {
                screen.draw(&mut gui, &e.queue, &o, None, 0.0);
            }
            let pixels = tests_opt::render(&e, &mut gui, &mut Uploader::new(&e.device, true));
            (pixels, gui.scale, gui.width as i32)
        };
        let ((before, scale, width), (after, ..)) = (rendered(true), rendered(false));
        let changed = tests_opt::diff_at(&before, &after);
        assert!(changed.len() > 500, "the subtitle changed ({} pixels)", changed.len());
        // Subtitle area of both placements (rows 66 to 82), in pixels.
        let (x0, x1) = (((width / 2 - 64) as u32) * scale, ((width / 2 + 64) as u32) * scale);
        let (y0, y1) = (66 * scale, 82 * scale);
        let outside: Vec<_> = changed.iter().filter(|p| p.0 < x0 || p.0 >= x1 || p.1 < y0 || p.1 >= y1).copied().collect();
        assert!(outside.is_empty(), "pixels changed outside the subtitle: {}", tests_opt::where_diff(&outside));
    }

    /// The welcome screen lays its labels out again in the language chosen over it (its language
    /// button opens the language menu on top): they stayed in the previous language before.
    #[test]
    fn onboarding_labels_follow_the_language() {
        use crate::gui::tests_opt;
        let Some(e) = tests_opt::env() else { return };
        let Some(tables) = tests_opt::game_languages(&["en_us", "fr_fr"]) else { return };
        let mut gui = tests_opt::gui(&e, tests_opt::ALL);
        gui.begin(tests_opt::W, tests_opt::H);
        let mut screen = settled_onboarding();
        let label = |screen: &Onboarding| -> String {
            screen.panel.get(CONTINUE).expect("continue button").msg.iter().map(|s| s.c).collect()
        };
        for table in tables {
            let want = tr(&table, "gui.continue", &[]);
            gui.lang = std::sync::Arc::new(table);
            screen.layout(&gui);
            assert_eq!(label(&screen), want);
        }
    }

    fn area(n: &Notice) -> (&Widget, &widgets::TextArea) {
        let w = n.panel.get(MESSAGE).expect("message area");
        let Kind::TextArea(a) = &w.kind else { panic!("not a text area") };
        (w, a)
    }

    /// Short message: framed area cut to its lines; long one: `height − 100` high and scrolled
    /// (wheel, arrows with focus, bar held), the whole text kept.
    #[test]
    fn warning_message_is_framed_and_scrolls() {
        let mut n = Notice::new(NoticeKind::Conduct("Be nice.".into()));
        n.arrange(&Fixed, &lang(), (400, 300));
        let (w, a) = area(&n);
        assert_eq!((w.x, w.w, w.h, a.lines.len()), (50, 300, 17, 1));
        assert_eq!(w.y, n.title_y + 9 + 8 + 12);
        let long = vec!["word"; 400].join(" ");
        let mut n = Notice::new(NoticeKind::Conduct(long));
        n.arrange(&Fixed, &lang(), (400, 200));
        let (w, a) = area(&n);
        let (x, y, h, lines) = (w.x, w.y, w.h, a.lines.len());
        assert_eq!(h, 100, "height − 100");
        assert!(9 * lines as i32 + 8 > 100 && lines > 20, "nothing cut: {lines} lines");
        let max = f64::from(9 * lines as i32 + 8 - 100);
        let scroll = |n: &Notice| area(n).1.scroll;
        let mut ev = Vec::new();
        n.panel.wheel((x as f32 + 10.0, y as f32 + 10.0), -1.0, &mut ev);
        assert_eq!(scroll(&n), 9.0);
        // Over the bar (right of the frame) the wheel scrolls too.
        n.panel.wheel((x as f32 + 300.0 + 3.0, y as f32 + 10.0), -2.0, &mut ev);
        assert_eq!(scroll(&n), 27.0);
        n.panel.wheel((x as f32 + 10.0, y as f32 + 10.0), 10.0, &mut ev);
        assert_eq!(scroll(&n), 0.0);
        let none = winit::keyboard::ModifiersState::empty();
        n.panel.key(winit::keyboard::KeyCode::Tab, none, &mut ev);
        assert_eq!(n.panel.focus, Some(0), "the message takes the focus first");
        assert!(n.panel.key(winit::keyboard::KeyCode::ArrowDown, none, &mut ev));
        assert_eq!(scroll(&n), 9.0);
        // Bar: 10 px down moves by 10 × content / free height.
        n.panel.press((x as f32 + 302.0, y as f32 + 5.0), 0, false, &mut ev);
        n.panel.motion((x as f32 + 302.0, y as f32 + 15.0), &mut ev);
        let bar = (100 * 100 / (9 * lines as i32 + 8)).max(32);
        let k = (max / f64::from(100 - bar)).max(1.0);
        assert!((scroll(&n) - (9.0 + 10.0 * k)).abs() < 1e-3, "{} vs {}", scroll(&n), 9.0 + 10.0 * k);
        n.panel.motion((x as f32 + 302.0, y as f32 + 150.0), &mut ev);
        assert_eq!(scroll(&n), max, "below the frame: to the end");
        n.panel.release(&mut ev);
        n.panel.motion((x as f32 + 302.0, y as f32 - 50.0), &mut ev);
        assert_eq!(scroll(&n), max, "released: no more drag");
        // A new layout keeps the scroll.
        n.arrange(&Fixed, &lang(), (400, 201));
        assert_eq!(scroll(&n), max - 1.0, "clamped to the new maximum");
    }

    /// Checkbox label: light grey for the online play warning, default white for the code of conduct.
    #[test]
    fn checkbox_label_colours() {
        let colour = |kind: NoticeKind| {
            let mut n = Notice::new(kind);
            n.arrange(&Fixed, &lang(), (400, 300));
            n.panel.get(CHECK).expect("checkbox").msg[0].style.color
        };
        assert_eq!(colour(NoticeKind::Multiplayer), Some(0xE0E0E0));
        assert_eq!(colour(NoticeKind::Conduct("x".into())), None);
    }
}
