//! Resource packs imposed by the server (`docs/specs/resource-packs.md` § 5): prompt,
//! per-server preference (`servers.dat`), download into `downloads/` with SHA-1 check,
//! states sent, application by reloading the resources, removal.

use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use rc_text::Component;
use winit::event::{ElementState, MouseButton, WindowEvent};
use winit::keyboard::KeyCode;

use crate::App;
use crate::options::Options;
use crate::server_list::Column;
use crate::servers::{PackStatus, ServerList};
use crate::widgets;

/// States of the `resource_pack` reply (`protocol-supplements.md` § 9).
mod status {
    pub const LOADED: i32 = 0;
    pub const DECLINED: i32 = 1;
    pub const FAILED_DOWNLOAD: i32 = 2;
    pub const ACCEPTED: i32 = 3;
    pub const DOWNLOADED: i32 = 4;
    pub const FAILED_RELOAD: i32 = 6;
    pub const DISCARDED: i32 = 7;
}

/// Maximum size of a downloaded pack.
const MAX_SIZE: u64 = 262_144_000;
/// Files kept in `downloads/`.
const KEEP: usize = 20;

/// Prompt status of the session.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum Prompt {
    #[default]
    Pending,
    Allowed,
    Declined,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Download {
    Requested,
    Pending,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Activation {
    Inactive,
    Pending,
    Active,
}

/// Reason for a pack's removal; the terminal reply sent when it leaves the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Removal {
    DownloadFailed,
    ActivationFailed,
    Declined,
    Discarded,
    /// Removed or replaced by the server: no reply.
    Server,
}

impl Removal {
    fn response(self) -> Option<i32> {
        match self {
            Removal::DownloadFailed => Some(status::FAILED_DOWNLOAD),
            Removal::ActivationFailed => Some(status::FAILED_RELOAD),
            Removal::Declined => Some(status::DECLINED),
            Removal::Discarded => Some(status::DISCARDED),
            Removal::Server => None,
        }
    }
}

#[derive(Debug, Clone)]
struct Pack {
    id: u128,
    url: String,
    /// Valid SHA-1 fingerprint (40 hexadecimal digits, lowercase), otherwise `None`.
    hash: Option<String>,
    path: Option<PathBuf>,
    removal: Option<Removal>,
    download: Download,
    activation: Activation,
    accepted: bool,
}

impl Pack {
    fn removed(&self) -> bool {
        self.removal.is_some()
    }

    fn remove(&mut self, r: Removal) {
        if self.removal.is_none() {
            self.removal = Some(r);
        }
    }
}

/// Result of a batch of downloads.
struct Batch {
    done: Vec<(u128, PathBuf)>,
    failed: Vec<u128>,
}

/// Reload requested for the server's packs: packs to load and to unload, expected reload
/// number.
struct Waiting {
    generation: u64,
    load: Vec<u128>,
    unload: Vec<u128>,
}

/// Prompt open: pending requests (id, URL, fingerprint), screen.
struct PromptScreen {
    requests: Vec<(u128, String, String)>,
    required: bool,
    column: Column,
    opened: Instant,
}

const YES: u32 = 1;
const NO: u32 = 2;

/// State of the server's packs for the current connection.
#[derive(Default)]
pub(crate) struct ServerPacks {
    packs: Vec<Pack>,
    status: Prompt,
    /// Entry of the server list (name, address) whose preference is tracked.
    entry: Option<(String, String)>,
    pref: PackStatus,
    prompt: Option<PromptScreen>,
    batch: Option<crossbeam_channel::Receiver<Batch>>,
    waiting: Option<Waiting>,
    /// The `downloads/` folder has been cleaned (once per launch).
    vacuumed: bool,
    /// Notifications of the batches in progress, number of the last batch.
    notices: Vec<Notice>,
    serial: u64,
    /// World pack of the singleplayer world being loaded: expected reload number.
    world: Option<u64>,
}

/// Does the world pack load? Readable `pack.mcmeta` meta (otherwise immediate failure, like the game
/// which discards a pack without a valid meta even before reloading).
fn world_pack_valid(path: &Path) -> bool {
    rc_assets::packs::read_root(path, "pack.mcmeta").and_then(|b| rc_assets::packs::parse_meta(&b)).is_some()
}

/// Fingerprint taken into account: 40 hexadecimal digits.
fn parse_hash(h: &str) -> Option<String> {
    (h.len() == 40 && h.bytes().all(|b| b.is_ascii_hexdigit())).then(|| h.to_ascii_lowercase())
}

/// UUID with dashes (name of the subfolder).
fn dashed(id: u128) -> String {
    let h = format!("{id:032x}");
    format!("{}-{}-{}-{}-{}", &h[..8], &h[8..12], &h[12..16], &h[16..20], &h[20..])
}

fn sha1_file(path: &Path) -> Option<String> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut ctx = ring::digest::Context::new(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY);
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = f.read(&mut buf).ok()?;
        if n == 0 {
            break;
        }
        ctx.update(&buf[..n]);
    }
    Some(hex(ctx.finish().as_ref()))
}

fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// `downloads/` folder of the game folder.
pub(crate) fn downloads_dir() -> PathBuf {
    crate::servers::game_dir().join("downloads")
}

/// Headers sent with every request (§ 5.2).
fn headers(name: &str, uuid: u128) -> Vec<(&'static str, String)> {
    vec![
        ("X-Minecraft-Username", name.to_owned()),
        ("X-Minecraft-UUID", format!("{uuid:032x}")),
        ("X-Minecraft-Version", "26.3".into()),
        ("X-Minecraft-Version-ID", "26.3".into()),
        // Format written "major.minor", like the game ("97.1").
        ("X-Minecraft-Pack-Format", format!("{}.{}", rc_assets::packs::RESOURCE_FORMAT.0, rc_assets::packs::RESOURCE_FORMAT.1)),
        ("User-Agent", "Minecraft Java/26.3".into()),
    ]
}

/// Progress of a batch, sent by the download thread to the notification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Progress {
    /// Start of a file (reused or downloaded).
    Start,
    /// Reply received: announced size, if any.
    Size(Option<u64>),
    /// Bytes received so far for the current file.
    Bytes(u64),
    /// End of a file: succeeded or not.
    Finished(bool),
}

/// Readable bytes: "512 B", "1.5 KB", "3.2 MB" (one decimal rounded half up,
/// power of 1024 drawn from the logarithm like the game).
fn human_readable(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} B");
    }
    let exp = ((bytes as f64).ln() / 1024f64.ln()) as i32;
    let prefix = ['K', 'M', 'G', 'T', 'P', 'E'][(exp - 1).clamp(0, 5) as usize];
    // `bytes / 1024^exp` and its tenfold are exact in f64 (division by a power of 2):
    // the half-up rounding is therefore exact.
    let tenths = (bytes as f64 / 1024f64.powi(exp) * 10.0 + 0.5).floor() as u64;
    format!("{}.{} {prefix}B", tenths / 10, tenths % 10)
}

/// Progress message of the current file.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoticeLine {
    None,
    Percent(u64),
    Bytes(u64),
}

/// What the notification must do after events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoticeChange {
    Nothing,
    Show,
    Hide,
    /// Successful batch whose notification was requested in the same frame: hide the one that
    /// already exists, otherwise create it (it then stays its whole duration).
    HideOrShow,
}

/// Notification of a download batch: "Downloading pack n/N", progress in % or
/// in bytes, failures at the end; hidden at once if everything succeeded.
struct Notice {
    rx: crossbeam_channel::Receiver<Progress>,
    /// Batch number (id of the notification).
    serial: u64,
    total: usize,
    count: usize,
    failed: usize,
    size: Option<u64>,
    line: NoticeLine,
    /// Failures displayed (final title): the notification is no longer updated.
    title_failed: bool,
    done: bool,
}

impl Notice {
    fn new(rx: crossbeam_channel::Receiver<Progress>, serial: u64, total: usize) -> Notice {
        Notice {
            rx,
            serial,
            total,
            count: 0,
            failed: 0,
            size: None,
            line: NoticeLine::None,
            title_failed: false,
            done: false,
        }
    }

    /// Events received since the previous frame applied; change to make on screen.
    /// Game quirk reproduced: the update of the notification goes through the main
    /// thread (once per frame), but the final hiding is done at once in the download
    /// thread. If the notification has not been created yet (batch finished in the frame where it
    /// started, case of the packs already in the cache), the hiding finds nothing and the notification
    /// then appears for its whole duration.
    fn drain(&mut self) -> NoticeChange {
        let mut change = NoticeChange::Nothing;
        let mut shown = false;
        loop {
            match self.rx.try_recv() {
                Ok(p) => match self.apply(p) {
                    NoticeChange::Nothing => {}
                    c => {
                        shown |= c == NoticeChange::Show;
                        change = c;
                    }
                },
                // Thread stopped without finishing: notification forgotten.
                Err(crossbeam_channel::TryRecvError::Disconnected) => {
                    self.done = true;
                    break;
                }
                Err(crossbeam_channel::TryRecvError::Empty) => break,
            }
        }
        if change == NoticeChange::Hide && shown { NoticeChange::HideOrShow } else { change }
    }

    /// An event applied.
    fn apply(&mut self, p: Progress) -> NoticeChange {
        match p {
            Progress::Start => {
                self.count += 1;
                NoticeChange::Show
            }
            // Announced size zero: game quirk, the percentage divides by zero before the
            // message changes; the notification is not updated (the previous message
            // stays, none for the first file) and the thread fails (see `download`).
            Progress::Size(Some(0)) => {
                self.size = Some(0);
                NoticeChange::Nothing
            }
            Progress::Size(s) => {
                self.size = s;
                self.line = self.progress(0);
                NoticeChange::Show
            }
            Progress::Bytes(b) => {
                self.line = self.progress(b);
                NoticeChange::Show
            }
            Progress::Finished(ok) => {
                if !ok {
                    self.failed += 1;
                }
                if self.count != self.total {
                    return NoticeChange::Nothing;
                }
                self.done = true;
                if self.failed > 0 {
                    self.title_failed = true;
                    self.line = NoticeLine::None;
                    NoticeChange::Show
                } else {
                    NoticeChange::Hide
                }
            }
        }
    }

    fn progress(&self, b: u64) -> NoticeLine {
        match self.size {
            // Size zero: the thread has already failed (the game's division by zero, see `download`).
            Some(t) if t > 0 => NoticeLine::Percent(b.saturating_mul(100) / t),
            Some(_) => NoticeLine::Percent(0),
            None => NoticeLine::Bytes(b),
        }
    }

    fn title(&self) -> Component {
        let n = |v: usize| rc_text::Arg::Text(v.to_string());
        if self.title_failed {
            Component::translate("download.pack.failed", vec![n(self.failed), n(self.total)])
        } else {
            Component::translate("download.pack.title", vec![n(self.count), n(self.total)])
        }
    }

    fn message(&self) -> Option<Component> {
        match self.line {
            NoticeLine::None => None,
            NoticeLine::Percent(p) => {
                Some(Component::translate("download.pack.progress.percent", vec![rc_text::Arg::Text(p.to_string())]))
            }
            NoticeLine::Bytes(b) => {
                Some(Component::translate("download.pack.progress.bytes", vec![rc_text::Arg::Text(human_readable(b))]))
            }
        }
    }
}

/// Downloads (or reuses) a pack: `downloads/<id>/<sha1>`. `on` receives the announced size and
/// the bytes received (notification).
fn download(
    dir: &Path,
    id: u128,
    url: &str,
    hash: Option<&str>,
    hdrs: &[(&'static str, String)],
    on: &mut dyn FnMut(Progress),
) -> Result<PathBuf, String> {
    let target_dir = dir.join(dashed(id));
    let target = hash.map(|h| target_dir.join(h));
    if let (Some(t), Some(h)) = (&target, hash) {
        if t.is_file() && sha1_file(t).as_deref() == Some(h) {
            // Reused: date refreshed (cleanup order).
            if let Ok(f) = std::fs::File::options().write(true).open(t) {
                let _ = f.set_modified(std::time::SystemTime::now());
            }
            eprintln!("server pack: already downloaded file reused ({})", t.display());
            return Ok(t.clone());
        }
        if t.exists() {
            std::fs::remove_file(t).map_err(|e| format!("{} : {e}", t.display()))?;
        }
    }
    // Launcher proxy and Java headers like the game; `User-Agent` set by `hdrs`.
    let agent: ureq::Agent = rc_client::session::http_config()
        .timeout_global(Some(std::time::Duration::from_secs(120)))
        .max_redirects(10)
        .build()
        .into();
    let mut req = agent.get(url);
    for (k, v) in hdrs {
        req = req.header(*k, v.as_str());
    }
    let mut resp = req.call().map_err(|e| e.to_string())?;
    let length = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
    on(Progress::Size(length));
    // Quirk of game 26.3 reproduced: the notification computes its percentage from the size
    // announced at the start of the response; a zero size makes this computation fail, and hence the
    // download ("download failed" reply to the server).
    if length == Some(0) {
        return Err("announced size zero: progress computation failed (like the game)".into());
    }
    if length.is_some_and(|l| l > MAX_SIZE) {
        return Err(format!("announced size {} beyond {MAX_SIZE}", length.unwrap_or(0)));
    }
    std::fs::create_dir_all(&target_dir).map_err(|e| e.to_string())?;
    let tmp = target_dir.join(format!("download{}.tmp", std::process::id()));
    let result = (|| {
        let mut out = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        let mut reader = resp.body_mut().with_config().limit(MAX_SIZE + 1).reader();
        let mut ctx = ring::digest::Context::new(&ring::digest::SHA1_FOR_LEGACY_USE_ONLY);
        let mut buf = vec![0u8; 64 * 1024];
        let mut total = 0u64;
        loop {
            let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            total += n as u64;
            on(Progress::Bytes(total));
            if total > MAX_SIZE {
                return Err(format!("more than {MAX_SIZE} bytes"));
            }
            ctx.update(&buf[..n]);
            out.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        }
        let got = hex(ctx.finish().as_ref());
        if let Some(h) = hash
            && got != h
        {
            return Err(format!("fingerprint {got} instead of {h}"));
        }
        let path = target_dir.join(&got);
        drop(out);
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
        eprintln!("server pack: {total} bytes downloaded → {}", path.display());
        Ok(path)
    })();
    let _ = std::fs::remove_file(&tmp);
    result
}

/// Line of the `downloads/log.json` log.
fn log_line(dir: &Path, id: u128, url: &str, hash: Option<&str>, r: &Result<PathBuf, String>) {
    let mut v = serde_json::json!({
        "id": dashed(id),
        "url": url,
        "time": iso_now(),
    });
    if let Some(h) = hash {
        v["hash"] = h.into();
    }
    match r {
        Ok(p) => {
            let name = p.strip_prefix(dir).unwrap_or(p).to_string_lossy().into_owned();
            let size = std::fs::metadata(p).map_or(0, |m| m.len());
            v["file"] = serde_json::json!({ "name": name, "size": size });
        }
        Err(_) => v["error"] = "download_failed".into(),
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("log.json")) {
        let _ = writeln!(f, "{v}");
    }
}

/// UTC time ISO 8601 (log).
fn iso_now() -> String {
    let s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs()) as i64;
    let (days, rem) = (s.div_euclid(86_400), s.rem_euclid(86_400));
    // Days since 1970 → civil date.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem / 60 % 60, rem % 60)
}

/// Cleanup of the folder: only the `KEEP` most recent files remain, empty folders
/// removed (the log is not counted). Like the game, symbolic links are never
/// followed nor counted: only real folders and ordinary files.
fn vacuum(dir: &Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut files = Vec::new();
    let mut dirs = Vec::new();
    for e in rd.flatten() {
        if !e.file_type().is_ok_and(|t| t.is_dir()) {
            continue;
        }
        let p = e.path();
        if let Ok(inner) = std::fs::read_dir(&p) {
            for f in inner.flatten() {
                if !f.file_type().is_ok_and(|t| t.is_file()) {
                    continue;
                }
                let t = f.metadata().and_then(|m| m.modified()).unwrap_or(std::time::UNIX_EPOCH);
                files.push((t, f.path()));
            }
        }
        dirs.push(p);
    }
    files.sort_by_key(|f| std::cmp::Reverse(f.0));
    for (_, p) in files.into_iter().skip(KEEP) {
        let _ = std::fs::remove_file(p);
    }
    for d in dirs {
        let _ = std::fs::remove_dir(d);
    }
}

impl ServerPacks {
    /// Active packs of the server, in order of arrival (resource stack).
    fn active_paths(&self, ids: &[u128]) -> Vec<PathBuf> {
        ids.iter().filter_map(|id| self.packs.iter().find(|p| p.id == *id)?.path.clone()).collect()
    }

    /// Drawing of the prompt (on top of the current screen).
    pub fn draw(&mut self, gui: &mut crate::gui::Gui, q: &wgpu::Queue, o: &Options, mouse: Option<(f32, f32)>) {
        if let Some(p) = &mut self.prompt {
            p.column.draw(gui, q, o, mouse, 0.0);
        }
    }

    /// Terminal or intermediate reply.
    fn send(link: &rc_client::link::Link, id: u128, s: i32) {
        eprintln!("server pack {}: state {s}", dashed(id));
        let _ = link.send_pack_status(id, s);
    }

    fn accept(link: &rc_client::link::Link, p: &mut Pack) {
        Self::send(link, p.id, status::ACCEPTED);
        p.accepted = true;
    }

    /// Addition of a pack (replaces the one with the same id); refused status → 1.
    fn push(&mut self, link: &rc_client::link::Link, id: u128, url: String, hash: &str) {
        if self.status == Prompt::Declined {
            Self::send(link, id, status::DECLINED);
            return;
        }
        for p in self.packs.iter_mut().filter(|p| p.id == id) {
            p.remove(Removal::Server);
        }
        let mut p = Pack {
            id,
            url,
            hash: parse_hash(hash),
            path: None,
            removal: None,
            download: Download::Requested,
            activation: Activation::Inactive,
            accepted: false,
        };
        if self.status == Prompt::Allowed {
            Self::accept(link, &mut p);
        }
        self.packs.push(p);
    }

    fn allow(&mut self, link: &rc_client::link::Link) {
        self.status = Prompt::Allowed;
        for p in self.packs.iter_mut().filter(|p| !p.accepted && !p.removed()) {
            Self::accept(link, p);
        }
    }

    fn reject(&mut self) {
        self.status = Prompt::Declined;
        for p in self.packs.iter_mut().filter(|p| !p.accepted) {
            p.remove(Removal::Declined);
        }
    }
}

impl App {
    /// Start of a connection (except transfer): the entry's preference → session status.
    pub(crate) fn server_packs_connect(&mut self, entry: &crate::servers::ServerEntry) {
        let sp = &mut self.packs.server;
        sp.entry = Some((entry.name.clone(), entry.ip.clone()));
        sp.pref = entry.pack_status;
        sp.status = match entry.pack_status {
            PackStatus::Enabled => Prompt::Allowed,
            PackStatus::Disabled => Prompt::Declined,
            PackStatus::Prompt => Prompt::Pending,
        };
    }

    /// End of session (except transfer): server packs removed, status "pending".
    pub(crate) fn server_packs_disconnect(&mut self) {
        let sp = &mut self.packs.server;
        let had = !self.packs.stack.server.is_empty();
        sp.packs.clear();
        sp.prompt = None;
        sp.batch = None;
        sp.waiting = None;
        sp.status = Prompt::Pending;
        sp.entry = None;
        sp.pref = PackStatus::Prompt;
        sp.world = None;
        self.packs.stack.world = false;
        if had {
            self.packs.stack.server.clear();
            self.reload_resources();
        }
    }

    /// Opening of a singleplayer world with `resources.zip`: the pack is stacked alone in place of the
    /// server's packs (like an imposed pack, without prompt or network reply), then the resources
    /// reloaded; the result is handed to the singleplayer screens at the end of the reload.
    pub(crate) fn world_pack_push(&mut self, path: PathBuf) {
        if !world_pack_valid(&path) {
            eprintln!("world pack {}: invalid meta", path.display());
            self.front.world_pack_result = Some(false);
            return;
        }
        eprintln!("world pack: {}", path.display());
        self.packs.stack.server = vec![path];
        self.packs.stack.world = true;
        self.reload_resources();
        self.packs.server.world = Some(self.packs.generation + 1);
    }

    /// Opening abandoned: world pack removed (resources reloaded if it was loaded).
    pub(crate) fn world_pack_pop(&mut self) {
        self.packs.server.world = None;
        let had = std::mem::take(&mut self.packs.stack.world) && !self.packs.stack.server.is_empty();
        if had {
            self.packs.stack.server.clear();
            self.reload_resources();
        }
    }

    /// End of the reload awaited by the world pack: loaded if it is still in the stack
    /// (a fallback reload without packs removed it).
    fn world_pack_tick(&mut self) {
        if let Some(g) = self.packs.server.world
            && self.packs.generation >= g
            && !self.packs.reloading_pending()
        {
            self.packs.server.world = None;
            let ok = self.packs.stack.world && !self.packs.stack.server.is_empty();
            if !ok {
                self.packs.stack.world = false;
            }
            self.front.world_pack_result = Some(ok);
        }
    }

    /// Fallback reload without the server's packs (default font failing even without the
    /// player's packs): those pending or active are no longer loaded, their failure is replied.
    pub(crate) fn server_packs_dropped(&mut self) {
        let sp = &mut self.packs.server;
        sp.waiting = None;
        for p in sp.packs.iter_mut() {
            if p.activation != Activation::Inactive {
                p.activation = Activation::Inactive;
                p.remove(Removal::ActivationFailed);
            }
        }
    }

    /// `resource_pack_push` (URL already validated by the network thread).
    pub(crate) fn on_pack_push(
        &mut self,
        id: u128,
        url: String,
        hash: String,
        required: bool,
        prompt: Option<Component>,
    ) {
        let sp = &mut self.packs.server;
        let ask = sp.pref == PackStatus::Prompt || (required && sp.pref == PackStatus::Disabled);
        if !ask {
            sp.push(&self.session.link, id, url, &hash);
            return;
        }
        // Prompt: added to the one already open, otherwise new (text of the last packet).
        let mut requests = sp.prompt.take().map(|p| p.requests).unwrap_or_default();
        requests.push((id, url, hash));
        let line2 = if required {
            let mut c = Component::translate("multiplayer.requiredTexturePrompt.line2", vec![]);
            c.style.color = Some(0xFFFF55);
            c.style.bold = Some(true);
            c
        } else {
            Component::translate("multiplayer.texturePrompt.line2", vec![])
        };
        let message = match prompt {
            Some(p) => Component::translate(
                "multiplayer.texturePrompt.serverPrompt",
                vec![rc_text::Arg::Component(line2), rc_text::Arg::Component(p)],
            ),
            None => line2,
        };
        let title = Component::translate(
            if required { "multiplayer.requiredTexturePrompt.line1" } else { "multiplayer.texturePrompt.line1" },
            vec![],
        );
        let lang = self.renderer.as_ref().map(|r| &r.gui.lang);
        let t = |k: &str| lang.map_or_else(|| k.to_owned(), |l| widgets::tr(l, k, &[]));
        let (yes, no) = if required { (t("gui.proceed"), t("menu.disconnect")) } else { (t("gui.yes"), t("gui.no")) };
        let column = Column::new(title, message, vec![(YES, yes, 150), (NO, no, 150)], 20, 0);
        sp.prompt = Some(PromptScreen { requests, required, column, opened: Instant::now() });
        self.grab(false);
    }

    /// `resource_pack_pop`: removal without reply.
    pub(crate) fn on_pack_pop(&mut self, which: Option<u128>) {
        let sp = &mut self.packs.server;
        match which {
            Some(id) => {
                if let Some(p) = sp.packs.iter_mut().find(|p| !p.removed() && p.id == id) {
                    p.remove(Removal::Server);
                }
            }
            None => sp.packs.iter_mut().for_each(|p| p.remove(Removal::Server)),
        }
    }

    /// Reply to the prompt.
    fn pack_prompt_answer(&mut self, yes: bool) {
        let Some(p) = self.packs.server.prompt.take() else { return };
        let link = self.session.link.clone();
        let sp = &mut self.packs.server;
        if yes {
            sp.pref = PackStatus::Enabled;
            sp.allow(&link);
        } else {
            sp.reject();
            if p.required {
                // Immediate local disconnection, preference unchanged.
                eprintln!("required pack declined: disconnection");
                self.front.kicked = Some(Component::translate("multiplayer.requiredTexturePrompt.disconnect", vec![]));
                self.session_end("required resource pack declined");
                return;
            }
            sp.pref = PackStatus::Disabled;
        }
        for (id, url, hash) in p.requests {
            sp.push(&link, id, url, &hash);
        }
        // Preference recorded in `servers.dat` (entry with the same name and address).
        if let Some((name, ip)) = sp.entry.clone() {
            let dir = crate::servers::game_dir();
            let mut list = ServerList::load(&dir);
            if let Some(e) =
                list.visible.iter_mut().chain(list.hidden.iter_mut()).find(|e| e.name == name && e.ip == ip)
            {
                e.pack_status = sp.pref;
                if let Err(e) = list.save(&dir) {
                    eprintln!("servers.dat : {e}");
                }
            }
        }
        if self.front.in_world && !self.menus.is_open() {
            self.grab(true);
        }
    }

    /// Input taken by the prompt; true if consumed.
    pub(crate) fn pack_prompt_window_event(&mut self, event: &WindowEvent) -> bool {
        if self.packs.server.prompt.is_none() {
            return false;
        }
        let mouse = self.gui_mouse();
        let Some(r) = &self.renderer else { return true };
        let Some(p) = &mut self.packs.server.prompt else { return true };
        p.column.layout(&r.gui);
        let mut ev = Vec::new();
        match event {
            WindowEvent::CursorMoved { .. } => {
                if let Some(m) = mouse {
                    p.column.panel.motion(m, &mut ev);
                }
                return false;
            }
            WindowEvent::MouseInput { state: ElementState::Pressed, button: MouseButton::Left, .. } => {
                if let Some(m) = mouse {
                    p.column.panel.press(m, 0, false, &mut ev);
                }
            }
            WindowEvent::MouseInput { state: ElementState::Released, button: MouseButton::Left, .. } => {
                p.column.panel.release(&mut ev);
            }
            WindowEvent::KeyboardInput { event, .. } if event.state == ElementState::Pressed => {
                if let Some(k) = crate::widgets::edit_key(event, self.mods) {
                    let used = p.column.panel.key(k, self.mods, &mut ev);
                    // Escape: like "No".
                    if !used && k == KeyCode::Escape {
                        self.pack_prompt_answer(false);
                        return true;
                    }
                }
            }
            WindowEvent::KeyboardInput { .. } | WindowEvent::MouseInput { .. } | WindowEvent::MouseWheel { .. } => {}
            _ => return false,
        }
        for e in ev {
            if let widgets::Event::Press(id) = e {
                self.front.clicks += 1;
                self.pack_prompt_answer(id == YES);
                break;
            }
        }
        true
    }

    /// On every frame: downloads, reload, removals (the game's state machine).
    pub(crate) fn server_packs_tick(&mut self) {
        // Mouse-free demo: `RUSTCRAFT_PACKPROMPT=yes|no`, reply 2 s after the prompt.
        if let Some(p) = &self.packs.server.prompt
            && p.opened.elapsed().as_secs_f32() >= 2.0
            && let Some(v) = rc_data::env_str!("RUSTCRAFT_PACKPROMPT")
        {
            self.pack_prompt_answer(v == "yes");
        }
        self.pack_notices();
        self.world_pack_tick();
        let link = self.session.link.clone();
        // Batch finished.
        if let Some(rx) = &self.packs.server.batch
            && let Ok(b) = rx.try_recv()
        {
            self.packs.server.batch = None;
            let sp = &mut self.packs.server;
            if !b.failed.is_empty() {
                for p in sp.packs.iter_mut().filter(|p| p.activation != Activation::Active) {
                    p.remove(if b.failed.contains(&p.id) { Removal::DownloadFailed } else { Removal::Discarded });
                }
            }
            for (id, path) in b.done {
                if let Some(p) = sp.packs.iter_mut().find(|p| p.id == id && p.download == Download::Pending) {
                    p.download = Download::Done;
                    p.path = Some(path);
                    if !p.removed() {
                        ServerPacks::send(&link, id, status::DOWNLOADED);
                    }
                }
            }
        }
        // Awaited reload finished: success.
        if let Some(w) = &self.packs.server.waiting
            && self.packs.generation >= w.generation
            && !self.packs.reloading_pending()
        {
            let w = self.packs.server.waiting.take().expect("wait");
            for p in self.packs.server.packs.iter_mut() {
                if w.load.contains(&p.id) {
                    p.activation = Activation::Active;
                    if !p.removed() {
                        ServerPacks::send(&link, p.id, status::LOADED);
                    }
                } else if w.unload.contains(&p.id) {
                    p.activation = Activation::Inactive;
                }
            }
        }
        if self.packs.server.waiting.is_none() {
            let pending = self.server_downloads();
            if !pending {
                self.server_reload_if_needed();
            }
        }
        // Removals: terminal reply, pack forgotten.
        self.packs.server.packs.retain(|p| {
            if p.activation != Activation::Inactive {
                return true;
            }
            match p.removal {
                Some(r) => {
                    if let Some(s) = r.response() {
                        ServerPacks::send(&link, p.id, s);
                    }
                    false
                }
                None => true,
            }
        });
    }

    /// Notifications of the batches: events received applied, notification updated once
    /// per frame at most (the text displayed is that of the last event, like the game which
    /// updates it on every event).
    fn pack_notices(&mut self) {
        let notices = &mut self.packs.server.notices;
        if notices.is_empty() {
            return;
        }
        let gui = self.renderer.as_ref().map(|r| &r.gui);
        for n in notices.iter_mut() {
            let id = ("download.pack", n.serial);
            match (n.drain(), gui) {
                (NoticeChange::Show, Some(g)) => {
                    self.toasts.system_keyed(g, Some(id), &n.title(), n.message().as_ref(), 5000.0)
                }
                (NoticeChange::Hide, _) => {
                    self.toasts.system_hide(id);
                }
                // The hiding finds only the notification already created; otherwise the one requested
                // in this frame appears afterwards and stays its whole duration (see `drain`).
                (NoticeChange::HideOrShow, Some(g)) if !self.toasts.system_hide(id) => {
                    self.toasts.system_keyed(g, Some(id), &n.title(), n.message().as_ref(), 5000.0);
                }
                _ => {}
            }
        }
        // Batch finished (or thread stopped): notification forgotten.
        notices.retain(|n| !n.done);
    }

    /// Starts the batch of the accepted packs not yet requested; true if an accepted download
    /// is not finished.
    fn server_downloads(&mut self) -> bool {
        let sp = &mut self.packs.server;
        let mut pending = false;
        let mut todo = Vec::new();
        for p in sp.packs.iter_mut().filter(|p| !p.removed() && p.accepted) {
            if p.download != Download::Done {
                pending = true;
            }
            if p.download == Download::Requested {
                p.download = Download::Pending;
                todo.push((p.id, p.url.clone(), p.hash.clone()));
            }
        }
        if todo.is_empty() {
            return pending;
        }
        let dir = downloads_dir();
        let vacuum_first = !std::mem::replace(&mut sp.vacuumed, true);
        let hdrs = headers(&self.local_name, self.local_uuid);
        let (tx, rx) = crossbeam_channel::bounded(1);
        sp.batch = Some(rx);
        // Batch notification: progress events, read on every frame.
        let (ptx, prx) = crossbeam_channel::unbounded();
        sp.serial += 1;
        sp.notices.push(Notice::new(prx, sp.serial, todo.len()));
        let spawned = std::thread::Builder::new().name("server-packs".into()).spawn(move || {
            let _ = std::fs::create_dir_all(&dir);
            if vacuum_first {
                vacuum(&dir);
            }
            let mut b = Batch { done: Vec::new(), failed: Vec::new() };
            for (id, url, hash) in todo {
                let _ = ptx.send(Progress::Start);
                let r = download(&dir, id, &url, hash.as_deref(), &hdrs, &mut |p| {
                    let _ = ptx.send(p);
                });
                let _ = ptx.send(Progress::Finished(r.is_ok()));
                log_line(&dir, id, &url, hash.as_deref(), &r);
                match r {
                    Ok(p) => b.done.push((id, p)),
                    Err(e) => {
                        eprintln!("server pack: download of {url} failed: {e}");
                        b.failed.push(id);
                    }
                }
            }
            let _ = tx.send(b);
        });
        if let Err(e) = spawned {
            eprintln!("server packs: {e}");
        }
        true
    }

    /// No more download in progress: reload if a pack must enter or leave.
    fn server_reload_if_needed(&mut self) {
        let sp = &mut self.packs.server;
        if sp.packs.iter().any(|p| p.activation == Activation::Pending) {
            return;
        }
        let mut need = false;
        let (mut load, mut unload) = (Vec::new(), Vec::new());
        for p in &sp.packs {
            let should = p.accepted && p.download == Download::Done && !p.removed();
            match p.activation {
                Activation::Inactive if should => {
                    load.push(p.id);
                    need = true;
                }
                Activation::Active if should => load.push(p.id),
                Activation::Active => {
                    unload.push(p.id);
                    need = true;
                }
                _ => {}
            }
        }
        if !need {
            return;
        }
        // Meta of each pack to load read first: an unreadable pack makes the
        // reload fail (pending → 6, inactive → 7, active kept).
        let bad = sp.active_paths(&load).iter().any(|p| {
            rc_assets::packs::read_root(p, "pack.mcmeta").and_then(|b| rc_assets::packs::parse_meta(&b)).is_none()
        });
        if bad {
            eprintln!("server packs: invalid meta, reload cancelled");
            for p in sp.packs.iter_mut() {
                match p.activation {
                    Activation::Inactive if load.contains(&p.id) => p.remove(Removal::ActivationFailed),
                    Activation::Inactive => p.remove(Removal::Discarded),
                    _ => {}
                }
            }
            return;
        }
        for p in sp.packs.iter_mut() {
            if load.contains(&p.id) && p.activation != Activation::Active || unload.contains(&p.id) {
                p.activation = Activation::Pending;
            }
        }
        let paths = sp.active_paths(&load);
        self.packs.stack.server = paths;
        self.packs.stack.world = false;
        self.reload_resources();
        let generation = self.packs.generation + 1;
        self.packs.server.waiting = Some(Waiting { generation, load, unload });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Header of the pack format written "major.minor" (the old code sent only "97").
    #[test]
    fn pack_format_header_has_minor() {
        let h = headers("Steve", 1);
        let f = h.iter().find(|(k, _)| *k == "X-Minecraft-Pack-Format").map(|(_, v)| v.as_str());
        assert_eq!(f, Some("97.1"));
    }

    /// Readable bytes like the game, rounded half up (1.25 KB → "1.3 KB").
    #[test]
    fn human_readable_bytes() {
        assert_eq!(human_readable(0), "0 B");
        assert_eq!(human_readable(1023), "1023 B");
        assert_eq!(human_readable(1024), "1.0 KB");
        assert_eq!(human_readable(1280), "1.3 KB");
        assert_eq!(human_readable(1536), "1.5 KB");
        assert_eq!(human_readable(1_048_576), "1.0 MB");
        assert_eq!(human_readable(5 * 1_048_576 + 104_858), "5.1 MB");
    }

    fn keys(c: &Component) -> String {
        match &c.content {
            rc_text::Content::Translate { key, with, .. } => {
                let a: Vec<String> = with
                    .iter()
                    .map(|a| match a {
                        rc_text::Arg::Text(t) => t.clone(),
                        _ => "?".into(),
                    })
                    .collect();
                format!("{key}({})", a.join(","))
            }
            _ => "?".into(),
        }
    }

    /// Notification of a batch of two: title n/N, percentage then bytes, final failure displayed;
    /// successful batch: hidden.
    #[test]
    fn download_notice_follows_the_batch() {
        let (_tx, rx) = crossbeam_channel::unbounded();
        let mut n = Notice::new(rx, 1, 2);
        assert_eq!(n.apply(Progress::Start), NoticeChange::Show);
        assert_eq!(keys(&n.title()), "download.pack.title(1,2)");
        assert!(n.message().is_none());
        n.apply(Progress::Size(Some(200)));
        assert_eq!(keys(&n.message().expect("message")), "download.pack.progress.percent(0)");
        n.apply(Progress::Bytes(100));
        assert_eq!(keys(&n.message().expect("message")), "download.pack.progress.percent(50)");
        assert_eq!(n.apply(Progress::Finished(true)), NoticeChange::Nothing);
        n.apply(Progress::Start);
        assert_eq!(keys(&n.title()), "download.pack.title(2,2)");
        n.apply(Progress::Size(None));
        n.apply(Progress::Bytes(1536));
        assert_eq!(keys(&n.message().expect("message")), "download.pack.progress.bytes(1.5 KB)");
        assert_eq!(n.apply(Progress::Finished(false)), NoticeChange::Show);
        assert_eq!(keys(&n.title()), "download.pack.failed(1,2)");
        assert!(n.message().is_none());
        assert!(n.done);

        let (_tx, rx) = crossbeam_channel::unbounded();
        let mut ok = Notice::new(rx, 2, 1);
        ok.apply(Progress::Start);
        assert_eq!(ok.apply(Progress::Finished(true)), NoticeChange::Hide);

        // Announced size zero (quirk): neither "0%" message nor update; the previous
        // file's message stays. The old code displayed "0%".
        let (_tx, rx) = crossbeam_channel::unbounded();
        let mut z = Notice::new(rx, 3, 2);
        z.apply(Progress::Start);
        assert_eq!(z.apply(Progress::Size(Some(0))), NoticeChange::Nothing);
        assert!(z.message().is_none(), "first file: no message");
        assert_eq!(z.apply(Progress::Finished(false)), NoticeChange::Nothing);
        z.apply(Progress::Start);
        z.apply(Progress::Size(Some(10)));
        z.apply(Progress::Bytes(5));
        assert_eq!(z.apply(Progress::Size(Some(0))), NoticeChange::Nothing);
        assert_eq!(keys(&z.message().expect("message")), "download.pack.progress.percent(50)");
    }

    /// Batch finished in the frame where it started (packs in cache): hiding or creation depending on whether the
    /// notification exists; finished in a later frame: hiding only. The old code always created
    /// then hid.
    #[test]
    fn cached_batch_notice_races_like_the_game() {
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut n = Notice::new(rx, 1, 1);
        tx.send(Progress::Start).expect("send");
        tx.send(Progress::Finished(true)).expect("send");
        assert_eq!(n.drain(), NoticeChange::HideOrShow);
        let (tx, rx) = crossbeam_channel::unbounded();
        let mut n = Notice::new(rx, 2, 1);
        tx.send(Progress::Start).expect("send");
        assert_eq!(n.drain(), NoticeChange::Show);
        tx.send(Progress::Finished(true)).expect("send");
        assert_eq!(n.drain(), NoticeChange::Hide);
        assert_eq!(n.drain(), NoticeChange::Nothing);
        drop(tx);
        n.drain();
        assert!(n.done);
        let mut t = crate::toasts::Toasts::default();
        assert!(!t.system_hide(("download.pack", 1)), "no notification to hide");
    }

    /// Cleanup: a folder linked in the downloads folder is neither walked nor emptied
    /// (the old code followed the link and erased files of its target).
    #[test]
    fn vacuum_never_follows_links() {
        let root = std::env::temp_dir().join(format!("rc-vacuum-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let downloads = root.join("downloads");
        let outside = root.join("elsewhere");
        std::fs::create_dir_all(downloads.join("a")).expect("folder");
        std::fs::create_dir_all(&outside).expect("folder");
        for i in 0..KEEP + 5 {
            std::fs::write(outside.join(format!("f{i}")), b"x").expect("file");
        }
        std::fs::write(downloads.join("a").join("pack"), b"x").expect("file");
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(&outside, downloads.join("link")).is_ok();
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&outside, downloads.join("link")).is_ok();
        if !made {
            eprintln!("symbolic links not allowed on this system: test skipped");
            let _ = std::fs::remove_dir_all(&root);
            return;
        }
        vacuum(&downloads);
        assert_eq!(std::fs::read_dir(&outside).expect("folder").count(), KEEP + 5);
        assert!(downloads.join("a").join("pack").is_file());
        let _ = std::fs::remove_dir_all(&root);
    }
}
