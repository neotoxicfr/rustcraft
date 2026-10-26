//! Wiring of the recipe book and the notifications into the application.

use rc_client::recipes::RecipeEvent;
use winit::event::{ElementState, MouseButton, WindowEvent};

use crate::App;
use crate::recipe_book::{Ctx, book_type};

impl App {
    /// Time of the notifications (ms since startup).
    pub(crate) fn toast_now(&self) -> f64 {
        self.started.elapsed().as_secs_f64() * 1000.0
    }

    /// Book packets (§ 9.5).
    pub(crate) fn on_recipes(&mut self, e: RecipeEvent) {
        if rc_data::env_flag!("RUSTCRAFT_INVLOG") {
            match &e {
                RecipeEvent::Add(v, r) => eprintln!("book: {} recipes added (replace {r})", v.len()),
                RecipeEvent::Remove(v) => eprintln!("book: {} recipes removed", v.len()),
                RecipeEvent::Settings(s) => eprintln!("book: settings {s:?}"),
                RecipeEvent::Ghost { window, .. } => eprintln!("book: ghost recipe (window {window})"),
            }
        }
        if let RecipeEvent::Ghost { window, display } = &e {
            let ctx = Ctx { data: &self.data, rec: &self.creative_rec, item_names: &self.item_names };
            self.recipe_book.on_ghost(*window, display, &self.inventory, &ctx);
            return;
        }
        let ctx = Ctx { data: &self.data, rec: &self.creative_rec, item_names: &self.item_names };
        self.recipe_book.on_event(e, &ctx);
        let now = self.toast_now();
        for (station, result) in std::mem::take(&mut self.recipe_book.unlocked) {
            self.toasts.recipe(station, result, now);
        }
    }

    /// Tick: open screen ↔ book, cycle time, screen position (§ 11.1).
    pub(crate) fn book_tick(&mut self) {
        let narrow = self.renderer.as_ref().is_some_and(|r| (r.gui.width as i32) < 379);
        if !self.screens.open {
            self.recipe_book.screen = None;
            self.recipe_book.ghost = None;
            self.screens.book = (false, narrow);
            return;
        }
        let (kind, _, window, _) = crate::screens::current(&self.inventory);
        if self.recipe_book.screen != Some((window, kind)) {
            self.recipe_book.screen = Some((window, kind));
            self.recipe_book.on_screen(kind);
        }
        if let Some(b) = book_type(kind) {
            // The server can change the open state of the book.
            self.recipe_book.visible = self.recipe_book.settings[b].0;
        }
        if !self.mods.control_key() {
            self.recipe_book.time += 1;
        }
        self.recipe_book.clock += 1;
        self.screens.book = (self.recipe_book.visible && book_type(kind).is_some(), narrow);
        if let Some(spec) = rc_data::env_str!("RUSTCRAFT_BOOKTEST") {
            self.book_test(spec);
        }
    }

    /// Demonstration: `RUSTCRAFT_BOOKTEST=open` opens the book of the current screen;
    /// `place:<n>` also places the n-th displayed collection (ghost recipe if the
    /// ingredients are missing).
    fn book_test(&mut self, spec: &str) {
        if !self.screens.open {
            return;
        }
        self.recipe_book.test += 1;
        let t = self.recipe_book.test;
        if t == 20 && !self.recipe_book.visible {
            self.recipe_book.toggle(&self.session.link);
            let kind = crate::screens::current(&self.inventory).0;
            if let Some(b) = book_type(kind) {
                self.recipe_book.settings[b].0 = true;
            }
        }
        if t == 40
            && let Some(n) = spec.strip_prefix("place:").and_then(|n| n.parse::<usize>().ok())
        {
            let ctx = Ctx { data: &self.data, rec: &self.creative_rec, item_names: &self.item_names };
            let window = crate::screens::current(&self.inventory).2;
            let kind = crate::screens::current(&self.inventory).0;
            if let Some(id) = self.recipe_book.nth_recipe(kind, &ctx, n) {
                eprintln!("book demo: placing recipe {id}");
                let _ = self.session.link.send_place_recipe(window, id, false);
            }
        }
    }

    /// Mouse while a screen with a book is open; true if consumed.
    pub(crate) fn book_mouse(&mut self, e: &WindowEvent) -> bool {
        let WindowEvent::MouseInput { state, button, .. } = e else { return false };
        let b = match button {
            MouseButton::Left => 0,
            MouseButton::Right => 1,
            _ => return false,
        };
        if self.env.game_mode == 3 {
            return false;
        }
        let kind = crate::screens::current(&self.inventory).0;
        if book_type(kind).is_none() {
            return false;
        }
        if *state == ElementState::Released {
            return std::mem::take(&mut self.recipe_book.swallow_release);
        }
        let (Some(r), Some(m)) = (&self.renderer, self.gui_mouse()) else { return false };
        let env = self.hud_env();

        let it = crate::hud::HotbarItems {
            inv: &self.inventory,
            ids: &self.component_ids,
            data: &self.data,
            infos: &self.data.items,
            names: &self.item_names,
            env: &env,
            pt: 0.0,
        };
        let ctx = Ctx { data: &self.data, rec: &self.creative_rec, item_names: &self.item_names };
        let shift = self.mods.shift_key();
        let used = self.recipe_book.press(
            &r.gui,
            &self.inventory,
            &it,
            &self.screens.hasher.enchantments,
            &ctx,
            m,
            b,
            shift,
            &self.session.link,
        );
        let narrow = (r.gui.width as i32) < 379;
        self.screens.book = (self.recipe_book.visible, narrow);
        for _ in 0..std::mem::take(&mut self.recipe_book.clicks) {
            self.play_ui(crate::widgets::CLICK_SOUND, 1.0);
        }
        if used {
            self.recipe_book.swallow_release = true;
        }
        used
    }

    /// Keyboard while a screen with a book is open; true if consumed.
    pub(crate) fn book_key(&mut self, e: &WindowEvent) -> bool {
        // Text committed by the IME: typed into the search field if it has focus.
        if let Some(t) = crate::ime::commit(e) {
            if self.env.game_mode == 3 || book_type(crate::screens::current(&self.inventory).0).is_none() {
                return false;
            }
            let narrow = self.renderer.as_ref().is_some_and(|r| (r.gui.width as i32) < 379);
            let window = crate::screens::current(&self.inventory).2;
            let k = crate::ime::TEXT_ONLY;
            return self.recipe_book.focused
                && self.recipe_book.key(k, self.mods, Some(t), false, narrow, window, &self.session.link).0;
        }
        let WindowEvent::KeyboardInput { event, .. } = e else { return false };
        if event.state != ElementState::Pressed {
            return false;
        }
        let Some(k) = crate::widgets::edit_key(event, self.mods) else { return false };
        // The book does not respond to a spectator.
        if self.env.game_mode == 3 {
            return false;
        }
        let kind = crate::screens::current(&self.inventory).0;
        if book_type(kind).is_none() {
            return false;
        }
        let chat_key = crate::widgets::physical_key(event)
            .is_some_and(|p| crate::options::key_name(p) == self.options.s("key_key.chat"));
        let narrow = self.renderer.as_ref().is_some_and(|r| (r.gui.width as i32) < 379);
        let window = crate::screens::current(&self.inventory).2;
        let (used, click) =
            self.recipe_book.key(k, self.mods, crate::widgets::typed_text(event, self.mods), chat_key, narrow, window, &self.session.link);
        if click {
            self.play_ui(crate::widgets::CLICK_SOUND, 1.0);
        }
        self.screens.book = (self.recipe_book.visible, narrow);
        used
    }
}
