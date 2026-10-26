//! "Resource Packs" screen of the options (`docs/specs/resource-packs.md` § 4,
//! `option-menus.md` § 3.13): available and selected packs of the `resourcepacks` folder,
//! order, search, confirmation of incompatible packs, watched folder, file drop;
//! "Done" rewrites `options.txt` and reloads the resources if the list changed.

use hashbrown::HashMap;
use std::path::PathBuf;
use std::time::{Instant, SystemTime};

use rc_assets::packs::Compat;
use rc_text::{Component, StyledChar};
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::keyboard::KeyCode;

use crate::App;
use crate::gui::Gui;
use crate::options::{Options, Val};
use crate::pack_stack::{self, PackEntry};
use crate::server_list::Column;
use crate::widgets::{self, Event, Kind, Panel, TextField, Widget, chars, tr};

const SEARCH: u32 = 1;
const OPEN_FOLDER: u32 = 2;
const DONE: u32 = 3;
const YES: u32 = 10;
const NO: u32 = 11;

const HEADER: i32 = 49;
const FOOTER: i32 = 33;
const LIST_W: i32 = 200;
const ENTRY_H: i32 = 36;
const TITLE_H: i32 = 13;
const DESC_W: f32 = 157.0;

/// List: 0 available, 1 selected.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Side {
    Available,
    Selected,
}

/// Confirmation in progress: selection of an incompatible pack, or copy of dropped files.
enum Confirm {
    Incompatible(String, Column),
    Drop(Vec<PathBuf>, Column),
    /// Warning (symbolic link refused) or alert (files that are not packs):
    /// "Back" closes it.
    Notice(Column),
}

/// Displayed pack: known entry, or the server's active pack (fixed, at the head of the selected ones).
#[derive(Clone)]
struct Shown {
    entry: PackEntry,
    /// Server pack: neither removed nor moved.
    fixed: bool,
    /// Decorated description (source), in gray.
    desc: Component,
}

pub(crate) struct PackScreen {
    /// Available (display order) and selected (top = highest priority).
    available: Vec<String>,
    selected: Vec<String>,
    /// Player's choice on opening (bottom to top), to know whether a reload is needed.
    initial: Vec<String>,
    known: HashMap<String, Shown>,
    panel: Panel,
    size: (i32, i32),
    scroll: [f32; 2],
    /// Entry chosen by keyboard or click (list, id).
    focus: Option<(Side, String)>,
    /// Icons: slot `dyn/pack_icon/<i>` per pack (`None`: unknown icon); to write.
    icons: HashMap<String, Option<usize>>,
    uploads: Vec<(usize, Vec<u8>)>,
    confirm: Option<Confirm>,
    /// Folder watching: state reread at most once per second.
    watch: (Instant, Vec<(String, u64, Option<SystemTime>)>),
    reload_in: Option<Instant>,
    inworld: bool,
    search_text: String,
}

fn snapshot() -> Vec<(String, u64, Option<SystemTime>)> {
    let Ok(rd) = std::fs::read_dir(pack_stack::pack_dir()) else { return Vec::new() };
    let mut v: Vec<_> = rd
        .flatten()
        .map(|e| {
            let m = e.metadata().ok();
            (
                e.file_name().to_string_lossy().into_owned(),
                m.as_ref().map_or(0, |m| m.len()),
                m.and_then(|m| m.modified().ok()),
            )
        })
        .collect();
    v.sort();
    v
}

/// Texts of an entry of width `max`: name cut with "...", description on two lines.
fn entry_text(
    m: &dyn rc_text::Metrics,
    name: &[StyledChar],
    desc: &[StyledChar],
    max: f32,
) -> (Vec<StyledChar>, Vec<Vec<StyledChar>>) {
    (rc_text::clamp(m, name, max as i32), desc_lines(m, desc, max))
}

/// Description on two lines at most, like the game: if some remains, the second loses the width
/// of "..." at its end (measured on itself, not on the allowed width) and ends with "...".
fn desc_lines(m: &dyn rc_text::Metrics, desc: &[StyledChar], max: f32) -> Vec<Vec<StyledChar>> {
    let mut lines = rc_text::wrap(m, desc, max);
    if lines.len() > 2 {
        lines.truncate(2);
        let dots = chars("...");
        let last = &mut lines[1];
        let n = rc_text::fit(m, last, rc_text::width(m, last) - rc_text::width(m, &dots));
        last.truncate(n);
        last.extend(dots);
    }
    lines
}

fn styled(c: &Component, lang: &rc_text::Lang) -> Vec<StyledChar> {
    rc_text::styled_chars(&rc_text::flatten(c, lang))
}

fn plain(c: &Component, lang: &rc_text::Lang) -> String {
    rc_text::plain_string(c, lang)
}

fn gray(mut c: Component) -> Component {
    if c.style.color.is_none() {
        c.style.color = Some(0x808080);
    }
    c
}

/// 64 × 64 image (nearest neighbor) of a `pack.png`.
fn icon_rgba(png: &[u8]) -> Option<Vec<u8>> {
    let img = rc_assets::texture::Image::decode_png(png)?;
    if img.width == 0 || img.height == 0 {
        return None;
    }
    let mut out = vec![0u8; 64 * 64 * 4];
    for y in 0..64u32 {
        for x in 0..64u32 {
            let sx = (x * img.width / 64) as usize;
            let sy = (y * img.height / 64) as usize;
            let s = (sy * img.width as usize + sx) * 4;
            let d = ((y * 64 + x) * 4) as usize;
            out[d..d + 4].copy_from_slice(&img.rgba[s..s + 4]);
        }
    }
    Some(out)
}

impl PackScreen {
    fn new(stack: &pack_stack::PackStack, inworld: bool) -> PackScreen {
        let mut s = PackScreen {
            available: Vec::new(),
            selected: Vec::new(),
            initial: stack.selected.clone(),
            known: HashMap::new(),
            panel: Panel::default(),
            size: (-1, -1),
            scroll: [0.0; 2],
            focus: None,
            icons: HashMap::new(),
            uploads: Vec::new(),
            confirm: None,
            watch: (Instant::now(), snapshot()),
            reload_in: None,
            inworld,
            search_text: String::new(),
        };
        s.fill(stack, stack.selected.iter().rev().cloned().collect());
        s
    }

    /// Lists from the known packs: `selected` (display order) kept if it still exists.
    fn fill(&mut self, stack: &pack_stack::PackStack, selected: Vec<String>) {
        self.known.clear();
        for p in &stack.known {
            let builtin = !p.id.starts_with("file/");
            let desc = if builtin {
                gray(Component::translate(
                    "pack.nameAndSource",
                    vec![
                        rc_text::Arg::Component(p.description.clone()),
                        rc_text::Arg::Component(Component::translate("pack.source.builtin", vec![])),
                    ],
                ))
            } else {
                gray(p.description.clone())
            };
            self.known.insert(p.id.clone(), Shown { entry: p.clone(), fixed: false, desc });
        }
        // Active server packs: at the head, fixed (last arrived on top).
        let mut server = Vec::new();
        for (i, path) in stack.server.iter().enumerate().rev() {
            let id = format!("server/{i:08X}");
            let entry = PackEntry {
                id: id.clone(),
                title: Component::translate("resourcePack.server.name", vec![]),
                description: Component::text(""),
                compat: Compat::Compatible,
                path: path.clone(),
                required: true,
            };
            let meta = rc_assets::packs::read_root(path, "pack.mcmeta").and_then(|b| rc_assets::packs::parse_meta(&b));
            let d = meta
                .and_then(|m| rc_client::status::json_component(&m.description))
                .unwrap_or_else(|| Component::text(""));
            let desc = gray(Component::translate(
                "pack.nameAndSource",
                vec![
                    rc_text::Arg::Component(d),
                    rc_text::Arg::Component(Component::translate(
                        if stack.world { "pack.source.world" } else { "pack.source.server" },
                        vec![],
                    )),
                ],
            ));
            self.known.insert(id.clone(), Shown { entry, fixed: true, desc });
            server.push(id);
        }
        self.selected = server;
        self.selected
            .extend(selected.into_iter().filter(|id| self.known.contains_key(id) && !id.starts_with("server/")));
        let mut avail: Vec<String> = stack.known.iter().map(|p| p.id.clone()).collect();
        avail.retain(|id| !self.selected.contains(id));
        self.available = avail;
        if let Some((_, id)) = &self.focus
            && !self.known.contains_key(id)
        {
            self.focus = None;
        }
    }

    fn shown(&self, id: &str) -> Option<&Shown> {
        self.known.get(id)
    }

    fn list(&self, side: Side) -> &Vec<String> {
        match side {
            Side::Available => &self.available,
            Side::Selected => &self.selected,
        }
    }

    /// Visible entries (search) of a list.
    fn visible(&self, side: Side, lang: &rc_text::Lang) -> Vec<String> {
        let needle = self.search_text.to_lowercase();
        self.list(side)
            .iter()
            .filter(|id| {
                needle.trim().is_empty()
                    || id.to_lowercase().contains(&needle)
                    || self.shown(id).is_some_and(|s| {
                        plain(&s.entry.title, lang).to_lowercase().contains(&needle)
                            || plain(&s.entry.description, lang).to_lowercase().contains(&needle)
                    })
            })
            .cloned()
            .collect()
    }

    fn list_x(&self, side: Side) -> i32 {
        match side {
            Side::Available => self.size.0 / 2 - 15 - LIST_W,
            Side::Selected => self.size.0 / 2 + 15,
        }
    }

    fn list_h(&self) -> i32 {
        self.size.1 - HEADER - FOOTER
    }

    fn max_scroll(&self, n: usize) -> f32 {
        (4 + TITLE_H + n as i32 * ENTRY_H + 4 - self.list_h()).max(0) as f32
    }

    /// Top of the content of entry `i` (icon), on screen.
    fn entry_y(&self, side: Side, i: usize) -> i32 {
        let k = if side == Side::Available { 0 } else { 1 };
        HEADER + 4 + TITLE_H - self.scroll[k] as i32 + i as i32 * ENTRY_H + 2
    }

    fn layout(&mut self, gui: &Gui) {
        let size = (gui.width as i32, gui.height as i32);
        if self.size == size {
            return;
        }
        let first = self.size.0 < 0;
        self.size = size;
        let (w, h) = size;
        let t = |k: &str| tr(&gui.lang, k, &[]);
        let mut hint = chars(&t("gui.packSelection.search"));
        for c in &mut hint {
            c.style.italic = true;
            // Game search hint: gray (0xAAAAAA) italic.
            c.style.color = Some(0xAA_AAAA);
        }
        let mut field = TextField::new(hint);
        field.text = self.search_text.chars().collect();
        field.cursor = field.text.len();
        field.anchor = field.cursor;
        let x0 = (w - 308) / 2;
        let widgets = vec![
            Widget::new(SEARCH, Kind::Field(Box::new(field)), w / 2 - 100, 30, 200, 15, Vec::new()),
            Widget::button(OPEN_FOLDER, x0, h - 26, 150, &t("pack.openFolder"))
                .with_tooltip(gui, &t("pack.folderInfo")),
            Widget::button(DONE, x0 + 158, h - 26, 150, &t("gui.done")),
        ];
        let keyboard = self.panel.keyboard;
        self.panel = Panel::new(widgets);
        self.panel.keyboard = keyboard;
        if first {
            self.panel.focus_widget(SEARCH);
        }
        if let Some(Confirm::Incompatible(_, c) | Confirm::Drop(_, c) | Confirm::Notice(c)) = &mut self.confirm {
            c.layout(gui);
        }
    }

    /// Selection (toward the top of the selected ones) or removal (toward the top of the available ones).
    fn toggle(&mut self, id: &str) {
        if let Some(i) = self.selected.iter().position(|x| x == id) {
            if self.shown(id).is_some_and(|s| s.entry.required || s.fixed) {
                return;
            }
            self.selected.remove(i);
            self.available.insert(0, id.to_owned());
            self.focus = Some((Side::Available, id.to_owned()));
        } else if let Some(i) = self.available.iter().position(|x| x == id) {
            self.available.remove(i);
            // Under the server's fixed packs.
            let at = self.selected.iter().take_while(|x| self.known.get(*x).is_some_and(|s| s.fixed)).count();
            self.selected.insert(at, id.to_owned());
            self.focus = Some((Side::Selected, id.to_owned()));
        }
    }

    fn can_move(&self, id: &str, dir: i32) -> bool {
        let Some(i) = self.selected.iter().position(|x| x == id) else { return false };
        if self.shown(id).is_some_and(|s| s.fixed) {
            return false;
        }
        let j = i as i32 + dir;
        j >= 0 && (j as usize) < self.selected.len() && !self.shown(&self.selected[j as usize]).is_some_and(|s| s.fixed)
    }

    fn shift(&mut self, id: &str, dir: i32) {
        if self.can_move(id, dir)
            && let Some(i) = self.selected.iter().position(|x| x == id)
        {
            self.selected.swap(i, (i as i32 + dir) as usize);
        }
    }

    /// Selection requested: incompatible pack → confirmation first.
    fn ask_select(&mut self, gui: &Gui, id: &str) {
        let Some(s) = self.shown(id) else { return };
        if s.entry.compat.ok() {
            self.toggle(id);
            return;
        }
        let t = |k: &str| tr(&gui.lang, k, &[]);
        let mut c = Column::new(
            Component::translate("pack.incompatible.confirm.title", vec![]),
            Component::translate(format!("pack.incompatible.confirm.{}", s.entry.compat.key()), vec![]),
            vec![(YES, t("gui.yes"), 150), (NO, t("gui.no"), 150)],
            20,
            0,
        );
        c.layout(gui);
        self.confirm = Some(Confirm::Incompatible(id.to_owned(), c));
    }

    /// Pack under the mouse: list, id, position relative to the icon.
    fn hit(&self, m: (f32, f32), lang: &rc_text::Lang) -> Option<(Side, String, f32, f32)> {
        if m.1 < HEADER as f32 || m.1 >= (self.size.1 - FOOTER) as f32 {
            return None;
        }
        for side in [Side::Available, Side::Selected] {
            let x = self.list_x(side);
            if m.0 < x as f32 || m.0 >= (x + LIST_W) as f32 {
                continue;
            }
            for (i, id) in self.visible(side, lang).iter().enumerate() {
                let y = self.entry_y(side, i);
                let (cx, cy) = (x + 2, y);
                if m.1 >= (cy - 2) as f32 && m.1 < (cy - 2 + ENTRY_H) as f32 {
                    return Some((side, id.clone(), m.0 - cx as f32, m.1 - cy as f32));
                }
            }
        }
        None
    }

    fn press(&mut self, gui: &Gui, m: (f32, f32)) {
        let Some((side, id, rx, ry)) = self.hit(m, &gui.lang) else { return };
        self.focus = Some((side, id.clone()));
        let Some(s) = self.shown(&id) else { return };
        if s.fixed && s.entry.required {
            return;
        }
        let on_icon = (0.0..32.0).contains(&rx) && (0.0..32.0).contains(&ry);
        if !on_icon {
            return;
        }
        match side {
            Side::Available => self.ask_select(gui, &id),
            Side::Selected => {
                if rx < 16.0 {
                    if !s.entry.required {
                        self.toggle(&id);
                    }
                } else if ry < 16.0 {
                    self.shift(&id, -1);
                } else {
                    self.shift(&id, 1);
                }
            }
        }
    }

    /// Pack icon: slot written at the next draw (reading of `pack.png`).
    fn icon_slot(&mut self, id: &str) -> Option<usize> {
        if let Some(s) = self.icons.get(id) {
            return *s;
        }
        let used = self.icons.values().flatten().count();
        let slot = (used < rc_assets::gui::PACK_ICON_SLOTS)
            .then(|| self.shown(id).and_then(|s| rc_assets::packs::read_root(&s.entry.path, "pack.png")))
            .flatten()
            .and_then(|png| icon_rgba(&png))
            .map(|rgba| {
                self.uploads.push((used, rgba));
                used
            });
        self.icons.insert(id.to_owned(), slot);
        slot
    }

    fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, o: &Options, mouse: Option<(f32, f32)>) {
        self.layout(gui);
        gui.next_layer();
        widgets::menu_background(gui, o.i("menuBackgroundBlurriness"), self.inworld);
        gui.next_layer();
        let (w, h) = self.size;
        let (title, drop) =
            (chars(&tr(&gui.lang, "resourcePack.title", &[])), chars(&tr(&gui.lang, "pack.dropInfo", &[])));
        widgets::centered(gui, q, &title, w / 2, 4, 0xffff_ffff);
        widgets::centered(gui, q, &drop, w / 2, 17, 0xffaa_aaaa);
        let hover = mouse.and_then(|m| self.hit(m, &gui.lang));
        for side in [Side::Available, Side::Selected] {
            self.draw_list(gui, q, side, hover.as_ref());
        }
        // Header and footer separators over the full width.
        let (head, foot) = if self.inworld {
            ("bg/inworld_header_separator", "bg/inworld_footer_separator")
        } else {
            ("bg/header_separator", "bg/footer_separator")
        };
        gui.sprite_tiled(head, 0.0, (HEADER - 2) as f32, w as f32, HEADER as f32, 32.0, 2.0, 0.0, 0.0);
        gui.sprite_tiled(foot, 0.0, (h - FOOTER) as f32, w as f32, (h - FOOTER + 2) as f32, 32.0, 2.0, 0.0, 0.0);
        self.panel.draw(gui, q, mouse, 0.0);
        for (slot, rgba) in self.uploads.drain(..) {
            gui.write_sprite(q, &format!("dyn/pack_icon/{slot}"), (64, 64), &rgba);
        }
        if let Some(Confirm::Incompatible(_, c) | Confirm::Drop(_, c) | Confirm::Notice(c)) = &mut self.confirm {
            c.draw(gui, q, o, mouse, 0.0);
        }
    }

    fn draw_list(&mut self, gui: &mut Gui, q: &wgpu::Queue, side: Side, hover: Option<&(Side, String, f32, f32)>) {
        let x = self.list_x(side);
        let (y0, y1) = (HEADER, self.size.1 - FOOTER);
        let ids = self.visible(side, &gui.lang);
        let k = if side == Side::Available { 0 } else { 1 };
        self.scroll[k] = self.scroll[k].clamp(0.0, self.max_scroll(ids.len()));
        let bg = if self.inworld { "bg/inworld_menu_list_background" } else { "bg/menu_list_background" };
        let (fx0, fx1) = (x as f32, (x + LIST_W) as f32);
        gui.sprite_tiled(bg, fx0, y0 as f32, fx1, y1 as f32, 32.0, 32.0, fx1, y1 as f32 + self.scroll[k].floor());
        gui.set_clip(Some([fx0, y0 as f32, fx1, y1 as f32]));
        // List title (underlined bold), in the scrolling content.
        let title = if side == Side::Available { "pack.available.title" } else { "pack.selected.title" };
        let mut tc = chars(&tr(&gui.lang, title, &[]));
        for c in &mut tc {
            c.style.bold = true;
            c.style.underlined = true;
        }
        let ty = y0 + 4 - self.scroll[k] as i32 + (TITLE_H - 9) / 2 + 1;
        widgets::centered(gui, q, &tc, x + LIST_W / 2, ty, 0xffff_ffff);
        let scrollable = self.max_scroll(ids.len()) > 0.0;
        for (i, id) in ids.iter().enumerate() {
            let cy = self.entry_y(side, i);
            if cy + ENTRY_H < y0 || cy - 2 > y1 {
                continue;
            }
            let cx = x + 2;
            let Some(s) = self.shown(id).cloned() else { continue };
            let focused = self.focus.as_ref().is_some_and(|(fs, fid)| *fs == side && fid == id);
            if focused {
                let (a, b, t0, t1) =
                    (x as f32, (x + LIST_W - if scrollable { 6 } else { 0 }) as f32, (cy - 2) as f32, (cy + 34) as f32);
                // White outline when the list has focus (no button or field has it).
                gui.fill(a, t0, b, t1, crate::widgets::selection_outline(self.panel.focus.is_none()));
                gui.fill(a + 1.0, t0 + 1.0, b - 1.0, t1 - 1.0, 0xff00_0000);
            }
            if !s.entry.compat.ok() {
                gui.fill(
                    (cx - 1) as f32,
                    (cy - 1) as f32,
                    (cx + LIST_W - 4 + 1) as f32,
                    (cy + 32 + 1) as f32,
                    0xff77_0000,
                );
            }
            match self.icon_slot(id) {
                Some(slot) => {
                    gui.sprite(&format!("dyn/pack_icon/{slot}"), cx as f32, cy as f32, 32.0, 32.0, 0xffff_ffff)
                }
                None => gui.sprite("tex/misc/unknown_pack", cx as f32, cy as f32, 32.0, 32.0, 0xffff_ffff),
            }
            let hovered = hover.is_some_and(|h| h.0 == side && h.1 == *id);
            let overlay = !(s.fixed && s.entry.required) && (hovered || focused);
            let mut name = styled(&s.entry.title, &gui.lang);
            let mut desc = styled(&s.desc, &gui.lang);
            if overlay {
                gui.fill(cx as f32, cy as f32, (cx + 32) as f32, (cy + 32) as f32, 0xa090_9090);
                if !s.entry.compat.ok() {
                    name = chars(&tr(&gui.lang, "pack.incompatible", &[]));
                    desc = styled(
                        &gray(Component::translate(format!("pack.incompatible.{}", s.entry.compat.key()), vec![])),
                        &gui.lang,
                    );
                }
                let (rx, ry) = hover.filter(|h| h.0 == side && h.1 == *id).map_or((-1.0, -1.0), |h| (h.2, h.3));
                let inside = |x0: f32, y0: f32, x1: f32, y1: f32| rx >= x0 && rx < x1 && ry >= y0 && ry < y1;
                let spr = |gui: &mut Gui, base: &str, hl: bool| {
                    let n = if hl {
                        format!("transferable_list/{base}_highlighted")
                    } else {
                        format!("transferable_list/{base}")
                    };
                    gui.sprite(&n, cx as f32, cy as f32, 32.0, 32.0, 0xffff_ffff);
                };
                if side == Side::Available {
                    spr(gui, "select", inside(0.0, 0.0, 32.0, 32.0));
                } else {
                    if !s.entry.required {
                        spr(gui, "unselect", inside(0.0, 0.0, 16.0, 32.0));
                    }
                    if self.can_move(id, -1) {
                        spr(gui, "move_up", inside(16.0, 0.0, 32.0, 16.0));
                    }
                    if self.can_move(id, 1) {
                        spr(gui, "move_down", inside(16.0, 16.0, 32.0, 32.0));
                    }
                }
            }
            let max = DESC_W - if scrollable { 6.0 } else { 0.0 };
            let tx = (cx + 34) as f32;
            // Name cut with "...", like the game (and not cut sharply by the clipping).
            let (name, lines) = entry_text(gui, &name, &desc, max);
            gui.text(q, &name, tx, (cy + 1) as f32, 0xffff_ffff, true, 0);
            for (j, line) in lines.iter().enumerate() {
                gui.text(q, line, tx, (cy + 12 + 9 * j as i32) as f32, 0xff80_8080, true, 0);
            }
        }
        gui.set_clip(None);
        // List separators and scrollbar.
        let max = self.max_scroll(ids.len());
        if max > 0.0 {
            let lh = self.list_h();
            let content = (lh + max as i32).max(1);
            let bar_h = (lh * lh / content).clamp(32, (lh - 8).max(32));
            let bx = (x + LIST_W - 6) as f32;
            gui.sprite("widget/scroller_background", bx, y0 as f32, 6.0, lh as f32, 0xffff_ffff);
            let by = y0 + (self.scroll[k] * (lh - bar_h) as f32 / max) as i32;
            gui.sprite("widget/scroller", bx, by as f32, 6.0, bar_h as f32, 0xffff_ffff);
        }
    }
}

impl App {
    /// "Resource Packs…" of the options.
    pub(crate) fn open_pack_screen(&mut self) {
        self.packs.stack.rescan();
        self.packs.screen = Some(Box::new(PackScreen::new(&self.packs.stack, self.front.in_world)));
    }

    /// "Done": choices saved, reload if the list changed (§ 4).
    fn close_pack_screen(&mut self) {
        let Some(s) = self.packs.screen.take() else { return };
        let chosen: Vec<String> = s.selected.iter().rev().filter(|id| !id.starts_with("server/")).cloned().collect();
        // High contrast: the option follows the presence of the pack.
        let hc = chosen.iter().any(|x| x == "high_contrast");
        if hc != s.initial.iter().any(|x| x == "high_contrast") {
            self.options.set("highContrast", Val::B(hc));
        }
        self.packs.stack.selected = chosen;
        let changed = self.packs.stack.write_options(&mut self.options);
        self.save_options();
        eprintln!("resource packs chosen: {:?}", self.packs.stack.selected);
        if changed {
            self.reload_resources();
        }
    }

    /// Packs screen frame: folder watching.
    pub(crate) fn pack_screen_tick(&mut self) {
        let Some(s) = &mut self.packs.screen else { return };
        if s.watch.0.elapsed().as_secs_f32() >= 1.0 {
            s.watch.0 = Instant::now();
            let snap = snapshot();
            if snap != s.watch.1 {
                s.watch.1 = snap;
                s.reload_in = Some(Instant::now());
            }
        }
        // Reread one second after the last change.
        if s.reload_in.is_some_and(|t| t.elapsed().as_secs_f32() >= 1.0) {
            s.reload_in = None;
            let keep = s.selected.clone();
            self.packs.stack.rescan();
            if let Some(s) = &mut self.packs.screen {
                s.fill(&self.packs.stack, keep);
                s.icons.clear();
            }
        }
    }

    /// Input of the packs screen; true if consumed (all input is when it is
    /// open; redraw, size, focus and cursor go to the rest of the application).
    pub(crate) fn pack_screen_window_event(&mut self, event: &WindowEvent) -> bool {
        self.pack_screen_input(classify(event))
    }

    /// Simulated left click at the mouse position (`RUSTCRAFT_MENUCLICKS` demo).
    pub(crate) fn pack_screen_click(&mut self) -> bool {
        self.pack_screen_input(In::Press) && self.pack_screen_input(In::Release)
    }

    fn pack_screen_input(&mut self, input: In<'_>) -> bool {
        if self.packs.screen.is_none() {
            return false;
        }
        let mouse = self.gui_mouse();
        let Some(r) = &self.renderer else { return true };
        let gui = &r.gui;
        let Some(s) = &mut self.packs.screen else { return true };
        s.layout(gui);
        let mut ev = Vec::new();
        let mut done = false;
        // Confirmation open: the only one to receive input.
        if let Some(Confirm::Incompatible(_, c) | Confirm::Drop(_, c) | Confirm::Notice(c)) = &mut s.confirm {
            match input {
                In::Motion => {
                    if let Some(m) = mouse {
                        c.panel.motion(m, &mut ev);
                    }
                    return false;
                }
                In::Press => {
                    if let Some(m) = mouse {
                        c.panel.press(m, 0, false, &mut ev);
                    }
                }
                In::Release => c.panel.release(&mut ev),
                In::Key(event) => {
                    if let Some(k) = crate::widgets::edit_key(event, self.mods)
                        && !c.panel.key(k, self.mods, &mut ev)
                        && k == KeyCode::Escape
                    {
                        ev.push(Event::Press(NO));
                    }
                }
                In::Swallow | In::Wheel(_) | In::Drop(_) | In::Text(_) => {}
                // Redraw, size, focus, cursor…: to the rest of the application.
                In::PassOn => return false,
            }
            if let Some(Confirm::Notice(_)) = &s.confirm {
                self.front.clicks += ev.iter().filter(|e| matches!(e, Event::Press(_))).count() as u32;
                // Esc arrives here as "No".
                if crate::symlink_notice::handle(&ev, ev.contains(&Event::Press(NO))) {
                    s.confirm = None;
                }
                return true;
            }
            for e in ev {
                if let Event::Press(b @ (YES | NO)) = e {
                    self.front.clicks += 1;
                    match s.confirm.take() {
                        Some(Confirm::Incompatible(id, _)) if b == YES => s.toggle(&id),
                        Some(Confirm::Drop(files, _)) if b == YES => {
                            let (copied, next) = drop_packs(gui, &files);
                            if copied {
                                s.reload_in = Some(Instant::now());
                            }
                            s.confirm = next;
                        }
                        _ => {}
                    }
                }
            }
            return true;
        }
        match input {
            In::Motion => {
                if let Some(m) = mouse {
                    s.panel.motion(m, &mut ev);
                }
                return false;
            }
            In::Press => {
                if let Some(m) = mouse {
                    s.panel.press(m, 0, false, &mut ev);
                    if ev.is_empty() && s.hit(m, &gui.lang).is_some() {
                        s.press(gui, m);
                        self.front.clicks += 1;
                    }
                }
            }
            In::Release => s.panel.release(&mut ev),
            In::Wheel(delta) => {
                let dy = match delta {
                    MouseScrollDelta::LineDelta(_, y) => *y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 20.0,
                };
                if let Some(m) = mouse {
                    let side = if m.0 < (s.size.0 / 2) as f32 { 0 } else { 1 };
                    s.scroll[side] -= dy * ENTRY_H as f32 / 2.0;
                }
            }
            In::Key(event) => {
                let Some(k) = crate::widgets::edit_key(event, self.mods) else { return true };
                let shift = self.mods.shift_key();
                let field_focus = s.panel.focus.and_then(|i| s.panel.widgets.get(i)).is_some_and(|w| w.id == SEARCH);
                let list_key = !field_focus && s.focus.is_some();
                match k {
                    KeyCode::Escape => done = true,
                    KeyCode::Enter | KeyCode::NumpadEnter | KeyCode::Space if list_key => {
                        if let Some((side, id)) = s.focus.clone() {
                            match side {
                                Side::Available => s.ask_select(gui, &id),
                                Side::Selected => s.toggle(&id),
                            }
                        }
                    }
                    KeyCode::ArrowUp | KeyCode::ArrowDown if list_key => {
                        let dir = if k == KeyCode::ArrowUp { -1 } else { 1 };
                        if let Some((side, id)) = s.focus.clone() {
                            if shift {
                                s.shift(&id, dir);
                            } else {
                                let v = s.visible(side, &gui.lang);
                                if let Some(i) = v.iter().position(|x| *x == id) {
                                    let j = (i as i32 + dir).clamp(0, v.len() as i32 - 1) as usize;
                                    s.focus = Some((side, v[j].clone()));
                                }
                            }
                        }
                    }
                    _ => {
                        s.panel.key(k, self.mods, &mut ev);
                        if let Some(t) = crate::widgets::typed_text(event, self.mods) {
                            s.panel.typed(t, &mut ev);
                        }
                    }
                }
            }
            In::Text(t) => s.panel.typed(t, &mut ev),
            In::Drop(path) => {
                let t = |k: &str| tr(&gui.lang, k, &[]);
                let files = match s.confirm.take() {
                    Some(Confirm::Drop(mut f, _)) => {
                        f.push(path.clone());
                        f
                    }
                    _ => vec![path.clone()],
                };
                let names = files
                    .iter()
                    .filter_map(|f| f.file_name())
                    .map(|n| n.to_string_lossy())
                    .collect::<Vec<_>>()
                    .join(", ");
                let mut c = Column::new(
                    Component::translate("pack.dropConfirm", vec![]),
                    Component::text(names),
                    vec![(YES, t("gui.yes"), 150), (NO, t("gui.no"), 150)],
                    20,
                    0,
                );
                c.layout(gui);
                s.confirm = Some(Confirm::Drop(files, c));
            }
            In::Swallow => {}
            // Redraw, size, focus, cursor…: to the rest of the application.
            In::PassOn => return false,
        }
        for e in ev {
            match e {
                Event::Press(DONE) => done = true,
                Event::Press(OPEN_FOLDER) => {
                    let dir = pack_stack::pack_dir();
                    let _ = std::fs::create_dir_all(&dir);
                    if let Err(e) = std::process::Command::new("explorer").arg(&dir).spawn() {
                        eprintln!("packs folder: {e}");
                    }
                }
                Event::Edit(SEARCH) => {
                    if let Some(Widget { kind: Kind::Field(f), .. }) = s.panel.get(SEARCH) {
                        s.search_text = f.value();
                    }
                    s.scroll = [0.0; 2];
                }
                _ => {}
            }
        }
        self.front.clicks += std::mem::take(&mut s.panel.clicks);
        if done {
            self.close_pack_screen();
        }
        true
    }

    /// Mouse-less demo: `RUSTCRAFT_PACKSCREEN=<s>` opens the screen after `s`
    /// seconds, selects all the packs of the folder one second later, then "Done"
    /// one second after that.
    pub(crate) fn pack_screen_test(&mut self) {
        let Some(at) = rc_data::env_str!("RUSTCRAFT_PACKSCREEN").and_then(|v| v.parse::<f64>().ok()) else { return };
        let t = self.started.elapsed().as_secs_f64();
        let step = &mut self.packs.screen_test;
        if *step == 0 && t >= at {
            *step = 1;
            self.open_pack_screen();
        } else if *step == 1 && t >= at + 1.5 {
            *step = 2;
            if let Some(s) = &mut self.packs.screen {
                let files: Vec<String> = s.available.iter().filter(|x| x.starts_with("file/")).cloned().collect();
                for id in files {
                    s.toggle(&id);
                }
                s.focus = s.selected.first().map(|id| (Side::Selected, id.clone()));
            }
        } else if *step == 2 && t >= at + 3.0 {
            *step = 3;
            self.close_pack_screen();
        }
    }
}

/// Drawing of the open screen (called by `Packs::draw`).
pub(crate) fn draw(screen: &mut PackScreen, gui: &mut Gui, q: &wgpu::Queue, o: &Options, mouse: Option<(f32, f32)>) {
    screen.draw(gui, q, o, mouse);
}

/// Dropped files accepted ("Yes"): each entry examined as a pack (links checked).
/// A refused link: warning, nothing copied. Otherwise the packs are copied into the folder (under
/// the name of their target for an allowed link) and the other entries reported — an allowed link
/// also counts among them, like in the game. Returns (copy done, next screen).
fn drop_packs(gui: &Gui, files: &[PathBuf]) -> (bool, Option<Confirm>) {
    let allow = crate::servers::allowed_symlinks();
    let mut refused = Vec::new();
    let mut packs: Vec<PathBuf> = Vec::new();
    for f in files {
        match allow.pack_entry(f, &mut refused) {
            Ok(Some(p)) => packs.push(p),
            Ok(None) => eprintln!("drop: {} does not look like a pack", f.display()),
            Err(e) => eprintln!("drop: {} unreadable: {e}", f.display()),
        }
    }
    if !refused.is_empty() {
        for r in &refused {
            eprintln!("drop: symbolic link refused {} -> {}", r.link.display(), r.target.display());
        }
        let mut c = crate::symlink_notice::column(&gui.lang, false);
        c.layout(gui);
        return (false, Some(Confirm::Notice(c)));
    }
    let dir = pack_stack::pack_dir();
    for p in &packs {
        let Some(name) = p.file_name() else { continue };
        let to = dir.join(name);
        let r = if p.is_dir() { copy_dir(p, &to) } else { std::fs::copy(p, &to).map(|_| ()) };
        if let Err(e) = r {
            eprintln!("dropped pack {}: {e}", p.display());
        }
    }
    let left: Vec<String> = files
        .iter()
        .filter(|f| !packs.contains(f))
        .filter_map(|f| f.file_name())
        .map(|n| n.to_string_lossy().into_owned())
        .collect();
    if left.is_empty() {
        return (!packs.is_empty(), None);
    }
    let mut c = Column::new(
        Component::translate("pack.dropRejected.title", vec![]),
        Component::translate("pack.dropRejected.message", vec![rc_text::Arg::Text(left.join(", "))]),
        vec![(crate::symlink_notice::BACK, tr(&gui.lang, "gui.back", &[]), 150)],
        20,
        20,
    );
    c.layout(gui);
    (!packs.is_empty(), Some(Confirm::Notice(c)))
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(to)?;
    for e in std::fs::read_dir(from)? {
        let e = e?;
        let p = e.path();
        if p.is_dir() {
            copy_dir(&p, &to.join(e.file_name()))?;
        } else {
            std::fs::copy(&p, to.join(e.file_name()))?;
        }
    }
    Ok(())
}

/// Normalized input of the screen (window or demo).
enum In<'a> {
    Motion,
    Press,
    Release,
    Key(&'a winit::event::KeyEvent),
    /// Text committed by the IME (typing without a key).
    Text(&'a str),
    Wheel(&'a MouseScrollDelta),
    Drop(&'a PathBuf),
    /// Input with no effect here, but kept.
    Swallow,
    /// Not an input: to the rest of the application.
    PassOn,
}

fn classify(e: &WindowEvent) -> In<'_> {
    match e {
        WindowEvent::CursorMoved { .. } => In::Motion,
        WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => In::Press,
        WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left, .. } => In::Release,
        WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => In::Key(event),
        WindowEvent::MouseWheel { delta, .. } => In::Wheel(delta),
        WindowEvent::DroppedFile(p) => In::Drop(p),
        WindowEvent::Ime(_) => crate::ime::commit(e).map_or(In::Swallow, In::Text),
        WindowEvent::KeyboardInput { .. } | WindowEvent::MouseInput { .. } => In::Swallow,
        _ => In::PassOn,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each character advances by 6, "." by 2.
    struct Fake;
    impl rc_text::Metrics for Fake {
        fn advance(&self, _font: &str, c: char) -> (f32, f32) {
            (if c == '.' { 2.0 } else { 6.0 }, 1.0)
        }
    }

    fn plain(v: &[StyledChar]) -> String {
        v.iter().map(|c| c.c).collect()
    }

    /// Name too long: "..." at the end (the old code cut it sharply); description of more than
    /// two lines: the second ends with "...".
    #[test]
    fn long_names_and_descriptions_end_with_dots() {
        let name = chars("abcdefghijklmnopqrstuvwxyzabcdef");
        let desc = chars("aaaa bbbb cccc dddd eeee ffff");
        // 60 wide: 54 for the name ("..." = 6) → 9 letters; description: 2 words per line
        // ("aaaa bbbb" = 54).
        let (n, lines) = entry_text(&Fake, &name, &desc, 60.0);
        assert_eq!(plain(&n), "abcdefghi...");
        assert_eq!(lines.len(), 2);
        assert_eq!(plain(&lines[0]), "aaaa bbbb");
        // Second line "cccc dddd" (54): cut to 54 − 6 = 48 → 8 characters, then "...".
        assert_eq!(plain(&lines[1]), "cccc ddd...");
        // Short enough: nothing is cut.
        let (n, lines) = entry_text(&Fake, &chars("abc"), &chars("aaaa bbbb"), 60.0);
        assert_eq!(plain(&n), "abc");
        assert_eq!(lines.iter().map(|l| plain(l)).collect::<Vec<_>>(), ["aaaa bbbb"]);
    }
}
