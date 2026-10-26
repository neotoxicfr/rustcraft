//! Wiring of advancements and statistics into the application (`advancements-stats.md`): packets,
//! notifications, screens (input, tick, draw).

use rc_client::progress::Progress;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};

use crate::App;
use crate::advancements::Advancements;
use crate::advancements_screen::{self as adv_screen, AdvancementsScreen, Cursor};
use crate::gui::Gui;
use crate::hud::HotbarItems;
use crate::stats::Stats;
use crate::stats_screen::{self, StatsScreen};

/// Open screen.
pub(crate) enum Screen {
    Advancements(Box<AdvancementsScreen>),
    Stats(Box<StatsScreen>),
}

/// Draw data taken from the application.
pub(crate) struct DrawCtx<'a> {
    pub assets: &'a rc_assets::Assets,
    pub blur: i32,
    pub time: f64,
}

/// Input forwarded to the open screen (interface units).
enum Input {
    /// Key, and true if it is the advancements key.
    Key(KeyCode, bool),
    Press((f32, f32), u8),
    Release,
    Wheel((f32, f32), f64, f64),
    Motion((f32, f32)),
}

/// State of the session's advancements and statistics.
#[derive(Default)]
pub(crate) struct Progression {
    pub adv: Advancements,
    pub stats: Stats,
    pub screen: Option<Screen>,
    /// Cursor shape applied to the window.
    cursor: Option<Cursor>,
}

impl Progression {
    /// New connection: empty tree and counters.
    pub fn join(&mut self) {
        self.adv.clear();
        self.stats = Stats::default();
        self.screen = None;
    }

    pub fn draw(
        &mut self,
        gui: &mut Gui,
        q: &wgpu::Queue,
        it: &HotbarItems<'_>,
        ctx: &DrawCtx<'_>,
        mouse: Option<(f32, f32)>,
    ) {
        match &mut self.screen {
            Some(Screen::Advancements(s)) => s.draw(gui, q, it, &mut self.adv, ctx.assets, ctx.blur, mouse, ctx.time),
            Some(Screen::Stats(s)) => s.draw(gui, q, it, &self.stats, ctx.blur, mouse, ctx.time),
            None => {}
        }
    }

    /// Input to the open screen; true if it asks to go back.
    fn input(&mut self, gui: &Gui, i: Input) -> bool {
        match &mut self.screen {
            Some(Screen::Advancements(s)) => {
                let a = match i {
                    Input::Key(k, l) => s.key(gui, k, l),
                    Input::Press(m, b) => s.press(gui, &mut self.adv, m, b),
                    Input::Release => {
                        s.release();
                        None
                    }
                    Input::Wheel(_, dx, dy) => {
                        s.wheel(&mut self.adv, dx, dy);
                        None
                    }
                    Input::Motion(m) => {
                        s.motion(gui, &mut self.adv, m);
                        None
                    }
                };
                a == Some(adv_screen::Action::Back)
            }
            Some(Screen::Stats(s)) => {
                let a = match i {
                    Input::Key(k, _) => s.key(gui, k),
                    Input::Press(m, b) => s.press(gui, m, b),
                    Input::Release => {
                        s.release();
                        None
                    }
                    Input::Wheel(m, _, dy) => {
                        s.wheel(gui, m, dy);
                        None
                    }
                    Input::Motion(m) => {
                        s.motion(gui, m);
                        None
                    }
                };
                a == Some(stats_screen::Action::Back)
            }
            None => false,
        }
    }
}

fn button(b: MouseButton) -> Option<u8> {
    match b {
        MouseButton::Left => Some(0),
        MouseButton::Right => Some(1),
        MouseButton::Middle => Some(2),
        _ => None,
    }
}

impl App {
    /// Advancement and statistics packets (§ 1.2, § 2.2).
    pub(crate) fn on_progress(&mut self, p: Progress) {
        match p {
            Progress::Advancements(u) => {
                if rc_data::env_flag!("RUSTCRAFT_ADVLOG") {
                    eprintln!(
                        "advancements: reset {}; {} additions, {} removals, {} advancements",
                        u.reset,
                        u.added.len(),
                        u.removed.len(),
                        u.progress.len()
                    );
                }
                for e in self.progress.adv.apply(*u) {
                    self.toasts.advancement(e.title, e.frame, e.icon);
                }
            }
            Progress::SelectTab(id) => self.progress.adv.select_tab(id),
            Progress::Stats(v) => {
                if rc_data::env_flag!("RUSTCRAFT_ADVLOG") {
                    eprintln!("statistics: {} values {v:?}", v.len());
                }
                self.progress.stats.award(&v);
            }
        }
    }

    fn open_progress_screen(&mut self, s: Screen) {
        self.progress.screen = Some(s);
        self.keys.clear();
        self.grab(false);
        self.progress_flush();
    }

    /// Opening of the advancements screen: from the pause menu (`parent`) or the L key.
    pub(crate) fn open_advancements(&mut self, parent: bool, keyboard: bool) {
        let s = AdvancementsScreen::new(&mut self.progress.adv, parent, keyboard);
        self.open_progress_screen(Screen::Advancements(Box::new(s)));
    }

    /// Opening of the statistics from the pause menu: request to the server (§ 2.1).
    pub(crate) fn open_stats(&mut self, keyboard: bool) {
        let _ = self.session.link.send_client_command(1);
        let s = StatsScreen::new(&self.progress.stats, true, keyboard);
        self.open_progress_screen(Screen::Stats(Box::new(s)));
    }

    /// Leaving the screen: "screen closed" (advancements), back to the parent (pause menu) or to the game.
    fn close_progress_screen(&mut self) {
        let Some(s) = self.progress.screen.take() else { return };
        let parent = match &s {
            Screen::Advancements(a) => {
                let _ = self.session.link.send_seen_advancements(None);
                a.parent
            }
            Screen::Stats(s) => s.parent,
        };
        self.set_progress_cursor(Cursor::Default);
        if !parent || !self.menus.is_open() {
            self.menus.close_all();
            self.grab(true);
        }
    }

    /// Pending packets, click sounds, cursor.
    pub(crate) fn progress_flush(&mut self) {
        let (out, clicks, cursor) = match &mut self.progress.screen {
            Some(Screen::Advancements(s)) => (std::mem::take(&mut s.outgoing), std::mem::take(&mut s.clicks), s.cursor),
            Some(Screen::Stats(s)) => (Vec::new(), std::mem::take(&mut s.clicks), Cursor::Default),
            None => return,
        };
        for tab in out {
            let _ = self.session.link.send_seen_advancements(tab.as_deref());
        }
        if let Some(a) = self.audio.get() {
            for _ in 0..clicks {
                a.play(crate::widgets::CLICK_SOUND, rc_audio::Category::Ui, None, 0.25, 1.0, None);
            }
        }
        self.set_progress_cursor(cursor);
    }

    fn set_progress_cursor(&mut self, c: Cursor) {
        if self.progress.cursor == Some(c) {
            return;
        }
        self.progress.cursor = Some(c);
        let Some(r) = &self.renderer else { return };
        use winit::window::CursorIcon as I;
        let icon = match c {
            Cursor::Default => I::Default,
            Cursor::Hand => I::Pointer,
            Cursor::Move => I::Move,
            Cursor::MoveH => I::EwResize,
            Cursor::MoveV => I::NsResize,
        };
        r.window.set_cursor(icon);
    }

    /// Tick (20 Hz) of the open screen.
    pub(crate) fn progress_tick(&mut self) {
        let mouse = self.gui_mouse();
        let Some(r) = &self.renderer else { return };
        match &mut self.progress.screen {
            Some(Screen::Advancements(s)) => s.tick(&r.gui, &mut self.progress.adv, mouse),
            Some(Screen::Stats(_)) => {}
            None => return,
        }
        self.progress_flush();
    }

    /// Input to the open screen, then possible return.
    fn progress_input(&mut self, i: Input) {
        let Some(r) = &self.renderer else { return };
        if self.progress.input(&r.gui, i) {
            self.close_progress_screen();
        } else {
            self.progress_flush();
        }
    }

    /// Demo (`RUSTCRAFT_ACTTEST`): `progadv` opens the advancements, `progpause` the pause
    /// menu, `progmouse:x:y` places the mouse (interface units), `progclick:x:y` left click
    /// (open screen or pause menu), `progdrag:x0:y0:x1:y1` drag, `progwheel:x:y:dy` wheel,
    /// `progkey:esc|l` key.
    pub(crate) fn progress_script(&mut self, step: &[String]) {
        let n = |i: usize| step.get(i).and_then(|v| v.parse::<f32>().ok()).unwrap_or(0.0);
        let scale = self.renderer.as_ref().map_or(1.0, |r| f64::from(r.gui.scale));
        let put = |app: &mut App, x: f32, y: f32| {
            app.cursor = Some((f64::from(x) * scale, f64::from(y) * scale));
            app.progress_input(Input::Motion((x, y)));
        };
        match step[0].as_str() {
            "progadv" => self.open_advancements(false, false),
            "progpause" => self.open_pause(false),
            "progmouse" => put(self, n(1), n(2)),
            "progclick" | "progdrag" => {
                put(self, n(1), n(2));
                if self.progress.screen.is_none() {
                    let Some(r) = &self.renderer else { return };
                    let gui = &r.gui;
                    let mut a = self.menus.press(gui, &mut self.options, (n(1), n(2)), 0, false);
                    a.extend(self.menus.release(gui, &mut self.options));
                    self.menu_actions(a);
                    return;
                }
                self.progress_input(Input::Press((n(1), n(2)), 0));
                if step[0] == "progdrag" {
                    // First move: arming; then movement in two steps.
                    put(self, n(1) + 1.0, n(2));
                    put(self, (n(1) + n(3)) / 2.0, (n(2) + n(4)) / 2.0);
                    put(self, n(3), n(4));
                }
                self.progress_input(Input::Release);
            }
            "progwheel" => self.progress_input(Input::Wheel((n(1), n(2)), 0.0, f64::from(n(3)))),
            "progkey" => {
                let l = step.get(1).is_some_and(|k| k == "l");
                let code = if l { KeyCode::KeyL } else { KeyCode::Escape };
                self.progress_input(Input::Key(code, l));
            }
            _ => {}
        }
    }

    /// Input: open screen (everything goes to it), or the advancements key in game.
    pub(crate) fn progress_window_event(&mut self, event: &WindowEvent) -> bool {
        let is_adv_key = |app: &App, code: KeyCode| {
            crate::options::key_name(code) == app.options.s("key_key.advancements")
        };
        if self.progress.screen.is_none() {
            // Advancements key (L) in game, with no other screen.
            if let WindowEvent::KeyboardInput { event: k, .. } = event
                && k.state == ElementState::Pressed
                && !k.repeat
                && let PhysicalKey::Code(code) = k.physical_key
                && is_adv_key(self, code)
                && self.front.in_world
                && !self.front.is_open()
                && !self.menus.is_open()
                && !self.screens.open
                && !self.creative_ui.open
                && self.chat.input.is_none()
                && self.death.is_none()
                && self.exact.dialog.is_none()
            {
                self.open_advancements(false, false);
                return true;
            }
            return false;
        }
        let mouse = self.gui_mouse();
        if let Some(Screen::Stats(s)) = &mut self.progress.screen {
            s.mods = self.mods;
        }
        let input = match event {
            WindowEvent::KeyboardInput { event: k, .. } => {
                let PhysicalKey::Code(code) = k.physical_key else { return true };
                if k.state != ElementState::Pressed {
                    return true;
                }
                Input::Key(code, is_adv_key(self, code))
            }
            WindowEvent::MouseInput { state, button: b, .. } => {
                let (Some(b), Some(m)) = (button(*b), mouse) else { return true };
                if *state == ElementState::Pressed { Input::Press(m, b) } else { Input::Release }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let (dx, dy) = match delta {
                    MouseScrollDelta::LineDelta(x, y) => (f64::from(*x), f64::from(*y)),
                    MouseScrollDelta::PixelDelta(p) => (p.x / 20.0, p.y / 20.0),
                };
                let Some(m) = mouse else { return true };
                Input::Wheel(m, dx, dy)
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = Some((position.x, position.y));
                let Some(m) = self.gui_mouse() else { return true };
                Input::Motion(m)
            }
            _ => return false,
        };
        self.progress_input(input);
        true
    }
}
