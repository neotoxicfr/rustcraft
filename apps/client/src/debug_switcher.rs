//! Game mode switcher F3+F4 (`debug-f3.md` § 6.5): four slots, F4 advances, the mouse
//! picks as soon as it has moved, releasing F3 applies. Does not interrupt the game.

use crate::gui::Gui;

/// Display and cycle order: creative, survival, adventure, spectator (network modes 1, 0, 2, 3).
const MODES: [i32; 4] = [1, 0, 2, 3];
const KEYS: [&str; 4] = ["gameMode.creative", "gameMode.survival", "gameMode.adventure", "gameMode.spectator"];
const ICONS: [&str; 4] = ["grass_block", "iron_sword", "map", "ender_eye"];

pub struct Switcher {
    /// Chosen slot (index into `MODES`).
    pub selected: usize,
    /// Mouse position at opening (or at the last F4): it only picks after having moved.
    mouse_at: Option<(f32, f32)>,
    /// Icon layers, computed at the first draw.
    icons: Option<Vec<(u64, Vec<rc_assets::item::Layer>)>>,
}

fn slot_of(mode: i32) -> usize {
    MODES.iter().position(|&m| m == mode).unwrap_or(0)
}

impl Switcher {
    /// Initial selection: previous mode, otherwise survival from creative, otherwise creative.
    pub fn new(current: i32, previous: Option<i32>, mouse: Option<(f32, f32)>) -> Switcher {
        let mode = match previous.filter(|p| (0..=3).contains(p)) {
            Some(p) => p,
            None if current == 1 => 0,
            None => 1,
        };
        Switcher { selected: slot_of(mode), mouse_at: mouse, icons: None }
    }

    pub fn next(&mut self, mouse: Option<(f32, f32)>) {
        self.selected = (self.selected + 1) % 4;
        self.mouse_at = mouse;
    }

    pub fn mode(&self) -> i32 {
        MODES[self.selected]
    }

    /// Icon layers, once (before `draw`).
    pub fn prepare(&mut self, items: &Icons<'_>) {
        if self.icons.is_none() {
            self.icons = Some(ICONS.iter().map(|n| items.layers(n)).collect());
        }
    }

    /// Centered drawing; the mouse (GUI units) picks the hovered slot if it has moved.
    pub fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, mouse: Option<(f32, f32)>, f4: &str) {
        let (w, h) = (gui.width as i32, gui.height as i32);
        let x0 = w / 2 - 59;
        let y0 = h / 2 - 31;
        if let Some(m) = mouse
            && self.mouse_at.is_some_and(|a| a != m)
        {
            for i in 0..4 {
                let sx = (x0 + 31 * i as i32) as f32;
                if m.0 >= sx && m.0 < sx + 26.0 && m.1 >= y0 as f32 && m.1 < y0 as f32 + 26.0 {
                    self.selected = i;
                }
            }
        }
        if self.mouse_at.is_none() {
            self.mouse_at = mouse;
        }
        gui.next_layer();
        gui.sprite_region(
            "container_bg/gamemode_switcher",
            128.0,
            128.0,
            0.0,
            0.0,
            (w / 2 - 62) as f32,
            (h / 2 - 31 - 27) as f32,
            125.0,
            75.0,
        );
        let title = crate::widgets::tr(&gui.lang, KEYS[self.selected], &[]);
        let chars = crate::gui::plain(&title);
        let tw = rc_text::width(gui, &chars) as f32;
        gui.text(q, &chars, (w / 2) as f32 - (tw / 2.0).floor(), (h / 2 - 31 - 20) as f32, 0xFFFF_FFFF, true, 0);
        let next = rc_text::flatten(
            &rc_text::Component::translate(
                "debug.gamemodes.select_next",
                vec![rc_text::Arg::Component({
                    let mut k = rc_text::Component::text(f4);
                    k.style.color = Some(0x55FFFF);
                    k
                })],
            ),
            &gui.lang,
        );
        let chars = rc_text::styled_chars(&next);
        let tw = rc_text::width(gui, &chars) as f32;
        gui.text(q, &chars, (w / 2) as f32 - (tw / 2.0).floor(), (h / 2 + 5) as f32, 0xFFFF_FFFF, true, 0);
        for i in 0..4 {
            let x = (x0 + 31 * i) as f32;
            gui.sprite("gamemode_switcher/slot", x, y0 as f32, 26.0, 26.0, 0xFFFF_FFFF);
        }
        gui.stratum(crate::gui::Blend::Premultiplied);
        for (i, (key, layers)) in self.icons.iter().flatten().enumerate() {
            if !layers.is_empty() {
                gui.item(*key, layers, (x0 + 31 * i as i32 + 5) as f32, (y0 + 5) as f32, 0.0);
            }
        }
        gui.stratum(crate::gui::Blend::Normal);
        let x = (x0 + 31 * self.selected as i32) as f32;
        gui.sprite("gamemode_switcher/selection", x, y0 as f32, 26.0, 26.0, 0xFFFF_FFFF);
    }
}

/// Access to the item models for the icons.
pub struct Icons<'a> {
    pub models: Option<&'a rc_assets::item::ItemModels>,
    pub names: &'a [String],
    pub infos: &'a [rc_data::ItemInfo],
    pub ids: &'a crate::stack::ComponentIds,
    pub data: &'a rc_data::VersionData,
    pub env: &'a crate::stack::HudEnv,
}

impl Icons<'_> {
    fn layers(&self, item: &str) -> (u64, Vec<rc_assets::item::Layer>) {
        use std::hash::{Hash, Hasher};
        let Some(models) = self.models else { return (0, Vec::new()) };
        let full = format!("minecraft:{item}");
        let Some(i) = self.names.iter().position(|n| n == &full || n == item) else { return (0, Vec::new()) };
        let stack = rc_proto_v777::item::ItemStack { item: i as i32, count: 1, added: Vec::new(), removed: Vec::new() };
        let view = crate::stack::StackRef {
            stack: &stack,
            info: self.infos.get(i),
            ids: self.ids,
            data: self.data,
            hold: crate::stack::Hold::NONE,
        };
        let mut layers = Vec::new();
        let mut key = Vec::new();
        if let Some(model) = view.model() {
            models.layers(&self.names[i], &model, &view, self.env, 0, &mut layers, &mut key);
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        key.hash(&mut h);
        (h.finish(), layers)
    }
}

impl crate::App {
    pub(crate) fn debug_switcher_open(&mut self) {
        let mouse = self.gui_mouse();
        self.debug.switcher = Some(Switcher::new(self.env.game_mode, self.debug.previous_mode, mouse));
        self.keys.clear();
        self.grab(false);
    }

    pub(crate) fn debug_switcher_next(&mut self) {
        let mouse = self.gui_mouse();
        if let Some(s) = &mut self.debug.switcher {
            s.next(mouse);
        }
    }

    /// F3 released: mode sent if it changes (and if allowed), then close.
    pub(crate) fn debug_switcher_apply(&mut self) {
        let Some(s) = self.debug.switcher.take() else { return };
        if s.mode() != self.env.game_mode && self.debug.op_level >= 2 {
            let _ = self.session.link.send_change_game_mode(s.mode());
        }
        self.grab(true);
    }
}
