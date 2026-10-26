//! Arguments passed by a launcher, like the official game (`--username`, `--uuid`,
//! `--accessToken`, `--version`, `--gameDir`, `--quickPlayMultiplayer`, `--width`…), and the
//! resulting player account (`docs/specs/authentication.md` § 1.8, option "without built-in
//! sign-in"). Unknown options ignored without error, like the game. The token lives only in
//! memory; no option value is logged (only names).

use std::sync::OnceLock;

use rc_client::auth::{self, AccessToken, Account};

/// Launcher options recognised.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Launch {
    pub username: Option<String>,
    pub uuid: Option<String>,
    pub access_token: Option<AccessToken>,
    pub user_type: Option<String>,
    pub xuid: Option<String>,
    pub client_id: Option<String>,
    pub version: Option<String>,
    pub version_type: Option<String>,
    pub game_dir: Option<String>,
    pub assets_dir: Option<String>,
    pub asset_index: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub fullscreen: bool,
    pub demo: bool,
    pub disable_multiplayer: bool,
    pub disable_chat: bool,
    /// `--quickPlayMultiplayer <address>`: direct connection to this server.
    pub quick_play_multiplayer: Option<String>,
    /// `--quickPlaySingleplayer [world]`: direct opening of this world (empty: the last one played),
    /// value unescaped the Java way.
    pub quick_play_singleplayer: Option<String>,
    /// `--quickPlayPath <file>`: log of the games played (relative to the game folder).
    pub quick_play_path: Option<String>,
    /// Quick launch options given (singleplayer, multiplayer, Realms): the game refuses to have
    /// more than one.
    pub quick_play_count: u8,
    /// `--proxyHost`, `--proxyPort`, `--proxyUser`, `--proxyPass` (credentials masked).
    pub proxy: rc_client::session::LauncherProxy,
    /// Names of the options recognised but without effect for us (startup log).
    pub unused: Vec<String>,
    /// Names of the unknown options, ignored.
    pub unknown: Vec<String>,
}

/// Options with a mandatory value (next value, or `--name=value`).
const WITH_VALUE: &[&str] = &[
    "username",
    "uuid",
    "accessToken",
    "userType",
    "version",
    "versionType",
    "gameDir",
    "assetsDir",
    "assetIndex",
    "resourcePackDir",
    "width",
    "height",
    "fullscreenWidth",
    "fullscreenHeight",
    "quickPlayPath",
    "quickPlayMultiplayer",
    "quickPlayRealms",
    "proxyHost",
    "proxyPort",
    "proxyUser",
    "proxyPass",
    "graphicsBackend",
];

/// Options with an optional value (taken if it does not start with `--`).
const OPTIONAL_VALUE: &[&str] = &["xuid", "clientId", "quickPlaySingleplayer"];

/// Options without a value.
const FLAGS: &[&str] = &[
    "demo",
    "disableMultiplayer",
    "disableChat",
    "fullscreen",
    "checkGlErrors",
    "renderDebugLabels",
    "vulkanValidation",
    "jfrProfile",
    "tracy",
    "tracyNoImages",
    "offlineDeveloperMode",
];

/// Main class of the game in a launcher's Java command line.
pub const MAIN_CLASS: &str = "net.minecraft.client.main.Main";

/// JVM options whose value is the next argument.
const JVM_WITH_VALUE: &[&str] = &[
    "-cp",
    "-classpath",
    "--class-path",
    "-p",
    "--module-path",
    "--upgrade-module-path",
    "--add-modules",
    "--add-exports",
    "--add-opens",
    "--add-reads",
    "--patch-module",
    "--limit-modules",
    "--enable-native-access",
];

/// Version requests addressed to "Java".
const JAVA_VERSION: &[&str] =
    &["-version", "--version", "-showversion", "--show-version", "-fullversion", "--full-version"];

/// Reading the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// Game arguments (launcher options and/or `host port username`).
    Game(Vec<String>),
    /// Rustcraft called as Java for a simple version question: text to write on
    /// the error output (like `java -version`), then exit.
    JavaVersion(String),
}

/// The official launcher, when the "Java executable" of an installation is `rustcraft.exe`,
/// passes the complete Java command line: JVM options, main class, then the game
/// arguments. Everything before the main class is ignored. Without JVM options
/// (first argument not `-x…`), the line is the game's as it is.
pub fn split_command(args: Vec<String>) -> Command {
    if let Some(i) = args.iter().position(|a| a == MAIN_CLASS) {
        return Command::Game(args[i + 1..].to_vec());
    }
    let jvm = args.first().is_some_and(|a| a.starts_with('-') && !a.starts_with("--"));
    if !jvm {
        return match args.as_slice() {
            [only] if JAVA_VERSION.contains(&only.as_str()) => Command::JavaVersion(java_version()),
            _ => Command::Game(args),
        };
    }
    let mut it = args.into_iter();
    let mut asked = false;
    while let Some(a) = it.next() {
        if JAVA_VERSION.contains(&a.as_str()) {
            asked = true;
        } else if JVM_WITH_VALUE.contains(&a.as_str()) {
            it.next();
        } else if !a.starts_with('-') {
            // Other main class (or `-jar` absent): what follows remains the game arguments.
            return Command::Game(it.collect());
        }
    }
    if asked { Command::JavaVersion(java_version()) } else { Command::Game(Vec::new()) }
}

/// Answer to `-version`, in Java's format (version requested by 26.3: 25).
fn java_version() -> String {
    format!(
        "openjdk version \"25\" 2025-09-16 LTS\nOpenJDK Runtime Environment (Rustcraft {v})\nOpenJDK 64-Bit Server VM (Rustcraft {v})",
        v = env!("CARGO_PKG_VERSION")
    )
}

impl Launch {
    /// Separates the launcher options from the positional arguments (`host port username`).
    pub fn parse(args: impl IntoIterator<Item = String>) -> (Launch, Vec<String>) {
        let mut out = Launch::default();
        let mut positional = Vec::new();
        let mut it = args.into_iter().peekable();
        while let Some(a) = it.next() {
            let Some(opt) = a.strip_prefix("--").filter(|o| !o.is_empty()) else {
                positional.push(a);
                continue;
            };
            let (name, inline) = match opt.split_once('=') {
                Some((n, v)) => (n.to_owned(), Some(v.to_owned())),
                None => (opt.to_owned(), None),
            };
            let value = if FLAGS.contains(&name.as_str()) {
                None
            } else if inline.is_some() {
                inline
            } else if WITH_VALUE.contains(&name.as_str()) {
                it.next()
            } else {
                // Optional value, or unknown option: its possible value is swallowed so as
                // not to be taken for a positional argument.
                it.next_if(|v: &String| !v.starts_with("--"))
            };
            out.set(&name, value);
        }
        (out, positional)
    }

    fn set(&mut self, name: &str, value: Option<String>) {
        let num = |v: &Option<String>| v.as_deref().and_then(|s| s.parse::<u32>().ok()).filter(|&n| n > 0);
        match name {
            "username" => self.username = value.filter(|v| !v.is_empty()),
            "uuid" => self.uuid = value,
            "accessToken" => self.access_token = value.and_then(AccessToken::new),
            "userType" => self.user_type = value,
            "xuid" => self.xuid = value.filter(|v| !v.is_empty()),
            "clientId" => self.client_id = value.filter(|v| !v.is_empty()),
            "version" => self.version = value,
            "versionType" => self.version_type = value,
            "gameDir" => self.game_dir = value,
            "assetsDir" => self.assets_dir = value,
            "assetIndex" => self.asset_index = value,
            "width" => self.width = num(&value),
            "height" => self.height = num(&value),
            "quickPlayMultiplayer" => {
                self.quick_play_count += 1;
                self.quick_play_multiplayer =
                    value.map(|v| crate::quickplay::unescape_java(&v)).filter(|v| !v.is_empty());
            }
            "quickPlaySingleplayer" => {
                self.quick_play_count += 1;
                self.quick_play_singleplayer = Some(crate::quickplay::unescape_java(&value.unwrap_or_default()));
            }
            "quickPlayRealms" => {
                // Realms out of scope: only counted (refusal of several options).
                self.quick_play_count += 1;
                self.unused.push(name.to_owned());
            }
            "quickPlayPath" => self.quick_play_path = value,
            "fullscreen" => self.fullscreen = true,
            "demo" => self.demo = true,
            "disableMultiplayer" => self.disable_multiplayer = true,
            "disableChat" => self.disable_chat = true,
            "proxyHost" => self.proxy.host = value,
            "proxyPort" => self.proxy.port = value,
            "proxyUser" => self.proxy.user = value,
            "proxyPass" => self.proxy.pass = value,
            n if WITH_VALUE.contains(&n) || OPTIONAL_VALUE.contains(&n) || FLAGS.contains(&n) => {
                self.unused.push(n.to_owned());
            }
            n => self.unknown.push(n.to_owned()),
        }
    }

    /// Launcher account: only if a UUID or a token is given (otherwise, offline username
    /// as before). Invalid UUID → offline UUID of the username, like the game.
    pub fn account(&self) -> Option<Account> {
        if self.uuid.is_none() && self.access_token.is_none() {
            return None;
        }
        let name = self.username.clone().unwrap_or_else(|| "Rustcraft".into());
        let uuid = self.uuid.as_deref().and_then(auth::parse_uuid).unwrap_or_else(|| rc_client::offline_uuid(&name));
        Some(Account {
            name,
            uuid,
            token: self.access_token.clone(),
            user_type: self.user_type.clone(),
            xuid: self.xuid.clone(),
            client_id: self.client_id.clone(),
        })
    }

    /// Startup line: what is received, without any secret value.
    pub fn summary(&self) -> Option<String> {
        let account = self.uuid.is_some() || self.access_token.is_some();
        if !account && self.unused.is_empty() && self.unknown.is_empty() && self.version.is_none() {
            return None;
        }
        let mut s = format!(
            "launcher: account {}, token {}",
            if account { "provided" } else { "absent" },
            if self.access_token.is_some() { "present" } else { "absent" }
        );
        if !self.unused.is_empty() {
            s += &format!(", no effect: {}", self.unused.join(" "));
        }
        if !self.unknown.is_empty() {
            s += &format!(", unknown ignored: {}", self.unknown.join(" "));
        }
        Some(s)
    }
}

static WINDOW: OnceLock<(u32, u32)> = OnceLock::new();

/// Window size requested by the launcher (`--width`, `--height`), in pixels.
pub fn set_window_size(launch: &Launch) {
    if launch.width.is_some() || launch.height.is_some() {
        let _ = WINDOW.set((launch.width.unwrap_or(854), launch.height.unwrap_or(480)));
    }
}

/// Initial size of the client area in pixels: the launcher's, otherwise 854 × 480 (the game's
/// default), at least 320 × 240.
pub fn window_size() -> winit::dpi::PhysicalSize<u32> {
    let (w, h) = WINDOW.get().copied().unwrap_or((854, 480));
    winit::dpi::PhysicalSize::new(w.max(320), h.max(240))
}

/// Window creation attributes (initial size and minimum size 320 × 240).
pub fn window_attributes() -> winit::window::WindowAttributes {
    winit::window::Window::default_attributes()
        .with_title("Rustcraft")
        .with_inner_size(window_size())
        .with_min_inner_size(winit::dpi::PhysicalSize::new(320u32, 240u32))
        // Test clients (`RUSTCRAFT_BACKGROUND=1`): created hidden, shown without activation
        // (`show_background`); Windows otherwise activates the first window of a launched process.
        .with_active(!background())
        .with_visible(!background())
}

/// Shows the window of a test client without giving it focus. Goes through winit: created
/// inactive, winit shows it with `SW_SHOWNOACTIVATE` and knows it is visible. A direct `ShowWindow`
/// made winit believe the window was hidden: its next state change (fullscreen from
/// `options.txt`, maximising) hid it again (`SW_HIDE`), and a hidden window receives
/// no more draw events, hence no more frames.
pub fn show_background(window: &winit::window::Window) {
    if background() {
        window.set_visible(true);
    }
}

/// Test client launched in the background: no focus on opening nor cursor capture.
pub fn background() -> bool {
    rc_data::env_str!("RUSTCRAFT_BACKGROUND").is_some_and(|v| v == "1")
}

/// Client area centred on the main screen (like the game at window creation); the
/// title bar sticks out above.
pub fn center_window(window: &winit::window::Window, el: &winit::event_loop::ActiveEventLoop) {
    let Some(mon) = el.primary_monitor().or_else(|| window.current_monitor()) else { return };
    let size = window_size();
    let (x, y) = centered(mon.position(), mon.size(), size);
    // Frame offset: requested outer position = client − (client − outer).
    let (dx, dy) = match (window.inner_position(), window.outer_position()) {
        (Ok(i), Ok(o)) => (i.x - o.x, i.y - o.y),
        _ => (0, 0),
    };
    window.set_outer_position(winit::dpi::PhysicalPosition::new(x - dx, y - dy));
}

/// Corner of the centred client area (integer division truncated towards zero, like the game).
fn centered(
    pos: winit::dpi::PhysicalPosition<i32>,
    mon: winit::dpi::PhysicalSize<u32>,
    win: winit::dpi::PhysicalSize<u32>,
) -> (i32, i32) {
    (pos.x + (mon.width as i32 - win.width as i32) / 2, pos.y + (mon.height as i32 - win.height as i32) / 2)
}

/// Window icons (root of the game pack: `icons/icon_<n>x<n>.png`, from largest to
/// smallest), read through the launcher's asset index.
pub const ICONS: [&str; 5] = [
    "assets/icons/icon_256x256.png",
    "assets/icons/icon_128x128.png",
    "assets/icons/icon_48x48.png",
    "assets/icons/icon_32x32.png",
    "assets/icons/icon_16x16.png",
];

/// Sets the icon: like the game's window library on Windows, the same image (the one at
/// the system icon size, 32 px at 100%) serves for the title bar and the taskbar.
pub fn set_icon(window: &winit::window::Window, assets: &rc_assets::Assets) {
    let images: Vec<rc_assets::texture::Image> = ICONS.iter().filter_map(|p| assets.image(p)).collect();
    if images.is_empty() {
        eprintln!("window icon not found (asset index missing?)");
        return;
    }
    let want = icon_metric();
    let pick = images.iter().filter(|i| i.width >= want).min_by_key(|i| i.width).unwrap_or(&images[0]);
    let icon = |i: &rc_assets::texture::Image| winit::window::Icon::from_rgba(i.rgba.clone(), i.width, i.height).ok();
    window.set_window_icon(icon(pick));
    #[cfg(windows)]
    {
        use winit::platform::windows::WindowExtWindows;
        window.set_taskbar_icon(icon(pick));
    }
}

/// System icon size in pixels (SM_CXICON on Windows; 32 elsewhere).
fn icon_metric() -> u32 {
    #[cfg(windows)]
    #[allow(unsafe_code)]
    {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn GetSystemMetrics(index: i32) -> i32;
        }
        // SAFETY: reading a system metric, no pointer.
        let v = unsafe { GetSystemMetrics(11) };
        if v > 0 {
            return v as u32;
        }
    }
    32
}

/// UUID and token to connect as `name`: those of the launcher's account if it carries this
/// username, otherwise the offline UUID and no token.
pub fn profile(name: &str) -> (u128, Option<AccessToken>) {
    match auth::account() {
        Some(a) if a.name == name => (a.uuid, a.token.clone()),
        _ => (rc_client::offline_uuid(name), None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_window_is_centered_854_by_480() {
        use winit::dpi::{PhysicalPosition, PhysicalSize};
        assert_eq!(window_size(), PhysicalSize::new(854, 480));
        assert_eq!(centered(PhysicalPosition::new(0, 0), PhysicalSize::new(1920, 1080), window_size()), (533, 300));
        assert_eq!(
            centered(PhysicalPosition::new(-1280, 0), PhysicalSize::new(1280, 1024), window_size()),
            (-1067, 272)
        );
        // Window larger than the screen: truncation towards zero.
        assert_eq!(centered(PhysicalPosition::new(0, 0), PhysicalSize::new(800, 600), window_size()), (-27, 60));
    }

    fn parse(v: &[&str]) -> (Launch, Vec<String>) {
        Launch::parse(v.iter().map(|s| (*s).to_owned()))
    }

    #[test]
    fn launcher_command_line() {
        let token = "eyJraWQiOiJhYmMiLCJhbGciOiJSUzI1NiJ9.payload.sig";
        let (l, pos) = parse(&[
            "--username",
            "Steve",
            "--version",
            "26.3",
            "--gameDir",
            "C:/inst/.minecraft",
            "--assetsDir",
            "C:/assets",
            "--assetIndex",
            "29",
            "--uuid",
            "069a79f4-44e9-4726-a5be-fca90e38aaf5",
            "--accessToken",
            token,
            "--clientId",
            "--xuid",
            "2535",
            "--userType",
            "msa",
            "--versionType",
            "release",
            "--width",
            "1280",
            "--height",
            "720",
            "--quickPlayMultiplayer",
            "play.example.net:25566",
            "--launcherFeature",
            "value",
            "--fullscreen",
        ]);
        assert!(pos.is_empty());
        assert_eq!(l.username.as_deref(), Some("Steve"));
        assert_eq!(l.client_id, None);
        assert_eq!(l.xuid.as_deref(), Some("2535"));
        assert_eq!((l.width, l.height), (Some(1280), Some(720)));
        assert_eq!(l.quick_play_multiplayer.as_deref(), Some("play.example.net:25566"));
        assert!(l.fullscreen);
        assert_eq!(l.unknown, vec!["launcherFeature".to_owned()]);
        let a = l.account().expect("account");
        assert_eq!(a.uuid, 0x069a79f444e94726a5befca90e38aaf5);
        assert!(a.token.is_some());
        let summary = l.summary().expect("summary");
        assert!(!summary.contains(token) && !summary.contains("2535"));
        assert!(!format!("{l:?}").contains("payload"));
    }

    fn split(v: &[&str]) -> Command {
        split_command(v.iter().map(|s| (*s).to_owned()).collect())
    }

    #[test]
    fn official_launcher_java_command_line() {
        // Shape of the official launcher's line (version 26.3.json, fictitious values).
        let c = split(&[
            "-Xmx2G",
            "-XX:+UnlockExperimentalVMOptions",
            "-XX:HeapDumpPath=x.heapdump",
            "-Xss1M",
            "--enable-native-access=ALL-UNNAMED",
            "--add-exports",
            "java.base/jdk.internal.misc=ALL-UNNAMED",
            "-Djava.library.path=C:/natives/java",
            "-Dminecraft.launcher.brand=minecraft-launcher",
            "-cp",
            "C:/libs/a.jar;C:/versions/26.3/26.3.jar",
            MAIN_CLASS,
            "--username",
            "Steve",
            "--version",
            "26.3",
        ]);
        assert_eq!(c, Command::Game(vec!["--username".into(), "Steve".into(), "--version".into(), "26.3".into()]));
        // Without a known class: first value outside a JVM option.
        assert_eq!(split(&["-Xmx1G", "-cp", "a.jar", "other.Main", "--demo"]), Command::Game(vec!["--demo".into()]));
        assert!(matches!(split(&["-version"]), Command::JavaVersion(v) if v.starts_with("openjdk version \"25\"")));
        assert!(matches!(split(&["-XshowSettings:properties", "-version"]), Command::JavaVersion(_)));
        assert!(matches!(split(&["--version"]), Command::JavaVersion(_)));
        // Game lines unchanged.
        assert_eq!(split(&["--version", "26.3"]), Command::Game(vec!["--version".into(), "26.3".into()]));
        assert_eq!(split(&["h", "1", "X"]), Command::Game(vec!["h".into(), "1".into(), "X".into()]));
        assert_eq!(split(&[]), Command::Game(Vec::new()));
    }

    #[test]
    fn quick_play_singleplayer_and_log_path() {
        let (l, pos) = parse(&["--quickPlaySingleplayer", "Caf\\u00e9 world", "--quickPlayPath", "quickPlay/log.json"]);
        assert!(pos.is_empty());
        assert_eq!(l.quick_play_singleplayer.as_deref(), Some("Caf\u{e9} world"));
        assert_eq!(l.quick_play_path.as_deref(), Some("quickPlay/log.json"));
        assert_eq!(l.quick_play_count, 1);
        // Without a value: last world played (empty string).
        let (l, _) = parse(&["--quickPlaySingleplayer", "--demo"]);
        assert_eq!((l.quick_play_singleplayer.as_deref(), l.demo), (Some(""), true));
        // Two quick launch options: refused by the game.
        let (l, _) = parse(&["--quickPlaySingleplayer", "a", "--quickPlayMultiplayer", "h"]);
        assert_eq!(l.quick_play_count, 2);
        // Multiplayer address unescaped too.
        let (l, _) = parse(&["--quickPlayMultiplayer", "caf\\u00e9.example.net"]);
        assert_eq!(l.quick_play_multiplayer.as_deref(), Some("caf\u{e9}.example.net"));
    }

    #[test]
    fn offline_placeholders_and_positionals() {
        let (l, pos) = parse(&["127.0.0.1", "25570", "Bob", "--accessToken=0", "--demo"]);
        assert_eq!(pos, ["127.0.0.1", "25570", "Bob"]);
        assert_eq!(l.access_token, None);
        assert!(l.demo);
        assert_eq!(l.account(), None);
        // Invalid UUID: offline UUID of the username, like the game.
        let (l, _) = parse(&["--username", "Bob", "--uuid", "zz"]);
        assert_eq!(l.account().map(|a| a.uuid), Some(rc_client::offline_uuid("Bob")));
    }
}
