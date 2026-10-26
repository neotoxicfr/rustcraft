//! Clickable and hoverable text (`docs/specs/text.md` § 3.6, `chat.md` § 4): lookup of the
//! style under the mouse, click actions (link with confirmation screen, verified command,
//! suggestion, copy, dialog, custom action), hover tooltips (text, item,
//! entity), system clipboard and browser opening.

use std::time::Instant;

use rc_text::{Arg, CharStyle, ClickEvent, Component, HoverEvent, StyledChar};
use winit::event::{ElementState, Ime, MouseButton, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};

use crate::gui::Gui;

/// Style of the character under the abscissa `x` (line frame, origin at the first character),
/// looked up like the game in the line as it is drawn (visual order, Arabic forms).
pub fn style_at<'a>(gui: &Gui, chars: &'a [StyledChar], x: f32) -> Option<&'a CharStyle> {
    style_in(gui, chars, gui.visual_order_of(chars).as_deref(), x)
}

/// `order`: visual order (character, rank of the source character that gives the style); `None`:
/// `chars` as is.
pub(crate) fn style_in<'a>(
    m: &dyn rc_text::Metrics,
    chars: &'a [StyledChar],
    order: Option<&[(char, u32)]>,
    x: f32,
) -> Option<&'a CharStyle> {
    if x < 0.0 {
        return None;
    }
    let n = order.map_or(chars.len(), <[_]>::len);
    let mut acc = 0.0;
    for k in 0..n {
        let (c, st) = match order {
            Some(o) => (o[k].0, &chars[o[k].1 as usize].style),
            None => (chars[k].c, &chars[k].style),
        };
        let (a, bold) = m.advance(&st.font, c);
        acc += if st.bold { a + bold } else { a };
        if x < acc {
            return Some(st);
        }
    }
    None
}

/// Calls to the system (clipboard read, address opening).
#[allow(unsafe_code)]
mod sys {
    #[cfg(windows)]
    mod win {
        #[link(name = "user32")]
        unsafe extern "system" {
            pub fn OpenClipboard(owner: isize) -> i32;
            pub fn GetClipboardData(format: u32) -> isize;
            pub fn CloseClipboard() -> i32;
        }
        #[link(name = "kernel32")]
        unsafe extern "system" {
            pub fn GlobalLock(mem: isize) -> *mut u8;
            pub fn GlobalUnlock(mem: isize) -> i32;
        }
        #[link(name = "shell32")]
        unsafe extern "system" {
            pub fn ShellExecuteW(
                hwnd: isize,
                op: *const u16,
                file: *const u16,
                params: *const u16,
                dir: *const u16,
                show: i32,
            ) -> isize;
        }
    }

    /// CF_UNICODETEXT text of the clipboard (empty if unreadable).
    pub fn clipboard() -> String {
        #[cfg(windows)]
        // SAFETY: clipboard opened then closed; the block is locked for the duration of the
        // copy, read up to the final zero (aligned UTF-16, allocated by the system).
        unsafe {
            if win::OpenClipboard(0) == 0 {
                return String::new();
            }
            let mut out = String::new();
            let mem = win::GetClipboardData(13);
            if mem != 0 {
                let p = win::GlobalLock(mem).cast::<u16>();
                if !p.is_null() {
                    let mut n = 0;
                    while *p.add(n) != 0 {
                        n += 1;
                    }
                    out = String::from_utf16_lossy(std::slice::from_raw_parts(p, n));
                    win::GlobalUnlock(mem);
                }
            }
            win::CloseClipboard();
            return out;
        }
        // Linux: desktop tools (Wayland then X11), the first one present answers.
        #[cfg(all(unix, not(target_os = "macos")))]
        for cmd in [&["wl-paste", "--no-newline"][..], &["xclip", "-o", "-selection", "clipboard"], &["xsel", "-bo"]] {
            if let Ok(o) = std::process::Command::new(cmd[0]).args(&cmd[1..]).output()
                && o.status.success()
            {
                return String::from_utf8_lossy(&o.stdout).into_owned();
            }
        }
        #[allow(unreachable_code)]
        String::new()
    }

    /// Opening by the system (default browser); false on failure.
    pub fn shell_open(url: &str) -> bool {
        #[cfg(windows)]
        {
            let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
            let (op, file) = (wide("open"), wide(url));
            // SAFETY: zero-terminated UTF-16 strings, alive during the call.
            let r = unsafe { win::ShellExecuteW(0, op.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), 1) };
            return r > 32;
        }
        #[cfg(all(unix, not(target_os = "macos")))]
        return std::process::Command::new("xdg-open").arg(url).status().is_ok_and(|s| s.success());
        #[allow(unreachable_code)]
        false
    }
}

/// Text of the system clipboard (empty if unreadable).
pub fn get_clipboard() -> String {
    sys::clipboard()
}

/// Copy to the system clipboard.
pub fn set_clipboard(text: &str) {
    crate::debug_sys::set_clipboard(text);
}

/// Opening an `http(s)` address in the default browser, off the main thread.
/// `RUSTCRAFT_NOBROWSER`: opening only logged (tests).
/// Opens a local file with the default application (screenshots link).
pub fn open_file(path: &str) {
    if rc_data::env_flag!("RUSTCRAFT_NOBROWSER") {
        eprintln!("file: simulated opening of {path}");
        return;
    }
    if !std::path::Path::new(path).is_file() {
        return;
    }
    let path = path.to_owned();
    std::thread::spawn(move || {
        if !sys::shell_open(&path) {
            eprintln!("file: cannot open {path}");
        }
    });
}

/// Opens a folder in the system explorer (disconnect report).
pub fn open_dir(dir: &std::path::Path) {
    if rc_data::env_flag!("RUSTCRAFT_NOBROWSER") {
        eprintln!("folder: simulated opening of {}", dir.display());
        return;
    }
    let dir = dir.to_string_lossy().into_owned();
    std::thread::spawn(move || {
        if !sys::shell_open(&dir) {
            eprintln!("folder: cannot open {dir}");
        }
    });
}

pub fn open_url(url: &str) {
    if rc_data::env_flag!("RUSTCRAFT_NOBROWSER") {
        eprintln!("link: simulated opening of {url}");
        return;
    }
    if !rc_text::valid_url(url) {
        return;
    }
    let url = url.to_owned();
    std::thread::spawn(move || {
        if !sys::shell_open(&url) {
            eprintln!("link: cannot open {url}");
        }
    });
}

/// Screen the click comes from (return after confirmation, effect of a command).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Origin {
    Chat,
    Book,
    Death,
}

/// Accept button of a command to confirm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Accept {
    Run,
    Suggest,
    Copy,
}

#[derive(Clone, Debug)]
enum Kind {
    Link(String),
    Command { command: String, key: &'static str, accept: Accept },
}

/// Confirmation screen: link (Yes / Copy / No) or command (accept / back).
pub struct Confirm {
    kind: Kind,
    origin: Origin,
    /// Buttons (x, y, width, label) at the last draw.
    buttons: Vec<(i32, i32, i32, Vec<StyledChar>)>,
}

impl Confirm {
    fn new(kind: Kind, origin: Origin) -> Confirm {
        Confirm { kind, origin, buttons: Vec::new() }
    }

    /// Index of the "no / back" button (Esc).
    fn back(&self) -> usize {
        match self.kind {
            Kind::Link(_) => 2,
            Kind::Command { .. } => 1,
        }
    }

    pub(crate) fn hit(&self, m: (f32, f32)) -> Option<usize> {
        self.buttons.iter().position(|(x, y, w, _)| {
            m.0 >= *x as f32 && m.1 >= *y as f32 && m.0 < (x + w) as f32 && m.1 < (y + 20) as f32
        })
    }

    /// Drawing (centered column, spacing 8; buttons 16 lower, spaced by 4).
    pub fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, mouse: Option<(f32, f32)>) {
        let line = |gui: &Gui, c: &Component| rc_text::styled_chars(&rc_text::flatten(c, &gui.lang));
        let key = |gui: &Gui, k: &str| line(gui, &Component::translate(k, Vec::new()));
        let (w, h) = (gui.width as i32, gui.height as i32);
        let (title, message, warning, labels, bw) = match &self.kind {
            Kind::Link(url) => (
                key(gui, "chat.link.confirm"),
                line(gui, &Component::text(url.clone())),
                Some(key(gui, "chat.link.warning")),
                vec![key(gui, "gui.yes"), key(gui, "chat.copy"), key(gui, "gui.no")],
                100,
            ),
            Kind::Command { command, key: k, accept } => {
                let mut cmd = Component::text(command.clone());
                cmd.style.color = Some(0xFFFF55);
                let accept = match accept {
                    Accept::Run => "multiplayer.confirm_command.run_command",
                    Accept::Suggest => "multiplayer.confirm_command.suggest_command",
                    Accept::Copy => "chat.copy",
                };
                (
                    key(gui, "multiplayer.confirm_command.title"),
                    line(gui, &Component::translate(*k, vec![Arg::Component(cmd)])),
                    None,
                    vec![key(gui, accept), key(gui, "gui.back")],
                    150,
                )
            }
        };
        let mut lines = rc_text::wrap(gui, &message, (w - 50) as f32);
        lines.truncate(15);
        let total = 9 + 8 + 9 * lines.len() as i32 + 8 + warning.as_ref().map_or(0, |_| 9 + 8) + 16 + 20;
        let mut y = (h - total) / 2;
        gui.next_layer();
        crate::widgets::ingame_background(gui);
        let mid = w / 2;
        crate::widgets::centered(gui, q, &title, mid, y, 0xffff_ffff);
        y += 9 + 8;
        for l in &lines {
            crate::widgets::centered(gui, q, l, mid, y, 0xffff_ffff);
            y += 9;
        }
        y += 8;
        if let Some(wn) = &warning {
            crate::widgets::centered(gui, q, wn, mid, y, 0xffff_cccc);
            y += 9 + 8;
        }
        y += 16;
        let n = labels.len() as i32;
        let row = n * bw + (n - 1) * 4;
        let x0 = (w - row) / 2;
        self.buttons = labels.into_iter().enumerate().map(|(i, l)| (x0 + i as i32 * (bw + 4), y, bw, l)).collect();
        for (x, by, bw, label) in &self.buttons {
            let hovered = mouse.is_some_and(|(mx, my)| {
                mx >= *x as f32 && my >= *by as f32 && mx < (x + bw) as f32 && my < (by + 20) as f32
            });
            let sprite = if hovered { "widget/button_highlighted" } else { "widget/button" };
            gui.sprite(sprite, *x as f32, *by as f32, *bw as f32, 20.0, 0xffff_ffff);
            let lw = rc_text::width(gui, label);
            gui.text(q, label, (x + bw / 2 - lw / 2) as f32, (by + 6) as f32, 0xffff_ffff, true, 0);
        }
    }
}

/// Tooltip lines of a hover (text cut at max(W/2, 200); entity only with the advanced
/// tooltips).
pub fn hover_lines(gui: &Gui, h: &HoverEvent, data: &rc_data::VersionData, advanced: bool) -> Vec<Vec<StyledChar>> {
    let line = |c: &Component| rc_text::styled_chars(&rc_text::flatten(c, &gui.lang));
    match h {
        HoverEvent::Text(c) => rc_text::wrap(gui, &line(c), (gui.width / 2.0).floor().max(200.0)),
        HoverEvent::Item { id, count: _, components } => {
            let full = if id.contains(':') { id.clone() } else { format!("minecraft:{id}") };
            let index = data.registry("minecraft:item").and_then(|r| r.entries.iter().position(|e| *e == full));
            let info = index.and_then(|i| data.items.get(i));
            let comp = |k: &str| {
                components
                    .as_ref()
                    .and_then(|c| rc_data::ids::with_key(&["minecraft:", k], |key| c.get(key)).or_else(|| c.get(k)))
            };
            let custom = comp("custom_name").and_then(rc_text::from_nbt);
            let italic = custom.is_some();
            let (ns, path) = full.split_once(':').unwrap_or(("minecraft", &full));
            let key = info.map(|i| i.name_key.clone()).filter(|k| !k.is_empty());
            let name = custom.or_else(|| comp("item_name").and_then(rc_text::from_nbt)).unwrap_or_else(|| {
                Component::translate(key.unwrap_or_else(|| format!("item.{ns}.{path}")), Vec::new())
            });
            let mut root = Component::text("");
            root.style.color =
                Some([0xff_ffff, 0xff_ff55, 0x55_ffff, 0xff_55ff][info.map_or(0, |i| i.rarity).min(3) as usize]);
            if italic {
                root.style.italic = Some(true);
            }
            root.extra.push(name);
            let mut out = vec![line(&root)];
            if let Some(rc_proto::nbt::Nbt::List(lore)) = comp("lore") {
                for l in lore.iter().filter_map(rc_text::from_nbt) {
                    let mut c = Component::text("");
                    c.style.color = Some(0xAA00AA);
                    c.style.italic = Some(true);
                    c.extra.push(l);
                    out.push(line(&c));
                }
            }
            if advanced {
                let mut c = Component::text(full.clone());
                c.style.color = Some(0x555555);
                out.push(line(&c));
            }
            out
        }
        HoverEvent::Entity { id, uuid, name } => {
            if !advanced {
                return Vec::new();
            }
            let (ns, path) = id.split_once(':').unwrap_or(("minecraft", id));
            let mut out = Vec::new();
            if let Some(n) = name {
                out.push(line(n));
            }
            let kind = Component::translate(format!("entity.{ns}.{path}"), Vec::new());
            out.push(line(&Component::translate("gui.entity_tooltip.type", vec![Arg::Component(kind)])));
            let h = format!("{uuid:032x}");
            let text = format!("{}-{}-{}-{}-{}", &h[0..8], &h[8..12], &h[12..16], &h[16..20], &h[20..32]);
            out.push(line(&Component::text(text)));
            out
        }
    }
}

/// Hover tooltip drawn near the mouse.
pub fn draw_hover(
    gui: &mut Gui,
    q: &wgpu::Queue,
    h: &HoverEvent,
    m: (f32, f32),
    data: &rc_data::VersionData,
    advanced: bool,
) {
    let lines = hover_lines(gui, h, data, advanced);
    if lines.is_empty() {
        return;
    }
    gui.next_layer();
    crate::widgets::draw_tooltip(gui, q, &lines, (m.0 as i32, m.1 as i32, 0, 0), Some(m));
}

/// Book open for reading (hover tooltips).
pub fn book_of(o: &mut Option<crate::special::Overlay>) -> Option<&mut crate::book::Book> {
    match o {
        Some(crate::special::Overlay::Book(b)) => Some(&mut **b),
        _ => None,
    }
}

/// Top of the interface: tooltips of the chat, the book and the death screen, then the
/// confirmation screen.
#[allow(clippy::too_many_arguments)]
pub fn draw_overlay(
    gui: &mut Gui,
    q: &wgpu::Queue,
    chat: &mut crate::chat::Chat,
    book: Option<&mut crate::book::Book>,
    death: Option<&crate::death::DeathScreen>,
    mouse: Option<(f32, f32)>,
    data: &rc_data::VersionData,
    advanced: bool,
) {
    let mouse = chat.test_mouse.or(mouse);
    if let Some(c) = &mut chat.confirm {
        c.draw(gui, q, mouse);
        return;
    }
    let Some(m) = mouse else { return };
    let hover = chat
        .hover
        .clone()
        .or_else(|| book.and_then(|b| b.style_at(gui, m)).and_then(|s| s.hover))
        .or_else(|| death.and_then(|d| d.cause_style_at(gui, m)).and_then(|s| s.hover));
    if let Some(h) = hover {
        draw_hover(gui, q, &h, m, data, advanced);
    }
}

/// Hand on hovering a clickable text, like the game: action on click in the open chat, the
/// book and the death screen; text to insert into the chat with Shift held.
pub fn clickable_under(
    gui: &Gui,
    chat: &mut crate::chat::Chat,
    book: Option<&mut crate::book::Book>,
    death: Option<&crate::death::DeathScreen>,
    m: (f32, f32),
    shift: bool,
) -> bool {
    if chat.confirm.is_some() {
        return false;
    }
    let style = chat
        .style_at(gui, m)
        .or_else(|| book.and_then(|b| b.style_at(gui, m)))
        .or_else(|| death.and_then(|d| d.cause_style_at(gui, m)));
    style.is_some_and(|s| wants_hand(&s, shift))
}

/// Style that calls for the hand: action on click, or text to insert with Shift.
fn wants_hand(s: &CharStyle, shift: bool) -> bool {
    s.click.is_some() || shift && s.insertion.is_some()
}

impl crate::App {
    /// Inputs of this module, before the others: confirmation screen (everything goes to it), click
    /// on a link of the death screen, mouse and IME of the open chat. True: consumed.
    pub(crate) fn links_window_event(&mut self, event: &WindowEvent) -> bool {
        self.sync_ime();
        // IME composition (common to all fields); the committed text follows the keystroke path.
        if crate::ime::observe(event) {
            return true;
        }
        if self.chat.confirm.is_some() {
            match event {
                WindowEvent::KeyboardInput { event, .. } => {
                    if event.state == ElementState::Pressed
                        && event.physical_key == PhysicalKey::Code(KeyCode::Escape)
                        && let Some(c) = &self.chat.confirm
                    {
                        let b = c.back();
                        self.confirm_press(b);
                    }
                    return true;
                }
                WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                    let m = self.chat.test_mouse.or_else(|| self.gui_mouse());
                    if let (Some(m), Some(c)) = (m, &self.chat.confirm)
                        && let Some(i) = c.hit(m)
                    {
                        self.confirm_press(i);
                    }
                    return true;
                }
                WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. } | WindowEvent::Ime(_) => return true,
                _ => return false,
            }
        }
        // Death screen: only a link (`open_url`) of the cause is clickable.
        if self.death.is_some()
            && let WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } = event
            && let (Some(r), Some(m)) = (&self.renderer, self.gui_mouse())
            && let Some(d) = &self.death
            && let Some(ev) = d.cause_style_at(&r.gui, m).and_then(|s| s.click)
            && ev.action == "open_url"
        {
            self.click_event(&ev, Origin::Death);
            return true;
        }
        if self.chat.input.is_none() || self.menus.is_open() {
            return false;
        }
        match event {
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                if let Some(m) = self.gui_mouse() {
                    self.chat_click_at(m, self.mods.shift_key());
                }
                true
            }
            WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left, .. } => {
                self.chat.dragging = false;
                true
            }
            WindowEvent::CursorMoved { position, .. } => {
                if self.chat.dragging
                    && let Some(r) = &self.renderer
                {
                    let s = f64::from(r.gui.scale);
                    let m = ((position.x / s) as f32, (position.y / s) as f32);
                    self.chat.field_drag(&r.gui, m);
                }
                false
            }
            WindowEvent::Ime(Ime::Commit(t)) => {
                if self.chat.type_text(t) {
                    self.cmd.text_changed(&mut self.chat);
                }
                true
            }
            _ => false,
        }
    }

    /// IME input allowed as long as an editable field has focus (`ime.rs`), candidate window
    /// placed on the field's area (pixels = units × scale).
    fn sync_ime(&mut self) {
        let Some(r) = &self.renderer else { return };
        let (allow, area) = crate::ime::window_update();
        if let Some(a) = allow {
            r.window.set_ime_allowed(a);
        }
        if let Some([x0, y0, x1, y1]) = area {
            let s = f64::from(r.gui.scale);
            let px = |v: i32| (f64::from(v) * s).round() as i32;
            let len = |v: i32| ((f64::from(v) * s).round() as u32).max(1);
            r.window.set_ime_cursor_area(
                winit::dpi::PhysicalPosition::new(px(x0), px(y0)),
                winit::dpi::PhysicalSize::new(len(x1 - x0), len(y1 - y0)),
            );
        }
    }

    /// Left click in the chat screen (`m` in interface units): suggestion list,
    /// then message text (Shift: insertion), then input field.
    pub(crate) fn chat_click_at(&mut self, m: (f32, f32), shift: bool) {
        if self.cmd.click(&mut self.chat, m) {
            return;
        }
        let Some(r) = &self.renderer else { return };
        let style = self.chat.style_at(&r.gui, m);
        if let Some(s) = style {
            if shift {
                if let Some(ins) = &s.insertion {
                    if self.chat.insert_text(ins, false) {
                        self.cmd.text_changed(&mut self.chat);
                    }
                    return;
                }
            } else if let Some(ev) = &s.click {
                self.click_event(ev, Origin::Chat);
                return;
            }
        }
        let Some(r) = &self.renderer else { return };
        let double = self.chat.last_click.is_some_and(|(t, p)| t.elapsed().as_millis() < 250 && p == m);
        self.chat.last_click = Some((Instant::now(), m));
        self.chat.field_click(&r.gui, m, shift, double);
    }

    /// Action of a click on a text.
    pub(crate) fn click_event(&mut self, ev: &ClickEvent, origin: Origin) {
        match ev.action.as_str() {
            "open_url" => {
                if !self.chat.settings.links {
                    return;
                }
                if self.chat.settings.links_prompt {
                    self.chat.confirm = Some(Confirm::new(Kind::Link(ev.value.clone()), origin));
                } else {
                    open_url(&ev.value);
                }
            }
            "run_command" => {
                let c = ev.value.strip_prefix('/').unwrap_or(&ev.value).to_owned();
                self.run_unattended(&c, origin);
            }
            "suggest_command" => {
                if origin == Origin::Chat && self.chat.insert_text(&ev.value, true) {
                    self.cmd.text_changed(&mut self.chat);
                }
            }
            "copy_to_clipboard" => set_clipboard(&ev.value),
            // Local file (screenshot): produced by the client only, the components
            // received from servers refuse it on reading.
            "open_file" => open_file(&ev.value),
            "show_dialog" => {
                let d = match &ev.nbt {
                    Some(rc_proto::nbt::Nbt::String(id)) => {
                        let short = |s: &str| s.strip_prefix("minecraft:").unwrap_or(s).to_owned();
                        let i = self.exact.dialogs.iter().position(|(k, _)| short(k) == short(id));
                        i.map(|i| rc_client::extra::DialogRef::Registry(i as i32))
                    }
                    Some(n) => Some(rc_client::extra::DialogRef::Inline(n.clone())),
                    None => None,
                };
                if let Some(d) = d {
                    if origin == Origin::Chat {
                        self.cmd.close(&mut self.chat);
                        self.chat.close();
                    }
                    self.show_dialog(d, false);
                }
            }
            // Internal ids handled by the chat screen alone; from another origin (book),
            // the custom action goes to the server like any other.
            "custom" if ev.value == crate::chat::QUEUE_EXPAND && origin == Origin::Chat => self.chat.accept_next(),
            // Restricted chat prompt: restrictions screen over the chat (Esc comes back to it,
            // scroll reset to the bottom as when the game leaves the chat screen).
            "custom" if ev.value == crate::chat::GO_TO_RESTRICTIONS && origin == Origin::Chat => {
                self.chat.reset_scroll();
                self.menus.open(crate::menus::Id::Restrictions, false);
            }
            "custom" => {
                let _ = self.session.link.send_custom_click_action(false, &ev.value, ev.nbt.as_ref());
            }
            // `change_page`: handled by the book; elsewhere no effect.
            _ => {}
        }
    }

    /// Command of a clicked text: sent if it parses without
    /// error, without signable argument and without particular permissions; otherwise confirmation screen.
    pub(crate) fn run_unattended(&mut self, command: &str, origin: Origin) {
        use rc_commands::parse::{Source, signable};
        let input: Vec<char> = std::iter::once('/').chain(command.chars()).collect();
        let (tree, data) = (&self.cmd.tree, &self.cmd.data);
        let full = rc_commands::parse(tree, &input, data, Source { restricted: true, selectors: data.selectors() });
        let issue = if !full.valid(&input) {
            Some(("multiplayer.confirm_command.parse_errors", Accept::Run))
        } else if !signable(tree, &full, &input).is_empty() {
            // From the chat: copy; with no screen to reopen (book): suggestion in the chat.
            let accept = if origin == Origin::Book && self.cmd.allowed.1 { Accept::Suggest } else { Accept::Copy };
            Some(("multiplayer.confirm_command.signature_required", accept))
        } else {
            let bare =
                rc_commands::parse(tree, &input, data, Source { restricted: false, selectors: data.selectors() });
            (!bare.valid(&input)).then_some(("multiplayer.confirm_command.permissions_required", Accept::Run))
        };
        match issue {
            None => {
                let _ = self.session.link.send_command(command);
                self.after_command(origin);
            }
            Some((key, accept)) => {
                let kind = Kind::Command { command: command.to_owned(), key, accept };
                self.chat.confirm = Some(Confirm::new(kind, origin));
            }
        }
    }

    /// Screen after a sent command: the chat stays open; a book closes.
    fn after_command(&mut self, origin: Origin) {
        if origin == Origin::Book {
            self.close_overlay();
        }
    }

    /// Button of the confirmation screen.
    pub(crate) fn confirm_press(&mut self, i: usize) {
        let Some(c) = self.chat.confirm.take() else { return };
        if let Some(a) = self.audio.get() {
            a.play(crate::widgets::CLICK_SOUND, rc_audio::Category::Ui, None, 0.25, 1.0, None);
        }
        match (&c.kind, i) {
            (Kind::Link(url), 0) => open_url(url),
            (Kind::Link(url), 1) => set_clipboard(url),
            (Kind::Command { command, accept, .. }, 0) => match accept {
                Accept::Run => {
                    let _ = self.session.link.send_command(command);
                    self.after_command(c.origin);
                }
                Accept::Copy => set_clipboard(&format!("/{command}")),
                Accept::Suggest => {
                    self.close_overlay();
                    self.chat.open("/");
                    self.chat.insert_text(command, false);
                    self.commands_refresh();
                    self.cmd.open(&mut self.chat);
                    self.commands_send();
                    self.keys.clear();
                    self.grab(false);
                }
            },
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixed;
    impl rc_text::Metrics for Fixed {
        fn advance(&self, _: &str, c: char) -> (f32, f32) {
            (if c == ' ' { 4.0 } else { 6.0 }, 1.0)
        }
    }

    #[test]
    fn click_finds_the_style_where_the_rtl_run_is_drawn() {
        // "abc שלום": "של" leads to one link, "ום" to another; the Hebrew run is
        // drawn backwards after "abc", so "ום" on the left. Under the mouse, the style is
        // that of the letter drawn at that spot.
        let st = |s: &str| CharStyle { insertion: Some(s.into()), ..CharStyle::default() };
        let mut chars: Vec<StyledChar> = "abc ".chars().map(|c| StyledChar { c, style: CharStyle::default() }).collect();
        chars.extend("של".chars().map(|c| StyledChar { c, style: st("a") }));
        chars.extend("ום".chars().map(|c| StyledChar { c, style: st("b") }));
        let order = rc_text::bidi::visual(&chars, false).expect("reordered line");
        let at = |o: Option<&[(char, u32)]>, x: f32| {
            style_in(&Fixed, &chars, o, x).and_then(|s| s.insertion.as_deref().map(str::to_owned))
        };
        // "abc" 0..18, space 18..22, "םו" 22..34, "לש" 34..46.
        assert_eq!(at(Some(&order), 1.0), None);
        assert_eq!(at(Some(&order), 23.0).as_deref(), Some("b"));
        assert_eq!(at(Some(&order), 45.0).as_deref(), Some("a"));
        assert_eq!(at(Some(&order), 47.0), None);
        // In logical order, the abscissa 23 fell on "של".
        assert_eq!(at(None, 23.0).as_deref(), Some("a"));
    }

    #[test]
    fn hand_over_clickable_text_and_insertions_with_shift() {
        let click = CharStyle {
            click: Some(std::sync::Arc::new(ClickEvent { action: "open_url".into(), value: "https://a".into(), nbt: None })),
            ..Default::default()
        };
        let insertion = CharStyle { insertion: Some("x".into()), ..Default::default() };
        assert!(wants_hand(&click, false));
        assert!(!wants_hand(&insertion, false) && wants_hand(&insertion, true));
        assert!(!wants_hand(&CharStyle::default(), true));
    }
}
