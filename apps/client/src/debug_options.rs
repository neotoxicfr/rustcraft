//! Debug options screen F3+F6 (`debug-f3.md` § 5): search, list of entries
//! by category with their three status buttons, profiles, F3 screen scale.

use rc_text::StyledChar;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};

use crate::debug_screen::{DebugScreen, ENTRIES, Profile, Status};
use crate::gui::Gui;
use crate::widgets::{self, Event, Kind, List, Panel, TextField, Widget, chars, tr};

const FIELD: u32 = 5;
const DEFAULT: u32 = 1;
const PERFORMANCE: u32 = 2;
const SCALE: u32 = 3;
const DONE: u32 = 4;
/// Entry buttons: `ROW + 3·i + {0 OFF, 1 In Overlay, 2 Always}`.
const ROW: u32 = 1000;
const ROW_WIDTH: i32 = 350;
const HEADER: i32 = 61;
const FOOTER: i32 = 33;

pub struct OptionsScreen {
    pub panel: Panel,
    /// UI size, version of the active list and scale setting at the last mount.
    built: Option<(i32, i32, u64, i32, String)>,
}

/// Single-color text.
fn colored(s: &str, rgb: u32) -> Vec<StyledChar> {
    let mut c = crate::gui::plain(s);
    for ch in &mut c {
        ch.style.color = Some(rgb & 0xff_ffff);
    }
    c
}

/// Label of the F3 screen scale: "GUI Scale: Auto".
fn scale_label(lang: &rc_text::Lang, v: i32) -> String {
    let value = match v {
        -1 => tr(lang, "options.debugGuiScale.unchanged", &[]),
        0 => tr(lang, "options.guiScale.auto", &[]),
        n => n.to_string(),
    };
    tr(lang, "options.generic_value", &[tr(lang, "options.guiScale", &[]), value])
}

impl OptionsScreen {
    /// Mounting of the components (§ 5.1); the search field and the scroll are kept.
    fn build(&mut self, gui: &Gui, d: &DebugScreen, scale: i32) {
        let (w, h) = (gui.width as i32, gui.height as i32);
        let lang = &gui.lang;
        let t = |k: &str| tr(lang, k, &[]);
        let old_field = self.panel.widgets.iter().find(|x| x.id == FIELD).cloned();
        let scroll = self.panel.list.as_ref().map_or(0.0, |l| l.scroll);
        let focus = self.panel.focus;
        let mut widgets = Vec::new();
        // Header: space (1/3 of the list), title, search field (1/3, height 20).
        let title = chars(&t("debug.options.title"));
        let tw = rc_text::width(gui, &title);
        let third = ROW_WIDTH / 3;
        let row_w = third + 8 + tw + 8 + third;
        let x0 = (w - row_w) / 2;
        let field = old_field.unwrap_or_else(|| {
            let hint = chars(&format!("§7§o{}", t("debug.options.search")));
            Widget::new(FIELD, Kind::Field(Box::new(TextField::new(hint))), 0, 0, third, 20, Vec::new())
        });
        let search = match &field.kind {
            Kind::Field(f) => f.value(),
            _ => String::new(),
        };
        let mut field = field;
        (field.x, field.y) = (x0 + third + 8 + tw + 8, 7);
        widgets.push(field);
        let mut tl =
            Widget::new(0, Kind::Label { color: 0xffff_ffff, centered: false }, x0 + third + 8, 13, tw, 9, title);
        tl.active = true;
        widgets.push(tl);
        // Red subtitle, cut to the width of the list, centered.
        let warn = chars(&t("debug.options.warning"));
        for (k, line) in rc_text::wrap(gui, &warn, ROW_WIDTH as f32).into_iter().enumerate() {
            widgets.push(Widget::new(
                0,
                Kind::Label { color: 0xffdf_5050, centered: true },
                0,
                35 + 9 * k as i32,
                w,
                9,
                line,
            ));
        }
        // List: text then renders, by id; category row at each change.
        let mut list = List::new(0, HEADER, w, h - HEADER - FOOTER, ROW_WIDTH, 20);
        let left = list.row_left();
        let mut top = 0;
        let mut cat: Option<bool> = None;
        let mut order: Vec<usize> = (0..ENTRIES.len()).filter(|&i| ENTRIES[i].id.contains(search.as_str())).collect();
        order.sort_by_key(|&i| (!ENTRIES[i].text, ENTRIES[i].id));
        let reduced = d.reduced();
        for i in order {
            let e = &ENTRIES[i];
            if cat != Some(e.text) {
                cat = Some(e.text);
                let key = if e.text { "debug.options.category.text" } else { "debug.options.category.renderer" };
                let mut lw = Widget::new(
                    0,
                    Kind::Label { color: 0xffff_ffff, centered: true },
                    0,
                    2 + top + 6,
                    w,
                    9,
                    chars(&format!("§l§n{}", t(key))),
                );
                lw.in_list = true;
                widgets.push(lw);
                top += 20;
            }
            let allowed = !reduced || e.reduced;
            let name = if allowed { chars(e.id) } else { chars(&format!("§o{}", e.id)) };
            let mut nl = Widget::new(
                0,
                Kind::Label { color: if allowed { 0xffff_ffff } else { 0xff80_8080 }, centered: false },
                left,
                2 + top + 5,
                150,
                9,
                name,
            );
            if !allowed {
                nl.set_tooltip(gui, &t("debug.options.notAllowed.tooltip"));
            }
            nl.in_list = true;
            widgets.push(nl);
            let status = d.status_of(i);
            let bx = left + ROW_WIDTH - 180;
            for (k, (st, label, color)) in [
                (Status::Never, t("options.off"), 0xffff_ffff),
                (Status::InOverlay, t("debug.entry.overlay"), 0xffff_ff55),
                (Status::AlwaysOn, t("debug.entry.always"), 0xffdf_5050),
            ]
            .into_iter()
            .enumerate()
            {
                let current = st == status;
                let msg = colored(&label, if current { color } else { 0xffba_baba });
                let mut b =
                    Widget::new(ROW + 3 * i as u32 + k as u32, Kind::Button, bx + 60 * k as i32, 2 + top, 60, 16, msg);
                b.active = !current;
                b.in_list = true;
                widgets.push(b);
            }
            top += 20;
        }
        list.content = top + 4;
        list.set_scroll(scroll);
        // Footer: profiles (120), scale (110), Done (60), gap 4.
        let fw = 120 + 4 + 120 + 4 + 110 + 4 + 60;
        let fx = (w - fw) / 2;
        let fy = h - FOOTER + (FOOTER - 20) / 2;
        let mut def = Widget::button(DEFAULT, fx, fy, 120, &t("debug.options.profile.default"));
        def.active = d.profile != Some(Profile::Default);
        let mut perf = Widget::button(PERFORMANCE, fx + 124, fy, 120, &t("debug.options.profile.performance"));
        perf.active = d.profile != Some(Profile::Performance);
        let max = crate::gui::auto_scale((gui.width * gui.scale as f32) as u32, (gui.height * gui.scale as f32) as u32)
            as i32;
        let pos = f64::from(scale.clamp(-1, max) + 1) / f64::from(max + 1);
        let sl = Widget::new(
            SCALE,
            Kind::Slider { value: pos, steps: (max + 1) as u32, adjusting: false },
            fx + 248,
            fy,
            110,
            20,
            chars(&scale_label(lang, scale)),
        )
        .with_tooltip(gui, &t("options.debugGuiScale.tooltip"));
        widgets.extend([def, perf, sl, Widget::button(DONE, fx + 362, fy, 60, &t("gui.done"))]);
        let keyboard = self.panel.keyboard;
        self.panel = Panel::new(widgets);
        self.panel.list = Some(list);
        self.panel.keyboard = keyboard;
        self.panel.focus = focus.or(Some(0));
        self.built = Some((w, h, d.version, 0, search));
    }
}

impl DebugScreen {
    pub fn options_open(&self) -> bool {
        self.options_screen.is_some()
    }

    /// Drawing of the screen (above the F3 screen); rebuilt if the size, the statuses,
    /// the scale or the search changed.
    pub fn draw_options(&mut self, gui: &mut Gui, q: &wgpu::Queue, mouse: Option<(f32, f32)>, scale: i32, time: f64) {
        let Some(mut s) = self.options_screen.take() else { return };
        let search = s.panel.widgets.iter().find_map(|w| match &w.kind {
            Kind::Field(f) if w.id == FIELD => Some(f.value()),
            _ => None,
        });
        // Scale excluded from the key: the slider updates in place (continuous drag).
        let key = (gui.width as i32, gui.height as i32, self.version, 0, search.unwrap_or_default());
        if s.built.as_ref() != Some(&key) {
            s.build(gui, self, scale);
        }
        if let Some(w) = s.panel.get_mut(SCALE) {
            w.msg = chars(&scale_label(&gui.lang, scale));
        }
        gui.next_layer();
        widgets::menu_background(gui, self.menu_blur, true);
        gui.next_layer();
        s.panel.draw(gui, q, mouse, time);
        self.options_screen = Some(s);
    }
}

impl crate::App {
    /// F3+F6: closes the options screen if it is open, otherwise closes the current screen and opens it.
    pub(crate) fn debug_options_toggle(&mut self) {
        if self.debug.options_screen.take().is_some() {
            self.grab(true);
            return;
        }
        if self.menus.is_open() {
            self.menus.close_all();
        }
        if self.screens.open {
            self.close_screen();
        }
        self.debug.switcher = None;
        let mut s = OptionsScreen { panel: Panel::default(), built: None };
        s.panel.keyboard = true;
        self.debug.options_screen = Some(s);
        self.keys.clear();
        self.grab(false);
    }

    /// Keyboard of the options screen; true (consumed) as long as it is open.
    pub(crate) fn debug_options_key(&mut self, event: &winit::event::KeyEvent) -> bool {
        if !self.debug.options_open() {
            return false;
        }
        if event.state != ElementState::Pressed {
            return true;
        }
        if event.physical_key == PhysicalKey::Code(KeyCode::Escape) {
            self.debug.options_screen = None;
            self.grab(true);
            return true;
        }
        let mut ev = Vec::new();
        if let (Some(s), PhysicalKey::Code(k)) = (&mut self.debug.options_screen, event.physical_key) {
            s.panel.key(k, self.mods, &mut ev);
            if let Some(t) = crate::widgets::typed_text(event, self.mods) {
                s.panel.typed(t, &mut ev);
            }
        }
        self.debug_options_events(ev);
        true
    }

    /// Mouse of the options screen (click, wheel, movement); true if consumed.
    pub(crate) fn debug_options_mouse(&mut self, e: &WindowEvent) -> bool {
        if let WindowEvent::CursorMoved { position, .. } = e {
            self.cursor = Some((position.x, position.y));
        }
        if !self.debug.options_open() {
            return false;
        }
        let Some(m) = self.gui_mouse() else { return true };
        let mut ev = Vec::new();
        let shift = self.mods.shift_key();
        let Some(s) = &mut self.debug.options_screen else { return true };
        match e {
            WindowEvent::MouseInput { state, button, .. } => {
                let b = match button {
                    MouseButton::Left => 0,
                    MouseButton::Right => 1,
                    MouseButton::Middle => 2,
                    _ => return true,
                };
                if *state == ElementState::Pressed {
                    s.panel.press(m, b, shift, &mut ev);
                } else {
                    s.panel.release(&mut ev);
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let y = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => f64::from(*y),
                    winit::event::MouseScrollDelta::PixelDelta(p) => p.y / 20.0,
                };
                s.panel.wheel(m, y, &mut ev);
            }
            WindowEvent::CursorMoved { .. } => s.panel.motion(m, &mut ev),
            _ => {}
        }
        self.debug_options_events(ev);
        true
    }

    /// Simulated left click (mouse-less driving).
    pub(crate) fn debug_options_click(&mut self, m: (f32, f32)) {
        let mut ev = Vec::new();
        if let Some(s) = &mut self.debug.options_screen {
            s.panel.motion(m, &mut ev);
            s.panel.press(m, 0, false, &mut ev);
            s.panel.release(&mut ev);
        }
        self.debug_options_events(ev);
    }

    pub(crate) fn debug_options_events(&mut self, ev: Vec<Event>) {
        if let Some(s) = &mut self.debug.options_screen {
            let clicks = std::mem::take(&mut s.panel.clicks);
            if let Some(a) = self.audio.get() {
                for _ in 0..clicks {
                    a.play(widgets::CLICK_SOUND, rc_audio::Category::Ui, None, 0.25, 1.0, None);
                }
            }
        }
        let max = self.renderer.as_ref().map_or(1, |r| {
            let (w, h) = r.size();
            crate::gui::auto_scale(w, h) as i32
        });
        let mut save = false;
        for e in ev {
            match e {
                Event::Press(DONE) => {
                    self.debug.options_screen = None;
                    self.grab(true);
                    return;
                }
                Event::Press(DEFAULT) => self.debug.load_profile(Profile::Default),
                Event::Press(PERFORMANCE) => self.debug.load_profile(Profile::Performance),
                Event::Press(id) if id >= ROW => {
                    let i = ((id - ROW) / 3) as usize;
                    let st = [Status::Never, Status::InOverlay, Status::AlwaysOn][((id - ROW) % 3) as usize];
                    if i < ENTRIES.len() {
                        self.debug.set_status(i, st);
                    }
                }
                Event::Slide(SCALE, p) => {
                    let v = ((p * f64::from(max + 1)).round() as i32 - 1).clamp(-1, max);
                    self.options.set("debugGuiScale", crate::options::Val::I(v));
                }
                Event::Step(SCALE, dir) => {
                    let v = (self.options.i("debugGuiScale") + dir).clamp(-1, max);
                    self.options.set("debugGuiScale", crate::options::Val::I(v));
                    save = true;
                }
                Event::SlideEnd(SCALE) => save = true,
                _ => {}
            }
        }
        if save {
            self.save_options();
        }
    }
}
