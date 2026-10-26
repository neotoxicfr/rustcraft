//! Advancements screen (`docs/specs/advancements-stats.md` § 4): tabs, tree, scrolling, hover
//! overlay, detailed tooltip, tiled background.

use rc_proto_v777::item::ItemStack;
use rc_text::{Component, StyledChar};
use winit::keyboard::KeyCode;

use crate::advancements::{Advancements, Frame, Progress};
use crate::gui::{Blend, Gui};
use crate::hud::HotbarItems;
use crate::screens::{Cached, refresh};
use crate::widgets::{self, Event, Panel, Widget, chars, tr};

const DONE: u32 = 1;
/// Inside of the window.
const IW: i32 = 234;
const IH: i32 = 113;
const MAX_TABS: usize = 26;

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
/// Tab: position relative to the window, size, icon offset, sprite.
type Geometry = ((i32, i32), (i32, i32), (i32, i32), String);

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
}

struct Tab {
    root: String,
    index: usize,
    title: Component,
    icon: ItemStack,
    icon_cache: Option<Cached>,
    /// Background texture (path in the resources) and its atlas slot.
    background: String,
    nodes: Vec<TabNode>,
    scroll: (f64, f64),
    centered: bool,
    min: (i32, i32),
    max: (i32, i32),
}

impl Tab {
    /// Position of the tab relative to the window, size, icon offset, sprite.
    fn geometry(&self, selected: bool) -> Geometry {
        let (side, li, last) = match self.index {
            i @ 0..=7 => (0, i as i32, 7),
            i @ 8..=15 => (1, i as i32 - 8, 7),
            i @ 16..=20 => (2, i as i32 - 16, 4),
            i => (3, i as i32 - 21, 4),
        };
        let pos = if li == 0 {
            0
        } else if li == last {
            2
        } else {
            1
        };
        let (p, size, icon, name, parts) = match side {
            0 => ((32 * li, -28), (28, 32), (6, 9), "above", ["left", "middle", "right"]),
            1 => ((32 * li, 136), (28, 32), (6, 6), "below", ["left", "middle", "right"]),
            2 => ((-28, 28 * li), (32, 28), (10, 5), "left", ["top", "middle", "bottom"]),
            _ => ((248, 28 * li), (32, 28), (6, 5), "right", ["top", "middle", "bottom"]),
        };
        let sel = if selected { "_selected" } else { "" };
        (p, size, icon, format!("advancements/tab_{name}_{}{sel}", parts[pos]))
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
    slots: Vec<Option<String>>,
    pub cursor: Cursor,
    pub clicks: u32,
}

impl AdvancementsScreen {
    pub fn new(adv: &mut Advancements, parent: bool, keyboard: bool) -> AdvancementsScreen {
        let mut panel = Panel::default();
        panel.keyboard = keyboard;
        let mut s = AdvancementsScreen {
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
            slots: vec![None; rc_assets::gui::ADV_BG_SLOTS],
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
                icon: d.icon.clone(),
                icon_cache: None,
                background: bg.clone(),
                nodes: Vec::new(),
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
            t.nodes.push(TabNode {
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
                cache: None,
                layout: None,
            });
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
            let ((x, y), (w, hh), _, _) = t.geometry(false);
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
        let hit = mouse.and_then(|(mx, my)| {
            let t = self.tabs.get(self.selected?)?;
            let (rx, ry) = ((mx.floor() as i32) - g - 9, (my.floor() as i32) - h - 18);
            if !(0 < rx && rx < IW && 0 < ry && ry < IH) {
                return None;
            }
            let (sx, sy) = (t.scroll.0.floor() as i32, t.scroll.1.floor() as i32);
            t.nodes.iter().position(|n| {
                n.visible()
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
        for (i, t) in self.tabs.iter().enumerate() {
            if self.slots[i].as_deref() == Some(&t.background) {
                continue;
            }
            let (ns, path) = t.background.split_once(':').unwrap_or(("minecraft", &t.background));
            let Some(img) = assets.image(&format!("assets/{ns}/{path}")) else { continue };
            // Scaled down to 16 × 16 (nearest): the background is drawn as a whole 16 × 16.
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
            if gui.write_sprite(q, &format!("dyn/adv_bg/{i}"), (16, 16), &rgba) {
                self.slots[i] = Some(t.background.clone());
            }
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
        let title = chars(&tr(&gui.lang, "gui.advancements", &[]));
        widgets::centered(gui, q, &title, w / 2, 12, 0xffff_ffff);
        self.panel.draw(gui, q, mouse, time);
        let (g, hh) = Self::origin(gui);
        let (ix, iy) = (g + 9, hh + 18);
        // Inside, clipped.
        gui.next_layer();
        // Icon renders (tabs, nodes of the selected tab).
        if let Some(m) = gui.items.as_ref().map(|a| &a.models) {
            for t in &mut self.tabs {
                refresh(&mut t.icon_cache, &t.icon, it, m, 0);
            }
            if let Some(t) = self.selected.and_then(|i| self.tabs.get_mut(i)) {
                for n in t.nodes.iter_mut().filter(|n| n.visible()) {
                    refresh(&mut n.cache, &n.icon, it, m, 0);
                }
            }
        }
        match self.selected.and_then(|i| self.tabs.get_mut(i).map(|t| (i, t))) {
            None => {
                gui.fill(f(ix), f(iy), f(ix + IW), f(iy + IH), 0xff00_0000);
                let empty = chars(&tr(&gui.lang, "advancements.empty", &[]));
                widgets::centered(gui, q, &empty, g + 126, iy + 56 - 4, 0xffff_ffff);
                let sad = chars(&tr(&gui.lang, "advancements.sad_label", &[]));
                widgets::centered(gui, q, &sad, g + 126, iy + IH - 9, 0xffff_ffff);
            }
            Some((slot, t)) => {
                t.center();
                gui.set_clip(Some([f(ix), f(iy), f(ix + IW), f(iy + IH)]));
                let (sx, sy) = (t.scroll.0.floor() as i32, t.scroll.1.floor() as i32);
                let (ox, oy) = (sx % 16, sy % 16);
                let bg = format!("dyn/adv_bg/{slot}");
                for i in -1..=15 {
                    for j in -1..=8 {
                        gui.sprite(&bg, f(ix + ox + 16 * i), f(iy + oy + 16 * j), 16.0, 16.0, 0xffff_ffff);
                    }
                }
                // Links: all the black ones, then all the white ones, parent before children.
                let order = tree_order(&t.nodes);
                for white in [false, true] {
                    for &k in &order {
                        let n = &t.nodes[k];
                        let Some(p) = n.parent else { continue };
                        let p = &t.nodes[p];
                        connect(gui, (ix + sx + n.x, iy + sy + n.y), (ix + sx + p.x, iy + sy + p.y), white);
                    }
                }
                for &k in &order {
                    let n = &t.nodes[k];
                    if !n.visible() {
                        continue;
                    }
                    let state = if n.percent() >= 1.0 { "obtained" } else { "unobtained" };
                    let s = format!("advancements/{}_frame_{state}", n.frame.name());
                    gui.sprite(&s, f(ix + sx + n.x + 3), f(iy + sy + n.y), 26.0, 26.0, 0xffff_ffff);
                }
                gui.stratum(Blend::Premultiplied);
                for &k in &order {
                    let n = &t.nodes[k];
                    if !n.visible() {
                        continue;
                    }
                    if let Some(c) = &n.cache
                        && !c.layers.is_empty()
                    {
                        gui.item(c.key, &c.layers, f(ix + sx + n.x + 8), f(iy + sy + n.y + 5), 0.0);
                    }
                }
                gui.stratum(Blend::Normal);
                gui.set_clip(None);
            }
        }
        // Window, tabs, title.
        gui.next_layer();
        gui.sprite_region("bg/advancements/window", 256.0, 256.0, 0.0, 0.0, f(g), f(hh), 252.0, 140.0);
        let many = self.tabs.len() >= 2;
        if many {
            for (i, t) in self.tabs.iter().enumerate() {
                let ((x, y), (tw, th), _, s) = t.geometry(self.selected == Some(i));
                gui.sprite(&s, f(g + x), f(hh + y), f(tw), f(th), 0xffff_ffff);
            }
            gui.stratum(Blend::Premultiplied);
            for t in &self.tabs {
                let ((x, y), _, (dx, dy), _) = t.geometry(false);
                if let Some(c) = &t.icon_cache
                    && !c.layers.is_empty()
                {
                    gui.item(c.key, &c.layers, f(g + x + dx), f(hh + y + dy), 0.0);
                }
            }
            gui.stratum(Blend::Normal);
        }
        let wt = match self.selected.and_then(|i| self.tabs.get(i)) {
            Some(t) => rc_text::styled_chars(&rc_text::flatten(&t.title, &gui.lang)),
            None => title,
        };
        gui.text(q, &wt, f(g + 8), f(hh + 6), 0xff40_4040, false, 0);
        // Overlay and tooltip of the hovered node, tab tooltip.
        gui.next_layer();
        if let Some(t) = self.selected.and_then(|i| self.tabs.get_mut(i)) {
            let a = (self.fade * 255.0) as u32;
            if a > 0 {
                gui.fill(f(ix), f(iy), f(ix + IW), f(iy + IH), a << 24);
            }
            if let Some(k) = self.hovered.filter(|&k| k < t.nodes.len()) {
                let (sx, sy) = (t.scroll.0.floor() as i32, t.scroll.1.floor() as i32);
                let n = &mut t.nodes[k];
                node_tooltip(gui, q, n, (ix, iy), (sx, sy), g, w);
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
            let t = &self.tabs[i];
            let tip = rc_text::styled_chars(&rc_text::flatten(&t.title, &gui.lang));
            let ((x, y), (tw, th), _, _) = t.geometry(false);
            widgets::draw_tooltip(gui, q, &[tip], (g + x, hh + y, tw, th), Some(m));
            if self.selected != Some(i) && self.cursor == Cursor::Default {
                self.cursor = Cursor::Hand;
            }
        }
        let _ = h;
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

/// Link between a node and its drawn parent (§ 4.4), screen coordinates.
fn connect(gui: &mut Gui, (cx, cy): (i32, i32), (px, py): (i32, i32), white: bool) {
    let (dx, sx, dy, mx, my) = (px + 13, px + 30, py + 13, cx + 13, cy + 13);
    let hl = |gui: &mut Gui, a: i32, b: i32, y: i32, c: u32| {
        gui.fill(a.min(b) as f32, y as f32, (a.max(b) + 1) as f32, (y + 1) as f32, c);
    };
    let vl = |gui: &mut Gui, x: i32, a: i32, b: i32, c: u32| {
        if b.max(a) - a.min(b) > 1 {
            gui.fill(x as f32, (a.min(b) + 1) as f32, (x + 1) as f32, a.max(b) as f32, c);
        }
    };
    if white {
        let c = 0xffff_ffff;
        hl(gui, sx, dx, dy, c);
        hl(gui, mx, sx, my, c);
        vl(gui, sx, my, dy, c);
    } else {
        let c = 0xff00_0000;
        hl(gui, sx, dx, dy - 1, c);
        hl(gui, sx + 1, dx, dy, c);
        hl(gui, sx, dx, dy + 1, c);
        hl(gui, mx, sx - 1, my - 1, c);
        hl(gui, mx, sx - 1, my, c);
        hl(gui, mx, sx - 1, my + 1, c);
        vl(gui, sx - 1, my, dy, c);
        vl(gui, sx + 1, my, dy, c);
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
    let Some((title, desc, w)) = n.layout.clone() else { return };
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
    let state = if frame_ok { "obtained" } else { "unobtained" };
    let s = format!("advancements/{}_frame_{state}", n.frame.name());
    gui.sprite(&s, f(ax + sx + n.x + 3), f(ay + sy + n.y), 26.0, 26.0, 0xffff_ffff);
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
            icon: ItemStack { item: 0, count: 1, added: Vec::new(), removed: Vec::new() },
            icon_cache: None,
            background: String::new(),
            nodes: Vec::new(),
            scroll: (0.0, 0.0),
            centered: false,
            min: (0, 0),
            max: (500, 50),
        };
        assert_eq!(mk(0).geometry(true).3, "advancements/tab_above_left_selected");
        assert_eq!(mk(7).geometry(false).0, (224, -28));
        assert_eq!(mk(7).geometry(false).3, "advancements/tab_above_right");
        assert_eq!(mk(9).geometry(false).0, (32, 136));
        assert_eq!(mk(20).geometry(false).3, "advancements/tab_left_bottom");
        assert_eq!(mk(21).geometry(false).0, (248, 0));
        // Scrolling: horizontal only (500 > 234), clamped to [−266, 0].
        let mut t = mk(0);
        t.scroll_by(-1000.0, -1000.0);
        assert_eq!(t.scroll, (-266.0, 0.0));
        t.scroll_by(5000.0, 0.0);
        assert_eq!(t.scroll, (0.0, 0.0));
    }
}
