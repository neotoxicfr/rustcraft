//! Creative inventory screen (`docs/specs/creative-inventory.md` § 1–6): tabs, grid,
//! scrollbar, search, inventory tab with trash, tooltips.

use rc_data::CreativeKind;
use rc_proto_v777::item::ItemStack;
use rc_text::{Component, StyledChar};

use crate::creative::{Lists, Params, Received, SearchIndex};
use crate::gui::{Blend, Gui, Pre};
use crate::hud::HotbarItems;
use crate::inventory::PlayerInventory;
use crate::menu::{self, Kind};
use crate::screens::{Cached, decorate, draw_tooltip, refresh, tooltip_lines};
use crate::widgets::TextField;

/// Tab `container/creative_inventory/tab_<top|bottom>_<unselected|selected>_<column + 1>`
/// (26 × 32): sprite resolved once for the 7 columns, looked up by name beyond.
fn tab_sprite(gui: &mut Gui, top: bool, selected: bool, column: usize, x: f32, y: f32) {
    if column < 7 {
        return gui.sprite_pre(Pre::creative_tab(top, selected, column), x, y, 26.0, 32.0, 0xffff_ffff);
    }
    let (row, sel) = (if top { "top" } else { "bottom" }, if selected { "selected" } else { "unselected" });
    gui.sprite(&format!("container/creative_inventory/tab_{row}_{sel}_{}", column + 1), x, y, 26.0, 32.0, 0xffff_ffff);
}

/// Size of the image (§ 6.1).
pub const W: i32 = 195;
pub const H: i32 = 136;

/// Hovered or targeted slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hit {
    /// Slot of the creative grid (0..45).
    Grid(usize),
    /// Slot of the player's menu 0.
    Menu(usize),
    /// Trash of the inventory tab.
    Trash,
}

pub struct CreativeScreen {
    pub open: bool,
    /// Selected tab, remembered for the session (§ 1).
    pub tab: usize,
    /// Scroll `s ∈ [0, 1]` (§ 6.3).
    pub scroll: f32,
    pub bar_drag: bool,
    pub lists: Lists,
    /// Stacks of the current tab's grid (empty: empty slot).
    pub shown: Vec<Option<ItemStack>>,
    pub search: TextField,
    index: Option<((usize, u64), SearchIndex)>,
    /// Visible tags of the last `#` query (§ 4.2).
    pub visible_tags: Vec<String>,
    /// Item tags received (name, items), copied with the index.
    index_tags: Vec<(String, Vec<i32>)>,
    /// Counter of list rebuilds (search index to redo).
    rebuilt: u64,
    /// Left press consumed by a tab (selection on release, § 6.2).
    pub tab_pressed: bool,
    cache: Vec<Option<Cached>>,
    icons: Vec<Option<Cached>>,
    carried_cache: Option<Cached>,
    tooltip: Option<(ItemStack, usize, Vec<Vec<StyledChar>>, i32)>,
    /// Demos: ticks waited, opening done, hotbar ticks, click ticks.
    pub test: (u32, bool, u32, u32),
    /// Drag in progress: button, menu 0 slots retained.
    pub drag: Option<(u8, Vec<usize>)>,
    /// Local limiter of drops (§ 7.1).
    pub drops: i32,
    /// Entity of the local player, drawn in the inventory tab.
    pub player_model: Option<i32>,
}

impl Default for CreativeScreen {
    fn default() -> Self {
        let mut search = TextField::new(Vec::new());
        search.max_len = 50;
        search.bordered = false;
        search.text_color = 0xffff_ffff;
        search.invert_highlight = false;
        CreativeScreen {
            open: false,
            tab: 0,
            scroll: 0.0,
            bar_drag: false,
            lists: Lists::default(),
            shown: Vec::new(),
            search,
            index: None,
            visible_tags: Vec::new(),
            index_tags: Vec::new(),
            rebuilt: 0,
            tab_pressed: false,
            cache: Vec::new(),
            icons: Vec::new(),
            carried_cache: None,
            tooltip: None,
            test: (0, false, 0, 0),
            drag: None,
            drops: 0,
            player_model: None,
        }
    }
}

/// Corner of the image (§ 6.1).
pub fn origin(gui: &Gui) -> (i32, i32) {
    ((gui.width as i32 - W) / 2, (gui.height as i32 - H) / 2)
}

/// Relative x of a tab (§ 6.2).
fn tab_x(t: &rc_data::CreativeTab) -> i32 {
    let c = i32::from(t.column);
    if t.right { W - 27 * (7 - c) + 1 } else { 27 * c }
}

/// Position of a menu 0 slot in the inventory tab (§ 5), `None` if off screen.
pub fn inventory_slot_pos(i: usize) -> Option<(i32, i32)> {
    let i32_ = |v: usize| v as i32;
    Some(match i {
        5..=8 => {
            let p = i32_(i - 5);
            (54 + 54 * (p / 2), 6 + 27 * (p % 2))
        }
        45 => (35, 20),
        9..=35 => (9 + 18 * (i32_(i - 9) % 9), 54 + 18 * (i32_(i - 9) / 9)),
        36..=44 => (9 + 18 * i32_(i - 36), 112),
        _ => return None,
    })
}

/// Relative position of a slot (§ 5, § 6.1).
pub fn hit_pos(h: Hit) -> Option<(i32, i32)> {
    match h {
        Hit::Grid(i) => Some((9 + 18 * (i as i32 % 9), 18 + 18 * (i as i32 / 9))),
        Hit::Menu(i) => inventory_slot_pos(i),
        Hit::Trash => Some((173, 112)),
    }
}

impl CreativeScreen {
    pub fn kind(&self, data: &rc_data::VersionData) -> CreativeKind {
        data.creative.tabs.get(self.tab).map_or(CreativeKind::Category, |t| t.kind)
    }

    /// Displayed tab (§ 2.3).
    pub fn visible(&self, data: &rc_data::VersionData, k: usize) -> bool {
        data.creative
            .tabs
            .get(k)
            .is_some_and(|t| t.kind != CreativeKind::Category || self.lists.tab.get(k).is_some_and(|l| !l.is_empty()))
    }

    fn default_tab(data: &rc_data::VersionData) -> usize {
        data.creative.tabs.iter().position(|t| t.name == "minecraft:building_blocks").unwrap_or(0)
    }

    /// Displayed slots of the current tab.
    pub fn hits(&self, data: &rc_data::VersionData) -> Vec<Hit> {
        if self.kind(data) == CreativeKind::Inventory {
            let mut v: Vec<Hit> = (5..46).map(Hit::Menu).collect();
            v.push(Hit::Trash);
            v
        } else {
            (0..45).map(Hit::Grid).chain((36..45).map(Hit::Menu)).collect()
        }
    }

    /// Hovered slot, if the cursor is over a slot.
    pub fn hovered(&self, gui: &Gui, data: &rc_data::VersionData, mouse: (f32, f32)) -> Option<Hit> {
        let (gx, gy) = origin(gui);
        let (mx, my) = (mouse.0.floor() as i32 - gx, mouse.1.floor() as i32 - gy);
        self.hits(data).into_iter().find(|&h| {
            let Some((x, y)) = hit_pos(h) else { return false };
            mx >= x - 1 && mx < x + 17 && my >= y - 1 && my < y + 17
        })
    }

    /// Tab under the cursor for a click (§ 6.2).
    pub fn tab_at(&self, gui: &Gui, data: &rc_data::VersionData, mouse: (f32, f32)) -> Option<usize> {
        let (gx, gy) = origin(gui);
        let (mx, my) = (mouse.0 - gx as f32, mouse.1 - gy as f32);
        data.creative.tabs.iter().enumerate().find_map(|(k, t)| {
            let x = tab_x(t) as f32;
            let y = if t.row == 0 { -32.0 } else { H as f32 };
            (self.visible(data, k) && x <= mx && mx <= x + 26.0 && y <= my && my <= y + 32.0).then_some(k)
        })
    }

    /// Tab hovered for the tooltip (§ 6.2).
    fn tab_hover(&self, gui: &Gui, data: &rc_data::VersionData, mouse: (f32, f32)) -> Option<usize> {
        let (gx, gy) = origin(gui);
        let (mx, my) = (mouse.0 - gx as f32, mouse.1 - gy as f32);
        data.creative.tabs.iter().enumerate().find_map(|(k, t)| {
            let x = tab_x(t) as f32 + 3.0;
            let y = if t.row == 0 { -32.0 } else { H as f32 } + 3.0;
            (self.visible(data, k) && x <= mx && mx < x + 21.0 && y <= my && my < y + 27.0).then_some(k)
        })
    }

    /// Outside the image and not on the current tab (§ 6.2).
    pub fn outside(&self, gui: &Gui, data: &rc_data::VersionData, mouse: (f32, f32)) -> bool {
        let (gx, gy) = origin(gui);
        let (mx, my) = (mouse.0.floor() as i32 - gx, mouse.1.floor() as i32 - gy);
        let out = mx < 0 || my < 0 || mx >= W || my >= H;
        out && self.tab_at(gui, data, mouse) != Some(self.tab)
    }

    fn rows_over(&self) -> i32 {
        (self.shown.len() as i32 + 8) / 9 - 5
    }

    pub fn can_scroll(&self, data: &rc_data::VersionData) -> bool {
        data.creative.tabs.get(self.tab).is_some_and(|t| t.scroll) && self.shown.len() > 45
    }

    fn first_row(&self) -> i32 {
        ((self.scroll * self.rows_over() as f32 + 0.5).floor() as i32).max(0)
    }

    /// Stack displayed in grid slot `i`.
    pub fn grid_stack(&self, i: usize) -> Option<&ItemStack> {
        let k = self.first_row() as usize;
        self.shown.get(i + 9 * k)?.as_ref()
    }

    /// Stack of a displayed slot.
    pub fn stack_at(&self, inv: &PlayerInventory, h: Hit) -> Option<ItemStack> {
        match h {
            Hit::Grid(i) => self.grid_stack(i).cloned(),
            Hit::Menu(i) => inv.menu.get(i).cloned().flatten(),
            Hit::Trash => None,
        }
    }

    pub fn wheel(&mut self, data: &rc_data::VersionData, delta: f32) {
        if self.can_scroll(data) {
            self.scroll = (self.scroll - delta / self.rows_over() as f32).clamp(0.0, 1.0);
        }
    }

    /// Scrollbar: press (true if the drag starts), movement.
    pub fn bar_press(&mut self, gui: &Gui, data: &rc_data::VersionData, mouse: (f32, f32)) -> bool {
        let (gx, gy) = origin(gui);
        let (mx, my) = (mouse.0 - gx as f32, mouse.1 - gy as f32);
        let has_bar = data.creative.tabs.get(self.tab).is_some_and(|t| t.scroll);
        let inside = (175.0..189.0).contains(&mx) && (18.0..130.0).contains(&my);
        if has_bar && inside && self.can_scroll(data) {
            self.bar_drag = true;
            self.bar_motion(gui, mouse);
            return true;
        }
        false
    }

    pub fn bar_motion(&mut self, gui: &Gui, mouse: (f32, f32)) {
        if self.bar_drag {
            let gy = origin(gui).1 as f32;
            self.scroll = ((mouse.1 - (gy + 18.0) - 7.5) / (112.0 - 15.0)).clamp(0.0, 1.0);
        }
    }

    /// Grid contents for the current tab; `keep_row` keeps the starting row.
    pub fn reload(&mut self, data: &rc_data::VersionData, hotbars: &[Option<ItemStack>], keep_row: bool) {
        let row = self.first_row();
        self.shown = match self.kind(data) {
            CreativeKind::Category => {
                self.lists.tab.get(self.tab).map(|l| l.iter().cloned().map(Some).collect()).unwrap_or_default()
            }
            CreativeKind::Search => self.filtered(),
            CreativeKind::Hotbar => hotbars.to_vec(),
            CreativeKind::Inventory => Vec::new(),
        };
        self.scroll =
            if keep_row && self.rows_over() > 0 { (row as f32 / self.rows_over() as f32).clamp(0.0, 1.0) } else { 0.0 };
    }

    fn filtered(&mut self) -> Vec<Option<ItemStack>> {
        let q = self.search.value();
        match &self.index {
            Some((_, idx)) if !q.is_empty() => {
                let (hits, tags) = idx.query(&q, &self.index_tags);
                self.visible_tags = tags;
                hits.into_iter().filter_map(|i| self.lists.search.get(i).cloned().map(Some)).collect()
            }
            _ => {
                self.visible_tags.clear();
                self.lists.search.iter().cloned().map(Some).collect()
            }
        }
    }

    /// Choice of a tab (§ 6.2, § 4.3).
    pub fn select(&mut self, data: &rc_data::VersionData, k: usize, hotbars: &[Option<ItemStack>]) {
        let was = self.tab;
        self.tab = k;
        if self.kind(data) == CreativeKind::Search && was != k {
            self.search.text.clear();
            self.search.cursor = 0;
            self.search.anchor = 0;
        }
        self.bar_drag = false;
        self.reload(data, hotbars, false);
    }

    /// Rebuilding the lists if the parameters changed (§ 3.1); on opening and on
    /// every tick of the open screen.
    pub fn update_lists(
        &mut self,
        data: &rc_data::VersionData,
        rec: &Received,
        params: Params,
        hotbars: &[Option<ItemStack>],
    ) {
        if !self.lists.update(data, rec, params) {
            return;
        }
        self.rebuilt += 1;
        self.index = None;
        if self.kind(data) == CreativeKind::Category && !self.visible(data, self.tab) {
            self.select(data, Self::default_tab(data), hotbars);
        } else {
            self.reload(data, hotbars, true);
        }
    }

    /// Search index (§ 4.1), redone after a rebuild or a language change.
    pub fn ensure_index(&mut self, gui: &Gui, it: &HotbarItems<'_>, enchantments: &[String], rec: &Received) {
        let key = (gui.lang.map.len(), self.rebuilt ^ rec.generation.rotate_left(32));
        if self.index.as_ref().is_some_and(|(k, _)| *k == key) {
            return;
        }
        let by_item = crate::creative::tags_by_item(rec);
        let names = it.names;
        let mut idx = SearchIndex::default();
        crate::tooltips_more::set_creative(true);
        for s in &self.lists.search {
            let lines = crate::screens::tooltip_search_lines(s, it, enchantments, gui, crate::screens::advanced());
            idx.lines.push(lines);
            idx.ids.push(usize::try_from(s.item).ok().and_then(|i| names.get(i)).cloned().unwrap_or_default());
            idx.tags.push(by_item.get(&s.item).cloned().unwrap_or_default());
        }
        crate::tooltips_more::set_creative(false);
        self.index_tags = rec.item_tags().to_vec();
        self.index = Some((key, idx));
    }

    /// Search text modified: filter and back to the top (§ 4.2).
    pub fn search_changed(&mut self, data: &rc_data::VersionData) {
        if self.kind(data) == CreativeKind::Search {
            self.shown = self.filtered();
            self.scroll = 0.0;
        }
    }

    /// Drawing (§ 6).
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        gui: &mut Gui,
        queue: &wgpu::Queue,
        inv: &PlayerInventory,
        it: &HotbarItems<'_>,
        mouse: Option<(f32, f32)>,
        enchantments: &[String],
        time: f32,
    ) {
        if !self.open {
            return;
        }
        let data = it.data;
        let tabs = &data.creative.tabs;
        let Some(cur) = tabs.get(self.tab) else { return };
        let (gx, gy) = origin(gui);
        let f = |v: i32| v as f32;
        let hover = mouse.and_then(|m| self.hovered(gui, data, m));
        let kind = cur.kind;
        gui.next_layer();
        crate::widgets::ingame_ui_background(gui);
        // Unselected tabs, then background of the current tab.
        let tab_rect = |t: &rc_data::CreativeTab| {
            let x = gx + tab_x(t);
            let y = if t.row == 0 { gy - 28 } else { gy + H - 4 };
            (x, y)
        };
        for (k, t) in tabs.iter().enumerate() {
            if k == self.tab || !self.visible(data, k) {
                continue;
            }
            let (x, y) = tab_rect(t);
            tab_sprite(gui, t.row == 0, false, usize::from(t.column), f(x), f(y));
        }
        let bg = cur.background.strip_prefix("minecraft:textures/gui/container/").unwrap_or(&cur.background);
        let bg = format!("container_bg/{}", bg.strip_suffix(".png").unwrap_or(bg));
        gui.sprite_region(&bg, 256.0, 256.0, 0.0, 0.0, f(gx), f(gy), f(W), f(H));
        // Inventory tab: local player following the mouse (size 20, offset 1/16).
        if kind == CreativeKind::Inventory
            && let Some(id) = self.player_model
        {
            let rect = [f(gx + 73), f(gy + 6), f(gx + 105), f(gy + 49)];
            let m = mouse.unwrap_or(((rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0));
            let req = crate::special::ModelReq::Entity { id, rect, size: 20.0, offset: 0.0625, mouse: m };
            crate::special::request_model(gui, req);
        }
        if kind == CreativeKind::Search {
            let blink = (time * 1000.0) as i64 / 300 % 2 == 0;
            crate::widgets::draw_field_blink(gui, queue, &self.search, [gx + 82, gy + 6, 80, 9], true, blink);
        }
        if cur.scroll {
            let name = if self.can_scroll(data) { Pre::CREATIVE_SCROLLER } else { Pre::CREATIVE_SCROLLER_DISABLED };
            let y = gy + 18 + ((112 - 17) as f32 * self.scroll).floor() as i32;
            gui.sprite_pre(name, f(gx + 175), f(y), 12.0, 15.0, 0xffff_ffff);
        }
        {
            let (x, y) = tab_rect(cur);
            tab_sprite(gui, cur.row == 0, true, usize::from(cur.column), f(x), f(y));
        }
        // Title (§ 6.1).
        if cur.show_title && kind != CreativeKind::Inventory {
            let title =
                rc_text::styled_chars(&rc_text::flatten(&Component::translate(cur.title.clone(), vec![]), &gui.lang));
            gui.text(queue, &title, f(gx + 8), f(gy + 6), 0xff40_4040, false, 0);
        }
        // Empty slots (inventory tab), back highlight.
        let hits = self.hits(data);
        if kind == CreativeKind::Inventory {
            for &h in &hits {
                if let Hit::Menu(i) = h
                    && inv.menu.get(i).is_some_and(Option::is_none)
                    && let Some(sp) = menu::slot_def(Kind::Player, i, 46).empty_sprite
                    && let Some((x, y)) = hit_pos(h)
                {
                    gui.sprite(sp, f(gx + x), f(gy + y), 16.0, 16.0, 0xffff_ffff);
                }
            }
        }
        if let Some(h) = hover
            && let Some((x, y)) = hit_pos(h)
        {
            gui.sprite("container/slot_highlight_back", f(gx + x - 4), f(gy + y - 4), 24.0, 24.0, 0xffff_ffff);
        }
        // Items (up-to-date layers first), tab icons.
        let stacks: Vec<Option<ItemStack>> = hits.iter().map(|&h| self.stack_at(inv, h)).collect();
        let icon_stacks: Vec<Option<ItemStack>> = tabs.iter().map(|t| icon_stack(t, it)).collect();
        self.cache.resize_with(hits.len(), || None);
        self.icons.resize_with(tabs.len(), || None);
        if let Some(models) = gui.items.as_ref().map(|a| &a.models) {
            for (i, s) in stacks.iter().enumerate() {
                match s {
                    Some(st) => refresh(&mut self.cache[i], st, it, models, i as i32),
                    None => self.cache[i] = None,
                }
            }
            for (i, s) in icon_stacks.iter().enumerate() {
                match s {
                    Some(st) => refresh(&mut self.icons[i], st, it, models, 1000 + i as i32),
                    None => self.icons[i] = None,
                }
            }
            match &inv.carried {
                Some(st) => crate::screens::refresh_held(&mut self.carried_cache, st, it, models, 0, crate::stack::Hold::CARRIED),
                None => self.carried_cache = None,
            }
        }
        gui.stratum(Blend::Premultiplied);
        for (i, &h) in hits.iter().enumerate() {
            if let (Some(c), Some((x, y))) = (&self.cache[i], hit_pos(h))
                && !c.layers.is_empty()
            {
                gui.item(c.key, &c.layers, f(gx + x), f(gy + y), 0.0);
            }
        }
        for (k, t) in tabs.iter().enumerate() {
            if !self.visible(data, k) {
                continue;
            }
            if let Some(c) = &self.icons[k]
                && !c.layers.is_empty()
            {
                let (x, y) = tab_rect(t);
                let dy = if t.row == 0 { 9 } else { 7 };
                gui.item(c.key, &c.layers, f(x + 5), f(y + dy), 0.0);
            }
        }
        gui.stratum(Blend::Normal);
        for (i, &h) in hits.iter().enumerate() {
            if let (Some(c), Some((x, y))) = (&self.cache[i], hit_pos(h)) {
                decorate(gui, queue, c, f(gx + x), f(gy + y), (c.stack.count, false));
            }
        }
        gui.next_layer();
        if let Some(h) = hover
            && let Some((x, y)) = hit_pos(h)
        {
            gui.sprite("container/slot_highlight_front", f(gx + x - 4), f(gy + y - 4), 24.0, 24.0, 0xffff_ffff);
        }
        // Carried item.
        if let (Some((mx, my)), Some(c)) = (mouse, &self.carried_cache) {
            let (x, y) = (mx.floor() - 8.0, my.floor() - 8.0);
            gui.stratum(Blend::Premultiplied);
            if !c.layers.is_empty() {
                gui.item(c.key, &c.layers, x, y, 0.0);
            }
            gui.stratum(Blend::Normal);
            decorate(gui, queue, c, x, y, (c.stack.count, false));
        }
        let Some((mx, my)) = mouse else { return };
        // Tooltips: hovered tab, trash, item (§ 5, § 6.4).
        if inv.carried.is_none() {
            if let Some(k) = self.tab_hover(gui, data, (mx, my)) {
                let t = rc_text::styled_chars(&rc_text::flatten(
                    &Component::translate(tabs[k].title.clone(), vec![]),
                    &gui.lang,
                ));
                let w = rc_text::width(gui, &t);
                draw_tooltip(gui, queue, &[t], w, mx.floor() as i32, my.floor() as i32);
                return;
            }
            if hover == Some(Hit::Trash) {
                let t = rc_text::styled_chars(&rc_text::flatten(
                    &Component::translate("inventory.binSlot", vec![]),
                    &gui.lang,
                ));
                let w = rc_text::width(gui, &t);
                draw_tooltip(gui, queue, &[t], w, mx.floor() as i32, my.floor() as i32);
                return;
            }
        }
        if let (Some(h), None) = (hover, &inv.carried)
            && let Some(st) = self.stack_at(inv, h)
        {
            let fresh = self.tooltip.as_ref().is_some_and(|t| t.0 == st && t.1 == self.tab);
            if !fresh {
                let lines = self.tooltip_for(&st, h, it, enchantments, gui);
                let width = lines.iter().map(|l| rc_text::width(gui, l)).max().unwrap_or(0);
                self.tooltip = Some((st, self.tab, lines, width));
            }
            if let Some((st, _, lines, tw)) = &self.tooltip {
                let info = usize::try_from(st.item).ok().and_then(|i| it.infos.get(i));
                let view = crate::stack::StackRef { stack: st, info, ids: it.ids, data: it.data, hold: crate::stack::Hold::NONE };
                let style = crate::screens::tooltip_style(&view);
                let m = (mx.floor() as i32, my.floor() as i32);
                crate::screens::draw_tooltip_ex(gui, queue, lines, *tw, m, true, style.as_deref());
            }
        }
    }

    /// Creative tooltip (§ 6.4): tab names in blue and visible tags in purple.
    fn tooltip_for(
        &self,
        st: &ItemStack,
        h: Hit,
        it: &HotbarItems<'_>,
        enchantments: &[String],
        gui: &Gui,
    ) -> Vec<Vec<StyledChar>> {
        let data = it.data;
        crate::tooltips_more::set_creative(true);
        let mut lines = tooltip_lines(st, it, enchantments, gui, crate::screens::advanced());
        crate::tooltips_more::set_creative(false);
        let category_grid = self.kind(data) == CreativeKind::Category && matches!(h, Hit::Grid(_));
        if category_grid || lines.is_empty() {
            return lines;
        }
        let line = |c: Component| rc_text::styled_chars(&rc_text::flatten(&c, &gui.lang));
        if self.kind(data) == CreativeKind::Search && matches!(h, Hit::Grid(_)) {
            let mine: Vec<&String> =
                self.index_tags.iter().filter(|(_, ids)| ids.contains(&st.item)).map(|t| &t.0).collect();
            for t in &self.visible_tags {
                if mine.contains(&t) {
                    let mut c = Component::text(format!("#{t}"));
                    c.style.color = Some(0xaa_00aa);
                    lines.insert(1, line(c));
                }
            }
        }
        let mut at = 1;
        for (k, t) in data.creative.tabs.iter().enumerate() {
            if t.kind == CreativeKind::Search || !self.visible(data, k) {
                continue;
            }
            if self.lists.for_search.get(k).is_some_and(|l| l.iter().any(|x| crate::creative::same(x, st))) {
                let mut c = Component::translate(t.title.clone(), vec![]);
                c.style.color = Some(0x55_55ff);
                lines.insert(at, line(c));
                at += 1;
            }
        }
        lines
    }
}

/// Tab icon: simple stack of the extracted item.
fn icon_stack(t: &rc_data::CreativeTab, it: &HotbarItems<'_>) -> Option<ItemStack> {
    let id = it.names.iter().position(|n| *n == t.icon.item)?;
    Some(ItemStack { item: id as i32, count: 1, added: Vec::new(), removed: Vec::new() })
}
