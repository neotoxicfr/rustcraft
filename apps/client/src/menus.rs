//! In-game menu screens (`docs/specs/option-menus.md` § 2–3): pause menu, Options screen and
//! sub-screens (graphics, sounds, controls, mouse, keys, chat, accessibility, language,
//! skin). Each screen carries a [`Panel`] rebuilt when the interface size changes;
//! the options are read and written in [`Options`].

use std::time::Instant;

use rc_text::StyledChar;
use winit::keyboard::{KeyCode, ModifiersState};

use crate::gui::Gui;
use crate::options::{self, Cat, KEYS, Options, Ty, Val, java_double};
use crate::widgets::{self, Entry, Event, Kind, List, Panel, TextField, Widget, chars, tr};

/// Request from the screen to the application.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Return to the game (mouse recaptured).
    Resume,
    /// Disconnection (closing on the client side).
    Disconnect,
    /// Settings changed: effects to apply.
    Changed,
    /// Volume slider moved: category, new value (sound preview outside the world).
    SoundPreview(rc_audio::Category, f32),
    /// Writing of `options.txt` (and client information if changed).
    Save,
    /// Language chosen (code): translations to reload.
    Language(String),
    /// "Resource Packs" screen (`pack_screen.rs`).
    ResourcePacks,
    /// Advancements and statistics screens, opened on top of the pause menu (keyboard).
    Advancements(bool),
    Stats(bool),
    /// World options (`world_options.rs`): difficulty, lock, personal mode (singleplayer).
    ChangeDifficulty(i32),
    LockDifficulty,
    ChangeGameMode(i32),
    /// Game rules: request for the values, then sending of the modified rules.
    RequestGameRules,
    SetGameRules(Vec<(String, String)>),
    /// Button added by the server to the pause menu (dialog).
    PauseExtra,
    /// Credits scroll ("Credits & Attributions").
    Credits,
    /// Link confirmed: opened in the browser.
    OpenUrl(String),
    /// Player hidden (true) or shown again in the chat: name (chat message).
    HidePlayer(bool, String),
    /// Opening to the local network (singleplayer): open, port, guests' commands, forced mode.
    Publish { open: bool, port: u16, guest: bool, force: Option<i32> },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Id {
    Pause,
    Options,
    Video,
    Sounds,
    Controls,
    Mouse,
    Keybinds,
    Chat,
    Accessibility,
    Language,
    Skin,
    /// Screens of `world_options.rs`.
    WorldOptions,
    GameRules,
    Restrictions,
    Credits,
    Social,
    Confirm(Confirm),
    /// Generic sub-screens: online, font.
    Online,
    Font,
}

/// Confirmations (`world_options.rs`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Confirm {
    /// Locking of the difficulty (window over the world options).
    LockDifficulty,
    /// Abandoning the modified game rules.
    DiscardRules,
    /// Opening of a link (`World::url`).
    Link,
}

impl Id {
    /// Screens to stack on the pause menu for the demonstration (`RUSTCRAFT_MENUTEST`).
    pub fn test_path(name: &str) -> Option<Vec<Id>> {
        Some(match name {
            "pause" => Vec::new(),
            "world_options" => vec![Id::WorldOptions],
            "game_rules" => vec![Id::WorldOptions, Id::GameRules],
            "restrictions" => vec![Id::WorldOptions, Id::Restrictions],
            "online" => vec![Id::Options, Id::Online],
            "font" => vec![Id::Options, Id::Language, Id::Font],
            "credits" => vec![Id::Options, Id::Credits],
            "social" => vec![Id::Social],
            "options" => vec![Id::Options],
            "video" => vec![Id::Options, Id::Video],
            "sounds" => vec![Id::Options, Id::Sounds],
            "controls" => vec![Id::Options, Id::Controls],
            "mouse" => vec![Id::Options, Id::Controls, Id::Mouse],
            "keybinds" => vec![Id::Options, Id::Controls, Id::Keybinds],
            "chat" => vec![Id::Options, Id::Chat],
            "accessibility" => vec![Id::Options, Id::Accessibility],
            "language" => vec![Id::Options, Id::Language],
            "skin" => vec![Id::Options, Id::Skin],
            _ => return None,
        })
    }

    fn title(self) -> &'static str {
        match self {
            Id::Pause => "menu.game",
            Id::Options => "options.title",
            Id::Video => "options.videoTitle",
            Id::Sounds => "options.sounds.title",
            Id::Controls => "controls.title",
            Id::Mouse => "options.mouse_settings.title",
            Id::Keybinds => "controls.keybinds.title",
            Id::Chat => "options.chat.title",
            Id::Accessibility => "options.accessibility.title",
            Id::Language => "options.language.title",
            Id::Skin => "options.skinCustomisation.title",
            Id::Online => "options.online.title",
            Id::Font => "options.font.title",
            id => crate::world_options::title(id).unwrap_or(""),
        }
    }
}

/// Setting displayed by a component.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Opt {
    /// Boolean: cyclic ON / OFF button (true then false).
    Bool(&'static str, &'static str),
    /// Named values: (value in the file, translation key).
    Choice(&'static str, &'static str, &'static [(&'static str, &'static str)]),
    Slider(&'static str, &'static str),
    /// Interface scale: automatic then 1..maximum.
    GuiScale,
    /// Button to a sub-screen.
    Nav(Id, &'static str),
    /// Button to the resource packs screen (`pack_screen.rs`, outside this module).
    Packs(&'static str),
    /// Inactive button (screen or feature absent).
    Off(&'static str),
    /// Inactive setting "name: value".
    OffValue(&'static str, &'static str),
    /// Sound output device (`soundDevice`): default then the devices present.
    Device,
    /// Button to a link (opening confirmation): label, address.
    Link(&'static str, &'static str),
}

/// Row of an option list (§ 3.3).
enum Row {
    Big(Opt),
    Pair(Opt, Option<Opt>),
    Header(&'static str),
}

const HOLD_TOGGLE: &[(&str, &str)] = &[("false", "options.key.hold"), ("true", "options.key.toggle")];

fn pairs(opts: &[Opt]) -> Vec<Row> {
    opts.chunks(2).map(|c| Row::Pair(c[0], c.get(1).copied())).collect()
}

fn sound_label(c: &str) -> &'static str {
    match c {
        "master" => "soundCategory.master",
        "music" => "soundCategory.music",
        "record" => "soundCategory.record",
        "weather" => "soundCategory.weather",
        "block" => "soundCategory.block",
        "hostile" => "soundCategory.hostile",
        "neutral" => "soundCategory.neutral",
        "player" => "soundCategory.player",
        "ambient" => "soundCategory.ambient",
        "voice" => "soundCategory.voice",
        _ => "soundCategory.ui",
    }
}

const SOUND_KEYS: [&str; 11] = [
    "soundCategory_master",
    "soundCategory_music",
    "soundCategory_record",
    "soundCategory_weather",
    "soundCategory_block",
    "soundCategory_hostile",
    "soundCategory_neutral",
    "soundCategory_player",
    "soundCategory_ambient",
    "soundCategory_voice",
    "soundCategory_ui",
];

/// Contents of the option lists, in the game's order (§ 3.4–3.11).
fn rows(id: Id, opts: &Options) -> Vec<Row> {
    use Opt::*;
    match id {
        Id::Video => {
            let mut r = vec![Row::Header("options.video.display.header")];
            r.push(Row::Big(Slider("fullscreenResolution", "options.fullscreen.exclusive.mode")));
            r.extend(pairs(&[
                Slider("maxFps", "options.framerateLimit"),
                Bool("enableVsync", "options.vsync"),
                Choice(
                    "inactivityFpsLimit",
                    "options.inactivityFpsLimit",
                    &[("minimized", "options.inactivityFpsLimit.minimized"), ("afk", "options.inactivityFpsLimit.afk")],
                ),
                GuiScale,
                Bool("fullscreen", "options.fullscreen"),
                Bool("exclusiveFullscreen", "options.exclusiveFullscreen"),
                Slider("gamma", "options.gamma"),
                Choice(
                    "preferredGraphicsBackend",
                    "options.graphicsApi",
                    &[
                        ("default", "options.graphicsApi.default"),
                        ("opengl", "options.graphicsApi.opengl"),
                        ("vulkan", "options.graphicsApi.vulkan"),
                    ],
                ),
            ]));
            r.push(Row::Header("options.video.quality.header"));
            r.push(Row::Big(Slider("graphicsPreset", "options.graphics.preset")));
            let aniso = opts.i("textureFiltering") == 2;
            r.extend(pairs(&[
                Slider("biomeBlendRadius", "options.biomeBlendRadius"),
                Slider("renderDistance", "options.renderDistance"),
                Choice(
                    "prioritizeChunkUpdates",
                    "options.prioritizeChunkUpdates",
                    &[
                        ("0", "options.prioritizeChunkUpdates.none"),
                        ("1", "options.prioritizeChunkUpdates.byPlayer"),
                        ("2", "options.prioritizeChunkUpdates.nearby"),
                    ],
                ),
                Slider("simulationDistance", "options.simulationDistance"),
                Bool("ao", "options.ao"),
                Choice(
                    "renderClouds",
                    "options.renderClouds",
                    &[("false", "options.off"), ("fast", "options.clouds.fast"), ("true", "options.clouds.fancy")],
                ),
                Choice(
                    "particles",
                    "options.particles",
                    &[
                        ("0", "options.particles.all"),
                        ("1", "options.particles.decreased"),
                        ("2", "options.particles.minimal"),
                    ],
                ),
                Slider("mipmapLevels", "options.mipmapLevels"),
                Bool("entityShadows", "options.entityShadows"),
                Slider("entityDistanceScaling", "options.entityDistanceScaling"),
                Slider("menuBackgroundBlurriness", "options.accessibility.menu_background_blurriness"),
                Slider("cloudRange", "options.renderCloudsDistance"),
                Bool("cutoutLeaves", "options.cutoutLeaves"),
                Bool("improvedTransparency", "options.improvedTransparency"),
                Choice(
                    "textureFiltering",
                    "options.textureFiltering",
                    &[
                        ("0", "options.textureFiltering.none"),
                        ("1", "options.textureFiltering.rgss"),
                        ("2", "options.textureFiltering.anisotropic"),
                    ],
                ),
                if aniso { Slider("maxAnisotropyBit", "options.maxAnisotropy") } else { Off("maxAnisotropyBit") },
                Slider("weatherRadius", "options.weatherRadius"),
            ]));
            r.push(Row::Header("options.video.preferences.header"));
            r.extend(pairs(&[
                Bool("showAutosaveIndicator", "options.autosaveIndicator"),
                Bool("vignette", "options.vignette"),
                Choice(
                    "attackIndicator",
                    "options.attackIndicator",
                    &[("0", "options.off"), ("1", "options.attack.crosshair"), ("2", "options.attack.hotbar")],
                ),
                Slider("chunkSectionFadeInTime", "options.chunkFade"),
            ]));
            r
        }
        Id::Sounds => {
            let mut r = vec![Row::Big(Slider(SOUND_KEYS[0], sound_label("master")))];
            let others: Vec<Opt> = SOUND_KEYS[1..]
                .iter()
                .map(|k| Slider(k, sound_label(k.trim_start_matches("soundCategory_"))))
                .collect();
            r.extend(pairs(&others));
            r.push(Row::Big(Device));
            r.extend(pairs(&[
                Bool("showSubtitles", "options.showSubtitles"),
                Bool("directionalAudio", "options.directionalAudio"),
                Choice(
                    "musicFrequency",
                    "options.music_frequency",
                    &[
                        ("DEFAULT", "options.music_frequency.default"),
                        ("FREQUENT", "options.music_frequency.frequent"),
                        ("CONSTANT", "options.music_frequency.constant"),
                    ],
                ),
                Choice(
                    "musicToast",
                    "options.musicToast",
                    &[
                        ("never", "options.musicToast.never"),
                        ("pause", "options.musicToast.pauseMenu"),
                        ("pause_and_toast", "options.musicToast.pauseMenuAndToast"),
                    ],
                ),
            ]));
            r
        }
        Id::Chat => pairs(&[
            Choice(
                "chatVisibility",
                "options.chat.visibility",
                &[
                    ("0", "options.chat.visibility.full"),
                    ("1", "options.chat.visibility.system"),
                    ("2", "options.chat.visibility.hidden"),
                ],
            ),
            Bool("chatColors", "options.chat.color"),
            Bool("chatLinks", "options.chat.links"),
            Bool("chatLinksPrompt", "options.chat.links.prompt"),
            Slider("chatOpacity", "options.chat.opacity"),
            Slider("textBackgroundOpacity", "options.accessibility.text_background_opacity"),
            Slider("chatScale", "options.chat.scale"),
            Slider("chatLineSpacing", "options.chat.line_spacing"),
            Slider("chatDelay", "options.chat.delay_instant"),
            Slider("chatWidth", "options.chat.width"),
            Slider("chatHeightFocused", "options.chat.height.focused"),
            Slider("chatHeightUnfocused", "options.chat.height.unfocused"),
            OffValue("options.narrator", "options.narrator.notavailable"),
            Bool("autoSuggestions", "options.autoSuggestCommands"),
            Bool("hideMatchedNames", "options.hideMatchedNames"),
            Bool("reducedDebugInfo", "options.reducedDebugInfo"),
            Bool("onlyShowSecureChat", "options.onlyShowSecureChat"),
            Bool("saveChatDrafts", "options.chat.drafts"),
        ]),
        Id::Accessibility => {
            let mut r = vec![Row::Pair(
                OffValue("options.narrator", "options.narrator.notavailable"),
                Some(Nav(Id::Controls, "options.controls")),
            )];
            r.extend(pairs(&[
                Bool("showSubtitles", "options.showSubtitles"),
                Off("options.accessibility.high_contrast"),
                Slider("menuBackgroundBlurriness", "options.accessibility.menu_background_blurriness"),
                Slider("textBackgroundOpacity", "options.accessibility.text_background_opacity"),
                Choice(
                    "backgroundForChatOnly",
                    "options.accessibility.text_background",
                    &[
                        ("true", "options.accessibility.text_background.chat"),
                        ("false", "options.accessibility.text_background.everywhere"),
                    ],
                ),
                Slider("chatOpacity", "options.chat.opacity"),
                Slider("chatLineSpacing", "options.chat.line_spacing"),
                Slider("chatDelay", "options.chat.delay_instant"),
                Slider("notificationDisplayTime", "options.notifications.display_time"),
                Bool("bobView", "options.viewBobbing"),
                Slider("screenEffectScale", "options.screenEffectScale"),
                Slider("fovEffectScale", "options.fovEffectScale"),
                Slider("darknessEffectScale", "options.darknessEffectScale"),
                Slider("damageTiltStrength", "options.damageTiltStrength"),
                Slider("glintSpeed", "options.glintSpeed"),
                Slider("glintStrength", "options.glintStrength"),
                Bool("hideLightningFlashes", "options.hideLightningFlashes"),
                Bool("darkMojangStudiosBackground", "options.darkMojangStudiosBackgroundColor"),
                Slider("panoramaScrollSpeed", "options.accessibility.panorama_speed"),
                Bool("hideSplashTexts", "options.hideSplashTexts"),
                Bool("narratorHotkey", "options.accessibility.narrator_hotkey"),
                Off("options.rotateWithMinecart"),
                Bool("highContrastBlockOutline", "options.accessibility.high_contrast_block_outline"),
            ]));
            r
        }
        Id::Controls => {
            let mut r =
                vec![Row::Pair(Nav(Id::Mouse, "options.mouse_settings"), Some(Nav(Id::Keybinds, "controls.keybinds")))];
            r.extend(pairs(&[
                Choice("toggleCrouch", "key.sneak", HOLD_TOGGLE),
                Choice("toggleSprint", "key.sprint", HOLD_TOGGLE),
                Choice("toggleAttack", "key.attack", HOLD_TOGGLE),
                Choice("toggleUse", "key.use", HOLD_TOGGLE),
                Bool("autoJump", "options.autoJump"),
                Slider("sprintWindow", "options.sprintWindow"),
                Bool("quitShortcuts", "options.quitShortcuts"),
                Bool("operatorItemsTab", "options.operatorItemsTab"),
            ]));
            r
        }
        Id::Mouse => pairs(&[
            Slider("mouseSensitivity", "options.sensitivity"),
            Slider("mouseWheelSensitivity", "options.mouseWheelSensitivity"),
            Bool("discrete_mouse_scroll", "options.discrete_mouse_scroll"),
            Bool("invertXMouse", "options.invertMouseX"),
            Bool("invertYMouse", "options.invertMouseY"),
            Bool("allowCursorChanges", "options.allowCursorChanges"),
        ]),
        Id::Skin => pairs(&[
            Bool("modelPart_cape", "options.modelPart.cape"),
            Bool("modelPart_jacket", "options.modelPart.jacket"),
            Bool("modelPart_left_sleeve", "options.modelPart.left_sleeve"),
            Bool("modelPart_right_sleeve", "options.modelPart.right_sleeve"),
            Bool("modelPart_left_pants_leg", "options.modelPart.left_pants_leg"),
            Bool("modelPart_right_pants_leg", "options.modelPart.right_pants_leg"),
            Bool("modelPart_hat", "options.modelPart.hat"),
            Choice(
                "mainHand",
                "options.mainHand",
                &[("left", "options.mainHand.left"), ("right", "options.mainHand.right")],
            ),
        ]),
        // Friends list: Microsoft account service, unavailable (like the game offline).
        Id::Online => {
            let presence = match opts.s("sharePresence") {
                "none" => "options.sharePresence.none",
                "limited" => "options.sharePresence.limited",
                _ => "options.sharePresence.all",
            };
            let notif = if opts.b("inGameNotification") { "options.on" } else { "options.off" };
            vec![
                Row::Header("options.online.friends.header"),
                Row::Pair(
                    OffValue("options.friendsList", "options.off"),
                    Some(OffValue("options.allowFriendRequests", "options.off")),
                ),
                Row::Pair(
                    OffValue("options.inGameNotification", notif),
                    Some(OffValue("options.sharePresence", presence)),
                ),
                Row::Big(Link("options.online.xboxSettings", "https://aka.ms/MinecraftJavaXboxPrivacyAndSafety")),
                Row::Header("options.online.servers.header"),
                Row::Big(Bool("allowServerListing", "options.allowServerListing")),
                Row::Header("options.online.realms.header"),
                Row::Big(Bool("realmsNotifications", "options.realmsNotifications.button")),
            ]
        }
        Id::Font => pairs(&[
            Bool("forceUnicodeFont", "options.forceUnicodeFont"),
            Bool("japaneseGlyphVariants", "options.japaneseGlyphVariants"),
        ]),
        _ => Vec::new(),
    }
}

// ---------- Sliders: position ↔ value (§ 3.1) ----------

#[derive(Debug, Clone, Copy)]
enum Sk {
    /// Integer `[lo, hi]`.
    Int(i32, i32),
    /// Integer `k × mul`, `k ∈ [lo, hi]`.
    Mul(i32, i32, i32),
    /// Real `k / div`, `k ∈ [lo, hi]`.
    Div(i32, i32, f64),
    Unit,
    /// Field of view: integer 30–110 stored as `(fov − 70) / 40`.
    Fov,
    /// Value = square of the position.
    Square,
    /// Wheel: `10^(n / 100)`, `n ∈ [−200, 100]`, not discrete.
    Wheel,
    /// Exclusive fullscreen mode: −1 (current mode) then the screen's modes (`video.rs`).
    Modes,
    /// Graphics preset (4-step enumeration).
    Preset,
}

fn slider_kind(key: &str) -> Sk {
    match key {
        "biomeBlendRadius" => Sk::Int(0, 7),
        "renderDistance" => Sk::Int(2, 32),
        "simulationDistance" => Sk::Int(5, 32),
        "maxFps" => Sk::Mul(1, 26, 10),
        "mipmapLevels" => Sk::Int(0, 4),
        "entityDistanceScaling" => Sk::Div(2, 20, 4.0),
        "menuBackgroundBlurriness" => Sk::Int(0, 10),
        "cloudRange" => Sk::Int(2, 128),
        "maxAnisotropyBit" => Sk::Int(1, 3),
        "weatherRadius" => Sk::Int(3, 10),
        "chunkSectionFadeInTime" => Sk::Div(0, 40, 20.0),
        "chatDelay" => Sk::Div(0, 60, 10.0),
        "notificationDisplayTime" => Sk::Div(5, 100, 10.0),
        "sprintWindow" => Sk::Int(0, 10),
        "fov" => Sk::Fov,
        "fovEffectScale" => Sk::Square,
        "mouseWheelSensitivity" => Sk::Wheel,
        "graphicsPreset" => Sk::Preset,
        "fullscreenResolution" => Sk::Modes,
        _ => Sk::Unit,
    }
}

/// Integer bounds of a discrete slider (or of the wheel).
fn int_range(k: Sk) -> Option<(i32, i32)> {
    match k {
        Sk::Int(lo, hi) | Sk::Mul(lo, hi, _) | Sk::Div(lo, hi, _) => Some((lo, hi)),
        Sk::Fov => Some((30, 110)),
        Sk::Wheel => Some((-200, 100)),
        Sk::Preset => Some((0, 3)),
        Sk::Modes => Some((-1, crate::video::mode_count() - 1)),
        Sk::Unit | Sk::Square => None,
    }
}

fn int_pos(k: i32, lo: i32, hi: i32) -> f64 {
    if k <= lo {
        0.0
    } else if k >= hi {
        1.0
    } else {
        (f64::from(k - lo) + 0.5) / f64::from(hi + 1 - lo)
    }
}

fn pos_int(v: f64, lo: i32, hi: i32) -> i32 {
    let v = v.clamp(0.0, 0.99999);
    lo + (v * f64::from(hi + 1 - lo)).floor() as i32
}

const PRESET_NAMES: [&str; 4] = ["fast", "fancy", "fabulous", "custom"];

/// Current integer index of a discrete slider.
fn current_int(key: &str, o: &Options, k: Sk) -> i32 {
    match k {
        Sk::Int(..) => o.i(key),
        Sk::Mul(_, _, m) => o.i(key) / m,
        Sk::Div(_, _, d) => (o.f(key) * d).round() as i32,
        Sk::Fov => o.fov(),
        Sk::Wheel => (o.f(key).log10() * 100.0).round() as i32,
        Sk::Preset => PRESET_NAMES.iter().position(|p| *p == o.s(key)).unwrap_or(1) as i32,
        Sk::Modes => crate::video::mode_index(o.s(key)),
        Sk::Unit | Sk::Square => 0,
    }
}

/// Option value from an integer index.
fn int_value(k: Sk, n: i32) -> Val {
    match k {
        Sk::Int(..) => Val::I(n),
        Sk::Mul(_, _, m) => Val::I(n * m),
        Sk::Div(_, _, d) => Val::F(f64::from(n) / d),
        Sk::Fov => Val::F(f64::from(n - 70) / 40.0),
        Sk::Wheel => Val::F(10f64.powf(f64::from(n) / 100.0)),
        Sk::Preset => Val::S(PRESET_NAMES[n.clamp(0, 3) as usize].into()),
        Sk::Modes => Val::S(crate::video::mode_json(n)),
        Sk::Unit | Sk::Square => Val::I(n),
    }
}

/// Slider position for the current value.
fn slider_pos(key: &str, o: &Options) -> f64 {
    let k = slider_kind(key);
    match k {
        Sk::Unit => o.f(key),
        Sk::Square => o.f(key).sqrt(),
        Sk::Preset => f64::from(current_int(key, o, k)) / 3.0,
        Sk::Wheel => f64::from(current_int(key, o, k) + 200) / 300.0,
        _ => {
            let (lo, hi) = int_range(k).unwrap_or((0, 1));
            int_pos(current_int(key, o, k), lo, hi)
        }
    }
}

/// Option value from a slider position.
fn pos_value(key: &str, v: f64) -> Val {
    let k = slider_kind(key);
    match k {
        Sk::Unit => Val::F(v),
        Sk::Square => Val::F(v * v),
        Sk::Preset => int_value(k, ((v * 4.0).floor() as i32).clamp(0, 3)),
        Sk::Wheel => int_value(k, (v * 300.0).round() as i32 - 200),
        _ => {
            let (lo, hi) = int_range(k).unwrap_or((0, 1));
            int_value(k, pos_int(v, lo, hi))
        }
    }
}

/// Steps of a slider for the keyboard (0 = continuous).
fn slider_steps(key: &str) -> u32 {
    match slider_kind(key) {
        Sk::Unit | Sk::Square | Sk::Wheel => 0,
        k => int_range(k).map_or(0, |(lo, hi)| (hi - lo + 1) as u32),
    }
}

/// Sliders applied 600 ms after the last movement (§ 1.5).
fn deferred(key: &str) -> bool {
    matches!(key, "renderDistance" | "simulationDistance" | "biomeBlendRadius")
}

// ---------- Labels ----------

fn val_text(v: &Val) -> String {
    match v {
        Val::B(b) => b.to_string(),
        Val::I(i) => i.to_string(),
        Val::F(f) => java_double(*f),
        Val::S(s) => s.clone(),
        Val::L(_) => String::new(),
    }
}

/// Label of a slider for a value (§ 3.1, 3.4–3.9).
fn slider_label(lang: &rc_text::Lang, key: &str, name: &str, v: &Val) -> String {
    let t = |k: &str, a: &[String]| tr(lang, k, a);
    let n = t(name, &[]);
    let gv = |value: String| t("options.generic_value", &[n.clone(), value]);
    let pct = |f: f64| t("options.percent_value", &[n.clone(), ((f * 100.0).floor() as i64).to_string()]);
    let off = || gv(t("options.off", &[]));
    let (i, f) = match v {
        Val::I(i) => (*i, f64::from(*i)),
        Val::F(f) => (*f as i32, *f),
        _ => (0, 0.0),
    };
    match key {
        "fullscreenResolution" if crate::video::mode_count() == 0 => t("options.fullscreen.unavailable", &[]),
        "fullscreenResolution" => match crate::video::mode_label_args(&val_text(v)) {
            Some(a) => gv(t("options.fullscreen.entry", &a)),
            None => gv(t("options.fullscreen.current", &[])),
        },
        "renderDistance" | "simulationDistance" | "cloudRange" => gv(t("options.chunks", &[i.to_string()])),
        "biomeBlendRadius" => gv(t(&format!("options.biomeBlendRadius.{}", 2 * i + 1), &[])),
        "maxFps" if i >= 260 => gv(t("options.framerateLimit.max", &[])),
        "maxFps" => gv(t("options.framerate", &[i.to_string()])),
        "mipmapLevels" | "menuBackgroundBlurriness" | "sprintWindow" if i == 0 => off(),
        "mipmapLevels" | "menuBackgroundBlurriness" | "sprintWindow" => gv(i.to_string()),
        "maxAnisotropyBit" => gv(t("options.multiplier", &[(1 << i).to_string()])),
        "weatherRadius" => gv(t("options.blocks", &[i.to_string()])),
        "entityDistanceScaling" => pct(f),
        "chunkSectionFadeInTime" if f == 0.0 => t("options.chunkFade.none", &[]),
        "chunkSectionFadeInTime" => t("options.chunkFade.seconds", &[java_double(f)]),
        "chatDelay" if f == 0.0 => t("options.chat.delay_none", &[]),
        "chatDelay" => t("options.chat.delay", &[java_double(f)]),
        "notificationDisplayTime" => gv(t("options.multiplier", &[java_double(f)])),
        "gamma" if f == 0.0 => gv(t("options.gamma.min", &[])),
        "gamma" if f == 0.5 => gv(t("options.gamma.default", &[])),
        "gamma" if f == 1.0 => gv(t("options.gamma.max", &[])),
        "fov" => {
            let deg = (70.0 + f * 40.0).round() as i32;
            match deg {
                70 => gv(t("options.fov.min", &[])),
                110 => gv(t("options.fov.max", &[])),
                d => gv(d.to_string()),
            }
        }
        "mouseSensitivity" if f == 0.0 => gv(t("options.sensitivity.min", &[])),
        "mouseSensitivity" if f == 1.0 => gv(t("options.sensitivity.max", &[])),
        "mouseSensitivity" => pct(2.0 * f),
        "mouseWheelSensitivity" => gv(format!("{f:.2}")),
        "chatOpacity" => pct(f * 0.9 + 0.1),
        "chatWidth" => t("options.pixel_value", &[n.clone(), ((f * 280.0 + 40.0).floor() as i32).to_string()]),
        "chatHeightFocused" | "chatHeightUnfocused" => {
            t("options.pixel_value", &[n.clone(), ((f * 160.0 + 20.0).floor() as i32).to_string()])
        }
        "graphicsPreset" => gv(t(&format!("options.graphics.{}", val_text(v)), &[])),
        k if (k.starts_with("soundCategory_") || matches!(k, "screenEffectScale" | "chatScale" | "fovEffectScale"))
            && f == 0.0 =>
        {
            off()
        }
        _ => pct(f),
    }
}

/// Label of an option component.
fn opt_label(lang: &rc_text::Lang, o: &Options, opt: &Opt, pending: Option<&Val>, max_scale: u32) -> String {
    let t = |k: &str, a: &[String]| tr(lang, k, a);
    let gv = |name: &str, value: String| t("options.generic_value", &[t(name, &[]), value]);
    match *opt {
        Opt::Bool(key, name) => gv(name, t(if o.b(key) { "options.on" } else { "options.off" }, &[])),
        Opt::Choice(key, name, values) => {
            let cur = match o.ty(key) {
                Some(Ty::Bool) => o.b(key).to_string(),
                Some(Ty::Int(..)) => o.i(key).to_string(),
                _ => o.s(key).to_owned(),
            };
            let label = values.iter().find(|v| v.0 == cur).map_or_else(|| cur.clone(), |v| t(v.1, &[]));
            gv(name, label)
        }
        Opt::Slider(key, name) => {
            let v = pending.cloned().unwrap_or_else(|| match o.ty(key) {
                Some(Ty::Int(..)) => Val::I(o.i(key)),
                Some(Ty::Enum(_) | Ty::RawStr) => Val::S(o.s(key).to_owned()),
                _ => Val::F(o.f(key)),
            });
            slider_label(lang, key, name, &v)
        }
        Opt::GuiScale => {
            let s = o.i("guiScale");
            let v = if s == 0 || s as u32 > max_scale { t("options.guiScale.auto", &[]) } else { s.to_string() };
            gv("options.guiScale", v)
        }
        Opt::Nav(_, name) | Opt::Packs(name) | Opt::Off(name) | Opt::Link(name, _) => t(name, &[]),
        Opt::OffValue(name, value) => gv(name, t(value, &[])),
        Opt::Device => match o.s("soundDevice") {
            "" => gv("options.audioDevice", t("options.audioDevice.default", &[])),
            d => gv("options.audioDevice", d.strip_prefix("OpenAL Soft on ").unwrap_or(d).to_owned()),
        },
    }
}

// ---------- Screens ----------

/// Identifiers: option components = 100 + rank; keys = 1000 + i (assignment),
/// 2000 + i (reset).
const OPT_BASE: u32 = 100;
const BIND: u32 = 1000;
const RESET: u32 = 2000;
const DONE: u32 = 50;
const RESET_ALL: u32 = 51;
const SEARCH: u32 = 52;

const RETURN: u32 = 1;
const ADVANCEMENTS: u32 = 2;
const STATS: u32 = 3;
const REPORT_BUGS: u32 = 4;
const FEEDBACK: u32 = 5;
const SOCIAL: u32 = 7;
/// Links of the pause menu icons (stable version of the main series: both active).
const BUGS_URL: &str = "https://aka.ms/snapshotbugs?ref=game";
const FEEDBACK_URL: &str = "https://aka.ms/javafeedback?ref=game";
const OPTIONS: u32 = 8;
const WORLD_OPTIONS: u32 = 9;
const DISCONNECT: u32 = 10;
const PAUSE_EXTRA: u32 = 11;
const ONLINE: u32 = 60;
const FONT: u32 = 61;

struct Screen {
    id: Id,
    panel: Panel,
    title: Vec<StyledChar>,
    /// Interface size at the last layout.
    size: (i32, i32),
    /// Settings of the components (identifier − 100).
    opts: Vec<Opt>,
    /// Deferred sliders: key, value, last movement.
    pending: Vec<(&'static str, Val, Instant)>,
    /// Key awaiting reassignment (index into `KEYS`).
    waiting: Option<usize>,
    /// Languages displayed (indices into the full list).
    shown: Vec<usize>,
    /// Maximum interface scale (cyclic scale button).
    max_scale: u32,
}

/// Available language: code, name, region.
#[derive(Debug, Clone, PartialEq)]
pub struct Language {
    pub code: String,
    pub name: String,
    pub region: String,
    /// Right-to-left language (reordered text, neutral paragraphs on the right).
    pub bidirectional: bool,
}

/// List of the languages (`pack.mcmeta` of the asset index, `language` section).
pub fn languages(pack_mcmeta: &[u8]) -> Vec<Language> {
    let Ok(v) = serde_json::from_slice::<serde_json::Value>(pack_mcmeta) else { return Vec::new() };
    let mut out: Vec<Language> = v["language"]
        .as_object()
        .map(|o| {
            o.iter()
                .map(|(code, l)| Language {
                    code: code.clone(),
                    name: l["name"].as_str().unwrap_or(code).to_owned(),
                    region: l["region"].as_str().unwrap_or("").to_owned(),
                    bidirectional: l["bidirectional"].as_bool().unwrap_or(false),
                })
                .collect()
        })
        .unwrap_or_default();
    out.sort_by(|a, b| a.code.cmp(&b.code));
    out
}

/// Stack of the open menu screens.
#[derive(Default)]
pub struct Menus {
    stack: Vec<Screen>,
    /// Clicks to sound (`ui.button.click`).
    pub clicks: u32,
    pub languages: Vec<Language>,
    /// Opened outside the world (from the title screen): backgrounds outside the world, over the panorama.
    pub outside: bool,
    /// State of the world options screens, the rules, the social interactions.
    pub world: crate::world_options::World,
    /// Credits scroll over the stack ("Credits & Attributions").
    pub roll: Option<crate::credits_roll::CreditsRoll>,
}

impl Menus {
    pub fn new(languages: Vec<Language>) -> Menus {
        Menus { languages, ..Default::default() }
    }

    /// Game state seen by the world options; a change rebuilds the top screen.
    pub fn set_world(&mut self, ctx: crate::world_options::Ctx) {
        if self.world.ctx != ctx {
            self.world.ctx = ctx;
            if let Some(top) = self.stack.last_mut() {
                top.size = (-2, -2);
            }
        }
    }

    /// Values of the game rules received (first answer only, like the game).
    pub fn game_rules(&mut self, values: Vec<(String, String)>) {
        if self.world.rules.is_none() && self.stack.iter().any(|s| s.id == Id::GameRules) {
            self.world.rules = Some(values);
            if let Some(top) = self.stack.last_mut() {
                top.size = (-2, -2);
            }
        }
    }

    /// Top screen.
    pub fn top(&self) -> Option<Id> {
        self.stack.last().map(|s| s.id)
    }

    pub fn is_open(&self) -> bool {
        !self.stack.is_empty()
    }

    /// True if the top screen is waiting for the key of a reassignment (physical code required).
    pub fn waiting_key(&self) -> bool {
        self.stack.last().is_some_and(|s| s.waiting.is_some())
    }

    /// Top screen.
    pub fn top_id(&self) -> Option<Id> {
        self.stack.last().map(|s| s.id)
    }

    pub fn close_all(&mut self) {
        self.stack.clear();
    }

    /// Opens a screen over the others; `keyboard`: opened by keyboard (initial focus).
    pub fn open(&mut self, id: Id, keyboard: bool) {
        let mut panel = Panel::default();
        panel.keyboard = keyboard;
        self.stack.push(Screen {
            id,
            panel,
            title: Vec::new(),
            size: (-1, -1),
            opts: Vec::new(),
            pending: Vec::new(),
            waiting: None,
            shown: Vec::new(),
            max_scale: 1,
        });
    }

    /// Layout if the size changed; focus and scroll kept.
    fn layout(&mut self, gui: &Gui, o: &Options) {
        let size = (gui.width as i32, gui.height as i32);
        let langs = &self.languages;
        let world = &mut self.world;
        let Some(s) = self.stack.last_mut() else { return };
        if s.size == size {
            return;
        }
        let first = s.size.0 < 0;
        let focus_id = s.panel.focus.and_then(|i| s.panel.widgets.get(i)).map(|w| w.id);
        let scroll = s.panel.list.as_ref().map(|l| l.scroll);
        let keyboard = s.panel.keyboard;
        let search = s.panel.get(SEARCH).and_then(|w| match &w.kind {
            Kind::Field(t) => Some(t.value()),
            _ => None,
        });
        s.size = size;
        s.max_scale = crate::gui::auto_scale(gui.width as u32 * gui.scale, gui.height as u32 * gui.scale);
        s.title = chars(&tr(&gui.lang, s.id.title(), &[]));
        build(s, gui, o, langs, world, search.as_deref().unwrap_or(""));
        if let Some(l) = &mut s.panel.list {
            l.inworld = !self.outside;
        }
        if let (Some(l), Some(sc)) = (&mut s.panel.list, scroll) {
            l.set_scroll(sc);
        }
        if first {
            s.panel.initial_focus(keyboard);
            if matches!(s.id, Id::Language | Id::GameRules | Id::Social) {
                s.panel.focus = s.panel.widgets.iter().position(|w| w.id == SEARCH);
            }
        } else {
            s.panel.keyboard = keyboard;
            s.panel.focus = focus_id.and_then(|id| s.panel.widgets.iter().position(|w| w.id == id));
        }
    }

    /// Closes the top screen: deferred sliders applied, options written.
    fn pop(&mut self, o: &mut Options, out: &mut Vec<Action>) {
        let Some(mut s) = self.stack.pop() else { return };
        let applied = apply_pending(&mut s, o, true);
        if applied {
            out.push(Action::Changed);
        }
        match s.id {
            Id::Pause => out.push(Action::Resume),
            Id::Language => {
                if let Some(sel) = s.panel.list.as_ref().and_then(|l| l.selected).and_then(|i| s.shown.get(i))
                    && let Some(l) = self.languages.get(*sel)
                    && l.code != o.s("lang")
                {
                    o.set("lang", Val::S(l.code.clone()));
                    out.push(Action::Language(l.code.clone()));
                }
                out.push(Action::Save);
            }
            _ => out.push(Action::Save),
        }
        // The screen below rebuilds its labels (values changed in the sub-screen).
        if let Some(top) = self.stack.last_mut() {
            top.size = (-2, -2);
        }
    }

    fn handle(&mut self, gui: &Gui, o: &mut Options, events: Vec<Event>) -> Vec<Action> {
        let mut out = Vec::new();
        for e in events {
            let Some(top) = self.stack.last_mut() else { break };
            let id = top.id;
            let mut next: Option<Next> = None;
            match (id, e) {
                (Id::Pause, Event::Press(RETURN)) => next = Some(Next::Clear),
                (Id::Pause, Event::Press(ADVANCEMENTS)) => out.push(Action::Advancements(top.panel.keyboard)),
                (Id::Pause, Event::Press(STATS)) => out.push(Action::Stats(top.panel.keyboard)),
                (Id::Pause, Event::Press(OPTIONS)) => next = Some(Next::Open(Id::Options, top.panel.keyboard)),
                (Id::Pause, Event::Press(WORLD_OPTIONS)) => {
                    self.world.start_world_options();
                    next = Some(Next::Open(Id::WorldOptions, top.panel.keyboard));
                }
                (Id::Pause, Event::Press(SOCIAL)) => next = Some(Next::Open(Id::Social, top.panel.keyboard)),
                (Id::Pause, Event::Press(b @ (REPORT_BUGS | FEEDBACK))) => {
                    self.world.url = (if b == REPORT_BUGS { BUGS_URL } else { FEEDBACK_URL }).to_owned();
                    next = Some(Next::Open(Id::Confirm(Confirm::Link), top.panel.keyboard));
                }
                (Id::Pause, Event::Press(PAUSE_EXTRA)) => out.push(Action::PauseExtra),
                (Id::Options, Event::Press(ONLINE)) => next = Some(Next::Open(Id::Online, top.panel.keyboard)),
                (Id::Language, Event::Press(FONT)) => next = Some(Next::Open(Id::Font, top.panel.keyboard)),
                (id, e) if crate::world_options::owns(id) => {
                    let search = top.panel.get(SEARCH).and_then(|w| match &w.kind {
                        Kind::Field(t) => Some(t.value()),
                        _ => None,
                    });
                    let step = crate::world_options::handle(
                        id,
                        &e,
                        gui,
                        o,
                        &mut self.world,
                        &mut top.panel,
                        search.as_deref().unwrap_or(""),
                        &mut out,
                    );
                    next = match step {
                        crate::world_options::Step::None => None,
                        crate::world_options::Step::Rebuild => {
                            top.size = (-2, -2);
                            None
                        }
                        crate::world_options::Step::Pop(n) => Some(Next::PopN(n)),
                        crate::world_options::Step::Open(target) => Some(Next::Open(target, top.panel.keyboard)),
                    };
                }
                (Id::Pause, Event::Press(DISCONNECT)) => {
                    if let Some(w) = top.panel.get_mut(DISCONNECT) {
                        w.active = false;
                    }
                    out.push(Action::Disconnect);
                }
                (_, Event::Press(DONE)) => next = Some(Next::Pop),
                (Id::Keybinds, Event::Press(RESET_ALL)) => {
                    for k in KEYS {
                        o.reset(&format!("key_{}", k.0));
                    }
                    top.size = (-2, -2);
                    out.push(Action::Save);
                }
                (Id::Keybinds, Event::Press(b)) if (BIND..BIND + KEYS.len() as u32).contains(&b) => {
                    top.waiting = Some((b - BIND) as usize);
                    top.size = (-2, -2);
                }
                (Id::Keybinds, Event::Press(b)) if (RESET..RESET + KEYS.len() as u32).contains(&b) => {
                    o.reset(&format!("key_{}", KEYS[(b - RESET) as usize].0));
                    top.size = (-2, -2);
                    out.push(Action::Save);
                }
                (Id::Language, Event::Edit(SEARCH)) => top.size = (-2, -2),
                (Id::Language, Event::Activate(_)) => next = Some(Next::Pop),
                (_, e) => {
                    let Some(opt) = event_opt(top, &e) else { continue };
                    match (opt, e) {
                        (Opt::Nav(target, _), Event::Press(_)) => next = Some(Next::Open(target, top.panel.keyboard)),
                        (Opt::Packs(_), Event::Press(_)) => out.push(Action::ResourcePacks),
                        (Opt::Link(_, url), Event::Press(_)) => {
                            self.world.url = url.to_owned();
                            next = Some(Next::Open(Id::Confirm(Confirm::Link), top.panel.keyboard));
                        }
                        (_, Event::Cycle(_, dir)) => {
                            cycle(o, &opt, dir, top.max_scale);
                            // Skin parts: written when the screen closes (§ 3.11).
                            if id != Id::Skin {
                                out.push(Action::Save);
                            }
                            out.push(Action::Changed);
                            top.size = (-2, -2);
                        }
                        (Opt::Slider(key, _), Event::Slide(_, v)) => {
                            set_slider(top, o, key, pos_value(key, v), &mut out);
                        }
                        (Opt::Slider(key, _), Event::Step(wid, dir)) => {
                            let k = slider_kind(key);
                            if let Some((lo, hi)) = int_range(k) {
                                let cur = match top.pending.iter().find(|p| p.0 == key) {
                                    Some((_, Val::I(i), _)) => *i,
                                    _ => current_int(key, o, k),
                                };
                                let n = (cur + dir).clamp(lo, hi);
                                set_slider(top, o, key, int_value(k, n), &mut out);
                                if let Some(w) = top.panel.get_mut(wid)
                                    && let Kind::Slider { value, .. } = &mut w.kind
                                {
                                    *value = int_pos(n, lo, hi);
                                }
                            }
                        }
                        (Opt::Slider(key, _), Event::SlideEnd(wid)) if !deferred(key) => {
                            // Handle re-aligned to the retained value.
                            let p = slider_pos(key, o);
                            if let Some(w) = top.panel.get_mut(wid)
                                && let Kind::Slider { value, .. } = &mut w.kind
                            {
                                *value = p;
                            }
                        }
                        _ => {}
                    }
                }
            }
            match next {
                Some(Next::Clear) => {
                    self.clicks += self.stack.last_mut().map_or(0, |s| std::mem::take(&mut s.panel.clicks));
                    self.stack.clear();
                    out.push(Action::Resume);
                }
                Some(Next::Open(target, kb)) => {
                    self.clicks += self.stack.last_mut().map_or(0, |s| std::mem::take(&mut s.panel.clicks));
                    self.open(target, kb);
                }
                Some(Next::Pop) => {
                    self.clicks += self.stack.last_mut().map_or(0, |s| std::mem::take(&mut s.panel.clicks));
                    self.pop(o, &mut out);
                }
                Some(Next::PopN(n)) => {
                    self.clicks += self.stack.last_mut().map_or(0, |s| std::mem::take(&mut s.panel.clicks));
                    for _ in 0..n {
                        self.pop(o, &mut out);
                    }
                }
                None => {}
            }
        }
        if let Some(s) = self.stack.last_mut() {
            self.clicks += std::mem::take(&mut s.panel.clicks);
            refresh_labels(s, gui, o);
        }
        out
    }

    /// Deferred sliders due (600 ms); to call every tick.
    pub fn tick(&mut self, o: &mut Options) -> Vec<Action> {
        let Some(s) = self.stack.last_mut() else { return Vec::new() };
        if apply_pending(s, o, false) { vec![Action::Changed] } else { Vec::new() }
    }

    pub fn motion(&mut self, gui: &Gui, o: &mut Options, m: (f32, f32)) -> Vec<Action> {
        if self.roll.is_some() {
            return Vec::new();
        }
        self.layout(gui, o);
        let mut ev = Vec::new();
        if let Some(s) = self.stack.last_mut() {
            s.panel.motion(m, &mut ev);
        }
        self.handle(gui, o, ev)
    }

    pub fn press(&mut self, gui: &Gui, o: &mut Options, m: (f32, f32), button: u8, shift: bool) -> Vec<Action> {
        if self.roll.is_some() {
            return Vec::new();
        }
        self.layout(gui, o);
        // Reassignment pending: the mouse button becomes the key.
        if let Some(s) = self.stack.last_mut()
            && let Some(k) = s.waiting.take()
        {
            o.set(&format!("key_{}", KEYS[k].0), Val::S(options::mouse_name(button)));
            s.size = (-2, -2);
            return vec![Action::Save];
        }
        let mut ev = Vec::new();
        if let Some(s) = self.stack.last_mut() {
            s.panel.press(m, button, shift, &mut ev);
        }
        self.handle(gui, o, ev)
    }

    pub fn release(&mut self, gui: &Gui, o: &mut Options) -> Vec<Action> {
        self.layout(gui, o);
        let mut ev = Vec::new();
        if let Some(s) = self.stack.last_mut() {
            s.panel.release(&mut ev);
        }
        self.handle(gui, o, ev)
    }

    pub fn wheel(&mut self, gui: &Gui, o: &mut Options, m: (f32, f32), dy: f64) -> Vec<Action> {
        self.layout(gui, o);
        let mut ev = Vec::new();
        if let Some(s) = self.stack.last_mut() {
            s.panel.wheel(m, dy, &mut ev);
        }
        self.handle(gui, o, ev)
    }

    /// Key released: only the credits scroll uses it (acceleration, rewind).
    pub fn key_release(&mut self, k: KeyCode) {
        if let Some(r) = &mut self.roll {
            r.key(k, false);
        }
    }

    /// Key pressed: pending reassignment, components, then Escape (previous screen;
    /// pause menu: back to the game).
    pub fn key(&mut self, gui: &Gui, o: &mut Options, k: KeyCode, mods: ModifiersState) -> Vec<Action> {
        if let Some(r) = &mut self.roll {
            if r.key(k, true) {
                self.roll = None;
            }
            return Vec::new();
        }
        self.layout(gui, o);
        if let Some(s) = self.stack.last_mut()
            && let Some(i) = s.waiting.take()
        {
            let name = if k == KeyCode::Escape { "key.keyboard.unknown" } else { options::key_name(k) };
            o.set(&format!("key_{}", KEYS[i].0), Val::S(name.to_owned()));
            s.size = (-2, -2);
            return vec![Action::Save];
        }
        let mut ev = Vec::new();
        let used = match self.stack.last_mut() {
            Some(s) => s.panel.key(k, mods, &mut ev),
            None => return Vec::new(),
        };
        let mut out = self.handle(gui, o, ev);
        if !used && k == KeyCode::Escape {
            match self.stack.last().map(|s| s.id) {
                Some(id) if crate::world_options::owns(id) => match crate::world_options::escape(id, &mut self.world) {
                    crate::world_options::Step::Open(target) => self.open(target, true),
                    crate::world_options::Step::Pop(n) => {
                        for _ in 0..n {
                            self.pop(o, &mut out);
                        }
                    }
                    _ => {}
                },
                _ => self.pop(o, &mut out),
            }
        }
        out
    }

    pub fn typed(&mut self, gui: &Gui, o: &mut Options, s: &str) -> Vec<Action> {
        self.layout(gui, o);
        let mut ev = Vec::new();
        if let Some(sc) = self.stack.last_mut() {
            sc.panel.typed(s, &mut ev);
        }
        self.handle(gui, o, ev)
    }

    /// Drawing of the top screen (§ 1.12): menu background, title, components, tooltip.
    pub fn draw(&mut self, gui: &mut Gui, q: &wgpu::Queue, o: &Options, mouse: Option<(f32, f32)>, time: f64) {
        if let Some(r) = &mut self.roll {
            r.draw(gui, q, o.i("menuBackgroundBlurriness"));
            if r.finished(gui.height) {
                self.roll = None;
            }
            return;
        }
        self.layout(gui, o);
        let n = self.stack.len();
        let popup = n >= 2 && self.stack.last().is_some_and(|s| crate::world_options::is_popup(s.id));
        gui.next_layer();
        widgets::menu_background(gui, o.i("menuBackgroundBlurriness"), !self.outside);
        gui.next_layer();
        if popup {
            // Pop-up window: screen below frozen, veil, then the window.
            let below = &mut self.stack[n - 2];
            widgets::centered(gui, q, &below.title, gui.width as i32 / 2, 12, 0xffff_ffff);
            below.panel.draw(gui, q, None, time);
            gui.next_layer();
            widgets::ingame_background(gui);
            gui.next_layer();
        }
        let Some(s) = self.stack.last_mut() else { return };
        let (w, h) = (gui.width as i32, gui.height as i32);
        // Video screen: red "restart required" banner under the title, the two lines
        // (9 + 2 + 9) centred in the 33 header.
        let restart = s.id == Id::Video && restart_required(o);
        let title_y = match s.id {
            Id::Pause => 40,
            Id::Language | Id::GameRules => 4,
            _ if restart => 6,
            _ => 12,
        };
        widgets::centered(gui, q, &s.title, w / 2, title_y, 0xffff_ffff);
        if restart {
            let warn = chars(&tr(&gui.lang, "options.restartRequired", &[]));
            widgets::centered(gui, q, &warn, w / 2, title_y + 11, 0xffdf_5050);
        }
        if s.id == Id::Language {
            let warn = chars(&tr(&gui.lang, "options.languageAccuracyWarning", &[]));
            widgets::centered(gui, q, &warn, w / 2, h - 45, 0xffba_baba);
        }
        // Conflicting keys: yellow bar to the left of the button (§ 3.10).
        if s.id == Id::Keybinds
            && let Some(l) = &s.panel.list
        {
            let clip = [l.x as f32, l.y as f32, (l.x + l.w) as f32, (l.y + l.h) as f32];
            let bars: Vec<(i32, i32)> = s
                .panel
                .widgets
                .iter()
                .filter(|wd| (BIND..RESET).contains(&wd.id) && !wd.tooltip.is_empty())
                .map(|wd| {
                    let (x, y, _, hh) = s.panel.rect(wd);
                    (x - 6, y + hh / 2)
                })
                .collect();
            gui.set_clip(Some(clip));
            for (x, mid) in bars {
                gui.fill(x as f32, (mid - 9) as f32, (x + 3) as f32, (mid + 9) as f32, 0xffff_ff00);
            }
            gui.set_clip(None);
        }
        crate::world_options::draw(s.id, gui, q, &self.world, &s.panel, time);
        if s.id == Id::Social {
            let world = &self.world;
            s.panel
                .draw_with(gui, q, mouse, time, &mut |g, q, l| crate::world_options::draw_social_rows(g, q, world, l));
        } else {
            s.panel.draw(gui, q, mouse, time);
        }
    }
}

/// Stack change requested by an event.
enum Next {
    Clear,
    Open(Id, bool),
    Pop,
    PopN(usize),
}

/// Setting associated with a component event.
fn event_opt(s: &Screen, e: &Event) -> Option<Opt> {
    let id = match e {
        Event::Press(i) | Event::Cycle(i, _) | Event::Slide(i, _) | Event::SlideEnd(i) | Event::Step(i, _) => *i,
        _ => return None,
    };
    id.checked_sub(OPT_BASE).and_then(|i| s.opts.get(i as usize)).copied()
}

/// Next / previous value of a cyclic button (§ 1.4).
fn cycle(o: &mut Options, opt: &Opt, dir: i32, max_scale: u32) {
    let step = |n: usize, cur: usize| ((cur as i32 + dir).rem_euclid(n as i32)) as usize;
    match *opt {
        Opt::Bool(key, _) => {
            let v = !o.b(key);
            set_quality(o, key, Val::B(v));
        }
        Opt::Choice(key, _, values) => {
            let ty = o.ty(key);
            let cur = match ty {
                Some(Ty::Bool) => o.b(key).to_string(),
                Some(Ty::Int(..)) => o.i(key).to_string(),
                _ => o.s(key).to_owned(),
            };
            let i = values.iter().position(|v| v.0 == cur).unwrap_or(0);
            let next = values[step(values.len(), i)].0;
            let v = match ty {
                Some(Ty::Bool) => Val::B(next == "true"),
                Some(Ty::Int(..)) => Val::I(next.parse().unwrap_or(0)),
                _ => Val::S(next.to_owned()),
            };
            set_quality(o, key, v);
        }
        Opt::GuiScale => {
            // Automatic (0), then 1..maximum.
            let n = max_scale as usize + 1;
            let cur = (o.i("guiScale").max(0) as usize).min(max_scale as usize);
            o.set("guiScale", Val::I(step(n, cur) as i32));
        }
        Opt::Device => {
            // Default, then the devices present (listed at the click, like the game).
            let mut list = vec![String::new()];
            list.extend(rc_audio::output_devices());
            let cur = list.iter().position(|d| d == o.s("soundDevice")).unwrap_or(0);
            let next = list.swap_remove(step(list.len(), cur));
            o.set("soundDevice", Val::S(next));
        }
        _ => {}
    }
}

/// Assignment; a "quality" setting switches the preset to Custom, the
/// preset itself applies (§ 3.1, 3.5).
fn set_quality(o: &mut Options, key: &str, v: Val) {
    let changed = o.set(key, v);
    if key == "graphicsPreset" {
        o.apply_preset();
    } else if changed && options::QUALITY.contains(&key) {
        o.set("graphicsPreset", Val::S("custom".into()));
    }
}

/// New value of a slider: immediate, or deferred (renderDistance…).
fn set_slider(s: &mut Screen, o: &mut Options, key: &'static str, v: Val, out: &mut Vec<Action>) {
    if deferred(key) {
        s.pending.retain(|p| p.0 != key);
        s.pending.push((key, v, Instant::now()));
    } else {
        let before = o.f(key);
        set_quality(o, key, v);
        out.push(Action::Changed);
        // Volume of a category changed: sound preview (played only outside the world).
        if let Some(c) = key.strip_prefix("soundCategory_").and_then(rc_audio::Category::from_name)
            && o.f(key) != before
        {
            out.push(Action::SoundPreview(c, o.f(key) as f32));
        }
        if key == "graphicsPreset" {
            // All the quality sliders move with the preset.
            s.size = (-2, -2);
        }
    }
}

/// Deferred sliders due (or all if `all`); returns true if a value was applied.
fn apply_pending(s: &mut Screen, o: &mut Options, all: bool) -> bool {
    let mut applied = false;
    let mut keep = Vec::new();
    for (k, v, t) in std::mem::take(&mut s.pending) {
        if all || t.elapsed().as_millis() >= 600 {
            set_quality(o, k, v);
            applied = true;
        } else {
            keep.push((k, v, t));
        }
    }
    s.pending = keep;
    applied
}

/// Labels recomputed (values changed, deferred sliders pending).
fn refresh_labels(s: &mut Screen, gui: &Gui, o: &Options) {
    for w in &mut s.panel.widgets {
        let Some(opt) = w.id.checked_sub(OPT_BASE).and_then(|i| s.opts.get(i as usize)) else { continue };
        if w.id >= BIND {
            continue;
        }
        let pending = match opt {
            Opt::Slider(k, _) => s.pending.iter().find(|p| p.0 == *k).map(|p| &p.1),
            _ => None,
        };
        w.msg = chars(&opt_label(&gui.lang, o, opt, pending, s.max_scale));
        if matches!(opt, Opt::Choice(GRAPHICS_API, ..)) {
            w.set_tooltip(gui, &graphics_api_tooltip(gui, o));
        }
    }
}

/// Component of a setting.
#[allow(clippy::too_many_arguments)]
fn opt_widget(gui: &Gui, o: &Options, opt: &Opt, id: u32, x: i32, y: i32, w: i32, max_scale: u32) -> Widget {
    let msg = chars(&opt_label(&gui.lang, o, opt, None, max_scale));
    let mut wd = match *opt {
        Opt::Slider(key, _) => Widget::new(
            id,
            Kind::Slider { value: slider_pos(key, o), steps: slider_steps(key), adjusting: false },
            x,
            y,
            w,
            20,
            msg,
        ),
        Opt::Nav(..) | Opt::Packs(..) | Opt::Link(..) => Widget::new(id, Kind::Button, x, y, w, 20, msg),
        Opt::Off(_) | Opt::OffValue(..) => Widget::new(id, Kind::Button, x, y, w, 20, msg).inactive(),
        _ => Widget::new(id, Kind::Cycle, x, y, w, 20, msg),
    };
    // Tooltips specific to certain settings (`<key>.tooltip`).
    let tip = match *opt {
        // `….button` (new Realms): tooltip without this suffix.
        Opt::Bool(_, name) | Opt::Choice(_, name, _) | Opt::Slider(_, name) => {
            format!("{}.tooltip", name.strip_suffix(".button").unwrap_or(name))
        }
        _ => String::new(),
    };
    if matches!(*opt, Opt::Choice(GRAPHICS_API, ..)) {
        wd.set_tooltip(gui, &graphics_api_tooltip(gui, o));
    } else if !tip.is_empty() && gui.lang.map.contains_key(&tip) {
        wd.set_tooltip(gui, &tr(&gui.lang, &tip, &[]));
    }
    wd
}

/// Graphics API setting, applied at the next startup (`gpu_backend`).
const GRAPHICS_API: &str = "preferredGraphicsBackend";

/// A value applied only at startup has changed since launch (red banner of
/// the video screen).
fn restart_required(o: &Options) -> bool {
    o.s(GRAPHICS_API) != crate::gpu_backend::startup_preference()
}

/// Graphics API tooltip: "restart required" if the value differs from that of the
/// startup, the general text, then the Vulkan warning when it is chosen (empty
/// lines between the blocks, like the game).
fn graphics_api_tooltip(gui: &Gui, o: &Options) -> String {
    let t = |k: &str| tr(&gui.lang, k, &[]);
    let mut lines = Vec::new();
    if restart_required(o) {
        lines.push(t("options.needsRestart"));
        lines.push(String::new());
    }
    lines.push(t("options.graphicsApi.tooltip"));
    if o.s(GRAPHICS_API) == "vulkan" {
        lines.push(String::new());
        lines.push(t("options.graphicsApi.tooltip.vulkan"));
    }
    lines.join("\n")
}

/// Components of a screen for a `W × H` interface.
fn build(
    s: &mut Screen,
    gui: &Gui,
    o: &Options,
    langs: &[Language],
    world: &mut crate::world_options::World,
    search: &str,
) {
    let (w, h) = (gui.width as i32, gui.height as i32);
    let t = |k: &str| tr(&gui.lang, k, &[]);
    s.opts.clear();
    let mut widgets = Vec::new();
    let mut list = None;
    let done = |x: i32, wd: i32| Widget::button(DONE, x, h - 26, wd, &t("gui.done"));
    match s.id {
        Id::Pause => {
            // Grid of 2 columns of 106 (§ 2.3), 212 × 166 (+ 24 with the server button),
            // placed at a quarter of the height.
            let extra = if world.pause_extra.is_some() { 24 } else { 0 };
            let gx = ((w - 212) as f32 * 0.5) as i32;
            let gy = ((h - 166 - extra) as f32 * 0.25) as i32;
            let b = |id, x, y, bw, k: &str| Widget::button(id, gx + x, gy + y, bw, &t(k));
            let icon = |id, x, sprite: &'static str, k: &str| {
                // Tooltip message, like the pause icons of the game.
                Widget::new(id, Kind::Icon(sprite), gx + x, gy + 98, 20, 20, chars(&t(k))).with_tooltip(gui, &t(k))
            };
            widgets = vec![
                b(RETURN, 4, 50, 204, "menu.returnToGame"),
                // Advancements and statistics: screens of `progress_app.rs` (parent = pause).
                b(ADVANCEMENTS, 4, 74, 98, "gui.advancements"),
                b(STATS, 110, 74, 98, "gui.stats"),
                // Icons: report a bug and give feedback (confirmed link), friends list of the
                // Microsoft account (active outside demo and outside offline developer mode, like
                // the game), social interactions.
                icon(REPORT_BUGS, 60, "pause_menu/bug", "menu.reportBugs"),
                icon(FEEDBACK, 84, "pause_menu/social_interactions", "menu.sendFeedback"),
                icon(6, 108, "friends/friends", "options.friendsList"),
            ];
            let mut social = icon(SOCIAL, 132, "pause_menu/player_reporting", "menu.playerReporting");
            if world.ctx.solo && world.players.iter().all(|p| p.uuid == world.local) {
                social.active = false;
                social.set_tooltip(gui, &t("menu.playerReporting.no_players"));
            }
            widgets.push(social);
            // Button added by the server (dialog of the pause or server links).
            if let Some(label) = &world.pause_extra {
                widgets.push(
                    Widget::new(PAUSE_EXTRA, Kind::Button, gx + 4, gy + 122, 204, 20, label.clone())
                        .with_tooltip(gui, &t("menu.custom_options.tooltip")),
                );
            }
            widgets.push(b(OPTIONS, 4, 122 + extra, 98, "menu.options"));
            widgets.push(b(WORLD_OPTIONS, 110, 122 + extra, 98, "options.worldOptions.button"));
            // Singleplayer: "Save and quit to title".
            widgets.push(b(
                DISCONNECT,
                4,
                146 + extra,
                204,
                if crate::world_list::solo_open() { "menu.returnToMenu" } else { "menu.disconnect" },
            ));
        }
        id if crate::world_options::owns(id) => {
            (widgets, list) = crate::world_options::build(id, gui, o, world, search);
        }
        Id::Options => {
            // Header of 61: field of view and "Online…" (§ 3.2); grid 2 × 5.
            let hx = (w - 308) / 2;
            s.opts.push(Opt::Slider("fov", "options.fov"));
            widgets.push(opt_widget(gui, o, &s.opts[0], OPT_BASE, hx, 29, 150, s.max_scale));
            widgets.push(Widget::button(ONLINE, hx + 158, 29, 150, &t("options.online")));
            let grid = [
                Opt::Nav(Id::Skin, "options.skinCustomisation"),
                Opt::Nav(Id::Sounds, "options.sounds"),
                Opt::Nav(Id::Video, "options.video"),
                Opt::Nav(Id::Controls, "options.controls"),
                Opt::Nav(Id::Language, "options.language"),
                Opt::Nav(Id::Chat, "options.chat"),
                Opt::Packs("options.resourcepack"),
                Opt::Nav(Id::Accessibility, "options.accessibility"),
                // Rustcraft sends no telemetry: inactive button, like the game when
                // telemetry is not permitted.
                Opt::Off("options.telemetry"),
                Opt::Nav(Id::Credits, "options.credits_and_attribution"),
            ];
            let gx = (w - 316) / 2;
            let gy = 91.min(h - 153);
            for (i, opt) in grid.iter().enumerate() {
                let (r, c) = (i as i32 / 2, i as i32 % 2);
                s.opts.push(*opt);
                let id = OPT_BASE + s.opts.len() as u32 - 1;
                let mut wd = opt_widget(gui, o, opt, id, gx + 158 * c + 4, gy + 24 * r, 150, s.max_scale);
                if *opt == Opt::Off("options.telemetry") {
                    wd.set_tooltip(gui, &t("options.telemetry.disabled"));
                }
                widgets.push(wd);
            }
            widgets.push(done((w - 200) / 2, 200));
        }
        Id::Keybinds => {
            // List with row width 340, entries of 20 (§ 3.10), sorted by category then order and name.
            let mut l = List::new(0, 33, w, h - 66, 340, 20);
            let mut order: Vec<usize> = (0..KEYS.len()).collect();
            let name = |i: usize| t(KEYS[i].0);
            order.sort_by(|&a, &b| (KEYS[a].2, KEYS[a].3, name(a)).cmp(&(KEYS[b].2, KEYS[b].3, name(b))));
            let left = l.row_left();
            let bar_x = left + 340 + 8;
            let (rx, bx) = (bar_x - 50 - 10, bar_x - 50 - 10 - 5 - 75);
            let mut top = 0;
            let mut cat: Option<Cat> = None;
            let bound = |i: usize| o.s(&format!("key_{}", KEYS[i].0)).to_owned();
            for &i in &order {
                if cat != Some(KEYS[i].2) {
                    cat = Some(KEYS[i].2);
                    let label = chars(&format!("§l§n{}", t(KEYS[i].2.key())));
                    let mut lw = Widget::new(
                        0,
                        Kind::Label { color: 0xffff_ffff, centered: true },
                        0,
                        2 + top + 10,
                        w,
                        9,
                        label,
                    );
                    lw.in_list = true;
                    widgets.push(lw);
                    top += 20;
                }
                let key = bound(i);
                let mut nl = Widget::new(
                    0,
                    Kind::Label { color: 0xffff_ffff, centered: false },
                    left + 2,
                    2 + top + 8,
                    150,
                    9,
                    chars(&name(i)),
                );
                nl.in_list = true;
                widgets.push(nl);
                // Conflicts: same key as another assignment, unless both are default.
                let others: Vec<String> = (0..KEYS.len())
                    .filter(|&j| {
                        j != i
                            && key != "key.keyboard.unknown"
                            && bound(j) == key
                            && !(o.is_default(&format!("key_{}", KEYS[i].0))
                                && o.is_default(&format!("key_{}", KEYS[j].0)))
                    })
                    .map(name)
                    .collect();
                let label = options::key_label(&gui.lang, &key);
                let msg = if s.waiting == Some(i) {
                    format!("§e> §f§n{label}§r§e <")
                } else if !others.is_empty() {
                    format!("§e[ §f{label}§e ]")
                } else {
                    label
                };
                let mut b = Widget::new(BIND + i as u32, Kind::Button, bx, 2 + top, 75, 20, chars(&msg));
                if !others.is_empty() {
                    b.set_tooltip(gui, &tr(&gui.lang, "controls.keybinds.duplicateKeybinds", &[others.join("\n")]));
                }
                b.in_list = true;
                widgets.push(b);
                let mut r = Widget::button(RESET + i as u32, rx, 2 + top, 50, &t("controls.reset"));
                r.active = !o.is_default(&format!("key_{}", KEYS[i].0));
                r.in_list = true;
                widgets.push(r);
                top += 20;
            }
            l.content = top + 4;
            list = Some(l);
            let all_default = KEYS.iter().all(|k| o.is_default(&format!("key_{}", k.0)));
            let x0 = (w - 308) / 2;
            let mut ra = Widget::button(RESET_ALL, x0, h - 26, 150, &t("controls.resetAll"));
            ra.active = !all_default;
            widgets.push(ra);
            widgets.push(done(x0 + 158, 150));
        }
        Id::Language => {
            // Header of 36 (title, search 200 × 15), footer of 53 (§ 3.11).
            let mut field = TextField::new(chars(""));
            field.text = search.chars().collect();
            field.cursor = field.text.len();
            field.anchor = field.cursor;
            widgets.push(Widget::new(SEARCH, Kind::Field(Box::new(field)), w / 2 - 100, 17, 200, 15, Vec::new()));
            let mut l = List::new(0, 36, w, h - 36 - 53, 270, 18);
            let needle = search.to_lowercase();
            s.shown = (0..langs.len())
                .filter(|&i| {
                    needle.is_empty()
                        || langs[i].name.to_lowercase().contains(&needle)
                        || langs[i].region.to_lowercase().contains(&needle)
                })
                .collect();
            let mut top = 0;
            for &i in &s.shown {
                let lg = &langs[i];
                l.entries.push(Entry { top, h: 18, label: chars(&format!("{} ({})", lg.name, lg.region)) });
                top += 18;
            }
            l.content = top + 4;
            l.selected = s.shown.iter().position(|&i| langs[i].code == o.s("lang"));
            if let Some(sel) = l.selected {
                // Current language centred when opened.
                l.set_scroll(f64::from(2 + 18 * sel as i32 + 9 - l.h / 2));
            }
            list = Some(l);
            let x0 = (w - 308) / 2;
            widgets.push(Widget::button(FONT, x0, h - 28, 150, &t("options.font")));
            let mut d = done(x0 + 158, 150);
            d.y = h - 28;
            widgets.push(d);
        }
        id => {
            // Generic sub-screen (§ 3.3): list with row width 310, entries of 25.
            let mut l = List::new(0, 33, w, h - 66, 310, 25);
            let mut top = 0;
            let (x0, x1) = (w / 2 - 155, w / 2 + 5);
            let mut first = true;
            for row in rows(id, o) {
                let mut add = |opt: Opt, x: i32, y: i32, ww: i32, s: &mut Screen| {
                    s.opts.push(opt);
                    let wid = OPT_BASE + s.opts.len() as u32 - 1;
                    let mut wd = opt_widget(gui, o, &opt, wid, x, y, ww, s.max_scale);
                    wd.in_list = true;
                    widgets.push(wd);
                };
                match row {
                    Row::Header(k) => {
                        let margin = if first { 0 } else { 18 };
                        let mut tw = Widget::new(0, Kind::Title, x0, 2 + top + margin, 310, 17, chars(&t(k)));
                        tw.in_list = true;
                        widgets.push(tw);
                        top += margin + 18;
                    }
                    Row::Big(opt) => {
                        add(opt, x0, 2 + top + 2, 310, s);
                        top += 25;
                    }
                    Row::Pair(a, b) => {
                        add(a, x0, 2 + top + 2, 150, s);
                        if let Some(b) = b {
                            add(b, x1, 2 + top + 2, 150, s);
                        }
                        top += 25;
                    }
                }
                first = false;
            }
            l.content = top + 4;
            list = Some(l);
            if id == Id::Accessibility {
                let x = (w - 308) / 2;
                widgets.push(Widget::button(62, x, h - 26, 150, &t("options.accessibility.link")).inactive());
                widgets.push(done(x + 158, 150));
            } else {
                widgets.push(done((w - 200) / 2, 200));
            }
        }
    }
    s.panel.widgets = widgets;
    s.panel.list = list;
    s.panel.focus = None;
    refresh_labels(s, gui, o);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slider_int_mapping_round_trips() {
        for n in 2..=32 {
            let p = int_pos(n, 2, 32);
            assert_eq!(pos_int(p, 2, 32), n, "{n}");
        }
        assert_eq!(pos_int(1.0, 2, 32), 32);
        assert_eq!(pos_int(0.0, 2, 32), 2);
    }

    #[test]
    fn slider_values() {
        let o = Options::default();
        assert_eq!(pos_value("chatDelay", int_pos(3, 0, 60)), Val::F(0.3));
        assert_eq!(pos_value("maxFps", 1.0), Val::I(260));
        assert_eq!(pos_value("fov", 0.0), Val::F(-1.0));
        assert_eq!(pos_value("graphicsPreset", 0.5), Val::S("fabulous".into()));
        assert!((slider_pos("mouseWheelSensitivity", &o) - 2.0 / 3.0).abs() < 1e-9);
        assert_eq!(slider_pos("gamma", &o), 0.5);
    }

    #[test]
    fn quality_change_switches_preset_to_custom() {
        let mut o = Options::default();
        set_quality(&mut o, "renderDistance", Val::I(20));
        assert_eq!(o.s("graphicsPreset"), "custom");
        set_quality(&mut o, "graphicsPreset", Val::S("fast".into()));
        assert_eq!((o.s("graphicsPreset"), o.i("renderDistance")), ("fast", 8));
        cycle(&mut o, &Opt::Choice("particles", "", &[("0", ""), ("1", ""), ("2", "")]), 1, 3);
        assert_eq!((o.i("particles"), o.s("graphicsPreset")), (2, "custom"));
    }

    #[test]
    fn language_list_from_pack_metadata() {
        let l = languages(br#"{"language":{"fr_fr":{"name":"Fran\u00e7ais","region":"France"},"en_us":{"name":"English","region":"US"}}}"#);
        assert_eq!(l.len(), 2);
        assert_eq!((l[1].code.as_str(), l[1].name.as_str()), ("fr_fr", "Français"));
    }
}
