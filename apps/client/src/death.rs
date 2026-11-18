//! Death screen (`docs/specs/hud.md` § 5.6): gradient background, title ×2, cause, score, two
//! buttons active after 20 ticks. "Title Screen": in hardcore mode, direct return to the menu; otherwise
//! confirmation screen on the same background ("Title Screen" / "Respawn"), buttons active
//! after 20 ticks, Esc = "Respawn".

use rc_text::{Arg, Component, StyledChar};

use crate::gui::Gui;

/// Screen button: GUI position and size, label.
struct Button {
    x: i32,
    y: i32,
    w: i32,
    label: Vec<StyledChar>,
    inactive_label: Vec<StyledChar>,
    active: bool,
}

impl Button {
    fn new(label: Vec<StyledChar>, x: i32, y: i32, w: i32) -> Button {
        let mut inactive_label = label.clone();
        for c in &mut inactive_label {
            c.style.color.get_or_insert(0xA0_A0A0);
        }
        Button { x, y, w, label, inactive_label, active: true }
    }

    fn contains(&self, mx: f32, my: f32) -> bool {
        mx >= self.x as f32 && my >= self.y as f32 && mx < (self.x + self.w) as f32 && my < (self.y + 20) as f32
    }

    fn draw(&self, gui: &mut Gui, queue: &wgpu::Queue, mouse: Option<(f32, f32)>) {
        let hovered = mouse.is_some_and(|(mx, my)| self.contains(mx, my));
        let sprite = match (self.active, hovered) {
            (false, _) => "widget/button_disabled",
            (true, true) => "widget/button_highlighted",
            (true, false) => "widget/button",
        };
        gui.sprite(sprite, self.x as f32, self.y as f32, self.w as f32, 20.0, 0xffff_ffff);
        // Label centered between 2 px margins, 9 px line centered vertically.
        let label = if self.active { &self.label } else { &self.inactive_label };
        let lw = rc_text::width(gui, label);
        let (left, right) = (self.x + 2, self.x + self.w - 2);
        let cx = ((left + right) / 2).clamp(left + lw / 2, right - lw / 2);
        let top = (self.y + self.y + 20 - 9) / 2 + 1;
        gui.text(queue, label, (cx - lw / 2) as f32, top as f32, 0xffff_ffff, true, 0);
    }
}

/// Button pressed (each case plays the button sound).
pub enum Action {
    Respawn,
    Quit,
    /// Confirmation open, nothing to send.
    Confirm,
}

pub struct DeathScreen {
    cause: Option<Component>,
    hardcore: bool,
    /// Player score (local player's entity data) at the time of death.
    score: i32,
    delay: i32,
    /// Labels and lines laid out at the first draw (interface language).
    built: Option<Built>,
    respawn_active: bool,
    /// Confirmation of the return to the title screen: ticks left before the buttons activate.
    confirm: Option<i32>,
    /// "Respawn" chosen from the confirmation: the screen is closed while waiting for the server.
    hidden: bool,
}

struct Built {
    title: Vec<StyledChar>,
    cause: Option<Vec<StyledChar>>,
    score: Vec<StyledChar>,
    buttons: [Button; 2],
    confirm_title: Vec<StyledChar>,
    confirm_buttons: [Button; 2],
    width: i32,
    height: i32,
}

impl DeathScreen {
    pub fn new(cause: Option<Component>, hardcore: bool, score: i32) -> Self {
        DeathScreen {
            cause,
            hardcore,
            score,
            delay: 0,
            built: None,
            respawn_active: true,
            confirm: None,
            hidden: false,
        }
    }

    pub fn tick(&mut self) {
        self.delay += 1;
        if let Some(c) = &mut self.confirm {
            *c = (*c - 1).max(0);
        }
    }

    pub fn ticks(&self) -> i32 {
        self.delay
    }

    fn active(&self) -> bool {
        self.delay >= 20
    }

    fn build(&mut self, gui: &Gui) {
        let (w, h) = (gui.width as i32, gui.height as i32);
        if self.built.as_ref().is_some_and(|b| b.width == w && b.height == h) {
            return;
        }
        let line = |c: &Component| rc_text::styled_chars(&rc_text::flatten(c, &gui.lang));
        let key = |k: &str| line(&Component::translate(k, vec![]));
        let title = key(if self.hardcore { "deathScreen.title.hardcore" } else { "deathScreen.title" });
        // Score: the number in yellow.
        let mut value = Component::text(self.score.to_string());
        value.style.color = Some(0xFF_FF55);
        let score = line(&Component::translate("deathScreen.score.value", vec![Arg::Component(value)]));
        let first = if self.hardcore { "deathScreen.spectate" } else { "deathScreen.respawn" };
        // Confirmation: title, empty message (one line), 150 px buttons spaced by 4, 16 px
        // lower; column spaced by 8 centered on screen (height 9 + 8 + 9 + 8 + 16 + 20).
        let top = (h - 70) / 2;
        let x0 = (w - 304) / 2;
        self.built = Some(Built {
            title,
            cause: self.cause.as_ref().map(line),
            score,
            buttons: [
                Button::new(key(first), w / 2 - 100, h / 4 + 72, 200),
                Button::new(key("deathScreen.titleScreen"), w / 2 - 100, h / 4 + 96, 200),
            ],
            confirm_title: key("deathScreen.quit.confirm"),
            confirm_buttons: [
                Button::new(key("deathScreen.titleScreen"), x0, top + 50, 150),
                Button::new(key("deathScreen.respawn"), x0 + 154, top + 50, 150),
            ],
            width: w,
            height: h,
        });
    }

    /// Drawing over the HUD; `mouse` in GUI pixels.
    pub fn draw(&mut self, gui: &mut Gui, queue: &wgpu::Queue, mouse: Option<(f32, f32)>) {
        if self.hidden {
            return;
        }
        self.build(gui);
        let active = self.active();
        let respawn_active = self.respawn_active;
        let confirm = self.confirm;
        let Some(b) = &mut self.built else { return };
        let (w, h) = (gui.width as i32, gui.height as i32);
        gui.stratum(crate::gui::Blend::Normal);
        gui.fill_gradient(0.0, 0.0, w as f32, h as f32, 0x6050_0000, 0xA080_3030);
        if let Some(delay) = confirm {
            for bt in &mut b.confirm_buttons {
                bt.active = delay <= 0;
                bt.draw(gui, queue, mouse);
            }
            crate::widgets::centered(gui, queue, &b.confirm_title, w / 2, (h - 70) / 2, 0xffff_ffff);
            return;
        }
        b.buttons[0].active = active && respawn_active;
        b.buttons[1].active = active;
        for bt in &b.buttons {
            bt.draw(gui, queue, mouse);
        }
        // Texts: title at scale 2 centered on (W/2)/2 in the enlarged frame, then cause, score.
        let mid = w / 2;
        // Centered on the width of the drawn line (visual order), like the game.
        let center = |gui: &mut Gui, chars: &[StyledChar], anchor: i32, y: i32| {
            let x = anchor - gui.visual_width(chars) / 2;
            gui.text(queue, chars, x as f32, y as f32, 0xffff_ffff, true, 0);
        };
        gui.pose = 2.0;
        center(gui, &b.title, mid / 2, 30);
        gui.pose = 1.0;
        if let Some(c) = &b.cause {
            center(gui, c, mid, 85);
        }
        center(gui, &b.score, mid, 100);
    }

    /// Style of the death cause under the mouse (link, tooltip; `links.rs`).
    pub fn cause_style_at(&self, gui: &Gui, m: (f32, f32)) -> Option<rc_text::CharStyle> {
        if self.hidden || self.confirm.is_some() {
            return None;
        }
        let c = self.built.as_ref()?.cause.as_ref()?;
        if !(85.0..94.0).contains(&m.1) {
            return None;
        }
        let x = (gui.width as i32 / 2 - gui.visual_width(c) / 2) as f32;
        crate::links::style_at(gui, c, m.0 - x).cloned()
    }

    /// "Respawn" answer of the confirmation: respawn, screen closed.
    fn confirm_respawn(&mut self) -> Option<Action> {
        self.confirm = None;
        self.hidden = true;
        self.respawn_active = false;
        Some(Action::Respawn)
    }

    /// Esc: only in the confirmation, once the buttons are active.
    pub fn escape(&mut self) -> Option<Action> {
        match self.confirm {
            Some(d) if d <= 0 && !self.hidden => self.confirm_respawn(),
            _ => None,
        }
    }

    /// Left click in GUI pixels.
    pub fn click(&mut self, x: f32, y: f32) -> Option<Action> {
        if self.hidden {
            return None;
        }
        let b = self.built.as_ref()?;
        if let Some(delay) = self.confirm {
            if delay > 0 {
                return None;
            }
            if b.confirm_buttons[0].contains(x, y) {
                return Some(Action::Quit);
            }
            if b.confirm_buttons[1].contains(x, y) {
                return self.confirm_respawn();
            }
            return None;
        }
        let hit = |bt: &Button| bt.active && bt.contains(x, y);
        if hit(&b.buttons[0]) {
            self.respawn_active = false;
            return Some(Action::Respawn);
        }
        if !hit(&b.buttons[1]) {
            return None;
        }
        if self.hardcore {
            return Some(Action::Quit);
        }
        self.confirm = Some(20);
        Some(Action::Confirm)
    }
}

impl crate::App {
    /// Death screen button: button sound, then respawn or return to the menu.
    pub(crate) fn death_action(&mut self, action: Option<Action>) {
        let Some(action) = action else { return };
        if let Some(a) = self.audio.get() {
            a.play(crate::widgets::CLICK_SOUND, rc_audio::Category::Ui, None, 0.25, 1.0, None);
        }
        match action {
            Action::Respawn => {
                let _ = self.session.link.send_client_command(0);
                self.binds.reset_toggles();
            }
            Action::Quit => self.disconnect_to_menu(),
            Action::Confirm => {}
        }
    }

    /// Keyboard during the death screen: Esc only (confirmation).
    pub(crate) fn death_key(&mut self, event: &winit::event::KeyEvent) {
        use winit::keyboard::{KeyCode, PhysicalKey};
        if event.state != winit::event::ElementState::Pressed
            || event.physical_key != PhysicalKey::Code(KeyCode::Escape)
        {
            return;
        }
        let action = self.death.as_mut().and_then(DeathScreen::escape);
        self.death_action(action);
    }

    /// Entity data of the local player: absorption (17) and score (18); critical hits
    /// (`animate` 1 and 2, `world_fx.rs`).
    pub(crate) fn m6_entity_event(&mut self, e: &rc_client::EntityEvent) {
        if let rc_client::EntityEvent::Animate { id, action } = *e {
            self.fx_animate(id, action);
            return;
        }
        let rc_client::EntityEvent::Data { id, values } = e else { return };
        if *id != self.player_id {
            return;
        }
        for (i, v) in values {
            match (i, v) {
                (17, rc_proto_v777::entity::Meta::Float(a)) => self.hud.absorption = *a,
                (18, rc_proto_v777::entity::Meta::Int(s)) => self.hud.score = *s,
                _ => {}
            }
        }
    }
}
