//! Wiring of the creative inventory into the application (`creative-inventory.md` § 1):
//! opening in place of the inventory in instant-build mode, mode switches, input.

use rc_data::CreativeKind;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::KeyCode;

use rc_proto_v777::item::ItemStack;

use crate::App;
use crate::creative::Params;
use crate::creative_screen::Hit;

impl App {
    /// Infinite materials ("instant build" ability).
    pub(crate) fn instabuild(&self) -> bool {
        self.world_sounds.instabuild
    }

    pub(crate) fn creative_params(&self) -> Params {
        let permission = self.options.b("operatorItemsTab") && self.instabuild() && self.debug.op_level >= 2;
        Params { features: self.exact.features.clone(), permission, generation: self.creative_rec.generation }
    }

    /// Inventory key: creative screen with infinite materials, otherwise survival inventory.
    pub(crate) fn open_inventory(&mut self) {
        if self.instabuild() {
            self.open_creative();
        } else {
            self.screens.show();
        }
    }

    pub(crate) fn open_creative(&mut self) {
        self.screens.hide();
        let rows = self.hotbar_rows();
        let params = self.creative_params();
        let c = &mut self.creative_ui;
        c.open = true;
        c.bar_drag = false;
        c.tab_pressed = false;
        c.update_lists(&self.data, &self.creative_rec, params, &rows);
        if !c.visible(&self.data, c.tab) {
            c.tab = self.data.creative.tabs.iter().position(|t| t.name == "minecraft:building_blocks").unwrap_or(0);
        }
        let tab = c.tab;
        c.select(&self.data, tab, &rows);
    }

    /// Closing: `container_close` (window 0); the carried item disappears (§ 14, point 3).
    pub(crate) fn close_creative(&mut self) {
        if !self.creative_ui.open {
            return;
        }
        let _ = self.session.link.send_container_close(0);
        self.creative_ui.open = false;
        self.creative_ui.bar_drag = false;
        self.inventory.carried = None;
        self.grab(true);
    }

    /// Tick: survival ↔ creative switch according to the abilities, rebuilding of the lists (§ 1, § 3.1).
    pub(crate) fn creative_tick(&mut self) {
        self.creative_ui.drops = (self.creative_ui.drops - 1).max(0);
        if let Some(spec) = rc_data::env_str!("RUSTCRAFT_HOTBARTEST") {
            self.hotbar_test(spec);
        }
        if self.creative_ui.open {
            if !self.instabuild() {
                self.creative_ui.open = false;
                self.inventory.carried = None;
                self.screens.show();
                return;
            }
            let rows = self.hotbar_rows();
            let params = self.creative_params();
            self.creative_ui.update_lists(&self.data, &self.creative_rec, params, &rows);
            if self.creative_ui.kind(&self.data) == CreativeKind::Hotbar {
                self.creative_ui.shown = rows;
            }
            if self.creative_ui.test.1 {
                self.creative_clicks_test();
            }
        } else if self.screens.open && self.inventory.container.is_none() && self.instabuild() {
            self.open_creative();
        }
        if let Some(spec) = rc_data::env_str!("RUSTCRAFT_CREATIVETEST") {
            self.creative_test(spec);
        }
    }

    /// Keyboard-less demo: `RUSTCRAFT_CREATIVETEST=<tab>[:<search>]` opens the creative screen
    /// on the named tab (`minecraft:combat`, `search:diamond`…) after connecting.
    fn creative_test(&mut self, spec: &str) {
        if !self.positioned || self.creative_ui.open || self.creative_ui.test.1 {
            return;
        }
        self.creative_ui.test.0 += 1;
        if self.creative_ui.test.0 < 200 || !self.instabuild() {
            return;
        }
        self.creative_ui.test.1 = true;
        self.open_creative();
        let (tab, query) = spec.split_once(':').map_or((spec, ""), |(a, b)| (a, b));
        let name = if tab.contains(':') { tab.to_owned() } else { format!("minecraft:{tab}") };
        let rows = self.hotbar_rows();
        if let Some(k) = self.data.creative.tabs.iter().position(|t| t.name == name) {
            self.creative_ui.select(&self.data, k, &rows);
        }
        if !query.is_empty() {
            self.creative_ui.search.insert(query);
            self.creative_search_changed();
        }
    }

    /// Hotbar demo: `RUSTCRAFT_HOTBARTEST=c3,x3` (save then load of
    /// hotbar 3), one action every 40 ticks after connecting.
    fn hotbar_test(&mut self, spec: &str) {
        if !self.positioned {
            return;
        }
        self.creative_ui.test.2 += 1;
        let t = self.creative_ui.test.2;
        if t < 240 || !t.is_multiple_of(40) {
            return;
        }
        let Some(step) = spec.split(',').nth((t / 40 - 6) as usize) else { return };
        let (k, d) = step.split_at(1);
        let Ok(i) = d.parse::<usize>() else { return };
        let key = if k == "c" {
            crate::keys::SAVE_TOOLBAR
        } else if k == "x" {
            crate::keys::LOAD_TOOLBAR
        } else {
            return;
        };
        if k == "x" {
            // Hotbar emptied first to see the loading.
            for c in 36..45 {
                self.inventory.menu[c] = None;
                self.send_slot(c as i16, None);
            }
        }
        self.binds.force(key, true);
        eprintln!("hotbar demo: {step} → {}", self.hotbar_activator(i.saturating_sub(1)));
        self.binds.force(key, false);
    }

    /// Demo clicks: `RUSTCRAFT_CREATIVECLICKS=g3:0:0,m36:0:0,o:0:0,t:0:1`
    /// (grid, menu slot 0, outside the image, trash), one every 10 ticks after opening.
    fn creative_clicks_test(&mut self) {
        let Some(spec) = rc_data::env_str!("RUSTCRAFT_CREATIVECLICKS") else { return };
        self.creative_ui.test.3 += 1;
        let t = self.creative_ui.test.3;
        if !t.is_multiple_of(10) {
            return;
        }
        let Some(step) = spec.split(',').nth((t / 10 - 1) as usize) else { return };
        let v: Vec<&str> = step.split(':').collect();
        let [h, b, m] = v[..] else { return };
        let (Ok(b), Ok(m)) = (b.parse::<u8>(), m.parse::<i32>()) else { return };
        let hit = match h.split_at(1) {
            ("g", i) => i.parse().ok().map(Hit::Grid),
            ("m", i) => i.parse().ok().map(Hit::Menu),
            ("t", _) => Some(Hit::Trash),
            _ => None,
        };
        eprintln!("creative demo: {hit:?} button {b} mode {m}");
        self.creative_click(hit, hit.is_none(), b, m);
        eprintln!(
            "  carried {:?}, hotbar {:?}",
            self.inventory.carried.as_ref().map(|c| (c.item, c.count)),
            self.inventory.menu[36..45].iter().map(|s| s.as_ref().map(|c| (c.item, c.count))).collect::<Vec<_>>()
        );
    }

    fn creative_search_changed(&mut self) {
        if let Some(r) = &self.renderer {
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
            self.creative_ui.ensure_index(&r.gui, &it, &self.screens.hasher.enchantments, &self.creative_rec);
        }
        self.creative_ui.search_changed(&self.data);
    }

    /// Input while the creative screen is open; false if the event must continue its course.
    pub(crate) fn creative_event(&mut self, e: &WindowEvent) -> bool {
        // Text validated by the IME: typing in the search (search tab only).
        if let Some(t) = crate::ime::commit(e) {
            if self.creative_ui.kind(&self.data) != CreativeKind::Search {
                return false;
            }
            let before = self.creative_ui.search.text.clone();
            self.creative_ui.search.insert(t);
            if self.creative_ui.search.text != before {
                self.creative_search_changed();
            }
            return true;
        }
        match e {
            WindowEvent::CursorMoved { position, .. } => {
                self.cursor = Some((position.x, position.y));
                if let (Some(r), Some(m)) = (&self.renderer, self.gui_mouse()) {
                    self.creative_ui.bar_motion(&r.gui, m);
                    self.creative_motion(m);
                }
                false
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let y = match delta {
                    MouseScrollDelta::LineDelta(_, y) => *y,
                    MouseScrollDelta::PixelDelta(p) => (p.y / 40.0) as f32,
                };
                self.creative_ui.wheel(&self.data, y);
                true
            }
            WindowEvent::MouseInput { state, button, .. } => {
                let b = match button {
                    MouseButton::Left => 0,
                    MouseButton::Right => 1,
                    MouseButton::Middle => 2,
                    _ => return true,
                };
                let rows = self.hotbar_rows();
                let (Some(r), Some(m)) = (&self.renderer, self.gui_mouse()) else { return true };
                if *state == ElementState::Pressed {
                    if b == 0 && self.creative_ui.tab_at(&r.gui, &self.data, m).is_some() {
                        self.creative_ui.tab_pressed = true;
                        return true;
                    }
                    if b == 0 && self.creative_ui.bar_press(&r.gui, &self.data, m) {
                        return true;
                    }
                    self.creative_press(b, m);
                } else {
                    if b == 0 && self.creative_ui.bar_drag {
                        self.creative_ui.bar_drag = false;
                        return true;
                    }
                    if b == 0 && std::mem::take(&mut self.creative_ui.tab_pressed) {
                        if let Some(k) = self.creative_ui.tab_at(&r.gui, &self.data, m) {
                            self.creative_ui.select(&self.data, k, &rows);
                            if self.creative_ui.kind(&self.data) == CreativeKind::Search {
                                self.creative_search_changed();
                            }
                        }
                        return true;
                    }
                    self.creative_release(b, m);
                }
                true
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return true;
                }
                let Some(k) = crate::widgets::edit_key(event, self.mods) else { return true };
                let search = self.creative_ui.kind(&self.data) == CreativeKind::Search;
                if k == KeyCode::Escape {
                    self.close_creative();
                    return true;
                }
                if !search {
                    // Game keys (close, chat, drop, digits): physical code, even
                    // with Ctrl (Ctrl + Drop drops the whole stack).
                    let Some(k) = crate::widgets::physical_key(event) else { return true };
                    let inp = crate::keys::Input::Key(k);
                    if self.binds.is(crate::keys::INVENTORY, inp) {
                        self.close_creative();
                        return true;
                    }
                    // "Chat" key: search tab, typing ignored (§ 4.3).
                    if self.binds.is(crate::keys::CHAT, inp) {
                        if let Some(s) = self.data.creative.tabs.iter().position(|t| t.kind == CreativeKind::Search) {
                            let rows = self.hotbar_rows();
                            self.creative_ui.select(&self.data, s, &rows);
                            self.creative_search_changed();
                        }
                        return true;
                    }
                    self.creative_key(k);
                    return true;
                }
                if self.creative_search_key(k) {
                    return true;
                }
                let (_, changed) = self.creative_ui.search.key(k, self.mods);
                let mut changed = changed;
                if let Some(t) = crate::widgets::typed_text(event, self.mods) {
                    let before = self.creative_ui.search.text.clone();
                    self.creative_ui.search.insert(t);
                    changed |= self.creative_ui.search.text != before;
                }
                if changed {
                    self.creative_search_changed();
                }
                true
            }
            _ => false,
        }
    }

    // --- Creative clicks (§ 7): local prediction then `set_creative_mode_slot`. ---

    fn creative_rules(&self) -> crate::screens::ItemRules<'_> {
        crate::screens::ItemRules {
            infos: &self.data.items,
            names: &self.item_names,
            ids: &self.component_ids,
            data: &self.data,
        }
    }

    fn max_stack(&self, s: &ItemStack) -> i32 {
        crate::menu::Rules::max_stack(&self.creative_rules(), s)
    }

    /// Locked paper (`creative_slot_lock`): cannot be taken (§ 3.4).
    fn locked(&self, s: &ItemStack) -> bool {
        self.data
            .registry("minecraft:data_component_type")
            .and_then(|r| r.entries.iter().position(|n| n == "minecraft:creative_slot_lock"))
            .is_some_and(|id| s.added.iter().any(|c| usize::from(c.0) == id))
    }

    fn send_slot(&self, slot: i16, s: Option<&ItemStack>) {
        if self.instabuild() {
            if rc_data::env_flag!("RUSTCRAFT_INVLOG") {
                eprintln!("creative: slot {slot} ← {s:?}");
            }
            let _ = self.session.link.send_set_creative_slot(slot, s);
        }
    }

    /// Drop (slot −1) if the local limiter allows it (§ 7.1).
    fn creative_throw(&mut self, s: ItemStack) {
        if self.creative_ui.drops < 1280 && self.instabuild() {
            self.send_slot(-1, Some(&s));
            self.creative_ui.drops += 20;
        }
    }

    /// Menu 0 broadcast: one packet per changed slot (§ 7.1).
    fn creative_broadcast(&self, before: &[Option<ItemStack>]) {
        for i in crate::menu::changed(before, &self.inventory.menu) {
            if (1..46).contains(&i) {
                self.send_slot(i as i16, self.inventory.menu[i].as_ref());
            }
        }
    }

    fn with_count(s: &ItemStack, n: i32) -> ItemStack {
        ItemStack { count: n, ..s.clone() }
    }

    fn carried_minus_one(&mut self) {
        if let Some(c) = &mut self.inventory.carried {
            c.count -= 1;
            if c.count <= 0 {
                self.inventory.carried = None;
            }
        }
    }

    /// Logical click on the player's menu 0 (survival logic, creative mode for cloning).
    fn menu_click(&mut self, slot: i16, button: i8, mode: i32) {
        let rules = crate::screens::ItemRules {
            infos: &self.data.items,
            names: &self.item_names,
            ids: &self.component_ids,
            data: &self.data,
        };
        let inv = &mut self.inventory;
        let mut m = crate::menu::Menu {
            kind: crate::menu::Kind::Player,
            slots: &mut inv.menu,
            carried: &mut inv.carried,
            rules: &rules,
            creative: true,
        };
        let mut hotbar = |b: i8| match b {
            0..=8 => Some(36 + b as usize),
            40 => Some(45),
            _ => None,
        };
        m.click(slot, button, mode, &mut hotbar);
    }

    /// Click translated into (target slot, button, mode) (§ 7.2, § 7.3).
    pub(crate) fn creative_click(&mut self, hit: Option<Hit>, outside: bool, button: u8, mode: i32) {
        let before = self.inventory.menu.clone();
        let inventory_tab = self.creative_ui.kind(&self.data) == CreativeKind::Inventory;
        // Bundle moved quickly or swapped: "no item selected" announced before the click, like
        // the container screens (`screens.rs`), with the slot index in the creative screen.
        if matches!(mode, 1 | 2)
            && let Some(h) = hit
            && let Some(index) = creative_slot_index(h, inventory_tab)
        {
            let st = match h {
                Hit::Grid(i) => self.creative_ui.grid_stack(i),
                Hit::Menu(i) => self.inventory.menu.get(i).and_then(Option::as_ref),
                Hit::Trash => None,
            };
            let name = |s: &ItemStack| usize::try_from(s.item).ok().and_then(|i| self.item_names.get(i));
            if st.and_then(name).map(String::as_str).is_some_and(crate::bundle::is_bundle_name) {
                let _ = self.session.link.send_bundle_item_selected(index, -1);
            }
        }
        match hit {
            None => {
                if !outside {
                    return;
                }
                let Some(c) = self.inventory.carried.clone() else { return };
                if inventory_tab || button == 0 {
                    self.inventory.carried = None;
                    self.creative_throw(c);
                } else if button == 1 {
                    self.carried_minus_one();
                    self.creative_throw(Self::with_count(&c, 1));
                }
                return;
            }
            Some(Hit::Trash) => {
                if mode == 1 {
                    for i in 0..self.inventory.menu.len() {
                        self.inventory.menu[i] = None;
                        self.send_slot(i as i16, None);
                    }
                } else {
                    self.inventory.carried = None;
                }
                return;
            }
            Some(Hit::Grid(i)) => {
                let st = self.creative_ui.grid_stack(i).cloned();
                if st.as_ref().is_some_and(|s| self.locked(s)) {
                    return;
                }
                let max = |s: &ItemStack| self.max_stack(s);
                match (mode, st) {
                    (2, Some(s)) => {
                        let t = if button == 40 { 45 } else { 36 + usize::from(button.min(8)) };
                        self.inventory.menu[t] = Some(Self::with_count(&s, max(&s)));
                    }
                    (3, Some(s)) if self.inventory.carried.is_none() => {
                        self.inventory.carried = Some(Self::with_count(&s, max(&s)));
                    }
                    (4, Some(s)) => {
                        let n = if button == 1 { max(&s) } else { 1 };
                        self.creative_throw(Self::with_count(&s, n));
                    }
                    (0 | 1, st) => {
                        let shift = mode == 1;
                        let same =
                            matches!((&self.inventory.carried, &st), (Some(c), Some(s)) if crate::creative::same(c, s));
                        if same {
                            let m = max(st.as_ref().expect("same stack"));
                            if button == 0 {
                                if let Some(c) = &mut self.inventory.carried {
                                    c.count = if shift { m } else { (c.count + 1).min(m) };
                                }
                            } else {
                                self.carried_minus_one();
                            }
                        } else if let (None, Some(s)) = (&self.inventory.carried, &st) {
                            let n = if shift { max(s) } else { s.count };
                            self.inventory.carried = Some(Self::with_count(s, n));
                        } else if button == 0 {
                            self.inventory.carried = None;
                        } else {
                            self.carried_minus_one();
                        }
                    }
                    _ => {}
                }
            }
            Some(Hit::Menu(i)) => {
                if mode == 4 {
                    if self.inventory.carried.is_none()
                        && let Some(s) = self.inventory.menu[i].clone()
                    {
                        let n = if button == 1 { if inventory_tab { self.max_stack(&s) } else { s.count } } else { 1 }
                            .min(s.count);
                        self.inventory.menu[i] = (s.count > n).then(|| Self::with_count(&s, s.count - n));
                        self.creative_throw(Self::with_count(&s, n));
                    }
                } else if mode == 1 && !inventory_tab {
                    // Shift+click on the hotbar in a grid tab: clears the slot.
                    self.inventory.menu[i] = None;
                } else {
                    self.menu_click(i as i16, button as i8, mode);
                }
            }
        }
        self.creative_broadcast(&before);
    }

    /// End of a drag over menu 0 slots (§ 6.5, § 7.2).
    fn creative_drag_end(&mut self, slots: &[usize], kind: u8) {
        let before = self.inventory.menu.clone();
        let rules = crate::screens::ItemRules {
            infos: &self.data.items,
            names: &self.item_names,
            ids: &self.component_ids,
            data: &self.data,
        };
        let inv = &mut self.inventory;
        let mut m = crate::menu::Menu {
            kind: crate::menu::Kind::Player,
            slots: &mut inv.menu,
            carried: &mut inv.carried,
            rules: &rules,
            creative: true,
        };
        m.drag_end(slots, kind);
        if self.creative_ui.kind(&self.data) == CreativeKind::Inventory {
            self.creative_broadcast(&before);
        } else {
            for i in 36..45 {
                self.send_slot(i as i16, self.inventory.menu[i].as_ref());
            }
        }
    }

    fn creative_press(&mut self, button: u8, m: (f32, f32)) {
        let Some(r) = &self.renderer else { return };
        let hit = self.creative_ui.hovered(&r.gui, &self.data, m);
        let outside = self.creative_ui.outside(&r.gui, &self.data, m);
        if self.creative_ui.kind(&self.data) == CreativeKind::Search && matches!(hit, Some(Hit::Grid(_))) {
            // Click on the grid: field cursor at the end of the text (§ 4.3).
            let n = self.creative_ui.search.text.len();
            self.creative_ui.search.cursor = n;
            self.creative_ui.search.anchor = n;
        }
        if self.creative_ui.drag.is_some() {
            return;
        }
        if self.inventory.carried.is_some() {
            if hit.is_some() || outside {
                self.creative_ui.drag = Some((button, Vec::new()));
            }
            return;
        }
        let shift = self.mods.shift_key();
        match button {
            2 => self.creative_click(hit, outside, 2, 3),
            0 | 1 => self.creative_click(hit, outside, button, i32::from(shift)),
            _ => {}
        }
    }

    fn creative_release(&mut self, button: u8, m: (f32, f32)) {
        let Some(r) = &self.renderer else { return };
        let hit = self.creative_ui.hovered(&r.gui, &self.data, m);
        let outside = self.creative_ui.outside(&r.gui, &self.data, m);
        let Some((b, slots)) = self.creative_ui.drag.take() else { return };
        if b != button {
            return;
        }
        if slots.len() > 1 {
            self.creative_drag_end(&slots, button.min(2));
            return;
        }
        let shift = self.mods.shift_key();
        if button == 2 {
            self.creative_click(hit, outside, 2, 3);
        } else {
            self.creative_click(hit, outside, button, i32::from(shift && hit.is_some()));
        }
    }

    fn creative_motion(&mut self, m: (f32, f32)) {
        let Some(r) = &self.renderer else { return };
        let Some(Hit::Menu(i)) = self.creative_ui.hovered(&r.gui, &self.data, m) else { return };
        let Some((b, slots)) = &self.creative_ui.drag else { return };
        if slots.contains(&i) {
            return;
        }
        let (kind, taken) = (b.min(&2).to_owned(), slots.len());
        let rules = self.creative_rules();
        let mut carried = self.inventory.carried.clone();
        let mut menu = self.inventory.menu.clone();
        let mm = crate::menu::Menu {
            kind: crate::menu::Kind::Player,
            slots: &mut menu,
            carried: &mut carried,
            rules: &rules,
            creative: true,
        };
        if mm.drag_accepts(i, kind, taken, true)
            && let Some((_, s)) = &mut self.creative_ui.drag
        {
            s.push(i);
        }
    }

    /// Keys while hovering outside the search: 1-9 / F swap, Q drops (Ctrl: whole stack).
    fn creative_key(&mut self, k: KeyCode) {
        let Some(m) = self.gui_mouse() else { return };
        let Some(r) = &self.renderer else { return };
        let Some(hit) = self.creative_ui.hovered(&r.gui, &self.data, m) else { return };
        if self.creative_ui.drag.is_some() {
            return;
        }
        let (digit, drop, _) = self.screen_key_actions(k);
        if let Some(d) = digit {
            if self.inventory.carried.is_none() {
                self.creative_click(Some(hit), false, d as u8, 2);
            }
        } else if let Some(ctrl) = drop
            && self.inventory.carried.is_none()
        {
            self.creative_click(Some(hit), false, u8::from(ctrl), 4);
        }
    }

    /// Search tab: a digit key swaps first if the hovered slot is not an
    /// empty grid slot (§ 4.3); true if the keystroke is consumed.
    fn creative_search_key(&mut self, k: KeyCode) -> bool {
        let Some(d) = crate::DIGITS.iter().position(|&x| x == k) else { return false };
        let (Some(m), Some(r)) = (self.gui_mouse(), &self.renderer) else { return false };
        let Some(hit) = self.creative_ui.hovered(&r.gui, &self.data, m) else { return false };
        let empty_grid = matches!(hit, Hit::Grid(i) if self.creative_ui.grid_stack(i).is_none());
        if empty_grid || self.inventory.carried.is_some() {
            return false;
        }
        self.creative_click(Some(hit), false, d as u8, 2);
        true
    }

    // --- Saved hotbars (§ 8). ---

    fn hotbar_names(&self) -> crate::hotbars::Names<'_> {
        let reg = |n: &str| self.data.registry(n).map(|r| &r.entries[..]).unwrap_or(&[]);
        crate::hotbars::Names {
            items: &self.item_names,
            components: reg("minecraft:data_component_type"),
            potions: reg("minecraft:potion"),
            effects: reg("minecraft:mob_effect"),
            paintings: self.creative_rec.names("minecraft:painting_variant"),
            instruments: self.creative_rec.names("minecraft:instrument"),
            enchantments: self.creative_rec.names("minecraft:enchantment"),
            banner_patterns: self.creative_rec.names("minecraft:banner_pattern"),
        }
    }

    fn key_text(&self, option: &str) -> String {
        let name = self.options.s(&format!("key_{option}")).to_owned();
        match &self.renderer {
            Some(r) => crate::options::key_label(&r.gui.lang, &name),
            None => name,
        }
    }

    /// Contents of the hotbars tab: 9 rows of 9 (§ 3.4).
    fn hotbar_rows(&mut self) -> Vec<Option<ItemStack>> {
        let mut out = Vec::with_capacity(81);
        let mut hb = std::mem::take(&mut self.hotbars);
        for r in 0..9 {
            if hb.is_empty(r) {
                let mut row = vec![None; 9];
                row[r] = self.locked_paper(r);
                out.extend(row);
            } else {
                let names = self.hotbar_names();
                out.extend(hb.row(r, &names));
            }
        }
        self.hotbars = hb;
        out
    }

    /// "Creative slot lock" paper named `inventory.hotbarInfo` (§ 3.4).
    fn locked_paper(&self, r: usize) -> Option<ItemStack> {
        use rc_proto::nbt::Nbt;
        let comps = self.data.registry("minecraft:data_component_type")?;
        let id = |n: &str| comps.entries.iter().position(|e| e == n).map(|i| i as u16);
        let item = self.item_names.iter().position(|n| n == "minecraft:paper")? as i32;
        let name = Nbt::Compound(vec![
            ("translate".into(), Nbt::String("inventory.hotbarInfo".into())),
            (
                "with".into(),
                Nbt::List(vec![
                    Nbt::String(self.key_text("key.saveToolbarActivator")),
                    Nbt::String(self.key_text(&format!("key.hotbar.{}", r + 1))),
                ]),
            ),
        ]);
        let mut b = Vec::new();
        rc_proto::nbt::write(&mut b, Some(&name));
        Some(ItemStack {
            item,
            count: 1,
            added: vec![
                (id("minecraft:creative_slot_lock")?, Vec::new().into_boxed_slice()),
                (id("minecraft:custom_name")?, b.into_boxed_slice()),
            ],
            removed: Vec::new(),
        })
    }

    /// Hotbar slot key in game (§ 8.1): X + digit loads, C + digit saves;
    /// true if the key was used.
    pub(crate) fn hotbar_activator(&mut self, i: usize) -> bool {
        if !self.instabuild() || self.env.game_mode == 3 {
            return false;
        }
        if self.binds.down(crate::keys::LOAD_TOOLBAR) {
            let mut hb = std::mem::take(&mut self.hotbars);
            let names = self.hotbar_names();
            let row = hb.row(i, &names);
            drop(names);
            self.hotbars = hb;
            for (c, s) in row.into_iter().enumerate() {
                self.inventory.menu[36 + c] = s;
                self.send_slot(36 + c as i16, self.inventory.menu[36 + c].as_ref());
            }
            return true;
        }
        if self.binds.down(crate::keys::SAVE_TOOLBAR) {
            let row: Vec<Option<ItemStack>> = self.inventory.menu[36..45].to_vec();
            let mut hb = std::mem::take(&mut self.hotbars);
            let names = self.hotbar_names();
            if let Err(e) = hb.save(i, &row, &names) {
                eprintln!("hotbar.nbt: {e}");
            }
            drop(names);
            self.hotbars = hb;
            let msg = rc_text::Component::translate(
                "inventory.hotbarSaved",
                vec![
                    rc_text::Arg::Text(self.key_text("key.loadToolbarActivator")),
                    rc_text::Arg::Text(self.key_text(&format!("key.hotbar.{}", i + 1))),
                ],
            );
            self.chat.action_bar(msg);
            return true;
        }
        false
    }
}

/// Index of a slot in the creative screen's menu, as the game announces it to the server:
/// grid 0..44, hotbar 45..53; the inventory tab recreates its slots without giving them an index
/// (all 0, game quirk reproduced). Trash: never (always empty).
fn creative_slot_index(hit: Hit, inventory_tab: bool) -> Option<i32> {
    match hit {
        Hit::Grid(i) => i32::try_from(i).ok(),
        Hit::Menu(_) if inventory_tab => Some(0),
        Hit::Menu(i) => i32::try_from(i).ok().filter(|i| (36..45).contains(i)).map(|i| i + 9),
        Hit::Trash => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{Hit, creative_slot_index as idx};

    #[test]
    fn creative_bundle_unselect_slot_index() {
        assert_eq!(idx(Hit::Grid(7), false), Some(7));
        assert_eq!(idx(Hit::Menu(36), false), Some(45));
        assert_eq!(idx(Hit::Menu(44), false), Some(53));
        assert_eq!(idx(Hit::Menu(12), true), Some(0));
        assert_eq!(idx(Hit::Menu(45), true), Some(0));
        assert_eq!(idx(Hit::Trash, true), None);
    }
}