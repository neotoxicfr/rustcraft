//! Multiplayer screen (`docs/specs/title-screen-servers.md` § 2–3): `servers.dat` list
//! queried by at most 5 threads, LAN worlds (multicast), and the "add / edit",
//! "direct connection" and delete confirmation screens.

use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::Duration;

use crossbeam_channel::{Receiver, Sender};
use rc_client::status::{self, Favicon, PingOutcome, ServerAddress, StatusError};
use rc_text::{Component, StyledChar};
use winit::keyboard::{KeyCode, ModifiersState};

use crate::gui::{Gui, TextContext};
use crate::options::Options;
use crate::servers::{IconUpdate, PackStatus, ServerEntry, ServerList};
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

/// Optimisation switches of the screen (on unless `0`), read when it opens and kept by a refresh.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Switches {
    /// An icon is decoded when its bytes change, not on every frame
    /// (`RUSTCRAFT_OPT_FAVICON_DECODE_ON_CHANGE`).
    decode_icon_on_change: bool,
    /// The texts of a row are kept from one frame to the next
    /// (`RUSTCRAFT_OPT_SERVER_LIST_ROW_TEXT_CACHE`).
    keep_row_texts: bool,
    /// The icons received in one frame are written to `servers.dat` in one pass
    /// (`RUSTCRAFT_OPT_SERVER_LIST_BATCHED_SAVE`).
    batched_icon_save: bool,
    /// Status queries run on five threads kept for the session, like the game, instead of five
    /// new threads per opening of the screen (`RUSTCRAFT_OPT_STATUS_PINGER_POOL`).
    session_status_threads: bool,
}

impl Switches {
    fn from_environment() -> Switches {
        Switches {
            decode_icon_on_change: rc_data::opt_switch!("RUSTCRAFT_OPT_FAVICON_DECODE_ON_CHANGE"),
            keep_row_texts: rc_data::opt_switch!("RUSTCRAFT_OPT_SERVER_LIST_ROW_TEXT_CACHE"),
            batched_icon_save: rc_data::opt_switch!("RUSTCRAFT_OPT_SERVER_LIST_BATCHED_SAVE"),
            session_status_threads: rc_data::opt_switch!("RUSTCRAFT_OPT_STATUS_PINGER_POOL"),
        }
    }
}

// --- Queries ---

type PingResult = (u64, Result<PingOutcome, StatusError>);

/// Tooltip of a row: text lines and hovered rectangle.
type Tooltip = (Vec<Vec<StyledChar>>, (i32, i32, i32, i32));

/// One status query: the address to ask and the screen that waits for the answer.
struct PingJob {
    id: u64,
    address: String,
    answers: Sender<PingResult>,
    /// Raised when the asking screen is gone: a query that has not started is skipped.
    abandoned: Arc<AtomicBool>,
}

/// Starts five threads that answer the queries sent to the returned queue, in arrival order. They
/// stop once the queue has no sender left.
fn start_status_threads() -> Sender<PingJob> {
    let (queue, jobs) = crossbeam_channel::unbounded::<PingJob>();
    for i in 0..PING_THREADS {
        let jobs = jobs.clone();
        let started = std::thread::Builder::new().name(format!("status-{i}")).spawn(move || {
            while let Ok(job) = jobs.recv() {
                if !job.abandoned.load(Ordering::Relaxed) {
                    // The screen may have gone during the query: nobody reads the answer then.
                    let _ = job.answers.send((job.id, status::ping(&job.address, PING_TIMEOUT)));
                }
            }
        });
        if let Err(e) = &started {
            eprintln!("status thread: {e}");
        }
        #[cfg(test)]
        if started.is_ok() {
            tests::count_status_thread();
        }
    }
    queue
}

/// Queue of the session's status threads, started by the first query of the session.
static SESSION_STATUS_QUEUE: LazyLock<Sender<PingJob>> = LazyLock::new(start_status_threads);

/// Threads that answer the queries of a screen.
enum StatusThreads {
    /// The five threads of the session, shared by every opening of the screen.
    Session,
    /// Threads fed by this queue alone: they stop with it.
    Own(Sender<PingJob>),
}

/// Status queries of one opening of the screen. Its answers come back on a channel of its own:
/// an answer to a previous opening, whose query numbers start again at 1, cannot be taken for one
/// of this opening.
struct Pinger {
    threads: StatusThreads,
    answers: Sender<PingResult>,
    results: Receiver<PingResult>,
    abandoned: Arc<AtomicBool>,
}

impl Pinger {
    fn start(session_threads: bool) -> Pinger {
        Pinger::on(if session_threads { StatusThreads::Session } else { StatusThreads::Own(start_status_threads()) })
    }

    fn on(threads: StatusThreads) -> Pinger {
        let (answers, results) = crossbeam_channel::unbounded();
        Pinger { threads, answers, results, abandoned: Arc::new(AtomicBool::new(false)) }
    }

    fn submit(&self, id: u64, address: &str) {
        let queue = match &self.threads {
            StatusThreads::Session => &*SESSION_STATUS_QUEUE,
            StatusThreads::Own(queue) => queue,
        };
        let job = PingJob {
            id,
            address: address.to_owned(),
            answers: self.answers.clone(),
            abandoned: Arc::clone(&self.abandoned),
        };
        let _ = queue.send(job);
    }
}

impl Drop for Pinger {
    fn drop(&mut self) {
        // The queries still waiting are skipped; a thread finishes the one it is running.
        self.abandoned.store(true, Ordering::Relaxed);
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

/// How long the listening thread waits for an announcement before it looks at its stop flag: a
/// closed screen frees its socket and its thread within this delay.
const LAN_READ_TIMEOUT: Duration = Duration::from_millis(250);

/// Socket that hears the LAN announcements on `bind`: port 4445, member of the group
/// 224.0.2.60. The address is bound as reusable, like the game's multicast socket: a second
/// client on the machine, or a screen reopened while the previous listener still holds its
/// socket, binds the port too instead of failing (and finding no LAN world).
fn lan_socket(bind: Ipv4Addr) -> std::io::Result<UdpSocket> {
    use socket2::{Domain, Protocol, Socket, Type};
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    socket.bind(&SocketAddrV4::new(bind, rc_sim::lan::announce::PORT).into())?;
    let socket: UdpSocket = socket.into();
    socket.join_multicast_v4(&rc_sim::lan::announce::GROUP, &bind)?;
    socket.set_read_timeout(Some(LAN_READ_TIMEOUT))?;
    Ok(socket)
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
        let sock = lan_socket(rc_sim::lan::announce::bind_addr()).ok()?;
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
    /// The icon bytes have been decoded once and are a valid picture: a row left without an
    /// atlas slot does not decode them again on every frame to know it.
    icon_known_valid: bool,
    /// Texts as drawn, kept until the row, the language, the fonts or the row width change.
    texts: Option<(RowTextKey, RowTexts)>,
}

impl Row {
    fn new(entry: ServerEntry) -> Row {
        Row { entry, ping: Ping::Initial, id: 0, icon_known_valid: false, texts: None }
    }

    /// New state of the query; the kept texts showed the previous one.
    fn set_ping(&mut self, ping: Ping) {
        self.ping = ping;
        self.texts = None;
    }

    /// Builds the row's texts for `key`, unless the ones kept (`keep`) were built for it.
    fn refresh_texts(&mut self, key: RowTextKey, keep: bool, m: &dyn rc_text::Metrics, lang: &rc_text::Lang) {
        if keep && self.texts.as_ref().is_some_and(|(kept_for, _)| *kept_for == key) {
            return;
        }
        #[cfg(test)]
        tests::count_row_texts_built();
        let texts = RowTexts::new(m, lang, &self.entry.name, &self.ping, key.content_width);
        self.texts = Some((key, texts));
    }

    /// Sends the row's query, at its first drawing (§ 3.1).
    fn start_query(&mut self, next_id: &mut u64, pinger: &Pinger) {
        self.id = *next_id;
        *next_id += 1;
        pinger.submit(self.id, &self.entry.ip);
        self.set_ping(Ping::Pinging);
    }

    /// Answer to the row's query: its new state. Returns `true` when the answer brought an icon
    /// that differs from the known one: the entry keeps it, and it is to be written to
    /// `servers.dat`.
    fn take_answer(&mut self, answer: Result<PingOutcome, StatusError>) -> bool {
        let mut new_icon = false;
        let ping = match answer {
            Ok(PingOutcome::Modern(pinged)) => {
                let reply = pinged.status;
                if let Some(favicon) = reply.favicon
                    && self.entry.icon.as_deref() != Some(&favicon.png[..])
                {
                    self.entry.icon = Some(favicon.png);
                    self.icon_known_valid = false;
                    new_icon = true;
                }
                Ping::Done(Box::new(Status {
                    motd: status::json_component(&reply.description).unwrap_or_else(|| Component::text("")),
                    players: reply.players.as_ref().map(|players| (players.online, players.max)),
                    sample: reply.players.as_ref().map_or_else(Vec::new, sample_lines),
                    version: reply.version.map_or_else(String::new, |version| version.name),
                    compatible: pinged.compatible,
                    latency: pinged.latency_ms,
                }))
            }
            Ok(PingOutcome::Legacy(legacy)) => Ping::Done(Box::new(Status {
                motd: Component::text(legacy.motd),
                players: Some((legacy.online, legacy.max)),
                sample: Vec::new(),
                version: legacy.version,
                compatible: false,
                latency: 0,
            })),
            Err(StatusError::UnknownHost) => Ping::Failed("multiplayer.status.cannot_resolve"),
            Err(_) => Ping::Failed("multiplayer.status.cannot_connect"),
        };
        self.set_ping(ping);
        new_icon
    }
}

/// Lines of the players tooltip as kept in [`Status::sample`]: a name, or a marker (`\u{1}`
/// prefix) for an anonymous player and for the "and N more" line, translated when shown.
fn sample_lines(players: &status::Players) -> Vec<String> {
    let mut lines: Vec<String> = players
        .sample
        .iter()
        .map(|player| if player.is_anonymous() { "\u{1}anonymous".into() } else { player.name.clone() })
        .collect();
    if (players.sample.len() as i32) < players.online {
        lines.push(format!("\u{1}more:{}", players.online - players.sample.len() as i32));
    }
    lines
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
    switches: Switches,
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
        Multiplayer::open(dir, Switches::from_environment())
    }

    /// The screen with its query threads and its LAN listener started.
    fn open(dir: PathBuf, switches: Switches) -> Multiplayer {
        Multiplayer::with_parts(dir, switches, Pinger::start(switches.session_status_threads), Lan::start())
    }

    fn with_parts(dir: PathBuf, switches: Switches, pinger: Pinger, lan: Option<Lan>) -> Multiplayer {
        let list = ServerList::load(&dir);
        let rows = list.visible.iter().cloned().map(Row::new).collect();
        Multiplayer {
            panel: Panel::default(),
            size: (-1, -1),
            dir,
            switches,
            list,
            rows,
            lan,
            lan_worlds: Vec::new(),
            pinger,
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
            self.rows.push(Row::new(last.clone()));
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
            r.set_ping(Ping::Initial);
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

    /// Writes the icons just received by these rows to `servers.dat`: the file is reread, the
    /// entry with the row's name and address takes the icon, and the file is rewritten. One pass
    /// for all the rows of the frame; the old path rereads and rewrites the file for each row.
    fn store_new_icons(&self, rows_with_new_icon: &[usize]) {
        let updates: Vec<IconUpdate> = rows_with_new_icon
            .iter()
            .map(|&i| &self.rows[i].entry)
            .map(|entry| IconUpdate { name: &entry.name, ip: &entry.ip, icon: entry.icon.as_deref() })
            .collect();
        let per_write = if self.switches.batched_icon_save { updates.len().max(1) } else { 1 };
        for batch in updates.chunks(per_write) {
            if let Err(e) = ServerList::update_icons(&self.dir, batch) {
                eprintln!("servers.dat: {e}");
            }
        }
    }

    /// Answers arrived and LAN worlds discovered.
    pub fn poll(&mut self) {
        let mut rows_with_new_icon = Vec::new();
        while let Ok((id, answer)) = self.pinger.results.try_recv() {
            let Some(i) = self.rows.iter().position(|row| row.id == id) else { continue };
            if self.rows[i].take_answer(answer) {
                rows_with_new_icon.push(i);
            }
        }
        self.store_new_icons(&rows_with_new_icon);
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
        // The listener is told to stop before its successor binds the port: both hold it for a
        // quarter of a second at most.
        self.lan = None;
        *self = Multiplayer::open(self.dir.clone(), self.switches);
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
        let lang = std::sync::Arc::clone(&gui.lang);
        let ms = (time * 1000.0) as u64;
        let hide_ip = o.b("hideServerAddress");
        let focused = self.panel.focus.is_none();
        let Multiplayer { panel, rows, lan_worlds, pinger, next_id, slots, switches, .. } = self;
        let nrows = rows.len();
        let mut tooltip: Option<Tooltip> = None;
        let mut icon_rejected = false;
        let selected = panel.list.as_ref().and_then(|l| l.selected);
        panel.draw_with(gui, q, mouse, time, &mut |gui, q, l| {
            let hovered = mouse.and_then(|m| l.entry_under(m));
            let frame = RowFrame { lang: &lang, ms, text_context: gui.text_context(), switches: *switches };
            let mut first_visible = None;
            for i in 0..l.entries.len() {
                let Some((left, top, rw, rh)) = l.entry_rect(i) else { continue };
                if top + rh < l.y || top > l.y + l.h {
                    continue;
                }
                let first_visible = *first_visible.get_or_insert(i);
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
                        row.start_query(next_id, pinger);
                    }
                    let spot = RowSpot {
                        index: i,
                        x,
                        y,
                        content_width: lc,
                        icon_slot: icon_slot(i, first_visible, slots.len()),
                        hovered: hovered == Some(i),
                        mouse,
                    };
                    let (tip, rejected) = draw_server_row(gui, q, &frame, row, slots, &spot);
                    icon_rejected |= rejected;
                    if tip.is_some() {
                        tooltip = tip;
                    }
                    if spot.hovered {
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
        if icon_rejected {
            // Like the game: an icon that cannot be shown is forgotten and the list saved.
            self.save();
        }
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

/// What all the rows of one frame share.
struct RowFrame<'a> {
    lang: &'a rc_text::Lang,
    /// Clock of the status animation, in milliseconds.
    ms: u64,
    text_context: TextContext,
    switches: Switches,
}

/// Where a server row is drawn and what the mouse does there.
struct RowSpot {
    /// Rank of the row in the list.
    index: usize,
    /// Top left corner of the row's content, and its width.
    x: i32,
    y: i32,
    content_width: i32,
    /// Atlas slot the row's icon may use this frame.
    icon_slot: Option<usize>,
    /// The mouse is over this row.
    hovered: bool,
    mouse: Option<(f32, f32)>,
}

/// A saved server's row (§ 2.3): icon, name, MOTD, status. Returns the tooltip under the mouse,
/// if any, and whether the row's icon was rejected (the caller saves the list).
fn draw_server_row(
    gui: &mut Gui,
    q: &wgpu::Queue,
    frame: &RowFrame,
    row: &mut Row,
    slots: &mut [Option<Vec<u8>>],
    spot: &RowSpot,
) -> (Option<Tooltip>, bool) {
    let icon_rejected = draw_row_icon(gui, q, row, slots, frame.switches.decode_icon_on_change, spot);
    let key = RowTextKey { context: frame.text_context, content_width: spot.content_width };
    row.refresh_texts(key, frame.switches.keep_row_texts, gui, frame.lang);
    (draw_row_texts(gui, q, frame, row, spot), icon_rejected)
}

// --- Row icon ---

/// Atlas sprites reserved for the server icons, by slot.
static ICON_SLOT_SPRITES: LazyLock<Vec<String>> =
    LazyLock::new(|| (0..rc_assets::gui::SERVER_ICON_SLOTS).map(|slot| format!("dyn/server_icon/{slot}")).collect());
const UNKNOWN_SERVER_SPRITE: &str = "tex/misc/unknown_server";

/// What a row shows as its icon this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IconShown {
    /// The picture, written in this atlas slot.
    InSlot(usize),
    /// The "unknown server" picture: no icon, or no slot to show it in.
    Unknown,
    /// The icon bytes are not a 64 × 64 picture: to be removed from the entry.
    Rejected,
}

/// Atlas slot for the icon of row `index` when `first_visible` is the first row on screen: the
/// rank modulo the number of slots, so that a list longer than the slots still shows the icons
/// of the rows on screen (the game has one texture per entry). A row a whole turn of slots below
/// the first one on screen would overwrite the slot of a row drawn this frame: it gets none.
fn icon_slot(index: usize, first_visible: usize, slot_count: usize) -> Option<usize> {
    (index - first_visible < slot_count).then(|| index % slot_count)
}

/// Pixels of a row's icon, or `None` if it is not a 64 × 64 picture.
fn decode_icon(png: &[u8]) -> Option<Vec<u8>> {
    #[cfg(test)]
    tests::count_icon_decoding();
    status::icon_pixels(png)
}

/// The icon is decoded only when the slot does not hold it yet: an unchanged icon costs one
/// comparison of its bytes per frame instead of a copy and a PNG decoding. `upload` writes
/// pixels into a slot of the atlas and tells whether it could.
fn show_icon_decoding_on_change(
    row: &mut Row,
    slot: Option<usize>,
    slots: &mut [Option<Vec<u8>>],
    upload: &mut dyn FnMut(usize, &[u8]) -> bool,
) -> IconShown {
    let Some(png) = row.entry.icon.as_deref() else { return IconShown::Unknown };
    let Some(slot) = slot else {
        // No slot to draw from: only the validity matters, and one decoding settles it.
        if !row.icon_known_valid {
            if decode_icon(png).is_none() {
                return IconShown::Rejected;
            }
            row.icon_known_valid = true;
        }
        return IconShown::Unknown;
    };
    if slots[slot].as_deref() == Some(png) {
        return IconShown::InSlot(slot);
    }
    let Some(pixels) = decode_icon(png) else { return IconShown::Rejected };
    if upload(slot, &pixels) {
        slots[slot] = Some(png.to_vec());
    }
    if slots[slot].is_some() { IconShown::InSlot(slot) } else { IconShown::Unknown }
}

/// Old path: the PNG is copied and decoded on every frame, then compared with the slot.
fn show_icon_decoding_every_frame(
    row: &Row,
    slot: Option<usize>,
    slots: &mut [Option<Vec<u8>>],
    upload: &mut dyn FnMut(usize, &[u8]) -> bool,
) -> IconShown {
    let Some(png) = &row.entry.icon else { return IconShown::Unknown };
    #[cfg(test)]
    tests::count_icon_decoding();
    let Some(pixels) = Favicon::from_png(png.clone()).and_then(|icon| icon.rgba()) else { return IconShown::Rejected };
    let Some(slot) = slot else { return IconShown::Unknown };
    if slots[slot].as_deref() != Some(&png[..]) && upload(slot, &pixels) {
        slots[slot] = Some(png.clone());
    }
    if slots[slot].is_some() { IconShown::InSlot(slot) } else { IconShown::Unknown }
}

/// The row's icon: its atlas slot once the picture is written there, the "unknown server"
/// picture otherwise. An icon that is not a 64 × 64 picture is removed from the entry, like the
/// game; returns `true` then.
fn draw_row_icon(
    gui: &mut Gui,
    q: &wgpu::Queue,
    row: &mut Row,
    slots: &mut [Option<Vec<u8>>],
    decode_on_change: bool,
    spot: &RowSpot,
) -> bool {
    let mut upload = |slot: usize, pixels: &[u8]| gui.write_sprite(q, &ICON_SLOT_SPRITES[slot], (64, 64), pixels);
    let shown = if decode_on_change {
        show_icon_decoding_on_change(row, spot.icon_slot, slots, &mut upload)
    } else {
        show_icon_decoding_every_frame(row, spot.icon_slot, slots, &mut upload)
    };
    let sprite = match shown {
        IconShown::InSlot(slot) => ICON_SLOT_SPRITES[slot].as_str(),
        IconShown::Unknown | IconShown::Rejected => UNKNOWN_SERVER_SPRITE,
    };
    gui.sprite(sprite, spot.x as f32, spot.y as f32, 32.0, 32.0, 0xffff_ffff);
    let rejected = shown == IconShown::Rejected;
    if rejected {
        row.entry.icon = None;
    }
    rejected
}

// --- Row texts and status ---

/// What the texts of a row were built for: another language, other fonts or another width
/// build them again.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RowTextKey {
    context: TextContext,
    content_width: i32,
}

/// Texts of a server row as drawn (§ 2.3). Built when the row changes instead of on every
/// frame: the MOTD is flattened and cut into lines, the status translated and measured.
#[derive(Debug, Clone, PartialEq)]
struct RowTexts {
    name: Vec<StyledChar>,
    /// MOTD, or failure message in red: the two lines shown.
    motd_lines: Vec<Vec<StyledChar>>,
    /// Text left of the status picture: players, or the version of an incompatible server.
    status: Vec<StyledChar>,
    status_width: i32,
}

impl RowTexts {
    fn new(m: &dyn rc_text::Metrics, lang: &rc_text::Lang, name: &str, ping: &Ping, content_width: i32) -> RowTexts {
        let motd = match ping {
            Ping::Done(status) => styled(&status.motd, lang),
            Ping::Failed(key) => styled(&colored(&tr(lang, key, &[]), 0xff_0000), lang),
            Ping::Initial | Ping::Pinging => Vec::new(),
        };
        let mut motd_lines = rc_text::wrap(m, &motd, (content_width - 34) as f32);
        motd_lines.truncate(2);
        let status = match ping {
            Ping::Done(status) if !status.compatible => styled(&colored(&status.version, 0xff_5555), lang),
            Ping::Done(status) => player_count(status.players, lang),
            Ping::Initial | Ping::Pinging | Ping::Failed(_) => Vec::new(),
        };
        let status_width = rc_text::width(m, &status);
        RowTexts { name: chars(name), motd_lines, status, status_width }
    }
}

/// "online/max", numbers in gray and slash in dark gray; `???` when the answer had no players.
fn player_count(players: Option<(i32, i32)>, lang: &rc_text::Lang) -> Vec<StyledChar> {
    let Some((online, max)) = players else { return styled(&colored("???", 0x55_5555), lang) };
    let number = |n: i32| rc_text::Arg::Component(colored(&n.to_string(), 0xaa_aaaa));
    let mut count = Component::translate("multiplayer.status.player_count", vec![number(online), number(max)]);
    count.style.color = Some(0x55_5555);
    styled(&count, lang)
}

const PINGING_SPRITES: [&str; 5] = [
    "server_list/pinging_1",
    "server_list/pinging_2",
    "server_list/pinging_3",
    "server_list/pinging_4",
    "server_list/pinging_5",
];
/// Signal bars, from one (slow) to five (fast).
const PING_SPRITES: [&str; 5] =
    ["server_list/ping_1", "server_list/ping_2", "server_list/ping_3", "server_list/ping_4", "server_list/ping_5"];

/// Status picture of a row (10 × 8): a back-and-forth animation while the query runs, shifted by
/// the row's rank, then the signal bars of the latency, or the unreachable or incompatible picture.
fn status_sprite(ping: &Ping, index: usize, ms: u64) -> &'static str {
    match ping {
        Ping::Initial | Ping::Pinging => {
            let mut step = ((ms / 100) as usize + 2 * index) & 7;
            if step > 4 {
                step = 8 - step;
            }
            PINGING_SPRITES[step]
        }
        Ping::Failed(_) => "server_list/unreachable",
        Ping::Done(status) if !status.compatible => "server_list/incompatible",
        Ping::Done(status) => {
            let bars = match status.latency {
                0..150 => 5,
                150..300 => 4,
                300..600 => 3,
                600..1000 => 2,
                _ => 1,
            };
            PING_SPRITES[bars - 1]
        }
    }
}

/// Tooltip of the status picture.
fn status_tooltip(ping: &Ping, lang: &rc_text::Lang) -> String {
    match ping {
        Ping::Initial | Ping::Pinging => tr(lang, "multiplayer.status.pinging", &[]),
        Ping::Failed(_) => tr(lang, "multiplayer.status.no_connection", &[]),
        Ping::Done(status) if !status.compatible => tr(lang, "multiplayer.status.incompatible", &[]),
        Ping::Done(status) => tr(lang, "multiplayer.status.ping", &[status.latency.to_string()]),
    }
}

/// Name, MOTD, status picture and status text of a server row, from its kept texts. Returns the
/// tooltip under the mouse, if any.
fn draw_row_texts(gui: &mut Gui, q: &wgpu::Queue, frame: &RowFrame, row: &Row, spot: &RowSpot) -> Option<Tooltip> {
    let (_, texts) = row.texts.as_ref()?;
    let (x, y) = (spot.x, spot.y);
    gui.text(q, &texts.name, (x + 35) as f32, (y + 1) as f32, 0xffff_ffff, true, 0);
    for (k, line) in texts.motd_lines.iter().enumerate() {
        gui.text(q, line, (x + 35) as f32, (y + 12 + 9 * k as i32) as f32, GRAY, true, 0);
    }
    // State: 10 × 8 picture and text to its left.
    let picture_x = x + spot.content_width - 15;
    let sprite = status_sprite(&row.ping, spot.index, frame.ms);
    gui.sprite(sprite, picture_x as f32, y as f32, 10.0, 8.0, 0xffff_ffff);
    let text_x = picture_x - texts.status_width - 5;
    if !texts.status.is_empty() {
        gui.text(q, &texts.status, text_x as f32, (y + 1) as f32, GRAY, true, 0);
    }
    let mouse = spot.mouse.filter(|_| spot.hovered)?;
    // Game tooltip zones: bounds inclusive, in whole pixels (10 × 8 picture → 11 × 9).
    let (mx, my) = (mouse.0.floor() as i32, mouse.1.floor() as i32);
    let inside = |rx: i32, ry: i32, rw: i32, rh: i32| mx >= rx && mx <= rx + rw && my >= ry && my <= ry + rh;
    if inside(picture_x, y, 10, 8) {
        return Some((vec![chars(&status_tooltip(&row.ping, frame.lang))], (picture_x, y, 10, 8)));
    }
    if let Ping::Done(status) = &row.ping
        && inside(text_x, y, texts.status_width, 8)
        && !status.sample.is_empty()
    {
        let lines = status.sample.iter().map(|line| sample_line_shown(line, frame.lang)).collect();
        return Some((lines, (text_x, y, texts.status_width, 9)));
    }
    None
}

/// A line of the players tooltip as shown: the markers of [`sample_lines`] translated.
fn sample_line_shown(line: &str, lang: &rc_text::Lang) -> Vec<StyledChar> {
    match line.strip_prefix('\u{1}') {
        Some("anonymous") => chars(&tr(lang, "multiplayer.status.anonymous_player", &[])),
        Some(more) => {
            let count = more.strip_prefix("more:").unwrap_or("0").to_owned();
            chars(&tr(lang, "multiplayer.status.and_more", &[count]))
        }
        None => chars(line),
    }
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
mod tests;
