//! Bed, spyglass and remaining game events (`docs/specs/end-sleep-spectator.md`):
//! local player lying down (position in the bed, camera turned toward the foot of the bed, fade to black),
//! "in bed" chat with the "Leave Bed" button, field of view of the bow and of the
//! spyglass, game events 0, 4, 5, 6, 9, 10, 12.

use rc_proto_v777::entity::Meta;
use rc_text::Component;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};

use crate::gui::Gui;

#[derive(Default)]
pub struct Bed {
    /// The open chat is the bed's (Esc = get up).
    pub in_bed_chat: bool,
    /// Bed where the player was placed (once per lying down).
    placed: Option<[i32; 3]>,
    /// Event 12: crafting limited to known recipes (no visible effect on the client).
    pub limited_crafting: bool,
    /// Keyboard-less test (`RUSTCRAFT_M5TEST="wait:100;spec:1;mid;wheel:1;win:space:1;bedclick"`),
    /// current step and wait.
    test: Option<(Vec<Vec<String>>, usize, u32)>,
    test_read: bool,
}

/// "Leave Bed" button: 200 × 20 centered, 40 px above the bottom.
fn button_rect(gui_w: f32, gui_h: f32) -> [f32; 4] {
    let x = (gui_w as i32 / 2 - 100) as f32;
    let y = gui_h - 40.0;
    [x, y, x + 200.0, y + 20.0]
}

/// Demo help (game event 5, values 101 to 104): keys set by the player,
/// named by `key` (name of the options key, e.g. `key.forward`).
fn demo_message(param: f32, key: impl Fn(&str) -> String) -> Option<Component> {
    let keys = |names: &[&str]| names.iter().map(|n| rc_text::Arg::Text(key(n))).collect();
    let (msg, names): (&str, &[&str]) = match param {
        101.0 => ("demo.help.movement", &["key.forward", "key.left", "key.back", "key.right"]),
        102.0 => ("demo.help.jump", &["key.jump"]),
        103.0 => ("demo.help.inventory", &["key.inventory"]),
        104.0 => ("demo.day.6", &["key.screenshot"]),
        _ => return None,
    };
    Some(Component::translate(msg, keys(names)))
}

#[test]
fn demo_help_names_the_bound_keys() {
    let key = |k: &str| format!("<{k}>");
    let arg = |k: &str| rc_text::Arg::Text(format!("<{k}>"));
    assert_eq!(
        demo_message(101.0, key),
        Some(Component::translate(
            "demo.help.movement",
            vec![arg("key.forward"), arg("key.left"), arg("key.back"), arg("key.right")]
        ))
    );
    assert_eq!(demo_message(104.0, key), Some(Component::translate("demo.day.6", vec![arg("key.screenshot")])));
    assert_eq!(demo_message(100.0, key), None);
}

impl Bed {
    /// Button drawing (in-bed chat open); `mouse` in GUI pixels.
    pub fn draw(&self, gui: &mut Gui, queue: &wgpu::Queue, mouse: Option<(f32, f32)>, chat_open: bool) {
        if !(self.in_bed_chat && chat_open) {
            return;
        }
        let r = button_rect(gui.width, gui.height);
        let hovered = mouse.is_some_and(|(x, y)| x >= r[0] && y >= r[1] && x < r[2] && y < r[3]);
        gui.next_layer();
        let sprite = if hovered { "widget/button_highlighted" } else { "widget/button" };
        gui.sprite(sprite, r[0], r[1], 200.0, 20.0, 0xffff_ffff);
        let c = Component::translate("multiplayer.stopSleeping", vec![]);
        let label = rc_text::styled_chars(&rc_text::flatten(&c, &gui.lang));
        let lw = rc_text::width(gui, &label);
        let (left, right) = (r[0] as i32 + 2, r[2] as i32 - 2);
        let cx = ((left + right) / 2).clamp(left + lw / 2, right - lw / 2);
        let top = (r[1] as i32 * 2 + 20 - 9) / 2 + 1;
        gui.text(queue, &label, (cx - lw / 2) as f32, top as f32, 0xffff_ffff, true, 0);
    }
}

impl crate::App {
    /// Bed occupied by the local player (data 14 of the living entity).
    pub(crate) fn sleeping_pos(&self) -> Option<[i32; 3]> {
        match self.entities.me.as_ref()?.meta.get(&14) {
            Some(Meta::BlockPos(Some(p))) => Some(*p),
            _ => None,
        }
    }

    /// Spyglass in use, seen in first person.
    pub(crate) fn scoping_first_person(&self) -> bool {
        self.hud.more.scoping && !self.camera.view.detached() && !self.free_cam
    }

    /// Hand hidden: lying down, spyglass in use, end screen.
    pub(crate) fn m5_hide_hand(&self) -> bool {
        self.sleeping_pos().is_some() || self.scoping_first_person() || self.win.is_some()
    }

    fn any_screen(&self) -> bool {
        self.chat.input.is_some()
            || self.screens.open
            || self.menus.is_open()
            || self.death.is_some()
            || self.win.is_some()
            || self.creative_ui.open
            || self.exact.dialog.is_some()
            || self.special.overlay.is_some()
            || self.progress.screen.is_some()
    }

    /// One tick: sleep counter, player placed in the bed, in-bed chat, item in use.
    pub(crate) fn bed_tick(&mut self) {
        self.m5_test_tick();
        let pos = self.sleeping_pos();
        self.hud.more.tick_sleep(pos.is_some());
        if self.env.game_mode != 3 && self.spectator.active() {
            self.spectator.close();
        }
        match pos {
            Some(p) if self.bed.placed != Some(p) => {
                // Feet at the center of the bed, at its height + 0.125.
                let view = crate::environ::WorldView {
                    grid: &self.world.grid,
                    min_y: self.world.min_y,
                    height: self.world.height,
                };
                let state = view.state(p[0], p[1], p[2]);
                let top = self
                    .data
                    .state(state)
                    .map(|s| self.data.shape(s.outline).iter().map(|b| b.max[1]).fold(f64::NAN, f64::max))
                    .filter(|h| h.is_finite());
                if let Some(h) = top {
                    let feet = [f64::from(p[0]) + 0.5, f64::from(p[1]) + h + 0.125, f64::from(p[2]) + 0.5];
                    self.player.pos = feet;
                    self.player.vel = [0.0; 3];
                    self.prev_pos = feet;
                    self.bed.placed = Some(p);
                }
            }
            None => self.bed.placed = None,
            _ => {}
        }
        // In-bed chat: opened when nothing else is; on waking, kept only if it
        // holds typed text (a draft resumed as is closes too), otherwise closed.
        if self.bed.in_bed_chat {
            if self.chat.input.is_none() {
                self.bed.in_bed_chat = false;
            } else if pos.is_none() {
                self.bed.in_bed_chat = false;
                if self.chat.input.as_ref().is_some_and(crate::chat::Input::closes_on_wake) {
                    self.cmd.close(&mut self.chat);
                    self.chat.close();
                    self.grab(true);
                }
            }
        } else if pos.is_some() && !self.any_screen() && self.positioned {
            self.chat.open("");
            self.commands_refresh();
            self.cmd.open(&mut self.chat);
            self.commands_send();
            self.keys.clear();
            self.grab(false);
            self.bed.in_bed_chat = true;
        }
        // Item in use: field of view (bow, spyglass) and spyglass overlay.
        let using = self
            .act
            .using
            .as_ref()
            .map(|u| (self.item_names.get(u.item as usize).map_or("", String::as_str), u.duration - u.remaining));
        self.camera.use_fov = match using {
            Some(("minecraft:bow", t)) => crate::camera::UseFov::Bow(t),
            Some(("minecraft:spyglass", _)) => crate::camera::UseFov::Spyglass,
            _ => crate::camera::UseFov::None,
        };
        self.hud.more.scoping = matches!(using, Some(("minecraft:spyglass", _)));
    }

    /// Step of the test script `RUSTCRAFT_M5TEST` (one per tick, after `wait:n`).
    pub(crate) fn m5_test_tick(&mut self) {
        if !self.bed.test_read {
            self.bed.test_read = true;
            self.bed.test = rc_data::env_str!("RUSTCRAFT_M5TEST").map(|s| {
                let steps = s.split(';').filter(|x| !x.is_empty()).map(|x| x.split(':').map(str::to_owned).collect());
                (steps.collect(), 0, 0)
            });
        }
        let Some((steps, at, wait)) = &mut self.bed.test else { return };
        if *wait > 0 {
            *wait -= 1;
            return;
        }
        let Some(step) = steps.get(*at).cloned() else { return };
        *at += 1;
        let arg = |i: usize| step.get(i).map_or("", String::as_str);
        let num = |i: usize| arg(i).parse::<i32>().unwrap_or(0);
        eprintln!("test M5: {}", step.join(":"));
        match step[0].as_str() {
            "wait" => *wait = num(1) as u32,
            "spec" => self.spectator_hotbar((num(1) - 1).clamp(0, 8) as usize),
            "mid" => {
                let a = self.spectator.action_key(&self.players, &self.scores);
                self.spectator_act(a);
            }
            "wheel" => self.spectator.scroll(num(1)),
            // End screen key: `win:space:1` (press) / `win:space:0` (release).
            "win" => {
                let k = match arg(1) {
                    "space" => KeyCode::Space,
                    "up" => KeyCode::ArrowUp,
                    "ctrl" => KeyCode::ControlLeft,
                    _ => KeyCode::Escape,
                };
                if let Some(w) = &mut self.win
                    && w.key(k, num(2) == 1)
                {
                    self.close_win_screen();
                }
            }
            "bedbutton" if self.bed.in_bed_chat => self.wake_up(),
            "bedesc" if self.bed.in_bed_chat => self.wake_up(),
            "cmd" => {
                let _ = self.session.link.send_command(&step[1..].join(":"));
            }
            "f5" => self.camera.view = self.camera.view.next(),
            // Capture now (with `RUSTCRAFT_CAPTURE`), then quit.
            "shot" => {
                let now = self.started.elapsed().as_secs_f64();
                if let Some(c) = &mut self.capture {
                    c.1 = now;
                }
            }
            _ => {}
        }
    }

    /// Lying down in first person: camera at the eyes (0.2) + 0.15, turned toward the foot of the bed.
    pub(crate) fn bed_shot(&self) -> Option<crate::camera::Shot> {
        if self.camera.view.detached() {
            return None;
        }
        let p = self.sleeping_pos()?;
        let view =
            crate::environ::WorldView { grid: &self.world.grid, min_y: self.world.min_y, height: self.world.height };
        let state = view.state(p[0], p[1], p[2]);
        let facing = self.data.state_properties(state).find(|(n, _)| *n == "facing").map(|(_, v)| v);
        let yrot = match facing? {
            "south" => 0.0,
            "west" => 90.0,
            "north" => 180.0,
            _ => 270.0,
        };
        let f = self.player.pos;
        Some(crate::camera::Shot { pos: [f[0], f[1] + 0.2 + 0.15, f[2]], yaw: yrot - 180.0, pitch: 0.0 })
    }

    /// Entity event of the local player: wake-up (animation 0) → fade-out.
    pub(crate) fn bed_entity_event(&mut self, e: &rc_client::EntityEvent) {
        if let rc_client::EntityEvent::Animate { id, action: 0 } = *e
            && id == self.player_id
        {
            self.hud.more.sleep_timer = 100;
        }
    }

    /// In-bed chat: Esc requests waking up (the chat closes when the server gets us up).
    pub(crate) fn bed_key(&mut self, e: &WindowEvent) -> bool {
        let WindowEvent::KeyboardInput { event, .. } = e else { return false };
        if !self.bed.in_bed_chat || self.chat.input.is_none() {
            return false;
        }
        if event.state == ElementState::Pressed && event.physical_key == PhysicalKey::Code(KeyCode::Escape) {
            self.wake_up();
            return true;
        }
        false
    }

    /// Click on "Leave Bed".
    pub(crate) fn bed_mouse(&mut self, e: &WindowEvent) -> bool {
        let WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } = e else {
            return false;
        };
        if !self.bed.in_bed_chat || self.chat.input.is_none() {
            return false;
        }
        let (Some(r), Some((x, y))) = (&self.renderer, self.gui_mouse()) else { return false };
        let b = button_rect(r.gui.width, r.gui.height);
        if x >= b[0] && y >= b[1] && x < b[2] && y < b[3] {
            if let Some(a) = self.audio.get() {
                a.play("minecraft:ui.button.click", rc_audio::Category::Ui, None, 0.25, 1.0, None);
            }
            self.wake_up();
            return true;
        }
        false
    }

    fn wake_up(&mut self) {
        let _ = self.session.link.send_player_command(rc_client::link::action::STOP_SLEEPING);
    }

    /// Game events with no effect on the weather (`main.rs`: `GameEvent` arm).
    pub(crate) fn m5_game_event(&mut self, kind: u8, param: f32) {
        let eye = [self.player.pos[0], self.player.pos[1] + self.player.eye_height(), self.player.pos[2]];
        let feet = self.player.pos;
        let mut sound = |s: &str, cat, pos, vol, pitch| {
            if let Some(a) = self.audio.get() {
                a.play(s, cat, Some(pos), vol, pitch, None);
            }
        };
        use rc_audio::Category as C;
        match kind {
            0 => self.chat.add_client(Component::translate("block.minecraft.spawn.not_valid", vec![])),
            4 => self.open_win_screen(),
            5 => {
                let msg = self.renderer.as_ref().and_then(|r| {
                    let options = &self.options;
                    demo_message(param, |k| crate::options::key_label(&r.gui.lang, options.s(&format!("key_{k}"))))
                });
                if let Some(m) = msg {
                    self.chat.add_client(m);
                }
            }
            6 => sound("minecraft:entity.arrow.hit_player", C::Player, eye, 0.18, 0.45),
            9 => sound("minecraft:entity.puffer_fish.sting", C::Neutral, feet, 1.0, 1.0),
            10 => {
                // Elder guardian appearance particle: not rendered (see the list of gaps).
                if (param + 0.5).floor() as i32 == 1 {
                    sound("minecraft:entity.elder_guardian.curse", C::Hostile, feet, 1.0, 1.0);
                }
            }
            12 => self.bed.limited_crafting = param == 1.0,
            _ => {}
        }
    }

    /// Event 4: end screen (poem then credits), mouse released, chat closed.
    pub(crate) fn open_win_screen(&mut self) {
        let assets = self.assets.clone();
        self.win = Some(crate::win_screen::WinScreen::new(true, &self.local_name, |p| assets.read(p)));
        if self.chat.input.is_some() {
            self.cmd.close(&mut self.chat);
            self.chat.close();
        }
        self.bed.in_bed_chat = false;
        self.keys.clear();
        self.grab(false);
        self.world_sounds.screen_music = Some("minecraft:music.credits");
    }

    /// End of the credits (or Esc): respawn requested, screen closed, music stopped.
    pub(crate) fn close_win_screen(&mut self) {
        if self.win.take().is_none() {
            return;
        }
        let _ = self.session.link.send_client_command(0);
        self.world_sounds.screen_music = None;
        if let Some(a) = self.audio.get() {
            a.stop(Some(rc_audio::Category::Music), Some("minecraft:music.credits"));
        }
        self.grab(true);
    }

    /// End screen keyboard (before everything else).
    pub(crate) fn win_key(&mut self, e: &WindowEvent) -> bool {
        let Some(w) = &mut self.win else { return false };
        if let WindowEvent::KeyboardInput { event, .. } = e
            && let PhysicalKey::Code(k) = event.physical_key
            && w.key(k, event.state == ElementState::Pressed)
        {
            self.close_win_screen();
        }
        matches!(e, WindowEvent::KeyboardInput { .. } | WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. })
    }

    /// End screen tick: respawn once the scroll is finished.
    pub(crate) fn win_tick(&mut self) {
        if self.win.as_ref().is_some_and(|w| w.finished) {
            self.close_win_screen();
        }
    }
}
