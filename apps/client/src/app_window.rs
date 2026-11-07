//! The window event loop: the `winit` callbacks (resume, window events, device events, end of loop).

use std::sync::Arc;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, ElementState, MouseButton, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::PhysicalKey;
use winit::window::WindowId;

use crate::{
    App, FIRST_DRAWN, STARTED_CLEANLY, boot, chat, create_world, ime, keys, launch, options, present_watch, screens,
    widgets,
};

impl ApplicationHandler for App {
    fn resumed(&mut self, el: &ActiveEventLoop) {
        if self.renderer.is_some() {
            return;
        }
        let window = Arc::new(el.create_window(launch::window_attributes()).expect("window"));
        boot::mark("window");
        launch::center_window(&window, el);
        launch::show_background(&window);
        launch::set_icon(&window, &self.assets);
        present_watch::set_window(&window);
        present_watch::enter(present_watch::Stage::Frame);
        self.build_renderer(window, crate::reload::RendererBuild::Startup);
        present_watch::leave();
    }

    fn window_event(&mut self, el: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        self.video.on_window_event(&event);
        // Held keys, activity, global keys (fullscreen, capture): `keys.rs`.
        if self.keys_observe(&event) {
            return;
        }
        // Resource reload (`reload.rs`): the loading screen takes the input.
        if self.reload_blocks_input(&event)
            || self.links_window_event(&event)
            || self.pack_prompt_window_event(&event)
            || self.pack_screen_window_event(&event)
        {
            return;
        }
        if !self.menus.is_open() && self.dialog_window_event(&event) {
            return;
        }
        if self.front_window_event(&event) {
            return;
        }
        if self.progress_window_event(&event) {
            return;
        }
        if !self.menus.is_open() && self.special_window_event(&event) {
            return;
        }
        match event {
            WindowEvent::CloseRequested => {
                if self.quit_allowed() {
                    el.exit();
                }
            }
            WindowEvent::Resized(s) => {
                // The game lays its screen out again each time the window takes another non-zero
                // size (a minimised window restored included).
                static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
                let size = u64::from(s.width) << 32 | u64::from(s.height);
                let changed = LAST.swap(size, std::sync::atomic::Ordering::Relaxed) != size;
                if let Some(r) = &mut self.renderer {
                    r.resize(s.width, s.height);
                }
                if changed && s.width > 0 && s.height > 0 {
                    self.screen_resized();
                }
            }
            // F3 and F3 + … keys (`debug-f3.md` § 6), before the screens.
            WindowEvent::KeyboardInput { ref event, .. } if self.debug_key_event(event) => {}
            // Debug options screen (F3+F6): the mouse goes back to it.
            ref e @ (WindowEvent::MouseInput { .. }
            | WindowEvent::MouseWheel { .. }
            | WindowEvent::CursorMoved { .. })
                if self.debug.options_open() =>
            {
                self.debug_options_mouse(e);
            }
            // Text validated by the IME: typing in the field of the debug options screen.
            ref e @ WindowEvent::Ime(_) if self.debug.options_open() => {
                if let (Some(t), Some(s)) = (ime::commit(e), &mut self.debug.options_screen) {
                    let mut ev = Vec::new();
                    s.panel.typed(t, &mut ev);
                    self.debug_options_events(ev);
                }
            }
            // Recipe book: clicks and typing before the container screen.
            ref e @ (WindowEvent::MouseInput { .. } | WindowEvent::KeyboardInput { .. } | WindowEvent::Ime(_))
                if self.screens.open
                    && self.death.is_none()
                    && self.chat.input.is_none()
                    && (self.book_mouse(e) || self.book_key(e)) => {}
            // Creative inventory: its inputs first.
            ref e
                if self.creative_ui.open
                    && self.death.is_none()
                    && self.chat.input.is_none()
                    && self.creative_event(e) => {}
            // End screen, chat in bed, spectator menu (`win_screen.rs`, `bed.rs`, `spectator.rs`).
            ref e if self.win_key(e) || self.bed_key(e) || self.bed_mouse(e) || self.spectator_event(e) => {}
            // Death screen: neither game keyboard nor Esc.
            WindowEvent::KeyboardInput { event, .. } if self.death.is_some() => self.death_key(&event),
            // Menus (pause, options): all the input goes to them.
            WindowEvent::KeyboardInput { event, .. } if self.menus.is_open() => {
                if event.state != ElementState::Pressed {
                    if let PhysicalKey::Code(k) = event.physical_key {
                        self.menus.key_release(k);
                    }
                    return;
                }
                let Some(r) = &self.renderer else { return };
                let mut actions = Vec::new();
                let key = if self.menus.waiting_key() {
                    widgets::physical_key(&event)
                } else {
                    widgets::edit_key(&event, self.mods)
                };
                if let Some(k) = key {
                    actions = self.menus.key(&r.gui, &mut self.options, k, self.mods);
                }
                if let Some(t) = widgets::typed_text(&event, self.mods) {
                    actions.extend(self.menus.typed(&r.gui, &mut self.options, t));
                }
                self.menu_actions(actions);
            }
            // Text validated by the IME: like typing in the menus field.
            ref e @ WindowEvent::Ime(_) if self.menus.is_open() => {
                if let (Some(r), Some(t)) = (&self.renderer, ime::commit(e)) {
                    let actions = self.menus.typed(&r.gui, &mut self.options, t);
                    self.menu_actions(actions);
                }
            }
            WindowEvent::MouseInput { state, button, .. } if self.menus.is_open() => {
                let b = match button {
                    MouseButton::Left => 0,
                    MouseButton::Right => 1,
                    MouseButton::Middle => 2,
                    _ => return,
                };
                let (Some(r), Some(m)) = (&self.renderer, self.gui_mouse()) else { return };
                let actions = if state == ElementState::Pressed {
                    self.menus.press(&r.gui, &mut self.options, m, b, self.mods.shift_key())
                } else {
                    self.menus.release(&r.gui, &mut self.options)
                };
                self.menu_actions(actions);
            }
            WindowEvent::MouseWheel { delta, .. } if self.menus.is_open() => {
                let (_, y) = keys::wheel_offsets(&delta);
                let (Some(r), Some(m)) = (&self.renderer, self.gui_mouse()) else { return };
                let actions = self.menus.wheel(&r.gui, &mut self.options, m, y, self.mods.control_key());
                self.menu_actions(actions);
            }
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = Some((position.x, position.y));
                if self.menus.is_open()
                    && let (Some(r), Some(m)) = (&self.renderer, self.gui_mouse())
                {
                    let actions = self.menus.motion(&r.gui, &mut self.options, m);
                    self.menu_actions(actions);
                }
                if self.screens.open
                    && let (Some(r), Some(m)) = (&self.renderer, self.gui_mouse())
                {
                    let rules = screens::ItemRules {
                        infos: &self.data.items,
                        names: &self.item_names,
                        ids: &self.component_ids,
                        data: &self.data,
                    };
                    self.screens.motion(&r.gui, &self.inventory, m, &rules);
                }
            }
            WindowEvent::ModifiersChanged(m) => {
                self.mods = m.state();
                create_world::set_alt_down(self.mods.alt_key());
            }
            WindowEvent::KeyboardInput { event, .. } if self.screens.open && self.chat.input.is_none() => {
                // Repeats do not go to the screens.
                if event.state != ElementState::Pressed || event.repeat {
                    return;
                }
                let PhysicalKey::Code(k) = event.physical_key else { return };
                let (digit, drop, close) = self.screen_key_actions(k);
                if close {
                    self.close_screen();
                    return;
                }
                // Crafter: disabled slot re-enabled by the swap key (before the swap).
                if let (Some(d), Some(m)) = (digit, self.gui_mouse()) {
                    self.crafter_swap_key(m, d);
                }
                let click = match (&self.renderer, self.gui_mouse()) {
                    (Some(r), Some(m)) if digit.is_some() || drop.is_some() => {
                        self.screens.key(&r.gui, &self.inventory, m, digit, drop)
                    }
                    _ => None,
                };
                if let Some(c) = click {
                    self.screen_clicks(vec![c]);
                }
            }
            WindowEvent::MouseInput { state, button, .. } if self.screens.open && self.death.is_none() => {
                let b = match button {
                    MouseButton::Left => 0,
                    MouseButton::Right => 1,
                    MouseButton::Middle => 2,
                    _ => return,
                };
                let (Some(r), Some(m)) = (&self.renderer, self.gui_mouse()) else { return };
                let shift = self.mods.shift_key();
                let clicks = if state == ElementState::Pressed {
                    self.screens.press(&r.gui, &self.inventory, b, m, shift, self.env.game_mode == 1)
                } else {
                    self.screens.release(&r.gui, &self.inventory, b, m, shift)
                };
                self.screen_clicks(clicks);
            }
            // In-game buttons (captured mouse): handled at the tick (`interact.rs`).
            WindowEvent::MouseInput { state, button, .. }
                if self.chat.input.is_none() && self.grabbed && self.death.is_none() =>
            {
                self.game_mouse(state, button);
            }
            WindowEvent::CursorLeft { .. } => self.cursor = None,
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. }
                if self.death.is_some() =>
            {
                let s = self.renderer.as_ref().map_or(1.0, |r| f64::from(r.gui.scale));
                let action = match (self.cursor, &mut self.death) {
                    (Some((x, y)), Some(d)) => d.click((x / s) as f32, (y / s) as f32),
                    _ => None,
                };
                self.death_action(action);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                // Chat input open: the keyboard goes to the field.
                if self.chat.input.is_some() {
                    if event.state == ElementState::Pressed {
                        self.chat_key(&event);
                    }
                    return;
                }
                // Rebindable game keys (`keys.rs`): Esc, actions, key state.
                self.game_keyboard(&event);
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. }
                if self.chat.input.is_some() =>
            {
                if let Some(m) = self.gui_mouse() {
                    self.cmd.click(&mut self.chat, m);
                }
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. }
                if self.chat.input.is_none() =>
            {
                // Free mouse: the click captures it (without a game action).
                self.grab(true)
            }
            WindowEvent::MouseWheel { delta, .. } if self.chat.input.is_some() => {
                let lines = keys::wheel_offsets(&delta).1;
                if let Some(m) = self.gui_mouse()
                    && self.cmd.wheel(m, lines)
                {
                    return;
                }
                self.chat.scroll(chat::wheel_lines(lines, self.mods.shift_key()));
            }
            // Container screen: the wheel picks a bundle's item, never the bar.
            WindowEvent::MouseWheel { delta, .. } if self.screens.open => {
                let (x, y) = keys::wheel_offsets(&delta);
                self.bundle_wheel(x, y);
            }
            WindowEvent::MouseWheel { delta, .. } => self.game_scroll(delta),
            WindowEvent::Focused(false) => {
                self.keys.clear();
                self.grab(false);
                self.unfocused_at = Some(Instant::now());
            }
            WindowEvent::Focused(true) => self.unfocused_at = None,
            WindowEvent::RedrawRequested => {
                // Frame zones for the F3+1 and F3+L pie chart (`profiler.rs`).
                present_watch::enter(present_watch::Stage::Frame);
                self.profiler_frame_start();
                self.frame();
                // End of loading (equivalent of the game's end of loading): startup successful.
                use std::sync::atomic::Ordering::Relaxed;
                if FIRST_DRAWN.load(Relaxed) && !STARTED_CLEANLY.swap(true, Relaxed) {
                    self.options.set("startedCleanly", options::Val::B(true));
                    self.save_options();
                }
                self.profiler_frame_end();
                present_watch::leave();
            }
            _ => {}
        }
    }

    fn device_event(&mut self, _: &ActiveEventLoop, _: winit::event::DeviceId, event: DeviceEvent) {
        // Accumulated then applied to the frame (`keys.rs` `mouse_frame`: sensitivity, cinematic).
        if let DeviceEvent::MouseMotion { delta } = event {
            self.mouse_motion(delta.0, delta.1);
        }
    }

    fn about_to_wait(&mut self, el: &ActiveEventLoop) {
        if self.exit_requested {
            el.exit();
            return;
        }
        self.video_tick();
        // Next frame, at the allowed pace (`keys.rs` `pace_frames`).
        self.pace_frames(el);
    }
}
