//! Bundles (`docs/specs/items.md`): contents (stack templates of the `bundle_contents` component),
//! weights as exact fractions, fill bar, grid tooltip, mouse-wheel selection
//! (`bundle_item_selected`) and click prediction (insertion, removal, sounds).

use std::cell::{Cell, RefCell};

use rc_proto::Reader;
use rc_proto::varint::{read_varint, write_varint};
use rc_proto_v777::item::ItemStack;
use rc_text::{Component, StyledChar};

use crate::gui::{Blend, Gui};
use crate::hud::HotbarItems;
use crate::screens::{Cached, decorate, refresh};
use crate::stack::{ComponentIds, StackRef};

/// Reduced fraction `n / d` (d > 0).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Frac(pub i64, pub i64);

fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 { a.abs().max(1) } else { gcd(b, a % b) }
}

impl Frac {
    pub const ZERO: Frac = Frac(0, 1);
    pub const ONE: Frac = Frac(1, 1);

    pub fn new(n: i64, d: i64) -> Frac {
        let g = gcd(n, d);
        Frac(n / g, d / g)
    }

    pub fn add(self, o: Frac) -> Frac {
        Frac::new(self.0 * o.1 + o.0 * self.1, self.1 * o.1)
    }

    pub fn sub(self, o: Frac) -> Frac {
        Frac::new(self.0 * o.1 - o.0 * self.1, self.1 * o.1)
    }

    pub fn times(self, k: i64) -> Frac {
        Frac::new(self.0 * k, self.1)
    }

    /// `self ≥ 1`.
    pub fn full(self) -> bool {
        self.0 >= self.1
    }

    /// Integer part of `self × k` (truncation).
    pub fn mul_trunc(self, k: i64) -> i64 {
        self.0 * k / self.1
    }
}

/// Bundle (tag `bundles`): the bundle and its 16 colors.
pub fn is_bundle_name(name: &str) -> bool {
    let n = name.strip_prefix("minecraft:").unwrap_or(name);
    n == "bundle" || n.ends_with("_bundle")
}

/// Network contents: list of templates (item, count, patch).
pub fn decode(ids: &ComponentIds, bytes: &[u8]) -> Option<Vec<ItemStack>> {
    let mut r = Reader::new(bytes);
    let n = read_varint(&mut r).ok()?;
    (0..n).map(|_| ids.codec.read_template(&mut r).ok()).collect()
}

/// Inverse encoding of [`decode`].
pub fn encode(items: &[ItemStack]) -> Vec<u8> {
    let mut w = Vec::new();
    write_varint(&mut w, items.len() as i32);
    for s in items {
        write_varint(&mut w, s.item);
        write_varint(&mut w, s.count);
        write_varint(&mut w, s.added.len() as i32);
        write_varint(&mut w, s.removed.len() as i32);
        for (ty, b) in &s.added {
            write_varint(&mut w, i32::from(*ty));
            w.extend_from_slice(b);
        }
        for ty in &s.removed {
            write_varint(&mut w, i32::from(*ty));
        }
    }
    w
}

/// Item data as seen by the bundles.
#[derive(Clone, Copy)]
pub struct Ctx<'a> {
    pub ids: &'a ComponentIds,
    pub data: &'a rc_data::VersionData,
}

impl Ctx<'_> {
    fn view<'s>(&'s self, st: &'s ItemStack) -> StackRef<'s> {
        let info = usize::try_from(st.item).ok().and_then(|i| self.data.items.get(i));
        StackRef { stack: st, info, ids: self.ids, data: self.data, hold: crate::stack::Hold::NONE }
    }

    pub fn name(&self, st: &ItemStack) -> &str {
        let n = usize::try_from(st.item)
            .ok()
            .and_then(|i| self.data.registry("minecraft:item")?.entries.get(i))
            .map_or("", String::as_str);
        n.strip_prefix("minecraft:").unwrap_or(n)
    }

    /// Contents of the component (patch), otherwise empty for a bundle; `None`: not a container.
    pub fn contents(&self, st: &ItemStack) -> Option<Vec<ItemStack>> {
        let id = self.ids.bundle_contents?;
        if st.removed.contains(&id) {
            return None;
        }
        match st.component(id) {
            Some(b) => decode(self.ids, b),
            None => is_bundle_name(self.name(st)).then(Vec::new),
        }
    }

    /// Contents of a bundle only (click behaviors).
    pub fn bundle(&self, st: &ItemStack) -> Option<Vec<ItemStack>> {
        if !is_bundle_name(self.name(st)) {
            return None;
        }
        self.contents(st)
    }

    /// Stack with new contents.
    pub fn with_contents(&self, st: &ItemStack, items: &[ItemStack]) -> ItemStack {
        let mut s = st.clone();
        if let Some(id) = self.ids.bundle_contents {
            s.added.retain(|c| c.0 != id);
            if !items.is_empty() || !is_bundle_name(self.name(st)) {
                s.added.push((id, encode(items).into_boxed_slice()));
                s.added.sort_by_key(|c| c.0);
            }
        }
        s
    }

    pub fn max_stack(&self, st: &ItemStack) -> i32 {
        rc_assets::item::StackView::max_stack_size(&self.view(st)).max(1)
    }

    /// Stackable: more than one per stack and not damaged.
    pub fn stackable(&self, st: &ItemStack) -> bool {
        let v = self.view(st);
        use rc_assets::item::StackView;
        v.max_stack_size() > 1 && !(v.max_damage() > 0 && v.damage() > 0)
    }

    /// Shulker boxes do not fit in a container.
    pub fn fits(&self, st: &ItemStack) -> bool {
        !self.name(st).ends_with("shulker_box")
    }

    /// Weight of an item: nested bundle = 1/16 + its contents; occupied beehive = 1; otherwise
    /// 1 / stack size.
    pub fn weight(&self, st: &ItemStack) -> Frac {
        if let Some(items) = self.contents(st) {
            return self.total(&items).add(Frac::new(1, 16));
        }
        let bees = self.ids.bees.and_then(|i| st.component(i)).and_then(|b| read_varint(&mut Reader::new(b)).ok());
        if bees.is_some_and(|n| n > 0) {
            return Frac::ONE;
        }
        Frac::new(1, i64::from(self.max_stack(st)))
    }

    pub fn total(&self, items: &[ItemStack]) -> Frac {
        items.iter().fold(Frac::ZERO, |w, s| w.add(self.weight(s).times(i64::from(s.count))))
    }

    /// Number of items the bundle can still receive at this unit weight.
    pub fn room(&self, weight: Frac, item: Frac) -> i32 {
        let left = Frac::ONE.sub(weight);
        // (left / item), integer part, never negative.
        let q = (left.0 * item.1).div_euclid(left.1 * item.0);
        q.clamp(0, i64::from(i32::MAX)) as i32
    }

    /// Insertion at the head (merging with an identical stack); returns the number inserted, `add` reduced.
    pub fn try_insert(&self, items: &mut Vec<ItemStack>, add: &mut ItemStack) -> i32 {
        if add.count <= 0 || !self.fits(add) {
            return 0;
        }
        let w = self.weight(add);
        let n = add.count.min(self.room(self.total(items), w));
        if n == 0 {
            return 0;
        }
        let same = self.stackable(add).then(|| items.iter().position(|s| crate::menu::same(s, add))).flatten();
        match same {
            Some(k) => {
                let mut s = items.remove(k);
                s.count += n;
                items.insert(0, s);
            }
            None => items.insert(0, ItemStack { count: n, ..add.clone() }),
        }
        add.count -= n;
        n
    }

    /// Removal of the chosen item (otherwise of the first).
    pub fn remove_one(&self, items: &mut Vec<ItemStack>, selected: i32) -> Option<ItemStack> {
        if items.is_empty() {
            return None;
        }
        let k = usize::try_from(selected).ok().filter(|&k| k < items.len()).unwrap_or(0);
        Some(items.remove(k))
    }
}

/// Items shown in the tooltip: 12 at most (11 and "+n" beyond), incomplete row at
/// the top.
pub fn shown(n: usize) -> usize {
    let avail = if n > 12 { 11 } else { 12 };
    let partial = n % 4;
    let empty = if partial == 0 { 0 } else { 4 - partial };
    n.min(avail - empty)
}

/// Fill bar (/ width / color).
pub fn bar(ctx: &Ctx<'_>, st: &ItemStack) -> Option<(i32, u32)> {
    let items = ctx.bundle(st)?;
    let w = ctx.total(&items);
    if w.0 <= 0 {
        return None;
    }
    let width = (1 + w.mul_trunc(12)).min(13) as i32;
    Some((width, if w.full() { 0xFF_5454 } else { 0x70_87FF }))
}

// ---------------------------------------------------------------------------------------------
// Client state: wheel choice, prediction sounds.

thread_local! {
    /// Chosen item per stack key (menu slot, or `CREATIVE_GRID` + place in the creative grid's
    /// list): the game keeps the choice on the stack itself, so a grid stack moved away from under
    /// the mouse keeps its own while another is chosen.
    static SELECTED: RefCell<Vec<(usize, i32)>> = const { RefCell::new(Vec::new()) };
    /// Wheel accumulation (x, y), like the game's handler.
    static WHEEL: Cell<(f64, f64)> = const { Cell::new((0.0, 0.0)) };
    /// Sound to play after a prediction: (event, volume, random pitch).
    static SOUND: Cell<Option<(&'static str, f32, bool)>> = const { Cell::new(None) };
    /// Tooltip pieces: items in cache.
    static CACHE: RefCell<Vec<Option<Cached>>> = const { RefCell::new(Vec::new()) };
}

/// Chosen item of the stack `key` (−1: none).
pub fn selected(key: usize) -> i32 {
    SELECTED.with(|s| s.borrow().iter().find(|e| e.0 == key).map_or(-1, |e| e.1))
}

/// Chooses item `index` of the stack `key` (−1: forgets its choice).
pub fn set_selected(key: usize, index: i32) {
    SELECTED.with(|s| {
        let mut s = s.borrow_mut();
        s.retain(|e| e.0 != key);
        if index >= 0 {
            s.push((key, index));
        }
    });
}

/// The creative grid shows other stacks (search, tab, reload): its old stacks are gone with
/// their choices.
pub fn forget_grid() {
    SELECTED.with(|s| s.borrow_mut().retain(|e| e.0 < CREATIVE_GRID));
}

/// Quick move (mode 1) or swap with the hotbar (mode 2) on a slot that holds a bundle: its stack
/// (`key`) forgets its chosen item, and the game announces −1 for the slot before the click even if
/// nothing was chosen. True if −1 is to be announced.
pub fn unselect_on_click(mode: i32, key: usize, holds_bundle: bool) -> bool {
    let unselect = matches!(mode, 1 | 2) && holds_bundle;
    if unselect {
        set_selected(key, -1);
    }
    unselect
}

/// [`unselect_on_click`] in a container screen: slot `slot` of `slots`; returns the slot to announce.
pub fn unselect_before_click(
    mode: i32,
    slot: i16,
    slots: &[Option<ItemStack>],
    is_bundle: impl Fn(&ItemStack) -> bool,
) -> Option<usize> {
    let i = usize::try_from(slot).ok()?;
    let holds = slots.get(i)?.as_ref().is_some_and(is_bundle);
    unselect_on_click(mode, i, holds).then_some(i)
}

pub fn sound(name: &'static str, volume: f32, random_pitch: bool) {
    SOUND.with(|s| s.set(Some((name, volume, random_pitch))));
}

pub fn take_sound() -> Option<(&'static str, f32, bool)> {
    SOUND.with(Cell::take)
}

pub const INSERT: &str = "minecraft:item.bundle.insert";
pub const INSERT_FAIL: &str = "minecraft:item.bundle.insert_fail";
pub const REMOVE_ONE: &str = "minecraft:item.bundle.remove_one";

/// Wheel: accumulation, whole notches returned (x, y); a change of direction resets to zero.
pub fn wheel(x: f64, y: f64) -> (i32, i32) {
    WHEEL.with(|c| {
        let (mut ax, mut ay) = c.get();
        // An idle axis (0) also resets its accumulation, as the game's sign of 0 is 0.
        if ax != 0.0 && rc_physics::signum(x) != rc_physics::signum(ax) {
            ax = 0.0;
        }
        if ay != 0.0 && rc_physics::signum(y) != rc_physics::signum(ay) {
            ay = 0.0;
        }
        ax += x;
        ay += y;
        let (wx, wy) = (ax as i32, ay as i32);
        c.set((ax - f64::from(wx), ay - f64::from(wy)));
        (wx, wy)
    })
}

/// Next choice: one notch up goes back, wraps around at `limit`.
pub fn next_selection(wheel: i32, current: i32, limit: i32) -> i32 {
    let mut c = (current - wheel.signum()).max(-1);
    while c < 0 {
        c += limit;
    }
    while c >= limit {
        c -= limit;
    }
    c
}

// ---------------------------------------------------------------------------------------------
// Tooltip.

/// Height of the image (grid or empty-bundle description, bar, margins).
pub fn image_height(gui: &Gui, items: &[ItemStack]) -> i32 {
    if items.is_empty() {
        let lines = rc_text::wrap(gui, &empty_description(gui), 96.0).len() as i32;
        return lines * 9 + 13 + 8;
    }
    rows(items.len()) * 24 + 13 + 8
}

fn rows(n: usize) -> i32 {
    (n.min(12) as i32 + 3) / 4
}

fn empty_description(gui: &Gui) -> Vec<StyledChar> {
    let c = Component::translate("item.minecraft.bundle.empty.description", vec![]);
    rc_text::styled_chars(&rc_text::flatten(&c, &gui.lang))
}

/// Drawing of the image at `(x, y)`, tooltip width `w`; `selected`: chosen item.
#[allow(clippy::too_many_arguments)]
pub fn draw_image(
    gui: &mut Gui,
    queue: &wgpu::Queue,
    it: &HotbarItems<'_>,
    ctx: &Ctx<'_>,
    items: &[ItemStack],
    selected: i32,
    (x, y, w): (i32, i32, i32),
) {
    let f = |v: i32| v as f32;
    let left = x + (w - 96) / 2;
    let weight = ctx.total(items);
    let text =
        |gui: &Gui, k: &str| rc_text::styled_chars(&rc_text::flatten(&Component::translate(k, vec![]), &gui.lang));
    if items.is_empty() {
        let lines = rc_text::wrap(&*gui, &empty_description(gui), 96.0);
        for (k, l) in lines.iter().enumerate() {
            gui.text(queue, l, f(left), f(y + 9 * k as i32), 0xFFAA_AAAA, true, 0);
        }
        progress(
            gui,
            queue,
            left,
            y + lines.len() as i32 * 9 + 4,
            Frac::ZERO,
            &text(gui, "item.minecraft.bundle.empty"),
        );
        return;
    }
    let overflow = items.len() > 12;
    let n = shown(items.len());
    let grid = rows(items.len());
    let (x0, y0) = (left + 96, y + grid * 24);
    // Items in cache, in display order.
    CACHE.with(|c| {
        let mut cache = c.borrow_mut();
        cache.resize_with(n, || None);
        if let Some(models) = gui.items.as_ref().map(|a| &a.models) {
            for (k, s) in items.iter().take(n).enumerate() {
                refresh(&mut cache[k], s, it, models, k as i32);
            }
        }
        let mut slot = 1;
        let mut placed: Vec<(i32, i32, usize, bool)> = Vec::new();
        for row in 1..=grid {
            for col in 1..=4 {
                let (dx, dy) = (x0 - col * 24, y0 - row * 24);
                if overflow && col * row == 1 {
                    let hidden: i32 = items.iter().skip(n).map(|s| s.count).sum();
                    let chars = crate::gui::plain(&format!("+{hidden}"));
                    let tw = rc_text::width(gui, &chars);
                    gui.text(queue, &chars, f(dx + 12 - tw / 2), f(dy + 10), 0xffff_ffff, true, 0);
                } else if slot <= n {
                    let k = n - slot;
                    let hl = k as i32 == selected;
                    let back =
                        if hl { "container/bundle/slot_highlight_back" } else { "container/bundle/slot_background" };
                    gui.sprite(back, f(dx), f(dy), 24.0, 24.0, 0xffff_ffff);
                    placed.push((dx, dy, k, hl));
                    slot += 1;
                }
            }
        }
        gui.stratum(Blend::Premultiplied);
        for &(dx, dy, k, _) in &placed {
            if let Some(c) = &cache[k]
                && !c.layers.is_empty()
            {
                gui.item(c.key, &c.layers, f(dx + 4), f(dy + 4), 0.0);
            }
        }
        gui.stratum(Blend::Normal);
        for &(dx, dy, k, hl) in &placed {
            if let Some(c) = &cache[k] {
                decorate(gui, queue, c, f(dx + 4), f(dy + 4), (c.stack.count, false));
            }
            if hl {
                gui.sprite("container/bundle/slot_highlight_front", f(dx), f(dy), 24.0, 24.0, 0xffff_ffff);
            }
        }
    });
    let label = if weight.full() { text(gui, "item.minecraft.bundle.full") } else { Vec::new() };
    progress(gui, queue, left, y + grid * 24 + 4, weight, &label);
    // Name of the chosen item: ordinary tooltip (common positioner, style of this stack)
    // anchored above the center.
    if let Some(s) = usize::try_from(selected).ok().and_then(|k| items.get(k)) {
        let view = ctx.view(s);
        let chars = rc_text::styled_chars(&rc_text::flatten(&view.display_name(), &gui.lang));
        let tw = rc_text::width(gui, &chars);
        let style = crate::screens::tooltip_style(&view);
        crate::screens::draw_tooltip_ex(gui, queue, &[chars], tw, selected_anchor(x, y, w, tw), true, style.as_deref());
    }
}

/// Anchor of the chosen item's tooltip (before the common positioner: +12, −12, fallbacks).
fn selected_anchor(x: i32, y: i32, w: i32, text_width: i32) -> (i32, i32) {
    (x + w / 2 - 12 - text_width / 2, y - 15)
}

/// Fill bar: fill (full: red), frame, centered label.
fn progress(gui: &mut Gui, queue: &wgpu::Queue, x: i32, y: i32, w: Frac, label: &[StyledChar]) {
    let fill = w.mul_trunc(94).clamp(0, 94) as f32;
    let sprite =
        if w.full() { "container/bundle/bundle_progressbar_full" } else { "container/bundle/bundle_progressbar_fill" };
    if fill > 0.0 {
        gui.sprite(sprite, (x + 1) as f32, y as f32, fill, 13.0, 0xffff_ffff);
    }
    gui.sprite("container/bundle/bundle_progressbar_border", x as f32, y as f32, 96.0, 13.0, 0xffff_ffff);
    if !label.is_empty() {
        let tw = rc_text::width(gui, label);
        gui.text(queue, label, (x + 48 - tw / 2) as f32, (y + 3) as f32, 0xffff_ffff, true, 0);
    }
}

/// Full tooltip of a container: first line, image, other lines.
#[allow(clippy::too_many_arguments)]
pub fn draw_tooltip(
    gui: &mut Gui,
    queue: &wgpu::Queue,
    it: &HotbarItems<'_>,
    ctx: &Ctx<'_>,
    lines: &[Vec<StyledChar>],
    items: &[ItemStack],
    selected: i32,
    (mx, my): (i32, i32),
    style: Option<&str>,
) {
    let tw = lines.iter().map(|l| rc_text::width(gui, l)).max().unwrap_or(0).max(96);
    let img = image_height(gui, items);
    // Name (10) + image + 10 per remaining line; the 2 px gap under the name is not in h.
    let h = 10 + img + 10 * lines.len().saturating_sub(1) as i32;
    let (x, y) = crate::screens::tooltip_origin((mx, my), (tw, h), (gui.width as i32, gui.height as i32));
    let r = |v: i32| v as f32;
    gui.tooltip_layer();
    let (bg, frame) = crate::screens::tooltip_sprites(style);
    gui.sprite(&bg, r(x - 12), r(y - 12), r(tw + 24), r(h + 24), 0xffff_ffff);
    gui.sprite(&frame, r(x - 12), r(y - 12), r(tw + 24), r(h + 24), 0xffff_ffff);
    if let Some(first) = lines.first() {
        gui.text(queue, first, r(x), r(y), 0xffff_ffff, true, 0);
    }
    let iy = y + 12;
    draw_image(gui, queue, it, ctx, items, selected, (x, iy, tw));
    let mut ty = iy + img;
    for l in lines.iter().skip(1) {
        gui.text(queue, l, r(x), r(ty), 0xffff_ffff, true, 0);
        ty += 10;
    }
}

/// Key of a creative grid slot, and of a stack of the grid's list, in the bundle tables: after
/// every menu slot (the creative screen's menu slots keep their index).
pub const CREATIVE_GRID: usize = 1000;

thread_local! {
    /// Slot hovered at the previous frame (key), to announce the one the mouse leaves.
    static HOVERED: Cell<Option<usize>> = const { Cell::new(None) };
    /// Screen of the previous frame (`HoverScreen::id`), `None` without a screen.
    static SCREEN: Cell<Option<u32>> = const { Cell::new(None) };
}

/// What the stop-hover rule needs of the open screen, for slot keys.
pub(crate) trait HoverScreen {
    /// The slot holds an item of the `bundles` tag.
    fn holds_bundle(&self, key: usize) -> bool;
    /// Slot index announced to the server.
    fn index(&self, key: usize) -> Option<i32>;
    /// Key of the stack the slot shows, in the chosen-item table.
    fn stack_key(&self, key: usize) -> usize;
    /// Identity of the screen: another one replaces it without any slot being left.
    fn id(&self) -> u32;
}

/// Everything forgotten: chosen items, and the wheel accumulation (the game's next screen makes its
/// own).
fn forget() {
    SELECTED.with(|c| c.borrow_mut().clear());
    WHEEL.with(|w| w.set((0.0, 0.0)));
}

/// Hovered slot of the frame (`now`, `None` without a screen): when it changes, the slot left
/// forgets its chosen item and, if it holds a bundle, −1 is announced for it, whatever was
/// chosen, like the game each time its hovered slot changes; without a screen, or when another
/// screen replaced the previous one (opened by the server, creative and survival inventories
/// swapped), everything is forgotten silently and the wheel accumulation starts again from zero.
/// Returns the announcement to send.
pub(crate) fn hover_frame(now: Option<usize>, screen: Option<&dyn HoverScreen>) -> Option<i32> {
    let before = HOVERED.with(|h| h.replace(now));
    let id = screen.map(|s| s.id());
    let replaced = SCREEN.with(|c| c.replace(id)) != id;
    let Some(s) = screen.filter(|_| !replaced) else {
        forget();
        return None;
    };
    let k = before.filter(|_| before != now)?;
    leave(k, s)
}

/// The mouse leaves slot `k` (or its screen is closed by the player): the stack shown there
/// forgets its chosen item, −1 to announce if it is a bundle.
fn leave(k: usize, s: &dyn HoverScreen) -> Option<i32> {
    set_selected(s.stack_key(k), -1);
    if s.holds_bundle(k) { s.index(k) } else { None }
}

/// Screen laid out again (window resized): a new wheel accumulation, like the game's new mouse
/// action; `rebuilt`: its slots are new (creative inventory tab), so the hovered one is left
/// (announced if it holds a bundle) and the next frame enters the new one.
pub(crate) fn screen_init(rebuilt: Option<&dyn HoverScreen>) -> Option<i32> {
    WHEEL.with(|w| w.set((0.0, 0.0)));
    hover_close(rebuilt?)
}

/// Screen closed by the player: its hovered slot is left (announced after the closing packet,
/// like the game).
pub(crate) fn hover_close(s: &dyn HoverScreen) -> Option<i32> {
    let k = HOVERED.with(Cell::take)?;
    leave(k, s)
}

/// Open screen seen through slot keys: container screen (menu slots) or creative screen.
struct OpenScreen<'a> {
    app: &'a crate::App,
}

impl<'a> OpenScreen<'a> {
    fn stack(&self, key: usize) -> Option<&'a ItemStack> {
        let a = self.app;
        if a.creative_ui.open {
            return match key.checked_sub(CREATIVE_GRID) {
                Some(i) => a.creative_ui.grid_stack(i),
                None => a.inventory.menu.get(key)?.as_ref(),
            };
        }
        crate::screens::current(&a.inventory).1.get(key)?.as_ref()
    }
}

impl HoverScreen for OpenScreen<'_> {
    fn holds_bundle(&self, key: usize) -> bool {
        let name = |s: &ItemStack| usize::try_from(s.item).ok().and_then(|i| self.app.item_names.get(i));
        self.stack(key).and_then(name).is_some_and(|n| is_bundle_name(n))
    }

    fn index(&self, key: usize) -> Option<i32> {
        let a = self.app;
        if !a.creative_ui.open {
            return i32::try_from(key).ok();
        }
        creative_index(key, a.creative_ui.kind(&a.data) == rc_data::CreativeKind::Inventory)
    }

    fn stack_key(&self, key: usize) -> usize {
        match key.checked_sub(CREATIVE_GRID) {
            Some(i) if self.app.creative_ui.open => self.app.creative_ui.stack_key(crate::creative_screen::Hit::Grid(i)),
            _ => key,
        }
    }

    /// Creative screen, otherwise the window of the container screen (0: inventory).
    fn id(&self) -> u32 {
        if self.app.creative_ui.open { u32::MAX } else { crate::screens::current(&self.app.inventory).2 as u32 }
    }
}

/// Index the game announces for a creative screen slot key: grid 0..44, bar 45..53, every slot
/// of the inventory tab 0 (`creative_app::creative_slot_index`).
pub(crate) fn creative_index(key: usize, inventory_tab: bool) -> Option<i32> {
    let hit = match key.checked_sub(CREATIVE_GRID) {
        Some(i) => crate::creative_screen::Hit::Grid(i),
        None => crate::creative_screen::Hit::Menu(key),
    };
    crate::creative_app::creative_slot_index(hit, inventory_tab)
}

impl crate::App {
    fn screen_open(&self) -> bool {
        self.screens.open || self.creative_ui.open
    }

    /// Slot under the mouse in the open screen, as a slot key (`HoverScreen`): menu slot of a
    /// container screen; creative screen: menu slot, or grid slot `CREATIVE_GRID + i`.
    fn hovered_key(&self) -> Option<usize> {
        let gui = self.renderer.as_ref().map(|r| &r.gui)?;
        let m = self.gui_mouse()?;
        if self.creative_ui.open {
            return match self.creative_ui.hovered(gui, &self.data, m)? {
                crate::creative_screen::Hit::Trash => None,
                h => Some(crate::creative_screen::hit_key(h)),
            };
        }
        if self.screens.open { self.screens.hovered_slot(gui, &self.inventory, m) } else { None }
    }

    /// Items shown by the hovered bundle (0: no bundle, or nothing to show).
    fn hovered_bundle_shown(&self) -> Option<(usize, i32)> {
        let key = self.hovered_key()?;
        let st = OpenScreen { app: self }.stack(key)?;
        let ctx = Ctx { ids: &self.component_ids, data: &self.data };
        let n = ctx.bundle(st).map_or(0, |items| shown(items.len()) as i32);
        (n > 0).then_some((key, n))
    }

    /// The wheel goes to the hovered bundle first (it shows items): container screens with their
    /// own scrolling (merchant, stonecutter, loom, creative grid) only get it otherwise.
    pub(crate) fn bundle_takes_wheel(&self) -> bool {
        self.hovered_bundle_shown().is_some()
    }

    /// Wheel over the open screen (scaled offsets): choice of the hovered bundle's item (never the
    /// bar); true if a bundle took it.
    pub(crate) fn bundle_wheel(&mut self, x: f64, y: f64) -> bool {
        let Some((key, limit)) = self.hovered_bundle_shown() else { return false };
        let (wx, wy) = wheel(x, y);
        let w = if wy == 0 { -wx } else { wy };
        if w == 0 {
            return true;
        }
        let screen = OpenScreen { app: self };
        let stack = screen.stack_key(key);
        let cur = selected(stack);
        let next = next_selection(w, cur, limit);
        if next != cur {
            set_selected(stack, next);
            if let Some(i) = screen.index(key) {
                let _ = self.session.link.send_bundle_item_selected(i, next);
            }
        }
        true
    }

    /// Each frame: slot left by the mouse (`hover_frame`).
    pub(crate) fn bundle_hover(&self) {
        let now = if self.screen_open() { self.hovered_key() } else { None };
        let screen = OpenScreen { app: self };
        let open: Option<&dyn HoverScreen> = if self.screen_open() { Some(&screen) } else { None };
        if let Some(i) = hover_frame(now, open) {
            let _ = self.session.link.send_bundle_item_selected(i, -1);
        }
    }

    /// Screen closed by the player, its closing packet sent: the hovered bundle is left.
    pub(crate) fn bundle_close(&self) {
        if let Some(i) = hover_close(&OpenScreen { app: self }) {
            let _ = self.session.link.send_bundle_item_selected(i, -1);
        }
    }

    /// Window resized with a screen open (`screen_init`): the creative inventory tab rebuilds its
    /// slots.
    pub(crate) fn bundle_resize(&self) {
        let screen = OpenScreen { app: self };
        let rebuilt =
            self.creative_ui.open && self.creative_ui.kind(&self.data) == rc_data::CreativeKind::Inventory;
        if let Some(i) = screen_init(rebuilt.then_some(&screen as &dyn HoverScreen)) {
            let _ = self.session.link.send_bundle_item_selected(i, -1);
        }
    }

    /// Sound of a bundle click prediction (local player, players category).
    pub(crate) fn bundle_sound(&mut self) {
        let Some((name, volume, random)) = take_sound() else { return };
        let pitch = if random { 0.8 + self.world_random.f() * 0.4 } else { 1.0 };
        if let Some(a) = self.audio.get() {
            let p = self.player.pos;
            a.play(name, rc_audio::Category::Player, Some([p[0], p[1], p[2]]), volume, pitch, None);
        }
    }
}

#[cfg(test)]
mod tests {
    /// Quick move and swap of a bundle slot: choice reset to −1; other modes, empty
    /// slot, other item or slot outside the menu: nothing.
    #[test]
    fn quick_move_and_swap_unselect_bundle() {
        use super::unselect_before_click as u;
        let st = |item| Some(rc_proto_v777::item::ItemStack { item, count: 1, added: Vec::new(), removed: Vec::new() });
        let slots = vec![st(7), None, st(3)];
        let is_bundle = |s: &rc_proto_v777::item::ItemStack| s.item == 7;
        assert_eq!(u(1, 0, &slots, is_bundle), Some(0));
        assert_eq!(u(2, 0, &slots, is_bundle), Some(0));
        for mode in [0, 3, 4, 5, 6] {
            assert_eq!(u(mode, 0, &slots, is_bundle), None);
        }
        assert_eq!(u(1, 1, &slots, is_bundle), None);
        assert_eq!(u(1, 2, &slots, is_bundle), None);
        assert_eq!(u(1, -999, &slots, is_bundle), None);
        assert_eq!(u(2, 9, &slots, is_bundle), None);
        // L10: the bundle's choice goes before the click (it stayed on the slot).
        super::set_selected(0, 2);
        assert_eq!(u(0, 0, &slots, is_bundle), None);
        assert_eq!(super::selected(0), 2, "a simple click keeps the choice");
        assert_eq!(u(2, 0, &slots, is_bundle), Some(0));
        assert_eq!(super::selected(0), -1, "swap: choice forgotten");
    }

    /// L10: choices are per stack: forgetting one keeps the others; a rebuilt creative grid
    /// forgets its stacks' choices only; quick move or swap of a bundle forgets its choice, other
    /// clicks and other items keep theirs.
    #[test]
    fn choices_per_stack() {
        use super::{CREATIVE_GRID, forget_grid, selected, set_selected, unselect_on_click};
        set_selected(20, 2);
        set_selected(CREATIVE_GRID + 50, 1);
        set_selected(CREATIVE_GRID + 3, 0);
        assert_eq!((selected(20), selected(CREATIVE_GRID + 50), selected(CREATIVE_GRID + 3)), (2, 1, 0));
        set_selected(CREATIVE_GRID + 3, -1);
        assert_eq!((selected(20), selected(CREATIVE_GRID + 50), selected(CREATIVE_GRID + 3)), (2, 1, -1));
        set_selected(20, 4);
        assert_eq!(selected(20), 4, "a new choice replaces the old one");
        forget_grid();
        assert_eq!((selected(20), selected(CREATIVE_GRID + 50)), (4, -1));
        assert!(!unselect_on_click(0, 20, true));
        assert!(!unselect_on_click(2, 20, false));
        assert_eq!(selected(20), 4);
        assert!(unselect_on_click(1, 20, true));
        assert_eq!(selected(20), -1);
        assert!(unselect_on_click(2, 21, true), "announced even with nothing chosen");
    }

    /// Screen whose slots 5 and 9 hold bundles; announced index = key + 100; grid slots
    /// (`CREATIVE_GRID` + i) show the stack `CREATIVE_GRID + i + 9 × row`; identity `id`.
    struct Two {
        row: std::cell::Cell<usize>,
        id: std::cell::Cell<u32>,
    }
    impl super::HoverScreen for Two {
        fn holds_bundle(&self, key: usize) -> bool {
            key == 5 || key == 9 || key >= super::CREATIVE_GRID
        }
        fn index(&self, key: usize) -> Option<i32> {
            Some(key as i32 + 100)
        }
        fn stack_key(&self, key: usize) -> usize {
            if key >= super::CREATIVE_GRID { key + 9 * self.row.get() } else { key }
        }
        fn id(&self) -> u32 {
            self.id.get()
        }
    }

    fn two() -> Two {
        Two { row: std::cell::Cell::new(0), id: std::cell::Cell::new(0) }
    }

    /// L10: a grid scrolled under the mouse keeps each stack's choice (the slot is not left): the
    /// stack brought under the mouse has none, the one moved away keeps its own, and leaving the
    /// slot forgets the choice of the stack it shows then.
    #[test]
    fn grid_choice_stays_on_its_stack() {
        use super::{CREATIVE_GRID as G, hover_frame, selected, set_selected};
        let t = two();
        let s: &dyn super::HoverScreen = &t;
        assert_eq!(hover_frame(Some(G), Some(s)), None);
        set_selected(s.stack_key(G), 2);
        t.row.set(1);
        assert_eq!(hover_frame(Some(G), Some(s)), None, "same slot: nothing announced");
        assert_eq!(selected(s.stack_key(G)), -1, "the new stack has no choice");
        set_selected(s.stack_key(G), 1);
        t.row.set(0);
        assert_eq!(hover_frame(Some(G), Some(s)), None);
        assert_eq!(selected(s.stack_key(G)), 2, "the old stack kept its choice");
        assert_eq!(hover_frame(Some(G + 1), Some(s)), Some(G as i32 + 100));
        assert_eq!((selected(G), selected(G + 9)), (-1, 1), "only the stack shown when left forgets");
        // Slot left while the grid is scrolled: the stack it shows (place 9) forgets, place 0 keeps.
        set_selected(G, 2);
        t.row.set(1);
        assert_eq!(hover_frame(Some(G), Some(s)), Some(G as i32 + 101));
        assert_eq!(hover_frame(Some(G + 2), Some(s)), Some(G as i32 + 100));
        assert_eq!((selected(G), selected(G + 9)), (2, -1));
    }

    /// L10: another screen replacing the open one (no frame without a screen) forgets everything
    /// silently and starts a new wheel accumulation; a resize starts one too, and a rebuilt
    /// creative inventory tab leaves its hovered slot.
    #[test]
    fn replaced_or_resized_screen() {
        use super::{hover_frame, screen_init, selected, set_selected, wheel};
        let t = two();
        let s: &dyn super::HoverScreen = &t;
        assert_eq!(hover_frame(Some(5), Some(s)), None);
        set_selected(5, 3);
        assert_eq!(wheel(0.0, 0.6), (0, 0));
        t.id.set(7);
        assert_eq!(hover_frame(Some(9), Some(s)), None, "replaced: slot 5 not announced");
        assert_eq!(selected(5), -1);
        assert_eq!(wheel(0.0, 0.6), (0, 0), "accumulation started again");
        assert_eq!(screen_init(None), None);
        assert_eq!(wheel(0.0, 0.6), (0, 0), "resize: started again");
        assert_eq!(wheel(0.0, 0.6), (0, 1));
        set_selected(9, 1);
        assert_eq!(screen_init(Some(s)), Some(109), "slots rebuilt: hovered bundle left");
        assert_eq!(selected(9), -1);
        assert_eq!(hover_frame(Some(9), Some(s)), None, "the new slot is entered");
    }

    /// Leaving a bundle slot announces −1 for it even with nothing chosen; leaving another slot,
    /// staying on one, or a screen closed by the server announce nothing; the chosen item is
    /// forgotten when its slot is left; closing by the player leaves the hovered slot.
    #[test]
    fn leaving_a_bundle_slot_announces_no_choice() {
        use super::{hover_close, hover_frame, selected, set_selected};
        let t = two();
        let s: &dyn super::HoverScreen = &t;
        assert_eq!(hover_frame(None, None), None);
        assert_eq!(hover_frame(Some(5), Some(s)), None, "entering announces nothing");
        assert_eq!(hover_frame(Some(5), Some(s)), None, "staying announces nothing");
        assert_eq!(hover_frame(Some(7), Some(s)), Some(105), "bundle left, nothing chosen");
        assert_eq!(hover_frame(None, Some(s)), None, "slot 7 holds no bundle");
        assert_eq!(hover_frame(Some(9), Some(s)), None);
        set_selected(9, 2);
        assert_eq!(hover_frame(Some(5), Some(s)), Some(109));
        assert_eq!(selected(9), -1, "choice forgotten");
        set_selected(5, 1);
        assert_eq!(hover_frame(None, None), None, "closed by the server: silent");
        assert_eq!(selected(5), -1);
        assert_eq!(hover_frame(Some(9), Some(s)), None);
        assert_eq!(hover_close(s), Some(109), "closed by the player");
        assert_eq!(hover_close(s), None);
    }

    /// Micro-bench of the per-frame lookups: chosen item of a drawn slot (former single cell against
    /// the per-stack table, empty and with 4 choices) and the hovered-slot rule of a frame.
    /// `cargo test --release -p rc-app --bin rustcraft bench_bundle_choices -- --ignored --nocapture`
    #[test]
    #[ignore = "micro-bench"]
    fn bench_bundle_choices() {
        use super::{CREATIVE_GRID as G, hover_frame, selected, set_selected};
        use std::hint::black_box;
        let cell: std::cell::Cell<Option<(usize, i32)>> = std::cell::Cell::new(Some((3, 1)));
        let former = |slot: usize| cell.get().filter(|(k, _)| *k == slot).map_or(-1, |(_, i)| i);
        let n = 4_000_000usize;
        let t = two();
        let s: &dyn super::HoverScreen = &t;
        let time = |f: &mut dyn FnMut(usize) -> i64| {
            let t0 = std::time::Instant::now();
            let mut k = 0i64;
            for i in 0..n {
                k += f(black_box(i % 64));
            }
            black_box(k);
            t0.elapsed().as_nanos() as f64 / n as f64
        };
        for pass in 0..3 {
            let a = time(&mut |i| i64::from(former(i)));
            for k in [3, 40, G + 5, G + 70] {
                set_selected(k, -1);
            }
            let b = time(&mut |i| i64::from(selected(i)));
            for (k, v) in [(3, 1), (40, 3), (G + 5, 2), (G + 70, 0)] {
                set_selected(k, v);
            }
            let c = time(&mut |i| i64::from(selected(i)));
            let h = time(&mut |i| i64::from(hover_frame(Some(5 + (i & 1) * 2), Some(s)).unwrap_or(0)));
            eprintln!("pass {pass}: former cell {a:.2} ns, table empty {b:.2} ns, 4 choices {c:.2} ns, hover rule {h:.2} ns");
        }
    }

    /// Creative screen keys: grid slots after every menu, announced with the creative screen's
    /// index (grid 0..44, bar 45..53, inventory tab 0).
    #[test]
    fn creative_bundle_keys() {
        use crate::creative_screen::{Hit, hit_key};
        assert_eq!(hit_key(Hit::Grid(7)), super::CREATIVE_GRID + 7);
        assert_eq!(hit_key(Hit::Menu(36)), 36);
        assert_eq!(super::creative_index(super::CREATIVE_GRID + 7, false), Some(7));
        assert_eq!(super::creative_index(36, false), Some(45));
        assert_eq!(super::creative_index(12, true), Some(0));
    }

    /// Tooltip of the chosen item: anchor (center − 12 − half-width, y − 15) then common positioner
    /// (+12, −12, fallbacks at the edges).
    #[test]
    fn chosen_item_tooltip() {
        // Bundle tooltip at (100, 50), 120 wide; name 40 px; screen 427 × 240.
        let at = super::selected_anchor(100, 50, 120, 40);
        assert_eq!(at, (128, 35));
        assert_eq!(crate::screens::tooltip_origin(at, (40, 8), (427, 240)), (140, 23));
        // Near the right edge: flipped to the left of the anchor.
        let at = super::selected_anchor(380, 50, 120, 40);
        assert_eq!(crate::screens::tooltip_origin(at, (40, 8), (427, 240)), (356, 23));
    }
}
