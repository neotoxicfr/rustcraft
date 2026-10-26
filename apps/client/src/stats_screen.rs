//! Statistics screen (`docs/specs/advancements-stats.md` § 5): tab bar, loading,
//! General, Items (column sort) and Mobs lists.

use rc_proto_v777::item::ItemStack;
use rc_text::StyledChar;

use crate::gui::{Blend, Gui};
use crate::hud::HotbarItems;
use crate::screens::{Cached, refresh};
use crate::stats::{self, CUSTOM_STATS, Stats};
use crate::widgets::{self, Entry, Event, List, Panel, Widget, chars, tr};

const DONE: u32 = 1;
const HEADER: i32 = 24;
const FOOTER: i32 = 33;
const ROW_WIDTH: i32 = 280;
const GRAY: u32 = 0xffba_baba;

/// Columns of the Items list: statistic type, header icon, type name.
const COLUMNS: [(i32, &str, &str); 6] = [
    (stats::MINED, "statistics/block_mined", "mined"),
    (stats::BROKEN, "statistics/item_broken", "broken"),
    (stats::CRAFTED, "statistics/item_crafted", "crafted"),
    (stats::USED, "statistics/item_used", "used"),
    (stats::PICKED_UP, "statistics/item_picked_up", "picked_up"),
    (stats::DROPPED, "statistics/item_dropped", "dropped"),
];

fn col_x(c: usize) -> i32 {
    75 + 40 * c as i32
}

/// Items that place a block of another name (item → block).
const ITEM_BLOCKS: [(&str, &str); 14] = [
    ("redstone", "redstone_wire"),
    ("string", "tripwire"),
    ("wheat_seeds", "wheat"),
    ("carrot", "carrots"),
    ("potato", "potatoes"),
    ("beetroot_seeds", "beetroots"),
    ("melon_seeds", "melon_stem"),
    ("pumpkin_seeds", "pumpkin_stem"),
    ("torchflower_seeds", "torchflower_crop"),
    ("pitcher_pod", "pitcher_crop"),
    ("sweet_berries", "sweet_berry_bush"),
    ("cocoa_beans", "cocoa"),
    ("glow_berries", "cave_vines"),
    // The powder snow bucket is the item of the powder snow block.
    ("powder_snow_bucket", "powder_snow"),
];

/// Simple items that bear the name of a block without being the block item.
const SIMPLE_ITEMS: [&str; 1] = ["wheat"];

/// Block placed by an item (name without namespace), `None` if the item places no block.
fn block_of_item(item: &str, is_block: impl Fn(&str) -> bool) -> Option<&str> {
    // Wheat is a simple item (the "wheat" crop is tied to wheat seeds).
    if SIMPLE_ITEMS.contains(&item) {
        return None;
    }
    let b = ITEM_BLOCKS.iter().find(|(i, _)| *i == item).map_or(item, |(_, b)| b);
    is_block(b).then_some(b)
}

/// Item of a block (name without namespace): same name, item that places it under another name,
/// or wall variant of the item (torch, sign, banner, head, coral); `None` otherwise (air).
fn item_of_block(block: &str, is_item: impl Fn(&str) -> bool) -> Option<String> {
    if let Some((i, _)) = ITEM_BLOCKS.iter().find(|(_, b)| *b == block) {
        return Some((*i).to_owned());
    }
    if is_item(block) {
        return Some(block.to_owned());
    }
    let wall = block.replacen("wall_", "", 1);
    (wall != block && is_item(&wall)).then_some(wall)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Back,
}

/// Sort of the Items list: column, descending.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Sort {
    col: usize,
    desc: bool,
}

struct ItemRow {
    item: i32,
    /// Item's block (id of the block registry).
    block: Option<i32>,
    stack: ItemStack,
    cache: Option<Cached>,
}

pub struct StatsScreen {
    pub parent: bool,
    /// First `award_stats` received: lists built.
    loaded: bool,
    generation: u64,
    tab: usize,
    /// Active tabs (General always).
    active: [bool; 3],
    panel: Panel,
    size: (i32, i32),
    /// Custom statistics sorted by translated name: (network id, name).
    general: Vec<(usize, Vec<StyledChar>)>,
    items: Vec<ItemRow>,
    sort: Option<Sort>,
    /// Sort to redo at the next draw (click on a header).
    resort: bool,
    /// Entity types shown (id, translated name).
    mobs: Vec<(i32, String)>,
    tooltip: Option<(ItemStack, Vec<Vec<StyledChar>>, i32)>,
    pub clicks: u32,
    /// Keyboard modifiers (shortcuts of the tab bar).
    pub mods: winit::keyboard::ModifiersState,
    /// Scroll kept per tab.
    scrolls: [f64; 3],
    /// Opening instant (animated dots of the loading).
    opened: std::time::Instant,
}

impl StatsScreen {
    pub fn new(stats: &Stats, parent: bool, keyboard: bool) -> StatsScreen {
        let mut panel = Panel::default();
        panel.keyboard = keyboard;
        StatsScreen {
            parent,
            loaded: false,
            generation: stats.generation,
            tab: 0,
            active: [true, false, false],
            panel,
            size: (-1, -1),
            general: Vec::new(),
            items: Vec::new(),
            sort: None,
            resort: false,
            mobs: Vec::new(),
            tooltip: None,
            clicks: 0,
            mods: winit::keyboard::ModifiersState::empty(),
            scrolls: [0.0; 3],
            opened: std::time::Instant::now(),
        }
    }

    /// First `award_stats` received since opening: lists built (§ 5.1).
    fn sync(&mut self, gui: &Gui, s: &Stats, it: &HotbarItems<'_>) {
        if self.loaded || s.generation == self.generation {
            return;
        }
        self.loaded = true;
        let lang = &gui.lang;
        let mut general: Vec<(usize, String)> = CUSTOM_STATS
            .iter()
            .enumerate()
            .map(|(i, (n, _))| (i, tr(lang, &format!("stat.minecraft.{n}"), &[])))
            .collect();
        general.sort_by(|a, b| a.1.encode_utf16().cmp(b.1.encode_utf16()));
        self.general = general.into_iter().map(|(i, n)| (i, chars(&n))).collect();
        // Items: a non-zero value in an item column, or the item of a mined block
        // (the game's order comes from an identity-hash set, not reproducible: we
        // keep the id order).
        let strip = |n: &str| n.strip_prefix("minecraft:").unwrap_or(n).to_owned();
        let item_names: hashbrown::HashSet<String> = it.names.iter().map(|n| strip(n)).collect();
        let block_names: hashbrown::HashSet<String> = it.data.blocks.iter().map(|b| strip(&b.name)).collect();
        let item_id = |name: &str| it.names.iter().position(|n| strip(n) == name).map(|i| i as i32);
        let row_of_block = |b: i32| -> Option<i32> {
            let name = strip(&it.data.blocks.get(usize::try_from(b).ok()?)?.name);
            item_id(&item_of_block(&name, |n| item_names.contains(n))?)
        };
        let mut ids: Vec<i32> = Vec::new();
        for (kind, _, _) in &COLUMNS[1..] {
            ids.extend(s.ids(*kind));
        }
        ids.extend(s.ids(stats::MINED).filter_map(row_of_block));
        let air = it.names.iter().position(|n| n == "minecraft:air").map(|i| i as i32);
        ids.retain(|&i| Some(i) != air);
        ids.sort_unstable();
        ids.dedup();
        self.items = ids
            .into_iter()
            .map(|item| {
                let name = strip(it.names.get(item as usize).map_or("", String::as_str));
                let block = block_of_item(&name, |b| block_names.contains(b))
                    .and_then(|b| it.data.blocks.iter().position(|x| strip(&x.name) == b))
                    .map(|b| b as i32);
                let stack = ItemStack { item, count: 1, added: Vec::new(), removed: Vec::new() };
                ItemRow { item, block, stack, cache: None }
            })
            .collect();
        let entities = it.data.registry("minecraft:entity_type").map(|r| r.entries.clone()).unwrap_or_default();
        self.mobs = entities
            .iter()
            .enumerate()
            .filter(|(i, _)| s.get(stats::KILLED, *i as i32) > 0 || s.get(stats::KILLED_BY, *i as i32) > 0)
            .map(|(i, n)| {
                let key = format!("entity.{}", n.replace(':', "."));
                (i as i32, tr(lang, &key, &[]))
            })
            .collect();
        self.active = [true, !self.items.is_empty(), !self.mobs.is_empty()];
        self.tab = 0;
        self.size = (-2, -2);
        // The game replaces the "loading" tab with the General tab going through the focus
        // of the tab bar: audible tab change, hence a click when the stats arrive.
        self.clicks += 1;
    }

    /// Switch to an active tab (click, Ctrl+Tab, Ctrl+digit): click sound if it changes.
    fn select_tab(&mut self, i: usize) {
        if self.loaded && self.active[i] && i != self.tab {
            // Each tab keeps its own list, hence its scroll.
            if let Some(l) = &self.panel.list {
                self.scrolls[self.tab] = l.scroll;
            }
            self.tab = i;
            self.size = (-2, -2);
            self.panel.list = None;
            self.clicks += 1;
        }
    }

    fn row_h(&self) -> i32 {
        [14, 22, 36][self.tab]
    }

    fn rows(&self) -> usize {
        match self.tab {
            0 => self.general.len(),
            1 if self.items.is_empty() => 0,
            1 => self.items.len() + 1,
            _ => self.mobs.len(),
        }
    }

    fn layout(&mut self, gui: &Gui) {
        let size = (gui.width as i32, gui.height as i32);
        if self.size == size {
            return;
        }
        let first = self.size.0 == -1;
        let scroll = self.panel.list.as_ref().map(|l| l.scroll).or(Some(self.scrolls[self.tab]));
        self.size = size;
        let (w, h) = size;
        let done = Widget::button(DONE, (w - 200) / 2, h - 26, 200, &tr(&gui.lang, "gui.done", &[]));
        let keyboard = self.panel.keyboard;
        self.panel.widgets = vec![done];
        if self.loaded {
            let rh = self.row_h();
            let mut l = List::new(0, HEADER, w, h - HEADER - FOOTER, ROW_WIDTH, rh);
            l.custom_rows = true;
            l.background = false;
            l.separators = self.tab == 1;
            l.entries = (0..self.rows()).map(|i| Entry { top: i as i32 * rh, h: rh, label: Vec::new() }).collect();
            l.content = self.rows() as i32 * rh + 4;
            if let Some(s) = scroll {
                l.set_scroll(s);
            }
            self.panel.list = Some(l);
        } else {
            self.panel.list = None;
        }
        if first {
            self.panel.initial_focus(keyboard);
        }
    }

    /// Tab bar (§ 1.10): x, width of a tab (same bar as world creation).
    fn tab_bar(w: i32) -> (i32, i32) {
        crate::create_world::tab_bar(w)
    }

    fn tab_at(&self, m: (f32, f32)) -> Option<usize> {
        let (x0, tw) = Self::tab_bar(self.size.0);
        if m.1 < 0.0 || m.1 >= HEADER as f32 {
            return None;
        }
        (0..3).find(|&i| {
            let x = (x0 + tw * i as i32) as f32;
            m.0 >= x && m.0 < x + tw as f32
        })
    }

    pub fn press(&mut self, gui: &Gui, m: (f32, f32), button: u8) -> Option<Action> {
        self.layout(gui);
        if button == 0
            && let Some(i) = self.tab_at(m)
        {
            self.select_tab(i);
            return None;
        }
        // Header of the Items list: sort.
        if button == 0
            && self.tab == 1
            && let Some(c) = self.header_button_at(m)
        {
            self.sort = match self.sort {
                Some(Sort { col, desc: true }) if col == c => Some(Sort { col, desc: false }),
                Some(Sort { col, desc: false }) if col == c => None,
                _ => Some(Sort { col: c, desc: true }),
            };
            self.clicks += 1;
            self.resort = true;
            return None;
        }
        let mut ev = Vec::new();
        self.panel.press(m, button, false, &mut ev);
        self.clicks += std::mem::take(&mut self.panel.clicks);
        ev.contains(&Event::Press(DONE)).then_some(Action::Back)
    }

    pub fn release(&mut self) {
        let mut ev = Vec::new();
        self.panel.release(&mut ev);
    }

    pub fn motion(&mut self, gui: &Gui, m: (f32, f32)) {
        self.layout(gui);
        let mut ev = Vec::new();
        self.panel.motion(m, &mut ev);
    }

    pub fn wheel(&mut self, gui: &Gui, m: (f32, f32), dy: f64) {
        self.layout(gui);
        let mut ev = Vec::new();
        self.panel.wheel(m, dy, &mut ev);
    }

    pub fn key(&mut self, gui: &Gui, k: winit::keyboard::KeyCode) -> Option<Action> {
        use winit::keyboard::KeyCode as K;
        self.layout(gui);
        // Tab bar first: Ctrl+digit (1 → first, 0 → tenth, clamped to the last),
        // Ctrl+Tab / Ctrl+Shift+Tab (next / previous active tab, wrapping).
        if self.mods.control_key() {
            const DIGITS: [K; 10] = [
                K::Digit0,
                K::Digit1,
                K::Digit2,
                K::Digit3,
                K::Digit4,
                K::Digit5,
                K::Digit6,
                K::Digit7,
                K::Digit8,
                K::Digit9,
            ];
            let target = if let Some(d) = DIGITS.iter().position(|&x| x == k) {
                Some(((d as i32 - 1).rem_euclid(10) as usize).min(2))
            } else if k == K::Tab {
                let dir = if self.mods.shift_key() { 2 } else { 1 };
                let mut i = (self.tab + dir) % 3;
                while !(self.loaded && self.active[i]) && i != self.tab {
                    i = (i + dir) % 3;
                }
                Some(i)
            } else {
                None
            };
            if let Some(i) = target {
                self.select_tab(i);
                return None;
            }
        }
        let mut ev = Vec::new();
        let used = self.panel.key(k, winit::keyboard::ModifiersState::empty(), &mut ev);
        self.clicks += std::mem::take(&mut self.panel.clicks);
        (ev.contains(&Event::Press(DONE)) || (!used && k == winit::keyboard::KeyCode::Escape)).then_some(Action::Back)
    }

    /// Stable sort by the chosen column, ties: item id in the same direction.
    /// "None" leaves the current order.
    fn apply_sort(&mut self, s: &Stats) {
        let Some(sort) = self.sort else { return };
        let kind = COLUMNS[sort.col].0;
        let key = |r: &ItemRow| -> (i32, i32) {
            let v = if sort.col == 0 { r.block.map_or(-1, |b| s.get(kind, b)) } else { s.get(kind, r.item) };
            (v, r.item)
        };
        self.items.sort_by(|a, b| {
            let o = key(a).cmp(&key(b));
            if sort.desc { o.reverse() } else { o }
        });
    }

    /// Header button under the mouse (Items list).
    fn header_button_at(&self, m: (f32, f32)) -> Option<usize> {
        let l = self.panel.list.as_ref()?;
        let (x, y, _, _) = l.entry_rect(0)?;
        if m.1 < l.y as f32 || m.1 >= (l.y + l.h) as f32 {
            return None;
        }
        let (xc, top) = (x + 2, y + 2);
        (0..6).find(|&c| {
            let bx = (xc + col_x(c) - 18) as f32;
            let by = (top + 1) as f32;
            m.0 >= bx && m.0 < bx + 18.0 && m.1 >= by && m.1 < by + 18.0
        })
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        gui: &mut Gui,
        q: &wgpu::Queue,
        it: &HotbarItems<'_>,
        s: &Stats,
        blur: i32,
        mouse: Option<(f32, f32)>,
        time: f64,
    ) {
        self.sync(gui, s, it);
        if std::mem::take(&mut self.resort) {
            self.apply_sort(s);
        }
        self.layout(gui);
        let f = |v: i32| v as f32;
        let (w, h) = (gui.width as i32, gui.height as i32);
        gui.next_layer();
        widgets::menu_background(gui, blur, true);
        gui.sprite_tiled("bg/tab_header_background", 0.0, 0.0, f(w), f(HEADER), 16.0, 16.0, 0.0, 0.0);
        gui.sprite_tiled("bg/footer_separator", 0.0, f(h - FOOTER), f(w), f(h - FOOTER + 2), 32.0, 2.0, 0.0, 0.0);
        gui.next_layer();
        self.draw_tabs(gui, q, mouse, time);
        if !self.loaded {
            // Loading: text and animated dots, centered above a margin of 30.
            let area = h - HEADER - FOOTER - 30;
            let top = HEADER + (area - 28) / 2;
            let msg = chars(&tr(&gui.lang, "multiplayer.downloadingStats", &[]));
            widgets::centered(gui, q, &msg, w / 2, top + 2, 0xffff_ffff);
            let frames = ["O o o", "o O o", "o o O", "o O o"];
            let k = (self.opened.elapsed().as_millis() / 300) as usize % 4;
            widgets::centered(gui, q, &chars(frames[k]), w / 2, top + 28 - 11, 0xff80_8080);
            self.panel.draw(gui, q, mouse, time);
            return;
        }
        if let Some(m) = gui.items.as_ref().map(|a| &a.models) {
            for r in &mut self.items {
                refresh(&mut r.cache, &r.stack, it, m, 0);
            }
        }
        let tab = self.tab;
        let general = &self.general;
        let items = &self.items;
        let mobs = &self.mobs;
        let sort = self.sort;
        let mut header_tip: Option<(usize, (i32, i32))> = None;
        let mut item_tip: Option<(usize, (i32, i32))> = None;
        let lang = gui.lang.clone();
        self.panel.draw_with(gui, q, mouse, time, &mut |gui, q, l| {
            for i in 0..l.entries.len() {
                let Some((x, y, _, eh)) = l.entry_rect(i) else { continue };
                if y + eh < l.y || y > l.y + l.h {
                    continue;
                }
                let (xc, top, xd) = (x + 2, y + 2, x + ROW_WIDTH - 2);
                let ym = top + (eh - 4) / 2;
                match tab {
                    0 => {
                        let (id, name) = &general[i];
                        let color = if i % 2 == 0 { 0xffff_ffff } else { GRAY };
                        gui.text(q, name, f(xc + 2), f(ym - 4), color, true, 0);
                        let v = chars(&stats::format(CUSTOM_STATS[*id].1, s.get(stats::CUSTOM, *id as i32)));
                        let vw = rc_text::width(gui, &v);
                        gui.text(q, &v, f(xd - 4 - vw), f(ym - 4), color, true, 0);
                    }
                    1 if i == 0 => {
                        for (c, (_, icon, _)) in COLUMNS.iter().enumerate() {
                            let (bx, by) = (xc + col_x(c) - 18, top + 1);
                            let over = mouse.is_some_and(|(mx, my)| {
                                mx >= f(bx) && mx < f(bx + 18) && my >= f(by) && my < f(by + 18) && my >= f(l.y)
                            });
                            let bg = if over { "container/slot" } else { "statistics/header" };
                            gui.sprite(bg, f(bx), f(by), 18.0, 18.0, 0xffff_ffff);
                            gui.sprite(icon, f(bx), f(by), 18.0, 18.0, 0xffff_ffff);
                            if over {
                                header_tip = Some((c, (bx, by)));
                            }
                            if let Some(so) = sort.filter(|so| so.col == c) {
                                let arrow = if so.desc { "statistics/sort_down" } else { "statistics/sort_up" };
                                gui.sprite(arrow, f(xc + col_x(c) - 36), f(top + 1), 18.0, 18.0, 0xffff_ffff);
                            }
                        }
                    }
                    1 => {
                        let r = &items[i - 1];
                        gui.sprite("container/slot", f(xc), f(top), 18.0, 18.0, 0xffff_ffff);
                        let color = if i % 2 == 0 { 0xffff_ffff } else { GRAY };
                        for (c, (kind, _, _)) in COLUMNS.iter().enumerate() {
                            let text = if c == 0 {
                                match r.block {
                                    Some(b) => stats::format(stats::Format::Int, s.get(*kind, b)),
                                    None => tr(&lang, "stats.none", &[]),
                                }
                            } else {
                                stats::format(stats::Format::Int, s.get(*kind, r.item))
                            };
                            let v = chars(&text);
                            let vw = rc_text::width(gui, &v);
                            gui.text(q, &v, f(xc + col_x(c) - vw), f(ym - 4), color, true, 0);
                        }
                        if mouse.is_some_and(|(mx, my)| {
                            mx >= f(xc) && mx < f(xc + 18) && my >= f(top) && my < f(top + 18) && my >= f(l.y)
                        }) {
                            item_tip = Some((i - 1, (xc + 18, top + 18)));
                        }
                    }
                    _ => {
                        let (id, name) = &mobs[i];
                        let (k, kb) = (s.get(stats::KILLED, *id), s.get(stats::KILLED_BY, *id));
                        gui.text(q, &chars(name), f(xc + 2), f(top + 1), 0xffff_ffff, true, 0);
                        let line2 = if k > 0 {
                            tr(&lang, "stat_type.minecraft.killed", &[k.to_string(), name.clone()])
                        } else {
                            tr(&lang, "stat_type.minecraft.killed.none", std::slice::from_ref(name))
                        };
                        let line3 = if kb > 0 {
                            tr(&lang, "stat_type.minecraft.killed_by", &[name.clone(), kb.to_string()])
                        } else {
                            tr(&lang, "stat_type.minecraft.killed_by.none", std::slice::from_ref(name))
                        };
                        let c2 = if k > 0 { GRAY } else { 0xff80_8080 };
                        let c3 = if kb > 0 { GRAY } else { 0xff80_8080 };
                        gui.text(q, &chars(&line2), f(xc + 12), f(top + 10), c2, true, 0);
                        gui.text(q, &chars(&line3), f(xc + 12), f(top + 19), c3, true, 0);
                    }
                }
            }
            // Item icons (premultiplied stratum, always clipped to the list).
            if tab == 1 {
                gui.stratum(Blend::Premultiplied);
                for i in 1..l.entries.len() {
                    let Some((x, y, _, eh)) = l.entry_rect(i) else { continue };
                    if y + eh < l.y || y > l.y + l.h {
                        continue;
                    }
                    if let Some(c) = &items[i - 1].cache
                        && !c.layers.is_empty()
                    {
                        gui.item(c.key, &c.layers, f(x + 3), f(y + 3), 0.0);
                    }
                }
                gui.stratum(Blend::Normal);
            }
        });
        if let Some((c, (bx, by))) = header_tip {
            let tip = chars(&tr(&gui.lang, &format!("stat_type.minecraft.{}", COLUMNS[c].2), &[]));
            widgets::draw_tooltip(gui, q, &[tip], (bx, by, 18, 18), mouse);
        }
        if let Some((k, (ax, ay))) = item_tip {
            let st = self.items[k].stack.clone();
            if self.tooltip.as_ref().is_none_or(|t| t.0 != st) {
                let lines = crate::screens::tooltip_lines(&st, it, &[], gui, crate::screens::advanced());
                let tw = lines.iter().map(|l| rc_text::width(gui, l)).max().unwrap_or(0);
                self.tooltip = Some((st, lines, tw));
            }
            if let Some((_, lines, tw)) = &self.tooltip {
                // Anchor at the bottom-right corner of the slot, usual offset (+12, −12) included.
                crate::screens::draw_tooltip(gui, q, lines, *tw, ax, ay);
            }
        }
    }

    fn draw_tabs(&mut self, gui: &mut Gui, q: &wgpu::Queue, mouse: Option<(f32, f32)>, time: f64) {
        let f = |v: i32| v as f32;
        let w = gui.width as i32;
        let (x0, tw) = Self::tab_bar(w);
        // Header separator outside the tabs.
        gui.sprite_tiled("bg/header_separator", 0.0, 22.0, f(x0), 24.0, 32.0, 2.0, 0.0, 0.0);
        gui.sprite_tiled("bg/header_separator", f(x0 + 3 * tw), 22.0, f(w), 24.0, 32.0, 2.0, 0.0, 0.0);
        let keys = ["stat.generalButton", "stat.itemsButton", "stat.mobsButton"];
        let hovered = mouse.and_then(|m| self.tab_at(m));
        for (i, key) in keys.iter().enumerate() {
            let x = x0 + tw * i as i32;
            let selected = i == self.tab;
            let active = self.loaded && self.active[i] || i == 0;
            let hl = hovered == Some(i) && active;
            let s = match (selected, hl) {
                (true, true) => "widget/tab_selected_highlighted",
                (true, false) => "widget/tab_selected",
                (false, true) => "widget/tab_highlighted",
                (false, false) => "widget/tab",
            };
            gui.sprite(s, f(x), 0.0, f(tw), f(HEADER), 0xffff_ffff);
            let label = chars(&tr(&gui.lang, key, &[]));
            let color = if active { 0xffff_ffff } else { 0xffa0_a0a0 };
            if selected {
                // Background of the chosen tab: always that of the out-of-world menus, like the game.
                gui.sprite_tiled(
                    "bg/menu_background",
                    f(x + 2),
                    2.0,
                    f(x + tw - 2),
                    f(HEADER),
                    32.0,
                    32.0,
                    0.0,
                    0.0,
                );
                let lw = rc_text::width(gui, &label).min(tw - 4);
                let ux = x + (tw - lw) / 2;
                gui.fill(f(ux), f(HEADER - 2), f(ux + lw), f(HEADER - 1), color);
            }
            let top = if selected { 0 } else { 3 };
            widgets::scrolling_text(gui, q, &label, x + 1, top, x + tw - 1, HEADER, color, time);
        }
        if let Some(i) = hovered
            && self.loaded
            && !self.active[i]
        {
            let tip = chars(&tr(&gui.lang, "gui.stats.none_found", &[]));
            let x = x0 + tw * i as i32;
            widgets::draw_tooltip(gui, q, &[tip], (x, 0, tw, HEADER), mouse);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_rows_map_to_the_item_that_places_them() {
        let items = [
            "wheat",
            "wheat_seeds",
            "carrot",
            "torch",
            "oak_sign",
            "zombie_head",
            "tube_coral_fan",
            "stone",
            "redstone",
        ];
        let blocks = ["wheat", "carrots", "torch", "stone", "redstone_wire"];
        let is_item = |n: &str| items.contains(&n);
        let is_block = |n: &str| blocks.contains(&n);
        // Mined block → item of the row.
        assert_eq!(item_of_block("carrots", is_item).as_deref(), Some("carrot"));
        assert_eq!(item_of_block("wall_torch", is_item).as_deref(), Some("torch"));
        assert_eq!(item_of_block("oak_wall_sign", is_item).as_deref(), Some("oak_sign"));
        assert_eq!(item_of_block("zombie_wall_head", is_item).as_deref(), Some("zombie_head"));
        assert_eq!(item_of_block("tube_coral_wall_fan", is_item).as_deref(), Some("tube_coral_fan"));
        assert_eq!(item_of_block("attached_melon_stem", is_item), None);
        assert_eq!(item_of_block("wheat", is_item).as_deref(), Some("wheat_seeds"));
        assert_eq!(block_of_item("wheat", is_block), None);
        assert_eq!(block_of_item("wheat_seeds", is_block), Some("wheat"));
        // Item → block of the "mined" column.
        assert_eq!(block_of_item("redstone", is_block), Some("redstone_wire"));
        assert_eq!(block_of_item("carrot", is_block), Some("carrots"));
        assert_eq!(block_of_item("stone", is_block), Some("stone"));
        assert_eq!(block_of_item("oak_sign", is_block), None);
        // Mined powder snow: row of the powder snow bucket.
        let is_item = |n: &str| n == "powder_snow_bucket";
        assert_eq!(item_of_block("powder_snow", is_item).as_deref(), Some("powder_snow_bucket"));
        assert_eq!(block_of_item("powder_snow_bucket", |n| n == "powder_snow"), Some("powder_snow"));
    }

    #[test]
    fn tab_bar_centres_the_reserved_width() {
        // 330 wide: 302 reserved, tabs of 100, origin (330 - 302) / 2 = 14 (not 16).
        assert_eq!(StatsScreen::tab_bar(330), (14, 100));
        assert_eq!(StatsScreen::tab_bar(854), (242, 124));
    }
}
