//! Inventory and container screens (`docs/specs/inventories.md` § 2–5): mouse
//! and keyboard input → clicks, prediction and sending of `container_click`, rendering (background, slots, items,
//! highlight, carried item, drag preview, tooltip).

use std::time::Instant;

use rc_proto_v777::container::ComponentHasher;
use rc_proto_v777::item::ItemStack;
use rc_text::{Component, StyledChar};

use crate::gui::{Blend, Gui};
use crate::hud::HotbarItems;
use crate::inventory::PlayerInventory;
use crate::menu::{self, Kind, Menu, Rules};

/// Protocol click: slot, button, mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Click {
    pub slot: i16,
    pub button: i8,
    pub mode: i32,
}

struct Drag {
    button: u8,
    kind: u8,
    slots: Vec<usize>,
}

/// Memoised rendering of a stack (redone when the stack or the tick change).
pub(crate) struct Cached {
    pub(crate) stack: ItemStack,
    pub(crate) tick: i64,
    /// Place of the stack and chosen bundle item at this rendering.
    pub(crate) hold: (crate::stack::Hold, i32),
    pub(crate) layers: Vec<rc_assets::item::Layer>,
    pub(crate) key: u64,
    pub(crate) bar: Option<(i32, u32)>,
}

#[derive(Default)]
pub struct Screens {
    /// A screen (inventory or container) is open.
    pub open: bool,
    pub hasher: ComponentHasher,
    drag: Option<Drag>,
    /// Slots and type of the last drag sent (prediction of phase 2).
    pub drag_sent: Vec<usize>,
    skip_release: bool,
    last_press: Option<(Instant, usize)>,
    double: bool,
    /// Stack of the last slot moved by Shift-click (filter of Shift + double click).
    last_quick_moved: Option<ItemStack>,
    /// Shift + double click in progress: (part aimed at, filter stack, next slot to examine).
    sweep: Option<(u8, ItemStack, usize)>,
    cache: Vec<Option<Cached>>,
    carried_cache: Option<Cached>,
    tooltip: Option<(ItemStack, Vec<Vec<StyledChar>>, i32)>,
    titles: Option<(i32, Vec<StyledChar>, Vec<StyledChar>)>,
    /// Advanced tooltips (F3 + H).
    advanced_tooltips: bool,
    /// Recipe book of the screen: (visible, narrow screen) (`creative-inventory.md` § 11.1).
    pub book: (bool, bool),
    /// Entity of the local player, drawn in the inventory (interface model).
    pub player_model: Option<i32>,
}

/// Item rules drawn from the version data (fuels and smeltings approximated,
/// `inventories.md` § 6.2: the server corrects).
pub struct ItemRules<'a> {
    pub infos: &'a [rc_data::ItemInfo],
    pub names: &'a [String],
    pub ids: &'a crate::stack::ComponentIds,
    pub data: &'a rc_data::VersionData,
}

impl ItemRules<'_> {
    fn name(&self, s: &ItemStack) -> &str {
        let n = usize::try_from(s.item).ok().and_then(|i| self.names.get(i)).map_or("", String::as_str);
        n.strip_prefix("minecraft:").unwrap_or(n)
    }
}

impl Rules for ItemRules<'_> {
    fn max_stack(&self, s: &ItemStack) -> i32 {
        let info = usize::try_from(s.item).ok().and_then(|i| self.infos.get(i));
        let view = crate::stack::StackRef { stack: s, info, ids: self.ids, data: self.data, hold: crate::stack::Hold::NONE };
        rc_assets::item::StackView::max_stack_size(&view).clamp(1, 99)
    }
    fn equip_slot(&self, s: &ItemStack) -> u8 {
        usize::try_from(s.item).ok().and_then(|i| self.infos.get(i)).map_or(0, |i| i.equip_slot)
    }
    fn is_fuel(&self, s: &ItemStack) -> bool {
        let n = self.name(s);
        matches!(n, "coal" | "charcoal" | "coal_block" | "lava_bucket" | "blaze_rod" | "stick" | "bucket" | "bamboo")
            || ["_log", "_planks", "_wood", "_stem", "_sapling", "_slab", "_fence"].iter().any(|e| n.ends_with(e))
    }
    fn smeltable(&self, s: &ItemStack) -> bool {
        let n = self.name(s);
        n.starts_with("raw_")
            || n.ends_with("_ore")
            || n.ends_with("_log")
            || matches!(
                n,
                "beef"
                    | "porkchop"
                    | "chicken"
                    | "mutton"
                    | "rabbit"
                    | "cod"
                    | "salmon"
                    | "potato"
                    | "kelp"
                    | "sand"
                    | "red_sand"
                    | "cobblestone"
                    | "stone"
                    | "clay_ball"
                    | "clay"
                    | "netherrack"
                    | "cactus"
                    | "wet_sponge"
                    | "ancient_debris"
            )
    }
    fn is_potion(&self, s: &ItemStack) -> bool {
        matches!(self.name(s), "potion" | "splash_potion" | "lingering_potion" | "glass_bottle")
    }
    fn brew_fuel(&self, s: &ItemStack) -> bool {
        self.name(s) == "blaze_powder"
    }
    fn item_name(&self, s: &ItemStack) -> &str {
        self.name(s)
    }
    fn max_damage(&self, s: &ItemStack) -> u32 {
        usize::try_from(s.item).ok().and_then(|i| self.infos.get(i)).map_or(0, |i| i.max_damage)
    }
    fn bundles(&self) -> Option<crate::bundle::Ctx<'_>> {
        Some(crate::bundle::Ctx { ids: self.ids, data: self.data })
    }
    fn enchantments_component(&self) -> Option<u16> {
        self.ids.of("minecraft:enchantments")
    }
    fn map_id_component(&self) -> Option<u16> {
        self.ids.map_id
    }
    fn brew_ingredient(&self, s: &ItemStack) -> bool {
        matches!(
            self.name(s),
            "nether_wart"
                | "redstone"
                | "glowstone_dust"
                | "fermented_spider_eye"
                | "gunpowder"
                | "dragon_breath"
                | "sugar"
                | "rabbit_foot"
                | "glistering_melon_slice"
                | "spider_eye"
                | "pufferfish"
                | "magma_cream"
                | "golden_carrot"
                | "blaze_powder"
                | "ghast_tear"
                | "turtle_helmet"
                | "phantom_membrane"
                | "breeze_rod"
                | "slime_block"
                | "cobweb"
                | "stone"
        )
    }
}

/// Current menu: type, slots, window, state number.
pub(crate) fn current(inv: &PlayerInventory) -> (Kind, &[Option<ItemStack>], i32, i32) {
    match &inv.container {
        Some(c) => (c.kind, &c.slots, c.window, c.state),
        None => (Kind::Player, &inv.menu, 0, inv.state),
    }
}

/// Corner of the screen image and its size (§ 4.1); shifted to the right if the recipe book
/// is visible on a wide enough screen.
pub(crate) fn origin(gui: &Gui, kind: Kind, n: usize, book: (bool, bool)) -> (i32, i32, i32, i32) {
    let (w, h) = menu::image_size(kind, n);
    let gx = if book.0 && !book.1 { 177 + (gui.width as i32 - w - 200) / 2 } else { (gui.width as i32 - w) / 2 };
    (gx, (gui.height as i32 - h) / 2, w, h)
}

impl Screens {
    pub fn new(hasher: ComponentHasher) -> Screens {
        Screens { hasher, ..Default::default() }
    }

    /// Hovered slot alone (bundle wheel).
    pub(crate) fn hovered_slot(&self, gui: &Gui, inv: &PlayerInventory, mouse: (f32, f32)) -> Option<usize> {
        if !self.open {
            return None;
        }
        self.hovered(gui, inv, mouse).0
    }

    /// Hovered slot (§ 4.2) and cursor outside the image.
    fn hovered(&self, gui: &Gui, inv: &PlayerInventory, mouse: (f32, f32)) -> (Option<usize>, bool) {
        let (kind, slots, _, _) = current(inv);
        if self.book.0 && self.book.1 {
            return (None, false);
        }
        let (gx, gy, w, h) = origin(gui, kind, slots.len(), self.book);
        let (mx, my) = (mouse.0.floor() as i32 - gx, mouse.1.floor() as i32 - gy);
        let in_book = self.book.0 && mx > -147 && mx < 0 && my > 0 && my < h;
        let outside = (mx < 0 || my < 0 || mx >= w || my >= h) && !in_book;
        let hit = (0..slots.len()).find(|&i| {
            let d = menu::slot_def(kind, i, slots.len());
            mx >= d.x - 1 && mx < d.x + 17 && my >= d.y - 1 && my < d.y + 17
        });
        (hit, outside)
    }

    /// Local opening (player inventory or container screen received).
    pub fn show(&mut self) {
        self.open = true;
        self.drag = None;
        self.skip_release = false;
        self.double = false;
        self.titles = None;
    }

    pub fn hide(&mut self) {
        self.open = false;
        self.drag = None;
        self.tooltip = None;
    }

    /// Advanced tooltips (option `advancedItemTooltips`): tooltip reformatted.
    pub fn set_advanced(&mut self, on: bool) {
        ADVANCED.store(on, std::sync::atomic::Ordering::Relaxed);
        if self.advanced_tooltips != on {
            self.advanced_tooltips = on;
            self.tooltip = None;
        }
    }

    /// Button press (0 left, 1 right, 2 middle).
    pub fn press(
        &mut self,
        gui: &Gui,
        inv: &PlayerInventory,
        button: u8,
        mouse: (f32, f32),
        shift: bool,
        creative: bool,
    ) -> Vec<Click> {
        let (slot, outside) = self.hovered(gui, inv, mouse);
        if button == 0
            && let Some(s) = slot
        {
            self.double = self.last_press.is_some_and(|(t, l)| l == s && t.elapsed().as_millis() < 250);
            self.last_press = Some((Instant::now(), s));
        }
        if self.drag.is_some() {
            return Vec::new();
        }
        if inv.carried.is_some() {
            if (button < 2 || creative) && (slot.is_some() || outside) {
                self.drag = Some(Drag { button, kind: button.min(2), slots: Vec::new() });
            }
            return Vec::new();
        }
        // Shift-click with an empty hand: stack of the slot retained (empty if the slot is).
        if shift
            && button < 2
            && let Some(s) = slot
        {
            self.last_quick_moved = current(inv).1.get(s).cloned().flatten();
        }
        let click = match (slot, button) {
            (Some(s), 2) if creative => Some(Click { slot: s as i16, button: 2, mode: 3 }),
            (Some(s), 0 | 1) => Some(Click { slot: s as i16, button: button as i8, mode: i32::from(shift) }),
            (None, 0 | 1) if outside => Some(Click { slot: -999, button: button as i8, mode: 4 }),
            _ => None,
        };
        self.skip_release = click.is_some();
        click.into_iter().collect()
    }

    /// Cursor moved: slots added to the drag (§ 2.7).
    pub fn motion(&mut self, gui: &Gui, inv: &PlayerInventory, mouse: (f32, f32), rules: &dyn Rules) {
        let Some(drag) = &self.drag else { return };
        let (Some(s), _) = self.hovered(gui, inv, mouse) else { return };
        if drag.slots.contains(&s) {
            return;
        }
        let (kind, taken, dk) = (current(inv).0, drag.slots.len(), drag.kind);
        let mut carried = inv.carried.clone();
        let mut slots = current(inv).1.to_vec();
        let m = Menu { kind, slots: &mut slots, carried: &mut carried, rules, creative: true };
        if m.drag_accepts(s, dk, taken, true)
            && let Some(d) = &mut self.drag
        {
            d.slots.push(s);
        }
    }

    /// Button release.
    pub fn release(
        &mut self,
        gui: &Gui,
        inv: &PlayerInventory,
        button: u8,
        mouse: (f32, f32),
        shift: bool,
    ) -> Vec<Click> {
        let (slot, outside) = self.hovered(gui, inv, mouse);
        let (kind, slots) = (current(inv).0, current(inv).1);
        if self.double
            && button == 0
            && let Some(s) = slot
            && pick_all_allowed(kind, s, slots.len())
        {
            self.double = false;
            self.last_press = None;
            self.drag = None;
            self.skip_release = false;
            if !shift {
                return vec![Click { slot: s as i16, button: 0, mode: 6 }];
            }
            // Shift + double click: Shift-click on each slot of the same part (container or
            // inventory) that carries the same stack as the last one moved by Shift-click.
            let Some(last) = &self.last_quick_moved else { return Vec::new() };
            // Each slot is re-evaluated just before its click (`next_sweep`), because the
            // previous clicks are predicted at once and change the contents of the slots.
            self.sweep = Some((slot_group(kind, s, slots.len()), last.clone(), 0));
            return Vec::new();
        }
        if button == 0 {
            self.double = false;
        }
        if let Some(drag) = self.drag.take() {
            if drag.button != button {
                return Vec::new();
            }
            if drag.slots.is_empty() {
                let target = match slot {
                    Some(s) => s as i16,
                    None if outside => -999,
                    None => return Vec::new(),
                };
                if button == 2 {
                    return vec![Click { slot: target, button: 2, mode: 3 }];
                }
                let mode = i32::from(shift && target >= 0);
                if mode == 1 {
                    self.last_quick_moved = slot.and_then(|s| slots.get(s).cloned().flatten());
                }
                return vec![Click { slot: target, button: button as i8, mode }];
            }
            let k = (drag.kind as i8) << 2;
            let mut out = vec![Click { slot: -999, button: k, mode: 5 }];
            out.extend(drag.slots.iter().map(|&s| Click { slot: s as i16, button: k | 1, mode: 5 }));
            out.push(Click { slot: -999, button: k | 2, mode: 5 });
            self.drag_sent = drag.slots;
            return out;
        }
        if std::mem::take(&mut self.skip_release) {
            return Vec::new();
        }
        Vec::new()
    }

    /// Next Shift-click of the Shift + double click: first slot, starting from the last one examined,
    /// that carries the filter stack in the same part, according to the CURRENT contents of the slots.
    pub fn next_sweep(&mut self, inv: &PlayerInventory) -> Option<Click> {
        let (kind, slots) = (current(inv).0, current(inv).1);
        let (group, last, from) = self.sweep.as_mut()?;
        let found = (*from..slots.len()).find(|&t| {
            slots[t].as_ref().is_some_and(|st| menu::same(st, last))
                && may_pickup(kind, t)
                && slot_group(kind, t, slots.len()) == *group
        });
        match found {
            Some(t) => {
                *from = t + 1;
                Some(Click { slot: t as i16, button: 0, mode: 1 })
            }
            None => {
                self.sweep = None;
                None
            }
        }
    }

    /// Key on hover: 1-9 (`digit` 0..8), F (40) swap; Q (Ctrl: whole stack) drops.
    pub fn key(
        &self,
        gui: &Gui,
        inv: &PlayerInventory,
        mouse: (f32, f32),
        digit: Option<i8>,
        drop: Option<bool>,
    ) -> Option<Click> {
        if self.drag.is_some() {
            return None;
        }
        let (Some(s), _) = self.hovered(gui, inv, mouse) else { return None };
        if let Some(d) = digit {
            return inv.carried.is_none().then_some(Click { slot: s as i16, button: d, mode: 2 });
        }
        let all = drop?;
        (inv.carried.is_none() && current(inv).1.get(s)?.is_some()).then_some(Click {
            slot: s as i16,
            button: i8::from(all),
            mode: 4,
        })
    }

    /// Local prediction then sending of the click (§ 2, § 1.6).
    pub fn perform(
        &mut self,
        inv: &mut PlayerInventory,
        rules: &dyn Rules,
        creative: bool,
        link: &rc_client::link::Link,
        c: Click,
    ) {
        let start = inv.container.as_ref().map(|c| c.player_start());
        let (kind, window, state) = {
            let (k, _, w, s) = current(inv);
            (k, w, s)
        };
        let carried = &mut inv.carried;
        let slots: &mut Vec<Option<ItemStack>> = match &mut inv.container {
            Some(ct) => &mut ct.slots,
            None => &mut inv.menu,
        };
        // Bundle moved quickly or swapped: chosen item forgotten, announced before the click.
        if let Some(i) = crate::bundle::unselect_before_click(c.mode, c.slot, slots, |st| {
            crate::bundle::is_bundle_name(rules.item_name(st))
        }) {
            if crate::bundle::selected_slot() == Some(i) {
                crate::bundle::set_selected(i, -1);
            }
            let _ = link.send_bundle_item_selected(i as i32, -1);
        }
        let before = slots.clone();
        let mut m = Menu { kind, slots, carried, rules, creative };
        if c.mode == 5 {
            if c.button & 3 == 2 {
                let cases = std::mem::take(&mut self.drag_sent);
                m.drag_end(&cases, (c.button >> 2) as u8);
            }
        } else {
            let mut hotbar = |b: i8| match (start, b) {
                (None, 0..=8) => Some(36 + b as usize),
                (None, 40) => Some(45),
                (Some(p), 0..=8) => Some(p + 27 + b as usize),
                _ => None,
            };
            m.click(c.slot, c.button, c.mode, &mut hotbar);
        }
        let changed: Vec<_> = menu::changed(&before, m.slots)
            .into_iter()
            .map(|i| (i as i16, m.slots[i].as_ref().map(|s| self.hasher.stack(s))))
            .collect();
        let held = m.carried.as_ref().map(|s| self.hasher.stack(s));
        if rc_data::env_flag!("RUSTCRAFT_INVLOG") {
            eprintln!("click {c:?} window {window} state {state}: {changed:?}, held {held:?}");
        }
        let _ = link.send_container_click(window, state, c.slot, c.button, c.mode, &changed, held.as_ref());
        if start.is_some() {
            inv.sync_from_container();
        }
    }

    /// Drag in progress (for the preview).
    fn drag_preview(&self) -> Option<(&[usize], u8)> {
        self.drag.as_ref().filter(|d| d.slots.len() > 1).map(|d| (&d.slots[..], d.kind))
    }

    /// Drawing of the open screen (§ 4).
    pub fn draw(
        &mut self,
        gui: &mut Gui,
        queue: &wgpu::Queue,
        inv: &PlayerInventory,
        it: &HotbarItems<'_>,
        mouse: Option<(f32, f32)>,
    ) {
        if !self.open {
            return;
        }
        let (kind, slots, window, _) = current(inv);
        if crate::special::hides_container(kind) {
            return;
        }
        let n = slots.len();
        let (gx, gy, w, h) = origin(gui, kind, n, self.book);
        let f = |v: i32| v as f32;
        let (hover, _) = mouse.map_or((None, true), |m| self.hovered(gui, inv, m));
        let rules = ItemRules { infos: it.infos, names: it.names, ids: it.ids, data: it.data };
        // Layer 1: veil, background, progress elements.
        gui.next_layer();
        crate::widgets::ingame_ui_background(gui);
        match kind {
            Kind::Chest(_) | Kind::Generic => {
                let rows = match kind {
                    Kind::Chest(r) => i32::from(r),
                    _ => menu::generic_rows(n),
                };
                let name = "container_bg/generic_54";
                gui.sprite_region(name, 256.0, 256.0, 0.0, 0.0, f(gx), f(gy), 176.0, f(18 * rows + 17));
                gui.sprite_region(name, 256.0, 256.0, 0.0, 126.0, f(gx), f(gy + 18 * rows + 17), 176.0, 96.0);
                // Generic screen: absent slots cleared (background colour).
                let c = n.saturating_sub(36) as i32;
                for k in c..rows * 9 {
                    let (x, y) = (gx + 7 + 18 * (k % 9), gy + 17 + 18 * (k / 9));
                    gui.fill(f(x), f(y), f(x + 18), f(y + 18), 0xffc6_c6c6);
                }
            }
            // Special screens: background, titles and own elements (`special.rs`).
            Kind::Special(_) => {
                crate::special::draw_back(gui, queue, inv, it, (gx, gy), mouse, &self.hasher.enchantments)
            }
            _ => {
                let name = match kind {
                    Kind::Player => "container_bg/inventory",
                    Kind::Dispenser => "container_bg/dispenser",
                    Kind::Hopper => "container_bg/hopper",
                    Kind::Crafting => "container_bg/crafting_table",
                    Kind::Furnace(1) => "container_bg/blast_furnace",
                    Kind::Furnace(2) => "container_bg/smoker",
                    Kind::Furnace(_) => "container_bg/furnace",
                    _ => "container_bg/brewing_stand",
                };
                gui.sprite_region(name, 256.0, 256.0, 0.0, 0.0, f(gx), f(gy), f(w), f(h));
                // Local player in the black frame, which follows the mouse (size 30, offset 1/16).
                if matches!(kind, Kind::Player)
                    && let Some(id) = self.player_model
                {
                    let rect = [f(gx + 26), f(gy + 8), f(gx + 75), f(gy + 78)];
                    let mouse = mouse.unwrap_or(((rect[0] + rect[2]) / 2.0, (rect[1] + rect[3]) / 2.0));
                    let req = crate::special::ModelReq::Entity { id, rect, size: 30.0, offset: 0.0625, mouse };
                    crate::special::request_model(gui, req);
                }
            }
        }
        if let Some(c) = &inv.container {
            let p = &c.props;
            match kind {
                Kind::Furnace(v) => {
                    let v = v.min(2) as usize;
                    if p[0] > 0 {
                        let d = if p[1] == 0 { 200 } else { p[1] };
                        let q = (p[0] as f32 / d as f32).clamp(0.0, 1.0);
                        let k = (13.0 * q).ceil() as i32 + 1;
                        let s = [
                            "container/furnace/lit_progress",
                            "container/blast_furnace/lit_progress",
                            "container/smoker/lit_progress",
                        ][v];
                        gui.sprite_region(s, 14.0, 14.0, 0.0, f(14 - k), f(gx + 56), f(gy + 36 + 14 - k), 14.0, f(k));
                    }
                    if p[2] != 0 && p[3] != 0 {
                        let q = (p[2] as f32 / p[3] as f32).clamp(0.0, 1.0);
                        let l = (24.0 * q).ceil();
                        let s = [
                            "container/furnace/burn_progress",
                            "container/blast_furnace/burn_progress",
                            "container/smoker/burn_progress",
                        ][v];
                        gui.sprite_region(s, 24.0, 16.0, 0.0, 0.0, f(gx + 79), f(gy + 34), l, 16.0);
                    }
                }
                Kind::Brewing => {
                    if p[3] > 0 {
                        let l = ((18.0 * p[1] as f32 / p[3] as f32).ceil() as i32).clamp(0, 18);
                        if l > 0 {
                            let s = "container/brewing_stand/fuel_length";
                            gui.sprite_region(s, 18.0, 4.0, 0.0, 0.0, f(gx + 60), f(gy + 44), f(l), 4.0);
                        }
                    }
                    if p[0] > 0 && p[2] > 0 {
                        let l = (28.0 * (1.0 - p[0] as f32 / p[2] as f32)).floor() as i32;
                        if l > 0 {
                            let s = "container/brewing_stand/brew_progress";
                            gui.sprite_region(s, 9.0, 28.0, 0.0, 0.0, f(gx + 97), f(gy + 16), 9.0, f(l));
                        }
                        let l = [29, 24, 20, 16, 11, 6, 0][((p[0] / 2) % 7) as usize];
                        if l > 0 {
                            let s = "container/brewing_stand/bubbles";
                            gui.sprite_region(
                                s,
                                12.0,
                                29.0,
                                0.0,
                                f(29 - l),
                                f(gx + 63),
                                f(gy + 14 + 29 - l),
                                12.0,
                                f(l),
                            );
                        }
                    }
                }
                _ => {}
            }
        }
        // Titles (§ 4.3), redone only when the window changes.
        if self.titles.as_ref().is_none_or(|t| t.0 != window) {
            let title = match &inv.container {
                Some(c) => c.title.clone().unwrap_or_else(|| Component::text("")),
                None => Component::translate("container.crafting", vec![]),
            };
            let chars = |c: &Component| rc_text::styled_chars(&rc_text::flatten(c, &gui.lang));
            let label = chars(&Component::translate("container.inventory", vec![]));
            self.titles = Some((window, chars(&title), label));
        }
        if let Some((_, title, label)) = &self.titles
            && !matches!(kind, Kind::Special(_))
        {
            let tw = rc_text::width(gui, title);
            let tx = match kind {
                Kind::Player => 97,
                Kind::Crafting => 29,
                Kind::Dispenser | Kind::Furnace(_) | Kind::Brewing => (176 - tw) / 2,
                _ => 8,
            };
            gui.text(queue, title, f(gx + tx), f(gy + 6), 0xff40_4040, false, 0);
            if kind != Kind::Player {
                gui.text(queue, label, f(gx + 8), f(gy + h - 94), 0xff40_4040, false, 0);
            }
        }
        // Empty slots, back highlight, drag veils.
        let preview = self.drag_preview().map(|(s, k)| (s.to_vec(), k));
        if let Some(i) = hover {
            let d = menu::slot_def(kind, i, n);
            gui.sprite("container/slot_highlight_back", f(gx + d.x - 4), f(gy + d.y - 4), 24.0, 24.0, 0xffff_ffff);
        }
        for (i, s) in slots.iter().enumerate() {
            let d = menu::slot_def(kind, i, n);
            if s.is_none()
                && let Some(sp) = d.empty_sprite
            {
                gui.sprite(sp, f(gx + d.x), f(gy + d.y), 16.0, 16.0, 0xffff_ffff);
            }
            if preview.as_ref().is_some_and(|(p, _)| p.contains(&i)) {
                gui.fill(f(gx + d.x), f(gy + d.y), f(gx + d.x + 16), f(gy + d.y + 16), 0x80ff_ffff);
            }
        }
        // Items: up-to-date layers first (the atlas is borrowed), then drawing.
        self.cache.resize_with(n, || None);
        let mut drag_counts: Vec<(usize, i32, bool)> = Vec::new();
        let mut remaining = inv.carried.as_ref().map_or(0, |c| c.count);
        if let Some((cases, k)) = &preview {
            let mut carried = inv.carried.clone();
            let mut tmp = slots.to_vec();
            let m = Menu { kind, slots: &mut tmp, carried: &mut carried, rules: &rules, creative: true };
            for &i in cases {
                if let Some((cnt, capped)) = m.drag_share(i, *k, cases.len()) {
                    let before = slots[i].as_ref().map_or(0, |c| c.count);
                    remaining -= cnt - before;
                    drag_counts.push((i, cnt, capped));
                }
            }
            if *k == 2 {
                remaining = inv.carried.as_ref().map_or(0, |c| rules.max_stack(c));
            }
        }
        if let Some(models) = gui.items.as_ref().map(|a| &a.models) {
            for (i, s) in slots.iter().enumerate() {
                let shown = if drag_counts.iter().any(|d| d.0 == i) { inv.carried.as_ref() } else { s.as_ref() };
                match shown {
                    Some(st) => {
                        let d = menu::slot_def(kind, i, n);
                        refresh_held(&mut self.cache[i], st, it, models, d.x + d.y * w, crate::stack::menu_hold(inv, i));
                    }
                    None => self.cache[i] = None,
                }
            }
            match &inv.carried {
                Some(st) => refresh_held(&mut self.carried_cache, st, it, models, 0, crate::stack::Hold::CARRIED),
                None => self.carried_cache = None,
            }
        }
        if crate::tooltips::log_enabled() {
            for st in slots.iter().flatten() {
                let lines = tooltip_lines(st, it, &self.hasher.enchantments, gui, self.advanced_tooltips);
                let h = {
                    use std::hash::{Hash, Hasher};
                    let mut h = std::collections::hash_map::DefaultHasher::new();
                    format!("{:?}", (st.item, &st.added)).hash(&mut h);
                    h.finish()
                };
                crate::tooltips::log_once(&format!("{}#{h:x}", st.item), &lines);
            }
        }
        gui.stratum(Blend::Premultiplied);
        for i in 0..n {
            if let Some(c) = &self.cache[i]
                && !c.layers.is_empty()
            {
                let d = menu::slot_def(kind, i, n);
                gui.item(c.key, &c.layers, f(gx + d.x), f(gy + d.y), 0.0);
            }
        }
        gui.stratum(Blend::Normal);
        for i in 0..n {
            let Some(c) = &self.cache[i] else { continue };
            let d = menu::slot_def(kind, i, n);
            let count = drag_counts.iter().find(|x| x.0 == i).map(|x| (x.1, x.2));
            decorate(gui, queue, c, f(gx + d.x), f(gy + d.y), count.unwrap_or((c.stack.count, false)));
        }
        // Front highlight.
        gui.next_layer();
        if let Some(i) = hover {
            let d = menu::slot_def(kind, i, n);
            gui.sprite("container/slot_highlight_front", f(gx + d.x - 4), f(gy + d.y - 4), 24.0, 24.0, 0xffff_ffff);
        }
        // Carried item (§ 4.5).
        if let (Some((mx, my)), Some(c)) = (mouse, &self.carried_cache) {
            let (x, y) = (mx.floor() - 8.0, my.floor() - 8.0);
            if preview.is_some() && remaining <= 0 {
                gui.next_layer();
                let chars = crate::gui::plain("§e0");
                let tw = rc_text::width(gui, &chars);
                gui.text(queue, &chars, x + 17.0 - tw as f32, y + 9.0, 0xffff_ffff, true, 0);
            } else {
                gui.stratum(Blend::Premultiplied);
                if !c.layers.is_empty() {
                    gui.item(c.key, &c.layers, x, y, 0.0);
                }
                gui.stratum(Blend::Normal);
                let count = if preview.is_some() { remaining } else { c.stack.count };
                decorate(gui, queue, c, x, y, (count, false));
            }
        }
        // Tooltip (§ 4.7).
        // Carried item: only the tooltip of a container (bundle) stays displayed.
        let ctx = crate::bundle::Ctx { ids: it.ids, data: it.data };
        if let (Some(i), Some((mx, my))) = (hover, mouse)
            && let Some(st) = slots[i].as_ref()
            && let Some(contents) = Some(ctx.contents(st))
            && (inv.carried.is_none() || contents.is_some())
        {
            let fresh = self.tooltip.as_ref().is_some_and(|t| t.0 == *st);
            if !fresh {
                let lines = tooltip_lines(st, it, &self.hasher.enchantments, gui, self.advanced_tooltips);
                let width = lines.iter().map(|l| rc_text::width(gui, l)).max().unwrap_or(0);
                self.tooltip = Some((st.clone(), lines, width));
            }
            let info = usize::try_from(st.item).ok().and_then(|k| it.infos.get(k));
            let view = crate::stack::StackRef { stack: st, info, ids: it.ids, data: it.data, hold: crate::stack::Hold::NONE };
            let style = tooltip_style(&view);
            let (mx, my) = (mx.floor() as i32, my.floor() as i32);
            match (&self.tooltip, contents) {
                // Container (bundle): grid of the items under the name (`bundle.rs`).
                (Some((_, lines, _)), Some(items)) if !lines.is_empty() => {
                    let sel = crate::bundle::selected(i);
                    crate::bundle::draw_tooltip(gui, queue, it, &ctx, lines, &items, sel, (mx, my), style.as_deref());
                }
                (Some((_, lines, tw)), _) => {
                    draw_tooltip_ex(gui, queue, lines, *tw, (mx, my), true, style.as_deref());
                }
                _ => {}
            }
        }
        if hover.is_none_or(|i| slots.get(i).is_none_or(Option::is_none)) {
            crate::special::draw_front(gui, queue, inv, it, (gx, gy), mouse, &self.hasher.enchantments);
        }
    }
}

pub(crate) fn refresh(
    cache: &mut Option<Cached>,
    st: &ItemStack,
    it: &HotbarItems<'_>,
    models: &rc_assets::item::ItemModels,
    seed: i32,
) {
    refresh_held(cache, st, it, models, seed, crate::stack::Hold::NONE);
}

/// Refreshes a placed stack (menu slot, cursor): also redone when its place or the chosen
/// item of its bundle change.
pub(crate) fn refresh_held(
    cache: &mut Option<Cached>,
    st: &ItemStack,
    it: &HotbarItems<'_>,
    models: &rc_assets::item::ItemModels,
    seed: i32,
    hold: crate::stack::Hold,
) {
    use std::hash::{Hash, Hasher};
    let held = (hold, hold.menu_slot.map_or(-1, |s| crate::bundle::selected(usize::from(s))));
    if cache.as_ref().is_some_and(|c| c.tick == it.env.tick && c.hold == held && c.stack == *st) {
        return;
    }
    let info = usize::try_from(st.item).ok().and_then(|i| it.infos.get(i));
    let name = usize::try_from(st.item).ok().and_then(|i| it.names.get(i)).map_or("", String::as_str);
    let view = crate::stack::StackRef { stack: st, info, ids: it.ids, data: it.data, hold };
    let (mut layers, mut key) = match cache.take() {
        Some(c) => (c.layers, Vec::new()),
        None => (Vec::new(), Vec::new()),
    };
    layers.clear();
    if let Some(model) = view.model() {
        models.layers(name, &model, &view, it.env, seed, &mut layers, &mut key);
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut h);
    *cache =
        Some(Cached { stack: st.clone(), tick: it.env.tick, hold: held, layers, key: h.finish(), bar: view.durability_bar() });
}

/// Durability bar and count (`hud.md` § 3.5); capped count written in yellow.
pub(crate) fn decorate(gui: &mut Gui, queue: &wgpu::Queue, c: &Cached, x: f32, y: f32, (count, capped): (i32, bool)) {
    if let Some((w, color)) = c.bar {
        gui.fill(x + 2.0, y + 13.0, x + 15.0, y + 15.0, 0xff00_0000);
        gui.fill(x + 2.0, y + 13.0, x + 2.0 + w as f32, y + 14.0, 0xff00_0000 | color);
    }
    crate::hud::cooldown_veil(gui, &c.stack, x, y);
    if count != 1 {
        let s = if capped { format!("§e{count}") } else { count.to_string() };
        let chars = crate::gui::plain(&s);
        let tw = rc_text::width(gui, &chars);
        gui.text(queue, &chars, x + 17.0 - tw as f32, y + 9.0, 0xffff_ffff, true, 0);
    }
}

/// Enchantments of maximum level 1: no Roman numeral.
pub(crate) const SINGLE_LEVEL: [&str; 9] = [
    "aqua_affinity",
    "channeling",
    "flame",
    "infinity",
    "mending",
    "multishot",
    "silk_touch",
    "binding_curse",
    "vanishing_curse",
];

/// Advanced tooltips (F3 + H), also read by the screens outside containers (creative, book).
static ADVANCED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub(crate) fn advanced() -> bool {
    ADVANCED.load(std::sync::atomic::Ordering::Relaxed)
}

/// Tooltip lines: coloured name, enchantments, lore (§ 4.7, simplified).
pub(crate) fn tooltip_lines(
    st: &ItemStack,
    it: &HotbarItems<'_>,
    enchantments: &[String],
    gui: &Gui,
    advanced: bool,
) -> Vec<Vec<StyledChar>> {
    tooltip_components(st, it, enchantments, advanced)
        .iter()
        .map(|c| rc_text::styled_chars(&rc_text::flatten(c, &gui.lang)))
        .collect()
}

/// Tooltip lines for a search index (creative, recipe book), like the game:
/// plain text of each line (item in the text → its fallback text), valid § codes
/// removed, no blanks at the edges, lowercase; empty lines discarded.
pub(crate) fn tooltip_search_lines(
    st: &ItemStack,
    it: &HotbarItems<'_>,
    enchantments: &[String],
    gui: &Gui,
    advanced: bool,
) -> Vec<String> {
    tooltip_components(st, it, enchantments, advanced)
        .iter()
        .map(|c| search_line(c, &gui.lang))
        .filter(|l| !l.is_empty())
        .collect()
}

fn search_line(c: &Component, lang: &rc_text::Lang) -> String {
    rc_text::strip_codes(&rc_text::plain_string(c, lang)).trim().to_lowercase()
}

#[cfg(test)]
#[test]
fn search_line_indexes_object_fallback() {
    // Item name carrying an `object` component: the search finds "diamond" (fallback
    // text), not the drawn carrier character.
    use rc_proto::nbt::Nbt;
    let s = |v: &str| Nbt::String(v.into());
    let name = rc_text::from_nbt(&Nbt::Compound(vec![
        ("text".into(), s(" §6Épée ")),
        ("extra".into(), Nbt::List(vec![Nbt::Compound(vec![("type".into(), s("object")), ("sprite".into(), s("item/diamond"))])])),
    ]))
    .unwrap();
    assert_eq!(search_line(&name, &rc_text::Lang::default()), "épée [item/diamond]");
}

fn tooltip_components(st: &ItemStack, it: &HotbarItems<'_>, enchantments: &[String], advanced: bool) -> Vec<Component> {
    use rc_proto::Reader;
    use rc_proto::varint::read_varint;
    let info = usize::try_from(st.item).ok().and_then(|i| it.infos.get(i));
    let view = crate::stack::StackRef { stack: st, info, ids: it.ids, data: it.data, hold: crate::stack::Hold::NONE };
    // Tooltip entirely hidden (`tooltip_display.hide_tooltip`): only the operator
    // warning remains.
    if crate::tooltips::hidden(&view) {
        return crate::tooltips::hidden_lines(&view);
    }
    let mut out = vec![view.display_name()];
    // Map (`maps.md` § 9): in place of the `map_id` component, before the enchantments.
    if let Some(id) = crate::maps::map_id(it.ids, st) {
        let post = view.patch("minecraft:map_post_processing").and_then(|b| read_varint(&mut Reader::new(b)).ok());
        let named = view.patch("minecraft:custom_name").is_some();
        out.extend(crate::maps::tooltip(id, post, named, advanced));
    }
    // Potion, enchantments, dye, caption, attributes, unbreakable, advanced lines.
    crate::tooltips::details(&view, enchantments, &SINGLE_LEVEL, advanced, &mut out);
    out
}

/// Result slot of a screen that refuses grouped pickup (crafting, stonecutter,
/// smithing table, cartography table).
fn result_slot(kind: Kind, i: usize, n: usize) -> bool {
    use crate::menu_special::Sp;
    menu::slot_def(kind, i, n).craft_result
        || matches!((kind, i), (Kind::Special(Sp::Cartography), 2) | (Kind::Special(Sp::Stonecutter), 1))
        || matches!((kind, i), (Kind::Special(Sp::Smithing), 3))
}

/// The double click can pick up from this slot (the merchant refuses it everywhere).
fn pick_all_allowed(kind: Kind, i: usize, n: usize) -> bool {
    !matches!(kind, Kind::Special(crate::menu_special::Sp::Merchant)) && !result_slot(kind, i, n)
}

fn may_pickup(kind: Kind, i: usize) -> bool {
    !matches!(kind, Kind::Special(s) if crate::menu_special::locked(s, i))
}

/// Part of the menu the slot belongs to: 0 player inventory (armor and off
/// hand included), 1 result, 2 container or grid.
fn slot_group(kind: Kind, i: usize, n: usize) -> u8 {
    let start = match kind {
        Kind::Player => {
            return if i == 0 {
                1
            } else if i <= 4 {
                2
            } else {
                0
            };
        }
        Kind::Special(s) => crate::menu_special::player_start(s, n),
        _ => n.saturating_sub(36),
    };
    if (start..start + 36).contains(&i) {
        0
    } else if result_slot(kind, i, n) || matches!(kind, Kind::Special(crate::menu_special::Sp::Crafter) if i == 45) {
        1
    } else {
        2
    }
}

/// Item tooltip (§ 4.7): 2 px gap under the name, default style.
pub(crate) fn draw_tooltip(gui: &mut Gui, queue: &wgpu::Queue, lines: &[Vec<StyledChar>], w: i32, mx: i32, my: i32) {
    draw_tooltip_ex(gui, queue, lines, w, (mx, my), true, None);
}

/// Text tooltip (screen hints): lines at a pitch of 10, no gap under the first.
pub(crate) fn draw_text_tooltip(
    gui: &mut Gui,
    queue: &wgpu::Queue,
    lines: &[Vec<StyledChar>],
    w: i32,
    mx: i32,
    my: i32,
) {
    draw_tooltip_ex(gui, queue, lines, w, (mx, my), false, None);
}

/// Tooltip style of a stack (`tooltip_style` component, resource id).
pub(crate) fn tooltip_style(view: &crate::stack::StackRef<'_>) -> Option<String> {
    use rc_proto::Reader;
    let b = view.patch("minecraft:tooltip_style")?;
    rc_proto::read_str(&mut Reader::new(b), 32767).ok().map(str::to_owned)
}

/// Inner height of the tooltip: 10 per line, −2 at the end; the item name keeps its
/// 2 px gap (`gap`) as soon as it is followed by other lines.
pub(crate) fn tooltip_height(n: i32, gap: bool) -> i32 {
    10 * n + if gap && n != 1 { 0 } else { -2 }
}

/// Background and frame sprites: `tooltip/<path>_background` / `_frame` of the style.
pub(crate) fn tooltip_sprites(style: Option<&str>) -> (String, String) {
    match style {
        None => ("tooltip/background".to_owned(), "tooltip/frame".to_owned()),
        Some(s) => {
            let (ns, path) = s.split_once(':').unwrap_or(("minecraft", s));
            let pre = if ns == "minecraft" { String::new() } else { format!("{ns}:") };
            (format!("{pre}tooltip/{path}_background"), format!("{pre}tooltip/{path}_frame"))
        }
    }
}

/// Common tooltip positioner: text at (+12, −12) from the anchor, folded to the left if it
/// overflows on the right (4 px from the edge at least), raised if it overflows at the bottom.
pub(crate) fn tooltip_origin((mx, my): (i32, i32), (w, h): (i32, i32), (sw, sh): (i32, i32)) -> (i32, i32) {
    let mut x = mx + 12;
    let mut y = my - 12;
    if x + w > sw {
        x = (x - 24 - w).max(4);
    }
    if y + h + 3 > sh {
        y = sh - h - 3;
    }
    (x, y)
}

pub(crate) fn draw_tooltip_ex(
    gui: &mut Gui,
    queue: &wgpu::Queue,
    lines: &[Vec<StyledChar>],
    w: i32,
    (mx, my): (i32, i32),
    gap: bool,
    style: Option<&str>,
) {
    if lines.is_empty() {
        return;
    }
    let n = lines.len() as i32;
    let h = tooltip_height(n, gap);
    let (x, y) = tooltip_origin((mx, my), (w, h), (gui.width as i32, gui.height as i32));
    gui.tooltip_layer();
    let r = |v: i32| v as f32;
    let (bg, frame) = tooltip_sprites(style);
    gui.sprite(&bg, r(x - 12), r(y - 12), r(w + 24), r(h + 24), 0xffff_ffff);
    gui.sprite(&frame, r(x - 12), r(y - 12), r(w + 24), r(h + 24), 0xffff_ffff);
    let mut ty = y;
    for (k, l) in lines.iter().enumerate() {
        gui.text(queue, l, r(x), r(ty), 0xffff_ffff, true, 0);
        ty += 10 + if gap && k == 0 { 2 } else { 0 };
    }
}

#[cfg(test)]
mod tooltip_tests {
    use super::{tooltip_height, tooltip_sprites};

    #[test]
    fn heights_match_reference() {
        // Item: 1 line = 8, then 10 per line (the 2 px gap is in the drawing).
        assert_eq!(tooltip_height(1, true), 8);
        assert_eq!(tooltip_height(2, true), 20);
        assert_eq!(tooltip_height(4, true), 40);
        // Text: 10 per line minus 2.
        assert_eq!(tooltip_height(1, false), 8);
        assert_eq!(tooltip_height(3, false), 28);
    }

    #[test]
    fn pick_all_groups() {
        use super::{Kind, pick_all_allowed, slot_group};
        use crate::menu_special::Sp;
        // Inventory: result, grid, then armor + main inventory + hotbar + off hand together.
        assert_eq!([0, 3, 5, 9, 44, 45].map(|i| slot_group(Kind::Player, i, 46)), [1, 2, 0, 0, 0, 0]);
        assert!(!pick_all_allowed(Kind::Player, 0, 46) && pick_all_allowed(Kind::Player, 9, 46));
        // Single chest: 27 container slots then 36 of the player.
        assert_eq!([0, 26, 27, 62].map(|i| slot_group(Kind::Chest(3), i, 63)), [2, 2, 0, 0]);
        // Merchant: never; stonecutter: not the result.
        assert!(!pick_all_allowed(Kind::Special(Sp::Merchant), 10, 39));
        assert!(!pick_all_allowed(Kind::Special(Sp::Stonecutter), 1, 38));
        assert!(pick_all_allowed(Kind::Special(Sp::Stonecutter), 0, 38));
    }

    #[test]
    fn style_sprites() {
        assert_eq!(tooltip_sprites(None).0, "tooltip/background");
        assert_eq!(
            tooltip_sprites(Some("minecraft:fancy")),
            ("tooltip/fancy_background".into(), "tooltip/fancy_frame".into())
        );
        assert_eq!(tooltip_sprites(Some("fancy")).1, "tooltip/fancy_frame");
        assert_eq!(tooltip_sprites(Some("pack:a/b")).0, "pack:tooltip/a/b_background");
    }
}
