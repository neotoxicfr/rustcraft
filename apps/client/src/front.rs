//! Out-of-world screens (`docs/specs/title-screen-servers.md`): title screen (panorama, logo,
//! splash, buttons, opening fade). Own screen stack; the options screens open
//! on top through [`crate::menus::Menus`] (action [`Action::Menu`]).

use std::time::Instant;

use rc_audio::JavaRandom;
use rc_text::{Component, StyledChar};
use winit::keyboard::{KeyCode, ModifiersState};

use crate::gui::Gui;
use crate::menus;
use crate::options::Options;
use crate::server_list::{Column, DirectJoin, EditOut, EditServer, Multiplayer, Out as MpOut};
use crate::servers::ServerEntry;
use crate::widgets::{self, Event, Kind, Panel, Widget, chars, tr};

/// Version shown at the bottom left of the title screen.
pub const VERSION_NAME: &str = "26.3";

/// Key of the mode in the window title: private singleplayer, singleplayer open to the LAN, server.
pub fn title_mode_key(solo: bool, published: bool) -> &'static str {
    match (solo, published) {
        (true, false) => "title.singleplayer",
        (true, true) => "title.multiplayer.lan",
        (false, _) => "title.multiplayer.other",
    }
}

#[test]
fn window_title_names_an_open_lan_world() {
    assert_eq!(title_mode_key(true, false), "title.singleplayer");
    assert_eq!(title_mode_key(true, true), "title.multiplayer.lan");
    assert_eq!(title_mode_key(false, false), "title.multiplayer.other");
}

/// Request for a screen to the application.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Closing the game ("Quit Game").
    Quit,
    /// Options screen to open on top (options, language, accessibility).
    Menu(Vec<menus::Id>),
    /// Connection to the entry's server.
    Connect(ServerEntry),
    /// Writing `options.txt` (`lastServer` changed).
    SaveOptions,
    /// "Cancel" during the connection.
    CancelConnect,
    /// Opening a world in singleplayer (folder, world just created).
    OpenWorld(std::path::PathBuf, bool),
    /// Resource pack of the world to load before opening (`resources.zip`), or to remove.
    WorldPack(std::path::PathBuf),
    DropWorldPack,
    /// "Singleplayer" on the title screen: world screens (version data provided by the app).
    Singleplayer,
    /// System notification (title, message).
    Toast(Box<(Component, Option<Component>)>),
    /// Reply to the server's code of conduct: accepted, "do not warn again" box, text.
    Conduct { accepted: bool, remember: bool, text: String },
    /// "Disconnect" on the reconfiguration screen.
    LeaveServer,
}

// Confirmation (deletion).
const YES: u32 = 1;
const NO: u32 = 2;

// Title screen.
const SINGLEPLAYER: u32 = 1;
const MULTIPLAYER: u32 = 2;
const REALMS: u32 = 3;
const FRIENDS: u32 = 4;
const LANGUAGE: u32 = 5;
const ACCESSIBILITY: u32 = 6;
const OPTIONS: u32 = 7;
const QUIT: u32 = 8;

/// "Splash" text (§ 1.3): lines of the file, without the line that is never shown.
pub fn splash_lines(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("This message will never appear"))
        .map(str::to_owned)
        .collect()
}

/// Day and month of the local date (system time zone, like the game).
fn day_month() -> (u32, u32) {
    let ms = std::time::SystemTime::UNIX_EPOCH.elapsed().map_or(0, |d| d.as_millis() as i64);
    let (_, m, d, ..) = rc_sim::storage::local_datetime(ms);
    (d, m)
}

/// Splash choice (§ 1.3): fixed dates, otherwise draw (42: "<USERNAME> IS YOU").
pub fn pick_splash(lines: &[String], (day, month): (u32, u32), user: &str, rng: &mut JavaRandom) -> Option<String> {
    match (day, month) {
        (24, 12) => return Some("Merry X-mas!".into()),
        (1, 1) => return Some("Happy new year!".into()),
        (31, 10) => return Some("OOoooOOOoooo! Spooky!".into()),
        _ => {}
    }
    if lines.is_empty() {
        return None;
    }
    let n = lines.len() as i32;
    if rng.next_int_bounded(n) == 42 && !user.is_empty() {
        return Some(format!("{} IS YOU", user.to_uppercase()));
    }
    lines.get(rng.next_int_bounded(n) as usize).cloned()
}

struct Title {
    panel: Panel,
    size: (i32, i32),
    splash: Option<Vec<StyledChar>>,
    /// Rare "Minceraft" logo.
    minceraft: bool,
    /// Opening fade (first title screen only): start at the first draw.
    fading: bool,
    fade_start: Option<Instant>,
    /// Accessibility onboarding opened before this screen (first launch): logo and edition stay
    /// opaque during the fade, like the game.
    keep_logo: bool,
}

impl Title {
    /// Opacities (logo and edition, rest of the screen) of the opening fade (§ 1.6): nothing before
    /// 1 s, then linear appearance until 2 s.
    fn alphas(&mut self) -> (f32, f32) {
        let rest = if self.fading {
            let start = *self.fade_start.get_or_insert_with(Instant::now);
            let k = (start.elapsed().as_secs_f32() / 2.0).clamp(0.0, 1.0);
            if k >= 1.0 {
                self.fading = false;
            }
            ((k - 0.5) * 2.0).clamp(0.0, 1.0)
        } else {
            1.0
        };
        (if self.keep_logo { 1.0 } else { rest }, rest)
    }
}

enum Screen {
    Title(Title),
    Multiplayer(Box<Multiplayer>),
    Edit(Box<EditServer>),
    Direct(DirectJoin),
    /// Deletion confirmation: screen and address of the entry.
    Delete(Box<Column>, String),
    Connecting(Connect),
    /// Disconnection: title, reason, back to the list.
    Disconnected(Box<Column>, DisconnectInfo),
    /// Confirmation to open a link (report to the server): screen and address.
    Link(Box<Column>, String),
    Loading(Loading),
    /// World screens (singleplayer): own stack (`world_list.rs`).
    Solo(Box<crate::world_list::Solo>),
    /// Warnings (`notices.rs`): online play, code of conduct.
    Notice(Box<crate::notices::Notice>),
    /// Accessibility onboarding (first launch).
    Onboarding(Box<crate::notices::Onboarding>),
    /// Reconfiguration requested by the server (in a world).
    Reconfig(Box<crate::notices::Reconfig>),
    /// World refused on opening (symbolic link not allowed): "Back" leads to the title screen.
    Symlink(Box<Column>),
}

/// "Connecting" screen (§ 4.1): state and "Cancel" button.
struct Connect {
    panel: Panel,
    size: (i32, i32),
    status: &'static str,
}

const CANCEL: u32 = 1;
const TO_MENU: u32 = 1;
const REPORT_TO_SERVER: u32 = 2;
const OPEN_REPORT_DIR: u32 = 3;
const LINK_COPY: u32 = 3;

/// What the disconnect screen offers besides going back: the server's bug report link
/// and the folder of the report written by the client.
#[derive(Clone, Debug, Default)]
pub struct DisconnectInfo {
    pub bug_link: Option<String>,
    pub report: Option<std::path::PathBuf>,
}

/// World loading screen (§ 4.3): start (at `login`), chunk loading started
/// (game event 13).
pub struct Loading {
    since: Instant,
    started: bool,
    /// World ready (after event 13): instant kept once and for all.
    ready_at: Option<Instant>,
    reason: LoadReason,
    /// First loading of a singleplayer world: preparation of the integrated server (chunk map,
    /// progress bar), and smoothed progress (0.2 per tick).
    prep: Option<std::sync::Arc<rc_sim::prep::Prep>>,
    smoothed: f32,
    /// `login` received: the screen may close (before, the integrated server is preparing).
    joined: bool,
}

impl Loading {
    fn new(reason: LoadReason) -> Loading {
        Loading {
            since: Instant::now(),
            started: false,
            ready_at: None,
            reason,
            prep: None,
            smoothed: 0.0,
            joined: true,
        }
    }
}

/// Top of the loading screen text: above the chunk map if there is one
/// (radius × 2 + 3 lines), otherwise 50 above the centre.
fn loading_text_top(height: i32, map: bool) -> i32 {
    if map { height / 2 - rc_sim::prep::RADIUS * 2 - 9 * 3 } else { height / 2 - 50 }
}

/// Colour of a chunk map cell (opaque; black without status), like the game.
fn status_color(s: Option<rc_sim::prep::Status>) -> u32 {
    use rc_sim::prep::Status;
    0xff00_0000
        | match s {
            None => 0,
            Some(Status::Empty) => 5_526_612,
            Some(Status::StructureStarts) => 10_066_329,
            Some(Status::StructureReferences) => 6_250_897,
            Some(Status::Biomes) => 8_434_258,
            Some(Status::Terrain) => 3_159_410,
            Some(Status::Features) => 2_213_376,
            Some(Status::InitializeLight) => 13_421_772,
            Some(Status::Light) => 16_769_184,
            Some(Status::Spawn) => 15_884_384,
            Some(Status::Full) => 16_777_215,
        }
}

/// Chunk map (cells of 2 px, no margin, centred): one strip per run of cells of the same
/// colour on a row.
fn draw_chunk_map(gui: &mut Gui, prep: &rc_sim::prep::Prep) {
    use rc_sim::prep::SIDE;
    const CELL: i32 = 2;
    let total = SIDE as i32 * CELL;
    let x0 = gui.width as i32 / 2 - total / 2;
    let y0 = gui.height as i32 / 2 - total / 2;
    for z in 0..SIDE {
        let y = (y0 + z as i32 * CELL) as f32;
        let mut x = 0;
        while x < SIDE {
            let c = status_color(prep.status(x, z));
            let mut end = x + 1;
            while end < SIDE && status_color(prep.status(end, z)) == c {
                end += 1;
            }
            let (a, b) = ((x0 + x as i32 * CELL) as f32, (x0 + end as i32 * CELL) as f32);
            gui.fill(a, y, b, y + CELL as f32, c);
            x = end;
        }
    }
}

/// Loading screen background: Nether portal, End portal, or panorama.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LoadReason {
    NetherPortal,
    EndPortal,
    Other,
}

impl LoadReason {
    /// Reason for a world change: other than death, the Nether then the End (left or joined).
    pub fn for_respawn(died: bool, old: &str, new: &str) -> LoadReason {
        let is = |d: &str, n: &str| d.strip_prefix("minecraft:").unwrap_or(d) == n;
        if died {
            LoadReason::Other
        } else if is(new, "the_nether") || is(old, "the_nether") {
            LoadReason::NetherPortal
        } else if is(new, "the_end") || is(old, "the_end") {
            LoadReason::EndPortal
        } else {
            LoadReason::Other
        }
    }
}

/// Maximum delay of the world loading screen.
const LOADING_MAX: std::time::Duration = std::time::Duration::from_secs(30);

/// Stack of the out-of-world screens.
pub struct Front {
    stack: Vec<Screen>,
    /// Clicks to sound.
    pub clicks: u32,
    splashes: Vec<String>,
    pub username: String,
    rng: JavaRandom,
    /// World joined (otherwise: out-of-world frames, over the panorama).
    pub in_world: bool,
    /// Connection being opened (connection thread).
    pub(crate) pending: Option<crate::flow::Pending>,
    /// Disconnect reason sent by the server, waiting for the end of the session.
    pub kicked: Option<Component>,
    /// Launched with `host port username` (test scripts): mouse left free in the world.
    pub direct: bool,
    /// Demo (`RUSTCRAFT_MENUTEST=title`): tick of the next step.
    pub test_next: u32,
    /// Accessibility onboarding finished, option to save (`notices.rs`).
    pub onboarding_done: bool,
    /// Closing of a singleplayer world in progress ("Saving world…" on top).
    save: Option<Box<crate::world_list::Solo>>,
    /// Loading screen kept after the world is ready: 500 ms for a just-created
    /// singleplayer world, nothing otherwise.
    pub close_delay: std::time::Duration,
    /// Result of loading the world's pack (`resources.zip`), handed to the singleplayer screens on the tick.
    pub world_pack_result: Option<bool>,
}

impl Front {
    pub fn new(splashes: Vec<String>, username: String) -> Front {
        Front {
            stack: Vec::new(),
            clicks: 0,
            splashes,
            username,
            rng: JavaRandom::from_entropy(),
            in_world: false,
            pending: None,
            kicked: None,
            direct: false,
            test_next: 60,
            onboarding_done: false,
            save: None,
            close_delay: std::time::Duration::ZERO,
            world_pack_result: None,
        }
    }

    /// Warning on top of the current screen (code of conduct: on the connection or the
    /// reconfiguration).
    pub fn open_notice(&mut self, kind: crate::notices::NoticeKind) {
        self.stack.push(Screen::Notice(Box::new(crate::notices::Notice::new(kind))));
    }

    /// Reconfiguration requested by the server (in a world): screen alone.
    pub fn open_reconfig(&mut self) {
        if !self.is_reconfiguring() {
            self.stack.clear();
            self.stack.push(Screen::Reconfig(Box::default()));
        }
    }

    pub fn is_reconfiguring(&self) -> bool {
        self.stack.iter().any(|s| matches!(s, Screen::Reconfig(_)))
    }

    /// Connection screen in the foreground (a single attempt at a time).
    pub fn is_connecting(&self) -> bool {
        matches!(self.stack.last(), Some(Screen::Connecting(_)))
    }

    pub fn open_connecting(&mut self) {
        self.kicked = None;
        self.stack.push(Screen::Connecting(Connect {
            panel: Panel::default(),
            size: (-1, -1),
            status: "connect.connecting",
        }));
    }

    /// State shown by the connection screen (`connect.joining`…).
    pub fn set_status(&mut self, key: &'static str) {
        if let Some(Screen::Connecting(c)) = self.stack.last_mut() {
            c.status = key;
        }
    }

    /// End of connection or session: disconnect screen above the server list
    /// (itself above the title screen). `title`: `connect.failed` or `disconnect.lost`.
    /// `solo`: integrated server session, whose parent screen is the title screen (not the
    /// server list).
    pub fn disconnected(
        &mut self,
        o: &Options,
        lang: &rc_text::Lang,
        title: &str,
        reason: Component,
        solo: bool,
        info: DisconnectInfo,
    ) {
        self.in_world = false;
        self.pending = None;
        // Multiplayer refused by the launcher or the account: back to the title screen, like the game.
        let to_title = !solo && !rc_client::session::allows_multiplayer();
        if self.is_connecting() {
            self.stack.pop();
        } else if solo || to_title {
            self.open_title(o, false);
        } else {
            self.back_to_list(o);
        }
        // Game order: report to the server, open the report folder, back.
        let mut buttons = Vec::new();
        if info.bug_link.is_some() {
            buttons.push((REPORT_TO_SERVER, tr(lang, "gui.report_to_server", &[]), 200));
        }
        if info.report.is_some() {
            buttons.push((OPEN_REPORT_DIR, tr(lang, "gui.open_report_dir", &[]), 200));
        }
        buttons.push((TO_MENU, tr(lang, if to_title { "gui.toTitle" } else { "gui.toMenu" }, &[]), 200));
        let col = Column::new(Component::text(tr(lang, title, &[])), reason, buttons, 10, 2).stacked();
        self.stack.push(Screen::Disconnected(Box::new(col), info));
    }

    /// Singleplayer quick launch to a missing world: error screen whose button leads to the
    /// world list (above the title screen), like the game.
    pub fn quick_play_error(&mut self, o: &Options, lang: &rc_text::Lang, data: std::sync::Arc<rc_data::VersionData>) {
        self.open_title(o, false);
        self.open_solo(data);
        let col = Column::new(
            Component::text(tr(lang, "quickplay.error.title", &[])),
            Component::translate("quickplay.error.invalid_identifier", Vec::new()),
            vec![(TO_MENU, tr(lang, "gui.toWorld", &[]), 200)],
            10,
            2,
        )
        .stacked();
        self.stack.push(Screen::Disconnected(Box::new(col), DisconnectInfo::default()));
    }

    /// World refused on opening (symbolic link not allowed): warning alone, whose
    /// back leads to the title screen, like the game.
    pub fn symlink_world(&mut self, lang: &rc_text::Lang) {
        self.stack.clear();
        self.stack.push(Screen::Symlink(Box::new(crate::symlink_notice::column(lang, true))));
    }

    /// Back out of the world: server list above the title screen.
    pub fn back_to_list(&mut self, o: &Options) {
        self.in_world = false;
        self.open_title(o, false);
        self.open_multiplayer();
    }

    /// Opening a singleplayer world: loading screen right away (no connection screen),
    /// with the integrated server's preparation; the out-of-world stack is left.
    pub fn open_solo_loading(&mut self, prep: std::sync::Arc<rc_sim::prep::Prep>) {
        self.kicked = None;
        self.stack.clear();
        let mut l = Loading::new(LoadReason::Other);
        l.prep = Some(prep);
        l.joined = false;
        self.stack.push(Screen::Loading(l));
    }

    /// Entering the world (`login`): loading screen alone; the out-of-world stack is left.
    /// The singleplayer one, already open, is kept (map and bar), its delay starts from here.
    pub fn enter_world(&mut self) {
        self.in_world = true;
        self.pending = None;
        let solo = match self.stack.pop() {
            Some(Screen::Loading(l)) if !l.joined => Some(l),
            _ => None,
        };
        self.stack.clear();
        let mut l = solo.unwrap_or_else(|| Loading::new(LoadReason::Other));
        l.joined = true;
        l.since = Instant::now();
        self.stack.push(Screen::Loading(l));
    }

    /// Loading screen tick: smoothed progress of the integrated server (0.2 per tick).
    pub fn loading_tick(&mut self) {
        if let Some(Screen::Loading(l)) = self.stack.last_mut()
            && let Some(p) = &l.prep
        {
            l.smoothed += (p.progress() - l.smoothed) * 0.2;
        }
    }

    /// Respawn packet: loading screen (Nether or End portal background depending on
    /// the dimensions left and joined, except death), or a simple background change if it is already there.
    pub fn respawn_loading(&mut self, reason: LoadReason) {
        if rc_data::env_flag!("RUSTCRAFT_LOADLOG") {
            eprintln!("loading: respawn, background {reason:?}");
        }
        // Respawn: text only (the preparation tracking is only for the first loading).
        if let Some(Screen::Loading(l)) = self.stack.last_mut() {
            *l = Loading::new(reason);
            return;
        }
        self.stack.clear();
        self.stack.push(Screen::Loading(Loading::new(reason)));
    }

    /// Game event 13: chunk loading started.
    pub fn chunks_started(&mut self) {
        if let Some(Screen::Loading(l)) = self.stack.last_mut() {
            l.started = true;
        }
    }

    /// End of the loading screen (§ 4.3): after event 13, player's world ready
    /// (`ready`: camera section meshed and visible, camera outside the world height,
    /// spectator or dead), instant kept, then `close_delay` more; or 30 s after `login`.
    /// Returns true if the world is loaded (screen closed, or absent and `ready`).
    pub fn loading_done(&mut self, ready: bool) -> bool {
        let delay = self.close_delay;
        match self.stack.last_mut() {
            Some(Screen::Loading(l)) => {
                // `RUSTCRAFT_LOADHOLD=<ms>`: loading screen kept at least this long (captures).
                static HOLD: std::sync::OnceLock<std::time::Duration> = std::sync::OnceLock::new();
                let hold = *HOLD.get_or_init(|| {
                    let ms = rc_data::env_str!("RUSTCRAFT_LOADHOLD").and_then(|v| v.parse().ok()).unwrap_or(0);
                    std::time::Duration::from_millis(ms)
                });
                if !l.joined {
                    return false;
                }
                if l.started && ready && l.ready_at.is_none() {
                    l.ready_at = Some(Instant::now());
                }
                let shown = l.ready_at.is_some_and(|t| t.elapsed() >= delay);
                let done = (shown && l.since.elapsed() >= hold) || l.since.elapsed() >= LOADING_MAX;
                if done && rc_data::env_flag!("RUSTCRAFT_LOADLOG") {
                    eprintln!("loading: finished after {:?} (13 received: {})", l.since.elapsed(), l.started);
                }
                if done {
                    self.stack.pop();
                    // Delay of a new world: first loading only (respawns without delay).
                    self.close_delay = std::time::Duration::ZERO;
                }
                done
            }
            _ => ready,
        }
    }

    pub fn is_open(&self) -> bool {
        !self.stack.is_empty() || self.save.is_some()
    }

    /// Title screen alone (at startup or on return from a server); `fading`: opening fade.
    pub fn open_title(&mut self, o: &Options, fading: bool) {
        let splash = if o.b("hideSplashTexts") {
            None
        } else {
            pick_splash(&self.splashes, day_month(), &self.username, &mut self.rng)
        };
        let minceraft = self.rng.next_float() < 1.0e-4;
        // First launch: accessibility onboarding before the title screen.
        let onboarding = fading && o.b("onboardAccessibility");
        self.stack.clear();
        self.stack.push(Screen::Title(Title {
            panel: Panel::default(),
            size: (-1, -1),
            splash: splash.map(|s| widgets::chars(&s)),
            minceraft,
            fading,
            fade_start: None,
            keep_logo: onboarding,
        }));
        if onboarding {
            self.stack.push(Screen::Onboarding(Box::default()));
        }
    }

    /// Multiplayer screen on top of the title screen (list from `servers.dat`).
    pub fn open_multiplayer(&mut self) {
        if !matches!(self.stack.last(), Some(Screen::Multiplayer(_))) {
            self.stack.push(Screen::Multiplayer(Box::new(Multiplayer::new(crate::servers::game_dir()))));
        }
    }

    fn layout(&mut self, gui: &Gui) {
        let size = (gui.width as i32, gui.height as i32);
        let Some(top) = self.stack.last_mut() else { return };
        match top {
            Screen::Title(t) if t.size != size => {
                t.size = size;
                let keyboard = t.panel.keyboard;
                t.panel = Panel::new(title_widgets(gui, size));
                t.panel.keyboard = keyboard;
            }
            Screen::Title(_) => {}
            Screen::Multiplayer(m) => m.layout(gui),
            Screen::Edit(e) => e.layout(gui),
            Screen::Direct(d) => d.layout(gui),
            Screen::Delete(c, _) | Screen::Disconnected(c, _) | Screen::Link(c, _) | Screen::Symlink(c) => {
                c.layout(gui)
            }
            Screen::Connecting(c) if c.size != size => {
                c.size = size;
                let cancel = tr(&gui.lang, "gui.cancel", &[]);
                c.panel = Panel::new(vec![Widget::button(CANCEL, size.0 / 2 - 100, size.1 / 4 + 132, 200, &cancel)]);
            }
            Screen::Connecting(_) | Screen::Loading(_) => {}
            Screen::Solo(_) => {}
            Screen::Notice(n) => n.layout(gui),
            Screen::Onboarding(n) => n.layout(gui),
            Screen::Reconfig(n) => n.layout(gui),
        }
    }

    fn panel(&mut self) -> Option<&mut Panel> {
        Some(match self.stack.last_mut()? {
            Screen::Title(t) => &mut t.panel,
            Screen::Multiplayer(m) => &mut m.panel,
            Screen::Edit(e) => &mut e.panel,
            Screen::Direct(d) => &mut d.panel,
            Screen::Delete(c, _) | Screen::Disconnected(c, _) | Screen::Link(c, _) | Screen::Symlink(c) => &mut c.panel,
            Screen::Connecting(c) => &mut c.panel,
            Screen::Loading(_) => return None,
            Screen::Solo(s) => return s.panel_mut(),
            Screen::Notice(n) => &mut n.panel,
            Screen::Onboarding(n) => &mut n.panel,
            Screen::Reconfig(n) => &mut n.panel,
        })
    }

    fn multiplayer(&mut self) -> Option<&mut Multiplayer> {
        self.stack.iter_mut().rev().find_map(|s| match s {
            Screen::Multiplayer(m) => Some(&mut **m),
            _ => None,
        })
    }

    /// Events of the components of the top screen; `escape`: Escape not used.
    fn handle(&mut self, gui: &Gui, o: &mut Options, events: Vec<Event>, escape: bool) -> Vec<Action> {
        let mut out = Vec::new();
        let Some(top) = self.stack.last_mut() else { return out };
        match top {
            Screen::Title(_) => {
                for e in events {
                    match e {
                        // First time: online play warning (`skipMultiplayerWarning`).
                        Event::Press(MULTIPLAYER) if !o.b("skipMultiplayerWarning") => {
                            self.open_notice(crate::notices::NoticeKind::Multiplayer);
                        }
                        Event::Press(MULTIPLAYER) => self.open_multiplayer(),
                        Event::Press(SINGLEPLAYER) => out.push(Action::Singleplayer),
                        Event::Press(OPTIONS) => out.push(Action::Menu(vec![menus::Id::Options])),
                        Event::Press(LANGUAGE) => out.push(Action::Menu(vec![menus::Id::Options, menus::Id::Language])),
                        Event::Press(ACCESSIBILITY) => {
                            out.push(Action::Menu(vec![menus::Id::Options, menus::Id::Accessibility]))
                        }
                        Event::Press(QUIT) => out.push(Action::Quit),
                        _ => {}
                    }
                }
            }
            Screen::Multiplayer(_) => unreachable!("list: `mp_outs`"),
            Screen::Edit(e) => {
                let r = if escape { Some(EditOut::Cancel) } else { e.handle(gui, events) };
                if let Some(r) = r {
                    let Some(Screen::Edit(e)) = self.stack.pop() else { return out };
                    if let (EditOut::Done(new), Some(m)) = (r, self.multiplayer()) {
                        match &e.original {
                            Some(old) => m.replace(old, new),
                            None => m.add(new),
                        }
                    }
                }
            }
            Screen::Direct(d) => {
                let r = if escape { Some(false) } else { d.handle(events) };
                if let Some(join) = r {
                    // On closing, the input becomes `lastServer` (options saved).
                    let ip = d.value();
                    o.set("lastServer", crate::options::Val::S(ip.clone()));
                    out.push(Action::SaveOptions);
                    self.stack.pop();
                    if join && let Some(m) = self.multiplayer() {
                        out.push(Action::Connect(m.direct_entry(&ip, &gui.lang)));
                    }
                }
            }
            // Escape inactive on these screens.
            Screen::Connecting(_) => {
                if events.contains(&Event::Press(CANCEL)) {
                    self.stack.pop();
                    out.push(Action::CancelConnect);
                }
            }
            Screen::Disconnected(_, info) => {
                if events.contains(&Event::Press(TO_MENU)) {
                    self.stack.pop();
                } else if events.contains(&Event::Press(REPORT_TO_SERVER)) {
                    if let Some(url) = info.bug_link.clone() {
                        let t = |k: &str| tr(&gui.lang, k, &[]);
                        let col = Column::new(
                            Component::text(t("chat.link.confirm")),
                            Component::text(url.clone()),
                            vec![(YES, t("gui.yes"), 100), (LINK_COPY, t("chat.copy"), 100), (NO, t("gui.no"), 100)],
                            8,
                            16,
                        )
                        .with_warning(Component::translate("chat.link.warning", Vec::new()));
                        self.stack.push(Screen::Link(Box::new(col), url));
                    }
                } else if events.contains(&Event::Press(OPEN_REPORT_DIR))
                    && let Some(dir) = info.report.as_deref().and_then(std::path::Path::parent)
                {
                    crate::links::open_dir(dir);
                }
            }
            Screen::Link(_, url) => {
                let url = url.clone();
                let yes = events.contains(&Event::Press(YES));
                let copy = events.contains(&Event::Press(LINK_COPY));
                if yes || copy || escape || events.contains(&Event::Press(NO)) {
                    self.stack.pop();
                    if yes {
                        crate::links::open_url(&url);
                    } else if copy {
                        crate::links::set_clipboard(&url);
                    }
                }
            }
            Screen::Loading(_) => {}
            Screen::Solo(_) => {}
            Screen::Symlink(_) => {
                if crate::symlink_notice::handle(&events, escape) {
                    self.open_title(o, false);
                }
            }
            Screen::Notice(n) => {
                let answer = if escape { n.escape() } else { n.handle(&events) };
                if let Some((accepted, checked)) = answer {
                    let kind = n.kind.clone();
                    self.stack.pop();
                    match kind {
                        crate::notices::NoticeKind::Multiplayer => {
                            if accepted {
                                if checked {
                                    o.set("skipMultiplayerWarning", crate::options::Val::B(true));
                                    out.push(Action::SaveOptions);
                                }
                                self.open_multiplayer();
                            }
                        }
                        crate::notices::NoticeKind::Conduct(text) => {
                            out.push(Action::Conduct { accepted, remember: checked, text });
                        }
                    }
                }
            }
            Screen::Onboarding(n) => {
                if let Some(crate::notices::OnboardOut::Menu(ids)) = n.handle(&events, escape) {
                    out.push(Action::Menu(ids));
                }
            }
            Screen::Reconfig(n) => {
                if n.wants_disconnect(&events) {
                    out.push(Action::LeaveServer);
                }
            }
            Screen::Delete(_, ip) => {
                let ip = ip.clone();
                let yes = events.contains(&Event::Press(YES));
                if yes || escape || events.contains(&Event::Press(NO)) {
                    self.stack.pop();
                    if yes && let Some(m) = self.multiplayer() {
                        m.remove(&ip);
                    }
                }
            }
        }
        self.take_clicks();
        out
    }

    fn take_clicks(&mut self) {
        if let Some(p) = self.panel() {
            let c = std::mem::take(&mut p.clicks);
            self.clicks += c;
        }
    }

    /// Requests of the server list.
    fn mp_outs(&mut self, gui: &Gui, o: &Options, outs: Vec<MpOut>) -> Vec<Action> {
        self.take_clicks();
        let mut out = Vec::new();
        for r in outs {
            match r {
                MpOut::Join(e) => out.push(Action::Connect(e)),
                MpOut::Add => {
                    let name = tr(&gui.lang, "selectServer.defaultName", &[]);
                    self.stack.push(Screen::Edit(Box::new(EditServer::new(None, name))));
                }
                MpOut::Edit(e) => self.stack.push(Screen::Edit(Box::new(EditServer::new(Some(e), String::new())))),
                MpOut::Direct => self.open_direct(o),
                MpOut::Delete(ip) => {
                    let name = self.multiplayer().map(|m| m.name_of(&ip)).unwrap_or_default();
                    let t = |k: &str| tr(&gui.lang, k, &[]);
                    let col = Column::new(
                        Component::text(t("selectServer.deleteQuestion")),
                        Component::text(tr(&gui.lang, "selectServer.deleteWarning", &[name])),
                        vec![(YES, t("selectServer.deleteButton"), 150), (NO, t("gui.cancel"), 150)],
                        8,
                        16,
                    );
                    self.stack.push(Screen::Delete(Box::new(col), ip));
                }
                MpOut::Back => {
                    self.stack.pop();
                }
            }
        }
        out
    }

    /// World screens ("Singleplayer") above the title screen.
    pub fn open_solo(&mut self, data: std::sync::Arc<rc_data::VersionData>) {
        self.stack.push(Screen::Solo(Box::new(crate::world_list::Solo::new(data))));
    }

    /// Closing of the singleplayer world: "Saving world…" on top of everything until the
    /// writing is done and the lock released.
    pub fn begin_save(&mut self, data: std::sync::Arc<rc_data::VersionData>, h: rc_sim::Handle) {
        self.save = Some(Box::new(crate::world_list::Solo::saving(data, h)));
    }

    /// Closing the game: save in progress awaited.
    pub fn finish_save(&mut self) {
        if let Some(mut s) = self.save.take() {
            s.finish_save();
        }
    }

    /// Out-of-world tick: background jobs of the singleplayer screens (creation, copy, closing).
    pub fn tick(&mut self, gui: &Gui) -> Vec<Action> {
        if let Some(s) = &mut self.save {
            let outs = s.tick(&gui.lang);
            if outs.iter().any(|o| matches!(o, crate::world_list::Out::Back)) {
                self.save = None;
            }
            return Vec::new();
        }
        let outs = match self.stack.last_mut() {
            Some(Screen::Solo(s)) => {
                let mut outs = match self.world_pack_result.take() {
                    Some(ok) => s.world_pack_done(gui, ok),
                    None => Vec::new(),
                };
                outs.extend(s.tick(&gui.lang));
                outs
            }
            // Singleplayer preparing (before `login`): out-of-world ticks.
            Some(Screen::Loading(_)) => {
                self.loading_tick();
                return Vec::new();
            }
            _ => return Vec::new(),
        };
        self.solo_outs(outs)
    }

    /// Requests of the singleplayer stack.
    fn solo_outs(&mut self, outs: Vec<crate::world_list::Out>) -> Vec<Action> {
        let mut out = Vec::new();
        for o in outs {
            match o {
                crate::world_list::Out::Back => {
                    if matches!(self.stack.last(), Some(Screen::Solo(_))) {
                        self.stack.pop();
                    }
                }
                crate::world_list::Out::Open(_) | crate::world_list::Out::OpenNew(_) => {
                    let (dir, new) = match o {
                        crate::world_list::Out::OpenNew(d) => (d, true),
                        crate::world_list::Out::Open(d) => (d, false),
                        _ => continue,
                    };
                    if matches!(self.stack.last(), Some(Screen::Solo(_))) {
                        self.stack.pop();
                    }
                    out.push(Action::OpenWorld(dir, new));
                }
                crate::world_list::Out::Toast(b) => out.push(Action::Toast(b)),
                crate::world_list::Out::WorldPack(p) => out.push(Action::WorldPack(p)),
                crate::world_list::Out::DropWorldPack => out.push(Action::DropWorldPack),
            }
        }
        out
    }

    /// Direct opening of a world (quick launch): same steps as from the list
    /// (copy of the experimental settings, world pack, disk space), a cancel goes back to
    /// the title screen like the game.
    pub fn open_solo_world(&mut self, gui: &Gui, data: std::sync::Arc<rc_data::VersionData>, dir: std::path::PathBuf) -> Vec<Action> {
        let (s, outs) = crate::world_list::Solo::opening(gui, data, dir);
        self.stack.push(Screen::Solo(Box::new(s)));
        self.solo_outs(outs)
    }

    /// "Direct connection" screen, prefilled with `lastServer`.
    pub fn open_direct(&mut self, o: &Options) {
        self.stack.push(Screen::Direct(DirectJoin::new(o.s("lastServer"))));
    }

    fn input(&mut self, gui: &Gui, o: &mut Options, i: Input<'_>) -> Vec<Action> {
        // "Saving world…": no input.
        if self.save.is_some() {
            return Vec::new();
        }
        self.layout(gui);
        if let Some(Screen::Solo(s)) = self.stack.last_mut() {
            use crate::world_list::In;
            let si = match i {
                Input::Motion(m) => In::Motion(m),
                Input::Press(m, b, sh) => In::Press(m, b, sh),
                Input::Release => In::Release,
                Input::Wheel(m, dy) => In::Wheel(m, dy),
                Input::Key(k, mods) => In::Key(k, mods),
                Input::Typed(t) => In::Typed(t),
            };
            let outs = s.input(gui, &si);
            self.take_clicks();
            return self.solo_outs(outs);
        }
        if let Some(Screen::Multiplayer(m)) = self.stack.last_mut() {
            let outs = match i {
                Input::Motion(p) => m.motion(p),
                Input::Press(p, b, s) => m.press(p, b, s),
                Input::Release => m.release(),
                Input::Wheel(p, dy) => m.wheel(p, dy),
                Input::Key(k, mods) => m.key(k, mods),
                Input::Typed(_) => Vec::new(),
            };
            return self.mp_outs(gui, o, outs);
        }
        // Direct connection: Enter in the field joins.
        if let (Some(Screen::Direct(d)), Input::Key(k, _)) = (self.stack.last(), &i)
            && d.key_confirms(*k)
        {
            return self.handle(gui, o, vec![Event::Press(crate::server_list::DIRECT_SELECT)], false);
        }
        // Copyright notice (bottom right): "Credits & Attribution".
        if let (Some(Screen::Title(_)), Input::Press(m, 0, _)) = (self.stack.last(), &i) {
            let credits = chars(&tr(&gui.lang, "title.credits", &[]));
            let cw = rc_text::width(gui, &credits);
            let (cx, cy) = (gui.width as i32 - cw - 2, gui.height as i32 - 10);
            if m.0 >= cx as f32 && m.0 < (cx + cw) as f32 && m.1 >= cy as f32 && m.1 < (cy + 10) as f32 {
                self.clicks += 1;
                return vec![Action::Menu(vec![menus::Id::Credits])];
            }
        }
        let mut ev = Vec::new();
        let Some(p) = self.panel() else { return Vec::new() };
        let mut escape = false;
        match i {
            Input::Motion(m) => p.motion(m, &mut ev),
            Input::Press(m, b, s) => p.press(m, b, s, &mut ev),
            Input::Release => p.release(&mut ev),
            Input::Wheel(m, dy) => p.wheel(m, dy, &mut ev),
            Input::Key(k, mods) => {
                let used = p.key(k, mods, &mut ev);
                escape = !used && k == KeyCode::Escape;
            }
            Input::Typed(s) => p.typed(s, &mut ev),
        }
        // Escape closes neither the title screen nor the connection ones.
        let escape = escape
            && !matches!(
                self.stack.last(),
                Some(Screen::Title(_) | Screen::Connecting(_) | Screen::Disconnected(..) | Screen::Loading(_))
            );
        self.handle(gui, o, ev, escape)
    }

    pub fn motion(&mut self, gui: &Gui, o: &mut Options, m: (f32, f32)) -> Vec<Action> {
        self.input(gui, o, Input::Motion(m))
    }

    pub fn press(&mut self, gui: &Gui, o: &mut Options, m: (f32, f32), button: u8, shift: bool) -> Vec<Action> {
        self.input(gui, o, Input::Press(m, button, shift))
    }

    pub fn release(&mut self, gui: &Gui, o: &mut Options) -> Vec<Action> {
        self.input(gui, o, Input::Release)
    }

    pub fn wheel(&mut self, gui: &Gui, o: &mut Options, m: (f32, f32), dy: f64) -> Vec<Action> {
        self.input(gui, o, Input::Wheel(m, dy))
    }

    pub fn key(&mut self, gui: &Gui, o: &mut Options, k: KeyCode, mods: ModifiersState) -> Vec<Action> {
        self.input(gui, o, Input::Key(k, mods))
    }

    pub fn typed(&mut self, gui: &Gui, o: &mut Options, s: &str) -> Vec<Action> {
        self.input(gui, o, Input::Typed(s))
    }

    /// Drawing of the top screen (the panorama is already drawn by the renderer).
    pub fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, o: &Options, mouse: Option<(f32, f32)>, time: f64) {
        if let Some(s) = &mut self.save {
            s.draw(gui, q, o, mouse, time);
            return;
        }
        self.layout(gui);
        let Some(top) = self.stack.last_mut() else { return };
        match top {
            Screen::Title(t) => draw_title(t, gui, q, mouse, time),
            Screen::Multiplayer(m) => m.draw(gui, q, o, mouse, time),
            Screen::Edit(e) => e.draw(gui, q, o, mouse, time),
            Screen::Direct(d) => d.draw(gui, q, o, mouse, time),
            Screen::Delete(c, _) | Screen::Disconnected(c, _) | Screen::Link(c, _) | Screen::Symlink(c) => {
                c.draw(gui, q, o, mouse, time)
            }
            Screen::Connecting(c) => {
                gui.next_layer();
                widgets::menu_background(gui, o.i("menuBackgroundBlurriness"), false);
                gui.next_layer();
                let status = chars(&tr(&gui.lang, c.status, &[]));
                widgets::centered(gui, q, &status, gui.width as i32 / 2, gui.height as i32 / 2 - 50, 0xffff_ffff);
                c.panel.draw(gui, q, mouse, time);
            }
            Screen::Solo(s) => s.draw(gui, q, o, mouse, time),
            Screen::Notice(n) => n.draw(gui, q, o, mouse, time),
            Screen::Reconfig(n) => n.draw(gui, q, o, mouse, time),
            Screen::Onboarding(n) => {
                n.draw(gui, q, o, mouse, time);
                if n.finished() {
                    self.stack.pop();
                    self.onboarding_done = true;
                }
            }
            Screen::Loading(l) => {
                gui.next_layer();
                let (w, h) = (gui.width, gui.height);
                match l.reason {
                    LoadReason::NetherPortal => {
                        // Portal texture stretched over the whole screen, animated (one image per tick).
                        let frame = ((time * 20.0) as usize) % 32;
                        let name = if frame == 0 {
                            "tex/block/nether_portal".to_owned()
                        } else {
                            format!("tex/block/nether_portal#{frame}")
                        };
                        // Written without blending: solid colours despite the texture's alpha.
                        gui.stratum(crate::gui::Blend::Opaque);
                        gui.sprite(&name, 0.0, 0.0, w, h, 0xffff_ffff);
                        gui.stratum(crate::gui::Blend::Normal);
                    }
                    LoadReason::EndPortal => end_portal_background(gui),
                    LoadReason::Other => widgets::menu_background(gui, o.i("menuBackgroundBlurriness"), false),
                }
                gui.next_layer();
                let top = loading_text_top(gui.height as i32, l.prep.is_some());
                if let Some(p) = &l.prep {
                    draw_chunk_map(gui, p);
                }
                let text = chars(&tr(&gui.lang, "multiplayer.downloadingTerrain", &[]));
                widgets::centered(gui, q, &text, gui.width as i32 / 2, top, 0xffff_ffff);
                // Integrated server progress bar (200 × 2, black background, green part).
                if l.prep.as_ref().is_some_and(|p| p.has_progress()) {
                    let (x, y) = ((gui.width as i32 / 2 - 100) as f32, (top + 9 + 3) as f32);
                    gui.fill(x, y, x + 200.0, y + 2.0, 0xff00_0000);
                    let part = (l.smoothed * 200.0 + 0.5).floor();
                    gui.fill(x, y, x + part, y + 2.0, 0xff00_ff00);
                }
            }
        }
        self.take_clicks();
    }
}

/// Input forwarded to the top screen.
enum Input<'a> {
    Motion((f32, f32)),
    Press((f32, f32), u8, bool),
    Release,
    Wheel((f32, f32), f64),
    Key(KeyCode, ModifiersState),
    Typed(&'a str),
}

/// Loading screen background to or from the End: same computation as the game's portal (End
/// sky and fifteen layers of the portal texture), frozen because game time is zero
/// during the loading.
fn end_portal_background(gui: &mut Gui) {
    let (w, h) = (gui.width, gui.height);
    gui.end_portal(0.0, 0.0, w, h);
}

fn title_widgets(gui: &Gui, (w, h): (i32, i32)) -> Vec<Widget> {
    let t = |k: &str| tr(&gui.lang, k, &[]);
    let y0 = h / 4 + 48;
    let x = w / 2 - 100;
    // Rustcraft: singleplayer through the integrated server, no Realms; friends unavailable offline.
    let mut multiplayer = Widget::button(MULTIPLAYER, x, y0 + 24, 200, &t("menu.multiplayer"));
    let mut realms = Widget::button(REALMS, x, y0 + 48, 200, &t("menu.online")).inactive();
    // Multiplayer refused by the launcher or the account (ban, username to change): buttons
    // greyed out with the reason in a tooltip, like the game (`rc_client::session`).
    if let Some(k) = rc_client::session::multiplayer_disabled_reason() {
        multiplayer = multiplayer.with_tooltip(gui, &t(k)).inactive();
        realms = realms.with_tooltip(gui, &t(k));
    }
    let mut v = vec![Widget::button(SINGLEPLAYER, x, y0, 200, &t("menu.singleplayer")), multiplayer, realms];
    let icon = |i: i32| w / 2 - (3 * 20 + 2 * 4) / 2 + (i - 1) * 24;
    let mut friends =
        Widget::new(FRIENDS, Kind::Icon("friends/friends"), icon(1), y0 + 72, 20, 20, chars(&t("gui.friends.open")));
    friends.active = false;
    v.push(friends);
    // Tooltips specific to the common language and accessibility buttons (friends
    // unavailable: none).
    v.push(
        Widget::new(LANGUAGE, Kind::Icon("icon/language"), icon(2), y0 + 72, 20, 20, chars(&t("options.language")))
            .with_tooltip(gui, &t("options.language.tooltip")),
    );
    v.push(
        Widget::new(
            ACCESSIBILITY,
            Kind::Icon("icon/accessibility"),
            icon(3),
            y0 + 72,
            20,
            20,
            chars(&t("options.accessibility")),
        )
        .with_tooltip(gui, &t("options.accessibility.tooltip")),
    );
    v.push(Widget::button(OPTIONS, x, y0 + 96, 98, &t("menu.options")));
    v.push(Widget::button(QUIT, w / 2 + 2, y0 + 96, 98, &t("menu.quit")));
    v
}

fn draw_title(t: &mut Title, gui: &mut Gui, q: &wgpu::Queue, mouse: Option<(f32, f32)>, time: f64) {
    let (logo_alpha, alpha) = t.alphas();
    let la = ((logo_alpha * 255.0) as u32) << 24;
    let a = ((alpha * 255.0) as u32) << 24;
    if la == 0 && a == 0 {
        return;
    }
    let (w, h) = (gui.width as i32, gui.height as i32);
    gui.next_layer();
    // Logo (§ 1.2): 256 × 44 on an image treated as 256 × 64; edition 128 × 14 on 128 × 16.
    let logo = if t.minceraft { "tex/gui/title/minceraft" } else { "tex/gui/title/minecraft" };
    let lx = (w / 2 - 128) as f32;
    gui.sprite_region_color(logo, (256.0, 64.0), (0.0, 0.0), [lx, 30.0, 256.0, 44.0], la | 0xff_ffff);
    // Edition: "RUST EDITION", image generated by Rustcraft (`rc_assets::gui`, same 128 × 16
    // template as the game's subtitle) in place of "Java Edition" (user's choice).
    gui.sprite_region_color(
        "rustcraft/edition",
        (128.0, 16.0),
        (0.0, 0.0),
        [(w / 2 - 64) as f32, 66.0, 128.0, 16.0],
        la | 0xff_ffff,
    );
    if a == 0 {
        return;
    }
    // Splash (§ 1.3): yellow, rotated by −20°, pulsing over 1 s.
    if let Some(s) = &t.splash {
        let tw = rc_text::width(gui, s) as f32;
        let ms = (time * 1000.0) as u64 % 1000;
        let phase = 1.8 - ((ms as f32 / 1000.0 * std::f32::consts::TAU).sin()).abs() * 0.1;
        let k = phase * 100.0 / (tw + 32.0);
        let mark = gui.mark();
        gui.text(q, s, -tw / 2.0, -8.0, a | 0xff_ff00, true, 0);
        let (sin, cos) = (-20f32).to_radians().sin_cos();
        gui.transform_since(mark, [cos * k, sin * k, -sin * k, cos * k, (w / 2 + 123) as f32, 69.0]);
    }
    // Version (§ 1.5) at the bottom left, copyright at the bottom right.
    let version = chars(&format!("Minecraft {VERSION_NAME}"));
    gui.text(q, &version, 2.0, (h - 10) as f32, a | 0xff_ffff, true, 0);
    let credits = chars(&tr(&gui.lang, "title.credits", &[]));
    let cw = rc_text::width(gui, &credits);
    let (cx, cy) = (w - cw - 2, h - 10);
    let hover =
        mouse.is_some_and(|m| m.0 >= cx as f32 && m.0 < (cx + cw) as f32 && m.1 >= cy as f32 && m.1 < (cy + 10) as f32);
    let credits = if hover {
        credits
            .into_iter()
            .map(|mut c| {
                c.style.underlined = true;
                c
            })
            .collect()
    } else {
        credits
    };
    gui.text(q, &credits, cx as f32, cy as f32, a | 0xff_ffff, true, 0);
    for wd in &mut t.panel.widgets {
        wd.alpha = alpha;
    }
    t.panel.draw(gui, q, mouse, time);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Just-created world: the loading screen stays 500 ms after the world is ready
    /// (the old code closed it at once), and stays even if the condition drops.
    #[test]
    fn new_world_keeps_loading_screen_half_a_second() {
        let mut f = Front::new(Vec::new(), "x".into());
        f.close_delay = std::time::Duration::from_millis(500);
        f.enter_world();
        f.chunks_started();
        assert!(!f.loading_done(true));
        assert!(f.is_open());
        if let Some(Screen::Loading(l)) = f.stack.last_mut() {
            l.ready_at = Instant::now().checked_sub(std::time::Duration::from_millis(500));
        }
        assert!(f.loading_done(false));
        // Respawn (portal) afterwards: new loading without delay.
        f.respawn_loading(LoadReason::Other);
        f.chunks_started();
        assert!(f.loading_done(true));
        // Without delay: closed as soon as ready.
        f.close_delay = std::time::Duration::ZERO;
        f.enter_world();
        f.chunks_started();
        assert!(f.loading_done(true));
    }

    /// Singleplayer: loading screen opened right away (the old code went through the
    /// connection screen), with map and bar; it does not close before `login`, which it crosses
    /// keeping the preparation; a respawn goes back to text only.
    #[test]
    fn solo_opens_the_loading_screen_with_its_map() {
        let mut f = Front::new(Vec::new(), "x".into());
        f.open_solo_loading(std::sync::Arc::default());
        assert!(!f.is_connecting());
        assert!(matches!(f.stack.last(), Some(Screen::Loading(l)) if l.prep.is_some() && !l.joined));
        f.chunks_started();
        assert!(!f.loading_done(true), "closed before login");
        f.enter_world();
        assert_eq!(f.stack.len(), 1);
        assert!(matches!(f.stack.last(), Some(Screen::Loading(l)) if l.prep.is_some() && l.joined));
        f.chunks_started();
        assert!(f.loading_done(true));
        f.respawn_loading(LoadReason::Other);
        assert!(matches!(f.stack.last(), Some(Screen::Loading(l)) if l.prep.is_none()));
        // Multiplayer: text only.
        f.enter_world();
        assert!(matches!(f.stack.last(), Some(Screen::Loading(l)) if l.prep.is_none()));
    }

    /// Game layout: text above the map (centre - 17 × 2 - 27), otherwise centre - 50;
    /// opaque colours, black without status.
    #[test]
    fn loading_layout_and_colors() {
        assert_eq!(loading_text_top(480, true), 179);
        assert_eq!(loading_text_top(480, false), 190);
        use rc_sim::prep::Status;
        assert_eq!(status_color(None), 0xff00_0000);
        assert_eq!(status_color(Some(Status::Full)), 0xffff_ffff);
        assert_eq!(status_color(Some(Status::Empty)), 0xff54_5454);
        assert_eq!(status_color(Some(Status::Light)), 0xffff_e0a0);
    }

    /// Accessibility onboarding of the first launch: logo opaque during the fade, the rest at 0;
    /// without onboarding, logo faded like the rest (the old code always faded the logo).
    #[test]
    fn logo_kept_through_fade_after_onboarding() {
        let mut o = Options::default();
        let mut f = Front::new(Vec::new(), "x".into());
        o.set("onboardAccessibility", crate::options::Val::B(true));
        f.open_title(&o, true);
        let Some(Screen::Title(t)) = f.stack.first_mut() else { panic!("title screen") };
        assert_eq!(t.alphas(), (1.0, 0.0));
        o.set("onboardAccessibility", crate::options::Val::B(false));
        f.open_title(&o, true);
        let Some(Screen::Title(t)) = f.stack.first_mut() else { panic!("title screen") };
        assert_eq!(t.alphas(), (0.0, 0.0));
        t.fade_start = Instant::now().checked_sub(std::time::Duration::from_secs(3));
        assert_eq!(t.alphas(), (1.0, 1.0));
    }

    #[test]
    fn splash_dates_and_filter() {
        let lines =
            splash_lines("  A  \n\nThis message will never appear on the splash screen, isn't that weird?\nB\n");
        assert_eq!(lines, ["A", "B"]);
        let mut rng = JavaRandom::new(1);
        assert_eq!(pick_splash(&lines, (24, 12), "x", &mut rng).as_deref(), Some("Merry X-mas!"));
        assert_eq!(pick_splash(&lines, (31, 10), "x", &mut rng).as_deref(), Some("OOoooOOOoooo! Spooky!"));
        assert!(pick_splash(&[], (2, 2), "x", &mut rng).is_none());
        let s = pick_splash(&lines, (2, 2), "x", &mut rng).unwrap();
        assert!(s == "A" || s == "B");
    }

    #[test]
    fn civil_date_is_plausible() {
        let (d, m) = day_month();
        assert!((1..=31).contains(&d) && (1..=12).contains(&m));
    }
}
