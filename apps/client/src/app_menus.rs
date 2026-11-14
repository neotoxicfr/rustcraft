//! Screens and menus driven by the application: closing, clicks, chat input, pause, options, test demonstrations,
//! cursor grab.

use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::CursorGrabMode;

use crate::{App, audio_slot, camera, commands, death, gui, launch, menus, options, pick, screens, widgets};

impl App {
    /// Closing of a screen by the client: `container_close` then local closing (§ 1.2).
    pub(crate) fn close_screen(&mut self) {
        if self.creative_ui.open {
            self.close_creative();
            return;
        }
        if !self.screens.open {
            return;
        }
        let window = self.inventory.container.as_ref().map_or(0, |c| c.window);
        let _ = self.session.link.send_container_close(window);
        self.bundle_close();
        self.inventory.container = None;
        self.screens.hide();
        self.grab(true);
    }

    /// Clicks produced by the screen: prediction and sending, in order.
    pub(crate) fn screen_clicks(&mut self, clicks: Vec<screens::Click>) {
        let rules = screens::ItemRules {
            infos: &self.data.items,
            names: &self.item_names,
            ids: &self.component_ids,
            data: &self.data,
        };
        let creative = self.env.game_mode == 1;
        let link = &self.session.link;
        let naming = crate::screens_work::AnvilNaming::new(&self.data, &self.component_ids, &self.renderer);
        // Between each prediction and its click packet (`Screens::perform`).
        let mut anvil = |inv: &mut crate::inventory::PlayerInventory| {
            crate::screens_work::anvil_watch_first_slot(inv, &naming, link)
        };
        for c in clicks {
            self.recipe_book.crafting_click(screens::current(&self.inventory).0, c.slot);
            self.screens.perform(&mut self.inventory, &rules, creative, link, c, &mut anvil);
        }
        // Shift + double click: each slot is re-evaluated after the previous clicks.
        while let Some(c) = self.screens.next_sweep(&self.inventory) {
            self.recipe_book.crafting_click(screens::current(&self.inventory).0, c.slot);
            self.screens.perform(&mut self.inventory, &rules, creative, link, c, &mut anvil);
        }
    }

    /// Cursor in interface units.
    pub(crate) fn gui_mouse(&self) -> Option<(f32, f32)> {
        static FORCED: std::sync::OnceLock<Option<(f32, f32)>> = std::sync::OnceLock::new();
        let forced = FORCED.get_or_init(|| {
            let v = rc_data::env_str!("RUSTCRAFT_INVMOUSE")?;
            let (x, y) = v.split_once(',')?;
            Some((x.trim().parse().ok()?, y.trim().parse().ok()?))
        });
        if forced.is_some() {
            return *forced;
        }
        if self.menutest.2.is_some() {
            return self.menutest.2;
        }
        let s = self.renderer.as_ref().map_or(1.0, |r| f64::from(r.gui.scale));
        self.cursor.map(|(x, y)| ((x / s) as f32, (y / s) as f32))
    }

    /// Keyboard-free demonstration (`RUSTCRAFT_INVTEST`): `e` opens the inventory, `chest`
    /// uses the aimed block; then `RUSTCRAFT_INVCLICKS` = "slot:button:mode" separated by
    /// commas, one click every 10 ticks.
    pub(crate) fn invtest_tick(&mut self) {
        let Some(test) = rc_data::env_str!("RUSTCRAFT_INVTEST") else { return };
        if !self.positioned {
            return;
        }
        self.invtest.1 += 1;
        if self.invtest.1 < 320 || !self.invtest.1.is_multiple_of(10) {
            return;
        }
        if self.invtest.0 == 0 {
            // Forced block (`x,y,z`: top face) or aimed block.
            let forced = test.split(',').map(|v| v.trim().parse::<i32>()).collect::<Result<Vec<_>, _>>().ok();
            if test == "e" {
                self.screens.show();
            } else if let Some([x, y, z]) = forced.as_deref() {
                let seq = self.act.next_seq();
                let _ = self.session.link.send_use_item_on([*x, *y, *z], 1, [0.5, 1.0, 0.5], false, seq);
                eprintln!("demo: use of block {x} {y} {z}");
            } else if let pick::Hit::Block { pos, face, at, inside } = self.pick() {
                let seq = self.act.next_seq();
                let rel = [0, 1, 2].map(|a| (at[a] - f64::from(pos[a])) as f32);
                let _ = self.session.link.send_use_item_on(pos, i32::from(face), rel, inside, seq);
                eprintln!("demo: use of block {pos:?}");
            }
            self.invtest.0 = 1;
            return;
        }
        let clicks = rc_data::env_str!("RUSTCRAFT_INVCLICKS").unwrap_or_default();
        let Some(spec) = clicks.split(',').filter(|s| !s.is_empty()).nth(self.invtest.0 - 1) else { return };
        let v: Vec<i32> = spec.split(':').filter_map(|x| x.trim().parse().ok()).collect();
        if let [slot, button, mode] = v[..] {
            let c = screens::Click { slot: slot as i16, button: button as i8, mode };
            eprintln!("demo: {c:?}");
            if mode == 5 && button & 3 == 1 {
                self.screens.drag_sent.push(slot as usize);
            }
            self.screen_clicks(vec![c]);
        }
        self.invtest.0 += 1;
    }

    /// Death of the local player: death screen, or direct respawn if the server disabled it.
    pub(crate) fn open_death(&mut self, message: Option<rc_text::Component>) {
        self.close_screen();
        self.menus.close_all();
        if self.show_death_screen {
            self.death = Some(death::DeathScreen::new(message, self.hud.hardcore, self.hud.score));
            self.chat.close();
            self.keys.clear();
            self.grab(false);
        } else {
            let _ = self.session.link.send_client_command(0);
        }
    }

    /// Key pressed during chat input (`chat.md` § 4, `commands.md` § 4.2).
    pub(crate) fn chat_key(&mut self, event: &winit::event::KeyEvent) {
        use winit::keyboard::{Key, NamedKey};
        let (ctrl, shift) = (self.mods.control_key() && !self.mods.alt_key(), self.mods.shift_key());
        // Ctrl + A/C/V/X shortcuts: layout letter, otherwise physical key.
        let letter = match &event.logical_key {
            Key::Character(c) => c.chars().next().map(|c| c.to_ascii_lowercase()),
            _ => None,
        }
        .or(match event.physical_key {
            PhysicalKey::Code(KeyCode::KeyA) => Some('a'),
            PhysicalKey::Code(KeyCode::KeyC) => Some('c'),
            PhysicalKey::Code(KeyCode::KeyV) => Some('v'),
            PhysicalKey::Code(KeyCode::KeyX) => Some('x'),
            _ => None,
        });
        let k = match &event.logical_key {
            Key::Named(NamedKey::Escape) => commands::Key::Escape,
            Key::Named(NamedKey::Enter) => commands::Key::Enter,
            Key::Named(NamedKey::Tab) => commands::Key::Tab { shift },
            Key::Named(NamedKey::Backspace) if ctrl => commands::Key::BackspaceWord,
            Key::Named(NamedKey::Backspace) => commands::Key::Backspace,
            Key::Named(NamedKey::Delete) if ctrl => commands::Key::DeleteWord,
            Key::Named(NamedKey::Delete) => commands::Key::Delete,
            Key::Named(NamedKey::Home) => commands::Key::Home { extend: shift },
            Key::Named(NamedKey::End) => commands::Key::End { extend: shift },
            Key::Named(NamedKey::ArrowLeft) => commands::Key::Move { dir: -1, word: ctrl, extend: shift },
            Key::Named(NamedKey::ArrowRight) => commands::Key::Move { dir: 1, word: ctrl, extend: shift },
            _ if ctrl && letter == Some('a') => commands::Key::SelectAll,
            _ if ctrl && letter == Some('c') => commands::Key::Copy,
            _ if ctrl && letter == Some('v') => commands::Key::Paste,
            _ if ctrl && letter == Some('x') => commands::Key::Cut,
            Key::Named(NamedKey::ArrowUp) => commands::Key::Up,
            Key::Named(NamedKey::ArrowDown) => commands::Key::Down,
            Key::Named(NamedKey::PageUp) => commands::Key::PageUp,
            Key::Named(NamedKey::PageDown) => commands::Key::PageDown,
            _ => match widgets::typed_text(event, self.mods) {
                Some(t) => commands::Key::Text(t.to_owned()),
                None => return,
            },
        };
        self.chat_input(k);
    }

    /// Input field key (keyboard or test script).
    pub(crate) fn chat_input(&mut self, k: commands::Key) {
        let outcome = self.cmd.key(&mut self.chat, k);
        self.commands_send();
        match outcome {
            commands::Outcome::None => {}
            commands::Outcome::Empty if self.bed.in_bed_chat => {
                // Chat in bed: Enter on an empty field only empties it (screen kept).
                self.chat.reopen_after_send();
                self.commands_refresh();
            }
            commands::Outcome::Close | commands::Outcome::Empty => {
                self.cmd.close(&mut self.chat);
                self.chat.close();
                self.grab(true);
            }
            commands::Outcome::Send { command, text } => {
                let link = &self.session.link;
                let _ = if command {
                    link.send_command_args(&text, &self.cmd.signable_args(&text))
                } else {
                    link.send_chat(&text)
                };
                if self.bed.in_bed_chat {
                    // Chat in bed: sent without leaving the screen (mouse still free).
                    self.chat.reopen_after_send();
                    self.commands_refresh();
                    return;
                }
                self.cmd.close(&mut self.chat);
                self.chat.close();
                self.grab(true);
            }
        }
    }

    /// Pause menu (`option-menus.md` § 2.1): mouse released, game keys released.
    pub(crate) fn open_pause(&mut self, keyboard: bool) {
        if self.menus.is_open() {
            return;
        }
        self.menus.open(menus::Id::Pause, keyboard);
        self.keys.clear();
        self.grab(false);
    }

    /// Writing of `options.txt`, then `client_information` if the information changed
    /// since the last send (§ 4.4).
    pub(crate) fn save_options(&mut self) {
        if let Err(e) = self.options.save(&self.options_path) {
            eprintln!("options: writing {}: {e}", self.options_path.display());
        }
        let info = self.options.client_info();
        if info != self.last_info {
            let _ = self.session.link.send_client_information(&info);
            self.last_info = info;
        }
    }

    pub(crate) fn menu_actions(&mut self, actions: Vec<menus::Action>) {
        for a in actions {
            match a {
                // Back to the open chat (restrictions screen): mouse left free.
                menus::Action::Resume if self.chat.is_open() => {}
                menus::Action::Resume => self.grab(true),
                // Closing on the client side, back to the server list (`flow.rs`).
                menus::Action::Disconnect => self.disconnect_to_menu(),
                menus::Action::Changed => self.apply_options(),
                // Sound preview of the volume sliders: only outside the world (title screen).
                menus::Action::SoundPreview(c, v) => {
                    if self.front.is_open()
                        && let Some(a) = self.audio.get()
                    {
                        a.preview(c, v);
                    }
                }
                menus::Action::Save => self.save_options(),
                menus::Action::ResourcePacks => self.open_pack_screen(),
                menus::Action::Advancements(kb) => self.open_advancements(true, kb),
                menus::Action::Stats(kb) => self.open_stats(kb),
                menus::Action::Language(code) => self.apply_language(&code),
                other => self.world_menu_action(other),
            }
        }
    }

    /// Loads the translations of `code` (already written in the options) into the interface; a
    /// failing load keeps the current ones, like the game.
    pub(crate) fn apply_language(&mut self, code: &str) {
        let lang = match crate::resources::load_translations(&self.assets, code, &self.menus.languages) {
            Ok(lang) => lang,
            Err(e) => {
                eprintln!("language {code}: translations not loaded ({e:?}), previous ones kept");
                return;
            }
        };
        if let Some(r) = &mut self.renderer {
            r.gui.lang = std::sync::Arc::new(lang);
        }
    }

    /// Effects of the settings the engine can apply (`option-menus.md` § 3); the others
    /// are only recorded. Field of view, view effects and tilt: in
    /// `Options` until the camera module exists.
    pub(crate) fn apply_options(&mut self) {
        let o = &self.options;
        // Engine still being built with these volumes: nothing to do, without joining.
        let v = audio_slot::volumes(o);
        if let Some(a) = self.audio.for_volumes(&v) {
            for c in rc_audio::Category::ALL {
                a.set_volume(c, v[c as usize]);
            }
        }
        self.env.gamma = Some(o.f("gamma") as f32);
        self.env.set_fov_option(o.fov());
        let clouds = o.s("renderClouds") != "false";
        self.env.clouds = Some((clouds, o.i("cloudRange"), o.s("renderClouds") == "fast"));
        self.chat.apply_options(o);
        rc_client::chat::set_only_secure(o.b("onlyShowSecureChat"));
        rc_client::chat::set_visibility(o.i("chatVisibility"));
        rc_client::chat::set_hide_matched_names(o.b("hideMatchedNames"));
        rc_text::set_keybinds(options::KEYS.iter().map(|k| (k.0.to_owned(), o.s(&format!("key_{}", k.0)).to_owned())));
        self.hud.no_vignette = !o.b("vignette");
        self.debug.set_reduced(None, Some(o.b("reducedDebugInfo")));
        self.particles.level = o.i("particles").clamp(0, 2) as u8;
        self.screens.set_advanced(o.b("advancedItemTooltips"));
        self.camera.settings = camera::CameraSettings {
            fov: o.fov() as f32,
            fov_effect_scale: o.f("fovEffectScale") as f32,
            bob_view: o.b("bobView"),
            damage_tilt: o.f("damageTiltStrength") as f32,
        };
        if let Some(r) = &mut self.renderer {
            r.gui.scale_option = o.i("guiScale").max(0) as u32;
            gui::set_force_unicode(o.b("forceUnicodeFont"));
            // Test client in the background: always windowed (a hidden fullscreen window
            // presents no frame).
            // Test clients: never fullscreen from `options.txt` (winit enables a fullscreen
            // window, which would cover the user's screen and take focus).
            let full = o.b("fullscreen") && !launch::background();
            if r.window.fullscreen().is_some() != full {
                r.window.set_fullscreen(full.then_some(winit::window::Fullscreen::Borderless(None)));
            }
        }
        self.apply_video_options();
    }

    /// Menu tick: pause after 500 ms without focus, click sounds, demonstration.
    pub(crate) fn menus_tick(&mut self) {
        let lost =
            self.unfocused_at.is_some_and(|t| t.elapsed().as_millis() > 500) && self.options.b("pauseOnLostFocus");
        // Automatic captures: no pause if the window loses focus.
        if lost && self.positioned && self.capture.is_none() && !self.quit_after_capture && !launch::background() {
            self.unfocused_at = None;
            if !self.screens.open && self.death.is_none() && self.chat.input.is_none() {
                self.open_pause(false);
            }
        }
        self.progress_tick();
        self.world_menus_tick();
        let deferred = self.menus.tick(&mut self.options);
        self.menu_actions(deferred);
        let clicks = std::mem::take(&mut self.menus.clicks);
        if let Some(a) = self.audio.get() {
            for _ in 0..clicks {
                a.play(widgets::CLICK_SOUND, rc_audio::Category::Ui, None, 0.25, 1.0, None);
            }
        }
        self.menutest_tick();
    }

    /// Keyboard-free demonstration: `RUSTCRAFT_MENUTEST=pause|options|video|controls` opens
    /// the screen 12 s after arrival, then `RUSTCRAFT_MENUCLICKS` = actions separated by
    /// commas, one every 10 ticks: `x:y` left click (GUI units), `m:x:y` mouse
    /// only, `d:x0:y0:x1:y1` drag, `w:x:y:notches` wheel, `k:<key>` (Tab, Down, Up,
    /// Left, Right, Enter, Escape).
    fn menutest_tick(&mut self) {
        let Some(test) = rc_data::env_str!("RUSTCRAFT_MENUTEST") else { return };
        if test == "title" {
            self.titletest_tick();
            return;
        }
        if !self.positioned {
            return;
        }
        self.menutest.1 += 1;
        if self.menutest.1 < 240 || !self.menutest.1.is_multiple_of(10) {
            return;
        }
        let Some(r) = &self.renderer else { return };
        if self.menutest.0 == 0 {
            self.open_pause(false);
            if let Some(id) = menus::Id::test_path(test) {
                for s in id {
                    self.menus.open(s, false);
                }
            }
            self.menutest.0 = 1;
            return;
        }
        let spec = rc_data::env_str!("RUSTCRAFT_MENUCLICKS").unwrap_or_default();
        let Some(step) = spec.split(',').filter(|s| !s.is_empty()).nth(self.menutest.0 - 1) else { return };
        self.menutest.0 += 1;
        eprintln!("menu demo: {step}");
        let p: Vec<&str> = step.split(':').collect();
        let f = |s: &str| s.trim().parse::<f32>().unwrap_or(0.0);
        let gui = &r.gui;
        let mut actions = Vec::new();
        match p[..] {
            [x, y] if x != "k" => {
                let m = (f(x), f(y));
                self.menutest.2 = Some(m);
                actions = self.menus.press(gui, &mut self.options, m, 0, false);
                actions.extend(self.menus.release(gui, &mut self.options));
            }
            ["m", x, y] => {
                self.menutest.2 = Some((f(x), f(y)));
                actions = self.menus.motion(gui, &mut self.options, (f(x), f(y)));
            }
            ["d", x0, y0, x1, y1] => {
                actions = self.menus.press(gui, &mut self.options, (f(x0), f(y0)), 0, false);
                actions.extend(self.menus.motion(gui, &mut self.options, (f(x1), f(y1))));
                actions.extend(self.menus.release(gui, &mut self.options));
                self.menutest.2 = Some((f(x1), f(y1)));
            }
            ["w", x, y, n] => {
                self.menutest.2 = Some((f(x), f(y)));
                actions = self.menus.wheel(gui, &mut self.options, (f(x), f(y)), f64::from(f(n)), false);
            }
            ["k", key] => {
                let k = match key {
                    "Tab" => KeyCode::Tab,
                    "Down" => KeyCode::ArrowDown,
                    "Up" => KeyCode::ArrowUp,
                    "Left" => KeyCode::ArrowLeft,
                    "Right" => KeyCode::ArrowRight,
                    "Enter" => KeyCode::Enter,
                    _ => KeyCode::Escape,
                };
                actions = self.menus.key(gui, &mut self.options, k, self.mods);
            }
            _ => {}
        }
        self.menu_actions(actions);
    }

    pub(crate) fn grab(&mut self, on: bool) {
        // Back in the game: the sounds frozen by the pause resume (`pause.rs`).
        if on && let Some(a) = self.audio.get() {
            a.resume();
        }
        let Some(r) = &self.renderer else { return };
        // Like the game, the pointer goes back to the centre of the window at each capture and each
        // release, before the lock (which pins it where it stands); the interface's mouse follows.
        if let Some((x, y)) = recentred_pointer(self.grabbed, on, launch::background(), r.window.inner_size()) {
            // Never moved while another window has the focus (the user is elsewhere).
            if r.window.has_focus() {
                let _ = r.window.set_cursor_position(winit::dpi::PhysicalPosition::new(x, y));
            }
            self.cursor = Some((x, y));
        }
        if on && !launch::background() {
            let _ = r
                .window
                .set_cursor_grab(CursorGrabMode::Locked)
                .or_else(|_| r.window.set_cursor_grab(CursorGrabMode::Confined));
        } else {
            let _ = r.window.set_cursor_grab(CursorGrabMode::None);
        }
        r.window.set_cursor_visible(!on || launch::background());
        self.keys_grab(on);
        self.grabbed = on;
    }
}

/// Where the pointer goes when the capture changes from `grabbed` to `on`: the centre of the
/// window's client area (half its width and height in pixels), only on a real change and never in
/// background mode (benches, nothing captured).
fn recentred_pointer(
    grabbed: bool,
    on: bool,
    background: bool,
    size: winit::dpi::PhysicalSize<u32>,
) -> Option<(f64, f64)> {
    (grabbed != on && !background).then(|| (f64::from(size.width) / 2.0, f64::from(size.height) / 2.0))
}

#[cfg(test)]
mod pointer_tests {
    use super::recentred_pointer;
    use winit::dpi::PhysicalSize;

    /// Opening the inventory (release) or going back to the game (capture) puts the pointer at the
    /// window's centre; it stayed where it was at the previous capture before.
    #[test]
    fn capture_changes_put_the_pointer_at_the_window_centre() {
        let size = PhysicalSize::new(1601, 900);
        assert_eq!(recentred_pointer(true, false, false, size), Some((800.5, 450.0)), "release");
        assert_eq!(recentred_pointer(false, true, false, size), Some((800.5, 450.0)), "capture");
        assert_eq!(recentred_pointer(false, false, false, size), None, "already free");
        assert_eq!(recentred_pointer(true, true, false, size), None, "already captured");
        assert_eq!(recentred_pointer(false, true, true, size), None, "background mode");
    }
}
