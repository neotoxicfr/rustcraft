//! Names and labels of the keyboard keys (`key.keyboard.*` of options.txt).
//!
//! Each key has a physical position number (USB HID usage, keyboard page, plus a few
//! codes beyond 256 for the media keys); the name saved in options.txt comes from
//! this number. The displayed label follows the keyboard LAYOUT: the system is asked for the
//! character produced by the key without a modifier (Z on AZERTY in place of QWERTY's W); a
//! key without a single character (F1, Shift…) takes the translation of its name.

use std::sync::OnceLock;
use winit::keyboard::KeyCode;

/// Name suffixes (`key.keyboard.<suffix>`) and position numbers, game order.
const KEYS: &[(&str, u16)] = &[
    ("unknown", 0),
    ("a", 4),
    ("b", 5),
    ("c", 6),
    ("d", 7),
    ("e", 8),
    ("f", 9),
    ("g", 10),
    ("h", 11),
    ("i", 12),
    ("j", 13),
    ("k", 14),
    ("l", 15),
    ("m", 16),
    ("n", 17),
    ("o", 18),
    ("p", 19),
    ("q", 20),
    ("r", 21),
    ("s", 22),
    ("t", 23),
    ("u", 24),
    ("v", 25),
    ("w", 26),
    ("x", 27),
    ("y", 28),
    ("z", 29),
    ("1", 30),
    ("2", 31),
    ("3", 32),
    ("4", 33),
    ("5", 34),
    ("6", 35),
    ("7", 36),
    ("8", 37),
    ("9", 38),
    ("0", 39),
    ("enter", 40),
    ("escape", 41),
    ("backspace", 42),
    ("tab", 43),
    ("space", 44),
    ("minus", 45),
    ("equal", 46),
    ("left.bracket", 47),
    ("right.bracket", 48),
    ("backslash", 49),
    ("world.2", 50),
    ("semicolon", 51),
    ("apostrophe", 52),
    ("grave.accent", 53),
    ("comma", 54),
    ("period", 55),
    ("slash", 56),
    ("caps.lock", 57),
    ("f1", 58),
    ("f2", 59),
    ("f3", 60),
    ("f4", 61),
    ("f5", 62),
    ("f6", 63),
    ("f7", 64),
    ("f8", 65),
    ("f9", 66),
    ("f10", 67),
    ("f11", 68),
    ("f12", 69),
    ("print.screen", 70),
    ("scroll.lock", 71),
    ("pause", 72),
    ("insert", 73),
    ("home", 74),
    ("page.up", 75),
    ("delete", 76),
    ("end", 77),
    ("page.down", 78),
    ("right", 79),
    ("left", 80),
    ("down", 81),
    ("up", 82),
    ("num.lock", 83),
    ("keypad.divide", 84),
    ("keypad.multiply", 85),
    ("keypad.subtract", 86),
    ("keypad.add", 87),
    ("keypad.enter", 88),
    ("keypad.1", 89),
    ("keypad.2", 90),
    ("keypad.3", 91),
    ("keypad.4", 92),
    ("keypad.5", 93),
    ("keypad.6", 94),
    ("keypad.7", 95),
    ("keypad.8", 96),
    ("keypad.9", 97),
    ("keypad.0", 98),
    ("keypad.period", 99),
    ("world.1", 100),
    ("application", 101),
    ("power", 102),
    ("keypad.equal", 103),
    ("f13", 104),
    ("f14", 105),
    ("f15", 106),
    ("f16", 107),
    ("f17", 108),
    ("f18", 109),
    ("f19", 110),
    ("f20", 111),
    ("f21", 112),
    ("f22", 113),
    ("f23", 114),
    ("f24", 115),
    ("execute", 116),
    ("help", 117),
    ("menu", 118),
    ("select", 119),
    ("stop", 120),
    ("again", 121),
    ("undo", 122),
    ("cut", 123),
    ("copy", 124),
    ("paste", 125),
    ("find", 126),
    ("mute", 127),
    ("volume.up", 128),
    ("volume.down", 129),
    ("keypad.comma", 133),
    ("keypad.equals.as400", 134),
    ("international1", 135),
    ("international2", 136),
    ("international3", 137),
    ("international4", 138),
    ("international5", 139),
    ("international6", 140),
    ("international7", 141),
    ("international8", 142),
    ("international9", 143),
    ("lang1", 144),
    ("lang2", 145),
    ("lang3", 146),
    ("lang4", 147),
    ("lang5", 148),
    ("lang6", 149),
    ("lang7", 150),
    ("lang8", 151),
    ("lang9", 152),
    ("alternate.erase", 153),
    ("sys.req", 154),
    ("cancel", 155),
    ("clear", 156),
    ("prior", 157),
    ("enter2", 158),
    ("separator", 159),
    ("out", 160),
    ("oper", 161),
    ("clear.again", 162),
    ("crsel", 163),
    ("exsel", 164),
    ("keypad.00", 176),
    ("keypad.000", 177),
    ("thousands.separator", 178),
    ("decimal.separator", 179),
    ("currency.unit", 180),
    ("currency.subunit", 181),
    ("keypad.left.parenthesis", 182),
    ("keypad.right.parenthesis", 183),
    ("keypad.left.brace", 184),
    ("keypad.right.brace", 185),
    ("keypad.tab", 186),
    ("keypad.backspace", 187),
    ("keypad.a", 188),
    ("keypad.b", 189),
    ("keypad.c", 190),
    ("keypad.d", 191),
    ("keypad.e", 192),
    ("keypad.f", 193),
    ("keypad.xor", 194),
    ("keypad.power", 195),
    ("keypad.percent", 196),
    ("keypad.less", 197),
    ("keypad.greater", 198),
    ("keypad.ampersand", 199),
    ("keypad.double.ampersand", 200),
    ("keypad.vertical.bar", 201),
    ("keypad.double.vertical.bar", 202),
    ("keypad.colon", 203),
    ("keypad.hash", 204),
    ("keypad.space", 205),
    ("keypad.at", 206),
    ("keypad.exclamation", 207),
    ("keypad.memory.store", 208),
    ("keypad.memory.recall", 209),
    ("keypad.memory.clear", 210),
    ("keypad.memory.add", 211),
    ("keypad.memory.subtract", 212),
    ("keypad.memory.multiply", 213),
    ("keypad.memory.divide", 214),
    ("keypad.plus.minus", 215),
    ("keypad.clear", 216),
    ("keypad.clear.entry", 217),
    ("keypad.binary", 218),
    ("keypad.octal", 219),
    ("keypad.decimal", 220),
    ("keypad.hexadecimal", 221),
    ("left.control", 224),
    ("left.shift", 225),
    ("left.alt", 226),
    ("left.win", 227),
    ("right.control", 228),
    ("right.shift", 229),
    ("right.alt", 230),
    ("right.win", 231),
    ("mode", 257),
    ("sleep", 258),
    ("wake", 259),
    ("channel.up", 260),
    ("channel.down", 261),
    ("media.play", 262),
    ("media.pause", 263),
    ("media.record", 264),
    ("media.fast.forward", 265),
    ("media.rewind", 266),
    ("media.next.track", 267),
    ("media.previous.track", 268),
    ("media.stop", 269),
    ("media.eject", 270),
    ("media.play.pause", 271),
    ("media.select", 272),
    ("ac.new", 273),
    ("ac.open", 274),
    ("ac.close", 275),
    ("ac.exit", 276),
    ("ac.save", 277),
    ("ac.print", 278),
    ("ac.properties", 279),
    ("ac.search", 280),
    ("ac.home", 281),
    ("ac.back", 282),
    ("ac.forward", 283),
    ("ac.stop", 284),
    ("ac.refresh", 285),
    ("ac.bookmarks", 286),
    ("soft.left", 287),
    ("soft.right", 288),
    ("call", 289),
    ("end.call", 290),
];

/// Full names, same order as `KEYS`.
fn full_names() -> &'static [String] {
    static NAMES: OnceLock<Vec<String>> = OnceLock::new();
    NAMES.get_or_init(|| KEYS.iter().map(|(s, _)| format!("key.keyboard.{s}")).collect())
}

/// Mapping physical key of winit → position number (HID usage).
fn position(k: KeyCode) -> Option<u16> {
    use KeyCode as K;
    let letters = [
        K::KeyA,
        K::KeyB,
        K::KeyC,
        K::KeyD,
        K::KeyE,
        K::KeyF,
        K::KeyG,
        K::KeyH,
        K::KeyI,
        K::KeyJ,
        K::KeyK,
        K::KeyL,
        K::KeyM,
        K::KeyN,
        K::KeyO,
        K::KeyP,
        K::KeyQ,
        K::KeyR,
        K::KeyS,
        K::KeyT,
        K::KeyU,
        K::KeyV,
        K::KeyW,
        K::KeyX,
        K::KeyY,
        K::KeyZ,
    ];
    if let Some(i) = letters.iter().position(|&l| l == k) {
        return Some(4 + i as u16);
    }
    let digits = [K::Digit1, K::Digit2, K::Digit3, K::Digit4, K::Digit5, K::Digit6, K::Digit7, K::Digit8, K::Digit9];
    if let Some(i) = digits.iter().position(|&d| d == k) {
        return Some(30 + i as u16);
    }
    let fkeys = [K::F1, K::F2, K::F3, K::F4, K::F5, K::F6, K::F7, K::F8, K::F9, K::F10, K::F11, K::F12];
    if let Some(i) = fkeys.iter().position(|&f| f == k) {
        return Some(58 + i as u16);
    }
    let fhigh = [K::F13, K::F14, K::F15, K::F16, K::F17, K::F18, K::F19, K::F20, K::F21, K::F22, K::F23, K::F24];
    if let Some(i) = fhigh.iter().position(|&f| f == k) {
        return Some(104 + i as u16);
    }
    let pad = [
        K::Numpad1,
        K::Numpad2,
        K::Numpad3,
        K::Numpad4,
        K::Numpad5,
        K::Numpad6,
        K::Numpad7,
        K::Numpad8,
        K::Numpad9,
        K::Numpad0,
    ];
    if let Some(i) = pad.iter().position(|&p| p == k) {
        return Some(89 + i as u16);
    }
    Some(match k {
        K::Digit0 => 39,
        K::Enter => 40,
        K::Escape => 41,
        K::Backspace => 42,
        K::Tab => 43,
        K::Space => 44,
        K::Minus => 45,
        K::Equal => 46,
        K::BracketLeft => 47,
        K::BracketRight => 48,
        K::Backslash => 49,
        K::Semicolon => 51,
        K::Quote => 52,
        K::Backquote => 53,
        K::Comma => 54,
        K::Period => 55,
        K::Slash => 56,
        K::CapsLock => 57,
        K::PrintScreen => 70,
        K::ScrollLock => 71,
        K::Pause => 72,
        K::Insert => 73,
        K::Home => 74,
        K::PageUp => 75,
        K::Delete => 76,
        K::End => 77,
        K::PageDown => 78,
        K::ArrowRight => 79,
        K::ArrowLeft => 80,
        K::ArrowDown => 81,
        K::ArrowUp => 82,
        K::NumLock => 83,
        K::NumpadDivide => 84,
        K::NumpadMultiply => 85,
        K::NumpadSubtract => 86,
        K::NumpadAdd => 87,
        K::NumpadEnter => 88,
        K::NumpadDecimal => 99,
        // ISO key between left Shift and the first letter ("< >" on AZERTY).
        K::IntlBackslash => 100,
        // "Context menu" key: position "application" (101), not "menu" (118).
        K::ContextMenu => 101,
        K::Power => 102,
        K::NumpadEqual => 103,
        K::Help => 117,
        K::Select => 119,
        K::Again => 121,
        K::Undo => 122,
        K::Cut => 123,
        K::Copy => 124,
        K::Paste => 125,
        K::Find => 126,
        K::AudioVolumeMute => 127,
        K::AudioVolumeUp => 128,
        K::AudioVolumeDown => 129,
        K::NumpadComma => 133,
        K::IntlRo => 135,
        K::KanaMode => 136,
        K::IntlYen => 137,
        K::Convert => 138,
        K::NonConvert => 139,
        K::Lang1 => 144,
        K::Lang2 => 145,
        K::Lang3 => 146,
        K::Lang4 => 147,
        K::Lang5 => 148,
        K::NumpadParenLeft => 182,
        K::NumpadParenRight => 183,
        K::NumpadBackspace => 187,
        K::NumpadHash => 204,
        K::NumpadMemoryStore => 208,
        K::NumpadMemoryRecall => 209,
        K::NumpadMemoryClear => 210,
        K::NumpadMemoryAdd => 211,
        K::NumpadMemorySubtract => 212,
        K::NumpadClear => 216,
        K::NumpadClearEntry => 217,
        K::ControlLeft => 224,
        K::ShiftLeft => 225,
        K::AltLeft => 226,
        K::SuperLeft => 227,
        K::ControlRight => 228,
        K::ShiftRight => 229,
        K::AltRight => 230,
        K::SuperRight => 231,
        K::Sleep => 258,
        K::WakeUp => 259,
        K::MediaTrackNext => 267,
        K::MediaTrackPrevious => 268,
        K::MediaStop => 269,
        K::Eject => 270,
        K::MediaPlayPause => 271,
        K::MediaSelect => 272,
        K::BrowserSearch => 280,
        K::BrowserHome => 281,
        K::BrowserBack => 282,
        K::BrowserForward => 283,
        K::BrowserStop => 284,
        K::BrowserRefresh => 285,
        K::BrowserFavorites => 286,
        _ => return None,
    })
}

/// All the winit keys that have a known position (for reading a name back).
pub const CODES: &[KeyCode] = {
    use KeyCode as K;
    &[
        K::KeyA,
        K::KeyB,
        K::KeyC,
        K::KeyD,
        K::KeyE,
        K::KeyF,
        K::KeyG,
        K::KeyH,
        K::KeyI,
        K::KeyJ,
        K::KeyK,
        K::KeyL,
        K::KeyM,
        K::KeyN,
        K::KeyO,
        K::KeyP,
        K::KeyQ,
        K::KeyR,
        K::KeyS,
        K::KeyT,
        K::KeyU,
        K::KeyV,
        K::KeyW,
        K::KeyX,
        K::KeyY,
        K::KeyZ,
        K::Digit1,
        K::Digit2,
        K::Digit3,
        K::Digit4,
        K::Digit5,
        K::Digit6,
        K::Digit7,
        K::Digit8,
        K::Digit9,
        K::Digit0,
        K::Enter,
        K::Escape,
        K::Backspace,
        K::Tab,
        K::Space,
        K::Minus,
        K::Equal,
        K::BracketLeft,
        K::BracketRight,
        K::Backslash,
        K::Semicolon,
        K::Quote,
        K::Backquote,
        K::Comma,
        K::Period,
        K::Slash,
        K::CapsLock,
        K::F1,
        K::F2,
        K::F3,
        K::F4,
        K::F5,
        K::F6,
        K::F7,
        K::F8,
        K::F9,
        K::F10,
        K::F11,
        K::F12,
        K::PrintScreen,
        K::ScrollLock,
        K::Pause,
        K::Insert,
        K::Home,
        K::PageUp,
        K::Delete,
        K::End,
        K::PageDown,
        K::ArrowRight,
        K::ArrowLeft,
        K::ArrowDown,
        K::ArrowUp,
        K::NumLock,
        K::NumpadDivide,
        K::NumpadMultiply,
        K::NumpadSubtract,
        K::NumpadAdd,
        K::NumpadEnter,
        K::Numpad1,
        K::Numpad2,
        K::Numpad3,
        K::Numpad4,
        K::Numpad5,
        K::Numpad6,
        K::Numpad7,
        K::Numpad8,
        K::Numpad9,
        K::Numpad0,
        K::NumpadDecimal,
        K::IntlBackslash,
        K::ContextMenu,
        K::Power,
        K::NumpadEqual,
        K::F13,
        K::F14,
        K::F15,
        K::F16,
        K::F17,
        K::F18,
        K::F19,
        K::F20,
        K::F21,
        K::F22,
        K::F23,
        K::F24,
        K::Help,
        K::Select,
        K::Again,
        K::Undo,
        K::Cut,
        K::Copy,
        K::Paste,
        K::Find,
        K::AudioVolumeMute,
        K::AudioVolumeUp,
        K::AudioVolumeDown,
        K::NumpadComma,
        K::IntlRo,
        K::KanaMode,
        K::IntlYen,
        K::Convert,
        K::NonConvert,
        K::Lang1,
        K::Lang2,
        K::Lang3,
        K::Lang4,
        K::Lang5,
        K::NumpadParenLeft,
        K::NumpadParenRight,
        K::NumpadBackspace,
        K::NumpadHash,
        K::NumpadMemoryStore,
        K::NumpadMemoryRecall,
        K::NumpadMemoryClear,
        K::NumpadMemoryAdd,
        K::NumpadMemorySubtract,
        K::NumpadClear,
        K::NumpadClearEntry,
        K::ControlLeft,
        K::ShiftLeft,
        K::AltLeft,
        K::SuperLeft,
        K::ControlRight,
        K::ShiftRight,
        K::AltRight,
        K::SuperRight,
        K::Sleep,
        K::WakeUp,
        K::MediaTrackNext,
        K::MediaTrackPrevious,
        K::MediaStop,
        K::Eject,
        K::MediaPlayPause,
        K::MediaSelect,
        K::BrowserSearch,
        K::BrowserHome,
        K::BrowserBack,
        K::BrowserForward,
        K::BrowserStop,
        K::BrowserRefresh,
        K::BrowserFavorites,
    ]
};

/// Name of a position (`key.keyboard.unknown` if the game does not name it).
fn name_of_position(sc: u16) -> &'static str {
    KEYS.iter().position(|&(_, v)| v == sc).map_or("key.keyboard.unknown", |i| full_names()[i].as_str())
}

/// Registered name of a physical key.
pub fn key_name(k: KeyCode) -> &'static str {
    position(k).map_or("key.keyboard.unknown", name_of_position)
}

/// Physical key of a registered name (`None` if unknown or with no equivalent in winit).
pub fn key_code(name: &str) -> Option<KeyCode> {
    let sc = position_of_name(name)?;
    CODES.iter().copied().find(|&k| position(k) == Some(sc))
}

/// Position of a name: game table, or `key.keyboard.<n>` for a position with no name of its own.
fn position_of_name(name: &str) -> Option<u16> {
    let rest = name.strip_prefix("key.keyboard.")?;
    if let Some(&(_, v)) = KEYS.iter().find(|(s, _)| *s == rest) {
        return (v != 0).then_some(v);
    }
    rest.parse::<u16>().ok().filter(|&v| v != 0)
}

/// Physical key of a position (inverse of `position`).
#[cfg(windows)]
fn code_of_position(sc: u16) -> Option<KeyCode> {
    CODES.iter().copied().find(|&k| position(k) == Some(sc))
}

/// Positions that produce a character (letters, digits, punctuation, Enter… and the ISO
/// key): only these are read in the layout; Delete (76) keeps its long name.
fn is_character_position(sc: u16) -> bool {
    (4..=56).contains(&sc) || sc == 100
}

/// Character of the US layout (fallback outside Windows or key absent from the keyboard).
fn default_char(sc: u16) -> u32 {
    const PUNCT: &[u8] = b"1234567890\r\x1b\x08\t -=[]\\#;'`,./";
    match sc {
        4..=29 => u32::from(b'a') + u32::from(sc - 4),
        30..=56 => u32::from(PUNCT[usize::from(sc - 30)]),
        100 => u32::from(b'\\'),
        _ => 0,
    }
}

/// Key code of a position in the current layout, without a modifier (`shift`
/// false) or with Shift. 0: no character; U+FFFD: several characters at once.
fn layout_char(sc: u16, shift: bool) -> u32 {
    #[cfg(windows)]
    if let Some(c) = code_of_position(sc).and_then(|k| win::char_of(k, shift)) {
        return c;
    }
    let c = default_char(sc);
    if shift && (u32::from(b'a')..=u32::from(b'z')).contains(&c) { c - 32 } else { c }
}

/// System name of a key with a character (a single letter); `None` if it is long.
fn system_char_name(sc: u16) -> Option<String> {
    let mut c = layout_char(sc, false);
    // Keys named by their function (Enter, Esc, Backspace, Tab, Space, Delete).
    if matches!(c, 0x0d | 0x1b | 0x08 | 0x09 | 0x20 | 0x7f) {
        return None;
    }
    // A letter is displayed by its capital when Shift gives it (first key that produces it).
    if c > 0x7f || (u32::from(b'a')..=u32::from(b'z')).contains(&c) {
        let first = (4..=56).chain([100]).find(|&s| layout_char(s, false) == c).unwrap_or(sc);
        let cap = layout_char(first, true);
        if cap > 0x7f || (u32::from(b'A')..=u32::from(b'Z')).contains(&cap) {
            c = cap;
        }
    }
    let ch = char::from_u32(c).filter(|_| c != 0)?;
    Some(ch.to_uppercase().collect())
}

/// Translation key of the long label (Windows: "windows" for the logo key).
fn translation_key(name: &str) -> &str {
    #[cfg(windows)]
    let over: &[(&str, &str)] = &[
        ("key.keyboard.left.win", "key.keyboard.left.windows"),
        ("key.keyboard.right.win", "key.keyboard.right.windows"),
    ];
    #[cfg(target_os = "macos")]
    let over: &[(&str, &str)] = &[
        ("key.keyboard.left.alt", "key.keyboard.left.option"),
        ("key.keyboard.right.alt", "key.keyboard.right.option"),
        ("key.keyboard.left.win", "key.keyboard.left.command"),
        ("key.keyboard.right.win", "key.keyboard.right.command"),
    ];
    #[cfg(target_os = "linux")]
    let over: &[(&str, &str)] =
        &[("key.keyboard.left.win", "key.keyboard.left.meta"), ("key.keyboard.right.win", "key.keyboard.right.meta")];
    #[cfg(not(any(windows, target_os = "macos", target_os = "linux")))]
    let over: &[(&str, &str)] = &[];
    over.iter().find(|(a, _)| *a == name).map_or(name, |(_, b)| b)
}

/// Label of a keyboard key: character of the layout if it is unique, otherwise the
/// translation key to look up in the language (`Err`).
pub fn keyboard_label(name: &str) -> Result<String, &str> {
    if name == "key.keyboard.unknown" {
        return Err(name);
    }
    match position_of_name(name).filter(|&sc| is_character_position(sc)) {
        Some(sc) => system_char_name(sc).ok_or_else(|| translation_key(name)),
        None => Err(translation_key(name)),
    }
}

/// Reading of the keyboard layout by the system.
#[cfg(windows)]
#[allow(unsafe_code)]
mod win {
    use std::sync::Mutex;
    use winit::keyboard::{KeyCode, PhysicalKey};
    use winit::platform::scancode::PhysicalKeyExtScancode;

    #[link(name = "user32")]
    unsafe extern "system" {
        fn GetKeyboardLayout(thread: u32) -> isize;
        fn MapVirtualKeyExW(code: u32, map: u32, hkl: isize) -> u32;
        fn ToUnicodeEx(vk: u32, sc: u32, state: *const u8, buf: *mut u16, len: i32, flags: u32, hkl: isize) -> i32;
    }

    /// Characters already read for the current layout: (layout, key, Shift) → code.
    type Entry = (KeyCode, bool, Option<u32>);
    static CACHE: Mutex<(isize, Vec<Entry>)> = Mutex::new((0, Vec::new()));

    /// Code of the key in the layout of the current thread; `None` if the key has no known
    /// scan code or if the system does not translate it to a virtual key.
    pub fn char_of(k: KeyCode, shift: bool) -> Option<u32> {
        // SAFETY: function with no side effect on an integer argument.
        let hkl = unsafe { GetKeyboardLayout(0) };
        let mut cache = CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if cache.0 != hkl {
            *cache = (hkl, Vec::new());
        }
        if let Some(&(_, _, c)) = cache.1.iter().find(|(kk, s, _)| *kk == k && *s == shift) {
            return c;
        }
        let c = read(k, shift, hkl);
        cache.1.push((k, shift, c));
        c
    }

    fn read(k: KeyCode, shift: bool, hkl: isize) -> Option<u32> {
        let sc = PhysicalKey::Code(k).to_scancode()?;
        // Extended code: 0xE0 prefix in the high byte, as the system expects.
        let sc = if sc & 0xff00 == 0xe000 { 0xe000 | (sc & 0xff) } else { sc & 0xff };
        // SAFETY: pure translation (MAPVK_VSC_TO_VK_EX = 3).
        let vk = unsafe { MapVirtualKeyExW(sc, 3, hkl) } & 0xff;
        if vk == 0 {
            return None;
        }
        let mut state = [0u8; 256];
        if shift {
            state[0x10] = 0x80;
        }
        let mut buf = [0u16; 16];
        // SAFETY: state of 256 bytes and buffer of 16 units, sizes passed; flag 4:
        // does not alter the state of the thread's dead keys.
        let r = unsafe { ToUnicodeEx(vk, sc, state.as_ptr(), buf.as_mut_ptr(), 16, 4, hkl) };
        let n = r.unsigned_abs().min(15) as usize;
        let mut it = char::decode_utf16(buf[..n].iter().copied());
        Some(match (it.next(), it.next()) {
            (None, _) => 0,
            (Some(Ok(c)), None) => u32::from(c),
            _ => 0xfffd,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_follow_positions() {
        assert_eq!(key_name(KeyCode::IntlBackslash), "key.keyboard.world.1");
        assert_eq!(key_name(KeyCode::ContextMenu), "key.keyboard.application");
        assert_eq!(key_name(KeyCode::F13), "key.keyboard.f13");
        assert_eq!(key_name(KeyCode::F24), "key.keyboard.f24");
        assert_eq!(key_name(KeyCode::NumpadEqual), "key.keyboard.keypad.equal");
        assert_eq!(key_name(KeyCode::MediaPlayPause), "key.keyboard.media.play.pause");
        assert_eq!(key_name(KeyCode::KeyW), "key.keyboard.w");
        assert_eq!(key_name(KeyCode::F25), "key.keyboard.unknown");
        assert_eq!(key_code("key.keyboard.world.1"), Some(KeyCode::IntlBackslash));
        assert_eq!(key_code("key.keyboard.world.2"), None);
        assert_eq!(key_code("key.keyboard.104"), Some(KeyCode::F13));
        assert_eq!(key_code("key.keyboard.unknown"), None);
        for &k in CODES {
            assert_ne!(key_name(k), "key.keyboard.unknown", "{k:?}");
            assert_eq!(key_code(key_name(k)), Some(k), "{k:?}");
        }
        assert_eq!(KEYS.len(), 247);
    }

    #[test]
    fn long_names_translate() {
        assert_eq!(keyboard_label("key.keyboard.unknown"), Err("key.keyboard.unknown"));
        assert_eq!(keyboard_label("key.keyboard.f1"), Err("key.keyboard.f1"));
        assert_eq!(keyboard_label("key.keyboard.space"), Err("key.keyboard.space"));
        assert_eq!(keyboard_label("key.keyboard.delete"), Err("key.keyboard.delete"));
        assert_eq!(keyboard_label("key.keyboard.keypad.1"), Err("key.keyboard.keypad.1"));
        #[cfg(windows)]
        assert_eq!(keyboard_label("key.keyboard.left.win"), Err("key.keyboard.left.windows"));
    }

    /// Activates an installed layout for the test thread; `false` if it is absent.
    #[cfg(windows)]
    #[allow(unsafe_code)]
    fn use_layout(id: &str, expect: isize) -> bool {
        #[link(name = "user32")]
        unsafe extern "system" {
            fn LoadKeyboardLayoutW(id: *const u16, flags: u32) -> isize;
            fn ActivateKeyboardLayout(hkl: isize, flags: u32) -> isize;
            fn GetKeyboardLayout(thread: u32) -> isize;
        }
        let w: Vec<u16> = id.encode_utf16().chain([0]).collect();
        // SAFETY: zero-terminated string; the activation only applies to the current thread.
        unsafe {
            let hkl = LoadKeyboardLayoutW(w.as_ptr(), 0);
            ActivateKeyboardLayout(hkl, 0);
            GetKeyboardLayout(0) & 0xffff_ffff == expect
        }
    }

    fn label(name: &str) -> String {
        keyboard_label(&format!("key.keyboard.{name}")).unwrap_or_else(|k| format!("<{k}>"))
    }

    #[cfg(windows)]
    #[test]
    fn azerty_labels() {
        if !use_layout("0000040C", 0x040c_040c) {
            return;
        }
        let got: Vec<String> =
            ["w", "a", "z", "q", "1", "2", "7", "9", "0", "semicolon", "world.1", "left.bracket", "grave.accent"]
                .iter()
                .map(|n| label(n))
                .collect();
        assert_eq!(got, ["Z", "Q", "W", "A", "&", "É", "È", "Ç", "À", "M", "<", "^", "²"]);
        assert_eq!(label("apostrophe"), "Ù");
        assert_eq!(label("world.2"), "#");
    }

    /// Comparison with the probe of the official game's library (`RUSTCRAFT_SDL_KEYS`:
    /// file "position, code, name"; `RUSTCRAFT_SDL_LAYOUT`: layout to activate).
    #[cfg(windows)]
    #[test]
    fn matches_official_probe() {
        let Some(path) = rc_data::env_os!("RUSTCRAFT_SDL_KEYS") else { return };
        if let Some(id) = rc_data::env_os!("RUSTCRAFT_SDL_LAYOUT") {
            let id = id.to_string_lossy().into_owned();
            let expect = i64::from_str_radix(&id, 16).expect("layout") as isize;
            assert!(use_layout(&format!("{expect:08X}"), expect | expect << 16), "layout {id} absent");
        }
        let text = std::fs::read_to_string(path).expect("read");
        let mut checked = 0;
        for line in text.lines() {
            let mut f = line.split('\t');
            let sc: u16 = f.next().and_then(|s| s.parse().ok()).expect("position");
            let sys_name = f.nth(1).unwrap_or("");
            let Some(&(suffix, _)) = KEYS.iter().find(|&&(_, v)| v == sc && v != 0) else { continue };
            let want = if sys_name.chars().count() == 1 { Some(sys_name.to_uppercase()) } else { None };
            let got = keyboard_label(&format!("key.keyboard.{suffix}")).ok();
            assert_eq!(got, want, "{suffix} ({sc})");
            checked += 1;
        }
        assert_eq!(checked, 246);
    }
}
