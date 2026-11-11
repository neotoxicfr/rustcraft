//! Advancements screen (`docs/specs/advancements-stats.md` § 4): tabs, tree, scrolling, hover
//! overlay, detailed tooltip, tiled background.

use rc_proto_v777::item::ItemStack;
use rc_text::{Component, StyledChar};
use winit::keyboard::KeyCode;

use crate::advancements::{Advancements, Frame, Progress};
use crate::gui::{Blend, Gui};
use crate::hud::HotbarItems;
use crate::kept_text::Kept;
use crate::screens::{Cached, refresh};
use crate::widgets::{self, Event, Panel, Widget, chars, tr};

#[cfg(test)]
mod equivalence_tests;

const DONE: u32 = 1;
/// Inside of the window.
const IW: i32 = 234;
const IH: i32 = 113;
const MAX_TABS: usize = 26;

/// Optimisations of the advancements screen, each behind its switch (`=0`: old path).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AdvancementOpts {
    /// `RUSTCRAFT_OPT_ADVANCEMENT_TILED_BACKGROUND`: the tab background is one repeated quad
    /// instead of 170 sprites.
    tiled_background: bool,
    /// `RUSTCRAFT_OPT_ADVANCEMENT_MERGED_LINKS`: the black outline of a link is 4 rectangles
    /// instead of 8 one-pixel lines.
    merged_links: bool,
    /// `RUSTCRAFT_OPT_ADVANCEMENT_SCREEN_CACHES`: draw order, node states and titles kept between
    /// frames, nodes and links outside the window skipped, a missing background not read again.
    screen_caches: bool,
}

impl AdvancementOpts {
    fn from_switches() -> AdvancementOpts {
        AdvancementOpts {
            tiled_background: rc_data::opt_switch!("RUSTCRAFT_OPT_ADVANCEMENT_TILED_BACKGROUND"),
            merged_links: rc_data::opt_switch!("RUSTCRAFT_OPT_ADVANCEMENT_MERGED_LINKS"),
            screen_caches: rc_data::opt_switch!("RUSTCRAFT_OPT_ADVANCEMENT_SCREEN_CACHES"),
        }
    }
}

/// Request from the screen to the application.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Back to the parent screen (pause menu) or to the game.
    Back,
}

/// Requested cursor shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    Default,
    Hand,
    Move,
    MoveH,
    MoveV,
}

/// Tooltip of a node: title lines, description lines, width.
type Layout = (Vec<Vec<StyledChar>>, Vec<Vec<StyledChar>>, i32);

struct TabNode {
    id: String,
    parent: Option<usize>,
    children: Vec<usize>,
    x: i32,
    y: i32,
    frame: Frame,
    hidden: bool,
    title: Component,
    description: Component,
    icon: ItemStack,
    progress: Option<Progress>,
    /// Answers of [`TabNode::visible`] and of "fully obtained" when the tab was built: the
    /// progress of a node only changes with a rebuild, the criteria need not be counted again on
    /// every frame.
    shown_when_built: bool,
    obtained_when_built: bool,
    cache: Option<Cached>,
    /// Tooltip: title lines, description lines, width (computed on first need).
    layout: Option<Layout>,
}

impl TabNode {
    fn percent(&self) -> f32 {
        self.progress.as_ref().map_or(0.0, Progress::percent)
    }

    fn done(&self) -> bool {
        self.progress.as_ref().is_some_and(Progress::done)
    }

    /// Drawn and hoverable: not hidden, or done.
    fn visible(&self) -> bool {
        !self.hidden || self.done()
    }

    /// [`TabNode::visible`], answered from the build when `kept`.
    fn shown(&self, kept: bool) -> bool {
        if kept { self.shown_when_built } else { self.visible() }
    }

    /// Every requirement met (obtained frame), answered from the build when `kept`.
    fn obtained(&self, kept: bool) -> bool {
        if kept { self.obtained_when_built } else { self.percent() >= 1.0 }
    }

    /// The node's frame (26 × 26 at `x + 3`, the icon inside it) reaches inside the window, for a
    /// tab scrolled by `scroll`: outside, the clipping would drop all of it.
    fn reaches_window(&self, scroll: (i32, i32)) -> bool {
        let (left, top) = (scroll.0 + self.x + 3, scroll.1 + self.y);
        left < IW && left + 26 > 0 && top < IH && top + 26 > 0
    }
}

/// Sprite of a node's frame.
fn frame_sprite(frame: Frame, obtained: bool) -> &'static str {
    match (frame, obtained) {
        (Frame::Task, false) => "advancements/task_frame_unobtained",
        (Frame::Task, true) => "advancements/task_frame_obtained",
        (Frame::Challenge, false) => "advancements/challenge_frame_unobtained",
        (Frame::Challenge, true) => "advancements/challenge_frame_obtained",
        (Frame::Goal, false) => "advancements/goal_frame_unobtained",
        (Frame::Goal, true) => "advancements/goal_frame_obtained",
    }
}

struct Tab {
    root: String,
    index: usize,
    title: Component,
    /// Title as drawn (window title, tab tooltip).
    kept_title: Kept<Vec<StyledChar>>,
    icon: ItemStack,
    icon_cache: Option<Cached>,
    /// Background texture (path in the resources) and its atlas slot.
    background: String,
    nodes: Vec<TabNode>,
    /// Draw order of the nodes ([`tree_order`]), computed when the tab is built.
    order: Vec<usize>,
    scroll: (f64, f64),
    centered: bool,
    min: (i32, i32),
    max: (i32, i32),
}

/// Where a tab sits: position relative to the window, size, offset of its icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TabPlace {
    pos: (i32, i32),
    size: (i32, i32),
    icon: (i32, i32),
}

/// Side of the window a tab sits on (0 above, 1 below, 2 left, 3 right), its rank on that side
/// and the last rank of the side.
fn tab_side(index: usize) -> (usize, i32, i32) {
    match index {
        i @ 0..=7 => (0, i as i32, 7),
        i @ 8..=15 => (1, i as i32 - 8, 7),
        i @ 16..=20 => (2, i as i32 - 16, 4),
        i => (3, i as i32 - 21, 4),
    }
}

fn tab_place(index: usize) -> TabPlace {
    let (side, rank, _) = tab_side(index);
    let (pos, size, icon) = match side {
        0 => ((32 * rank, -28), (28, 32), (6, 9)),
        1 => ((32 * rank, 136), (28, 32), (6, 6)),
        2 => ((-28, 28 * rank), (32, 28), (10, 5)),
        _ => ((248, 28 * rank), (32, 28), (6, 5)),
    };
    TabPlace { pos, size, icon }
}

/// Tab sprites by side, part of the side (first, middle, last) and selection.
const TAB_SPRITES: [[[&str; 2]; 3]; 4] = [
    [
        ["advancements/tab_above_left", "advancements/tab_above_left_selected"],
        ["advancements/tab_above_middle", "advancements/tab_above_middle_selected"],
        ["advancements/tab_above_right", "advancements/tab_above_right_selected"],
    ],
    [
        ["advancements/tab_below_left", "advancements/tab_below_left_selected"],
        ["advancements/tab_below_middle", "advancements/tab_below_middle_selected"],
        ["advancements/tab_below_right", "advancements/tab_below_right_selected"],
    ],
    [
        ["advancements/tab_left_top", "advancements/tab_left_top_selected"],
        ["advancements/tab_left_middle", "advancements/tab_left_middle_selected"],
        ["advancements/tab_left_bottom", "advancements/tab_left_bottom_selected"],
    ],
    [
        ["advancements/tab_right_top", "advancements/tab_right_top_selected"],
        ["advancements/tab_right_middle", "advancements/tab_right_middle_selected"],
        ["advancements/tab_right_bottom", "advancements/tab_right_bottom_selected"],
    ],
];

fn tab_sprite(index: usize, selected: bool) -> &'static str {
    let (side, rank, last) = tab_side(index);
    let part = if rank == 0 {
        0
    } else if rank == last {
        2
    } else {
        1
    };
    TAB_SPRITES[side][part][usize::from(selected)]
}

/// Atlas sprite reserved for the background of the tab `slot` (names built once).
fn background_sprite(slot: usize) -> &'static str {
    static NAMES: std::sync::OnceLock<Vec<String>> = std::sync::OnceLock::new();
    &NAMES.get_or_init(|| (0..rc_assets::gui::ADV_BG_SLOTS).map(|i| format!("dyn/adv_bg/{i}")).collect())[slot]
}

/// What the atlas slot reserved for a tab holds.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BackgroundSlot {
    Empty,
    /// This texture is written in the slot.
    Written(String),
    /// This texture could not be read or written.
    Missing(String),
}

impl Tab {
    /// Scroll in whole pixels.
    fn scroll_pixels(&self) -> (i32, i32) {
        (self.scroll.0.floor() as i32, self.scroll.1.floor() as i32)
    }

    /// Title of the tab as drawn (window title, tab tooltip), flattened once when `kept`.
    fn title_text(&mut self, gui: &Gui, kept: bool) -> &Vec<StyledChar> {
        let title = &self.title;
        self.kept_title.get(gui, kept, |gui| rc_text::styled_chars(&rc_text::flatten(title, &gui.lang)))
    }

    fn scroll_by(&mut self, dx: f64, dy: f64) {
        // Clamping as the game does it: below the low bound → low, otherwise min(v, high).
        let clamp = |v: f64, lo: f64| if v < lo { lo } else { v.min(0.0) };
        if self.max.0 - self.min.0 > IW {
            self.scroll.0 = clamp(self.scroll.0 + dx, -f64::from(self.max.0 - IW));
        }
        if self.max.1 - self.min.1 > IH {
            self.scroll.1 = clamp(self.scroll.1 + dy, -f64::from(self.max.1 - IH));
        }
    }

    fn center(&mut self) {
        if !self.centered {
            self.scroll = (
                f64::from(117 - (self.max.0 + self.min.0) / 2),
                f64::from(56 - (self.max.1 + self.min.1) / 2),
            );
            self.centered = true;
        }
    }
}

pub struct AdvancementsScreen {
    tabs: Vec<Tab>,
    selected: Option<usize>,
    generation: u64,
    panel: Panel,
    size: (i32, i32),
    /// Opened from the pause menu.
    pub parent: bool,
    /// "Tab opened" / "screen closed" to send (`seen_advancements`).
    pub outgoing: Vec<Option<String>>,
    /// Node hover (index in the selected tab) and overlay.
    hovered: Option<usize>,
    fade: f32,
    /// Drag: left button held, armed, last position.
    drag: Option<(bool, (f32, f32))>,
    /// Backgrounds written into the atlas slots.
    slots: Vec<BackgroundSlot>,
    /// Font identity of the interface whose atlas the slots were written in.
    slots_interface: Option<usize>,
    /// "Advancements" title of the screen.
    screen_title: Kept<Vec<StyledChar>>,
    opts: AdvancementOpts,
    pub cursor: Cursor,
    pub clicks: u32,
}

impl AdvancementsScreen {
    pub fn new(adv: &mut Advancements, parent: bool, keyboard: bool) -> AdvancementsScreen {
        Self::with_opts(adv, parent, keyboard, AdvancementOpts::from_switches())
    }

    fn with_opts(adv: &mut Advancements, parent: bool, keyboard: bool, opts: AdvancementOpts) -> AdvancementsScreen {
        let mut panel = Panel::default();
        panel.keyboard = keyboard;
        let mut s = AdvancementsScreen {
            opts,
            slots_interface: None,
            screen_title: Kept::default(),
            tabs: Vec::new(),
            selected: None,
            generation: u64::MAX,
            panel,
            size: (-1, -1),
            parent,
            outgoing: Vec::new(),
            hovered: None,
            fade: 0.0,
            drag: None,
            slots: vec![BackgroundSlot::Empty; rc_assets::gui::ADV_BG_SLOTS],
            cursor: Cursor::Default,
            clicks: 0,
        };
        adv.reset_seen = false;
        // On opening: first tab, "tab opened" sent twice (§ 4.2).
        adv.selected_tab = None;
        s.rebuild(adv);
        if let Some(t) = s.tabs.first() {
            let root = t.root.clone();
            s.outgoing.push(Some(root.clone()));
            adv.selected_tab = Some(root);
        }
        s
    }

    /// Rebuilding the tabs (§ 4.2): scroll of the tabs with the same root kept.
    fn rebuild(&mut self, adv: &mut Advancements) {
        self.generation = adv.generation;
        if std::mem::take(&mut adv.reset_seen) {
            self.tabs.clear();
            adv.selected_tab = None;
        }
        let old: Vec<Tab> = std::mem::take(&mut self.tabs);
        let mut tabs: Vec<Tab> = Vec::new();
        // Roots with a display and a background.
        for n in adv.nodes.iter().filter(|n| n.parent.is_none()) {
            let Some(d) = &n.display else { continue };
            let Some(bg) = &d.background else { continue };
            if tabs.len() >= MAX_TABS {
                continue;
            }
            let mut t = Tab {
                root: n.id.clone(),
                index: tabs.len(),
                title: d.title.clone(),
                kept_title: Kept::default(),
                icon: d.icon.clone(),
                icon_cache: None,
                background: bg.clone(),
                nodes: Vec::new(),
                order: Vec::new(),
                scroll: (0.0, 0.0),
                centered: false,
                min: (i32::MAX, i32::MAX),
                max: (i32::MIN, i32::MIN),
            };
            if let Some(o) = old.iter().find(|o| o.root == n.id) {
                (t.scroll, t.centered) = (o.scroll, o.centered);
            }
            tabs.push(t);
        }
        let a: &Advancements = adv;
        let root_of = |id: &str| -> String {
            let mut id = id.to_owned();
            while let Some(p) = a.get(&id).and_then(|n| n.parent.clone()).filter(|p| a.get(p).is_some()) {
                id = p;
            }
            id
        };
        for n in &a.nodes {
            let Some(d) = &n.display else { continue };
            let root = root_of(&n.id);
            let Some(t) = tabs.iter_mut().find(|t| t.root == root) else { continue };
            // Drawn parent: first ancestor with a display, searched in the tab.
            let mut parent = None;
            let mut cur = n.parent.as_deref();
            while let Some(p) = cur {
                let Some(pn) = a.get(p) else { break };
                if pn.display.is_some() {
                    parent = t.nodes.iter().position(|x| x.id == p);
                    break;
                }
                cur = pn.parent.as_deref();
            }
            let (x, y) = ((n.x * 28.0).floor() as i32, (n.y * 27.0).floor() as i32);
            let k = t.nodes.len();
            if let Some(p) = parent {
                t.nodes[p].children.push(k);
            }
            t.min = (t.min.0.min(x), t.min.1.min(y));
            t.max = (t.max.0.max(x + 28), t.max.1.max(y + 27));
            let mut node = TabNode {
                id: n.id.clone(),
                parent,
                children: Vec::new(),
                x,
                y,
                frame: d.frame,
                hidden: d.hidden,
                title: d.title.clone(),
                description: d.description.clone(),
                icon: d.icon.clone(),
                progress: a.progress.get(&n.id).cloned(),
                shown_when_built: false,
                obtained_when_built: false,
                cache: None,
                layout: None,
            };
            node.shown_when_built = node.visible();
            node.obtained_when_built = node.percent() >= 1.0;
            t.nodes.push(node);
        }
        for t in &mut tabs {
            t.order = tree_order(&t.nodes);
        }
        self.tabs = tabs;
        self.hovered = None;
        let prev = adv.selected_tab.clone();
        self.selected = prev.as_ref().and_then(|r| self.tabs.iter().position(|t| &t.root == r));
        if self.selected.is_none() && !self.tabs.is_empty() {
            self.selected = Some(0);
            let root = self.tabs[0].root.clone();
            self.outgoing.push(Some(root.clone()));
            adv.selected_tab = Some(root);
        }
    }

    /// Model tracking: rebuild after an update, tab imposed by the server.
    fn sync(&mut self, adv: &mut Advancements) {
        if self.generation != adv.generation {
            self.rebuild(adv);
        }
        let want = adv.selected_tab.as_ref().and_then(|r| self.tabs.iter().position(|t| &t.root == r));
        if want != self.selected {
            self.selected = want;
            self.hovered = None;
        }
    }

    fn origin(gui: &Gui) -> (i32, i32) {
        ((gui.width as i32 - 252) / 2, (gui.height as i32 - 140) / 2)
    }

    fn layout(&mut self, gui: &Gui) {
        let size = (gui.width as i32, gui.height as i32);
        if self.size == size {
            return;
        }
        let first = self.size.0 < 0;
        self.size = size;
        let done = Widget::button(DONE, (size.0 - 200) / 2, size.1 - 26, 200, &tr(&gui.lang, "gui.done", &[]));
        let keyboard = self.panel.keyboard;
        let focus = self.panel.focus;
        self.panel.widgets = vec![done];
        if first {
            self.panel.initial_focus(keyboard);
        } else {
            self.panel.focus = focus;
        }
    }

    fn tab_at(&self, gui: &Gui, m: (f32, f32)) -> Option<usize> {
        if self.tabs.len() < 2 {
            return None;
        }
        let (g, h) = Self::origin(gui);
        self.tabs.iter().position(|t| {
            let TabPlace { pos: (x, y), size: (w, hh), .. } = tab_place(t.index);
            let (x, y) = ((g + x) as f32, (h + y) as f32);
            x < m.0 && m.0 < x + w as f32 && y < m.1 && m.1 < y + hh as f32
        })
    }

    fn select(&mut self, i: usize, adv: &mut Advancements) {
        self.selected = Some(i);
        self.hovered = None;
        let root = self.tabs[i].root.clone();
        self.outgoing.push(Some(root.clone()));
        adv.selected_tab = Some(root);
    }

    pub fn press(&mut self, gui: &Gui, adv: &mut Advancements, m: (f32, f32), button: u8) -> Option<Action> {
        self.layout(gui);
        self.sync(adv);
        let mut ev = Vec::new();
        self.panel.press(m, button, false, &mut ev);
        self.clicks += std::mem::take(&mut self.panel.clicks);
        if ev.contains(&Event::Press(DONE)) {
            return Some(Action::Back);
        }
        // Drag: the left button arms it (even over a tab: the game scrolls on any
        // left drag), the others disarm it.
        self.drag = if button == 0 { Some((false, m)) } else { None };
        if button == 0
            && let Some(i) = self.tab_at(gui, m)
        {
            self.select(i, adv);
        }
        None
    }

    pub fn release(&mut self) {
        let mut ev = Vec::new();
        self.panel.release(&mut ev);
        self.drag = None;
    }

    pub fn motion(&mut self, gui: &Gui, adv: &mut Advancements, m: (f32, f32)) {
        self.layout(gui);
        self.sync(adv);
        let mut ev = Vec::new();
        self.panel.motion(m, &mut ev);
        if let Some((armed, last)) = &mut self.drag {
            let (dx, dy) = (m.0 - last.0, m.1 - last.1);
            *last = m;
            if !*armed {
                *armed = true;
            } else if let Some(t) = self.selected.and_then(|i| self.tabs.get_mut(i)) {
                t.scroll_by(f64::from(dx), f64::from(dy));
            }
        }
    }

    pub fn wheel(&mut self, adv: &mut Advancements, dx: f64, dy: f64) {
        self.sync(adv);
        if let Some(t) = self.selected.and_then(|i| self.tabs.get_mut(i)) {
            t.scroll_by(dx * 16.0, dy * 16.0);
        }
    }

    /// Key: `advancements` = the advancements key (L); returns an exit action.
    pub fn key(&mut self, gui: &Gui, k: KeyCode, advancements: bool) -> Option<Action> {
        self.layout(gui);
        let mut ev = Vec::new();
        let used = self.panel.key(k, winit::keyboard::ModifiersState::empty(), &mut ev);
        self.clicks += std::mem::take(&mut self.panel.clicks);
        if ev.contains(&Event::Press(DONE)) || (!used && (k == KeyCode::Escape || advancements)) {
            return Some(Action::Back);
        }
        None
    }

    /// Tick (20 Hz): hover and overlay (§ 4.6).
    pub fn tick(&mut self, gui: &Gui, adv: &mut Advancements, mouse: Option<(f32, f32)>) {
        self.sync(adv);
        let (g, h) = Self::origin(gui);
        let kept = self.opts.screen_caches;
        let hit = mouse.and_then(|(mx, my)| {
            let t = self.tabs.get(self.selected?)?;
            let (rx, ry) = ((mx.floor() as i32) - g - 9, (my.floor() as i32) - h - 18);
            if !(0 < rx && rx < IW && 0 < ry && ry < IH) {
                return None;
            }
            let (sx, sy) = t.scroll_pixels();
            t.nodes.iter().position(|n| {
                n.shown(kept)
                    && sx + n.x <= rx
                    && rx <= sx + n.x + 26
                    && sy + n.y <= ry
                    && ry <= sy + n.y + 26
            })
        });
        if hit.is_some() {
            self.fade = (self.fade + 0.06).clamp(0.0, 0.3);
            self.hovered = hit;
        } else {
            self.fade = (self.fade - 0.12).clamp(0.0, 1.0);
            self.hovered = None;
        }
    }

    /// Tab backgrounds: texture read from the resources, rewritten into the slot of
    /// the atlas reserved for the tab.
    fn upload_backgrounds(&mut self, gui: &Gui, q: &wgpu::Queue, assets: &rc_assets::Assets) {
        // An interface rebuilt by a resource reload has a new atlas: its slots hold nothing yet.
        if self.slots_interface != Some(gui.font_generation()) {
            self.slots_interface = Some(gui.font_generation());
            self.slots.fill(BackgroundSlot::Empty);
        }
        for (i, t) in self.tabs.iter().enumerate() {
            match &self.slots[i] {
                BackgroundSlot::Written(b) if *b == t.background => continue,
                // A texture absent from the resources stays absent until they are reloaded: the old
                // path looked it up in the packs again on every frame.
                BackgroundSlot::Missing(b) if self.opts.screen_caches && *b == t.background => continue,
                _ => {}
            }
            let written = background_pixels(assets, &t.background)
                .is_some_and(|rgba| gui.write_sprite(q, background_sprite(i), (16, 16), &rgba));
            let background = t.background.clone();
            self.slots[i] =
                if written { BackgroundSlot::Written(background) } else { BackgroundSlot::Missing(background) };
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        gui: &mut Gui,
        q: &wgpu::Queue,
        it: &HotbarItems<'_>,
        adv: &mut Advancements,
        assets: &rc_assets::Assets,
        blur: i32,
        mouse: Option<(f32, f32)>,
        time: f64,
    ) {
        self.layout(gui);
        self.sync(adv);
        self.upload_backgrounds(gui, q, assets);
        let f = |v: i32| v as f32;
        let (w, h) = (gui.width as i32, gui.height as i32);
        // Background, header, footer.
        gui.next_layer();
        widgets::menu_background(gui, blur, true);
        gui.next_layer();
        let kept = self.opts.screen_caches;
        let title = self.screen_title.get(gui, kept, |gui| chars(&tr(&gui.lang, "gui.advancements", &[])));
        widgets::centered(gui, q, title, w / 2, 12, 0xffff_ffff);
        self.panel.draw(gui, q, mouse, time);
        let (g, hh) = Self::origin(gui);
        let (ix, iy) = (g + 9, hh + 18);
        // Inside, clipped.
        gui.next_layer();
        let hovered = self.hovered;
        if let Some(t) = self.selected.and_then(|i| self.tabs.get_mut(i)) {
            t.center();
        }
        // Icon renders (tabs, nodes of the selected tab).
        if let Some(m) = gui.items.as_ref().map(|a| &a.models) {
            for t in &mut self.tabs {
                refresh(&mut t.icon_cache, &t.icon, it, m, 0);
            }
            if let Some(t) = self.selected.and_then(|i| self.tabs.get_mut(i)) {
                let scroll = t.scroll_pixels();
                for (k, n) in t.nodes.iter_mut().enumerate() {
                    // Only the icons drawn this frame: in the window, or under the tooltip of the
                    // hovered node (drawn unclipped). The old path redoes every shown node.
                    let drawn = n.shown(kept) && (!kept || n.reaches_window(scroll) || hovered == Some(k));
                    if drawn {
                        refresh(&mut n.cache, &n.icon, it, m, 0);
                    }
                }
            }
        }
        match self.selected.and_then(|i| self.tabs.get(i).map(|t| (i, t))) {
            None => {
                gui.fill(f(ix), f(iy), f(ix + IW), f(iy + IH), 0xff00_0000);
                let empty = chars(&tr(&gui.lang, "advancements.empty", &[]));
                widgets::centered(gui, q, &empty, g + 126, iy + 56 - 4, 0xffff_ffff);
                let sad = chars(&tr(&gui.lang, "advancements.sad_label", &[]));
                widgets::centered(gui, q, &sad, g + 126, iy + IH - 9, 0xffff_ffff);
            }
            Some((slot, t)) => {
                gui.set_clip(Some([f(ix), f(iy), f(ix + IW), f(iy + IH)]));
                let scroll = t.scroll_pixels();
                draw_tab_background(gui, slot, (ix, iy), scroll, self.opts.tiled_background);
                draw_tree(gui, t, (ix, iy), scroll, self.opts);
                gui.set_clip(None);
            }
        }
        // Window, tabs, title.
        gui.next_layer();
        gui.sprite_region("bg/advancements/window", 256.0, 256.0, 0.0, 0.0, f(g), f(hh), 252.0, 140.0);
        let many = self.tabs.len() >= 2;
        if many {
            for (i, t) in self.tabs.iter().enumerate() {
                let TabPlace { pos: (x, y), size: (tw, th), .. } = tab_place(t.index);
                let sprite = tab_sprite(t.index, self.selected == Some(i));
                gui.sprite(sprite, f(g + x), f(hh + y), f(tw), f(th), 0xffff_ffff);
            }
            gui.stratum(Blend::Premultiplied);
            for t in &self.tabs {
                let TabPlace { pos: (x, y), icon: (dx, dy), .. } = tab_place(t.index);
                if let Some(c) = &t.icon_cache
                    && !c.layers.is_empty()
                {
                    gui.item(c.key, &c.layers, f(g + x + dx), f(hh + y + dy), 0.0);
                }
            }
            gui.stratum(Blend::Normal);
        }
        let window_title = match self.selected.and_then(|i| self.tabs.get_mut(i)) {
            Some(t) => t.title_text(gui, kept),
            None => title,
        };
        gui.text(q, window_title, f(g + 8), f(hh + 6), 0xff40_4040, false, 0);
        // Overlay and tooltip of the hovered node, tab tooltip.
        gui.next_layer();
        if let Some(t) = self.selected.and_then(|i| self.tabs.get_mut(i)) {
            let a = (self.fade * 255.0) as u32;
            if a > 0 {
                gui.fill(f(ix), f(iy), f(ix + IW), f(iy + IH), a << 24);
            }
            if let Some(k) = self.hovered.filter(|&k| k < t.nodes.len()) {
                let scroll = t.scroll_pixels();
                node_tooltip(gui, q, &mut t.nodes[k], (ix, iy), scroll, g, w);
            }
        }
        self.cursor = if self.drag.is_some_and(|d| d.0) {
            match self.selected.and_then(|i| self.tabs.get(i)) {
                Some(t) if t.max.0 - t.min.0 > IW && t.max.1 - t.min.1 > IH => Cursor::Move,
                Some(t) if t.max.0 - t.min.0 > IW => Cursor::MoveH,
                Some(t) if t.max.1 - t.min.1 > IH => Cursor::MoveV,
                _ => Cursor::Move,
            }
        } else {
            Cursor::Default
        };
        if let Some(m) = mouse
            && let Some(i) = self.tab_at(gui, m)
        {
            let TabPlace { pos: (x, y), size: (tw, th), .. } = tab_place(i);
            let tip = self.tabs[i].title_text(gui, kept);
            widgets::draw_tooltip(gui, q, std::slice::from_ref(tip), (g + x, hh + y, tw, th), Some(m));
            if self.selected != Some(i) && self.cursor == Cursor::Default {
                self.cursor = Cursor::Hand;
            }
        }
        let _ = h;
    }
}

/// Background texture `<ns>:textures/<path>.png` of a tab, scaled down to 16 × 16 (nearest): the
/// background is drawn as a whole 16 × 16. `None` if the resources do not have it.
fn background_pixels(assets: &rc_assets::Assets, background: &str) -> Option<Vec<u8>> {
    let (ns, path) = background.split_once(':').unwrap_or(("minecraft", background));
    let img = assets.image(&format!("assets/{ns}/{path}"))?;
    let (w, h) = (img.width.max(1), img.height.max(1));
    let mut rgba = vec![0u8; 16 * 16 * 4];
    for y in 0..16u32 {
        for x in 0..16u32 {
            let (px, py) = (x * w / 16, y * h / 16);
            let s = ((py * img.width + px) * 4) as usize;
            let d = ((y * 16 + x) * 4) as usize;
            if let Some(src) = img.rgba.get(s..s + 4) {
                rgba[d..d + 4].copy_from_slice(src);
            }
        }
    }
    Some(rgba)
}

/// Background of the selected tab over the inside of the window at `(ix, iy)`, the 16 × 16
/// pattern following the scroll. `tiled`: one repeated quad; otherwise the 17 × 10 sprites that
/// cover the window whatever the scroll.
fn draw_tab_background(gui: &mut Gui, slot: usize, (ix, iy): (i32, i32), scroll: (i32, i32), tiled: bool) {
    let f = |v: i32| v as f32;
    let (ox, oy) = (scroll.0 % 16, scroll.1 % 16);
    let sprite = background_sprite(slot);
    if tiled {
        gui.sprite_tiled(sprite, f(ix), f(iy), f(ix + IW), f(iy + IH), 16.0, 16.0, f(-ox), f(-oy));
        return;
    }
    for i in -1..=15 {
        for j in -1..=8 {
            gui.sprite(sprite, f(ix + ox + 16 * i), f(iy + oy + 16 * j), 16.0, 16.0, 0xffff_ffff);
        }
    }
}

/// Draw order: from each root of the tab (node with no linked drawn parent), parent
/// before children. Nodes not linked to the root are not drawn.
fn tree_order(nodes: &[TabNode]) -> Vec<usize> {
    let mut out = Vec::with_capacity(nodes.len());
    if nodes.is_empty() {
        return out;
    }
    let mut stack = vec![0usize];
    while let Some(k) = stack.pop() {
        out.push(k);
        stack.extend(nodes[k].children.iter().rev());
    }
    out
}

/// Links, frames and icons of the nodes of tab `t`, inside the window at `(ix, iy)` (clipped by
/// the caller), the tab scrolled by `scroll`.
fn draw_tree(gui: &mut Gui, t: &Tab, (ix, iy): (i32, i32), scroll: (i32, i32), opts: AdvancementOpts) {
    let f = |v: i32| v as f32;
    let kept = opts.screen_caches;
    let (sx, sy) = scroll;
    let order_of_this_frame;
    let order: &[usize] = if kept {
        &t.order
    } else {
        order_of_this_frame = tree_order(&t.nodes);
        &order_of_this_frame
    };
    // Links: all the black ones, then all the white ones, parent before children.
    for white in [false, true] {
        for &k in order {
            let n = &t.nodes[k];
            let Some(p) = n.parent else { continue };
            let p = &t.nodes[p];
            let (child, parent) = ((sx + n.x, sy + n.y), (sx + p.x, sy + p.y));
            // A link wholly outside the window would lose each of its rectangles to the clipping.
            if kept && !link_reaches_window(child, parent) {
                continue;
            }
            connect(gui, (ix + child.0, iy + child.1), (ix + parent.0, iy + parent.1), white, opts.merged_links);
        }
    }
    let in_window = |n: &TabNode| n.shown(kept) && (!kept || n.reaches_window(scroll));
    for &k in order {
        let n = &t.nodes[k];
        if in_window(n) {
            let sprite = frame_sprite(n.frame, n.obtained(kept));
            gui.sprite(sprite, f(ix + sx + n.x + 3), f(iy + sy + n.y), 26.0, 26.0, 0xffff_ffff);
        }
    }
    gui.stratum(Blend::Premultiplied);
    for &k in order {
        let n = &t.nodes[k];
        if in_window(n)
            && let Some(c) = &n.cache
            && !c.layers.is_empty()
        {
            gui.item(c.key, &c.layers, f(ix + sx + n.x + 8), f(iy + sy + n.y + 5), 0.0);
        }
    }
    gui.stratum(Blend::Normal);
}

/// Corners of a link (§ 4.4): it leaves the centre of the parent at `(start_x, parent_y)`, turns
/// at `turn_x` and reaches the centre of the child at `(child_x, child_y)`.
struct LinkPath {
    start_x: i32,
    turn_x: i32,
    parent_y: i32,
    child_x: i32,
    child_y: i32,
}

impl LinkPath {
    /// Link between a node at `child` and its drawn parent at `parent` (top-left corners).
    fn between(child: (i32, i32), parent: (i32, i32)) -> LinkPath {
        LinkPath {
            start_x: parent.0 + 13,
            turn_x: parent.0 + 30,
            parent_y: parent.1 + 13,
            child_x: child.0 + 13,
            child_y: child.1 + 13,
        }
    }
}

/// The bounding box of the link (outline included), in window coordinates, reaches inside the
/// window.
fn link_reaches_window(child: (i32, i32), parent: (i32, i32)) -> bool {
    let l = LinkPath::between(child, parent);
    let (left, right) = (l.start_x.min(l.child_x), (l.turn_x + 2).max(l.child_x + 1));
    let (top, bottom) = (l.parent_y.min(l.child_y) - 1, l.parent_y.max(l.child_y) + 2);
    left < IW && right > 0 && top < IH && bottom > 0
}

/// Horizontal line one pixel high from `a` to `b` included, in either order.
fn horizontal_line(gui: &mut Gui, a: i32, b: i32, y: i32, color: u32) {
    gui.fill(a.min(b) as f32, y as f32, (a.max(b) + 1) as f32, (y + 1) as f32, color);
}

/// Vertical line one pixel wide strictly between rows `a` and `b`, in either order.
fn vertical_line(gui: &mut Gui, x: i32, a: i32, b: i32, color: u32) {
    if b.max(a) - a.min(b) > 1 {
        gui.fill(x as f32, (a.min(b) + 1) as f32, (x + 1) as f32, a.max(b) as f32, color);
    }
}

/// Link between a node and its drawn parent (§ 4.4), screen coordinates: its white line, or
/// its black outline (`merged`: in 4 rectangles instead of 8 lines).
fn connect(gui: &mut Gui, child: (i32, i32), parent: (i32, i32), white: bool, merged: bool) {
    let l = LinkPath::between(child, parent);
    if white {
        let c = 0xffff_ffff;
        horizontal_line(gui, l.turn_x, l.start_x, l.parent_y, c);
        horizontal_line(gui, l.child_x, l.turn_x, l.child_y, c);
        vertical_line(gui, l.turn_x, l.child_y, l.parent_y, c);
    } else if merged {
        black_outline_merged(gui, &l);
    } else {
        black_outline_by_lines(gui, &l);
    }
}

/// Black outline of a link, one fill per one-pixel line (old path): 8 rectangles.
fn black_outline_by_lines(gui: &mut Gui, l: &LinkPath) {
    let c = 0xff00_0000;
    horizontal_line(gui, l.turn_x, l.start_x, l.parent_y - 1, c);
    horizontal_line(gui, l.turn_x + 1, l.start_x, l.parent_y, c);
    horizontal_line(gui, l.turn_x, l.start_x, l.parent_y + 1, c);
    horizontal_line(gui, l.child_x, l.turn_x - 1, l.child_y - 1, c);
    horizontal_line(gui, l.child_x, l.turn_x - 1, l.child_y, c);
    horizontal_line(gui, l.child_x, l.turn_x - 1, l.child_y + 1, c);
    vertical_line(gui, l.turn_x - 1, l.child_y, l.parent_y, c);
    vertical_line(gui, l.turn_x + 1, l.child_y, l.parent_y, c);
}

/// Black outline of a link in 4 opaque rectangles that cover the pixels of the 8 lines of
/// [`black_outline_by_lines`], plus the column between the two vertical lines: that column is the
/// link's own white line, drawn after every outline, so the image is the same.
fn black_outline_merged(gui: &mut Gui, l: &LinkPath) {
    let c = 0xff00_0000;
    let f = |v: i32| v as f32;
    // Parent side: three rows up to the turn, and the pixel by which the middle row is longer.
    gui.fill(f(l.start_x), f(l.parent_y - 1), f(l.turn_x + 1), f(l.parent_y + 2), c);
    gui.fill(f(l.turn_x + 1), f(l.parent_y), f(l.turn_x + 2), f(l.parent_y + 1), c);
    // Child side: three rows of the same extent.
    let (left, right) = (l.child_x.min(l.turn_x - 1), l.child_x.max(l.turn_x - 1));
    gui.fill(f(left), f(l.child_y - 1), f(right + 1), f(l.child_y + 2), c);
    // Vertical part: three columns strictly between the two rows.
    let (top, bottom) = (l.child_y.min(l.parent_y), l.child_y.max(l.parent_y));
    if bottom - top > 1 {
        gui.fill(f(l.turn_x - 1), f(top + 1), f(l.turn_x + 2), f(bottom), c);
    }
}

/// Tooltip lines and width (§ 4.6), computed on creation.
fn tooltip_layout(gui: &Gui, n: &TabNode) -> Layout {
    let title = rc_text::styled_chars(&rc_text::flatten(&n.title, &gui.lang));
    let lines = rc_text::wrap(gui, &title, 163.0);
    let widest = |ls: &[Vec<StyledChar>]| ls.iter().map(|l| rc_text::width(gui, l)).max().unwrap_or(0);
    let lt = widest(&lines).max(80);
    let groups = n.progress.as_ref().map_or(0, |p| p.requirements.len());
    let lp = if groups > 1 {
        let s = tr(&gui.lang, "advancements.progress", &[groups.to_string(), groups.to_string()]);
        rc_text::width(gui, &chars(&s)) + 8
    } else {
        0
    };
    let l = 29 + lt + lp;
    let mut colored = Component::text("");
    colored.style.color = Some(n.frame.description_color());
    colored.extra.push(n.description.clone());
    let desc = rc_text::styled_chars(&rc_text::flatten(&colored, &gui.lang));
    let mut best: Option<(i32, Vec<Vec<StyledChar>>)> = None;
    for delta in [0, 10, -10, 25, -25] {
        let ls = rc_text::wrap(gui, &desc, (l - delta) as f32);
        let gap = (widest(&ls) - l).abs();
        if gap <= 10 {
            best = Some((gap, ls));
            break;
        }
        if best.as_ref().is_none_or(|b| gap < b.0) {
            best = Some((gap, ls));
        }
    }
    let desc_lines = best.map(|b| b.1).unwrap_or_default();
    let desc_lines: Vec<Vec<StyledChar>> = if desc.is_empty() { Vec::new() } else { desc_lines };
    let l = l.max(widest(&desc_lines));
    (lines, desc_lines, l + 8)
}

#[allow(clippy::too_many_arguments)]
fn node_tooltip(
    gui: &mut Gui,
    q: &wgpu::Queue,
    n: &mut TabNode,
    (ix, iy): (i32, i32),
    (sx, sy): (i32, i32),
    g: i32,
    screen_w: i32,
) {
    if n.layout.is_none() {
        n.layout = Some(tooltip_layout(gui, n));
    }
    let n = &*n;
    let Some((title, desc, w)) = n.layout.as_ref() else { return };
    let w = *w;
    let f = |v: i32| v as f32;
    let (nl, d) = (title.len() as i32, desc.len() as i32);
    let ht = 9 * nl + 17;
    let top = sy + n.y + (26 - ht) / 2;
    let bot = top + ht;
    let hd = 9 * d + 6;
    let left = g + sx + n.x + w + 26 >= screen_w;
    let above = bot + hd >= IH;
    let gt = if left { sx + n.x - w + 32 } else { sx + n.x };
    // Inner frame → screen.
    let (ax, ay) = (ix, iy);
    if d > 0 {
        let y = if above { bot - (ht + hd) } else { top };
        gui.sprite("advancements/title_box", f(ax + gt), f(ay + y), f(w), f(ht + hd), 0xffff_ffff);
    }
    let p = n.percent();
    let a = (p * w as f32).floor() as i32;
    let (bar, frame_ok): (Option<bool>, bool) = if p >= 1.0 {
        (Some(true), true)
    } else if a < 2 {
        (Some(false), false)
    } else if a > w - 2 {
        (Some(true), false)
    } else {
        (None, false)
    };
    match bar {
        Some(ok) => {
            let s = if ok { "advancements/box_obtained" } else { "advancements/box_unobtained" };
            gui.sprite(s, f(ax + gt), f(ay + top), f(w), f(ht), 0xffff_ffff);
        }
        None => {
            // Sprite drawn at 200 × hT, only a window of it is shown.
            let old = gui.clip();
            let inter = |r: [f32; 4]| match old {
                Some(c) => [c[0].max(r[0]), c[1].max(r[1]), c[2].min(r[2]), c[3].min(r[3])],
                None => r,
            };
            let (x0, y0) = (ax + gt, ay + top);
            gui.set_clip(Some(inter([f(x0), f(y0), f(x0 + a), f(y0 + ht)])));
            gui.sprite("advancements/box_obtained", f(x0), f(y0), 200.0, f(ht), 0xffff_ffff);
            gui.set_clip(Some(inter([f(x0 + a), f(y0), f(x0 + w), f(y0 + ht)])));
            gui.sprite("advancements/box_unobtained", f(x0 + w - 200), f(y0), 200.0, f(ht), 0xffff_ffff);
            gui.set_clip(old);
        }
    }
    let frame = frame_sprite(n.frame, frame_ok);
    gui.sprite(frame, f(ax + sx + n.x + 3), f(ay + sy + n.y), 26.0, 26.0, 0xffff_ffff);
    let tx = if left { gt + 5 } else { sx + n.x + 32 };
    for (k, l) in title.iter().enumerate() {
        gui.text(q, l, f(ax + tx), f(ay + top + 9 + 9 * k as i32), 0xffff_ffff, true, 0);
    }
    if let Some(pt) = n.progress.as_ref().and_then(Progress::progress_text) {
        let s = chars(&tr(&gui.lang, "advancements.progress", &split2(&pt)));
        let lp = rc_text::width(gui, &s);
        let px = if left { sx + n.x - lp } else { sx + n.x + w - lp - 5 };
        gui.text(q, &s, f(ax + px), f(ay + top + 9), 0xffff_ffff, true, 0);
    }
    let dy0 = if above { top - 9 * d + 1 } else { bot };
    for (k, l) in desc.iter().enumerate() {
        gui.text(q, l, f(ax + gt + 5), f(ay + dy0 + 9 * k as i32), 0xffff_ffff, true, 0);
    }
    if let Some(c) = &n.cache
        && !c.layers.is_empty()
    {
        gui.stratum(Blend::Premultiplied);
        gui.item(c.key, &c.layers, f(ax + sx + n.x + 8), f(ay + sy + n.y + 5), 0.0);
        gui.stratum(Blend::Normal);
    }
}

/// "x/y" → arguments of `advancements.progress`.
fn split2(s: &str) -> [String; 2] {
    let (a, b) = s.split_once('/').unwrap_or((s, ""));
    [a.to_owned(), b.to_owned()]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_geometry_by_index() {
        let mk = |index| Tab {
            root: String::new(),
            index,
            title: Component::text(""),
            kept_title: Kept::default(),
            icon: ItemStack { item: 0, count: 1, added: Vec::new(), removed: Vec::new() },
            icon_cache: None,
            background: String::new(),
            nodes: Vec::new(),
            order: Vec::new(),
            scroll: (0.0, 0.0),
            centered: false,
            min: (0, 0),
            max: (500, 50),
        };
        assert_eq!(tab_sprite(0, true), "advancements/tab_above_left_selected");
        assert_eq!(tab_place(7).pos, (224, -28));
        assert_eq!(tab_sprite(7, false), "advancements/tab_above_right");
        assert_eq!(tab_place(9).pos, (32, 136));
        assert_eq!(tab_sprite(20, false), "advancements/tab_left_bottom");
        assert_eq!(tab_place(21).pos, (248, 0));
        assert_eq!(tab_place(17), TabPlace { pos: (-28, 28), size: (32, 28), icon: (10, 5) });
        // Scrolling: horizontal only (500 > 234), clamped to [−266, 0].
        let mut t = mk(0);
        t.scroll_by(-1000.0, -1000.0);
        assert_eq!(t.scroll, (-266.0, 0.0));
        t.scroll_by(5000.0, 0.0);
        assert_eq!(t.scroll, (0.0, 0.0));
    }

    /// The sprite names kept in tables are the ones the screen used to build with a format on
    /// every frame: tabs (side, part of the side, selection), node frames, background slots.
    #[test]
    fn static_sprite_names_match_the_formatted_ones() {
        for index in 0..MAX_TABS {
            let (side, rank, last) = tab_side(index);
            let (name, parts) = match side {
                0 => ("above", ["left", "middle", "right"]),
                1 => ("below", ["left", "middle", "right"]),
                2 => ("left", ["top", "middle", "bottom"]),
                _ => ("right", ["top", "middle", "bottom"]),
            };
            let part = parts[if rank == 0 { 0 } else if rank == last { 2 } else { 1 }];
            for (selected, suffix) in [(false, ""), (true, "_selected")] {
                assert_eq!(tab_sprite(index, selected), format!("advancements/tab_{name}_{part}{suffix}"), "tab {index}");
            }
        }
        for frame in [Frame::Task, Frame::Challenge, Frame::Goal] {
            for (obtained, state) in [(false, "unobtained"), (true, "obtained")] {
                assert_eq!(frame_sprite(frame, obtained), format!("advancements/{}_frame_{state}", frame.name()));
            }
        }
        assert_eq!(background_sprite(0), "dyn/adv_bg/0");
        assert_eq!(background_sprite(MAX_TABS - 1), format!("dyn/adv_bg/{}", MAX_TABS - 1));
    }
}
