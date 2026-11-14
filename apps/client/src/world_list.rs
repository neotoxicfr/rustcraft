//! "Singleplayer" screens (`docs/specs/world-screens.md`): list of the worlds in the `saves` folder
//! (icons, search, sort, warnings), deletion, `level.dat` recovery, backup
//! copy before opening, waiting screens; own stack of these screens above the title
//! screen. Creation is in `create_world.rs`, editing in `edit_world.rs`.
//!
//! Also: "singleplayer world open" state (pause menu labels), HUD autosave
//! indicator.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread::JoinHandle;

use rc_data::VersionData;
use rc_proto::nbt::Nbt;
use rc_text::{Component, StyledChar};
use winit::keyboard::{KeyCode, ModifiersState};

use crate::gui::Gui;
use crate::options::Options;
use crate::server_list::Column;
use crate::widgets::{self, Entry, Event, Kind, List, Panel, TextField, Widget, chars, tr};

// ---------- Global singleplayer state ----------

static SOLO_OPEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// A singleplayer world is open (pause menu labels).
pub fn solo_open() -> bool {
    SOLO_OPEN.load(std::sync::atomic::Ordering::Relaxed)
}

pub fn set_solo_open(open: bool) {
    SOLO_OPEN.store(open, std::sync::atomic::Ordering::Relaxed);
}

/// Autosave indicator: previous and current value (smoothed at 0.2 per tick).
static AUTOSAVE: std::sync::Mutex<(f32, f32)> = std::sync::Mutex::new((0.0, 0.0));

/// Client tick in a world: is the integrated server saving?
pub fn tick_autosave(saving: bool) {
    let mut a = AUTOSAVE.lock().unwrap_or_else(|p| p.into_inner());
    a.0 = a.1;
    a.1 += 0.2 * (if saving { 1.0 } else { 0.0 } - a.1);
}

/// "Saving world…" at the bottom right, on a background, smoothed opacity (`partial`: tick fraction).
pub fn draw_autosave(gui: &mut Gui, q: &wgpu::Queue, partial: f32, enabled: bool, backdrop: u32) {
    let (last, cur) = *AUTOSAVE.lock().unwrap_or_else(|p| p.into_inner());
    if !enabled || (last <= 0.0 && cur <= 0.0) {
        return;
    }
    let alpha = (255.0 * (last + (cur - last) * partial).clamp(0.0, 1.0)).floor() as u32;
    if alpha == 0 {
        return;
    }
    let text = chars(&tr(&gui.lang, "menu.savingLevel", &[]));
    let tw = rc_text::width(gui, &text);
    let (x, y) = (gui.width as i32 - tw - 5, gui.height as i32 - 9 - 5);
    gui.next_layer();
    // Text background (chat background opacity), faded like the text.
    let ba = ((backdrop >> 24) * alpha / 255) << 24;
    if ba != 0 {
        gui.fill((x - 2) as f32, (y - 2) as f32, (x + tw + 2) as f32, (y + 9 + 2) as f32, ba | (backdrop & 0xff_ffff));
    }
    gui.text(q, &text, x as f32, y as f32, (alpha << 24) | 0xff_ffff, true, 0);
}

// ---------- Tools common to the singleplayer screens ----------

/// Input forwarded to the top screen.
pub(crate) enum In<'a> {
    Motion((f32, f32)),
    Press((f32, f32), u8, bool),
    Release,
    Wheel((f32, f32), f64),
    Key(KeyCode, ModifiersState),
    Typed(&'a str),
}

/// Input passed to a `Panel`: events, and Escape not used.
pub(crate) fn feed(panel: &mut Panel, i: &In<'_>) -> (Vec<Event>, bool) {
    let mut ev = Vec::new();
    let mut escape = false;
    match i {
        In::Motion(m) => panel.motion(*m, &mut ev),
        In::Press(m, b, s) => panel.press(*m, *b, *s, &mut ev),
        In::Release => panel.release(&mut ev),
        In::Wheel(m, dy) => panel.wheel(*m, *dy, &mut ev),
        In::Key(k, mods) => {
            let used = panel.key(*k, *mods, &mut ev);
            escape = !used && *k == KeyCode::Escape;
        }
        In::Typed(s) => panel.typed(s, &mut ev),
    }
    (ev, escape)
}

pub(crate) fn t(gui: &Gui, key: &str) -> String {
    tr(&gui.lang, key, &[])
}

/// Text of a fixed colour (RGB).
pub(crate) fn colored(s: &str, rgb: u32) -> Vec<StyledChar> {
    let mut v = chars(s);
    for c in &mut v {
        c.style.color = Some(rgb);
    }
    v
}

pub(crate) fn italic(mut v: Vec<StyledChar>) -> Vec<StyledChar> {
    for c in &mut v {
        c.style.italic = true;
    }
    v
}

/// Text cut to `max` wide, ending with "..." if it overflows.
pub(crate) fn clamp_text(gui: &Gui, s: &[StyledChar], max: i32) -> Vec<StyledChar> {
    rc_text::clamp(gui, s, max)
}

/// Animated dots of the waiting screens (`O o o`…), every 300 ms.
pub(crate) fn loading_dots() -> &'static str {
    let ms = std::time::SystemTime::UNIX_EPOCH.elapsed().map_or(0, |d| d.as_millis());
    ["O o o", "o O o", "o o O", "o O o"][(ms / 300 % 4) as usize]
}

pub(crate) fn background(gui: &mut Gui, o: &Options) {
    gui.next_layer();
    widgets::menu_background(gui, o.i("menuBackgroundBlurriness"), false);
    gui.next_layer();
}

/// Short local date of `LastPlayed` (short format of the language: `dd/mm/yyyy hh:mm` in
/// French, `m/d/yy, h:mm AM` in English).
pub(crate) fn short_date(ms: i64, lang: &str) -> String {
    let (y, mo, d, h, mi, _) = rc_sim::storage::local_datetime(ms);
    if lang.starts_with("en") {
        let (h12, ampm) = match h {
            0 => (12, "AM"),
            1..12 => (h, "AM"),
            12 => (12, "PM"),
            _ => (h - 12, "PM"),
        };
        format!("{mo}/{d}/{:02}, {h12}:{mi:02}\u{202f}{ampm}", y.rem_euclid(100))
    } else if lang.starts_with("de") {
        format!("{d:02}.{mo:02}.{:02}, {h:02}:{mi:02}", y.rem_euclid(100))
    } else {
        format!("{d:02}/{mo:02}/{y} {h:02}:{mi:02}")
    }
}

// ---------- Summary of a world ----------

/// What the list knows about a world (`level.dat` read when the screen opens).
#[derive(Debug, Clone, PartialEq)]
pub struct Summary {
    pub dir: PathBuf,
    /// Folder name.
    pub id: String,
    pub name: String,
    /// `LastPlayed` in ms (-1 unknown).
    pub last_played: i64,
    pub game_mode: i32,
    pub hardcore: bool,
    pub commands: bool,
    pub experimental: bool,
    pub version_name: Option<String>,
    pub series: String,
    pub data_version: i32,
    pub locked: bool,
    /// Unreadable `level.dat`: recovery offered.
    pub corrupted: bool,
    /// 64 × 64 icon in RGBA (readable `icon.png`).
    pub icon: Option<Vec<u8>>,
    /// `level.dat` is a symbolic link not allowed: warning entry ("More info").
    pub symlink: bool,
}

/// Symbolic link `p` refused by `allowed_symlinks.txt` (logged); not a link: no.
fn forbidden_link(p: &Path) -> bool {
    if !std::fs::symlink_metadata(p).is_ok_and(|m| m.file_type().is_symlink()) {
        return false;
    }
    let mut refused = Vec::new();
    let _ = crate::servers::allowed_symlinks().check_link(p, &mut refused);
    for f in &refused {
        eprintln!("symbolic link refused {} -> {}", f.link.display(), f.target.display());
    }
    !refused.is_empty()
}

/// Modification time of `path` in ms since 1970.
fn modified_ms(path: &Path) -> Option<i64> {
    let t = std::fs::metadata(path).and_then(|m| m.modified()).ok()?;
    t.duration_since(std::time::UNIX_EPOCH).ok().map(|d| d.as_millis() as i64)
}

/// The 64 × 64 icon (`icon.png`) in RGBA: none behind a refused link, nor when the file is not a
/// 64 × 64 PNG. The PNG header is read first, the rest only for that size
/// (`RUSTCRAFT_OPT_WORLD_ICON_HEADER_FIRST=0`: the whole file read first, the old path).
fn read_icon(dir: &Path) -> Option<Vec<u8>> {
    let path = dir.join("icon.png");
    if forbidden_link(&path) {
        return None;
    }
    let first = rc_data::env_str!("RUSTCRAFT_OPT_WORLD_ICON_HEADER_FIRST") != Some("0");
    let png = if first { read_png_if_icon_sized(&path) } else { std::fs::read(&path).ok() }?;
    rc_client::status::Favicon::from_png(png).and_then(|f| f.rgba())
}

/// The PNG at `path`, read whole only when its header (24 bytes) announces 64 × 64 pixels.
fn read_png_if_icon_sized(path: &Path) -> Option<Vec<u8>> {
    use std::io::Read;
    let mut file = std::fs::File::open(path).ok()?;
    let mut png = vec![0; 24];
    file.read_exact(&mut png).ok()?;
    let side = rc_client::status::ICON_SIDE;
    if rc_client::status::png_size(&png) != Some((side, side)) {
        return None;
    }
    file.read_to_end(&mut png).ok()?;
    Some(png)
}

impl Summary {
    /// Everything the list shows about a world: what `level.dat` says, the icon, the lock.
    pub fn read(dir: &Path) -> Summary {
        let mut s = Summary::read_level_data(dir);
        s.icon = read_icon(dir);
        // A refused `level.dat` link stands for the whole world: never shown locked.
        s.locked = !s.symlink && rc_sim::lock::SessionLock::is_locked(dir);
        s
    }

    /// What `level.dat` says (name, last played, mode, version…), without the icon or the lock:
    /// enough to place the world in the list.
    fn read_level_data(dir: &Path) -> Summary {
        let id = dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let mut s = Summary {
            dir: dir.to_owned(),
            name: id.clone(),
            id,
            last_played: -1,
            game_mode: 0,
            hardcore: false,
            commands: false,
            experimental: false,
            version_name: None,
            series: "main".into(),
            data_version: 0,
            locked: false,
            corrupted: false,
            icon: None,
            symlink: false,
        };
        // `level.dat` behind a refused link: nothing else is read (name = folder, never played).
        if forbidden_link(&dir.join("level.dat")) {
            s.symlink = true;
            return s;
        }
        let root = match rc_sim::storage::read_level_summary(dir) {
            Ok(r) => r,
            Err(_) => {
                // Unreadable (or only `level.dat_old`): dated by the file, like the game.
                s.corrupted = true;
                s.last_played =
                    ["level.dat", "level.dat_old"].iter().find_map(|f| modified_ms(&dir.join(f))).unwrap_or(-1);
                return s;
            }
        };
        let Some(d) = root.get("Data") else {
            s.corrupted = true;
            return s;
        };
        let int = |v: &Nbt, k: &str| rc_sim::nbtio::int(v, k);
        if let Some(n) = d.get("LevelName").and_then(Nbt::as_str).filter(|n| !n.is_empty()) {
            s.name = n.to_owned();
        }
        s.last_played = int(d, "LastPlayed").unwrap_or(-1);
        s.game_mode = int(d, "GameType").unwrap_or(0) as i32;
        let diff = d.get("difficulty_settings");
        s.hardcore = diff.and_then(|x| int(x, "hardcore")) == Some(1);
        s.commands = int(d, "allowCommands") == Some(1);
        s.data_version = int(d, "DataVersion").unwrap_or(0) as i32;
        if let Some(v) = d.get("Version") {
            s.version_name = v.get("Name").and_then(Nbt::as_str).filter(|n| !n.is_empty()).map(str::to_owned);
            if let Some(series) = v.get("Series").and_then(Nbt::as_str) {
                series.clone_into(&mut s.series);
            }
            if let Some(id) = int(v, "Id") {
                s.data_version = id as i32;
            }
        }
        s.experimental = rc_sim::nbtio::list(d.get("enabled_features"))
            .iter()
            .filter_map(Nbt::as_str)
            .any(|f| f != "minecraft:vanilla");
        s
    }

    /// Same version series (`main`) as the game.
    pub fn compatible(&self) -> bool {
        self.corrupted || self.series == "main"
    }

    /// World of a more recent version than the game (backup copy offered, in red).
    pub fn downgrade(&self) -> bool {
        !self.corrupted && self.data_version > rc_sim::chunk::DATA_VERSION
    }

    /// World of an older version: the conversion is not done by Rustcraft.
    pub fn older(&self) -> bool {
        !self.corrupted && self.data_version < rc_sim::chunk::DATA_VERSION
    }

    fn disabled(&self) -> bool {
        !self.symlink && !self.corrupted && (self.locked || !self.compatible())
    }

    /// "Play" active (recovery for an unreadable world, "More info" for a refused
    /// link).
    pub fn can_play(&self) -> bool {
        self.symlink || (!self.disabled() && !self.older())
    }

    pub fn can_edit(&self) -> bool {
        !self.symlink && !self.corrupted && !self.disabled() && !self.older()
    }

    fn version_text(&self, lang: &rc_text::Lang) -> String {
        self.version_name.clone().unwrap_or_else(|| tr(lang, "selectWorld.versionUnknown", &[]))
    }

    /// Third line: lock, incompatibility, otherwise mode, commands, experiments, version.
    fn info(&self, lang: &rc_text::Lang) -> Vec<StyledChar> {
        const GRAY: u32 = 0x80_8080;
        let t = |key: &str| tr(lang, key, &[]);
        if self.symlink {
            return colored(&t("symlink_warning.title"), 0xff_0000);
        }
        if self.corrupted {
            return colored(&t("recover_world.warning"), 0xff_0000);
        }
        if self.locked {
            return colored(&t("selectWorld.locked"), 0xff_5555);
        }
        if !self.compatible() {
            return colored(&tr(lang, "selectWorld.incompatible.info", &[self.version_text(lang)]), 0xff_5555);
        }
        let mut v = if self.hardcore {
            colored(&t("gameMode.hardcore"), 0xff_0000)
        } else {
            let mode = ["survival", "creative", "adventure", "spectator"][self.game_mode.clamp(0, 3) as usize];
            colored(&t(&format!("gameMode.{mode}")), GRAY)
        };
        if self.commands {
            v.extend(colored(&format!(", {}", t("selectWorld.commands")), GRAY));
        }
        if self.experimental {
            v.extend(colored(", ", GRAY));
            v.extend(colored(&t("selectWorld.experimental"), 0xff_ff55));
        }
        v.extend(colored(&format!(", {} ", t("selectWorld.version")), GRAY));
        let name = self.version_text(lang);
        if self.downgrade() {
            v.extend(colored(&name, 0xff_5555));
        } else if self.older() {
            v.extend(italic(colored(&name, GRAY)));
        } else {
            v.extend(colored(&name, GRAY));
        }
        v
    }
}

/// Folders of `saves` the list shows, like the game: a folder holding `level.dat`, or failing that
/// `level.dat_old` (then shown unreadable, with recovery).
fn world_dirs(game_dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(game_dir.join("saves")) else { return Vec::new() };
    entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir() && (p.join("level.dat").is_file() || p.join("level.dat_old").is_file()))
        .collect()
}

/// List order: last played first, then by folder name compared like the game (UTF-16 units: a
/// character beyond U+FFFF sorts by its surrogates).
fn world_order(a: &Summary, b: &Summary) -> std::cmp::Ordering {
    b.last_played.cmp(&a.last_played).then_with(|| a.id.encode_utf16().cmp(b.id.encode_utf16()))
}

/// Each world read with `read`: on the shared pool, one task per world like the game's background
/// reads, or one after the other.
fn read_each(dirs: &[PathBuf], read: fn(&Path) -> Summary, parallel: bool) -> Vec<Summary> {
    use rayon::prelude::*;
    if parallel { dirs.par_iter().map(|d| read(d)).collect() } else { dirs.iter().map(|d| read(d)).collect() }
}

/// Worlds of `saves` in list order (`RUSTCRAFT_OPT_WORLD_LIST_PARALLEL_READ=0`: read one after the
/// other, the old path).
pub fn scan(game_dir: &Path) -> Vec<Summary> {
    let parallel = rc_data::env_str!("RUSTCRAFT_OPT_WORLD_LIST_PARALLEL_READ") != Some("0");
    let mut worlds = read_each(&world_dirs(game_dir), Summary::read, parallel);
    worlds.sort_by(world_order);
    worlds
}

/// Folder of the world played last (the top of the list), for quick play: only `level.dat` is
/// read, the icons and locks not changing the order (`RUSTCRAFT_OPT_QUICK_PLAY_LIGHT_SCAN=0`: the
/// whole list read, the old path).
pub fn last_played_world(game_dir: &Path) -> Option<String> {
    if rc_data::env_str!("RUSTCRAFT_OPT_QUICK_PLAY_LIGHT_SCAN") == Some("0") {
        return scan(game_dir).into_iter().next().map(|s| s.id);
    }
    read_each(&world_dirs(game_dir), Summary::read_level_data, true).into_iter().min_by(world_order).map(|s| s.id)
}

pub fn saves_dir() -> PathBuf {
    crate::servers::game_dir().join("saves")
}

// ---------- Stack of the singleplayer screens ----------

/// Request of the singleplayer stack to the out-of-world screen.
pub enum Out {
    /// Stack empty: back to the title screen.
    Back,
    /// Opening of the world by the integrated server.
    Open(PathBuf),
    /// Opening of a world that has just been created (loading screen kept 500 ms longer).
    OpenNew(PathBuf),
    /// System notification (title, message).
    Toast(Box<(Component, Option<Component>)>),
    /// Resource pack of the world (`resources.zip`) to load before opening; the result
    /// comes back through `Solo::world_pack_done`.
    WorldPack(PathBuf),
    /// Opening abandoned: world pack removed.
    DropWorldPack,
}

/// Step of the opening of a world, in the game's order (after the reading and the copy offered
/// for another version): copy offered for experimental settings, world pack,
/// disk space, opening.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Stage {
    Experimental,
    Pack,
    Disk,
    Load,
}

impl Out {
    pub(crate) fn toast(title: Component, message: Option<Component>) -> Out {
        Out::Toast(Box::new((title, message)))
    }
}

/// Tooltip of a row: lines and hovered rectangle.
type Tip = Option<(Vec<Vec<StyledChar>>, (i32, i32, i32, i32))>;

/// Stack change requested by a screen.
pub(crate) enum Msg {
    /// Top screen closed; the list, if it shows again, is read again (the game's return to the list).
    Pop,
    /// Top screen closed, the list below shown again as it was: the game re-reads nothing there
    /// (back from a notice or an alert, or a screen replaced by the next step).
    Close,
    Push(Scr),
    /// Back to the list (re-read), screens above closed.
    ToList,
    /// Opening of a world, resumed at the given step.
    Open(PathBuf, Stage),
    /// Title screen (singleplayer stack closed).
    Title,
    /// Recovery: `level.dat_old` put back in place (after the copy).
    Restore(PathBuf),
    Out(Out),
    Job(Job),
}

/// Background work (separate thread, waiting screen above).
pub(crate) enum Job {
    /// New world: folder, settings, packs (enabled, disabled, features).
    Create(PathBuf, Box<rc_sim::newworld::NewWorld>, Box<crate::create_world::Packs>),
    /// Backup copy then continuation.
    Backup(PathBuf, After),
}

/// Continuation of a backup copy.
pub(crate) enum After {
    ToList,
    /// Opening resumed at the given step.
    Open(PathBuf, Stage),
    /// Optimisation (clearing of the caches).
    Optimize(PathBuf, bool),
    /// Recovery of `level.dat_old`.
    Restore(PathBuf),
}

pub(crate) enum Scr {
    List(Box<SelectWorld>),
    Create(Box<crate::create_world::CreateWorld>),
    Edit(Box<crate::edit_world::EditWorld>),
    Optimize(Box<crate::edit_world::Optimizing>),
    Backup(Box<BackupConfirm>),
    Recover(Box<Recover>),
    /// Confirmation: buttons → continuation (Escape: the last one).
    Ask(Box<Column>, Vec<(u32, ConfirmAction)>),
    /// Alert with a single "Back" button → continuation. Game quirk reproduced: Escape closes it without a
    /// continuation, towards the title screen (default closing of an out-of-world screen).
    Alert(Box<Column>, ConfirmAction),
    Message(Box<Message>),
}

/// Continuation of a confirmation button (`Msg` without a screen: copyable).
#[derive(Clone)]
pub(crate) enum ConfirmAction {
    Pop,
    /// See [`Msg::Close`].
    Close,
    ToList,
    /// Title screen (singleplayer stack closed).
    Title,
    Delete(PathBuf),
    Open(PathBuf, Stage),
    /// Opening abandoned: world pack removed, back to the list.
    CancelOpen,
    /// Address opened in the browser / copied; the screen stays.
    OpenUrl(&'static str),
    CopyUrl(&'static str),
}

enum Running {
    Create(JoinHandle<Result<(), String>>, PathBuf),
    Backup(JoinHandle<Result<(PathBuf, u64), String>>, String, Option<After>),
    Save(JoinHandle<()>),
    /// Opening: copy offered for unstable settings? (world)
    Question(JoinHandle<Option<rc_sim::packs::Unstable>>, PathBuf),
}

pub struct Solo {
    stack: Vec<Scr>,
    data: Arc<VersionData>,
    running: Option<Running>,
    /// Outputs pending (end of a background job), handed back on the tick.
    pending: Vec<Out>,
    /// World whose pack (`resources.zip`) is loading.
    world_pack: Option<PathBuf>,
    /// Direct opening (quick launch): an abandon goes back to the title screen, not to the list.
    direct: bool,
}

impl Solo {
    /// "Singleplayer" on the title screen: list, or direct creation if there is no world.
    pub fn new(data: Arc<VersionData>) -> Solo {
        crate::create_world::set_shared_data(data.clone());
        let mut s = Solo::empty(data);
        s.stack.push(Scr::List(Box::new(SelectWorld::new())));
        s.list_or_create();
        s
    }

    fn empty(data: Arc<VersionData>) -> Solo {
        Solo { stack: Vec::new(), data, running: None, pending: Vec::new(), world_pack: None, direct: false }
    }

    /// Direct opening of a world (quick launch): opening steps without the list
    /// underneath; an abandon goes back to the title screen.
    pub fn opening(gui: &Gui, data: Arc<VersionData>, dir: PathBuf) -> (Solo, Vec<Out>) {
        crate::create_world::set_shared_data(data.clone());
        let mut s = Solo::empty(data);
        s.direct = true;
        // Links in the folder checked first, as from the list.
        let first = if world_refused(&dir) {
            Msg::Push(symlink_refused(&gui.lang))
        } else {
            Msg::Open(dir, Stage::Experimental)
        };
        let out = s.apply(&gui.lang, vec![first]);
        (s, out)
    }

    /// Top list read and empty (no world, or last one deleted): creation opens in its place,
    /// like the game; cancelling it goes back to the title screen.
    fn list_or_create(&mut self) {
        if let Some(Scr::List(l)) = self.stack.last()
            && l.loaded
            && l.worlds.is_empty()
        {
            self.stack.pop();
            let c = crate::create_world::CreateWorld::fresh(self.data.clone());
            self.stack.push(Scr::Create(Box::new(c)));
        }
    }

    /// List back on top: read again (creation in its place once no world is left).
    fn reload_list_on_top(&mut self) {
        if let Some(Scr::List(l)) = self.stack.last_mut() {
            l.reload();
            self.list_or_create();
        }
    }

    /// The top list's background reading ended: its worlds are shown; none: creation in its place.
    fn show_finished_reading(&mut self) {
        if let Some(Scr::List(l)) = self.stack.last_mut()
            && l.take_finished_reading()
        {
            self.list_or_create();
        }
    }

    /// "Saving world…" screen during the closing of the integrated server.
    pub fn saving(data: Arc<VersionData>, h: rc_sim::Handle) -> Solo {
        let th = std::thread::Builder::new().name("singleplayer-closing".into()).spawn(move || h.wait()).ok();
        let mut s = Solo::empty(data);
        s.stack.push(Scr::Message(Box::new(Message::new("menu.savingLevel", None))));
        if let Some(th) = th {
            s.running = Some(Running::Save(th));
        }
        s
    }

    /// Closing in progress ("Saving world…"): awaited before leaving the game.
    pub fn finish_save(&mut self) {
        if let Some(Running::Save(th)) = self.running.take() {
            let _ = th.join();
            set_solo_open(false);
        }
    }

    pub fn panel_mut(&mut self) -> Option<&mut Panel> {
        Some(match self.stack.last_mut()? {
            Scr::List(s) => &mut s.panel,
            Scr::Create(s) => s.panel_mut(),
            Scr::Edit(s) => &mut s.panel,
            Scr::Optimize(s) => &mut s.panel,
            Scr::Backup(s) => &mut s.panel,
            Scr::Recover(s) => &mut s.panel,
            Scr::Ask(c, _) | Scr::Alert(c, _) => &mut c.panel,
            Scr::Message(m) => &mut m.panel,
        })
    }

    /// Does Escape close the top screen? (waiting screens: no)
    pub fn input(&mut self, gui: &Gui, i: &In<'_>) -> Vec<Out> {
        let msgs = match self.stack.last_mut() {
            None => return vec![Out::Back],
            Some(Scr::List(s)) => s.input(gui, i),
            Some(Scr::Create(s)) => s.input(gui, i),
            Some(Scr::Edit(s)) => s.input(gui, i),
            Some(Scr::Optimize(s)) => s.input(gui, i),
            Some(Scr::Backup(s)) => s.input(gui, i),
            Some(Scr::Recover(s)) => s.input(gui, i),
            Some(Scr::Ask(c, answers)) => {
                let (ev, escape) = feed(&mut c.panel, i);
                let mut m = Vec::new();
                for e in ev {
                    if let Event::Press(id) = e
                        && let Some((_, a)) = answers.iter().find(|(b, _)| *b == id)
                    {
                        m.extend(action_msgs(a.clone()));
                    }
                }
                if escape {
                    // Escape = last button ("Cancel", "Back").
                    if let Some((_, a)) = answers.last() {
                        m.extend(action_msgs(a.clone()));
                    }
                }
                m
            }
            Some(Scr::Alert(c, then)) => alert_input(c, then, i),
            Some(Scr::Message(_)) => Vec::new(),
        };
        self.apply(&gui.lang, msgs)
    }

    /// End of loading of the world pack: opening continued (disk space), otherwise
    /// confirmation "Continue without the pack" / "Cancel".
    pub fn world_pack_done(&mut self, gui: &Gui, ok: bool) -> Vec<Out> {
        let msgs = self.world_pack_msgs(&gui.lang, ok);
        self.apply(&gui.lang, msgs)
    }

    fn world_pack_msgs(&mut self, lang: &rc_text::Lang, ok: bool) -> Vec<Msg> {
        let Some(dir) = self.world_pack.take() else { return Vec::new() };
        if matches!(self.stack.last(), Some(Scr::Message(_))) {
            self.stack.pop();
        }
        if ok {
            return vec![Msg::Open(dir, Stage::Disk)];
        }
        let ask = confirm(
            lang,
            Component::translate("multiplayer.texturePrompt.failure.line1", vec![]),
            Component::translate("multiplayer.texturePrompt.failure.line2", vec![]),
            ("gui.proceed", ConfirmAction::Open(dir, Stage::Disk)),
            ("gui.cancel", ConfirmAction::CancelOpen),
        );
        vec![Msg::Push(ask)]
    }

    /// Steps of the opening from `stage`, in the game's order; each confirmation
    /// resumes at the next step.
    fn open_step(&mut self, lang: &rc_text::Lang, dir: PathBuf, mut stage: Stage, out: &mut Vec<Out>) {
        loop {
            match stage {
                // World settings and packs read in a thread (two .dat files, walk of the packs),
                // under the "Loading resources" screen that the game shows during this work;
                // the continuation resumes on the tick (`Running::Question`).
                Stage::Experimental => {
                    let d = dir.clone();
                    let th = std::thread::Builder::new()
                        .name("world-settings".into())
                        .spawn(move || rc_sim::packs::backup_question(&d));
                    match th {
                        Ok(th) => {
                            self.stack.push(Scr::Message(Box::new(Message::new("selectWorld.resource_load", None))));
                            self.running = Some(Running::Question(th, dir));
                            return;
                        }
                        Err(_) => {
                            if let Some(kind) = rc_sim::packs::backup_question(&dir) {
                                self.stack.push(Scr::Backup(Box::new(BackupConfirm::unstable(lang, &dir, kind))));
                                return;
                            }
                            stage = Stage::Pack;
                        }
                    }
                }
                Stage::Pack => {
                    let zip = dir.join("resources.zip");
                    if zip.is_file() {
                        self.stack.push(Scr::Message(Box::new(Message::new("selectWorld.resource_load", None))));
                        self.world_pack = Some(dir);
                        out.push(Out::WorldPack(zip));
                        return;
                    }
                    stage = Stage::Disk;
                }
                Stage::Disk => {
                    if rc_sim::storage::low_disk_space(&dir) {
                        let mut title = Component::translate("selectWorld.warning.lowDiskSpace.title", vec![]);
                        title.style.color = Some(0xFF5555);
                        self.stack.push(confirm(
                            lang,
                            title,
                            Component::translate("selectWorld.warning.lowDiskSpace.description", vec![]),
                            ("gui.continue", ConfirmAction::Open(dir, Stage::Load)),
                            ("gui.back", ConfirmAction::CancelOpen),
                        ));
                        return;
                    }
                    stage = Stage::Load;
                }
                Stage::Load => {
                    out.push(Out::Open(dir));
                    return;
                }
            }
        }
    }

    fn apply(&mut self, lang: &rc_text::Lang, msgs: Vec<Msg>) -> Vec<Out> {
        let mut out = Vec::new();
        for m in msgs {
            match m {
                Msg::Pop => {
                    self.stack.pop();
                    self.reload_list_on_top();
                }
                Msg::Close => {
                    self.stack.pop();
                    if !no_reload_on_cancel_on() {
                        self.reload_list_on_top();
                    }
                }
                Msg::Push(s) => self.stack.push(s),
                // Direct opening: no list underneath, the abandon goes back to the title screen.
                Msg::ToList if self.direct => self.stack.clear(),
                Msg::ToList => {
                    while self.stack.len() > 1 && !matches!(self.stack.last(), Some(Scr::List(_))) {
                        self.stack.pop();
                    }
                    match self.stack.last_mut() {
                        Some(Scr::List(l)) => l.reload(),
                        _ => {
                            // Creation opened directly (no world): list in its place.
                            self.stack.clear();
                            self.stack.push(Scr::List(Box::new(SelectWorld::new())));
                        }
                    }
                    self.list_or_create();
                }
                Msg::Open(dir, stage) => self.open_step(lang, dir, stage, &mut out),
                Msg::Title => self.stack.clear(),
                Msg::Restore(dir) => self.stack.push(restore(lang, &dir)),
                Msg::Out(o) => out.push(o),
                Msg::Job(j) => self.start(lang, j),
            }
        }
        if self.stack.is_empty() && self.running.is_none() {
            out.push(Out::Back);
        }
        out
    }

    fn start(&mut self, lang: &rc_text::Lang, j: Job) {
        match j {
            Job::Create(dir, w, packs) => {
                let data = self.data.clone();
                let d = dir.clone();
                let th = std::thread::Builder::new()
                    .name("world-creation".into())
                    .spawn(move || {
                        rc_sim::newworld::create_world(&d, &data, &w).map_err(|e| e.to_string())?;
                        rc_sim::storage::set_data_config(&d, &packs.enabled, &packs.disabled, &packs.features)
                            .map_err(|e| e.to_string())
                    })
                    .ok();
                self.stack.push(Scr::Message(Box::new(Message::new("createWorld.preparing", None))));
                if let Some(th) = th {
                    self.running = Some(Running::Create(th, dir));
                }
            }
            Job::Backup(dir, after) => {
                let id = dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let backups = crate::servers::game_dir().join("backups");
                let th = std::thread::Builder::new()
                    .name("world-copy".into())
                    .spawn(move || rc_sim::storage::backup(&dir, &backups).map_err(|e| e.to_string()))
                    .ok();
                let msg = italic_gray(&tr(lang, "selectWorld.waitingForBackup.message", &[]));
                self.stack.push(Scr::Message(Box::new(Message::new("selectWorld.waitingForBackup.title", Some(msg)))));
                if let Some(th) = th {
                    self.running = Some(Running::Backup(th, id, Some(after)));
                }
            }
        }
    }

    /// Out-of-world tick: end of the background jobs, screens that advance by themselves.
    pub fn tick(&mut self, lang: &rc_text::Lang) -> Vec<Out> {
        self.show_finished_reading();
        let mut msgs = Vec::new();
        if self.running.as_ref().is_some_and(|r| match r {
            Running::Create(th, _) => th.is_finished(),
            Running::Backup(th, ..) => th.is_finished(),
            Running::Save(th) => th.is_finished(),
            Running::Question(th, _) => th.is_finished(),
        }) {
            match self.running.take() {
                Some(Running::Create(th, dir)) => {
                    self.stack.pop();
                    match th.join().unwrap_or_else(|_| Err("creation thread stopped".into())) {
                        Ok(()) => {
                            // The world opens: the singleplayer stack is left.
                            self.stack.clear();
                            self.pending.push(Out::OpenNew(dir));
                        }
                        Err(e) => {
                            eprintln!("world creation: {e}");
                            let _ = std::fs::remove_dir_all(&dir);
                            msgs.push(Msg::Push(alert(
                                lang,
                                "selectWorld.create",
                                Component::text(e),
                                ConfirmAction::Pop,
                            )));
                        }
                    }
                }
                Some(Running::Backup(th, id, after)) => {
                    self.stack.pop();
                    let ok = match th.join().unwrap_or_else(|_| Err("copy thread stopped".into())) {
                        Ok((_, size)) => {
                            let mib = size.div_ceil(1_048_576);
                            self.pending.push(Out::toast(
                                Component::translate("selectWorld.edit.backupCreated", vec![rc_text::Arg::Text(id)]),
                                Some(Component::translate(
                                    "selectWorld.edit.backupSize",
                                    vec![rc_text::Arg::Text(mib.to_string())],
                                )),
                            ));
                            true
                        }
                        Err(e) => {
                            self.pending.push(Out::toast(
                                Component::translate("selectWorld.edit.backupFailed", vec![]),
                                Some(Component::text(e)),
                            ));
                            false
                        }
                    };
                    if let Some(a) = after {
                        msgs.push(after_backup(a, ok));
                    }
                }
                Some(Running::Save(th)) => {
                    let _ = th.join();
                    set_solo_open(false);
                    self.stack.clear();
                }
                Some(Running::Question(th, dir)) => {
                    self.stack.pop();
                    match th.join().ok().flatten() {
                        Some(kind) => {
                            msgs.push(Msg::Push(Scr::Backup(Box::new(BackupConfirm::unstable(lang, &dir, kind)))))
                        }
                        None => msgs.push(Msg::Open(dir, Stage::Pack)),
                    }
                }
                None => {}
            }
        }
        if let Some(Scr::Optimize(o)) = self.stack.last_mut() {
            msgs.extend(o.tick());
        }
        if let Some(Scr::Create(c)) = self.stack.last_mut() {
            msgs.extend(c.tick());
        }
        let mut out = std::mem::take(&mut self.pending);
        out.extend(self.apply(lang, msgs));
        if self.stack.is_empty() && self.running.is_none() && !out.iter().any(|o| matches!(o, Out::Back)) {
            out.push(Out::Back);
        }
        out
    }

    pub fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, o: &Options, mouse: Option<(f32, f32)>, time: f64) {
        // Checked at drawing, like the game: the list appears on the first frame after the reading.
        self.show_finished_reading();
        match self.stack.last_mut() {
            Some(Scr::List(s)) => s.draw(gui, q, o, mouse, time),
            Some(Scr::Create(s)) => s.draw(gui, q, o, mouse, time),
            Some(Scr::Edit(s)) => s.draw(gui, q, o, mouse, time),
            Some(Scr::Optimize(s)) => s.draw(gui, q, o, mouse, time),
            Some(Scr::Backup(s)) => s.draw(gui, q, o, mouse, time),
            Some(Scr::Recover(s)) => s.draw(gui, q, o, mouse, time),
            Some(Scr::Ask(c, _) | Scr::Alert(c, _)) => c.draw(gui, q, o, mouse, time),
            Some(Scr::Message(m)) => m.draw(gui, q, o),
            None => {}
        }
    }
}

fn italic_gray(s: &str) -> Vec<StyledChar> {
    colored(s, 0xaa_aaaa)
}

fn action_msgs(a: ConfirmAction) -> Vec<Msg> {
    match a {
        ConfirmAction::Pop => vec![Msg::Pop],
        ConfirmAction::Close => vec![Msg::Close],
        ConfirmAction::ToList => vec![Msg::ToList],
        ConfirmAction::Title => vec![Msg::Title],
        ConfirmAction::Open(d, stage) => vec![Msg::Open(d, stage)],
        ConfirmAction::CancelOpen => vec![Msg::Out(Out::DropWorldPack), Msg::ToList],
        ConfirmAction::OpenUrl(u) => {
            crate::links::open_url(u);
            Vec::new()
        }
        ConfirmAction::CopyUrl(u) => {
            crate::links::set_clipboard(u);
            Vec::new()
        }
        ConfirmAction::Delete(dir) => match rc_sim::storage::delete(&dir) {
            Ok(()) => vec![Msg::ToList],
            Err(e) => {
                // Failure (world open elsewhere…): notification, back to the list.
                eprintln!("deletion of {}: {e}", dir.display());
                let id = dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let title = Component::translate("selectWorld.delete_failure", vec![]);
                vec![Msg::ToList, Msg::Out(Out::toast(title, Some(Component::text(id))))]
            }
        },
    }
}

/// Warning of a refused world (symbolic link not allowed); "Back" (and Escape) goes back
/// to the list as it was, like the game.
fn symlink_notice(lang: &rc_text::Lang) -> Scr {
    symlink_screen(lang, ConfirmAction::Close)
}

/// World refused on opening (symbolic link not allowed in the folder): same warning,
/// but "Back" (and Escape) leads to the title screen, like the game; nothing of the world was read.
fn symlink_refused(lang: &rc_text::Lang) -> Scr {
    symlink_screen(lang, ConfirmAction::Title)
}

fn symlink_screen(lang: &rc_text::Lang, back: ConfirmAction) -> Scr {
    use crate::symlink_notice::{BACK, COPY, HELP, OPEN, column};
    let answers = vec![(OPEN, ConfirmAction::OpenUrl(HELP)), (COPY, ConfirmAction::CopyUrl(HELP)), (BACK, back)];
    Scr::Ask(Box::new(column(lang, true)), answers)
}

/// World with a refused symbolic link (logged), before editing or re-creation.
fn world_refused(dir: &Path) -> bool {
    match crate::servers::allowed_symlinks().check_world(dir) {
        Ok(refused) => {
            for f in &refused {
                eprintln!("symbolic link refused {} -> {}", f.link.display(), f.target.display());
            }
            !refused.is_empty()
        }
        Err(_) => false,
    }
}

/// Single button of an alert.
const ALERT_BUTTON: u32 = 1;

/// Game alert (title at 70, message at 90, a single "Back" button) → continuation; Escape goes back to
/// the title screen (see `Scr::Alert`).
pub(crate) fn alert(lang: &rc_text::Lang, title: &str, message: Component, then: ConfirmAction) -> Scr {
    let col = Column::new(
        Component::translate(title, vec![]),
        message,
        vec![(ALERT_BUTTON, tr(lang, "gui.back", &[]), 150)],
        20,
        20,
    )
    .alert();
    Scr::Alert(Box::new(col), then)
}

/// Confirmation (title, message, yes, no).
pub(crate) fn confirm(
    lang: &rc_text::Lang,
    title: Component,
    message: Component,
    (yes, yes_then): (&str, ConfirmAction),
    (no, no_then): (&str, ConfirmAction),
) -> Scr {
    let col = Column::new(title, message, vec![(1, tr(lang, yes, &[]), 150), (2, tr(lang, no, &[]), 150)], 20, 20);
    Scr::Ask(Box::new(col), vec![(1, yes_then), (2, no_then)])
}

/// After the copy: `level.dat_old` put back in place, then confirmation or failure.
fn restore(lang: &rc_text::Lang, dir: &Path) -> Scr {
    if rc_sim::storage::restore_old(dir) {
        confirm(
            lang,
            Component::translate("recover_world.done.title", vec![]),
            Component::translate("recover_world.done.success", vec![]),
            // "Continue": opening resumed (the game re-reads the world).
            ("gui.continue", ConfirmAction::Open(dir.to_owned(), Stage::Experimental)),
            ("gui.back", ConfirmAction::ToList),
        )
    } else {
        alert(
            lang,
            "recover_world.done.title",
            Component::translate("recover_world.done.failed", vec![]),
            ConfirmAction::ToList,
        )
    }
}

/// Input of an alert: its button → continuation; Escape → title screen (game quirk: the alert
/// closes without a continuation, and a screen closed out of a world gives way to the title screen).
fn alert_input(c: &mut Column, then: &ConfirmAction, i: &In<'_>) -> Vec<Msg> {
    let (ev, escape) = feed(&mut c.panel, i);
    if escape {
        vec![Msg::Title]
    } else if ev.contains(&Event::Press(ALERT_BUTTON)) {
        action_msgs(then.clone())
    } else {
        Vec::new()
    }
}

/// Re-creation impossible: alert with a single "Back" button (towards the list as it was), like
/// the game.
fn recreate_error(lang: &rc_text::Lang) -> Scr {
    alert(
        lang,
        "selectWorld.recreate.error.title",
        Component::translate("selectWorld.recreate.error.text", vec![]),
        ConfirmAction::Close,
    )
}

/// Continuation of a finished backup copy. The game ignores its result: the edit goes back to the
/// list even if the copy failed (`_ok` changes nothing).
fn after_backup(after: After, _ok: bool) -> Msg {
    match after {
        After::ToList => Msg::ToList,
        After::Open(dir, stage) => Msg::Open(dir, stage),
        After::Optimize(dir, erase) => {
            Msg::Push(Scr::Optimize(Box::new(crate::edit_world::Optimizing::new(&dir, erase))))
        }
        After::Restore(dir) => Msg::Restore(dir),
    }
}

// ---------- Waiting screen ----------

/// Message alone (`selectWorld.data_read`, `createWorld.preparing`, `menu.savingLevel`), or
/// wait with title, animated dots and message (backup copy).
pub(crate) struct Message {
    pub panel: Panel,
    key: &'static str,
    detail: Option<Vec<StyledChar>>,
}

impl Message {
    pub fn new(key: &'static str, detail: Option<Vec<StyledChar>>) -> Message {
        Message { panel: Panel::default(), key, detail }
    }

    fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, o: &Options) {
        background(gui, o);
        let (w, h) = (gui.width as i32, gui.height as i32);
        let title = chars(&t(gui, self.key));
        match &self.detail {
            None => draw_message_box(gui, q, &title, message_box(gui.visual_width(&title), w, h)),
            Some(d) => {
                widgets::centered(gui, q, &title, w / 2, 80, 0xffff_ffff);
                widgets::centered(gui, q, &chars(loading_dots()), w / 2, 120, 0xffa0_a0a0);
                for (k, l) in rc_text::wrap(gui, d, WAITING_MESSAGE_WIDTH).iter().enumerate() {
                    widgets::centered(gui, q, l, w / 2, 132 + 9 * k as i32, 0xffff_ffff);
                }
            }
        }
    }
}

/// Wrapping width of the message under the dots of a wait with a title (the game's fixed 360).
const WAITING_MESSAGE_WIDTH: f32 = 360.0;
/// Space between a lone message and the edge of its box.
const MESSAGE_BOX_PADDING: i32 = 12;
const MESSAGE_BOX_BORDER: u32 = 0xffa0_a0a0;

/// Box of a lone message, in interface units: as wide as the text plus the padding on both sides,
/// one line high plus the padding, centred horizontally with its top half a line above the middle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct MessageBox {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

fn message_box(text_width: i32, screen_width: i32, screen_height: i32) -> MessageBox {
    let width = text_width + 2 * MESSAGE_BOX_PADDING;
    let height = 9 + 2 * MESSAGE_BOX_PADDING;
    MessageBox { x: screen_width / 2 - width / 2, y: screen_height / 2 - 9 / 2, width, height }
}

/// Black inside (from one unit right of the left edge, like the game), one-unit gray outline, then
/// the text centred in the box below its top padding.
fn draw_message_box(gui: &mut Gui, q: &wgpu::Queue, text: &[StyledChar], b: MessageBox) {
    let f = |v: i32| v as f32;
    let (right, bottom) = (b.x + b.width, b.y + b.height);
    gui.fill(f(b.x + 1), f(b.y), f(right), f(bottom), 0xff00_0000);
    gui.fill(f(b.x), f(b.y), f(right), f(b.y + 1), MESSAGE_BOX_BORDER);
    gui.fill(f(b.x), f(bottom - 1), f(right), f(bottom), MESSAGE_BOX_BORDER);
    gui.fill(f(b.x), f(b.y + 1), f(b.x + 1), f(bottom - 1), MESSAGE_BOX_BORDER);
    gui.fill(f(right - 1), f(b.y + 1), f(right), f(bottom - 1), MESSAGE_BOX_BORDER);
    let text_y = b.y + MESSAGE_BOX_PADDING;
    widgets::centered(gui, q, text, b.x + b.width / 2, text_y, 0xffff_ffff);
}

// ---------- Backup copy before an action ----------

const B_BACKUP: u32 = 1;
const B_SKIP: u32 = 2;
const B_CANCEL: u32 = 3;
const B_ERASE: u32 = 4;

/// "Back up then …" / "… without a copy" / "Cancel" ("clear the caches" box
/// for the optimisation).
pub(crate) struct BackupConfirm {
    pub panel: Panel,
    size: (i32, i32),
    title: Vec<StyledChar>,
    message: Component,
    confirm_key: &'static str,
    erase_box: bool,
    erase: bool,
    lines: Vec<Vec<StyledChar>>,
    dir: PathBuf,
    /// Continuation: opening resumed at this step, or optimisation (None).
    next: Option<Stage>,
}

impl BackupConfirm {
    pub fn open_newer(lang: &rc_text::Lang, w: &Summary) -> BackupConfirm {
        let question = colored(&tr(lang, "selectWorld.backupQuestion.downgrade", &[]), 0xdf_5050);
        let version = w.version_name.clone().unwrap_or_else(|| tr(lang, "selectWorld.versionUnknown", &[]));
        let warning = Component::translate(
            "selectWorld.backupWarning.downgrade",
            vec![rc_text::Arg::Text(version), rc_text::Arg::Text(crate::front::VERSION_NAME.into())],
        );
        BackupConfirm {
            panel: Panel::default(),
            size: (-1, -1),
            title: question,
            message: warning,
            confirm_key: "selectWorld.backupJoinConfirmButton",
            erase_box: false,
            erase: false,
            lines: Vec::new(),
            dir: w.dir.clone(),
            next: Some(Stage::Experimental),
        }
    }

    /// World with experimental settings (or old customised world) on opening: uncoloured
    /// question, warning without an argument, then the world pack.
    pub fn unstable(lang: &rc_text::Lang, dir: &Path, kind: rc_sim::packs::Unstable) -> BackupConfirm {
        let k = kind.key();
        BackupConfirm {
            panel: Panel::default(),
            size: (-1, -1),
            title: chars(&tr(lang, &format!("selectWorld.backupQuestion.{k}"), &[])),
            message: Component::translate(format!("selectWorld.backupWarning.{k}"), vec![]),
            confirm_key: "selectWorld.backupJoinConfirmButton",
            erase_box: false,
            erase: false,
            lines: Vec::new(),
            dir: dir.to_owned(),
            next: Some(Stage::Pack),
        }
    }

    pub fn optimize(gui: &Gui, dir: &Path) -> BackupConfirm {
        BackupConfirm {
            panel: Panel::default(),
            size: (-1, -1),
            title: chars(&t(gui, "optimizeWorld.confirm.title")),
            message: Component::translate("optimizeWorld.confirm.description", vec![]),
            confirm_key: "optimizeWorld.confirm.proceed",
            erase_box: true,
            erase: false,
            lines: Vec::new(),
            dir: dir.to_owned(),
            next: None,
        }
    }

    fn layout(&mut self, gui: &Gui) {
        let size = (gui.width as i32, gui.height as i32);
        if self.size == size {
            return;
        }
        self.size = size;
        let w = size.0;
        let msg = rc_text::styled_chars(&rc_text::flatten(&self.message, &gui.lang));
        self.lines = rc_text::wrap(gui, &msg, (w - 50) as f32);
        let ts = (self.lines.len() as i32 + 1) * 9;
        let mut v = vec![
            Widget::button(B_BACKUP, w / 2 - 155, 100 + ts, 150, &t(gui, self.confirm_key)),
            Widget::button(B_SKIP, w / 2 - 155 + 160, 100 + ts, 150, &t(gui, "selectWorld.backupJoinSkipButton")),
            Widget::button(B_CANCEL, w / 2 - 155 + 80, 124 + ts, 150, &t(gui, "gui.cancel")),
        ];
        if self.erase_box {
            let label = colored(&t(gui, "selectWorld.backupEraseCache"), 0xe0_e0e0);
            let dots = rc_text::CharStyle { color: Some(0xe0_e0e0), ..Default::default() };
            v.push(Widget::checkbox(gui, B_ERASE, (w / 2 - 155 + 80, 76 + ts), self.erase, label, None, &dots));
        }
        let keyboard = self.panel.keyboard;
        self.panel = Panel::new(v);
        self.panel.keyboard = keyboard;
    }

    fn input(&mut self, gui: &Gui, i: &In<'_>) -> Vec<Msg> {
        self.layout(gui);
        let (ev, escape) = feed(&mut self.panel, i);
        let mut out = Vec::new();
        if escape {
            out.push(Msg::Pop);
        }
        for e in ev {
            match e {
                Event::Toggle(B_ERASE, on) => self.erase = on,
                Event::Press(B_CANCEL) => out.push(Msg::Pop),
                Event::Press(b @ (B_BACKUP | B_SKIP)) => {
                    let after = match self.next {
                        None => After::Optimize(self.dir.clone(), self.erase),
                        Some(stage) => After::Open(self.dir.clone(), stage),
                    };
                    // Replaced by the next step: the list below is not shown again here.
                    out.push(Msg::Close);
                    if b == B_BACKUP {
                        out.push(Msg::Job(Job::Backup(self.dir.clone(), after)));
                    } else {
                        match after {
                            After::Optimize(d, e) => {
                                out.push(Msg::Push(Scr::Optimize(Box::new(crate::edit_world::Optimizing::new(&d, e)))))
                            }
                            After::Open(d, stage) => out.push(Msg::Open(d, stage)),
                            _ => {}
                        }
                    }
                }
                _ => {}
            }
        }
        out
    }

    fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, o: &Options, mouse: Option<(f32, f32)>, time: f64) {
        self.layout(gui);
        background(gui, o);
        let w = gui.width as i32;
        widgets::centered(gui, q, &self.title, w / 2, 50, 0xffff_ffff);
        for (k, l) in self.lines.iter().enumerate() {
            widgets::centered(gui, q, l, w / 2, 70 + 9 * k as i32, 0xffff_ffff);
        }
        self.panel.draw(gui, q, mouse, time);
    }
}

// ---------- Recovery of an unreadable world ----------

const R_BUGS: u32 = 1;
const R_RESTORE: u32 = 2;
const R_BACK: u32 = 3;
const BUGS_URL: &str = "https://aka.ms/snapshotbugs?ref=game";

pub(crate) struct Recover {
    pub panel: Panel,
    size: (i32, i32),
    dir: PathBuf,
    id: String,
    issues: Vec<Vec<StyledChar>>,
    message: Vec<Vec<StyledChar>>,
    top: i32,
    can: bool,
}

impl Recover {
    fn new(dir: &Path) -> Recover {
        let id = dir.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let can = rc_sim::storage::read_level(dir, false).is_err() && rc_sim::storage::read_level(dir, true).is_ok();
        Recover {
            panel: Panel::default(),
            size: (-1, -1),
            dir: dir.to_owned(),
            id,
            issues: Vec::new(),
            message: Vec::new(),
            top: 0,
            can,
        }
    }

    /// Status line of a file: modification date, then "no problem", "file
    /// missing" or the read error.
    fn state(&self, gui: &Gui, old: bool, lang: &str) -> Option<Vec<StyledChar>> {
        let path = self.dir.join(if old { "level.dat_old" } else { "level.dat" });
        let err = rc_sim::storage::read_level(&self.dir, old).err();
        let missing = !path.is_file();
        if old && missing {
            return None;
        }
        let time =
            modified_ms(&path).map_or_else(|| t(gui, "recover_world.state_entry.unknown"), |ms| short_date(ms, lang));
        let entry = tr(&gui.lang, "recover_world.state_entry", &["\u{1}".into()]);
        let mut v = Vec::new();
        let (a, b) = entry.split_once('\u{1}').unwrap_or((entry.as_str(), ""));
        v.extend(chars(a));
        v.extend(colored(&time, 0xaa_aaaa));
        v.extend(chars(b));
        match err {
            None => v.extend(colored(&t(gui, "recover_world.issue.none"), 0x55_ff55)),
            Some(_) if missing => v.extend(colored(&t(gui, "recover_world.issue.missing_file"), 0xff_5555)),
            Some(e) => {
                // Read error without the file path.
                let s = e.to_string();
                let short =
                    s.rsplit_once(".dat : ").or_else(|| s.rsplit_once(".dat_old : ")).map_or(s.as_str(), |x| x.1);
                v.extend(colored(short, 0xff_5555));
            }
        }
        Some(v)
    }

    fn layout(&mut self, gui: &Gui, lang: &str) {
        let size = (gui.width as i32, gui.height as i32);
        if self.size == size {
            return;
        }
        self.size = size;
        let (w, h) = size;
        let id = italic_gray(&self.id);
        let msg = tr(&gui.lang, "recover_world.message", &["\u{1}".into()]);
        let (a, b) = msg.split_once('\u{1}').unwrap_or((msg.as_str(), ""));
        let mut m = chars(a);
        m.extend(id);
        m.extend(chars(b));
        self.message = rc_text::wrap(gui, &m, (w - 50) as f32);
        let mut issues = Vec::new();
        for old in [false, true] {
            if let Some(l) = self.state(gui, old, lang) {
                issues.extend(rc_text::wrap(gui, &l, (w - 50) as f32));
            } else {
                issues.push(Vec::new());
            }
        }
        self.issues = issues;
        let total = 9 + 8 + 9 * self.message.len() as i32 + 8 + 9 * self.issues.len() as i32 + 8 + 20 + 8 + 20;
        self.top = (h - total) / 2;
        let by = self.top + 9 + 8 + 9 * self.message.len() as i32 + 8 + 9 * self.issues.len() as i32 + 8;
        let mut restore = Widget::button(R_RESTORE, w / 2 + 3, by, 120, &t(gui, "recover_world.restore"));
        restore.active = self.can;
        if !self.can {
            restore.set_tooltip(gui, &t(gui, "recover_world.no_fallback"));
        }
        let v = vec![
            Widget::button(R_BUGS, w / 2 - 122, by, 120, &t(gui, "recover_world.bug_tracker")),
            restore,
            Widget::button(R_BACK, w / 2 - 60, by + 28, 120, &t(gui, "gui.back")),
        ];
        let keyboard = self.panel.keyboard;
        self.panel = Panel::new(v);
        self.panel.keyboard = keyboard;
    }

    fn input(&mut self, gui: &Gui, i: &In<'_>) -> Vec<Msg> {
        let (ev, escape) = feed(&mut self.panel, i);
        let mut out = Vec::new();
        if escape {
            out.push(Msg::ToList);
        }
        for e in ev {
            match e {
                Event::Press(R_BACK) => out.push(Msg::ToList),
                Event::Press(R_BUGS) => crate::links::open_url(BUGS_URL),
                Event::Press(R_RESTORE) if self.can => {
                    let _ = gui;
                    // Replaced by the copy then the restoration: the list is not shown again here.
                    out.push(Msg::Close);
                    out.push(Msg::Job(Job::Backup(self.dir.clone(), After::Restore(self.dir.clone()))));
                }
                _ => {}
            }
        }
        out
    }

    fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, o: &Options, mouse: Option<(f32, f32)>, time: f64) {
        self.layout(gui, o.s("lang"));
        background(gui, o);
        let w = gui.width as i32;
        let mut title = chars(&t(gui, "recover_world.title"));
        for c in &mut title {
            c.style.bold = true;
        }
        let mut y = self.top;
        widgets::centered(gui, q, &title, w / 2, y, 0xffff_ffff);
        y += 9 + 8;
        for l in &self.message {
            widgets::centered(gui, q, l, w / 2, y, 0xffff_ffff);
            y += 9;
        }
        y += 8;
        let iw = self.issues.iter().map(|l| rc_text::width(gui, l)).max().unwrap_or(0);
        for l in &self.issues {
            gui.text(q, l, (w / 2 - iw / 2) as f32, y as f32, 0xffff_ffff, true, 0);
            y += 9;
        }
        self.panel.draw(gui, q, mouse, time);
    }
}

// ---------- World list ----------

const SEARCH: u32 = 1;
const PLAY: u32 = 2;
const CREATE: u32 = 3;
const EDIT: u32 = 4;
const DELETE: u32 = 5;
const RECREATE: u32 = 6;
const BACK: u32 = 7;

const ROW_W: i32 = 270;
const ROW_H: i32 = 36;
const HEADER: i32 = 49;
const FOOTER: i32 = 60;
const GRAY: u32 = 0xff80_8080;

/// `saves` read on a background thread (`RUSTCRAFT_OPT_WORLD_LIST_ASYNC_SCAN`, on unless `0`).
fn async_scan_on() -> bool {
    rc_data::env_str!("RUSTCRAFT_OPT_WORLD_LIST_ASYNC_SCAN") != Some("0")
}

/// Back from a notice or an alert, the list is not read again, like the game
/// (`RUSTCRAFT_OPT_WORLD_LIST_NO_RELOAD_ON_CANCEL`, on unless `0`).
fn no_reload_on_cancel_on() -> bool {
    rc_data::env_str!("RUSTCRAFT_OPT_WORLD_LIST_NO_RELOAD_ON_CANCEL") != Some("0")
}

/// Row texts kept from one frame to the next (`RUSTCRAFT_OPT_WORLD_LIST_ROW_TEXT_CACHE`, on
/// unless `0`).
fn row_text_cache_on() -> bool {
    rc_data::env_str!("RUSTCRAFT_OPT_WORLD_LIST_ROW_TEXT_CACHE") != Some("0")
}

pub struct SelectWorld {
    pub panel: Panel,
    size: (i32, i32),
    worlds: Vec<Summary>,
    /// Worlds displayed (search filter): indices in `worlds`.
    shown: Vec<usize>,
    filter: String,
    /// World written in each icon slot (`dyn/server_icon/<i>`).
    slots: Vec<Option<PathBuf>>,
    title: Vec<StyledChar>,
    /// Reading of `saves` running on a background thread; the list shown stays until it ends.
    reading: Option<JoinHandle<Vec<Summary>>>,
    /// A first reading has arrived; until then the list shows the "loading" row, like the game.
    loaded: bool,
    /// Row texts by world (same indices as `worlds`), built at the first drawing of each row.
    row_texts: Vec<Option<RowText>>,
    /// Keep the row texts between frames (see [`row_text_cache_on`]).
    cache_row_texts: bool,
}

impl SelectWorld {
    pub fn new() -> SelectWorld {
        let mut list = SelectWorld {
            panel: Panel::default(),
            size: (-1, -1),
            worlds: Vec::new(),
            shown: Vec::new(),
            filter: String::new(),
            slots: vec![None; rc_assets::gui::SERVER_ICON_SLOTS],
            title: Vec::new(),
            reading: None,
            loaded: false,
            row_texts: Vec::new(),
            cache_row_texts: row_text_cache_on(),
        };
        list.reload();
        list
    }

    /// Reading of `saves` (opening, return from a sub-screen): on a background thread like the
    /// game, the current list staying shown until it ends (`RUSTCRAFT_OPT_WORLD_LIST_ASYNC_SCAN=0`:
    /// read here, the old path). A reading still running is dropped for the new one.
    fn reload(&mut self) {
        let game_dir = crate::servers::game_dir();
        self.reading = None;
        if async_scan_on() {
            let dir = game_dir.clone();
            match std::thread::Builder::new().name("world-list".into()).spawn(move || scan(&dir)) {
                Ok(th) => {
                    self.reading = Some(th);
                    return;
                }
                Err(e) => eprintln!("world list read in place: {e}"),
            }
        }
        self.show_worlds(scan(&game_dir));
    }

    /// Background reading ended: its worlds are shown. True when it ended now.
    fn take_finished_reading(&mut self) -> bool {
        if !self.reading.as_ref().is_some_and(JoinHandle::is_finished) {
            return false;
        }
        let worlds = self.reading.take().and_then(|th| th.join().ok()).unwrap_or_default();
        self.show_worlds(worlds);
        true
    }

    /// Worlds of a reading shown: filtered, selection kept by folder, icons and row texts redone.
    fn show_worlds(&mut self, worlds: Vec<Summary>) {
        let sel = self.selected().map(|w| w.dir.clone());
        self.worlds = worlds;
        self.loaded = true;
        self.refilter();
        if let Some(l) = &mut self.panel.list {
            l.selected = sel.and_then(|d| self.shown.iter().position(|&i| self.worlds[i].dir == d));
        }
        self.slots.iter_mut().for_each(|s| *s = None);
        self.row_texts = vec![None; self.worlds.len()];
        // Laid out again, unless never laid out yet (its first layout focuses the search).
        if self.size != (-1, -1) {
            self.size = (-2, -2);
        }
    }

    fn refilter(&mut self) {
        let f = self.filter.to_lowercase();
        self.shown = (0..self.worlds.len())
            .filter(|&i| {
                let w = &self.worlds[i];
                w.name.to_lowercase().contains(&f) || w.id.to_lowercase().contains(&f)
            })
            .collect();
    }

    fn selected(&self) -> Option<&Summary> {
        let i = self.panel.list.as_ref()?.selected?;
        self.worlds.get(*self.shown.get(i)?)
    }

    pub fn layout(&mut self, gui: &Gui) {
        let size = (gui.width as i32, gui.height as i32);
        if self.size == size {
            return;
        }
        let first = self.size.0 == -1;
        self.size = size;
        let (w, h) = size;
        self.title = chars(&t(gui, "selectWorld.title"));
        let old = self.panel.list.take();
        let mut l = List::new(0, HEADER, w, h - HEADER - FOOTER, ROW_W, ROW_H);
        l.inworld = false;
        l.custom_rows = true;
        // Before the first reading, a single row: the "loading" one.
        let n = if self.loaded { self.shown.len() as i32 } else { 1 };
        l.entries = (0..n).map(|i| Entry { top: i * ROW_H, h: ROW_H, label: Vec::new() }).collect();
        l.content = n * ROW_H + 4;
        if let Some(o) = old {
            l.selected = o.selected.filter(|&s| s < self.shown.len());
            l.set_scroll(o.scroll);
        }
        let x0 = (w - 308) / 2;
        let (y1, y2) = (h - 52, h - 28);
        let mut field = TextField::new(chars(&format!("§7§o{}", t(gui, "gui.selectWorld.search"))));
        field.text = self.filter.chars().collect();
        field.cursor = field.text.len();
        field.anchor = field.cursor;
        let search = Widget::new(SEARCH, Kind::Field(Box::new(field)), w / 2 - 100, 21, 200, 20, Vec::new());
        let focus = self.panel.focus.and_then(|i| self.panel.widgets.get(i)).map(|wd| wd.id);
        let keyboard = self.panel.keyboard;
        self.panel = Panel::new(vec![
            search,
            Widget::button(PLAY, x0, y1, 150, &t(gui, "selectWorld.select")),
            Widget::button(CREATE, x0 + 158, y1, 150, &t(gui, "selectWorld.create")),
            Widget::button(EDIT, x0, y2, 71, &t(gui, "selectWorld.edit")),
            Widget::button(DELETE, x0 + 79, y2, 71, &t(gui, "selectWorld.delete")),
            Widget::button(RECREATE, x0 + 158, y2, 71, &t(gui, "selectWorld.recreate")),
            Widget::button(BACK, x0 + 237, y2, 71, &t(gui, "gui.back")),
        ]);
        self.panel.list = Some(l);
        self.panel.keyboard = keyboard;
        // Initial focus: the search; otherwise the previous focus.
        match focus {
            _ if first => self.panel.focus_widget(SEARCH),
            Some(id) => self.panel.focus = self.panel.widgets.iter().position(|wd| wd.id == id),
            None => {}
        }
        self.update_buttons(gui);
    }

    fn update_buttons(&mut self, gui: &Gui) {
        let sel = self.selected().cloned();
        let set = |p: &mut Panel, id: u32, on: bool| {
            if let Some(w) = p.get_mut(id) {
                w.active = on;
            }
        };
        let Some(s) = sel else {
            for id in [PLAY, EDIT, DELETE, RECREATE] {
                set(&mut self.panel, id, false);
            }
            if let Some(p) = self.panel.get_mut(PLAY) {
                p.msg = chars(&t(gui, "selectWorld.select"));
                p.tooltip.clear();
            }
            return;
        };
        let label = if s.symlink {
            "symlink_warning.more_info"
        } else if s.corrupted {
            "recover_world.button"
        } else if s.older() {
            "selectWorld.upgrade_and_play"
        } else {
            "selectWorld.select"
        };
        set(&mut self.panel, PLAY, s.can_play());
        set(&mut self.panel, EDIT, s.can_edit());
        set(&mut self.panel, RECREATE, s.can_edit());
        set(&mut self.panel, DELETE, true);
        let older = s.older() && !s.disabled();
        for (id, key) in [
            (PLAY, "selectWorld.requiresFileFixingTooltip.play"),
            (EDIT, "selectWorld.requiresFileFixingTooltip.edit"),
            (RECREATE, "selectWorld.requiresFileFixingTooltip.recreate"),
        ] {
            if let Some(wd) = self.panel.get_mut(id) {
                if older {
                    let mut tip = t(gui, key);
                    if id == PLAY {
                        // Rustcraft does not convert worlds of older versions yet.
                        tip.push_str("\n(Rustcraft: conversion of older versions not supported)");
                    }
                    wd.set_tooltip(gui, &tip);
                } else {
                    wd.tooltip.clear();
                }
            }
        }
        if let Some(p) = self.panel.get_mut(PLAY) {
            p.msg = chars(&t(gui, label));
        }
    }

    /// Opening of the chosen world: recovery, copy offered (more recent version), otherwise play.
    fn join(lang: &rc_text::Lang, s: &Summary) -> Vec<Msg> {
        if !s.can_play() {
            return Vec::new();
        }
        if s.symlink {
            return vec![Msg::Push(symlink_notice(lang))];
        }
        // The game checks the links of the whole folder when opening access to the world, before any
        // reading (recovery, copy offered, settings, world pack).
        if world_refused(&s.dir) {
            return vec![Msg::Push(symlink_refused(lang))];
        }
        if s.corrupted {
            return vec![Msg::Push(Scr::Recover(Box::new(Recover::new(&s.dir))))];
        }
        if s.downgrade() {
            return vec![Msg::Push(Scr::Backup(Box::new(BackupConfirm::open_newer(lang, s))))];
        }
        vec![Msg::Open(s.dir.clone(), Stage::Experimental)]
    }

    fn input(&mut self, gui: &Gui, i: &In<'_>) -> Vec<Msg> {
        self.layout(gui);
        // Click on the icon of a row: play.
        if let In::Press(m, 0, _) = i
            && let Some(l) = &self.panel.list
            && let Some(k) = l.entry_under(*m)
            && let Some((x, _, _, _)) = l.entry_rect(k)
            && m.0 - ((x + 2) as f32) < 32.0
        {
            if let Some(l) = &mut self.panel.list {
                l.selected = Some(k);
            }
            self.update_buttons(gui);
            if let Some(s) = self.selected().cloned()
                && s.can_play()
            {
                self.panel.clicks += 1;
                return Self::join(&gui.lang, &s);
            }
        }
        let (ev, escape) = feed(&mut self.panel, i);
        let mut out = Vec::new();
        if escape {
            out.push(Msg::Pop);
        }
        for e in ev {
            match e {
                Event::Edit(SEARCH) => {
                    if let Some(wd) = self.panel.get(SEARCH)
                        && let Kind::Field(f) = &wd.kind
                    {
                        let v = f.value();
                        if v != self.filter {
                            let sel = self.selected().map(|w| w.dir.clone());
                            self.filter = v;
                            self.refilter();
                            self.size = (-2, -2);
                            self.layout(gui);
                            if let Some(l) = &mut self.panel.list {
                                l.selected = sel.and_then(|d| self.shown.iter().position(|&i| self.worlds[i].dir == d));
                                l.set_scroll(l.scroll);
                            }
                            self.update_buttons(gui);
                        }
                    }
                }
                Event::Select(_) => self.update_buttons(gui),
                Event::Activate(_) | Event::Press(PLAY) => {
                    // Double click or Enter on a world: click sound (the Play button has its own).
                    if matches!(e, Event::Activate(_)) {
                        self.panel.clicks += 1;
                    }
                    if let Some(s) = self.selected().cloned() {
                        out.extend(Self::join(&gui.lang, &s));
                    }
                }
                Event::Press(CREATE) => {
                    let data = crate::create_world::shared_data();
                    if let Some(data) = data {
                        out.push(Msg::Push(Scr::Create(Box::new(crate::create_world::CreateWorld::fresh(data)))));
                    }
                }
                Event::Press(EDIT) => {
                    if let Some(s) = self.selected().cloned() {
                        if world_refused(&s.dir) {
                            out.push(Msg::Push(symlink_notice(&gui.lang)));
                            continue;
                        }
                        match crate::edit_world::EditWorld::new(&s) {
                            Ok(e) => out.push(Msg::Push(Scr::Edit(Box::new(e)))),
                            Err(e) => {
                                eprintln!("edit of {}: {e}", s.id);
                                out.push(Msg::Out(Out::toast(
                                    Component::translate("selectWorld.access_failure", vec![]),
                                    Some(Component::text(s.id.clone())),
                                )));
                            }
                        }
                    }
                }
                Event::Press(DELETE) => {
                    if let Some(s) = self.selected().cloned() {
                        out.push(Msg::Push(confirm(
                            &gui.lang,
                            Component::translate("selectWorld.deleteQuestion", vec![]),
                            Component::translate("selectWorld.deleteWarning", vec![rc_text::Arg::Text(s.name.clone())]),
                            ("selectWorld.deleteButton", ConfirmAction::Delete(s.dir.clone())),
                            ("gui.cancel", ConfirmAction::Pop),
                        )));
                    }
                }
                Event::Press(RECREATE) => {
                    if let Some(s) = self.selected().cloned()
                        && let Some(data) = crate::create_world::shared_data()
                    {
                        if world_refused(&s.dir) {
                            out.push(Msg::Push(symlink_notice(&gui.lang)));
                            continue;
                        }
                        match crate::create_world::CreateWorld::recreate(data, &s.dir) {
                            Ok(c) => out.push(Msg::Push(Scr::Create(Box::new(c)))),
                            Err(e) => {
                                eprintln!("re-creation of {}: {e}", s.id);
                                out.push(Msg::Push(recreate_error(&gui.lang)));
                            }
                        }
                    }
                }
                Event::Press(BACK) => out.push(Msg::Pop),
                _ => {}
            }
        }
        out
    }

    pub fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, o: &Options, mouse: Option<(f32, f32)>, time: f64) {
        self.layout(gui);
        background(gui, o);
        let w = gui.width as i32;
        widgets::centered(gui, q, &self.title, w / 2, 8, 0xffff_ffff);
        let lang = o.s("lang").to_owned();
        let focused = self.panel.focus.is_none();
        let selected = self.panel.list.as_ref().and_then(|l| l.selected);
        let SelectWorld { panel, worlds, shown, slots, loaded, row_texts, cache_row_texts, .. } = self;
        let mut tooltip: Tip = None;
        panel.draw_with(gui, q, mouse, time, &mut |gui, q, l| {
            if !*loaded {
                if let Some((_, top, _, rh)) = l.entry_rect(0) {
                    draw_loading_row(gui, q, top, rh);
                }
                return;
            }
            let hovered = mouse.and_then(|m| l.entry_under(m));
            for k in 0..l.entries.len() {
                let Some((left, top, rw, rh)) = l.entry_rect(k) else { continue };
                if top + rh < l.y || top > l.y + l.h {
                    continue;
                }
                let Some(s) = shown.get(k).and_then(|&i| worlds.get(i)) else { continue };
                if selected == Some(k) {
                    let c = if focused { 0xffff_ffff } else { GRAY };
                    let f = |v: i32| v as f32;
                    gui.fill(f(left), f(top), f(left + rw), f(top + rh), c);
                    gui.fill(f(left + 1), f(top + 1), f(left + rw - 1), f(top + rh - 1), 0xff00_0000);
                }
                let build = |i: usize| RowText::new(gui, &gui.lang, &lang, &worlds[i]);
                let fresh;
                let text = match row_text_at(row_texts, shown, k).filter(|_| *cache_row_texts) {
                    Some((i, kept)) => kept.get_or_insert_with(|| build(i)),
                    None => {
                        fresh = RowText::new(gui, &gui.lang, &lang, s);
                        &fresh
                    }
                };
                let spot = RowSpot { rank: k, x: left + 2, y: top + 2, hovered: hovered == Some(k), mouse };
                let tip = draw_row(gui, q, s, text, slots, &spot);
                if tip.is_some() {
                    tooltip = tip;
                }
            }
        });
        if let Some((lines, rect)) = tooltip {
            gui.next_layer();
            widgets::draw_tooltip(gui, q, &lines, rect, mouse);
        }
    }
}

/// Row shown until the first reading of the list arrives, like the game: "Loading worlds"
/// centered on the screen, animated dots below in gray.
fn draw_loading_row(gui: &mut Gui, q: &wgpu::Queue, top: i32, height: i32) {
    let w = gui.width as i32;
    let y = top + 2 + (height - 4 - 9) / 2;
    for (text, dy, color) in [(t(gui, "selectWorld.loading_list"), 0, 0xffff_ffff), (loading_dots().into(), 9, GRAY)] {
        let text = chars(&text);
        let x = (w - rc_text::width(gui, &text)) / 2;
        gui.text(q, &text, x as f32, (y + dy) as f32, color, true, 0);
    }
}

/// Text width of a row: row minus its padding and the icon column.
const ROW_TEXT_W: i32 = ROW_W - 4 - 35;

/// The three lines of a world's row (name, folder and date, information), each as written and as
/// shown (cut with "..." to the row width). Built once per reading and kept, like the game which
/// builds them with the row: the date keeps the time zone of that moment, instead of a system
/// call and three text cuts per row every frame.
#[derive(Clone, Debug, PartialEq)]
struct RowText {
    lines: [(Vec<StyledChar>, Vec<StyledChar>); 3],
}

impl RowText {
    /// `lang`: the translations; `date_lang`: the language code that picks the date format.
    fn new(m: &dyn rc_text::Metrics, lang: &rc_text::Lang, date_lang: &str, s: &Summary) -> RowText {
        let mut id_date = s.id.clone();
        if s.last_played != -1 {
            id_date.push_str(&format!(" ({})", short_date(s.last_played, date_lang)));
        }
        let full = [chars(&s.name), chars(&id_date), s.info(lang)];
        RowText {
            lines: full.map(|text| {
                let shown = rc_text::clamp(m, &text, ROW_TEXT_W);
                (text, shown)
            }),
        }
    }
}

/// Kept text slot of the row at `rank`, with its world's index: texts are kept by world, not by
/// rank, since a search changes which world a rank shows.
fn row_text_at<'a>(
    texts: &'a mut [Option<RowText>],
    shown: &[usize],
    rank: usize,
) -> Option<(usize, &'a mut Option<RowText>)> {
    let i = *shown.get(rank)?;
    Some((i, texts.get_mut(i)?))
}

/// Where a row is drawn and where the mouse is.
struct RowSpot {
    /// Rank of the row in the list (gives its icon slot).
    rank: usize,
    /// Top left corner of the row's content.
    x: i32,
    y: i32,
    /// The mouse is over this row.
    hovered: bool,
    mouse: Option<(f32, f32)>,
}

impl RowSpot {
    fn mouse_in(&self, x: i32, y: i32, w: i32, h: i32) -> bool {
        self.mouse.is_some_and(|m| m.0 >= x as f32 && m.0 < (x + w) as f32 && m.1 >= y as f32 && m.1 < (y + h) as f32)
    }
}

/// A row of the list: its three lines, the icon, and on hover the veil and pictogram of the icon
/// (play, warning, error). Returns the hovered tooltip, if any.
fn draw_row(
    gui: &mut Gui,
    q: &wgpu::Queue,
    s: &Summary,
    text: &RowText,
    slots: &mut [Option<PathBuf>],
    spot: &RowSpot,
) -> Tip {
    let mut tip = draw_row_lines(gui, q, text, spot);
    draw_row_icon(gui, q, s, slots, spot);
    if spot.hovered
        && let Some(icon_tip) = draw_row_hover(gui, s, spot)
    {
        tip = Some(icon_tip);
    }
    tip
}

/// The three lines; a hovered line that was cut gives its whole text as tooltip.
fn draw_row_lines(gui: &mut Gui, q: &wgpu::Queue, text: &RowText, spot: &RowSpot) -> Tip {
    let mut tip = None;
    let x = spot.x + 35;
    for ((full, shown), (dy, color)) in text.lines.iter().zip([(1, 0xffff_ffff), (12, GRAY), (21, GRAY)]) {
        let y = spot.y + dy;
        if shown.len() != full.len() && spot.hovered && spot.mouse_in(x, y, ROW_TEXT_W, 9) {
            tip = Some((vec![full.clone()], (x, y, ROW_TEXT_W, 9)));
        }
        gui.text(q, shown, x as f32, y as f32, color, true, 0);
    }
    tip
}

/// The world's icon, its atlas slot rewritten when the world shown in that slot changes; the
/// "unknown" icon otherwise.
fn draw_row_icon(gui: &mut Gui, q: &wgpu::Queue, s: &Summary, slots: &mut [Option<PathBuf>], spot: &RowSpot) {
    let mut icon = "tex/misc/unknown_server".to_owned();
    if let Some(rgba) = &s.icon {
        let slot = spot.rank % slots.len().max(1);
        let name = format!("dyn/server_icon/{slot}");
        if slots[slot].as_deref() != Some(s.dir.as_path()) && gui.write_sprite(q, &name, (64, 64), rgba) {
            slots[slot] = Some(s.dir.clone());
        }
        if slots[slot].as_deref() == Some(s.dir.as_path()) {
            icon = name;
        }
    }
    gui.sprite(&icon, spot.x as f32, spot.y as f32, 32.0, 32.0, 0xffff_ffff);
}

/// Hovered row: veil over the icon and the pictogram of what "Play" does there (highlighted under
/// the mouse); the icon's tooltip when the mouse is on it.
fn draw_row_hover(gui: &mut Gui, s: &Summary, spot: &RowSpot) -> Tip {
    let (x, y) = (spot.x as f32, spot.y as f32);
    gui.fill(x, y, x + 32.0, y + 32.0, 0xa090_9090);
    let over = spot.mouse.is_some_and(|m| m.0 - x < 32.0 && m.0 >= x);
    let sprite_name = |n: &str| if over { format!("world_list/{n}_highlighted") } else { format!("world_list/{n}") };
    let wrapped_tooltip =
        |gui: &Gui, key: &str| -> Vec<Vec<StyledChar>> { rc_text::wrap(gui, &colored(&t(gui, key), 0xff_5555), 175.0) };
    let draw = |gui: &mut Gui, names: &[&str]| {
        for n in names {
            gui.sprite(&sprite_name(n), x, y, 32.0, 32.0, 0xffff_ffff);
        }
    };
    let mut icon_tip = None;
    if s.corrupted || s.symlink {
        draw(gui, &["error", "marked_join"]);
    } else if s.locked {
        draw(gui, &["error"]);
        icon_tip = Some(wrapped_tooltip(gui, "selectWorld.locked"));
    } else if !s.compatible() {
        draw(gui, &["error"]);
        icon_tip = Some(wrapped_tooltip(gui, "selectWorld.incompatible.tooltip"));
    } else if s.downgrade() {
        draw(gui, &["marked_join", "error"]);
        icon_tip = Some(
            ["selectWorld.tooltip.fromNewerVersion1", "selectWorld.tooltip.fromNewerVersion2"]
                .iter()
                .map(|k| colored(&t(gui, k), 0xff_5555))
                .collect(),
        );
        if over {
            widgets::request_cursor(widgets::Cursor::Hand);
        }
    } else if s.older() {
        draw(gui, &["marked_join"]);
    } else {
        draw(gui, &["join"]);
        if over {
            widgets::request_cursor(widgets::Cursor::Hand);
        }
    }
    icon_tip.filter(|_| over).map(|lines| (lines, (spot.x, spot.y, 32, 32)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lang() -> rc_text::Lang {
        rc_text::Lang::default()
    }

    fn solo() -> Solo {
        Solo::empty(Arc::new(VersionData::default()))
    }

    /// Background work finished, then a tick.
    fn settle(s: &mut Solo) -> Vec<Out> {
        while s.running.as_ref().is_some_and(|r| match r {
            Running::Create(th, _) => !th.is_finished(),
            Running::Backup(th, ..) => !th.is_finished(),
            Running::Save(th) => !th.is_finished(),
            Running::Question(th, _) => !th.is_finished(),
        }) {
            std::thread::yield_now();
        }
        wait_for_reading(s);
        s.tick(&lang())
    }

    /// Background reading of the top list finished (not yet shown: the next tick or frame does).
    fn wait_for_reading(s: &Solo) {
        while let Some(Scr::List(l)) = s.stack.last()
            && l.reading.as_ref().is_some_and(|th| !th.is_finished())
        {
            std::thread::yield_now();
        }
    }

    fn select_world(worlds: Vec<Summary>) -> SelectWorld {
        let shown = (0..worlds.len()).collect();
        SelectWorld {
            panel: Panel::default(),
            size: (-1, -1),
            row_texts: vec![None; worlds.len()],
            worlds,
            shown,
            filter: String::new(),
            slots: Vec::new(),
            title: Vec::new(),
            reading: None,
            loaded: true,
            cache_row_texts: true,
        }
    }

    fn list(worlds: Vec<Summary>) -> Scr {
        Scr::List(Box::new(select_world(worlds)))
    }

    fn summary(dir: &Path) -> Summary {
        Summary {
            dir: dir.to_owned(),
            id: "w".into(),
            name: "w".into(),
            last_played: -1,
            game_mode: 0,
            hardcore: false,
            commands: false,
            experimental: false,
            version_name: None,
            series: "main".into(),
            data_version: 0,
            locked: false,
            corrupted: false,
            icon: None,
            symlink: false,
        }
    }

    fn tmp(name: &str) -> PathBuf {
        let d = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-solo-{name}-{}", std::process::id())));
        std::fs::create_dir_all(&d).expect("folder");
        d
    }

    /// The game's box of a lone message: text width plus 12 on each side, 33 high, centred, top at
    /// h/2 - 4, text 12 below the top.
    #[test]
    fn lone_message_box_geometry() {
        let b = message_box(100, 533, 300);
        assert_eq!(b, MessageBox { x: 266 - 62, y: 146, width: 124, height: 33 });
        // Odd widths: both halves truncated, like the game's integer division.
        assert_eq!(message_box(101, 427, 240), MessageBox { x: 213 - 62, y: 116, width: 125, height: 33 });
    }

    /// "Saving world" alone: drawn in a black box with a gray outline (the text sat on the bare
    /// background before). Rendered at 1600 × 900, scale 3. Without a GPU: skipped.
    #[test]
    fn lone_message_is_drawn_in_its_bordered_box() {
        use crate::gui::tests_opt;
        use crate::render::upload::Uploader;
        let Some(e) = tests_opt::env() else { return };
        let mut gui = tests_opt::gui(&e, tests_opt::ALL);
        gui.scale_option = 3;
        gui.begin(tests_opt::W, tests_opt::H);
        gui.overlays_here();
        gui.models_here();
        let mut screen = Message::new("menu.savingLevel", None);
        screen.draw(&mut gui, &e.queue, &Options::default());
        let text = chars(&t(&gui, "menu.savingLevel"));
        let b = message_box(gui.visual_width(&text), gui.width as i32, gui.height as i32);
        let scale = gui.scale;
        let pixels = tests_opt::render(&e, &mut gui, &mut Uploader::new(&e.device, true));
        // Centre of one interface unit, in pixels of the 1600-wide frame.
        let rgb = |x: i32, y: i32| {
            let (px, py) = (x as u32 * scale + scale / 2, y as u32 * scale + scale / 2);
            let i = ((py * tests_opt::W + px) * 4) as usize;
            [pixels[i], pixels[i + 1], pixels[i + 2]]
        };
        let gray = [0xa0, 0xa0, 0xa0];
        assert_eq!(rgb(b.x + b.width / 2, b.y), gray, "top edge of the box");
        assert_eq!(rgb(b.x, b.y + b.height / 2), gray, "left edge of the box");
        assert_eq!(rgb(b.x + b.width - 1, b.y + b.height - 1), gray, "bottom right corner of the box");
        assert_eq!(rgb(b.x + 3, b.y + 3), [0, 0, 0], "inside of the box (padding)");
    }

    /// Backup copy of the edit: back to the list, successful or not (the old code left
    /// the edit open after a failure).
    #[test]
    fn failed_edit_backup_returns_to_the_list() {
        for ok in [false, true] {
            let mut s = solo();
            s.stack.push(list(vec![summary(Path::new("w"))]));
            // Edit screen (replaced by an alert) then waiting for the copy above.
            s.stack.push(recreate_error(&lang()));
            s.stack.push(Scr::Message(Box::new(Message::new("selectWorld.waitingForBackup.title", None))));
            let th =
                std::thread::spawn(move || if ok { Ok((PathBuf::from("c"), 1)) } else { Err("failure".to_string()) });
            while !th.is_finished() {
                std::thread::yield_now();
            }
            s.running = Some(Running::Backup(th, "w".into(), Some(After::ToList)));
            let out = s.tick(&lang());
            assert_eq!(s.stack.len(), 1, "copy {ok}: back to the list");
            // The list is read again from the game folder: without any world, creation opens instead.
            wait_for_reading(&s);
            s.show_finished_reading();
            if scan(&crate::servers::game_dir()).is_empty() {
                assert!(matches!(s.stack.last(), Some(Scr::Create(_))));
            } else {
                assert!(matches!(s.stack.last(), Some(Scr::List(_))));
            }
            let key = if ok { "selectWorld.edit.backupCreated" } else { "selectWorld.edit.backupFailed" };
            assert!(
                out.iter().any(|o| matches!(o, Out::Toast(b) if matches!(&b.0.content, rc_text::Content::Translate { key: k, .. } if k == key))),
                "notification {key}"
            );
        }
    }

    /// Last world deleted (list re-read empty): creation opens in place of the list;
    /// a non-empty list stays.
    #[test]
    fn empty_list_opens_creation() {
        let mut s = solo();
        s.stack.push(list(Vec::new()));
        s.list_or_create();
        assert!(matches!(s.stack.last(), Some(Scr::Create(_))), "empty list: creation");
        assert_eq!(s.stack.len(), 1);
        let mut s = solo();
        s.stack.push(list(vec![summary(Path::new("w"))]));
        s.list_or_create();
        assert!(matches!(s.stack.last(), Some(Scr::List(_))));
    }

    /// Background reading, like the game: the "loading" row until the first reading arrives (no
    /// creation opened before it, entries ignored), then its worlds; an empty reading opens the
    /// creation. A reading after a return keeps the list shown, then replaces it, selection kept.
    #[test]
    fn list_shows_its_reading_when_it_ends() {
        let reading = |worlds: Vec<Summary>| Some(std::thread::spawn(move || worlds));
        for found in [Vec::new(), vec![summary(Path::new("w"))]] {
            let mut s = solo();
            let mut l = select_world(Vec::new());
            l.loaded = false;
            l.reading = reading(found.clone());
            s.stack.push(Scr::List(Box::new(l)));
            s.list_or_create();
            assert!(matches!(s.stack.last(), Some(Scr::List(l)) if !l.loaded), "loading row, no creation yet");
            assert!(settle(&mut s).is_empty());
            if found.is_empty() {
                assert!(matches!(s.stack.last(), Some(Scr::Create(_))), "empty: creation");
            } else {
                assert!(matches!(s.stack.last(), Some(Scr::List(l)) if l.loaded && l.worlds == found));
            }
        }
        let (a, b) = (summary(Path::new("a")), summary(Path::new("b")));
        let mut l = select_world(vec![a.clone(), b.clone()]);
        let mut rows = List::new(0, HEADER, 400, 300, ROW_W, ROW_H);
        rows.selected = Some(1);
        l.panel.list = Some(rows);
        let mut b2 = b.clone();
        b2.name = "b renamed".into();
        l.reading = reading(vec![b2.clone(), a.clone()]);
        while !l.take_finished_reading() {
            assert_eq!(l.worlds, [a.clone(), b.clone()], "shown list kept while the reading runs");
            std::thread::yield_now();
        }
        assert_eq!(l.worlds, [b2, a]);
        assert_eq!(l.panel.list.as_ref().and_then(|r| r.selected), Some(0), "selection follows the folder");
        assert_eq!(l.row_texts.len(), 2);
    }

    /// Back from a notice or an alert: the list shown again as it was (no reading), like the game;
    /// a return the game reads again (cancel of a deletion…) starts a reading.
    #[test]
    fn notice_back_does_not_read_the_list_again() {
        let Scr::Ask(_, answers) = symlink_notice(&lang()) else { panic!("notice expected") };
        assert!(matches!(answers.last(), Some((_, ConfirmAction::Close))));
        let Scr::Alert(_, back) = recreate_error(&lang()) else { panic!("alert expected") };
        assert!(matches!(back, ConfirmAction::Close));
        let mut s = solo();
        s.stack.push(list(vec![summary(Path::new("w"))]));
        s.stack.push(recreate_error(&lang()));
        let _ = s.apply(&lang(), action_msgs(back));
        let Some(Scr::List(l)) = s.stack.last() else { panic!("list expected") };
        if no_reload_on_cancel_on() {
            assert!(l.reading.is_none() && l.worlds.len() == 1, "same list, nothing read");
        }
        wait_for_reading(&s);
        if async_scan_on() {
            s.stack.push(recreate_error(&lang()));
            let _ = s.apply(&lang(), vec![Msg::Pop]);
            assert!(matches!(s.stack.last(), Some(Scr::List(l)) if l.reading.is_some()), "read again");
            wait_for_reading(&s);
        }
    }

    /// Fixed-width text metrics (6 per character).
    struct Fixed;

    impl rc_text::Metrics for Fixed {
        fn advance(&self, _font: &str, _c: char) -> (f32, f32) {
            (6.0, 1.0)
        }
    }

    /// Row texts kept by world, not by row: after a search narrows the list, the row shows its
    /// world's text; a long name is cut and keeps its whole text for the tooltip.
    #[test]
    fn row_texts_follow_their_world() {
        let mut long = summary(Path::new("long"));
        long.id = "long".into();
        long.name = "A world with a name far too long for its row".into();
        long.last_played = 1_700_000_000_000;
        let mut short = summary(Path::new("short"));
        short.id = "short".into();
        short.name = "Short".into();
        let lang = rc_text::Lang::default();
        let text = RowText::new(&Fixed, &lang, "en_us", &long);
        let (full, shown) = &text.lines[0];
        assert!(shown.len() < full.len() && rc_text::width(&Fixed, shown) <= ROW_TEXT_W, "name cut");
        assert!(text.lines[1].0.iter().map(|c| c.c).collect::<String>().starts_with("long ("), "dated");
        let mut l = select_world(vec![long.clone(), short.clone()]);
        l.row_texts[0] = Some(text.clone());
        l.filter = "short".into();
        l.refilter();
        assert_eq!(l.shown, [1]);
        let (i, kept) = row_text_at(&mut l.row_texts, &l.shown, 0).expect("row 0");
        assert_eq!(i, 1);
        let shown_text = kept.get_or_insert_with(|| RowText::new(&Fixed, &lang, "en_us", &l.worlds[i]));
        assert_eq!(*shown_text, RowText::new(&Fixed, &lang, "en_us", &short));
        assert!(row_text_at(&mut l.row_texts, &l.shown, 1).is_none(), "no row 1");
        assert_eq!(l.row_texts[0], Some(text), "the other world's text untouched");
    }

    /// A folder of saves covering the list's cases: played worlds in both orders, a newer and an
    /// older version, an unreadable `level.dat`, only `level.dat_old`, no level file at all (not
    /// listed), a locked world, a 64 × 64 icon and an icon of another size, folder names that sort
    /// differently in UTF-16 than in UTF-8.
    fn sample_saves(root: &Path) -> rc_sim::lock::SessionLock {
        let level = |dir: &str, last: i64, version: i32| {
            let data = Nbt::Compound(vec![
                ("LevelName".into(), Nbt::String(format!("Name of {dir}"))),
                ("LastPlayed".into(), Nbt::Long(last)),
                ("GameType".into(), Nbt::Int(1)),
                ("DataVersion".into(), Nbt::Int(version)),
                ("Version".into(), Nbt::Compound(vec![("Name".into(), Nbt::String("26.3".into()))])),
            ]);
            let path = root.join("saves").join(dir);
            std::fs::create_dir_all(&path).expect("folder");
            let root = Nbt::Compound(vec![("Data".into(), data)]);
            std::fs::write(path.join("level.dat"), rc_sim::nbtio::gzip(&rc_sim::nbtio::encode_root(&root)))
                .expect("level.dat");
            path
        };
        let now = rc_sim::chunk::DATA_VERSION;
        level("played", 3_000, now);
        level("before", 1_000, now);
        level("newer", 2_000, now + 1);
        level("older", 2_000, now - 1);
        // Same date: by folder, U+FFFD before U+1F600 in UTF-8, after it in UTF-16.
        level("\u{fffd}", 500, now);
        level("\u{1f600}", 500, now);
        let broken = root.join("saves/broken");
        std::fs::create_dir_all(&broken).expect("folder");
        std::fs::write(broken.join("level.dat"), b"not nbt").expect("file");
        let old_only = root.join("saves/old only");
        std::fs::create_dir_all(&old_only).expect("folder");
        std::fs::write(old_only.join("level.dat_old"), b"x").expect("file");
        std::fs::create_dir_all(root.join("saves/empty")).expect("folder");
        let icon = |dir: &Path, side: u32| {
            let mut bytes = Vec::new();
            let mut enc = png::Encoder::new(&mut bytes, side, side);
            enc.set_color(png::ColorType::Rgba);
            let mut w = enc.write_header().expect("png");
            w.write_image_data(&vec![200; (side * side * 4) as usize]).expect("png");
            drop(w);
            std::fs::write(dir.join("icon.png"), bytes).expect("icon");
        };
        icon(&root.join("saves/played"), 64);
        icon(&root.join("saves/before"), 32);
        rc_sim::lock::SessionLock::take(&level("locked", 4_000, now)).expect("lock")
    }

    /// Micro-bench: what the old path paid per frame for the row texts (rebuilt for each visible
    /// row: date with its time zone read from the system, information line, three cuts), on 15
    /// visible rows, against the kept texts. Fixed-width metrics stand for the font. 2 passes.
    #[test]
    #[ignore = "micro-bench"]
    fn bench_row_text() {
        let lang = rc_text::Lang::default();
        let worlds: Vec<Summary> = (0..15)
            .map(|k| {
                let mut s = summary(Path::new("w"));
                s.id = format!("World number {k}");
                s.name = format!("A long world name to cut, number {k}");
                s.last_played = 1_700_000_000_000 + k;
                s.commands = true;
                s
            })
            .collect();
        for pass in 0..2 {
            let frames = 2000;
            let t = std::time::Instant::now();
            for _ in 0..frames {
                for s in &worlds {
                    std::hint::black_box(RowText::new(&Fixed, &lang, "en_us", s));
                }
            }
            let rebuilt = t.elapsed() / frames;
            let mut kept: Vec<Option<RowText>> = vec![None; worlds.len()];
            let shown: Vec<usize> = (0..worlds.len()).collect();
            let t = std::time::Instant::now();
            for _ in 0..frames {
                for rank in 0..worlds.len() {
                    let (i, slot) = row_text_at(&mut kept, &shown, rank).expect("row");
                    std::hint::black_box(slot.get_or_insert_with(|| RowText::new(&Fixed, &lang, "en_us", &worlds[i])));
                }
            }
            let cached = t.elapsed() / frames;
            eprintln!("pass {pass}: 15 rows per frame, rebuilt {rebuilt:?}, kept {cached:?}");
        }
    }

    /// Micro-bench: reading of 1, 20, 100 and 200 worlds (`level.dat` of the test world
    /// `RUSTCRAFT_SOLO_WORLD`, a 64 × 64 icon, a `session.lock`), one by one then on pools of 1, 2,
    /// 4… threads up to the logical cores; and what the main thread pays to start a background
    /// reading. Warm cache only (emptying the system's file cache needs administrator rights).
    #[test]
    #[ignore = "micro-bench"]
    fn bench_world_scan() {
        let Some(world) = rc_data::env_os!("RUSTCRAFT_SOLO_WORLD").map(PathBuf::from) else {
            panic!("RUSTCRAFT_SOLO_WORLD missing: no level.dat to copy");
        };
        let level = std::fs::read(world.join("level.dat")).expect("level.dat");
        let mut icon = Vec::new();
        let mut enc = png::Encoder::new(&mut icon, 64, 64);
        enc.set_color(png::ColorType::Rgba);
        let pixels: Vec<u8> = (0..64 * 64 * 4).map(|i: u32| (i.wrapping_mul(2_654_435_761) >> 24) as u8).collect();
        enc.write_header().and_then(|mut w| w.write_image_data(&pixels)).expect("png");
        let cores = std::thread::available_parallelism().map_or(1, std::num::NonZero::get);
        for count in [1usize, 20, 100, 200] {
            let root = rc_testdata::scratch(
                std::env::temp_dir().join(format!("rc-world-bench-{count}-{}", std::process::id())),
            );
            for k in 0..count {
                let dir = root.join("saves").join(format!("World {k}"));
                std::fs::create_dir_all(&dir).expect("folder");
                std::fs::write(dir.join("level.dat"), &level).expect("level.dat");
                std::fs::write(dir.join("icon.png"), &icon).expect("icon");
                std::fs::write(dir.join("session.lock"), "\u{2603}").expect("lock");
            }
            for pass in 0..2 {
                let best = |f: &dyn Fn() -> usize| {
                    (0..5)
                        .map(|_| {
                            let t = std::time::Instant::now();
                            assert_eq!(f(), count);
                            t.elapsed()
                        })
                        .min()
                        .unwrap_or_default()
                };
                let one_by_one = best(&|| read_each(&world_dirs(&root), Summary::read, false).len());
                let mut line = format!("{count} worlds, pass {pass}: one by one {one_by_one:?}");
                let mut threads = 1;
                loop {
                    let pool = rayon::ThreadPoolBuilder::new().num_threads(threads).build().expect("pool");
                    let t = best(&|| pool.install(|| read_each(&world_dirs(&root), Summary::read, true).len()));
                    line.push_str(&format!(", {threads} threads {t:?}"));
                    if threads >= cores {
                        break;
                    }
                    threads = (threads * 2).min(cores);
                }
                let start = std::time::Instant::now();
                let dir = root.clone();
                let th = std::thread::Builder::new().spawn(move || scan(&dir)).expect("thread");
                let main_thread = start.elapsed();
                assert_eq!(th.join().expect("reading").len(), count);
                eprintln!("{line}; main thread with a background reading: {main_thread:?}");
            }
        }
    }

    /// The list read in parallel is the list read one by one, and the expected one: order, dates
    /// (a file's date for unreadable worlds), flags, icons; quick play takes its top world.
    #[test]
    fn parallel_reading_matches_one_by_one() {
        let root = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-world-scan-{}", std::process::id())));
        let _lock = sample_saves(&root);
        let dirs = world_dirs(&root);
        let mut parallel = read_each(&dirs, Summary::read, true);
        let mut one_by_one = read_each(&dirs, Summary::read, false);
        parallel.sort_by(world_order);
        one_by_one.sort_by(world_order);
        assert!(parallel == one_by_one, "parallel reading differs");
        let ids: Vec<&str> = parallel.iter().map(|s| s.id.as_str()).collect();
        let dated = |dir: &str, file: &str| modified_ms(&root.join("saves").join(dir).join(file)).expect("date");
        let (broken, old_only) = (dated("broken", "level.dat"), dated("old only", "level.dat_old"));
        let mut want: Vec<(&str, i64)> = vec![
            ("locked", 4_000),
            ("played", 3_000),
            ("newer", 2_000),
            ("older", 2_000),
            ("before", 1_000),
            ("\u{1f600}", 500),
            ("\u{fffd}", 500),
            ("broken", broken),
            ("old only", old_only),
        ];
        want.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.encode_utf16().cmp(b.0.encode_utf16())));
        assert_eq!(ids, want.iter().map(|w| w.0).collect::<Vec<_>>());
        let get = |id: &str| parallel.iter().find(|s| s.id == id).expect("world");
        assert!(get("locked").locked && !get("played").locked);
        assert!(get("broken").corrupted && get("old only").corrupted && !get("played").corrupted);
        assert!(get("newer").downgrade() && get("older").older());
        assert_eq!(get("played").icon.as_ref().map(Vec::len), Some(64 * 64 * 4));
        assert!(get("before").icon.is_none(), "32 × 32 icon refused");
        assert_eq!((get("played").name.as_str(), get("played").game_mode), ("Name of played", 1));
        assert_eq!(read_png_if_icon_sized(&root.join("saves/before/icon.png")), None);
        assert_eq!(last_played_world(&root).as_deref(), Some(ids[0]));
    }

    /// Re-creation impossible: alert with a single "Back" button; Escape goes back to the title screen.
    #[test]
    fn recreate_error_is_a_one_button_alert() {
        let Scr::Alert(mut c, then) = recreate_error(&lang()) else { panic!("alert expected") };
        assert_eq!(c.buttons().len(), 1);
        assert_eq!(c.buttons()[0].1, "gui.back");
        let esc = alert_input(&mut c, &then, &In::Key(KeyCode::Escape, ModifiersState::empty()));
        assert!(matches!(esc.as_slice(), [Msg::Title]));
        let mut s = solo();
        s.stack.push(list(vec![summary(Path::new("w"))]));
        s.stack.push(Scr::Alert(c, then));
        let out = s.apply(&lang(), esc);
        assert!(s.stack.is_empty() && matches!(out.as_slice(), [Out::Back]), "title screen");
    }

    /// Opening of a world with `resources.zip`: wait, pack requested; failure: confirmation
    /// "Continue" / "Cancel" (abandon: pack removed, list); success: continuation (disk space).
    #[test]
    fn world_pack_is_loaded_before_opening() {
        let dir = tmp("pack");
        std::fs::write(dir.join("resources.zip"), b"zip").expect("file");
        let mut s = solo();
        s.stack.push(list(vec![summary(&dir)]));
        let out = s.apply(&lang(), vec![Msg::Open(dir.clone(), Stage::Pack)]);
        assert!(matches!(out.as_slice(), [Out::WorldPack(p)] if *p == dir.join("resources.zip")));
        assert!(matches!(s.stack.last(), Some(Scr::Message(_))));
        let m = s.world_pack_msgs(&lang(), false);
        assert!(matches!(s.stack.last(), Some(Scr::List(_))), "wait closed");
        let Some(Msg::Push(Scr::Ask(_, answers))) = m.into_iter().next() else { panic!("confirmation expected") };
        assert!(matches!(&answers[0].1, ConfirmAction::Open(d, Stage::Disk) if *d == dir));
        assert!(matches!(action_msgs(answers[1].1.clone()).as_slice(), [Msg::Out(Out::DropWorldPack), Msg::ToList]));
        let _ = s.apply(&lang(), vec![Msg::Open(dir.clone(), Stage::Pack)]);
        let m = s.world_pack_msgs(&lang(), true);
        assert!(matches!(m.as_slice(), [Msg::Open(d, Stage::Disk)] if *d == dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Ordinary world: no step, direct opening; quick launch abandoned: title screen.
    #[test]
    fn plain_world_opens_and_direct_cancel_goes_to_title() {
        let dir = tmp("plain");
        let mut s = solo();
        s.stack.push(list(vec![summary(&dir)]));
        let out = s.apply(&lang(), vec![Msg::Open(dir.clone(), Stage::Experimental)]);
        assert!(out.is_empty(), "settings read in a thread");
        assert!(matches!(s.stack.last(), Some(Scr::Message(_))), "'Loading resources'");
        let out = settle(&mut s);
        assert!(matches!(out.as_slice(), [Out::Open(d)] if *d == dir));
        assert!(matches!(s.stack.last(), Some(Scr::List(_))), "wait closed");
        let mut d = solo();
        d.direct = true;
        let out = d.apply(&lang(), vec![Msg::ToList]);
        assert!(matches!(out.as_slice(), [Out::Back]));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// World with a non-vanilla dimension: backup copy offered before everything else.
    #[test]
    fn experimental_world_asks_for_a_backup() {
        let dir = tmp("exp");
        let settings = Nbt::Compound(vec![(
            "data".into(),
            Nbt::Compound(vec![(
                "dimensions".into(),
                Nbt::Compound(vec![("mod:mine".into(), Nbt::Compound(Vec::new()))]),
            )]),
        )]);
        std::fs::create_dir_all(dir.join("data").join("minecraft")).expect("folder");
        std::fs::write(
            dir.join("data/minecraft/world_gen_settings.dat"),
            rc_sim::nbtio::gzip(&rc_sim::nbtio::encode_root(&settings)),
        )
        .expect("dat");
        let mut s = solo();
        s.stack.push(list(vec![summary(&dir)]));
        let out = s.apply(&lang(), vec![Msg::Open(dir.clone(), Stage::Experimental)]);
        assert!(out.is_empty());
        assert!(settle(&mut s).is_empty());
        assert_eq!(s.stack.len(), 2, "wait replaced by the question");
        let Some(Scr::Backup(b)) = s.stack.last() else { panic!("copy offered") };
        assert_eq!(b.next, Some(Stage::Pack));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// `level.dat` as a symbolic link not allowed: warning entry (name = folder, never
    /// played, "More info" active, neither edit nor re-creation); the old code read the link.
    /// Inner link refused: the world is refused before editing.
    #[test]
    fn symlinked_level_dat_is_a_warning_entry() {
        let root = rc_testdata::scratch(std::env::temp_dir().join(format!("rc-world-links-{}", std::process::id())));
        let world = root.join("Linked world");
        let outside = root.join("elsewhere");
        std::fs::create_dir_all(&world).expect("folder");
        std::fs::create_dir_all(&outside).expect("folder");
        std::fs::write(outside.join("level.dat"), b"not nbt").expect("file");
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(outside.join("level.dat"), world.join("level.dat")).is_ok();
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(outside.join("level.dat"), world.join("level.dat")).is_ok();
        if !made || crate::servers::allowed_symlinks().allows(&outside.join("level.dat")) {
            eprintln!("symbolic links not allowed (or allowed by the player's list): test skipped");
            let _ = std::fs::remove_dir_all(&root);
            return;
        }
        let s = Summary::read(&world);
        assert!(s.symlink);
        assert!(!s.corrupted);
        assert_eq!(s.name, "Linked world");
        assert_eq!(s.last_played, -1);
        assert!(s.can_play() && !s.can_edit());
        assert!(world_refused(&world));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// World with `resources.zip` one of whose inner links is refused: warning right at "Play"
    /// ("Back" towards the title screen), with no copy offered or world pack requested; the old
    /// code loaded the pack then refused the world, pack left active.
    #[test]
    fn refused_world_never_loads_its_pack() {
        let root =
            rc_testdata::scratch(std::env::temp_dir().join(format!("rc-world-pack-link-{}", std::process::id())));
        let world = root.join("world");
        let outside = root.join("elsewhere");
        std::fs::create_dir_all(&world).expect("folder");
        std::fs::create_dir_all(&outside).expect("folder");
        std::fs::write(world.join("resources.zip"), b"zip").expect("file");
        std::fs::write(outside.join("note.txt"), b"x").expect("file");
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_file(outside.join("note.txt"), world.join("note.txt")).is_ok();
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(outside.join("note.txt"), world.join("note.txt")).is_ok();
        if !made || crate::servers::allowed_symlinks().allows(&outside.join("note.txt")) {
            eprintln!("symbolic links not allowed (or allowed by the player's list): test skipped");
            let _ = std::fs::remove_dir_all(&root);
            return;
        }
        let mut w = summary(&world);
        w.data_version = rc_sim::chunk::DATA_VERSION;
        assert!(w.can_play());
        let msgs = SelectWorld::join(&lang(), &w);
        let [Msg::Push(Scr::Ask(_, answers))] = msgs.as_slice() else { panic!("warning expected") };
        let back = answers.last().map(|(_, a)| a.clone());
        assert!(matches!(back, Some(ConfirmAction::Title)), "'Back': title screen");
        let mut s = solo();
        s.stack.push(list(vec![summary(&world)]));
        let out = s.apply(&lang(), msgs);
        assert!(out.iter().all(|o| !matches!(o, Out::WorldPack(_) | Out::Open(_))), "nothing loaded");
        assert!(s.world_pack.is_none());
        let out = s.apply(&lang(), action_msgs(back.expect("back")));
        assert!(s.stack.is_empty() && matches!(out.as_slice(), [Out::Back]));
        let _ = std::fs::remove_dir_all(&root);
    }
}
