//! Multiplayer screen (`docs/specs/title-screen-servers.md` § 2–3): `servers.dat` list
//! queried by at most 5 threads, LAN worlds (multicast), and the "add / edit",
//! "direct connection" and delete confirmation screens.

use std::net::{Ipv4Addr, UdpSocket};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use rc_client::status::{self, Favicon, PingOutcome, ServerAddress, StatusError};
use rc_text::{Component, StyledChar};
use winit::keyboard::{KeyCode, ModifiersState};

use crate::gui::Gui;
use crate::options::Options;
use crate::servers::{PackStatus, ServerEntry, ServerList};
use crate::widgets::{self, Entry, Event, Kind, List, Panel, TextField, Widget, chars, tr};

/// Simultaneous query threads at most.
const PING_THREADS: usize = 5;
const PING_TIMEOUT: Duration = Duration::from_secs(5);
const ROW_H: i32 = 36;
const ROW_W: i32 = 305;
const GRAY: u32 = 0xff80_8080;

fn styled(c: &Component, lang: &rc_text::Lang) -> Vec<StyledChar> {
    rc_text::styled_chars(&rc_text::flatten(c, lang))
}

/// Text of a fixed color (RGB).
fn colored(s: &str, rgb: u32) -> Component {
    let mut c = Component::text(s.to_owned());
    c.style.color = Some(rgb);
    c
}

// --- Queries ---

type PingResult = (u64, Result<PingOutcome, StatusError>);

/// Tooltip of a row: text lines and hovered rectangle.
type Tooltip = (Vec<Vec<StyledChar>>, (i32, i32, i32, i32));

/// Group of query threads; the threads stop when it is dropped.
struct Pinger {
    jobs: Option<Sender<(u64, String)>>,
    results: Receiver<PingResult>,
}

impl Pinger {
    fn new() -> Pinger {
        let (jobs, rx) = crossbeam_channel::unbounded::<(u64, String)>();
        let (tx, results) = crossbeam_channel::unbounded();
        for i in 0..PING_THREADS {
            let (rx, tx) = (rx.clone(), tx.clone());
            let _ = std::thread::Builder::new().name(format!("status-{i}")).spawn(move || {
                while let Ok((id, ip)) = rx.recv() {
                    if tx.send((id, status::ping(&ip, PING_TIMEOUT))).is_err() {
                        break;
                    }
                }
            });
        }
        Pinger { jobs: Some(jobs), results }
    }

    fn submit(&self, id: u64, ip: &str) {
        if let Some(j) = &self.jobs {
            let _ = j.send((id, ip.to_owned()));
        }
    }
}

impl Drop for Pinger {
    fn drop(&mut self) {
        // End of the queue: the threads stop after their current query.
        self.jobs = None;
    }
}

// --- LAN (§ 2.4) ---

/// Announced LAN world: MOTD and port (`[MOTD]…[/MOTD][AD]…[/AD]`); `None` if malformed.
pub fn parse_lan(s: &str) -> Option<(String, String)> {
    let end = s.find("[/MOTD]")?;
    if s[end + 7..].contains("[/MOTD]") {
        return None;
    }
    let ad = end + 7 + s[end + 7..].find("[AD]")?;
    let ad_end = ad + 4 + s[ad + 4..].find("[/AD]")?;
    let port = s[ad + 4..ad_end].to_owned();
    let motd = match s.find("[MOTD]") {
        Some(start) if start + 6 <= end => s[start + 6..end].to_owned(),
        _ => "missing no".to_owned(),
    };
    Some((motd, port))
}

/// Multicast listening on 224.0.2.60:4445 on a thread; stopped on drop.
struct Lan {
    stop: Arc<AtomicBool>,
    found: Receiver<(String, String)>,
}

impl Lan {
    fn start() -> Option<Lan> {
        // All the interfaces like the game; `RUSTCRAFT_LAN_BIND=127.0.0.1` (builds and
        // agent tests) stays on the machine: no per-executable firewall prompt.
        let bind = rc_sim::lan::announce::bind_addr();
        let sock = UdpSocket::bind((bind, 4445)).ok()?;
        sock.join_multicast_v4(&Ipv4Addr::new(224, 0, 2, 60), &bind).ok()?;
        sock.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
        let stop = Arc::new(AtomicBool::new(false));
        let (tx, found) = crossbeam_channel::unbounded();
        let s = stop.clone();
        std::thread::Builder::new()
            .name("lan".into())
            .spawn(move || {
                let mut buf = [0u8; 1024];
                while !s.load(Ordering::Relaxed) {
                    let Ok((n, from)) = sock.recv_from(&mut buf) else { continue };
                    let text = String::from_utf8_lossy(&buf[..n]);
                    if let Some((motd, port)) = parse_lan(&text)
                        && tx.send((motd, format!("{}:{port}", from.ip()))).is_err()
                    {
                        break;
                    }
                }
            })
            .ok()?;
        Some(Lan { stop, found })
    }
}

impl Drop for Lan {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
    }
}

// --- List ---

#[derive(Debug, Clone)]
enum Ping {
    Initial,
    Pinging,
    Done(Box<Status>),
    /// Unreachable: key of the message shown in place of the MOTD.
    Failed(&'static str),
}

#[derive(Debug, Clone)]
struct Status {
    motd: Component,
    /// Players (online, max); `None`: `???`.
    players: Option<(i32, i32)>,
    sample: Vec<String>,
    version: String,
    compatible: bool,
    latency: u64,
}

struct Row {
    entry: ServerEntry,
    ping: Ping,
    /// Id of the query in progress (stale answers ignored).
    id: u64,
}

/// Request of the multiplayer screen to its owner.
#[derive(Debug, Clone, PartialEq)]
pub enum Out {
    Join(ServerEntry),
    Add,
    Edit(ServerEntry),
    Direct,
    Delete(String),
    Back,
}

const JOIN: u32 = 1;
const DIRECT: u32 = 2;
const ADD: u32 = 3;
const EDIT: u32 = 4;
const DELETE: u32 = 5;
const REFRESH: u32 = 6;
const BACK: u32 = 7;

pub struct Multiplayer {
    pub panel: Panel,
    size: (i32, i32),
    dir: PathBuf,
    list: ServerList,
    rows: Vec<Row>,
    lan: Option<Lan>,
    lan_worlds: Vec<(String, String)>,
    pinger: Pinger,
    next_id: u64,
    /// Content written into each icon slot (PNG), to rewrite only on change.
    slots: Vec<Option<Vec<u8>>>,
    title: Vec<StyledChar>,
}

impl Multiplayer {
    pub fn new(dir: PathBuf) -> Multiplayer {
        let list = ServerList::load(&dir);
        let rows = list.visible.iter().map(|e| Row { entry: e.clone(), ping: Ping::Initial, id: 0 }).collect();
        Multiplayer {
            panel: Panel::default(),
            size: (-1, -1),
            dir,
            list,
            rows,
            lan: Lan::start(),
            lan_worlds: Vec::new(),
            pinger: Pinger::new(),
            next_id: 1,
            slots: vec![None; rc_assets::gui::SERVER_ICON_SLOTS],
            title: Vec::new(),
        }
    }

    fn save(&mut self) {
        self.list.visible = self.rows.iter().map(|r| r.entry.clone()).collect();
        if let Err(e) = self.list.save(&self.dir) {
            eprintln!("servers.dat: {e}");
        }
    }

    /// Add from the "add" screen (unhides a hidden entry with the same address).
    pub fn add(&mut self, e: ServerEntry) {
        self.list.visible = self.rows.iter().map(|r| r.entry.clone()).collect();
        self.list.add(e);
        if let Some(last) = self.list.visible.last() {
            self.rows.push(Row { entry: last.clone(), ping: Ping::Initial, id: 0 });
        }
        self.save();
        self.size = (-2, -2);
    }

    /// Validated edit of entry `old_ip`/name: copy and re-query.
    pub fn replace(&mut self, old: &ServerEntry, e: ServerEntry) {
        if let Some(r) = self.rows.iter_mut().find(|r| r.entry == *old) {
            r.entry.name = e.name;
            r.entry.ip = e.ip;
            r.entry.pack_status = e.pack_status;
            r.ping = Ping::Initial;
        }
        self.save();
        self.size = (-2, -2);
    }

    pub fn remove(&mut self, ip: &str) {
        if let Some(i) = self.selected_server().filter(|&i| self.rows[i].entry.ip == ip) {
            self.rows.remove(i);
            if let Some(l) = &mut self.panel.list {
                l.selected = None;
            }
            self.save();
            self.size = (-2, -2);
        }
    }

    /// Direct connection: known entry (visible or hidden), otherwise new hidden entry.
    pub fn direct_entry(&mut self, ip: &str, lang: &rc_text::Lang) -> ServerEntry {
        if let Some(e) = self.list.find(ip) {
            return e.clone();
        }
        let e = ServerEntry::new(tr(lang, "selectServer.defaultName", &[]), ip);
        self.list.visible = self.rows.iter().map(|r| r.entry.clone()).collect();
        self.list.add_hidden(e.clone());
        self.save();
        e
    }

    fn selected(&self) -> Option<usize> {
        self.panel.list.as_ref().and_then(|l| l.selected)
    }

    fn selected_server(&self) -> Option<usize> {
        self.selected().filter(|&i| i < self.rows.len())
    }

    pub fn layout(&mut self, gui: &Gui) {
        let size = (gui.width as i32, gui.height as i32);
        if self.size == size {
            return;
        }
        let resized = self.size.0 >= 0;
        self.size = size;
        let (w, h) = size;
        self.title = chars(&tr(&gui.lang, "multiplayer.title", &[]));
        let old = self.panel.list.take();
        let mut l = List::new(0, 33, w, h - 33 - 60, ROW_W, ROW_H);
        l.inworld = false;
        l.custom_rows = true;
        let n = self.rows.len() + 1 + self.lan_worlds.len();
        l.entries = (0..n as i32).map(|i| Entry { top: i * ROW_H, h: ROW_H, label: Vec::new() }).collect();
        l.content = n as i32 * ROW_H + 4;
        if let Some(o) = old {
            l.selected = o.selected.filter(|&s| s < n);
            l.set_scroll(o.scroll);
        }
        let t = |k: &str| tr(&gui.lang, k, &[]);
        let x0 = (w - 308) / 2;
        let (y1, y2) = (h - 52, h - 28);
        let focus = self.panel.focus.and_then(|i| self.panel.widgets.get(i)).map(|wd| wd.id);
        let keyboard = self.panel.keyboard;
        self.panel = Panel::new(vec![
            Widget::button(JOIN, x0, y1, 100, &t("selectServer.select")),
            Widget::button(DIRECT, x0 + 104, y1, 100, &t("selectServer.direct")),
            Widget::button(ADD, x0 + 208, y1, 100, &t("selectServer.add")),
            Widget::button(EDIT, x0, y2, 74, &t("selectServer.edit")),
            Widget::button(DELETE, x0 + 78, y2, 74, &t("selectServer.delete")),
            Widget::button(REFRESH, x0 + 156, y2, 74, &t("selectServer.refresh")),
            Widget::button(BACK, x0 + 234, y2, 74, &t("gui.back")),
        ]);
        self.panel.list = Some(l);
        self.panel.keyboard = keyboard;
        // Resize: focus kept; return from a sub-screen: the list keeps control.
        if resized {
            self.panel.focus = focus.and_then(|id| self.panel.widgets.iter().position(|wd| wd.id == id));
        }
        self.update_buttons();
    }

    /// Buttons active according to the selection (§ 2.2).
    fn update_buttons(&mut self) {
        let sel = self.selected();
        let n = self.rows.len();
        let (join, edit) = match sel {
            Some(i) if i < n => (true, true),
            Some(i) if i > n => (true, false),
            _ => (false, false),
        };
        for (id, on) in [(JOIN, join), (EDIT, edit), (DELETE, edit)] {
            if let Some(w) = self.panel.get_mut(id) {
                w.active = on;
            }
        }
    }

    /// Answers arrived and LAN worlds discovered.
    pub fn poll(&mut self) {
        while let Ok((id, r)) = self.pinger.results.try_recv() {
            let Some(i) = self.rows.iter().position(|row| row.id == id) else { continue };
            let row = &mut self.rows[i];
            match r {
                Ok(PingOutcome::Modern(p)) => {
                    let s = p.status;
                    let motd = status::json_component(&s.description).unwrap_or_else(|| Component::text(""));
                    let sample = match &s.players {
                        Some(pl) => {
                            let mut v: Vec<String> = pl
                                .sample
                                .iter()
                                .map(|sp| if sp.is_anonymous() { "\u{1}anonymous".into() } else { sp.name.clone() })
                                .collect();
                            if (pl.sample.len() as i32) < pl.online {
                                v.push(format!("\u{1}more:{}", pl.online - pl.sample.len() as i32));
                            }
                            v
                        }
                        None => Vec::new(),
                    };
                    let version = s.version.as_ref().map_or_else(String::new, |v| v.name.clone());
                    // Icon: PNG different from the known one → stored and list saved.
                    if let Some(f) = &s.favicon
                        && row.entry.icon.as_deref() != Some(&f.png[..])
                    {
                        row.entry.icon = Some(f.png.clone());
                        let (name, ip, icon) = (row.entry.name.clone(), row.entry.ip.clone(), row.entry.icon.clone());
                        let _ = ServerList::update_icon(&self.dir, &name, &ip, icon);
                    }
                    row.ping = Ping::Done(Box::new(Status {
                        motd,
                        players: s.players.as_ref().map(|pl| (pl.online, pl.max)),
                        sample,
                        version,
                        compatible: p.compatible,
                        latency: p.latency_ms,
                    }));
                }
                Ok(PingOutcome::Legacy(l)) => {
                    row.ping = Ping::Done(Box::new(Status {
                        motd: Component::text(l.motd),
                        players: Some((l.online, l.max)),
                        sample: Vec::new(),
                        version: l.version,
                        compatible: false,
                        latency: 0,
                    }));
                }
                Err(StatusError::UnknownHost) => row.ping = Ping::Failed("multiplayer.status.cannot_resolve"),
                Err(_) => row.ping = Ping::Failed("multiplayer.status.cannot_connect"),
            }
        }
        let mut grew = false;
        if let Some(l) = &self.lan {
            while let Ok((motd, addr)) = l.found.try_recv() {
                match self.lan_worlds.iter_mut().find(|w| w.1 == addr) {
                    Some(w) => w.0 = motd,
                    None => {
                        self.lan_worlds.push((motd, addr));
                        grew = true;
                    }
                }
            }
        }
        if grew {
            self.size = (-2, -2);
        }
    }

    fn join_selected(&mut self) -> Option<Out> {
        let i = self.selected()?;
        let n = self.rows.len();
        if i < n {
            return Some(Out::Join(self.rows[i].entry.clone()));
        }
        let (motd, addr) = self.lan_worlds.get(i.checked_sub(n + 1)?)?;
        Some(Out::Join(ServerEntry::new(motd.clone(), addr.clone())))
    }

    /// Swaps row `i` with its neighbor (`dir` = ±1) and saves; the selection follows.
    fn swap(&mut self, i: usize, dir: i32) {
        let j = i as i32 + dir;
        if i >= self.rows.len() || j < 0 || j as usize >= self.rows.len() {
            return;
        }
        self.rows.swap(i, j as usize);
        if let Some(l) = &mut self.panel.list {
            l.selected = Some(j as usize);
        }
        self.save();
        self.update_buttons();
    }

    /// Recreates the screen (F5, "Refresh"): rereads `servers.dat` and re-queries everything.
    pub fn refresh(&mut self) {
        let dir = self.dir.clone();
        *self = Multiplayer::new(dir);
    }

    fn handle(&mut self, events: Vec<Event>) -> Vec<Out> {
        let mut out = Vec::new();
        for e in events {
            match e {
                Event::Select(_) => self.update_buttons(),
                Event::Activate(_) => out.extend(self.join_selected()),
                Event::Press(JOIN) => out.extend(self.join_selected()),
                Event::Press(DIRECT) => out.push(Out::Direct),
                Event::Press(ADD) => out.push(Out::Add),
                Event::Press(EDIT) => {
                    if let Some(i) = self.selected_server() {
                        out.push(Out::Edit(self.rows[i].entry.clone()));
                    }
                }
                Event::Press(DELETE) => {
                    if let Some(i) = self.selected_server() {
                        out.push(Out::Delete(self.rows[i].entry.ip.clone()));
                    }
                }
                Event::Press(REFRESH) => self.refresh(),
                Event::Press(BACK) => out.push(Out::Back),
                _ => {}
            }
        }
        out
    }

    /// Name of the entry to delete (confirmation).
    pub fn name_of(&self, ip: &str) -> String {
        self.rows.iter().find(|r| r.entry.ip == ip).map_or_else(String::new, |r| r.entry.name.clone())
    }

    pub fn motion(&mut self, m: (f32, f32)) -> Vec<Out> {
        let mut ev = Vec::new();
        self.panel.motion(m, &mut ev);
        self.handle(ev)
    }

    /// Click: icon zones of a hovered row (join, move up, move down), otherwise components.
    pub fn press(&mut self, m: (f32, f32), button: u8, shift: bool) -> Vec<Out> {
        if button == 0
            && let Some(l) = &self.panel.list
            && let Some(i) = l.entry_under(m).filter(|&i| i < self.rows.len())
            && let Some((x, y, _, _)) = l.entry_rect(i)
        {
            // Icon zones in whole pixels (right half, left quarters); silent clicks
            // as in the game (no sound for joining or moving a server).
            let (rx, ry) = (m.0.floor() as i32 - (x + 2), m.1.floor() as i32 - (y + 2));
            if (16..32).contains(&rx) && (0..32).contains(&ry) {
                if let Some(l) = &mut self.panel.list {
                    l.selected = Some(i);
                }
                self.update_buttons();
                return self.join_selected().into_iter().collect();
            }
            if (0..16).contains(&rx) && (0..16).contains(&ry) && i > 0 {
                self.swap(i, -1);
                return Vec::new();
            }
            if (0..16).contains(&rx) && (16..32).contains(&ry) && i + 1 < self.rows.len() {
                self.swap(i, 1);
                return Vec::new();
            }
        }
        let mut ev = Vec::new();
        self.panel.press(m, button, shift, &mut ev);
        self.handle(ev)
    }

    pub fn release(&mut self) -> Vec<Out> {
        let mut ev = Vec::new();
        self.panel.release(&mut ev);
        self.handle(ev)
    }

    pub fn wheel(&mut self, m: (f32, f32), dy: f64) -> Vec<Out> {
        let mut ev = Vec::new();
        self.panel.wheel(m, dy, &mut ev);
        self.handle(ev)
    }

    /// Keys: F5 refreshes, Shift + Up/Down moves the row, Esc goes back to the title.
    pub fn key(&mut self, k: KeyCode, mods: ModifiersState) -> Vec<Out> {
        match k {
            KeyCode::F5 => {
                self.refresh();
                return Vec::new();
            }
            KeyCode::ArrowUp | KeyCode::ArrowDown if mods.shift_key() && self.panel.focus.is_none() => {
                if let Some(i) = self.selected_server() {
                    self.swap(i, if k == KeyCode::ArrowUp { -1 } else { 1 });
                }
                return Vec::new();
            }
            _ => {}
        }
        let mut ev = Vec::new();
        let used = self.panel.key(k, mods, &mut ev);
        let mut out = self.handle(ev);
        if !used && k == KeyCode::Escape {
            out.push(Out::Back);
        }
        out
    }

    pub fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, o: &Options, mouse: Option<(f32, f32)>, time: f64) {
        self.poll();
        self.layout(gui);
        gui.next_layer();
        widgets::menu_background(gui, o.i("menuBackgroundBlurriness"), false);
        gui.next_layer();
        let w = gui.width as i32;
        widgets::centered(gui, q, &self.title, w / 2, 12, 0xffff_ffff);
        let lang = gui.lang.clone();
        let ms = (time * 1000.0) as u64;
        let hide_ip = o.b("hideServerAddress");
        let focused = self.panel.focus.is_none();
        let Multiplayer { panel, rows, lan_worlds, pinger, next_id, slots, .. } = self;
        let nrows = rows.len();
        let mut tooltip: Option<Tooltip> = None;
        let selected = panel.list.as_ref().and_then(|l| l.selected);
        panel.draw_with(gui, q, mouse, time, &mut |gui, q, l| {
            let hovered = mouse.and_then(|m| l.entry_under(m));
            for i in 0..l.entries.len() {
                let Some((left, top, rw, rh)) = l.entry_rect(i) else { continue };
                if top + rh < l.y || top > l.y + l.h {
                    continue;
                }
                if selected == Some(i) {
                    let c = if focused { 0xffff_ffff } else { GRAY };
                    gui.fill(left as f32, top as f32, (left + rw) as f32, (top + rh) as f32, c);
                    gui.fill(
                        (left + 1) as f32,
                        (top + 1) as f32,
                        (left + rw - 1) as f32,
                        (top + rh - 1) as f32,
                        0xff00_0000,
                    );
                }
                let (x, y, lc) = (left + 2, top + 2, rw - 4);
                if let Some(row) = rows.get_mut(i) {
                    // Query at first draw (§ 3.1).
                    if matches!(row.ping, Ping::Initial) {
                        row.id = *next_id;
                        *next_id += 1;
                        pinger.submit(row.id, &row.entry.ip);
                        row.ping = Ping::Pinging;
                    }
                    let hover = hovered == Some(i);
                    let tip = draw_server(gui, q, &lang, row, i, slots, (x, y, lc), ms, hover, mouse);
                    if tip.is_some() {
                        tooltip = tip;
                    }
                    if hover {
                        draw_hover_icons(gui, (x, y), i, i + 1 >= nrows, mouse);
                    }
                } else if i == rows.len() {
                    // LAN search: centered label and animated dots.
                    let label = chars(&tr(&lang, "lanServer.scanning", &[]));
                    let cx = x + lc / 2;
                    let tw = rc_text::width(gui, &label);
                    gui.text(q, &label, (cx - tw / 2) as f32, (y + 2) as f32, 0xffff_ffff, true, 0);
                    let frames = ["O o o", "o O o", "o o O", "o O o"];
                    let dots = chars(frames[(ms / 300 % 4) as usize]);
                    let dw = rc_text::width(gui, &dots);
                    gui.text(q, &dots, (cx - dw / 2) as f32, (y + 17) as f32, GRAY, true, 0);
                } else if let Some((motd, addr)) = lan_worlds.get(i - rows.len() - 1) {
                    let title = chars(&tr(&lang, "lanServer.title", &[]));
                    gui.text(q, &title, (x + 35) as f32, (y + 1) as f32, 0xffff_ffff, true, 0);
                    gui.text(q, &chars(motd), (x + 35) as f32, (y + 12) as f32, GRAY, true, 0);
                    let a = if hide_ip { tr(&lang, "selectServer.hiddenAddress", &[]) } else { addr.clone() };
                    gui.text(q, &chars(&a), (x + 35) as f32, (y + 23) as f32, GRAY, true, 0);
                }
            }
        });
        if let Some((lines, rect)) = tooltip {
            gui.next_layer();
            widgets::draw_tooltip(gui, q, &lines, rect, mouse);
        }
    }
}

/// Veil and arrows on the icon of a hovered row (§ 2.3).
fn draw_hover_icons(gui: &mut Gui, (x, y): (i32, i32), i: usize, last: bool, mouse: Option<(f32, f32)>) {
    let (xf, yf) = (x as f32, y as f32);
    gui.fill(xf, yf, xf + 32.0, yf + 32.0, 0xa090_9090);
    let (rx, ry) = mouse.map_or((-1, -1), |m| (m.0.floor() as i32 - x, m.1.floor() as i32 - y));
    let (left, top, bottom) = ((0..16).contains(&rx), (0..16).contains(&ry), (16..32).contains(&ry));
    let right = (16..32).contains(&rx) && (0..32).contains(&ry);
    let join = if right { "server_list/join_highlighted" } else { "server_list/join" };
    gui.sprite(join, xf, yf, 32.0, 32.0, 0xffff_ffff);
    if i > 0 {
        let s = if left && top { "server_list/move_up_highlighted" } else { "server_list/move_up" };
        gui.sprite(s, xf, yf, 32.0, 32.0, 0xffff_ffff);
    }
    if !last {
        let s = if left && bottom { "server_list/move_down_highlighted" } else { "server_list/move_down" };
        gui.sprite(s, xf, yf, 32.0, 32.0, 0xffff_ffff);
    }
}

/// Content of a server row (§ 2.3); returns the possible hovered tooltip.
#[allow(clippy::too_many_arguments)]
fn draw_server(
    gui: &mut Gui,
    q: &wgpu::Queue,
    lang: &rc_text::Lang,
    row: &mut Row,
    i: usize,
    slots: &mut [Option<Vec<u8>>],
    (x, y, lc): (i32, i32, i32),
    ms: u64,
    hover: bool,
    mouse: Option<(f32, f32)>,
) -> Option<Tooltip> {
    // Icon: atlas slot rewritten if the PNG changed; refused if not 64 × 64.
    let mut icon = "tex/misc/unknown_server".to_owned();
    if let Some(png) = &row.entry.icon {
        match Favicon::from_png(png.clone()).and_then(|f| f.rgba()) {
            Some(rgba) if i < slots.len() => {
                let name = format!("dyn/server_icon/{i}");
                if slots[i].as_deref() != Some(&png[..]) && gui.write_sprite(q, &name, (64, 64), &rgba) {
                    slots[i] = Some(png.clone());
                }
                if slots[i].is_some() {
                    icon = name;
                }
            }
            Some(_) => {}
            None => row.entry.icon = None,
        }
    }
    gui.sprite(&icon, x as f32, y as f32, 32.0, 32.0, 0xffff_ffff);
    gui.text(q, &chars(&row.entry.name), (x + 35) as f32, (y + 1) as f32, 0xffff_ffff, true, 0);
    // MOTD (2 lines at most) or failure message in red.
    let motd = match &row.ping {
        Ping::Done(s) => styled(&s.motd, lang),
        Ping::Failed(k) => styled(&colored(&tr(lang, k, &[]), 0xff_0000), lang),
        _ => Vec::new(),
    };
    for (k, line) in rc_text::wrap(gui, &motd, (lc - 34) as f32).iter().take(2).enumerate() {
        gui.text(q, line, (x + 35) as f32, (y + 12 + 9 * k as i32) as f32, GRAY, true, 0);
    }
    // State: 10 × 8 icon and text to its left.
    let (sprite, tip, text): (String, Option<String>, Vec<StyledChar>) = match &row.ping {
        Ping::Initial | Ping::Pinging => {
            let mut k = ((ms / 100) as usize + 2 * i) & 7;
            if k > 4 {
                k = 8 - k;
            }
            (format!("server_list/pinging_{}", k + 1), Some(tr(lang, "multiplayer.status.pinging", &[])), Vec::new())
        }
        Ping::Failed(_) => {
            ("server_list/unreachable".into(), Some(tr(lang, "multiplayer.status.no_connection", &[])), Vec::new())
        }
        Ping::Done(s) if !s.compatible => (
            "server_list/incompatible".into(),
            Some(tr(lang, "multiplayer.status.incompatible", &[])),
            styled(&colored(&s.version, 0xff_5555), lang),
        ),
        Ping::Done(s) => {
            let n = match s.latency {
                0..150 => 5,
                150..300 => 4,
                300..600 => 3,
                600..1000 => 2,
                _ => 1,
            };
            let text = match s.players {
                Some((on, max)) => {
                    let c = Component::translate(
                        "multiplayer.status.player_count",
                        vec![
                            rc_text::Arg::Component(colored(&on.to_string(), 0xaa_aaaa)),
                            rc_text::Arg::Component(colored(&max.to_string(), 0xaa_aaaa)),
                        ],
                    );
                    let mut c = c;
                    c.style.color = Some(0x55_5555);
                    styled(&c, lang)
                }
                None => styled(&colored("???", 0x55_5555), lang),
            };
            (format!("server_list/ping_{n}"), Some(tr(lang, "multiplayer.status.ping", &[s.latency.to_string()])), text)
        }
    };
    let ix = x + lc - 15;
    gui.sprite(&sprite, ix as f32, y as f32, 10.0, 8.0, 0xffff_ffff);
    let tw = rc_text::width(gui, &text);
    let tx = ix - tw - 5;
    if !text.is_empty() {
        gui.text(q, &text, tx as f32, (y + 1) as f32, GRAY, true, 0);
    }
    let m = mouse.filter(|_| hover)?;
    // Game tooltip zones: bounds inclusive, in whole pixels (10 × 8 icon → 11 × 9).
    let (mx, my) = (m.0.floor() as i32, m.1.floor() as i32);
    let inside = |rx: i32, ry: i32, rw: i32, rh: i32| mx >= rx && mx <= rx + rw && my >= ry && my <= ry + rh;
    if inside(ix, y, 10, 8) {
        return tip.map(|t| (vec![chars(&t)], (ix, y, 10, 8)));
    }
    if let Ping::Done(s) = &row.ping
        && inside(tx, y, tw, 8)
        && !s.sample.is_empty()
    {
        let lines = s
            .sample
            .iter()
            .map(|n| match n.strip_prefix('\u{1}') {
                Some("anonymous") => chars(&tr(lang, "multiplayer.status.anonymous_player", &[])),
                Some(more) => {
                    let k = more.strip_prefix("more:").unwrap_or("0").to_owned();
                    chars(&tr(lang, "multiplayer.status.and_more", &[k]))
                }
                None => chars(n),
            })
            .collect();
        return Some((lines, (tx, y, tw, 9)));
    }
    None
}

// --- "Add / edit" screen (§ 2.5) ---

const NAME: u32 = 1;
const IP: u32 = 2;
const PACK: u32 = 3;
const DONE: u32 = 4;
const CANCEL: u32 = 5;

pub struct EditServer {
    pub panel: Panel,
    size: (i32, i32),
    /// Original entry (`None`: add).
    pub original: Option<ServerEntry>,
    pub entry: ServerEntry,
    title: Vec<StyledChar>,
}

/// Outcome of an input screen.
pub enum EditOut {
    Done(ServerEntry),
    Cancel,
}

fn field(id: u32, x: i32, y: i32, max: usize, hint: Vec<StyledChar>, value: &str) -> Widget {
    let mut t = TextField::new(hint);
    t.max_len = max;
    t.text = value.chars().take(max).collect();
    t.cursor = t.text.len();
    t.anchor = t.cursor;
    Widget::new(id, Kind::Field(Box::new(t)), x, y, 200, 20, Vec::new())
}

fn field_value(p: &Panel, id: u32) -> String {
    match p.get(id).map(|w| &w.kind) {
        Some(Kind::Field(t)) => t.value(),
        _ => String::new(),
    }
}

fn label(gui: &Gui, x: i32, y: i32, key: &str) -> Widget {
    Widget::new(0, Kind::Label { color: 0xffa0_a0a0, centered: false }, x, y, 200, 9, chars(&tr(&gui.lang, key, &[])))
}

fn pack_label(gui: &Gui, s: PackStatus) -> String {
    let v = match s {
        PackStatus::Enabled => "manageServer.resourcePack.enabled",
        PackStatus::Disabled => "manageServer.resourcePack.disabled",
        PackStatus::Prompt => "manageServer.resourcePack.prompt",
    };
    tr(&gui.lang, "options.generic_value", &[tr(&gui.lang, "manageServer.resourcePack", &[]), tr(&gui.lang, v, &[])])
}

impl EditServer {
    pub fn new(original: Option<ServerEntry>, default_name: String) -> EditServer {
        let entry = original.clone().unwrap_or_else(|| ServerEntry::new(default_name, ""));
        EditServer { panel: Panel::default(), size: (-1, -1), original, entry, title: Vec::new() }
    }

    pub fn layout(&mut self, gui: &Gui) {
        let size = (gui.width as i32, gui.height as i32);
        if self.size == size {
            return;
        }
        let first = self.size.0 < 0;
        if !first {
            self.entry.name = field_value(&self.panel, NAME);
            self.entry.ip = field_value(&self.panel, IP);
        }
        self.size = size;
        let (w, h) = size;
        let key = if self.original.is_some() { "manageServer.edit.title" } else { "manageServer.add.title" };
        self.title = chars(&tr(&gui.lang, key, &[]));
        let hint = chars(&tr(&gui.lang, "selectServer.defaultName", &[]));
        let x = w / 2 - 100;
        let pack =
            Widget::new(PACK, Kind::Cycle, x, h / 4 + 72, 200, 20, chars(&pack_label(gui, self.entry.pack_status)));
        self.panel = Panel::new(vec![
            label(gui, x + 1, 53, "manageServer.enterName"),
            field(NAME, x, 66, 32, hint, &self.entry.name),
            label(gui, x + 1, 94, "manageServer.enterIp"),
            field(IP, x, 106, 128, Vec::new(), &self.entry.ip),
            pack,
            Widget::button(DONE, x, h / 4 + 114, 200, &tr(&gui.lang, "gui.done", &[])),
            Widget::button(CANCEL, x, h / 4 + 138, 200, &tr(&gui.lang, "gui.cancel", &[])),
        ]);
        self.panel.focus_widget(NAME);
        self.validate();
    }

    fn validate(&mut self) {
        let ok = ServerAddress::is_valid(&field_value(&self.panel, IP));
        if let Some(w) = self.panel.get_mut(DONE) {
            w.active = ok;
        }
    }

    pub fn handle(&mut self, gui: &Gui, events: Vec<Event>) -> Option<EditOut> {
        for e in events {
            match e {
                Event::Edit(_) => self.validate(),
                Event::Cycle(PACK, dir) => {
                    let order = [PackStatus::Enabled, PackStatus::Disabled, PackStatus::Prompt];
                    let i = order.iter().position(|s| *s == self.entry.pack_status).unwrap_or(2) as i32;
                    self.entry.pack_status = order[(i + dir).rem_euclid(3) as usize];
                    if let Some(w) = self.panel.get_mut(PACK) {
                        w.msg = chars(&pack_label(gui, self.entry.pack_status));
                    }
                }
                Event::Press(DONE) => {
                    let mut e = self.entry.clone();
                    e.name = field_value(&self.panel, NAME);
                    if e.name.is_empty() {
                        e.name = tr(&gui.lang, "selectServer.defaultName", &[]);
                    }
                    e.ip = field_value(&self.panel, IP);
                    return Some(EditOut::Done(e));
                }
                Event::Press(CANCEL) => return Some(EditOut::Cancel),
                _ => {}
            }
        }
        None
    }

    pub fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, o: &Options, mouse: Option<(f32, f32)>, time: f64) {
        self.layout(gui);
        gui.next_layer();
        widgets::menu_background(gui, o.i("menuBackgroundBlurriness"), false);
        gui.next_layer();
        widgets::centered(gui, q, &self.title, gui.width as i32 / 2, 17, 0xffff_ffff);
        self.panel.draw(gui, q, mouse, time);
    }
}

// --- Direct connection (§ 2.6) ---

/// "Join" button of the direct connection (also emitted by Enter in the field).
pub const DIRECT_SELECT: u32 = 1;
const SELECT: u32 = DIRECT_SELECT;
const D_CANCEL: u32 = 2;
const D_IP: u32 = 3;

pub struct DirectJoin {
    pub panel: Panel,
    size: (i32, i32),
    value: String,
    title: Vec<StyledChar>,
}

impl DirectJoin {
    pub fn new(last: &str) -> DirectJoin {
        DirectJoin { panel: Panel::default(), size: (-1, -1), value: last.to_owned(), title: Vec::new() }
    }

    pub fn value(&self) -> String {
        if self.size.0 < 0 { self.value.clone() } else { field_value(&self.panel, D_IP) }
    }

    pub fn layout(&mut self, gui: &Gui) {
        let size = (gui.width as i32, gui.height as i32);
        if self.size == size {
            return;
        }
        self.value = self.value();
        self.size = size;
        let (w, h) = size;
        self.title = chars(&tr(&gui.lang, "selectServer.direct", &[]));
        let x = w / 2 - 100;
        self.panel = Panel::new(vec![
            label(gui, x + 1, 100, "manageServer.enterIp"),
            field(D_IP, x, 116, 128, Vec::new(), &self.value),
            Widget::button(SELECT, x, h / 4 + 108, 200, &tr(&gui.lang, "selectServer.select", &[])),
            Widget::button(D_CANCEL, x, h / 4 + 132, 200, &tr(&gui.lang, "gui.cancel", &[])),
        ]);
        self.panel.focus_widget(D_IP);
        self.validate();
    }

    fn validate(&mut self) {
        let ok = ServerAddress::is_valid(&field_value(&self.panel, D_IP));
        if let Some(w) = self.panel.get_mut(SELECT) {
            w.active = ok;
        }
    }

    /// Enter with the field active: join (if the address is valid).
    pub fn key_confirms(&self, k: KeyCode) -> bool {
        matches!(k, KeyCode::Enter | KeyCode::NumpadEnter)
            && self.panel.focus.and_then(|i| self.panel.widgets.get(i)).is_some_and(|w| w.id == D_IP)
            && self.panel.get(SELECT).is_some_and(|w| w.active)
    }

    pub fn handle(&mut self, events: Vec<Event>) -> Option<bool> {
        for e in events {
            match e {
                Event::Edit(_) => self.validate(),
                Event::Press(SELECT) => return Some(true),
                Event::Press(D_CANCEL) => return Some(false),
                _ => {}
            }
        }
        None
    }

    pub fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, o: &Options, mouse: Option<(f32, f32)>, time: f64) {
        self.layout(gui);
        gui.next_layer();
        widgets::menu_background(gui, o.i("menuBackgroundBlurriness"), false);
        gui.next_layer();
        widgets::centered(gui, q, &self.title, gui.width as i32 / 2, 20, 0xffff_ffff);
        self.panel.draw(gui, q, mouse, time);
    }
}

// --- Centered column (confirmation, disconnection) ---

/// Screen "title, message, buttons" vertically centered (delete confirmation,
/// disconnection): `gap` between title and message, `pad` before the buttons.
pub struct Column {
    pub panel: Panel,
    size: (i32, i32),
    pub title: Component,
    pub message: Component,
    /// Buttons: id, text, width.
    buttons: Vec<(u32, String, i32)>,
    gap: i32,
    pad: i32,
    /// Stacked buttons (disconnect screen) instead of a row (confirmations).
    stacked: bool,
    /// Warning line under the message (link confirmation).
    warning: Option<Component>,
    lines: Vec<Vec<StyledChar>>,
    warn_line: Vec<StyledChar>,
    top: i32,
    /// Gap between the buttons of a row (4; 5 on the link warning).
    space: i32,
    /// Game alert layout: title at 70, message at 90 (no line limit), button at
    /// `clamp(90 + text + 12, h/6 + 96, h − 24)`.
    alert: bool,
    /// Y of the first line of the message.
    msg_y: i32,
}

impl Column {
    pub fn new(title: Component, message: Component, buttons: Vec<(u32, String, i32)>, gap: i32, pad: i32) -> Column {
        Column {
            panel: Panel::default(),
            size: (-1, -1),
            title,
            message,
            buttons,
            gap,
            pad,
            stacked: false,
            warning: None,
            lines: Vec::new(),
            warn_line: Vec::new(),
            top: 0,
            space: 4,
            alert: false,
            msg_y: 0,
        }
    }

    /// Layout of the game's alert screen (one button).
    pub fn alert(mut self) -> Column {
        self.alert = true;
        self
    }

    /// Buttons (id, text, width).
    #[cfg(test)]
    pub fn buttons(&self) -> &[(u32, String, i32)] {
        &self.buttons
    }

    /// Ys (title, message, buttons) of the alert for `text_lines` lines of message.
    fn alert_rows(h: i32, text_lines: i32) -> (i32, i32, i32) {
        // Lower bound applied first, like the game (no panic if it exceeds the upper one).
        let by = (90 + 9 * text_lines + 12).max(h / 6 + 96).min(h - 24);
        (70, 90, by)
    }

    /// Gap between the buttons of a row.
    pub fn spaced(mut self, space: i32) -> Column {
        self.space = space;
        self
    }

    /// Buttons one under the other (spaced by 4), centered as a block.
    pub fn stacked(mut self) -> Column {
        self.stacked = true;
        self
    }

    /// Warning line between the message and the buttons.
    pub fn with_warning(mut self, w: Component) -> Column {
        self.warning = Some(w);
        self
    }

    pub fn layout(&mut self, gui: &Gui) {
        let size = (gui.width as i32, gui.height as i32);
        if self.size == size {
            return;
        }
        self.size = size;
        let (w, h) = size;
        self.lines = rc_text::wrap(gui, &styled(&self.message, &gui.lang), (w - 50) as f32);
        if self.alert {
            let (top, msg_y, by) = Column::alert_rows(h, self.lines.len() as i32);
            (self.top, self.msg_y) = (top, msg_y);
            let v: Vec<Widget> =
                self.buttons.iter().map(|(id, text, bw)| Widget::button(*id, (w - bw) / 2, by, *bw, text)).collect();
            let keyboard = self.panel.keyboard;
            self.panel = Panel::new(v);
            self.panel.keyboard = keyboard;
            return;
        }
        self.lines.truncate(15);
        let text_h = 9 * self.lines.len() as i32;
        self.warn_line = match &self.warning {
            Some(c) => styled(c, &gui.lang),
            None => Vec::new(),
        };
        let warn_h = if self.warning.is_some() { 9 + self.gap } else { 0 };
        let n = self.buttons.len() as i32;
        let rows = if self.stacked { n.max(1) } else { 1 };
        let total = 9 + self.gap + text_h + self.gap + warn_h + self.pad + 20 * rows + 4 * (rows - 1);
        self.top = (h - total) / 2;
        self.msg_y = self.top + 9 + self.gap;
        let by = self.top + 9 + self.gap + text_h + self.gap + warn_h + self.pad;
        let mut v = Vec::new();
        if self.stacked {
            for (i, (id, text, bwid)) in self.buttons.iter().enumerate() {
                v.push(Widget::button(*id, (w - bwid) / 2, by + 24 * i as i32, *bwid, text));
            }
        } else {
            let bw: i32 = self.buttons.iter().map(|b| b.2).sum::<i32>() + self.space * (n - 1).max(0);
            let mut x = (w - bw) / 2;
            for (id, text, bwid) in &self.buttons {
                v.push(Widget::button(*id, x, by, *bwid, text));
                x += bwid + self.space;
            }
        }
        let keyboard = self.panel.keyboard;
        self.panel = Panel::new(v);
        self.panel.keyboard = keyboard;
    }

    pub fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, o: &Options, mouse: Option<(f32, f32)>, time: f64) {
        self.layout(gui);
        gui.next_layer();
        widgets::menu_background(gui, o.i("menuBackgroundBlurriness"), false);
        gui.next_layer();
        let cx = gui.width as i32 / 2;
        let title = styled(&self.title, &gui.lang);
        widgets::centered(gui, q, &title, cx, self.top, 0xffff_ffff);
        for (k, l) in self.lines.iter().enumerate() {
            widgets::centered(gui, q, l, cx, self.msg_y + 9 * k as i32, 0xffff_ffff);
        }
        if self.warning.is_some() {
            let y = self.top + 9 + self.gap + 9 * self.lines.len() as i32 + self.gap;
            widgets::centered(gui, q, &self.warn_line, cx, y, 0xffff_cccc);
        }
        self.panel.draw(gui, q, mouse, time);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Game alert: title 70, message 90, button clamped between h/6 + 96 and h − 24.
    #[test]
    fn alert_rows_follow_the_game() {
        assert_eq!(Column::alert_rows(240, 1), (70, 90, 136));
        assert_eq!(Column::alert_rows(240, 10), (70, 90, 192));
        assert_eq!(Column::alert_rows(240, 20), (70, 90, 216));
        // Tiny window: lower bound above the upper one, the upper one wins.
        assert_eq!(Column::alert_rows(100, 1), (70, 90, 76));
    }

    #[test]
    fn lan_announcements() {
        assert_eq!(parse_lan("[MOTD]A world[/MOTD][AD]51234[/AD]"), Some(("A world".into(), "51234".into())));
        assert_eq!(parse_lan("[/MOTD][AD]1[/AD]"), Some(("missing no".into(), "1".into())));
        assert_eq!(parse_lan("[MOTD]x[/MOTD]"), None);
        assert_eq!(parse_lan("[MOTD]x[/MOTD][/MOTD][AD]1[/AD]"), None);
        assert_eq!(parse_lan("[MOTD]x[AD]1[/AD]"), None);
    }
}
