//! Player settings and the `options.txt` file (`docs/specs/option-menus.md` § 3–4): table
//! of the options in the game's write order, tolerant reading, exact writing (data version,
//! JSON of typed values, raw fields), graphics presets, client information.

use hashbrown::HashMap;
use std::path::Path;
use std::sync::OnceLock;

use rc_proto_v777::play::ClientInformation;
use winit::keyboard::KeyCode;

/// Data version written at the head of the file (26.3).
pub const DATA_VERSION: i32 = 5023;

/// Type of an option and its form in the file.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Ty {
    /// JSON boolean.
    Bool,
    /// JSON integer in `[min, max]`.
    Int(i32, i32),
    /// JSON real in `[min, max]` (unit ones: `true` / `false` accepted on read).
    Real(f64, f64),
    /// JSON string among a list.
    Enum(&'static [&'static str]),
    /// Free JSON string.
    Str,
    /// Clouds: `"true"`, `"fast"`, `"false"` (JSON booleans accepted on read).
    Clouds,
    /// Raw fields: text, boolean (only an exact `true` is true), integer, JSON list.
    RawStr,
    RawBool,
    RawInt,
    RawList,
}

/// Value of an option.
#[derive(Debug, Clone, PartialEq)]
pub enum Val {
    B(bool),
    I(i32),
    F(f64),
    S(String),
    L(Vec<String>),
}

/// Constant default of an option.
#[derive(Debug, Clone, Copy)]
enum D {
    B(bool),
    I(i32),
    F(f64),
    S(&'static str),
    L,
}

impl D {
    fn val(self) -> Val {
        match self {
            D::B(b) => Val::B(b),
            D::I(i) => Val::I(i),
            D::F(f) => Val::F(f),
            D::S(s) => Val::S(s.to_owned()),
            D::L => Val::L(Vec::new()),
        }
    }
}

const UNIT: Ty = Ty::Real(0.0, 1.0);
const PRESETS: &[&str] = &["fast", "fancy", "fabulous", "custom"];

use D::{B, F, I, S};
use Ty::{Bool, Enum, Int, Real};

/// Typed then raw options, in write order (§ 4.3, groups 1 and 2).
const DEFS: &[(&str, Ty, D)] = &[
    ("ao", Bool, B(true)),
    ("biomeBlendRadius", Int(0, 7), I(2)),
    ("chunkSectionFadeInTime", Real(0.0, 2.0), F(0.75)),
    ("cutoutLeaves", Bool, B(true)),
    ("enableVsync", Bool, B(true)),
    ("entityDistanceScaling", Real(0.5, 5.0), F(1.0)),
    ("entityShadows", Bool, B(true)),
    ("forceUnicodeFont", Bool, B(false)),
    ("japaneseGlyphVariants", Bool, B(false)),
    ("fov", Real(-1.0, 1.0), F(0.0)),
    ("fovEffectScale", UNIT, F(1.0)),
    ("darknessEffectScale", UNIT, F(1.0)),
    ("glintSpeed", UNIT, F(0.5)),
    ("glintStrength", UNIT, F(0.75)),
    ("preferredGraphicsBackend", Enum(&["default", "opengl", "vulkan"]), S("default")),
    ("graphicsPreset", Enum(PRESETS), S("fancy")),
    ("prioritizeChunkUpdates", Int(0, 2), I(0)),
    ("fullscreen", Bool, B(false)),
    ("exclusiveFullscreen", Bool, B(false)),
    ("macFullscreenMenuVisibility", Bool, B(false)),
    ("gamma", UNIT, F(0.5)),
    ("guiScale", Int(0, i32::MAX - 1), I(0)),
    ("debugGuiScale", Int(-1, i32::MAX - 1), I(0)),
    ("maxAnisotropyBit", Int(1, 3), I(2)),
    ("textureFiltering", Int(0, 2), I(0)),
    ("maxFps", Int(10, 260), I(120)),
    ("improvedTransparency", Bool, B(false)),
    ("inactivityFpsLimit", Enum(&["minimized", "afk"]), S("afk")),
    ("mipmapLevels", Int(0, 4), I(4)),
    ("narrator", Int(0, 3), I(0)),
    ("particles", Int(0, 2), I(0)),
    ("reducedDebugInfo", Bool, B(false)),
    ("renderClouds", Ty::Clouds, S("true")),
    ("cloudRange", Int(2, 128), I(128)),
    ("renderDistance", Int(2, 32), I(12)),
    ("simulationDistance", Int(5, 32), I(12)),
    ("screenEffectScale", UNIT, F(1.0)),
    ("soundDevice", Ty::Str, S("")),
    ("vignette", Bool, B(true)),
    ("weatherRadius", Int(3, 10), I(10)),
    ("autoJump", Bool, B(false)),
    ("rotateWithMinecart", Bool, B(false)),
    ("operatorItemsTab", Bool, B(false)),
    ("autoSuggestions", Bool, B(true)),
    ("chatColors", Bool, B(true)),
    ("chatLinks", Bool, B(true)),
    ("chatLinksPrompt", Bool, B(true)),
    ("discrete_mouse_scroll", Bool, B(false)),
    ("invertXMouse", Bool, B(false)),
    ("invertYMouse", Bool, B(false)),
    ("realmsNotifications", Bool, B(true)),
    ("showSubtitles", Bool, B(false)),
    ("directionalAudio", Bool, B(false)),
    ("bobView", Bool, B(true)),
    ("toggleCrouch", Bool, B(false)),
    ("toggleSprint", Bool, B(false)),
    ("toggleAttack", Bool, B(false)),
    ("toggleUse", Bool, B(false)),
    ("sprintWindow", Int(0, 10), I(7)),
    ("darkMojangStudiosBackground", Bool, B(false)),
    ("hideLightningFlashes", Bool, B(false)),
    ("hideSplashTexts", Bool, B(false)),
    ("mouseSensitivity", UNIT, F(0.5)),
    ("damageTiltStrength", UNIT, F(1.0)),
    ("highContrast", Bool, B(false)),
    ("highContrastBlockOutline", Bool, B(false)),
    ("narratorHotkey", Bool, B(true)),
    ("resourcePacks", Ty::RawList, D::L),
    ("incompatibleResourcePacks", Ty::RawList, D::L),
    ("lastServer", Ty::RawStr, S("")),
    ("lang", Ty::RawStr, S("en_us")),
    ("chatVisibility", Int(0, 2), I(0)),
    ("chatOpacity", UNIT, F(1.0)),
    ("chatLineSpacing", UNIT, F(0.0)),
    ("textBackgroundOpacity", UNIT, F(0.5)),
    ("backgroundForChatOnly", Bool, B(true)),
    ("hideServerAddress", Ty::RawBool, B(false)),
    ("advancedItemTooltips", Ty::RawBool, B(false)),
    ("pauseOnLostFocus", Ty::RawBool, B(true)),
    ("overrideWidth", Ty::RawInt, I(0)),
    ("overrideHeight", Ty::RawInt, I(0)),
    ("chatHeightFocused", UNIT, F(1.0)),
    ("chatDelay", Real(0.0, 6.0), F(0.0)),
    ("chatHeightUnfocused", UNIT, F(0.4375)),
    ("chatScale", UNIT, F(1.0)),
    ("chatWidth", UNIT, F(1.0)),
    ("notificationDisplayTime", Real(0.5, 10.0), F(1.0)),
    ("useNativeTransport", Ty::RawBool, B(true)),
    ("mainHand", Enum(&["left", "right"]), S("right")),
    ("attackIndicator", Int(0, 2), I(1)),
    ("tutorialStep", Ty::RawStr, S("movement")),
    ("mouseWheelSensitivity", Real(0.01, 10.0), F(1.0)),
    ("allowCursorChanges", Bool, B(true)),
    ("quitShortcuts", Bool, B(true)),
    ("ctrlClickEmulatesRightClick", Bool, B(false)),
    ("glDebugVerbosity", Ty::RawInt, I(1)),
    ("skipMultiplayerWarning", Ty::RawBool, B(false)),
    ("hideMatchedNames", Bool, B(true)),
    ("joinedFirstServer", Ty::RawBool, B(false)),
    ("syncChunkWrites", Ty::RawBool, B(true)),
    ("showAutosaveIndicator", Bool, B(true)),
    ("allowServerListing", Bool, B(true)),
    ("inGameNotification", Bool, B(false)),
    ("sharePresence", Enum(&["none", "limited", "all"]), S("all")),
    ("onlyShowSecureChat", Bool, B(false)),
    ("saveChatDrafts", Bool, B(false)),
    ("panoramaScrollSpeed", UNIT, F(1.0)),
    ("telemetryOptInExtra", Ty::RawBool, B(false)),
    ("onboardAccessibility", Ty::RawBool, B(true)),
    ("menuBackgroundBlurriness", Int(0, 10), I(5)),
    ("startedCleanly", Ty::RawBool, B(true)),
    ("musicToast", Enum(&["never", "pause", "pause_and_toast"]), S("never")),
    ("musicFrequency", Enum(&["DEFAULT", "FREQUENT", "CONSTANT"]), S("DEFAULT")),
    // Specific to Rustcraft (no Microsoft account): last offline username used.
    ("lastUsername", Ty::RawStr, S("")),
    // Exclusive fullscreen mode (the game's JSON); written only if chosen.
    ("fullscreenResolution", Ty::RawStr, S("")),
];

/// Category of a key binding (order of the key list, § 3.10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Cat {
    Movement,
    Misc,
    Multiplayer,
    Gameplay,
    Inventory,
    Creative,
    Spectator,
    Debug,
}

impl Cat {
    pub fn key(self) -> &'static str {
        [
            "key.category.minecraft.movement",
            "key.category.minecraft.misc",
            "key.category.minecraft.multiplayer",
            "key.category.minecraft.gameplay",
            "key.category.minecraft.inventory",
            "key.category.minecraft.creative",
            "key.category.minecraft.spectator",
            "key.category.minecraft.debug",
        ][self as usize]
    }
}

/// Bindings in write order: name, default key, category, order.
pub const KEYS: &[(&str, &str, Cat, i32)] = &[
    ("key.attack", "key.mouse.left", Cat::Gameplay, 0),
    ("key.use", "key.mouse.right", Cat::Gameplay, 0),
    ("key.forward", "key.keyboard.w", Cat::Movement, 0),
    ("key.left", "key.keyboard.a", Cat::Movement, 0),
    ("key.back", "key.keyboard.s", Cat::Movement, 0),
    ("key.right", "key.keyboard.d", Cat::Movement, 0),
    ("key.jump", "key.keyboard.space", Cat::Movement, 0),
    ("key.sneak", "key.keyboard.left.shift", Cat::Movement, 0),
    ("key.sprint", "key.keyboard.left.control", Cat::Movement, 0),
    ("key.drop", "key.keyboard.q", Cat::Inventory, 0),
    ("key.inventory", "key.keyboard.e", Cat::Inventory, 0),
    ("key.chat", "key.keyboard.t", Cat::Multiplayer, 0),
    ("key.playerlist", "key.keyboard.tab", Cat::Multiplayer, 0),
    ("key.pickItem", "key.mouse.middle", Cat::Gameplay, 0),
    ("key.command", "key.keyboard.slash", Cat::Multiplayer, 0),
    ("key.friends", "key.keyboard.o", Cat::Multiplayer, 0),
    ("key.socialInteractions", "key.keyboard.p", Cat::Multiplayer, 0),
    ("key.toggleGui", "key.keyboard.f1", Cat::Misc, 0),
    ("key.toggleSpectatorShaderEffects", "key.keyboard.f4", Cat::Misc, 0),
    ("key.screenshot", "key.keyboard.f2", Cat::Misc, 0),
    ("key.togglePerspective", "key.keyboard.f5", Cat::Misc, 0),
    ("key.smoothCamera", "key.keyboard.unknown", Cat::Misc, 0),
    ("key.fullscreen", "key.keyboard.f11", Cat::Misc, 0),
    ("key.spectatorOutlines", "key.keyboard.unknown", Cat::Spectator, 0),
    ("key.spectatorHotbar", "key.mouse.middle", Cat::Spectator, 0),
    ("key.swapOffhand", "key.keyboard.f", Cat::Inventory, 0),
    ("key.saveToolbarActivator", "key.keyboard.c", Cat::Creative, 0),
    ("key.loadToolbarActivator", "key.keyboard.x", Cat::Creative, 0),
    ("key.advancements", "key.keyboard.l", Cat::Misc, 0),
    ("key.quickActions", "key.keyboard.g", Cat::Misc, 0),
    ("key.debug.overlay", "key.keyboard.f3", Cat::Debug, -2),
    ("key.debug.modifier", "key.keyboard.f3", Cat::Debug, -1),
    ("key.hotbar.1", "key.keyboard.1", Cat::Inventory, 0),
    ("key.hotbar.2", "key.keyboard.2", Cat::Inventory, 0),
    ("key.hotbar.3", "key.keyboard.3", Cat::Inventory, 0),
    ("key.hotbar.4", "key.keyboard.4", Cat::Inventory, 0),
    ("key.hotbar.5", "key.keyboard.5", Cat::Inventory, 0),
    ("key.hotbar.6", "key.keyboard.6", Cat::Inventory, 0),
    ("key.hotbar.7", "key.keyboard.7", Cat::Inventory, 0),
    ("key.hotbar.8", "key.keyboard.8", Cat::Inventory, 0),
    ("key.hotbar.9", "key.keyboard.9", Cat::Inventory, 0),
    ("key.debug.reloadChunk", "key.keyboard.a", Cat::Debug, 0),
    ("key.debug.showHitboxes", "key.keyboard.b", Cat::Debug, 0),
    ("key.debug.clearChat", "key.keyboard.d", Cat::Debug, 0),
    ("key.debug.crash", "key.keyboard.c", Cat::Debug, 0),
    ("key.debug.showChunkBorders", "key.keyboard.g", Cat::Debug, 0),
    ("key.debug.showAdvancedTooltips", "key.keyboard.h", Cat::Debug, 0),
    ("key.debug.copyRecreateCommand", "key.keyboard.i", Cat::Debug, 0),
    ("key.debug.spectate", "key.keyboard.n", Cat::Debug, 0),
    ("key.debug.switchGameMode", "key.keyboard.f4", Cat::Debug, 0),
    ("key.debug.debugOptions", "key.keyboard.f6", Cat::Debug, 0),
    ("key.debug.focusPause", "key.keyboard.p", Cat::Debug, 0),
    ("key.debug.dumpDynamicTextures", "key.keyboard.s", Cat::Debug, 0),
    ("key.debug.reloadResourcePacks", "key.keyboard.t", Cat::Debug, 0),
    ("key.debug.profiling", "key.keyboard.l", Cat::Debug, 0),
    ("key.debug.copyLocation", "key.keyboard.c", Cat::Debug, 0),
    ("key.debug.dumpVersion", "key.keyboard.v", Cat::Debug, 0),
    ("key.debug.profilingChart", "key.keyboard.1", Cat::Debug, 1),
    ("key.debug.fpsCharts", "key.keyboard.2", Cat::Debug, 2),
    ("key.debug.networkCharts", "key.keyboard.3", Cat::Debug, 3),
    ("key.debug.lightmapTexture", "key.keyboard.4", Cat::Debug, 4),
    ("key.debug.improvedTransparency", "key.keyboard.x", Cat::Debug, 0),
];

/// Sound categories, in write order.
pub const SOUND_CATEGORIES: [&str; 11] =
    ["master", "music", "record", "weather", "block", "hostile", "neutral", "player", "ambient", "voice", "ui"];

/// Model parts (bit 0 … bit 6 of the mask sent to the server).
pub const MODEL_PARTS: [&str; 7] =
    ["cape", "jacket", "left_sleeve", "right_sleeve", "left_pants_leg", "right_pants_leg", "hat"];

/// "Quality" settings: changing them switches the preset to Custom (§ 3.1).
pub const QUALITY: [&str; 17] = [
    "biomeBlendRadius",
    "renderDistance",
    "prioritizeChunkUpdates",
    "simulationDistance",
    "ao",
    "renderClouds",
    "particles",
    "mipmapLevels",
    "entityShadows",
    "entityDistanceScaling",
    "menuBackgroundBlurriness",
    "cloudRange",
    "cutoutLeaves",
    "improvedTransparency",
    "textureFiltering",
    "maxAnisotropyBit",
    "weatherRadius",
];

/// All keys: options, bindings, sound categories, model parts.
fn index() -> &'static HashMap<String, usize> {
    static INDEX: OnceLock<HashMap<String, usize>> = OnceLock::new();
    INDEX.get_or_init(|| {
        let mut m = HashMap::new();
        for (i, d) in DEFS.iter().enumerate() {
            m.insert(d.0.to_owned(), i);
        }
        let mut n = DEFS.len();
        for k in KEYS {
            m.insert(format!("key_{}", k.0), n);
            n += 1;
        }
        for c in SOUND_CATEGORIES {
            m.insert(format!("soundCategory_{c}"), n);
            n += 1;
        }
        for p in MODEL_PARTS {
            m.insert(format!("modelPart_{p}"), n);
            n += 1;
        }
        m
    })
}

/// Type and default of an entry of the full table.
fn def(i: usize) -> (Ty, Val) {
    let n = DEFS.len();
    if i < n {
        return (DEFS[i].1, DEFS[i].2.val());
    }
    let i = i - n;
    if i < KEYS.len() {
        return (Ty::RawStr, Val::S(KEYS[i].1.to_owned()));
    }
    let i = i - KEYS.len();
    if i < SOUND_CATEGORIES.len() {
        return (UNIT, Val::F(1.0));
    }
    (Ty::RawBool, Val::B(true))
}

fn total() -> usize {
    DEFS.len() + KEYS.len() + SOUND_CATEGORIES.len() + MODEL_PARTS.len()
}

/// Real written like Java's double-to-text conversion: `1.0`, `0.5`, `0.20860927152317882`, `1.0E-4`.
pub fn java_double(v: f64) -> String {
    if v.is_nan() {
        return "NaN".into();
    }
    if v.is_infinite() {
        return if v > 0.0 { "Infinity".into() } else { "-Infinity".into() };
    }
    let a = v.abs();
    if a == 0.0 || (1e-3..1e7).contains(&a) {
        let s = format!("{v}");
        return if s.contains('.') { s } else { format!("{s}.0") };
    }
    let s = format!("{v:e}");
    let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
    let m = if m.contains('.') { m.to_owned() } else { format!("{m}.0") };
    format!("{m}E{e}")
}

/// Tolerant decoding of a JSON value: strict JSON, otherwise bare text; empty = `""`.
fn json(text: &str) -> serde_json::Value {
    let t = text.trim();
    if t.is_empty() {
        return serde_json::Value::String(String::new());
    }
    serde_json::from_str(t).unwrap_or_else(|_| serde_json::Value::String(t.to_owned()))
}

/// Value read for a type; `None` = invalid (default kept).
fn decode(ty: Ty, text: &str) -> Option<Val> {
    use serde_json::Value as J;
    match ty {
        Ty::Bool => match json(text) {
            J::Bool(b) => Some(Val::B(b)),
            J::String(s) => s.parse().ok().map(Val::B),
            _ => None,
        },
        Ty::Int(min, max) => {
            let v = match json(text) {
                J::Number(n) => n.as_i64()?,
                J::String(s) => s.parse().ok()?,
                _ => return None,
            };
            // Decoder: [min, max + 1]; max + 1 rejected on assignment → default.
            (i64::from(min)..=i64::from(max)).contains(&v).then_some(Val::I(v as i32))
        }
        Ty::Real(min, max) => {
            // Exact reading of the number (the JSON decoder loses the last digit).
            let v = match text.trim().parse::<f64>().ok().map_or_else(|| json(text), |_| J::Null) {
                J::Null => text.trim().parse::<f64>().ok()?,
                J::Number(n) => n.as_f64()?,
                J::Bool(b) if (min, max) == (0.0, 1.0) => f64::from(u8::from(b)),
                J::String(s) => s.parse().ok()?,
                _ => return None,
            };
            (min..=max).contains(&v).then_some(Val::F(v))
        }
        Ty::Enum(values) => match json(text) {
            J::String(s) if values.contains(&s.as_str()) => Some(Val::S(s)),
            _ => None,
        },
        Ty::Str => match json(text) {
            J::String(s) => Some(Val::S(s)),
            _ => None,
        },
        Ty::Clouds => match json(text) {
            J::Bool(b) => Some(Val::S(b.to_string())),
            J::String(s) if matches!(s.as_str(), "true" | "fast" | "false") => Some(Val::S(s)),
            _ => None,
        },
        Ty::RawStr => Some(Val::S(text.to_owned())),
        Ty::RawBool => Some(Val::B(text == "true")),
        Ty::RawInt => text.trim().parse().ok().map(Val::I),
        Ty::RawList => match json(text) {
            J::Array(a) => Some(Val::L(a.into_iter().filter_map(|v| v.as_str().map(str::to_owned)).collect())),
            _ => None,
        },
    }
}

fn encode(ty: Ty, v: &Val) -> String {
    match (ty, v) {
        (Ty::RawStr, Val::S(s)) => s.clone(),
        (_, Val::B(b)) => b.to_string(),
        (_, Val::I(i)) => i.to_string(),
        (_, Val::F(f)) => java_double(*f),
        (_, Val::S(s)) => serde_json::Value::String(s.clone()).to_string(),
        (_, Val::L(l)) => serde_json::to_string(l).unwrap_or_else(|_| "[]".into()),
    }
}

/// Player settings.
#[derive(Debug, Clone, PartialEq)]
pub struct Options {
    vals: Vec<Val>,
}

impl Default for Options {
    fn default() -> Self {
        Options { vals: (0..total()).map(|i| def(i).1).collect() }
    }
}

impl Options {
    /// Reading the text of `options.txt` (without preset); invalid lines ignored.
    pub fn parse(text: &str) -> Options {
        let mut o = Options::default();
        for line in text.lines() {
            let Some((k, v)) = line.split_once(':') else {
                if !line.trim().is_empty() {
                    eprintln!("options: line ignored '{line}'");
                }
                continue;
            };
            let Some(&i) = index().get(k) else { continue };
            match decode(def(i).0, v) {
                Some(val) => o.vals[i] = val,
                None => eprintln!("options: invalid value for {k}: {v}"),
            }
        }
        o
    }

    /// File in the game folder (missing: defaults), then graphics preset (§ 4.4).
    pub fn load(path: &Path) -> Options {
        let mut o = std::fs::read_to_string(path).map(|t| Options::parse(&t)).unwrap_or_default();
        o.apply_preset();
        o
    }

    /// Game startup safeguard: reads `startedCleanly`, sets it to false (the caller saves)
    /// and, if the previous startup did not complete, lifts fullscreen (setting, remembered
    /// video mode) then downgrades the preferred graphics API: Vulkan to Default (kept), Default
    /// to OpenGL (in memory only). `api_forced`: API forced by the launch, setting
    /// ignored. Returns true if the previous startup was clean.
    pub fn begin_startup(&mut self, api_forced: bool) -> bool {
        let clean = self.b("startedCleanly");
        self.set("startedCleanly", Val::B(false));
        if !clean {
            eprintln!("options: unexpected shutdown at the last startup, fullscreen reset");
            self.set("fullscreen", Val::B(false));
            self.set("fullscreenResolution", Val::S(String::new()));
            if !api_forced {
                match self.s("preferredGraphicsBackend") {
                    "vulkan" => {
                        self.set("preferredGraphicsBackend", Val::S("default".into()));
                    }
                    "default" => {
                        self.set("preferredGraphicsBackend", Val::S("opengl".into()));
                    }
                    _ => {}
                }
            }
        }
        clean
    }

    /// Complete text, exact game order (§ 4.3).
    pub fn to_text(&self) -> String {
        let mut out = format!("version:{DATA_VERSION}\n");
        let n = DEFS.len();
        let name = |i: usize| -> String {
            if i < n {
                return DEFS[i].0.to_owned();
            }
            let j = i - n;
            if j < KEYS.len() {
                return format!("key_{}", KEYS[j].0);
            }
            let j = j - KEYS.len();
            if j < SOUND_CATEGORIES.len() {
                return format!("soundCategory_{}", SOUND_CATEGORIES[j]);
            }
            format!("modelPart_{}", MODEL_PARTS[j - SOUND_CATEGORIES.len()])
        };
        for (i, v) in self.vals.iter().enumerate() {
            if i < n && DEFS[i].0 == "fullscreenResolution" && matches!(v, Val::S(s) if s.is_empty()) {
                continue;
            }
            out.push_str(&name(i));
            out.push(':');
            out.push_str(&encode(def(i).0, v));
            out.push('\n');
        }
        out
    }

    /// Full write (temporary file then rename).
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension("txt.tmp");
        std::fs::write(&tmp, self.to_text())?;
        std::fs::rename(&tmp, path)
    }

    fn at(&self, key: &str) -> Option<&Val> {
        index().get(key).map(|&i| &self.vals[i])
    }

    pub fn b(&self, key: &str) -> bool {
        matches!(self.at(key), Some(Val::B(true)))
    }

    pub fn i(&self, key: &str) -> i32 {
        match self.at(key) {
            Some(Val::I(v)) => *v,
            _ => 0,
        }
    }

    pub fn f(&self, key: &str) -> f64 {
        match self.at(key) {
            Some(Val::F(v)) => *v,
            _ => 0.0,
        }
    }

    pub fn s(&self, key: &str) -> &str {
        match self.at(key) {
            Some(Val::S(v)) => v,
            _ => "",
        }
    }

    /// List of strings (`resourcePacks`…); empty if another type.
    pub fn list(&self, key: &str) -> Vec<String> {
        match self.at(key) {
            Some(Val::L(v)) => v.clone(),
            _ => Vec::new(),
        }
    }

    pub fn ty(&self, key: &str) -> Option<Ty> {
        index().get(key).map(|&i| def(i).0)
    }

    /// Assignment; an out-of-range value falls back to the default (§ 3.1). Returns true if changed.
    pub fn set(&mut self, key: &str, v: Val) -> bool {
        let Some(&i) = index().get(key) else { return false };
        let (ty, default) = def(i);
        let ok = match (ty, &v) {
            (Ty::Int(min, max), Val::I(x)) => (min..=max).contains(x),
            (Ty::Real(min, max), Val::F(x)) => (min..=max).contains(x),
            (Ty::Enum(values), Val::S(s)) => values.contains(&s.as_str()),
            _ => std::mem::discriminant(&v) == std::mem::discriminant(&default),
        };
        let v = if ok {
            v
        } else {
            eprintln!("options: value rejected for {key}: {v:?}");
            default
        };
        let changed = self.vals[i] != v;
        self.vals[i] = v;
        changed
    }

    pub fn is_default(&self, key: &str) -> bool {
        index().get(key).is_some_and(|&i| self.vals[i] == def(i).1)
    }

    pub fn reset(&mut self, key: &str) {
        if let Some(&i) = index().get(key) {
            self.vals[i] = def(i).1;
        }
    }

    /// Graphics preset (§ 3.5), except Custom.
    pub fn apply_preset(&mut self) {
        let k = match self.s("graphicsPreset") {
            "fast" => 0,
            "fancy" => 1,
            "fabulous" => 2,
            _ => return,
        };
        let int = |a: [i32; 3]| Val::I(a[k]);
        self.set("biomeBlendRadius", int([1, 2, 2]));
        self.set("renderDistance", int([8, 16, 32]));
        self.set("prioritizeChunkUpdates", int([0, 1, 1]));
        self.set("simulationDistance", int([6, 12, 12]));
        self.set("ao", Val::B(k > 0));
        self.set("renderClouds", Val::S(if k == 0 { "fast" } else { "true" }.into()));
        self.set("particles", int([1, 0, 0]));
        self.set("mipmapLevels", int([2, 4, 4]));
        self.set("entityShadows", Val::B(k > 0));
        self.set("entityDistanceScaling", Val::F([0.75, 1.0, 1.25][k]));
        self.set("menuBackgroundBlurriness", int([2, 5, 5]));
        self.set("cloudRange", int([32, 64, 128]));
        self.set("cutoutLeaves", Val::B(k > 0));
        self.set("improvedTransparency", Val::B(k == 2));
        self.set("weatherRadius", int([5, 10, 10]));
        self.set("maxAnisotropyBit", int([1, 1, 2]));
        self.set("textureFiltering", int([0, 1, 2]));
    }

    /// Mask of the model parts (bit 0 cape … bit 6 hat).
    pub fn skin_parts(&self) -> u8 {
        MODEL_PARTS
            .iter()
            .enumerate()
            .filter(|(_, p)| self.b(&format!("modelPart_{p}")))
            .fold(0, |m, (i, _)| m | 1 << i)
    }

    /// Client information to send to the server (§ 4.4).
    pub fn client_info(&self) -> ClientInformation {
        ClientInformation {
            language: self.s("lang").to_owned(),
            view_distance: self.i("renderDistance").clamp(2, 32) as u8,
            chat_visibility: self.i("chatVisibility"),
            chat_colors: self.b("chatColors"),
            skin_parts: self.skin_parts(),
            main_hand: i32::from(self.s("mainHand") == "right"),
            text_filtering: false,
            allow_listing: self.b("allowServerListing"),
            particles: self.i("particles"),
        }
    }

    /// Field of view in degrees (stored `(fov − 70) / 40`).
    pub fn fov(&self) -> i32 {
        (70.0 + self.f("fov") * 40.0).round() as i32
    }

    /// Mouse rotation factor (§ 3.9): `(v × 0.6 + 0.2)³ × 8`.
    pub fn mouse_factor(&self) -> f64 {
        let f = self.f("mouseSensitivity") * 0.6 + 0.2;
        f * f * f * 8.0
    }
}

/// Keyboard key names (`key.keyboard.*`) of winit's physical codes.
pub fn key_name(k: KeyCode) -> &'static str {
    crate::keynames::key_name(k)
}

/// Key name of a mouse button (0 left, 1 right, 2 middle, 3+ side buttons).
pub fn mouse_name(button: u8) -> String {
    match button {
        0 => "key.mouse.left".into(),
        1 => "key.mouse.right".into(),
        2 => "key.mouse.middle".into(),
        n => format!("key.mouse.{}", u32::from(n) + 1),
    }
}

/// Label of a key (§ 4.3): character of the keyboard layout in uppercase if alone
/// (Z for "Walk Forwards" on AZERTY), otherwise translation.
pub fn key_label(lang: &rc_text::Lang, name: &str) -> String {
    let name = if name.starts_with("key.keyboard.") {
        match crate::keynames::keyboard_label(name) {
            Ok(label) => return label,
            Err(key) => key,
        }
    } else {
        name
    };
    if let Some(t) = lang.map.get(name) {
        return t.clone();
    }
    match name.strip_prefix("key.mouse.") {
        Some(n) => crate::widgets::tr(lang, "key.mouse", &[n.to_owned()]),
        None => name.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The greatest cloud range fits in the radius of the compact formats: raising the
    /// `cloudRange` bound beyond would silently truncate the clouds.
    #[test]
    fn cloud_range_fits_the_packed_radius() {
        let Some((_, Int(_, max), _)) = DEFS.iter().find(|d| d.0 == "cloudRange") else {
            panic!("cloudRange missing or not an integer")
        };
        let radius = crate::sky_mesh::cloud_radius(*max);
        assert!(radius <= crate::sky_mesh::FACE_LIMIT, "radius {radius} for cloudRange {max}");
        assert_eq!(radius, 171);
    }

    #[test]
    fn unclean_start_resets_fullscreen_and_api() {
        let mut o = Options::parse(
            "startedCleanly:false
fullscreen:true
fullscreenResolution:1920x1080@60:24
preferredGraphicsBackend:vulkan
",
        );
        assert!(!o.begin_startup(false));
        assert!(!o.b("fullscreen") && o.s("fullscreenResolution").is_empty() && !o.b("startedCleanly"));
        assert_eq!(o.s("preferredGraphicsBackend"), "default");
        let mut o = Options::parse(
            "startedCleanly:false
",
        );
        o.begin_startup(false);
        assert_eq!(o.s("preferredGraphicsBackend"), "opengl");
        let mut o = Options::parse(
            "startedCleanly:false
preferredGraphicsBackend:vulkan
",
        );
        o.begin_startup(true);
        assert_eq!(o.s("preferredGraphicsBackend"), "vulkan");
    }

    #[test]
    fn clean_start_keeps_settings() {
        let mut o = Options::parse(
            "fullscreen:true
preferredGraphicsBackend:vulkan
",
        );
        assert!(o.begin_startup(false));
        assert!(o.b("fullscreen") && !o.b("startedCleanly"));
        assert_eq!(o.s("preferredGraphicsBackend"), "vulkan");
    }

    #[test]
    fn java_double_format() {
        assert_eq!(java_double(1.0), "1.0");
        assert_eq!(java_double(0.0), "0.0");
        assert_eq!(java_double(0.4375), "0.4375");
        assert_eq!(java_double(0.20860927152317882), "0.20860927152317882");
        assert_eq!(java_double(1e-4), "1.0E-4");
        assert_eq!(java_double(-1.0), "-1.0");
    }

    #[test]
    fn defaults_round_trip_and_order() {
        let o = Options::default();
        let t = o.to_text();
        let lines: Vec<&str> = t.lines().collect();
        assert_eq!(lines[0], "version:5023");
        assert_eq!(lines[1], "ao:true");
        assert!(lines.contains(&"fov:0.0"));
        assert!(lines.contains(&"graphicsPreset:\"fancy\""));
        assert!(lines.contains(&"lastServer:"));
        assert!(lines.contains(&"lang:en_us"));
        assert!(lines.contains(&"resourcePacks:[]"));
        assert!(lines.contains(&"key_key.forward:key.keyboard.w"));
        assert_eq!(lines.last(), Some(&"modelPart_hat:true"));
        assert_eq!(Options::parse(&t), o);
    }

    #[test]
    fn lenient_reading() {
        let o = Options::parse(
            "renderDistance:33\nsimulationDistance:20\ngamma:true\nrenderClouds:false\nfoo\nmodelPart_cape:TRUE\n\
             graphicsPreset:fast\nmaxFps:261\nsoundCategory_music:0.25\nkey_key.jump:key.keyboard.f",
        );
        assert_eq!(o.i("renderDistance"), 12);
        assert_eq!(o.i("simulationDistance"), 20);
        assert_eq!(o.f("gamma"), 1.0);
        assert_eq!(o.s("renderClouds"), "false");
        assert!(!o.b("modelPart_cape"));
        assert_eq!(o.s("graphicsPreset"), "fast");
        assert_eq!(o.i("maxFps"), 120);
        assert_eq!(o.f("soundCategory_music"), 0.25);
        assert_eq!(o.s("key_key.jump"), "key.keyboard.f");
        assert_eq!(o.skin_parts(), 0x7e);
    }

    #[test]
    fn preset_applies_on_load_unless_custom() {
        let mut o = Options::parse("graphicsPreset:\"fast\"\nrenderDistance:20");
        o.apply_preset();
        assert_eq!(o.i("renderDistance"), 8);
        assert_eq!(o.s("renderClouds"), "fast");
        let mut c = Options::parse("graphicsPreset:\"custom\"\nrenderDistance:20");
        c.apply_preset();
        assert_eq!(c.i("renderDistance"), 20);
        // First launch: Fancy column.
        let mut d = Options::default();
        d.apply_preset();
        assert_eq!((d.i("renderDistance"), d.i("cloudRange"), d.i("textureFiltering")), (16, 64, 1));
    }

    #[test]
    fn client_information() {
        let mut o = Options::default();
        o.set("mainHand", Val::S("left".into()));
        o.set("modelPart_hat", Val::B(false));
        let c = o.client_info();
        assert_eq!((c.language.as_str(), c.view_distance, c.main_hand, c.skin_parts), ("en_us", 12, 0, 0x3f));
        assert!(!o.set("renderDistance", Val::I(99)));
        assert_eq!(o.i("renderDistance"), 12);
    }

    /// File of the official game (`RUSTCRAFT_OPTIONS_SAMPLE`): re-read then rewritten identically.
    #[test]
    fn official_file_round_trip() {
        let Some(p) = rc_data::env_os!("RUSTCRAFT_OPTIONS_SAMPLE") else { return };
        let text = std::fs::read_to_string(p).expect("read");
        let o = Options::parse(&text);
        assert_eq!(o.to_text().replace("\r\n", "\n"), text.replace("\r\n", "\n"));
    }
}
